use std::path::{Path, PathBuf};
use std::process::Command;

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
        run_git(Some(path), &["fetch", "origin"], src).context("git fetch failed")?;
        run_git(Some(path), &["checkout", branch], src).context("git checkout failed")?;
        run_git(
            Some(path),
            &["reset", "--hard", &format!("origin/{branch}")],
            src,
        )
        .context("git reset failed")?;
        Ok(format!("{} (reset to origin/{branch})", src.path))
    } else if let Some(origin) = &src.origin {
        let origin = origin.as_str();
        let path_str = src.path.as_str();
        let mut args: Vec<&str> = vec!["clone", "--single-branch", "--branch", branch];
        args.extend_from_slice(&[origin, path_str]);
        run_git(None, &args, src).context("git clone failed")?;
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

// ── git helpers ───────────────────────────────────────────────────────────────

fn run_git(work_dir: Option<&Path>, sub_args: &[&str], src: &SourceConfig) -> Result<()> {
    let mut cmd = Command::new("git");

    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }
    if let Some(token) = &src.token {
        cmd.arg("-c")
            .arg(format!("http.extraHeader=Authorization: Bearer {token}"));
    }
    cmd.args(sub_args);
    if let Some(key) = &src.ssh_key {
        cmd.env(
            "GIT_SSH_COMMAND",
            format!("ssh -i {key} -o StrictHostKeyChecking=no -o BatchMode=yes"),
        );
    }

    let out = cmd
        .output()
        .with_context(|| format!("Failed to run: git {}", sub_args.join(" ")))?;

    if !out.status.success() {
        bail!(
            "git {} failed:\n{}",
            sub_args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
