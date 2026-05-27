use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
pub struct SourceConfig {
    /// Local path to the git working tree.
    pub path: String,
    /// Remote origin URL (SSH or HTTPS). Required for cloning from scratch.
    pub origin: Option<String>,
    /// Branch to track. Defaults to "main".
    pub branch: Option<String>,
    /// Path to SSH private key (used with SSH origins).
    pub ssh_key: Option<String>,
    /// Bearer token for HTTPS origins (GitHub PAT, GitLab token, etc.).
    pub token: Option<String>,
    /// Pull this source automatically when `serve` starts.
    #[serde(default)]
    pub update_on_serve: bool,
}

// ── Update ────────────────────────────────────────────────────────────────────

/// Pull or clone a single source. Returns a human-readable status line.
pub fn update_source(src: &SourceConfig) -> Result<String> {
    let path = Path::new(&src.path);
    let branch = src.branch.as_deref().unwrap_or("main");

    if path.join(".git").exists() {
        git_fetch_reset(path, branch, src)?;
        Ok(format!("{} (reset to origin/{branch})", src.path))
    } else if let Some(origin) = &src.origin {
        git_clone(origin, path, branch, src)
            .with_context(|| format!("git clone {} failed", origin))?;
        Ok(format!("Cloned {} → {}", origin, src.path))
    } else {
        bail!("{}: not a git repo and no origin configured", src.path)
    }
}

/// Update all sources. Returns per-path results (ok or error).
pub fn update_all(sources: &[SourceConfig]) -> Vec<(String, Result<String>)> {
    sources
        .iter()
        .map(|s| (s.path.clone(), update_source(s)))
        .collect()
}

// ── File walker ───────────────────────────────────────────────────────────────

const SKIP_DIRS: &[&str] = &[".git", "__pycache__", "node_modules", ".idea", ".vscode"];

/// Recursively collect all `.py` files under `dir`.
fn walk_py_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name) && !name.starts_with('.') {
                walk_py_files(&path, out);
            }
        } else if name.ends_with(".py") {
            out.push(path);
        }
    }
}

fn collect_py_files(sources: &[SourceConfig]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for src in sources {
        walk_py_files(Path::new(&src.path), &mut files);
    }
    files
}

// ── Model source lookup ───────────────────────────────────────────────────────

/// Return Python source files that define or inherit `model`.
pub fn find_model_source(model: &str, sources: &[SourceConfig]) -> Result<String> {
    if sources.is_empty() {
        bail!("No sources configured for this profile. Add `sources:` to config.yaml.");
    }

    let dq = format!("\"{model}\"");
    let sq = format!("'{model}'");

    let mut out = String::new();
    let mut count = 0usize;

    for file in collect_py_files(sources) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Fast pre-filter before the detailed check.
        if !content.contains(&dq) && !content.contains(&sq) {
            continue;
        }
        if !is_model_definition(&content, model) {
            continue;
        }
        if count > 0 {
            out.push_str("\n\n# ");
            out.push_str(&"─".repeat(72));
            out.push('\n');
        }
        out.push_str(&format!("# {}\n\n", file.display()));
        out.push_str(&content);
        count += 1;
    }

    if count == 0 {
        bail!("No Odoo model class found for '{model}'");
    }
    Ok(out)
}

/// Returns true if `content` is an Odoo model file defining or inheriting `model`.
fn is_model_definition(content: &str, model: &str) -> bool {
    let dq = format!("\"{model}\"");
    let sq = format!("'{model}'");

    let references_model = content.contains(&format!("_name = {dq}"))
        || content.contains(&format!("_name = {sq}"))
        || content.contains(&format!("_inherit = {dq}"))
        || content.contains(&format!("_inherit = {sq}"))
        || (content.contains("_inherit")
            && (content.contains(&dq) || content.contains(&sq)));

    let is_model_class = content.contains("models.Model")
        || content.contains("models.TransientModel")
        || content.contains("models.AbstractModel");

    references_model && is_model_class
}

// ── General source search ─────────────────────────────────────────────────────

/// Search for `query` (case-sensitive substring) across all Python source files.
///
/// `path_filter` — optional substring that the file path must contain
///   (e.g. "gt_billing" to restrict to that addon).
/// `context` — number of lines before/after each matching line.
/// Returns at most `max_matches` results.
pub fn search_source(
    query: &str,
    path_filter: Option<&str>,
    context: usize,
    max_matches: usize,
    sources: &[SourceConfig],
) -> Result<String> {
    if sources.is_empty() {
        bail!("No sources configured for this profile. Add `sources:` to config.yaml.");
    }

    let files = collect_py_files(sources);

    let mut out = String::new();
    let mut total = 0usize;
    let mut truncated = false;

    'files: for file in &files {
        // Apply optional path filter.
        if let Some(filter) = path_filter {
            if !file.to_str().map(|s| s.contains(filter)).unwrap_or(false) {
                continue;
            }
        }

        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        if !content.contains(query) {
            continue;
        }

        let lines: Vec<&str> = content.lines().collect();
        let mut file_written = false;

        for (i, line) in lines.iter().enumerate() {
            if !line.contains(query) {
                continue;
            }
            if total >= max_matches {
                truncated = true;
                break 'files;
            }

            if !file_written {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&format!("# {}\n", file.display()));
                file_written = true;
            }

            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(lines.len());

            out.push_str(&format!("  ── line {} ──\n", i + 1));
            for (j, l) in lines[start..end].iter().enumerate() {
                let lineno = start + j + 1;
                let marker = if start + j == i { '>' } else { ' ' };
                out.push_str(&format!("{marker} {lineno:5}: {l}\n"));
            }

            total += 1;
        }
    }

    if out.is_empty() {
        bail!("No matches found for '{query}'");
    }
    if truncated {
        out.push_str(&format!(
            "\n\n[Truncated at {max_matches} matches — use path_filter to narrow the search]"
        ));
    }
    Ok(out)
}

// ── Addon listing ────────────────────────────────────────────────────────────

/// Find all `__manifest__.py` files across source trees.
fn find_manifests(sources: &[SourceConfig]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for src in sources {
        find_manifests_in(Path::new(&src.path), &mut out);
    }
    out
}

fn find_manifests_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !path.is_dir() || SKIP_DIRS.contains(&name) || name.starts_with('.') {
            continue;
        }
        let manifest = path.join("__manifest__.py");
        if manifest.exists() {
            out.push(manifest);
        } else {
            find_manifests_in(&path, out);
        }
    }
}

/// Extract a single string value for `key` from manifest Python source.
fn manifest_str(content: &str, key: &str) -> String {
    for q in ['"', '\''] {
        let needle = format!("{q}{key}{q}");
        let Some(pos) = content.find(&needle) else { continue };
        let rest = content[pos + needle.len()..].trim_start();
        let Some(colon) = rest.find(':') else { continue };
        let val = rest[colon + 1..].trim_start();
        for vq in ['"', '\''] {
            if val.starts_with(vq) {
                let inner = &val[1..];
                if let Some(end) = inner.find(vq) {
                    return inner[..end].to_string();
                }
            }
        }
    }
    String::new()
}

/// Extract a list of strings for `key` from manifest Python source.
fn manifest_list(content: &str, key: &str) -> Vec<String> {
    for q in ['"', '\''] {
        let needle = format!("{q}{key}{q}");
        let Some(pos) = content.find(&needle) else { continue };
        let rest = &content[pos + needle.len()..];
        let Some(bracket) = rest.find('[') else { continue };
        let rest = &rest[bracket + 1..];
        let Some(end) = rest.find(']') else { continue };
        return extract_quoted_list(&rest[..end]);
    }
    Vec::new()
}

fn extract_quoted_list(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = s;
    while let Some(pos) = rest.find(|c| c == '\'' || c == '"') {
        let q = rest.chars().nth(pos).unwrap();
        let inner = &rest[pos + 1..];
        if let Some(end) = inner.find(q) {
            let item = inner[..end].trim().to_string();
            if !item.is_empty() {
                result.push(item);
            }
            rest = &inner[end + 1..];
        } else {
            break;
        }
    }
    result
}

/// Return a summary of all Odoo addons found across all source trees.
pub fn list_addons(sources: &[SourceConfig]) -> Result<String> {
    if sources.is_empty() {
        bail!("No sources configured for this profile. Add `sources:` to config.yaml.");
    }

    let manifests = find_manifests(sources);
    if manifests.is_empty() {
        bail!("No __manifest__.py files found in configured source paths.");
    }

    let mut out = format!("Found {} addons:\n\n", manifests.len());

    for manifest_path in &manifests {
        let addon_dir = manifest_path.parent().unwrap();
        let addon_name = addon_dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let Ok(content) = std::fs::read_to_string(manifest_path) else { continue };

        let name    = manifest_str(&content, "name");
        let version = manifest_str(&content, "version");
        let summary = {
            let s = manifest_str(&content, "summary");
            if s.is_empty() { manifest_str(&content, "description") } else { s }
        };
        let depends = manifest_list(&content, "depends");

        out.push_str(&format!("## {addon_name}"));
        if !name.is_empty() && name != addon_name {
            out.push_str(&format!(" ({name})"));
        }
        out.push('\n');
        if !version.is_empty() {
            out.push_str(&format!("  version:  {version}\n"));
        }
        if !summary.is_empty() {
            out.push_str(&format!("  summary:  {summary}\n"));
        }
        if !depends.is_empty() {
            out.push_str(&format!("  depends:  {}\n", depends.join(", ")));
        }
        out.push_str(&format!("  path:     {}\n\n", addon_dir.display()));
    }

    Ok(out)
}

// ── Addon structure ───────────────────────────────────────────────────────────

pub fn addon_structure(addon_name: &str, sources: &[SourceConfig]) -> Result<String> {
    if sources.is_empty() {
        bail!("No sources configured for this profile. Add `sources:` to config.yaml.");
    }

    // Find the addon directory by name.
    let addon_dir = find_manifests(sources)
        .into_iter()
        .find(|m| {
            m.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n == addon_name)
                .unwrap_or(false)
        })
        .and_then(|m| m.parent().map(|p| p.to_path_buf()))
        .with_context(|| format!("Addon '{addon_name}' not found in configured source paths"))?;

    // Parse manifest.
    let manifest_content = std::fs::read_to_string(addon_dir.join("__manifest__.py"))
        .unwrap_or_default();
    let name    = manifest_str(&manifest_content, "name");
    let version = manifest_str(&manifest_content, "version");
    let summary = manifest_str(&manifest_content, "summary");
    let depends = manifest_list(&manifest_content, "depends");
    let data    = manifest_list(&manifest_content, "data");

    let mut out = String::new();
    out.push_str(&format!("# {addon_name}"));
    if !name.is_empty() {
        out.push_str(&format!(" — {name}"));
    }
    out.push('\n');
    if !version.is_empty() {
        out.push_str(&format!("version: {version}\n"));
    }
    if !summary.is_empty() {
        out.push_str(&format!("summary: {summary}\n"));
    }
    if !depends.is_empty() {
        out.push_str(&format!("depends: {}\n", depends.join(", ")));
    }

    // Scan Python files.
    let mut py_files = Vec::new();
    walk_py_files(&addon_dir, &mut py_files);

    let mut models_defined:   Vec<String> = Vec::new();
    let mut models_inherited: Vec<String> = Vec::new();
    let mut controllers:      Vec<String> = Vec::new();

    for file in &py_files {
        let Ok(content) = std::fs::read_to_string(file) else { continue };
        let rel = file.strip_prefix(&addon_dir).unwrap_or(file);

        scan_models(&content, rel, &mut models_defined, &mut models_inherited);
        scan_routes(&content, rel, &mut controllers);
    }

    // Models section.
    if !models_defined.is_empty() {
        out.push_str("\n## Models defined\n");
        for m in &models_defined {
            out.push_str(&format!("  {m}\n"));
        }
    }
    if !models_inherited.is_empty() {
        out.push_str("\n## Models inherited / extended\n");
        // Deduplicate — same model may be extended in multiple files.
        let mut seen = std::collections::HashSet::new();
        for m in &models_inherited {
            if seen.insert(m.clone()) {
                out.push_str(&format!("  {m}\n"));
            }
        }
    }

    // Controllers section.
    if !controllers.is_empty() {
        out.push_str("\n## Controllers (HTTP routes)\n");
        for c in &controllers {
            out.push_str(&format!("  {c}\n"));
        }
    }

    // Data / XML files.
    if !data.is_empty() {
        out.push_str("\n## Data files\n");
        for d in &data {
            out.push_str(&format!("  {d}\n"));
        }
    }

    // Security files.
    let security_dir = addon_dir.join("security");
    if security_dir.is_dir() {
        out.push_str("\n## Security\n");
        if let Ok(entries) = std::fs::read_dir(&security_dir) {
            for e in entries.flatten() {
                let n = e.file_name();
                out.push_str(&format!("  security/{}\n", n.to_string_lossy()));
            }
        }
    }

    Ok(out)
}

/// Scan a Python file for model class definitions and inheritances.
fn scan_models(
    content: &str,
    rel_path: &Path,
    defined: &mut Vec<String>,
    inherited: &mut Vec<String>,
) {
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();

        if t.starts_with("_name") && t.contains('=') {
            if let Some(model_name) = extract_py_str_value(t) {
                // Look back up to 15 lines for the class definition.
                let class = find_class_above(&lines, i);
                let kind  = if content.contains("TransientModel") { "Transient" } else { "Model" };
                let entry = if let Some(cls) = class {
                    format!("{model_name}  ({cls})  [{kind}]  — {}:{}", rel_path.display(), i + 1)
                } else {
                    format!("{model_name}  [{kind}]  — {}:{}", rel_path.display(), i + 1)
                };
                defined.push(entry);
            }
        }

        if t.starts_with("_inherit") && t.contains('=') {
            let after = t.splitn(2, '=').nth(1).unwrap_or("").trim();
            let names: Vec<String> = if after.starts_with('[') {
                extract_quoted_list(after)
            } else {
                extract_py_str_value(t).into_iter().collect()
            };
            for name in names {
                inherited.push(format!(
                    "{name}  — {}:{}",
                    rel_path.display(),
                    i + 1
                ));
            }
        }
    }
}

/// Scan a Python file for @http.route decorators.
fn scan_routes(content: &str, rel_path: &Path, routes: &mut Vec<String>) {
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("@http.route") && !line.contains("route(") {
            continue;
        }
        // Collect the route arguments (may span a couple lines).
        let mut snippet = line.trim().to_string();
        for extra in lines.iter().skip(i + 1).take(3) {
            snippet.push(' ');
            snippet.push_str(extra.trim());
            if snippet.contains(')') {
                break;
            }
        }
        // Extract path strings from the route call.
        let paths = extract_quoted_list(&snippet);
        let entry = if paths.is_empty() {
            format!("(route)  — {}:{}", rel_path.display(), i + 1)
        } else {
            format!("{}  — {}:{}", paths.join(", "), rel_path.display(), i + 1)
        };
        routes.push(entry);
    }
}

/// Look backwards from line `i` to find a `class Foo(...)` definition.
fn find_class_above(lines: &[&str], i: usize) -> Option<String> {
    for j in (0..i.min(i + 1)).rev().take(20) {
        let t = lines[j].trim();
        if t.starts_with("class ") {
            return Some(
                t.trim_start_matches("class ")
                    .split(|c| c == '(' || c == ':')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

/// Extract a Python string value from an assignment like `_name = 'foo.bar'`.
fn extract_py_str_value(s: &str) -> Option<String> {
    let after = s.splitn(2, '=').nth(1)?.trim();
    for q in ['"', '\''] {
        if after.starts_with(q) {
            let inner = &after[1..];
            if let Some(end) = inner.find(q) {
                return Some(inner[..end].to_string());
            }
        }
    }
    None
}

// ── git helpers ───────────────────────────────────────────────────────────────

/// Build the list of in-memory config overrides for auth.
///
/// * SSH key  → `core.sshCommand=ssh -i <key> -o StrictHostKeyChecking=no -o BatchMode=yes`
/// * HTTPS token → `http.extraHeader=Authorization: Bearer <token>`
fn auth_config_overrides(src: &SourceConfig) -> Vec<String> {
    let mut overrides = Vec::new();
    if let Some(key) = &src.ssh_key {
        overrides.push(format!(
            "core.sshCommand=ssh -i {key} -o StrictHostKeyChecking=no -o BatchMode=yes"
        ));
    }
    if let Some(token) = &src.token {
        overrides.push(format!(
            "http.extraHeader=Authorization: Bearer {token}"
        ));
    }
    overrides
}

/// Clone `origin` into `path`, checking out `branch`.
fn git_clone(origin: &str, path: &Path, branch: &str, src: &SourceConfig) -> Result<()> {
    use gix::create;

    let interrupt = AtomicBool::new(false);
    let overrides = auth_config_overrides(src);

    let mut prepare = gix::clone::PrepareFetch::new(
        origin,
        path,
        create::Kind::WithWorktree,
        create::Options::default(),
        gix::open::Options::default(),
    )
    .with_context(|| format!("Failed to initialise clone of {origin}"))?
    .with_in_memory_config_overrides(overrides)
    .with_ref_name(Some(branch))
    .with_context(|| format!("Invalid branch name: {branch}"))?;

    let (mut checkout, _fetch_outcome) = prepare
        .fetch_then_checkout(gix::progress::Discard, &interrupt)
        .with_context(|| format!("Fetch from {origin} failed"))?;

    let (_repo, _checkout_outcome) = checkout
        .main_worktree(gix::progress::Discard, &interrupt)
        .context("Worktree checkout failed after clone")?;

    Ok(())
}

/// Fetch from origin and hard-reset the local `branch` to `origin/<branch>`.
///
/// Equivalent to:
///   git fetch origin
///   git checkout <branch>
///   git reset --hard origin/<branch>
fn git_fetch_reset(path: &Path, branch: &str, src: &SourceConfig) -> Result<()> {
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    use gix::remote::Direction;

    let interrupt = AtomicBool::new(false);
    let overrides = auth_config_overrides(src);

    // Open the repository, injecting auth overrides via the API config layer.
    let mut repo = gix::open_opts(
        path,
        gix::open::Options::default().config_overrides(overrides),
    )
    .with_context(|| format!("Failed to open git repo at {}", path.display()))?;

    // Fetch from origin.
    {
        let remote = repo
            .find_remote("origin")
            .context("No remote named 'origin' found")?;

        let connection = remote
            .connect(Direction::Fetch)
            .context("Failed to connect to origin")?;

        let fetch_prepare = connection
            .prepare_fetch(gix::progress::Discard, Default::default())
            .context("Failed to prepare fetch")?;

        fetch_prepare
            .receive(gix::progress::Discard, &interrupt)
            .context("Fetch from origin failed")?;
    }

    // Reload the repo so the new remote-tracking refs are visible.
    repo = gix::open_opts(path, gix::open::Options::default())
        .context("Failed to re-open repo after fetch")?;

    // Resolve refs/remotes/origin/<branch> to an OID.
    let remote_ref_name = format!("refs/remotes/origin/{branch}");
    let target_oid = repo
        .find_reference(remote_ref_name.as_str())
        .with_context(|| format!("Remote tracking ref {remote_ref_name} not found after fetch"))?
        .into_fully_peeled_id()
        .with_context(|| format!("Failed to peel {remote_ref_name} to commit"))?
        .detach();

    // Update refs/heads/<branch> to the fetched OID.
    let local_ref_name: gix::refs::FullName = format!("refs/heads/{branch}")
        .try_into()
        .with_context(|| format!("Invalid ref name refs/heads/{branch}"))?;

    repo.edit_reference(RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: format!("reset: moving to origin/{branch}").into(),
            },
            expected: PreviousValue::Any,
            new: gix::refs::Target::Object(target_oid),
        },
        name: local_ref_name,
        deref: false,
    })
    .with_context(|| format!("Failed to update refs/heads/{branch}"))?;

    // Hard-reset the working tree: rebuild index from the target tree and checkout.
    let workdir = repo
        .workdir()
        .with_context(|| format!("Repo at {} has no workdir", path.display()))?
        .to_owned();

    // Peel the commit to its root tree.
    let tree_id = repo
        .find_object(target_oid)
        .context("Failed to find target commit object")?
        .peel_to_tree()
        .context("Failed to peel commit to tree")?
        .id;

    // Build an in-memory index from the tree, then check out the working tree.
    let mut index = repo
        .index_from_tree(&tree_id)
        .context("Failed to build index from tree")?;

    let mut opts = repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .context("Failed to get checkout options")?;
    // Overwrite existing files — this is the "hard" part of reset --hard.
    opts.overwrite_existing = true;

    let discard = gix::progress::Discard;
    gix::worktree::state::checkout(
        &mut index,
        &workdir,
        repo.objects.clone().into_arc()?,
        &discard,
        &discard,
        &interrupt,
        opts,
    )
    .context("Worktree checkout failed during hard reset")?;

    index.write(Default::default()).context("Failed to write updated index")?;

    Ok(())
}
