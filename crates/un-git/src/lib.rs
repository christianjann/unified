use anyhow::{Context, Result};
use glob::Pattern;
use indicatif::ProgressBar;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use un_cache::Cache;
use un_core::GitReference;

#[derive(Debug, Clone)]
pub enum CheckoutMode {
    Worktree,
    Copy,
    SparseWorktree {
        includes: Vec<String>,
        excludes: Vec<String>,
    },
    FilteredCopy {
        includes: Vec<String>,
        excludes: Vec<String>,
    },
}

pub struct GitRemote {
    url: String,
}

impl GitRemote {
    pub fn new(url: &str) -> Self {
        let normalized = normalize_url(url);
        GitRemote { url: normalized }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn refspecs_for(&self, reference: &GitReference) -> Vec<String> {
        match reference {
            GitReference::Branch(name) => vec![format!("+refs/heads/{name}:refs/heads/{name}")],
            GitReference::Tag(name) => vec![format!("+refs/tags/{name}:refs/tags/{name}")],
            GitReference::Rev(_) => vec![], // No fetch needed for rev
            GitReference::DefaultBranch => vec!["+HEAD:refs/remotes/origin/HEAD".to_string()],
        }
    }
}

fn normalize_url(url: &str) -> String {
    let mut url = url.trim_end_matches('/').to_string();
    if url.starts_with("file://") {
        // Handle file:// URLs - resolve relative paths
        let path_part = &url[7..]; // Remove "file://"
        if path_part.starts_with("./")
            || (!path_part.starts_with('/') && !path_part.contains("://"))
        {
            // Relative path - resolve relative to current directory
            if let Ok(cwd) = std::env::current_dir() {
                let abs_path = cwd.join(path_part);
                if let Ok(abs_path) = abs_path.canonicalize() {
                    url = format!("file://{}", abs_path.display());
                }
            }
        }
    } else if url.contains('@') && !url.starts_with("ssh://") && !url.starts_with("https://") {
        // SCP style: git@github.com:user/repo.git -> ssh://git@github.com/user/repo.git
        if let Some(colon) = url.find(':') {
            let host = &url[..colon];
            let path = &url[colon + 1..];
            url = format!("ssh://{}/{}", host, path);
        }
    }
    url
}

pub struct GitDatabase {
    path: PathBuf,
}

impl GitDatabase {
    pub fn new(cache: &Cache, name: &str, url: &str) -> Result<Self> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();
        let short_hash = format!("{:x}", hash).chars().take(16).collect::<String>();
        let path = cache.git_db().join(format!("{}-{}", name, short_hash));
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
            let status = std::process::Command::new("git")
                .args(["init", "--bare"])
                .current_dir(&path)
                .status()
                .context("failed to run `git init --bare` — is git installed?")?;
            if !status.success() {
                anyhow::bail!("git init --bare failed in {}", path.display());
            }
            let status = std::process::Command::new("git")
                .args(["remote", "add", "origin", url])
                .current_dir(&path)
                .status()
                .context("failed to run `git remote add`")?;
            if !status.success() {
                anyhow::bail!("git remote add origin failed in {}", path.display());
            }
        }
        Ok(GitDatabase { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Prune stale worktree registrations (e.g., after a directory was manually deleted).
    pub fn prune_worktrees(&self) -> Result<()> {
        let status = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to run `git worktree prune`")?;
        if !status.success() {
            anyhow::bail!("git worktree prune failed in {}", self.path.display());
        }
        Ok(())
    }

    pub fn fetch(
        &self,
        remote: &GitRemote,
        reference: &GitReference,
        shallow: bool,
        _use_cli: bool,
        pb: Option<&ProgressBar>,
    ) -> Result<String> {
        // Always use CLI for now (gix integration deferred)
        self.fetch_with_cli(remote, reference, shallow, pb)?;
        self.resolve_oid(reference)
    }

    fn fetch_with_cli(
        &self,
        remote: &GitRemote,
        reference: &GitReference,
        shallow: bool,
        pb: Option<&ProgressBar>,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("fetch").arg("--progress").arg(remote.url());
        if shallow {
            cmd.args(["--depth", "1"]);
        }
        for refspec in &remote.refspecs_for(reference) {
            cmd.arg(refspec);
        }

        // If we have a progress bar, capture stderr and parse git progress.
        // Git outputs progress on stderr.
        if let Some(pb) = pb {
            cmd.current_dir(&self.path)
                .stdout(Stdio::null())
                .stderr(Stdio::piped());

            let mut child = cmd.spawn()
                .context("failed to run `git fetch` — is git installed?")?;

            let stderr = child.stderr.take().unwrap();
            let reader = BufReader::new(stderr);

            // Git progress lines use \r to overwrite. Read byte-by-byte to handle \r.
            let mut line_buf = String::new();
            let mut bytes_reader = reader;
            let mut byte = [0u8; 1];
            loop {
                use std::io::Read;
                match bytes_reader.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) => {
                        if byte[0] == b'\r' || byte[0] == b'\n' {
                            if !line_buf.is_empty() {
                                parse_git_progress(&line_buf, pb);
                                line_buf.clear();
                            }
                        } else {
                            line_buf.push(byte[0] as char);
                        }
                    }
                    Err(_) => break,
                }
            }
            if !line_buf.is_empty() {
                parse_git_progress(&line_buf, pb);
            }

            let status = child.wait()
                .context("failed to wait for `git fetch`")?;
            if !status.success() {
                anyhow::bail!("git fetch failed for {}", remote.url());
            }
        } else {
            let status = cmd
                .current_dir(&self.path)
                .status()
                .context("failed to run `git fetch` — is git installed?")?;
            if !status.success() {
                anyhow::bail!("git fetch failed for {}", remote.url());
            }
        }
        Ok(())
    }

    fn resolve_oid(&self, reference: &GitReference) -> Result<String> {
        let ref_str = match reference {
            GitReference::Branch(name) => format!("refs/heads/{}", name),
            GitReference::Tag(name) => format!("refs/tags/{}", name),
            GitReference::Rev(rev) => rev.to_string(),
            GitReference::DefaultBranch => "refs/remotes/origin/HEAD".to_string(),
        };
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--verify", &ref_str])
            .current_dir(&self.path)
            .output()
            .context("failed to run `git rev-parse`")?;
        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?.trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "failed to resolve ref '{}': {}",
                ref_str,
                stderr.trim()
            ))
        }
    }
}

/// Parse a git progress line and update the progress bar.
/// Git progress looks like:
///   "remote: Enumerating objects: 42, done."
///   "remote: Counting objects:  50% (21/42)"
///   "remote: Compressing objects: 100% (15/15), done."
///   "Receiving objects:  33% (14/42), 1.20 MiB | 640.00 KiB/s"
///   "Resolving deltas: 100% (8/8), done."
fn parse_git_progress(line: &str, pb: &ProgressBar) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    // Strip "remote: " prefix if present
    let text = line.strip_prefix("remote: ").unwrap_or(line);

    // Try to extract phase and percentage: "Phase:  XX% (n/total)"
    if let Some(pct_pos) = text.find('%') {
        // Walk back to find the start of the number
        let before_pct = &text[..pct_pos];
        let num_start = before_pct
            .rfind(|c: char| !c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(0);
        if let Ok(pct) = before_pct[num_start..].parse::<u64>() {
            // Extract phase name (before the colon)
            let phase = text.split(':').next().unwrap_or(text).trim();

            // Try to extract total from (n/total)
            if let Some(slash_pos) = text.find('/') {
                let after_slash = &text[slash_pos + 1..];
                let total_end = after_slash
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after_slash.len());
                if let Ok(total) = after_slash[..total_end].parse::<u64>() {
                    if pb.length().unwrap_or(0) != total {
                        pb.set_length(total);
                    }
                    let pos = total * pct / 100;
                    pb.set_position(pos);
                }
            }

            // Extract speed info if present (e.g. "| 640.00 KiB/s")
            let speed_info = text
                .find('|')
                .map(|i| text[i + 1..].trim())
                .unwrap_or("");

            if speed_info.is_empty() {
                pb.set_message(format!("{} {}%", phase, pct));
            } else {
                pb.set_message(format!("{} {}% ({})", phase, pct, speed_info));
            }
        }
    } else if text.contains("done") {
        // Phase completed
        let phase = text.split(':').next().unwrap_or(text).trim();
        pb.set_message(format!("{} done", phase));
    }
}

pub struct GitCheckout {
    path: PathBuf,
}

impl GitCheckout {
    pub fn new(
        database: &GitDatabase,
        oid: &str,
        workspace_path: &Path,
        mode: CheckoutMode,
    ) -> Result<Self> {
        match mode {
            CheckoutMode::Worktree => {
                // Prune stale worktree registrations before adding
                database.prune_worktrees()?;
                std::fs::create_dir_all(workspace_path.parent().unwrap_or(workspace_path))?;
                let status = std::process::Command::new("git")
                    .args(["worktree", "add", &workspace_path.to_string_lossy(), oid])
                    .current_dir(&database.path)
                    .status()
                    .context("failed to run `git worktree add`")?;
                if !status.success() {
                    anyhow::bail!("git worktree add failed for {}", workspace_path.display());
                }
            }
            CheckoutMode::Copy => {
                Self::checkout_copy(database, oid, workspace_path)?;
            }
            CheckoutMode::SparseWorktree { includes, excludes } => {
                Self::checkout_sparse_worktree(database, oid, workspace_path, includes, excludes)?;
            }
            CheckoutMode::FilteredCopy { includes, excludes } => {
                Self::checkout_filtered_copy(database, oid, workspace_path, includes, excludes)?;
            }
        }
        Ok(GitCheckout {
            path: workspace_path.to_path_buf(),
        })
    }

    fn checkout_copy(database: &GitDatabase, oid: &str, workspace_path: &Path) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path();
        let status = std::process::Command::new("git")
            .args([
                "--work-tree",
                &temp_path.to_string_lossy(),
                "checkout",
                oid,
                "--",
                ".",
            ])
            .current_dir(&database.path)
            .status()
            .context("failed to run `git checkout` for copy mode")?;
        if !status.success() {
            anyhow::bail!("git checkout failed for oid {}", oid);
        }
        std::fs::create_dir_all(workspace_path)?;
        Self::copy_recursive(temp_path, workspace_path)?;
        Ok(())
    }

    fn checkout_sparse_worktree(
        database: &GitDatabase,
        oid: &str,
        workspace_path: &Path,
        includes: Vec<String>,
        excludes: Vec<String>,
    ) -> Result<()> {
        // Prune stale worktree registrations before adding
        database.prune_worktrees()?;
        std::fs::create_dir_all(workspace_path.parent().unwrap_or(workspace_path))?;
        let status = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                &workspace_path.to_string_lossy(),
                oid,
            ])
            .current_dir(&database.path)
            .status()
            .context("failed to run `git worktree add --detach`")?;
        if !status.success() {
            anyhow::bail!(
                "git worktree add --detach failed for {}",
                workspace_path.display()
            );
        }

        let status = std::process::Command::new("git")
            .args(["sparse-checkout", "init", "--cone"])
            .current_dir(workspace_path)
            .status()
            .context("failed to run `git sparse-checkout init`")?;
        if !status.success() {
            anyhow::bail!(
                "git sparse-checkout init failed in {}",
                workspace_path.display()
            );
        }

        // Build sparse-checkout patterns: includes are added directly,
        // excludes are prefixed with '!' (negation)
        let mut patterns: Vec<String> = includes;
        for exclude in excludes {
            patterns.push(format!("!{}", exclude));
        }
        if !patterns.is_empty() {
            let mut cmd = std::process::Command::new("git");
            cmd.args(["sparse-checkout", "set"]);
            for pattern in &patterns {
                cmd.arg(pattern);
            }
            let status = cmd
                .current_dir(workspace_path)
                .status()
                .context("failed to run `git sparse-checkout set`")?;
            if !status.success() {
                anyhow::bail!(
                    "git sparse-checkout set failed in {}",
                    workspace_path.display()
                );
            }
        }
        Ok(())
    }

    fn checkout_filtered_copy(
        database: &GitDatabase,
        oid: &str,
        workspace_path: &Path,
        includes: Vec<String>,
        excludes: Vec<String>,
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path();
        let status = std::process::Command::new("git")
            .args([
                "--work-tree",
                &temp_path.to_string_lossy(),
                "checkout",
                oid,
                "--",
                ".",
            ])
            .current_dir(&database.path)
            .status()
            .context("failed to run `git checkout` for filtered copy")?;
        if !status.success() {
            anyhow::bail!("git checkout failed for oid {}", oid);
        }
        std::fs::create_dir_all(workspace_path)?;
        Self::copy_filtered(temp_path, workspace_path, &includes, &excludes)?;
        Ok(())
    }

    fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                std::fs::create_dir_all(&dst_path)?;
                Self::copy_recursive(&src_path, &dst_path)?;
            } else {
                // Try hard link first, fall back to copy
                if std::fs::hard_link(&src_path, &dst_path).is_err() {
                    std::fs::copy(&src_path, &dst_path)?;
                }
            }
        }
        Ok(())
    }

    fn copy_filtered(
        src: &Path,
        dst: &Path,
        includes: &[String],
        excludes: &[String],
    ) -> Result<()> {
        let include_patterns: Vec<Pattern> = includes
            .iter()
            .map(|p| Pattern::new(p))
            .collect::<Result<Vec<_>, _>>()
            .context("invalid include glob pattern")?;
        let exclude_patterns: Vec<Pattern> = excludes
            .iter()
            .map(|p| Pattern::new(p))
            .collect::<Result<Vec<_>, _>>()
            .context("invalid exclude glob pattern")?;

        Self::copy_filtered_recursive(src, dst, src, &include_patterns, &exclude_patterns)
    }

    fn copy_filtered_recursive(
        entry_path: &Path,
        dst_base: &Path,
        src_base: &Path,
        includes: &[Pattern],
        excludes: &[Pattern],
    ) -> Result<()> {
        for entry in std::fs::read_dir(entry_path)? {
            let entry = entry?;
            let src_path = entry.path();
            let rel_path = src_path
                .strip_prefix(src_base)
                .unwrap_or(&src_path)
                .to_string_lossy();

            if src_path.is_dir() {
                // Recurse into directories — filtering applies to files
                Self::copy_filtered_recursive(&src_path, dst_base, src_base, includes, excludes)?;
            } else {
                // Apply include filter: if includes are specified, file must match at least one
                let included = includes.is_empty() || includes.iter().any(|p| p.matches(&rel_path));
                if !included {
                    continue;
                }
                // Apply exclude filter: if file matches any exclude, skip it
                let excluded = excludes.iter().any(|p| p.matches(&rel_path));
                if excluded {
                    continue;
                }

                let dst_path = dst_base.join(src_path.strip_prefix(src_base).unwrap_or(&src_path));
                if let Some(parent) = dst_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if std::fs::hard_link(&src_path, &dst_path).is_err() {
                    std::fs::copy(&src_path, &dst_path)?;
                }
            }
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Status information for a workspace repo
#[derive(Debug)]
pub struct RepoStatus {
    pub is_dirty: bool,
    pub head_oid: String,
    pub branch: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

/// Query git status of a workspace path. Works for worktree-mode repos.
/// For copy-mode repos, returns a basic status (always clean).
pub fn repo_status(workspace_path: &Path) -> Result<RepoStatus> {
    if !workspace_path.exists() {
        anyhow::bail!(
            "workspace path does not exist: {}",
            workspace_path.display()
        );
    }

    // Check if it's a git repo (worktree or regular)
    let git_dir = workspace_path.join(".git");
    if !git_dir.exists() {
        // Copy-mode repo — not a git repo, always "clean"
        return Ok(RepoStatus {
            is_dirty: false,
            head_oid: String::from("(copy mode)"),
            branch: None,
            ahead: 0,
            behind: 0,
        });
    }

    // Get HEAD oid
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_path)
        .output()
        .context("failed to run `git rev-parse HEAD`")?;
    let head_oid = if output.status.success() {
        String::from_utf8(output.stdout)?.trim().to_string()
    } else {
        String::from("(unknown)")
    };

    // Get current branch
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(workspace_path)
        .output()
        .context("failed to run `git symbolic-ref`")?;
    let branch = if output.status.success() {
        Some(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        None // detached HEAD
    };

    // Check for dirty working tree
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_path)
        .output()
        .context("failed to run `git status`")?;
    let is_dirty = output.status.success() && !output.stdout.is_empty();

    // Get ahead/behind counts vs upstream (if tracking branch exists)
    let (ahead, behind) = if let Some(ref branch_name) = branch {
        let output = std::process::Command::new("git")
            .args([
                "rev-list",
                "--left-right",
                "--count",
                &format!("{}...@{{upstream}}", branch_name),
            ])
            .current_dir(workspace_path)
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                let parts: Vec<&str> = text.trim().split('\t').collect();
                if parts.len() == 2 {
                    (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0))
                } else {
                    (0, 0)
                }
            }
            _ => (0, 0),
        }
    } else {
        (0, 0)
    };

    Ok(RepoStatus {
        is_dirty,
        head_oid,
        branch,
        ahead,
        behind,
    })
}

/// Write a `.unified-ok` marker file in the workspace path to indicate successful checkout.
pub fn write_ok_marker(workspace_path: &Path) -> Result<()> {
    let marker = workspace_path.join(".unified-ok");
    std::fs::write(&marker, "")?;
    Ok(())
}

/// Check if a `.unified-ok` marker exists for a workspace path.
pub fn has_ok_marker(workspace_path: &Path) -> bool {
    workspace_path.join(".unified-ok").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url() {
        assert_eq!(
            normalize_url("https://github.com/user/repo.git"),
            "https://github.com/user/repo.git"
        );
        assert_eq!(
            normalize_url("https://github.com/user/repo.git/"),
            "https://github.com/user/repo.git"
        );
        assert_eq!(
            normalize_url("git@github.com:user/repo.git"),
            "ssh://git@github.com/user/repo.git"
        );
    }

    #[test]
    fn test_refspecs_for() {
        let remote = GitRemote::new("https://github.com/user/repo.git");
        assert_eq!(
            remote.refspecs_for(&GitReference::Branch("main".to_string())),
            vec!["+refs/heads/main:refs/heads/main"]
        );
        assert_eq!(
            remote.refspecs_for(&GitReference::Tag("v1.0".to_string())),
            vec!["+refs/tags/v1.0:refs/tags/v1.0"]
        );
        assert_eq!(
            remote.refspecs_for(&GitReference::Rev("abc123".to_string())),
            Vec::<String>::new()
        );
        assert_eq!(
            remote.refspecs_for(&GitReference::DefaultBranch),
            vec!["+HEAD:refs/remotes/origin/HEAD"]
        );
    }

    #[test]
    fn test_git_database_new() {
        let cache_root = std::path::PathBuf::from("tests/test_cache");
        let _ = std::fs::remove_dir_all(&cache_root); // Clean up from previous runs
        let cache = un_cache::Cache::with_custom_root(cache_root.clone());
        let db = GitDatabase::new(&cache, "test", "https://github.com/user/repo.git").unwrap();
        assert!(db.path().to_string_lossy().contains("test-"));
        let _ = std::fs::remove_dir_all(&cache_root); // Clean up after test
    }

    #[test]
    fn test_copy_recursive() {
        let temp_src = tempfile::tempdir().unwrap();
        let temp_dst = tempfile::tempdir().unwrap();

        // Create nested structure
        std::fs::create_dir_all(temp_src.path().join("sub/nested")).unwrap();
        std::fs::write(temp_src.path().join("root.txt"), "root").unwrap();
        std::fs::write(temp_src.path().join("sub/mid.txt"), "mid").unwrap();
        std::fs::write(temp_src.path().join("sub/nested/deep.txt"), "deep").unwrap();

        GitCheckout::copy_recursive(temp_src.path(), temp_dst.path()).unwrap();

        assert!(temp_dst.path().join("root.txt").exists());
        assert!(temp_dst.path().join("sub/mid.txt").exists());
        assert!(temp_dst.path().join("sub/nested/deep.txt").exists());
        assert_eq!(
            std::fs::read_to_string(temp_dst.path().join("sub/nested/deep.txt")).unwrap(),
            "deep"
        );
    }

    #[test]
    fn test_copy_filtered_with_patterns() {
        let temp_src = tempfile::tempdir().unwrap();
        let temp_dst = tempfile::tempdir().unwrap();

        // Create files
        std::fs::create_dir_all(temp_src.path().join("src")).unwrap();
        std::fs::create_dir_all(temp_src.path().join("tests")).unwrap();
        std::fs::write(temp_src.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(temp_src.path().join("src/main.rs"), "main").unwrap();
        std::fs::write(temp_src.path().join("tests/test.rs"), "test").unwrap();
        std::fs::write(temp_src.path().join("README.md"), "readme").unwrap();

        // Include only src/**, exclude nothing
        GitCheckout::copy_filtered(
            temp_src.path(),
            temp_dst.path(),
            &["src/**".to_string()],
            &[],
        )
        .unwrap();

        assert!(temp_dst.path().join("src/lib.rs").exists());
        assert!(temp_dst.path().join("src/main.rs").exists());
        assert!(!temp_dst.path().join("tests/test.rs").exists());
        assert!(!temp_dst.path().join("README.md").exists());
    }

    #[test]
    fn test_copy_filtered_with_excludes() {
        let temp_src = tempfile::tempdir().unwrap();
        let temp_dst = tempfile::tempdir().unwrap();

        // Create files
        std::fs::create_dir_all(temp_src.path().join("src")).unwrap();
        std::fs::write(temp_src.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(temp_src.path().join("src/test.rs"), "test").unwrap();
        std::fs::write(temp_src.path().join("README.md"), "readme").unwrap();

        // Include everything, exclude *test*
        GitCheckout::copy_filtered(
            temp_src.path(),
            temp_dst.path(),
            &[],
            &["**/test*".to_string()],
        )
        .unwrap();

        assert!(temp_dst.path().join("src/lib.rs").exists());
        assert!(!temp_dst.path().join("src/test.rs").exists());
        assert!(temp_dst.path().join("README.md").exists());
    }
}
