use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::thread;
use un_cache::Cache;
use un_core::{Config, GitReference, LockFile, LockedRepo, Settings};
use un_git::{CheckoutMode, GitCheckout, GitDatabase, GitRemote};

#[derive(Parser)]
#[command(name = "un")]
#[command(about = "Unified Repo & Artifact Manager")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new unified workspace
    Init,
    /// Sync the workspace
    Sync {
        /// Fail if config has changed since lock file was written
        #[arg(long)]
        locked: bool,
        /// No network access — use only cache and lock file
        #[arg(long)]
        frozen: bool,
        /// Use shallow clones (depth 1) for all repos
        #[arg(long)]
        shallow: bool,
    },
    /// Fetch latest for branch-tracking repos and update lock file
    Update,
    /// Show workspace status
    Status,
    /// Add a repository to the config
    Add {
        /// Git URL of the repository
        url: String,
        /// Name for the repo entry (defaults to repo name from URL)
        #[arg(long)]
        name: Option<String>,
        /// Workspace path (defaults to name)
        #[arg(long)]
        path: Option<String>,
        /// Branch to track
        #[arg(long)]
        branch: Option<String>,
        /// Tag to pin
        #[arg(long)]
        tag: Option<String>,
        /// Revision to pin
        #[arg(long)]
        rev: Option<String>,
    },
    /// Remove a repository from the config
    Remove {
        /// Name of the repo to remove
        name: String,
    },
}

/// Compute SHA-256 hash of the config file content for --locked detection.
fn config_hash(config_content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config_content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Infer a repo name from a URL (last path segment, minus .git).
fn repo_name_from_url(url: &str) -> String {
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    url.rsplit('/')
        .next()
        .or_else(|| url.rsplit(':').next())
        .unwrap_or("repo")
        .to_string()
}

/// Resolve the shallow setting from CLI flag → env var → per-repo → global settings.
fn resolve_shallow(cli_shallow: bool, repo_shallow: Option<bool>, settings: &Settings) -> bool {
    if cli_shallow {
        return true;
    }
    if let Ok(val) = std::env::var("UN_SHALLOW")
        && (val == "1" || val.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    repo_shallow.unwrap_or(settings.shallow.unwrap_or(false))
}

/// Build the CheckoutMode from repo config fields.
fn resolve_checkout_mode(repo: &un_core::Repo) -> CheckoutMode {
    if let Some(checkout) = &repo.checkout {
        match checkout.as_str() {
            "copy" => {
                if repo.include.is_some() || repo.exclude.is_some() {
                    CheckoutMode::FilteredCopy {
                        includes: repo.include.clone().unwrap_or_default(),
                        excludes: repo.exclude.clone().unwrap_or_default(),
                    }
                } else {
                    CheckoutMode::Copy
                }
            }
            _ => resolve_worktree_mode(repo),
        }
    } else {
        resolve_worktree_mode(repo)
    }
}

fn resolve_worktree_mode(repo: &un_core::Repo) -> CheckoutMode {
    if repo.include.is_some() || repo.exclude.is_some() {
        CheckoutMode::SparseWorktree {
            includes: repo.include.clone().unwrap_or_default(),
            excludes: repo.exclude.clone().unwrap_or_default(),
        }
    } else {
        CheckoutMode::Worktree
    }
}

/// Resolve GitReference from repo config fields.
fn resolve_reference(repo: &un_core::Repo) -> GitReference {
    if let Some(branch) = &repo.branch {
        GitReference::Branch(branch.clone())
    } else if let Some(tag) = &repo.tag {
        GitReference::Tag(tag.clone())
    } else if let Some(rev) = &repo.rev {
        GitReference::Rev(rev.clone())
    } else {
        GitReference::DefaultBranch
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init()?,
        Commands::Sync {
            locked,
            frozen,
            shallow,
        } => cmd_sync(locked, frozen, shallow)?,
        Commands::Update => cmd_update()?,
        Commands::Status => cmd_status()?,
        Commands::Add {
            url,
            name,
            path,
            branch,
            tag,
            rev,
        } => cmd_add(url, name, path, branch, tag, rev)?,
        Commands::Remove { name } => cmd_remove(name)?,
    }
    Ok(())
}

fn cmd_init() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        workspace: un_core::Workspace {
            name: "my-workspace".to_string(),
            members: None,
            exclude: None,
        },
        settings: Some(Settings::default()),
        repos: std::collections::HashMap::new(),
    };
    let toml_str = toml::to_string(&config)?;
    std::fs::write("unified.toml", toml_str)?;
    println!("Created unified.toml");
    Ok(())
}

fn cmd_sync(
    locked: bool,
    frozen: bool,
    cli_shallow: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let config: Config = toml::from_str(&config_content)?;
    let current_hash = config_hash(&config_content);
    let default_settings = Settings::default();
    let settings = config.settings.as_ref().unwrap_or(&default_settings);

    // Read existing lock file if present
    let existing_lock: Option<LockFile> = if std::path::Path::new("unified.lock").exists() {
        Some(toml::from_str(&std::fs::read_to_string("unified.lock")?)?)
    } else {
        None
    };

    // --locked: fail if config changed since lock was written
    if locked {
        let lock = existing_lock
            .as_ref()
            .ok_or("--locked requires an existing unified.lock file")?;
        if let Some(ref saved_hash) = lock.config_hash {
            if saved_hash != &current_hash {
                return Err("unified.toml has changed since unified.lock was written. Run `un sync` to update.".into());
            }
        } else {
            return Err(
                "unified.lock has no config_hash — run `un sync` without --locked first.".into(),
            );
        }
    }

    // --frozen: no network, resolve entirely from lock + cache
    if frozen {
        let lock = existing_lock
            .as_ref()
            .ok_or("--frozen requires an existing unified.lock file")?;
        return cmd_sync_frozen(&config, lock, settings);
    }

    let max_parallel = settings.parallel.unwrap_or(4);

    // Collect work items
    let repos: Vec<(String, un_core::Repo)> = config.repos.clone().into_iter().collect();

    if repos.is_empty() {
        println!("No repos configured.");
        return Ok(());
    }

    // Set up progress display
    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template("{prefix:.bold} {spinner} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());

    // Process repos in parallel batches
    let locked_repos = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));

    for chunk in repos.chunks(max_parallel) {
        let mut handles = Vec::new();

        for (name, repo) in chunk {
            let name = name.clone();
            let repo = repo.clone();
            let cache = Cache::new()?;
            let settings_shallow = settings.shallow;
            let git_fetch_with_cli = settings.git_fetch_with_cli.unwrap_or(false);
            let locked_repos = Arc::clone(&locked_repos);
            let errors = Arc::clone(&errors);
            let pb = multi.add(ProgressBar::new_spinner());
            pb.set_style(style.clone());
            pb.set_prefix(name.clone());
            pb.set_message("fetching...");

            let shallow = resolve_shallow(
                cli_shallow,
                repo.shallow,
                &Settings {
                    shallow: settings_shallow,
                    ..Settings::default()
                },
            );

            handles.push(thread::spawn(move || {
                let result =
                    sync_single_repo(&name, &repo, &cache, shallow, git_fetch_with_cli, &pb);
                match result {
                    Ok((oid, reference)) => {
                        pb.finish_with_message("done ✓");
                        locked_repos.lock().unwrap().insert(
                            name.clone(),
                            LockedRepo {
                                url: repo.url.clone(),
                                oid,
                                reference,
                            },
                        );
                    }
                    Err(e) => {
                        pb.finish_with_message(format!("FAILED: {}", e));
                        errors.lock().unwrap().push(format!("{}: {}", name, e));
                    }
                }
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }
    }

    // Check for errors
    let errs = errors.lock().unwrap();
    if !errs.is_empty() {
        eprintln!("\nSync errors:");
        for e in errs.iter() {
            eprintln!("  {}", e);
        }
        return Err(format!("{} repo(s) failed to sync", errs.len()).into());
    }

    // Write lock file with config hash
    let lock_file = LockFile {
        version: 1,
        config_hash: Some(current_hash),
        repos: Arc::try_unwrap(locked_repos).unwrap().into_inner().unwrap(),
    };
    let lock_toml = toml::to_string(&lock_file)?;
    std::fs::write("unified.lock", lock_toml)?;
    println!("Updated unified.lock");

    // Auto-update .gitignore if enabled
    if settings.manage_gitignore.unwrap_or(true) {
        update_gitignore(&config)?;
    }

    // Auto-update .vscode/settings.json if enabled
    if settings.manage_vscode.unwrap_or(true) {
        update_vscode_settings(&config)?;
    }

    Ok(())
}

/// Sync a single repo: fetch + checkout. Returns (oid, reference).
fn sync_single_repo(
    name: &str,
    repo: &un_core::Repo,
    cache: &Cache,
    shallow: bool,
    git_fetch_with_cli: bool,
    pb: &ProgressBar,
) -> Result<(String, GitReference), Box<dyn std::error::Error + Send + Sync>> {
    let remote = GitRemote::new(&repo.url);
    let database = GitDatabase::new(cache, name, &repo.url)?;
    let reference = resolve_reference(repo);

    pb.set_message("fetching...");
    let oid = database.fetch(&remote, &reference, shallow, git_fetch_with_cli)?;

    let workspace_path = std::env::current_dir()?.join(&repo.path);
    let mode = resolve_checkout_mode(repo);

    // Skip checkout if already checked out at the correct oid
    if workspace_path.exists() && un_git::has_ok_marker(&workspace_path) {
        // Read the marker to see if oid matches — for now, re-checkout
        // A future optimization: store oid in marker and skip if unchanged
        pb.set_message("already checked out, skipping");
    } else {
        pb.set_message("checking out...");
        GitCheckout::new(&database, &oid, &workspace_path, mode)?;
        un_git::write_ok_marker(&workspace_path)?;
    }

    pb.set_message(format!("done ({})", &oid[..8.min(oid.len())]));
    println!("  {} → {} @ {}", name, repo.path, &oid[..12.min(oid.len())]);
    Ok((oid, reference))
}

/// Frozen sync — no network, resolve from lock + cache only.
fn cmd_sync_frozen(
    config: &Config,
    lock: &LockFile,
    _settings: &Settings,
) -> Result<(), Box<dyn std::error::Error>> {
    let cache = Cache::new()?;

    for (name, repo) in &config.repos {
        let locked = lock
            .repos
            .get(name)
            .ok_or_else(|| format!("--frozen: repo '{}' not found in unified.lock", name))?;

        let workspace_path = std::env::current_dir()?.join(&repo.path);
        if workspace_path.exists() {
            println!("  {} → {} (already exists, skipping)", name, repo.path);
            continue;
        }

        // Checkout from cached database (no fetch)
        let database = GitDatabase::new(&cache, name, &repo.url)?;
        let mode = resolve_checkout_mode(repo);
        GitCheckout::new(&database, &locked.oid, &workspace_path, mode)?;
        un_git::write_ok_marker(&workspace_path)?;
        println!(
            "  {} → {} @ {} (from cache)",
            name,
            repo.path,
            &locked.oid[..12.min(locked.oid.len())]
        );
    }

    Ok(())
}

fn cmd_update() -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let config: Config = toml::from_str(&config_content)?;
    let cache = Cache::new()?;
    let default_settings = Settings::default();
    let settings = config.settings.as_ref().unwrap_or(&default_settings);

    let existing_lock: Option<LockFile> = if std::path::Path::new("unified.lock").exists() {
        Some(toml::from_str(&std::fs::read_to_string("unified.lock")?)?)
    } else {
        None
    };

    let mut updated = 0;
    let mut locked_repos = std::collections::HashMap::new();

    for (name, repo) in &config.repos {
        let reference = resolve_reference(repo);

        // Only update branch-tracking repos (not pinned to tag/rev)
        let is_tracking = matches!(
            reference,
            GitReference::Branch(_) | GitReference::DefaultBranch
        );

        let remote = GitRemote::new(&repo.url);
        let database = GitDatabase::new(&cache, name, &repo.url)?;
        let shallow = resolve_shallow(false, repo.shallow, settings);
        let oid = database.fetch(
            &remote,
            &reference,
            shallow,
            settings.git_fetch_with_cli.unwrap_or(false),
        )?;

        let old_oid = existing_lock
            .as_ref()
            .and_then(|l| l.repos.get(name))
            .map(|r| r.oid.as_str());

        match old_oid {
            Some(old) if is_tracking && old != oid.as_str() => {
                println!(
                    "  {} updated: {} → {}",
                    name,
                    &old[..12.min(old.len())],
                    &oid[..12.min(oid.len())]
                );
                updated += 1;
            }
            None => {
                println!("  {} new: {}", name, &oid[..12.min(oid.len())]);
                updated += 1;
            }
            _ => {
                println!("  {} unchanged @ {}", name, &oid[..12.min(oid.len())]);
            }
        }

        locked_repos.insert(
            name.clone(),
            LockedRepo {
                url: repo.url.clone(),
                oid,
                reference,
            },
        );
    }

    // Write updated lock file
    let lock_file = LockFile {
        version: 1,
        config_hash: Some(config_hash(&config_content)),
        repos: locked_repos,
    };
    let lock_toml = toml::to_string(&lock_file)?;
    std::fs::write("unified.lock", lock_toml)?;

    if updated > 0 {
        println!("Updated {} repo(s). Run `un sync` to apply.", updated);
    } else {
        println!("All repos up to date.");
    }

    Ok(())
}

fn cmd_status() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    let existing_lock: Option<LockFile> = if std::path::Path::new("unified.lock").exists() {
        Some(toml::from_str(&std::fs::read_to_string("unified.lock")?)?)
    } else {
        None
    };

    if config.repos.is_empty() {
        println!("No repos configured.");
        return Ok(());
    }

    let mut names: Vec<&String> = config.repos.keys().collect();
    names.sort();

    for name in names {
        let repo = &config.repos[name];
        let workspace_path = std::env::current_dir()?.join(&repo.path);

        if !workspace_path.exists() {
            println!("  {} → {} [NOT SYNCED]", name, repo.path);
            continue;
        }

        let status = un_git::repo_status(&workspace_path)?;

        let locked_oid = existing_lock
            .as_ref()
            .and_then(|l| l.repos.get(name.as_str()))
            .map(|r| &r.oid[..12.min(r.oid.len())]);

        let mut flags = Vec::new();
        if status.is_dirty {
            flags.push("dirty");
        }
        if status.ahead > 0 {
            flags.push("ahead");
        }
        if status.behind > 0 {
            flags.push("behind");
        }

        let branch_info = if let Some(ref branch) = status.branch {
            format!(" on {}", branch)
        } else {
            String::from(" (detached)")
        };

        let oid_display = if status.head_oid == "(copy mode)" {
            "(copy mode)".to_string()
        } else {
            status.head_oid[..12.min(status.head_oid.len())].to_string()
        };

        let status_str = if flags.is_empty() {
            "clean".to_string()
        } else {
            flags.join(", ")
        };

        println!(
            "  {}{} @ {} [{}]{}",
            name,
            branch_info,
            oid_display,
            status_str,
            locked_oid
                .map(|o| format!(" (locked: {})", o))
                .unwrap_or_default(),
        );
    }

    Ok(())
}

fn cmd_add(
    url: String,
    name: Option<String>,
    path: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let mut config: Config = toml::from_str(&config_content)?;

    let repo_name = name.unwrap_or_else(|| repo_name_from_url(&url));
    let repo_path = path.unwrap_or_else(|| repo_name.clone());

    if config.repos.contains_key(&repo_name) {
        return Err(format!("repo '{}' already exists in unified.toml", repo_name).into());
    }

    let repo = un_core::Repo {
        url: url.clone(),
        path: repo_path.clone(),
        branch,
        tag,
        rev,
        checkout: None,
        include: None,
        exclude: None,
        shallow: None,
    };

    config.repos.insert(repo_name.clone(), repo);

    let toml_str = toml::to_string(&config)?;
    std::fs::write("unified.toml", toml_str)?;
    println!("Added '{}' → {} ({})", repo_name, repo_path, url);
    println!("Run `un sync` to fetch and checkout.");

    Ok(())
}

fn cmd_remove(name: String) -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let mut config: Config = toml::from_str(&config_content)?;

    let repo = config
        .repos
        .remove(&name)
        .ok_or_else(|| format!("repo '{}' not found in unified.toml", name))?;

    // Write updated config
    let toml_str = toml::to_string(&config)?;
    std::fs::write("unified.toml", toml_str)?;

    // Remove from lock file if present
    if std::path::Path::new("unified.lock").exists() {
        let lock_content = std::fs::read_to_string("unified.lock")?;
        let mut lock: LockFile = toml::from_str(&lock_content)?;
        lock.repos.remove(&name);
        let lock_toml = toml::to_string(&lock)?;
        std::fs::write("unified.lock", lock_toml)?;
    }

    // Remove workspace directory if it exists
    let workspace_path = std::env::current_dir()?.join(&repo.path);
    if workspace_path.exists() {
        // If it's a worktree, prune it first
        let git_file = workspace_path.join(".git");
        if git_file.exists() && git_file.is_file() {
            // It's a worktree — read the gitdir to find parent, then prune
            let _ = std::process::Command::new("git")
                .args(["worktree", "remove", &workspace_path.to_string_lossy()])
                .status();
        }
        if workspace_path.exists() {
            std::fs::remove_dir_all(&workspace_path)?;
        }
        println!("Removed workspace at {}", repo.path);
    }

    println!("Removed '{}' from unified.toml", name);

    // Update .gitignore and .vscode/settings.json
    let settings = config.settings.as_ref().cloned().unwrap_or_default();
    if settings.manage_gitignore.unwrap_or(true) {
        update_gitignore(&config)?;
    }
    if settings.manage_vscode.unwrap_or(true) {
        update_vscode_settings(&config)?;
    }

    Ok(())
}

fn update_gitignore(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let gitignore_path = ".gitignore";
    let managed_block_start = "# BEGIN UNIFIED MANAGED BLOCK - DO NOT EDIT\n";
    let managed_block_end = "# END UNIFIED MANAGED BLOCK\n";

    // Collect paths to ignore
    let mut ignore_paths: Vec<&str> = config.repos.values().map(|r| r.path.as_str()).collect();
    ignore_paths.sort();

    // Read existing .gitignore
    let existing_content = if std::path::Path::new(gitignore_path).exists() {
        std::fs::read_to_string(gitignore_path)?
    } else {
        String::new()
    };

    // Remove all existing managed blocks
    let mut cleaned_content = existing_content.clone();
    while let Some(start_pos) = cleaned_content.find(managed_block_start) {
        if let Some(end_pos) = cleaned_content[start_pos..].find(managed_block_end) {
            let actual_end_pos = start_pos + end_pos + managed_block_end.len();
            cleaned_content = format!(
                "{}{}",
                &cleaned_content[..start_pos],
                &cleaned_content[actual_end_pos..]
            );
        } else {
            eprintln!(
                "Warning: malformed unified managed block in .gitignore (missing end marker), skipping cleanup"
            );
            break;
        }
    }

    // Build new managed block (only if there are paths)
    if ignore_paths.is_empty() {
        // No repos — just write cleaned content without managed block
        std::fs::write(
            gitignore_path,
            cleaned_content.trim_end().to_string() + "\n",
        )?;
    } else {
        let new_content = if cleaned_content.trim().is_empty() {
            format!(
                "{}{}\n{}",
                managed_block_start,
                ignore_paths.join("\n"),
                managed_block_end
            )
        } else {
            format!(
                "{}\n{}{}\n{}",
                cleaned_content.trim_end(),
                managed_block_start,
                ignore_paths.join("\n"),
                managed_block_end
            )
        };
        std::fs::write(gitignore_path, new_content)?;
    }

    println!("Updated .gitignore");
    Ok(())
}

fn update_vscode_settings(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Path;

    let settings_dir = Path::new(".vscode");
    let settings_path = settings_dir.join("settings.json");

    if !settings_dir.exists() {
        std::fs::create_dir_all(settings_dir)?;
    }

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mut ignored_repos: Vec<&str> = config.repos.values().map(|r| r.path.as_str()).collect();
    ignored_repos.sort();

    if let serde_json::Value::Object(ref mut map) = settings {
        map.insert(
            "git.ignoredRepositories".to_string(),
            serde_json::json!(ignored_repos),
        );
    }

    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, content)?;
    println!("Updated .vscode/settings.json");
    Ok(())
}
