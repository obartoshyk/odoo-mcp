use std::path::Path;
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
        run_git(Some(path), &["fetch", "origin"], src)
            .context("git fetch failed")?;
        run_git(Some(path), &["checkout", branch], src)
            .context("git checkout failed")?;
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

// ── Model source lookup ───────────────────────────────────────────────────────

/// Find and return Python source files that define or inherit `model`.
///
/// Searches `_name = "model"`, `_name = 'model'`, `_inherit = "model"`,
/// `_inherit = 'model'`, and list-style `_inherit = ["model", ...]` across
/// all configured source paths.
pub fn find_model_source(model: &str, sources: &[SourceConfig]) -> Result<String> {
    if sources.is_empty() {
        bail!("No sources configured for this profile. Add `sources:` to config.yaml.");
    }

    let dq = format!("\"{model}\"");
    let sq = format!("'{model}'");
    let source_paths: Vec<&str> = sources.iter().map(|s| s.path.as_str()).collect();

    // Grep across all source trees for either quote style.
    let mut cmd = Command::new("grep");
    cmd.args(["-rl", "--include=*.py", "-e", &dq, "-e", &sq]);
    cmd.args(&source_paths);

    let output = cmd.output().context("grep failed — is grep installed?")?;
    let files: Vec<&str> = std::str::from_utf8(&output.stdout)
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .collect();

    if files.is_empty() {
        bail!("No Python files found referencing model '{model}'");
    }

    let mut out = String::new();
    let mut count = 0usize;

    for file in &files {
        let content = std::fs::read_to_string(file)
            .with_context(|| format!("Cannot read {file}"))?;

        if !is_model_definition(&content, model) {
            continue;
        }

        if count > 0 {
            out.push_str("\n\n# ");
            out.push_str(&"─".repeat(72));
            out.push('\n');
        }
        out.push_str(&format!("# {file}\n\n"));
        out.push_str(&content);
        count += 1;
    }

    if count == 0 {
        bail!("No Odoo model class found for '{model}' (files matched but none define/inherit it)");
    }

    Ok(out)
}

/// Returns true if `content` is an Odoo model file that defines or inherits `model`.
fn is_model_definition(content: &str, model: &str) -> bool {
    let dq = format!("\"{model}\"");
    let sq = format!("'{model}'");

    let references_model = content.contains(&format!("_name = {dq}"))
        || content.contains(&format!("_name = {sq}"))
        || content.contains(&format!("_inherit = {dq}"))
        || content.contains(&format!("_inherit = {sq}"))
        // list-style _inherit = ["model", ...] or _inherit = ['model', ...]
        || (content.contains("_inherit")
            && (content.contains(&dq) || content.contains(&sq)));

    let is_model_class = content.contains("models.Model")
        || content.contains("models.TransientModel")
        || content.contains("models.AbstractModel");

    references_model && is_model_class
}

// ── git helpers ───────────────────────────────────────────────────────────────

fn run_git(work_dir: Option<&Path>, sub_args: &[&str], src: &SourceConfig) -> Result<()> {
    let mut cmd = Command::new("git");

    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }

    // -c options must come before the subcommand.
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
