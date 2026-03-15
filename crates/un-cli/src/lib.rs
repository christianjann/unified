use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::thread;
use tui_banner::{Banner, Style};
use un_cache::Cache;
use un_core::{
    App, Config, DownloadSource, GitReference, LockFile, LockedApp, LockedArtifact,
    LockedRepo, LockedTool, Settings, Tool, UserConfig,
};
use un_download::{DownloadEngine, GitHubProvider, ArtifactoryProvider, HttpProvider};
use un_git::{CheckoutMode, GitCheckout, GitDatabase, GitRemote};

#[derive(Parser)]
#[command(name = "un")]
#[command(about = "Unified Repo & Artifact Manager")]
#[command(version = version_string())]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

fn version_string() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let version = env!("CARGO_PKG_VERSION");
        let hash = option_env!("UN_COMMIT_SHORT_HASH");
        match hash {
            Some(hash) => format!("{version} ({hash})"),
            None => version.to_string(),
        }
    })
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
        /// Sync only the named collection
        #[arg(long)]
        collection: Option<String>,
        /// Sync everything, ignoring active collection
        #[arg(long)]
        all: bool,
    },
    /// Fetch latest for branch-tracking repos and update lock file
    Update {
        /// Update only the named collection
        #[arg(long)]
        collection: Option<String>,
        /// Update everything, ignoring active collection
        #[arg(long)]
        all: bool,
    },
    /// Show workspace status
    Status {
        /// Show status only for the named collection
        #[arg(long)]
        collection: Option<String>,
        /// Show status for all repos, ignoring active collection
        #[arg(long)]
        all: bool,
    },
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
    /// Create a new branch in a worktree-mode repo
    Branch {
        /// Name of the repo
        repo: String,
        /// Branch name to create
        name: String,
    },
    /// Commit all changes in a worktree-mode repo
    Commit {
        /// Name of the repo
        repo: String,
        /// Commit message (opens $EDITOR if omitted)
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Push a worktree-mode repo
    Push {
        /// Name of the repo
        repo: String,
    },
    /// Show git diff for one or all worktree-mode repos
    Diff {
        /// Name of the repo (shows all if omitted)
        repo: Option<String>,
        /// Diff only the named collection
        #[arg(long)]
        collection: Option<String>,
        /// Diff all repos, ignoring active collection
        #[arg(long)]
        all: bool,
    },
    /// Show git log for a worktree-mode repo
    Log {
        /// Name of the repo
        repo: String,
        /// Number of commits to show
        #[arg(short = 'n', long, default_value = "10")]
        count: usize,
    },
    /// Manage collections
    #[command(subcommand)]
    Collection(CollectionCommand),
    /// Download and run a tool
    Run {
        /// Tool name (from [tools] config)
        tool: String,
        /// Arguments to pass to the tool
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage tools
    #[command(subcommand)]
    Tool(ToolCommand),
    /// Download and launch an application
    App {
        /// App name (from [apps] config)
        name: String,
    },
    /// Run a task or list all tasks
    Task {
        /// Task name (omit to list all tasks)
        name: Option<String>,
    },
    /// Run setup commands from [setup]
    Setup,
    /// Interactive launcher menu
    Launch,
    /// Print version information
    Version,
    /// Print project information (author, license, commit)
    About,
}

#[derive(Subcommand)]
pub enum ToolCommand {
    /// Install a tool globally to ~/.unified/bin/
    Install {
        /// Tool name (from [tools] config), or all if omitted
        name: Option<String>,
    },
    /// List installed tools and their cached versions
    List,
}

#[derive(Subcommand)]
pub enum CollectionCommand {
    /// Set the default collection
    Use {
        /// Collection name to set as default (omit with --clear to remove)
        name: Option<String>,
        /// Remove the default collection
        #[arg(long)]
        clear: bool,
    },
    /// List all collections with member counts
    List,
    /// Show repos/artifacts/tools in a collection
    Show {
        /// Name of the collection
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

/// Resolve the active collection from CLI flag → env var → user.toml → None.
/// Returns None if `--all` is set.
fn resolve_active_collection(cli_collection: Option<&str>, cli_all: bool) -> Option<String> {
    if cli_all {
        return None;
    }
    if let Some(name) = cli_collection {
        return Some(name.to_string());
    }
    if let Ok(val) = std::env::var("UN_COLLECTION")
        && !val.is_empty()
    {
        return Some(val);
    }
    let workspace_root = std::env::current_dir().ok()?;
    let user_config = UserConfig::load(&workspace_root);
    user_config.default_collection
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init()?,
        Commands::Sync {
            locked,
            frozen,
            shallow,
            collection,
            all,
        } => cmd_sync(locked, frozen, shallow, collection.as_deref(), all)?,
        Commands::Update { collection, all } => cmd_update(collection.as_deref(), all)?,
        Commands::Status { collection, all } => cmd_status(collection.as_deref(), all)?,
        Commands::Add {
            url,
            name,
            path,
            branch,
            tag,
            rev,
        } => cmd_add(url, name, path, branch, tag, rev)?,
        Commands::Remove { name } => cmd_remove(name)?,
        Commands::Branch { repo, name } => cmd_branch(&repo, &name)?,
        Commands::Commit { repo, message } => cmd_commit(&repo, message.as_deref())?,
        Commands::Push { repo } => cmd_push(&repo)?,
        Commands::Diff {
            repo,
            collection,
            all,
        } => cmd_diff(repo.as_deref(), collection.as_deref(), all)?,
        Commands::Log { repo, count } => cmd_log(&repo, count)?,
        Commands::Collection(sub) => cmd_collection(sub)?,
        Commands::Run { tool, args } => cmd_run(&tool, args)?,
        Commands::Tool(sub) => cmd_tool(sub)?,
        Commands::App { name } => cmd_app(&name)?,
        Commands::Task { name } => cmd_task(name.as_deref())?,
        Commands::Setup => cmd_setup()?,
        Commands::Launch => cmd_launch()?,
        Commands::Version => cmd_version(),
        Commands::About => cmd_about(),
    }
    Ok(())
}

fn cmd_version() {
    println!("un {}", version_string());
}

fn cmd_about() {
    if let Ok(banner) = Banner::new("un - Unified Repo").map(|b| b.style(Style::NeonCyber).render())
    {
        println!("{banner}");
    }
    println!("Version:  {}", env!("CARGO_PKG_VERSION"));
    if let Some(hash) = option_env!("UN_COMMIT_HASH") {
        println!("Commit:   {}", hash);
    }
    if let Some(date) = option_env!("UN_COMMIT_DATE") {
        println!("Date:     {}", date);
    }
    println!("Author:   {}", env!("CARGO_PKG_AUTHORS"));
    println!("License:  {}", env!("CARGO_PKG_LICENSE"));
    println!("Homepage: {}", env!("CARGO_PKG_HOMEPAGE"));
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
        artifacts: std::collections::HashMap::new(),
        tools: std::collections::HashMap::new(),
        apps: std::collections::HashMap::new(),
        tasks: std::collections::HashMap::new(),
        setup: None,
        launcher: None,
        collections: std::collections::HashMap::new(),
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
    cli_collection: Option<&str>,
    cli_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let config: Config = toml::from_str(&config_content)?;
    let current_hash = config_hash(&config_content);
    let default_settings = Settings::default();
    let settings = config.settings.as_ref().unwrap_or(&default_settings);

    // Validate collections
    let validation_errors = config.validate_collections();
    if !validation_errors.is_empty() {
        for e in &validation_errors {
            eprintln!("error: {}", e);
        }
        return Err(format!("{} collection validation error(s)", validation_errors.len()).into());
    }

    // Resolve active collection
    let active_collection = resolve_active_collection(cli_collection, cli_all);
    if let Some(ref name) = active_collection {
        println!("Using collection: {}", name);
    }

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

    // Filter repos by collection
    let active_repos = config.repos_for_collection(active_collection.as_deref())?;

    // --frozen: no network, resolve entirely from lock + cache
    if frozen {
        let lock = existing_lock
            .as_ref()
            .ok_or("--frozen requires an existing unified.lock file")?;
        return cmd_sync_frozen(&config, lock, settings, &active_repos);
    }

    let max_parallel = settings.parallel.unwrap_or(4);

    // Collect work items
    let repos: Vec<(String, un_core::Repo)> = active_repos.into_iter().collect();

    if repos.is_empty() {
        println!("No repos to sync.");
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
    drop(errs);

    // ── Sync artifacts, tools, and apps ──
    let cache = Cache::new()?;
    let engine = DownloadEngine::new();

    let active_artifacts = config.artifacts_for_collection(active_collection.as_deref())?;
    let locked_artifacts = sync_downloadables(
        &engine,
        &cache,
        "artifacts",
        &active_artifacts
            .iter()
            .map(|(n, a)| {
                (
                    n.as_str(),
                    a.source(),
                    a.version.as_deref(),
                    a.sha256.as_deref(),
                    &a.platform,
                    Some(a.path.as_str()),
                )
            })
            .collect::<Vec<_>>(),
    )?;

    let active_tools = config.tools_for_collection(active_collection.as_deref())?;
    let empty_platform: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let locked_tools_raw = sync_downloadables(
        &engine,
        &cache,
        "tools",
        &active_tools
            .iter()
            .map(|(n, t)| {
                (
                    n.as_str(),
                    t.source(),
                    t.version.as_deref(),
                    None::<&str>,
                    &empty_platform,
                    None::<&str>,
                )
            })
            .collect::<Vec<_>>(),
    )?;
    let locked_tools: std::collections::HashMap<String, LockedTool> = locked_tools_raw
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                LockedTool {
                    source: v.source,
                    version: v.version,
                    url: v.url,
                    sha256: v.sha256,
                },
            )
        })
        .collect();

    let active_apps: std::collections::HashMap<String, App> = config.apps.clone();
    let locked_apps_raw = sync_downloadables(
        &engine,
        &cache,
        "apps",
        &active_apps
            .iter()
            .map(|(n, a)| {
                (
                    n.as_str(),
                    a.source(),
                    a.version.as_deref(),
                    None::<&str>,
                    &empty_platform,
                    None::<&str>,
                )
            })
            .collect::<Vec<_>>(),
    )?;
    let locked_apps: std::collections::HashMap<String, LockedApp> = locked_apps_raw
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                LockedApp {
                    source: v.source,
                    version: v.version,
                    url: v.url,
                    sha256: v.sha256,
                },
            )
        })
        .collect();

    // Place artifacts in workspace paths
    for (name, artifact) in &active_artifacts {
        if let Some(locked) = locked_artifacts.get(name.as_str()) {
            let cache_dir = un_download::DownloadEngine::cache_path(
                &cache,
                "artifacts",
                name,
                &locked.version,
            );
            let workspace_path = std::env::current_dir()?.join(&artifact.path);
            link_or_copy_artifact(&cache_dir, &workspace_path)?;
        }
    }

    // Write lock file with config hash
    // When syncing a collection, merge new results into existing lock (don't drop unsynced repos)
    let existing_lock_data = existing_lock;
    let mut all_locked_repos = existing_lock_data
        .as_ref()
        .map(|l| l.repos.clone())
        .unwrap_or_default();
    all_locked_repos.extend(Arc::try_unwrap(locked_repos).unwrap().into_inner().unwrap());

    let mut all_locked_artifacts = existing_lock_data
        .as_ref()
        .map(|l| l.artifacts.clone())
        .unwrap_or_default();
    all_locked_artifacts.extend(locked_artifacts);

    let mut all_locked_tools = existing_lock_data
        .as_ref()
        .map(|l| l.tools.clone())
        .unwrap_or_default();
    all_locked_tools.extend(locked_tools);

    let mut all_locked_apps = existing_lock_data
        .as_ref()
        .map(|l| l.apps.clone())
        .unwrap_or_default();
    all_locked_apps.extend(locked_apps);

    let lock_file = LockFile {
        version: 1,
        config_hash: Some(current_hash),
        repos: all_locked_repos,
        artifacts: all_locked_artifacts,
        tools: all_locked_tools,
        apps: all_locked_apps,
    };
    let lock_toml = toml::to_string(&lock_file)?;
    std::fs::write("unified.lock", lock_toml)?;
    println!("Updated unified.lock");

    // Auto-update .gitignore if enabled (only paths that were actually synced)
    if settings.manage_gitignore.unwrap_or(true) {
        let synced_repos = config.repos_for_collection(active_collection.as_deref())?;
        update_gitignore_for_repos(&synced_repos)?;
    }

    // Auto-update .vscode/settings.json if enabled
    if settings.manage_vscode.unwrap_or(true) {
        let synced_repos = config.repos_for_collection(active_collection.as_deref())?;
        update_vscode_settings_for_repos(&synced_repos)?;
    }

    // Generate launcher scripts if configured
    if let Some(ref launcher) = config.launcher
        && launcher.generate.unwrap_or(false) {
            generate_launcher_scripts(launcher, &config)?;
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
    _config: &Config,
    lock: &LockFile,
    _settings: &Settings,
    active_repos: &std::collections::HashMap<String, un_core::Repo>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cache = Cache::new()?;

    for (name, repo) in active_repos {
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

fn cmd_update(
    cli_collection: Option<&str>,
    cli_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string("unified.toml")?;
    let config: Config = toml::from_str(&config_content)?;
    let cache = Cache::new()?;
    let default_settings = Settings::default();
    let settings = config.settings.as_ref().unwrap_or(&default_settings);

    // Validate collections
    let validation_errors = config.validate_collections();
    if !validation_errors.is_empty() {
        for e in &validation_errors {
            eprintln!("error: {}", e);
        }
        return Err(format!("{} collection validation error(s)", validation_errors.len()).into());
    }

    let active_collection = resolve_active_collection(cli_collection, cli_all);
    if let Some(ref name) = active_collection {
        println!("Using collection: {}", name);
    }

    let active_repos = config.repos_for_collection(active_collection.as_deref())?;

    let existing_lock: Option<LockFile> = if std::path::Path::new("unified.lock").exists() {
        Some(toml::from_str(&std::fs::read_to_string("unified.lock")?)?)
    } else {
        None
    };

    let mut updated = 0;
    let mut locked_repos = existing_lock
        .as_ref()
        .map(|l| l.repos.clone())
        .unwrap_or_default();

    for (name, repo) in &active_repos {
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
        artifacts: existing_lock
            .as_ref()
            .map(|l| l.artifacts.clone())
            .unwrap_or_default(),
        tools: existing_lock
            .as_ref()
            .map(|l| l.tools.clone())
            .unwrap_or_default(),
        apps: existing_lock
            .as_ref()
            .map(|l| l.apps.clone())
            .unwrap_or_default(),
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

fn cmd_status(
    cli_collection: Option<&str>,
    cli_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    // Validate collections
    let validation_errors = config.validate_collections();
    if !validation_errors.is_empty() {
        for e in &validation_errors {
            eprintln!("error: {}", e);
        }
        return Err(format!("{} collection validation error(s)", validation_errors.len()).into());
    }

    let active_collection = resolve_active_collection(cli_collection, cli_all);
    if let Some(ref name) = active_collection {
        println!("Using collection: {}", name);
    }

    let active_repos = config.repos_for_collection(active_collection.as_deref())?;

    let existing_lock: Option<LockFile> = if std::path::Path::new("unified.lock").exists() {
        Some(toml::from_str(&std::fs::read_to_string("unified.lock")?)?)
    } else {
        None
    };

    if active_repos.is_empty() {
        println!("No repos configured.");
        return Ok(());
    }

    let mut names: Vec<&String> = active_repos.keys().collect();
    names.sort();

    for name in names {
        let repo = &active_repos[name];
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

/// Look up a repo by name, validate it exists and is in worktree mode.
fn require_worktree_repo<'a>(
    config: &'a Config,
    name: &str,
) -> Result<(&'a un_core::Repo, std::path::PathBuf), Box<dyn std::error::Error>> {
    let repo = config
        .repos
        .get(name)
        .ok_or_else(|| format!("repo '{}' not found in unified.toml", name))?;

    let mode = resolve_checkout_mode(repo);
    match mode {
        CheckoutMode::Copy | CheckoutMode::FilteredCopy { .. } => {
            return Err(format!(
                "repo '{}' is in copy mode, switch to worktree for git operations",
                name
            )
            .into());
        }
        _ => {}
    }

    let workspace_path = std::env::current_dir()?.join(&repo.path);
    if !workspace_path.exists() {
        return Err(format!("repo '{}' is not synced yet (run `un sync` first)", name).into());
    }

    Ok((repo, workspace_path))
}

fn cmd_branch(repo_name: &str, branch_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let (_repo, workspace_path) = require_worktree_repo(&config, repo_name)?;

    let status = std::process::Command::new("git")
        .args(["checkout", "-b", branch_name])
        .current_dir(&workspace_path)
        .status()?;

    if !status.success() {
        return Err(format!("git checkout -b failed for '{}'", repo_name).into());
    }

    println!("Created branch '{}' in {}", branch_name, repo_name);
    Ok(())
}

fn cmd_commit(repo_name: &str, message: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let (_repo, workspace_path) = require_worktree_repo(&config, repo_name)?;

    let mut cmd = std::process::Command::new("git");
    cmd.arg("commit").arg("-a");
    if let Some(msg) = message {
        cmd.args(["-m", msg]);
    }
    // Inherit stdin/stdout/stderr so $EDITOR works when no -m is given
    let status = cmd.current_dir(&workspace_path).status()?;

    if !status.success() {
        return Err(format!("git commit failed for '{}'", repo_name).into());
    }

    Ok(())
}

fn cmd_push(repo_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let (_repo, workspace_path) = require_worktree_repo(&config, repo_name)?;

    let status = std::process::Command::new("git")
        .arg("push")
        .current_dir(&workspace_path)
        .status()?;

    if !status.success() {
        return Err(format!("git push failed for '{}'", repo_name).into());
    }

    Ok(())
}

fn cmd_diff(
    repo_name: Option<&str>,
    cli_collection: Option<&str>,
    cli_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    if let Some(name) = repo_name {
        // Single repo
        let (_repo, workspace_path) = require_worktree_repo(&config, name)?;
        let status = std::process::Command::new("git")
            .arg("diff")
            .current_dir(&workspace_path)
            .status()?;
        if !status.success() {
            return Err(format!("git diff failed for '{}'", name).into());
        }
    } else {
        // All repos (respecting collection)
        let active_collection = resolve_active_collection(cli_collection, cli_all);
        let active_repos = config.repos_for_collection(active_collection.as_deref())?;
        let mut names: Vec<&String> = active_repos.keys().collect();
        names.sort();

        for name in names {
            let repo = &active_repos[name];
            let mode = resolve_checkout_mode(repo);
            if matches!(mode, CheckoutMode::Copy | CheckoutMode::FilteredCopy { .. }) {
                continue; // Skip copy-mode repos silently
            }
            let workspace_path = std::env::current_dir()?.join(&repo.path);
            if !workspace_path.exists() {
                continue;
            }

            let output = std::process::Command::new("git")
                .arg("diff")
                .current_dir(&workspace_path)
                .output()?;

            if !output.stdout.is_empty() {
                println!("── {} ──", name);
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
        }
    }

    Ok(())
}

fn cmd_log(repo_name: &str, count: usize) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let (_repo, workspace_path) = require_worktree_repo(&config, repo_name)?;

    let status = std::process::Command::new("git")
        .args(["log", "--oneline", "-n", &count.to_string()])
        .current_dir(&workspace_path)
        .status()?;

    if !status.success() {
        return Err(format!("git log failed for '{}'", repo_name).into());
    }

    Ok(())
}

fn update_gitignore(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    update_gitignore_for_repos(&config.repos)
}

fn update_gitignore_for_repos(
    repos: &std::collections::HashMap<String, un_core::Repo>,
) -> Result<(), Box<dyn std::error::Error>> {
    let gitignore_path = ".gitignore";
    let managed_block_start = "# BEGIN UNIFIED MANAGED BLOCK - DO NOT EDIT\n";
    let managed_block_end = "# END UNIFIED MANAGED BLOCK\n";

    // Collect paths to ignore (include .unified/ for user config)
    let mut ignore_paths: Vec<&str> = repos.values().map(|r| r.path.as_str()).collect();
    ignore_paths.sort();
    // Always ignore the .unified/ directory
    if !ignore_paths.contains(&".unified/") {
        ignore_paths.push(".unified/");
        ignore_paths.sort();
    }

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
    update_vscode_settings_for_repos(&config.repos)
}

fn update_vscode_settings_for_repos(
    repos: &std::collections::HashMap<String, un_core::Repo>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    let mut ignored_repos: Vec<&str> = repos.values().map(|r| r.path.as_str()).collect();
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

fn cmd_collection(sub: CollectionCommand) -> Result<(), Box<dyn std::error::Error>> {
    match sub {
        CollectionCommand::Use { name, clear } => cmd_collection_use(name, clear),
        CollectionCommand::List => cmd_collection_list(),
        CollectionCommand::Show { name } => cmd_collection_show(&name),
    }
}

fn cmd_collection_use(name: Option<String>, clear: bool) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = std::env::current_dir()?;

    if clear {
        let mut uc = UserConfig::load(&workspace_root);
        uc.default_collection = None;
        uc.save(&workspace_root)?;
        println!("Cleared default collection.");
        return Ok(());
    }

    let name = name.ok_or("provide a collection name, or use --clear to remove the default")?;

    // Validate that the collection exists
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    if !config.collections.contains_key(&name) {
        return Err(format!(
            "collection \"{}\" not found in unified.toml\nAvailable: {}",
            name,
            config
                .collections
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }

    let mut uc = UserConfig::load(&workspace_root);
    uc.default_collection = Some(name.clone());
    uc.save(&workspace_root)?;
    println!("Default collection set to \"{}\".", name);
    println!("Run `un sync` to sync only this collection's repos.");
    Ok(())
}

fn cmd_collection_list() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    if config.collections.is_empty() {
        println!("No collections defined in unified.toml.");
        return Ok(());
    }

    // Check active collection
    let active = resolve_active_collection(None, false);

    let mut names: Vec<&String> = config.collections.keys().collect();
    names.sort();

    for name in names {
        let coll = &config.collections[name];
        let marker = if active.as_deref() == Some(name.as_str()) {
            " (active)"
        } else {
            ""
        };
        println!("  {} — {} member(s){}", name, coll.member_count(), marker);
    }

    Ok(())
}

fn cmd_collection_show(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    let coll = config
        .collections
        .get(name)
        .ok_or_else(|| format!("collection \"{}\" not found in unified.toml", name))?;

    println!("Collection: {}", name);

    if !coll.repos.is_empty() {
        println!("  Repos:");
        for r in &coll.repos {
            let path = config
                .repos
                .get(r)
                .map(|repo| repo.path.as_str())
                .unwrap_or("(unknown)");
            println!("    {} → {}", r, path);
        }
    }

    if !coll.artifacts.is_empty() {
        println!("  Artifacts:");
        for a in &coll.artifacts {
            println!("    {}", a);
        }
    }

    if !coll.tools.is_empty() {
        println!("  Tools:");
        for t in &coll.tools {
            println!("    {}", t);
        }
    }

    if coll.member_count() == 0 {
        println!("  (empty)");
    }

    Ok(())
}

// ── Phase 5: Download, tool, app, task, setup, launcher ──

/// A downloadable item spec: (name, source, version_req, sha256, platform_map, workspace_path).
type DownloadSpec<'a> = (
    &'a str,
    Option<DownloadSource>,
    Option<&'a str>,
    Option<&'a str>,
    &'a std::collections::HashMap<String, String>,
    Option<&'a str>,
);

/// Sync downloadable items (artifacts, tools, or apps).
/// Returns a map of locked entries for the successfully downloaded items.
fn sync_downloadables(
    engine: &DownloadEngine,
    cache: &Cache,
    category: &str,
    items: &[DownloadSpec<'_>],
) -> Result<std::collections::HashMap<String, LockedArtifact>, Box<dyn std::error::Error>> {
    let mut locked = std::collections::HashMap::new();

    for &(name, ref source, version_str, expected_sha256, platform_map, _workspace_path) in items {
        let Some(source) = source else {
            eprintln!("  {} — no source configured, skipping", name);
            continue;
        };

        match source {
            DownloadSource::GitHub { owner_repo } => {
                let version_req_str = version_str.unwrap_or("*");
                let version_req = semver::VersionReq::parse(version_req_str).map_err(|e| {
                    format!("invalid version requirement '{}' for {}: {}", version_req_str, name, e)
                })?;

                // Check if already cached
                let releases = GitHubProvider::get_releases(engine, owner_repo)?;
                let resolved = DownloadEngine::choose_asset(&releases, &version_req, platform_map)
                    .ok_or_else(|| {
                        format!(
                            "no compatible release found for {} ({} {})",
                            name, owner_repo, version_req_str
                        )
                    })?;

                let cache_dir =
                    DownloadEngine::cache_path(cache, category, name, &resolved.version);

                if cache_dir.exists() {
                    println!(
                        "  Cached    {} v{} (already downloaded)",
                        name, resolved.version
                    );
                } else {
                    println!(
                        "  Download  {} v{} (GitHub: {})",
                        name, resolved.version, owner_repo
                    );
                    let data = engine.download_bytes(&resolved.url)?;
                    let sha = DownloadEngine::sha256(&data);

                    if let Some(expected) = expected_sha256
                        && sha != expected {
                            return Err(format!(
                                "SHA-256 mismatch for {}: expected {}, got {}",
                                name, expected, sha
                            )
                            .into());
                        }

                    un_download::extract_archive(&data, &resolved.asset_name, &cache_dir)?;
                }

                let sha = expected_sha256.unwrap_or("").to_string();
                locked.insert(
                    name.to_string(),
                    LockedArtifact {
                        source: format!("github:{}", owner_repo),
                        version: resolved.version,
                        url: resolved.url,
                        sha256: sha,
                    },
                );
            }
            DownloadSource::Artifactory { path } => {
                // For Artifactory, we need a base URL from env or convention
                let base_url = std::env::var("ARTIFACTORY_URL")
                    .unwrap_or_else(|_| "https://artifactory.example.com".to_string());

                let version_req_str = version_str.unwrap_or("*");
                let version_req = semver::VersionReq::parse(version_req_str).map_err(|e| {
                    format!("invalid version requirement '{}' for {}: {}", version_req_str, name, e)
                })?;

                let releases = ArtifactoryProvider::get_releases(engine, &base_url, path)?;
                let resolved = DownloadEngine::choose_asset(&releases, &version_req, platform_map)
                    .ok_or_else(|| {
                        format!(
                            "no compatible release found for {} ({} {})",
                            name, path, version_req_str
                        )
                    })?;

                let cache_dir =
                    DownloadEngine::cache_path(cache, category, name, &resolved.version);

                if cache_dir.exists() {
                    println!(
                        "  Cached    {} v{} (already downloaded)",
                        name, resolved.version
                    );
                } else {
                    println!(
                        "  Download  {} v{} (Artifactory: {})",
                        name, resolved.version, path
                    );
                    let data = ArtifactoryProvider::download_asset(engine, &resolved.url)?;
                    let sha = DownloadEngine::sha256(&data);

                    if let Some(expected) = expected_sha256
                        && sha != expected {
                            return Err(format!(
                                "SHA-256 mismatch for {}: expected {}, got {}",
                                name, expected, sha
                            )
                            .into());
                        }

                    un_download::extract_archive(&data, &resolved.asset_name, &cache_dir)?;
                }

                let sha = expected_sha256.unwrap_or("").to_string();
                locked.insert(
                    name.to_string(),
                    LockedArtifact {
                        source: format!("artifactory:{}", path),
                        version: resolved.version,
                        url: resolved.url,
                        sha256: sha,
                    },
                );
            }
            DownloadSource::Url { url } => {
                // Direct URL — no version resolution, just download
                let cache_dir = DownloadEngine::cache_path(cache, category, name, "latest");

                if cache_dir.exists() {
                    println!("  Cached    {} (already downloaded)", name);
                } else {
                    println!("  Download  {} ({})", name, url);
                    let data = HttpProvider::download(engine, url, expected_sha256)?;

                    // Infer filename from URL
                    let filename = url
                        .rsplit('/')
                        .next()
                        .unwrap_or("download");
                    un_download::extract_archive(&data, filename, &cache_dir)?;
                }

                let sha = expected_sha256.unwrap_or("").to_string();
                locked.insert(
                    name.to_string(),
                    LockedArtifact {
                        source: format!("url:{}", url),
                        version: "latest".to_string(),
                        url: url.clone(),
                        sha256: sha,
                    },
                );
            }
        }
    }

    Ok(locked)
}

/// Link or copy an artifact from cache to workspace path.
fn link_or_copy_artifact(
    cache_dir: &std::path::Path,
    workspace_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if workspace_path.exists() {
        return Ok(()); // Already placed
    }

    if let Some(parent) = workspace_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // If cache_dir is a directory with exactly one entry, link that entry.
    // Otherwise, create a symlink to the cache directory itself.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(cache_dir, workspace_path)?;
    }
    #[cfg(not(unix))]
    {
        // On Windows, copy instead of symlink  
        copy_dir_recursive(cache_dir, workspace_path)?;
    }

    Ok(())
}

#[cfg(not(unix))]
fn copy_dir_recursive(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

/// Ensure a tool is downloaded and return the path to its cache directory.
fn ensure_tool_downloaded(
    name: &str,
    tool: &Tool,
    cache: &Cache,
) -> Result<(std::path::PathBuf, String), Box<dyn std::error::Error>> {
    let engine = DownloadEngine::new();
    let source = tool.source().ok_or_else(|| format!("tool '{}' has no source configured", name))?;

    match source {
        DownloadSource::GitHub { owner_repo } => {
            let version_req_str = tool.version.as_deref().unwrap_or("*");
            let version_req = semver::VersionReq::parse(version_req_str)?;
            let releases = GitHubProvider::get_releases(&engine, &owner_repo)?;
            let resolved =
                DownloadEngine::choose_asset(&releases, &version_req, &std::collections::HashMap::new())
                    .ok_or_else(|| {
                        format!(
                            "no compatible release found for {} ({} {})",
                            name, owner_repo, version_req_str
                        )
                    })?;

            let cache_dir = DownloadEngine::cache_path(cache, "tools", name, &resolved.version);
            if !cache_dir.exists() {
                println!("  Downloading {} v{}...", name, resolved.version);
                let data = engine.download_bytes(&resolved.url)?;
                un_download::extract_archive(&data, &resolved.asset_name, &cache_dir)?;
            }
            Ok((cache_dir, resolved.version))
        }
        DownloadSource::Artifactory { path } => {
            let base_url = std::env::var("ARTIFACTORY_URL")
                .unwrap_or_else(|_| "https://artifactory.example.com".to_string());
            let version_req_str = tool.version.as_deref().unwrap_or("*");
            let version_req = semver::VersionReq::parse(version_req_str)?;
            let releases = ArtifactoryProvider::get_releases(&engine, &base_url, &path)?;
            let resolved =
                DownloadEngine::choose_asset(&releases, &version_req, &std::collections::HashMap::new())
                    .ok_or_else(|| {
                        format!(
                            "no compatible release found for {} ({} {})",
                            name, path, version_req_str
                        )
                    })?;

            let cache_dir = DownloadEngine::cache_path(cache, "tools", name, &resolved.version);
            if !cache_dir.exists() {
                println!("  Downloading {} v{}...", name, resolved.version);
                let data = ArtifactoryProvider::download_asset(&engine, &resolved.url)?;
                un_download::extract_archive(&data, &resolved.asset_name, &cache_dir)?;
            }
            Ok((cache_dir, resolved.version))
        }
        DownloadSource::Url { url } => {
            let cache_dir = DownloadEngine::cache_path(cache, "tools", name, "latest");
            if !cache_dir.exists() {
                println!("  Downloading {}...", name);
                let data = HttpProvider::download(&engine, &url, None)?;
                let filename = url.rsplit('/').next().unwrap_or("download");
                un_download::extract_archive(&data, filename, &cache_dir)?;
            }
            Ok((cache_dir, "latest".to_string()))
        }
    }
}

/// Find the executable in a cache directory — looks for files with exec permission or the tool name.
fn find_executable(
    cache_dir: &std::path::Path,
    tool_name: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    // First, try exact name match
    let direct = cache_dir.join(tool_name);
    if direct.exists() && direct.is_file() {
        return Ok(direct);
    }

    // Try with exe suffix on Windows
    #[cfg(target_os = "windows")]
    {
        let with_exe = cache_dir.join(format!("{}.exe", tool_name));
        if with_exe.exists() {
            return Ok(with_exe);
        }
    }

    // Walk the directory for an executable containing the tool name
    if cache_dir.is_dir() {
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                if fname.contains(tool_name) {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(meta) = std::fs::metadata(&path)
                            && meta.permissions().mode() & 0o111 != 0 {
                                return Ok(path);
                            }
                    }
                    #[cfg(not(unix))]
                    {
                        if fname.ends_with(".exe") || fname.ends_with(".cmd") || fname.ends_with(".bat") {
                            return Ok(path);
                        }
                    }
                }
            }
            // Check one level deep (common in tar.gz extractions)
            if path.is_dir()
                && let Ok(sub) = find_executable(&path, tool_name) {
                    return Ok(sub);
                }
        }
    }

    Err(format!(
        "could not find executable '{}' in {}",
        tool_name,
        cache_dir.display()
    )
    .into())
}

fn cmd_run(tool_name: &str, extra_args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let tool = config
        .tools
        .get(tool_name)
        .ok_or_else(|| format!("tool '{}' not found in unified.toml [tools]", tool_name))?;

    let cache = Cache::new()?;
    let (cache_dir, _version) = ensure_tool_downloaded(tool_name, tool, &cache)?;
    let exe = find_executable(&cache_dir, tool_name)?;

    // Build args: tool's default args + user args
    let mut args = tool.args.clone();
    args.extend(extra_args);

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args);

    // Set environment variables from tool config
    for (k, v) in &tool.env {
        cmd.env(k, v);
    }

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn cmd_tool(sub: ToolCommand) -> Result<(), Box<dyn std::error::Error>> {
    match sub {
        ToolCommand::Install { name } => cmd_tool_install(name),
        ToolCommand::List => cmd_tool_list(),
    }
}

fn cmd_tool_install(name: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let cache = Cache::new()?;

    let bin_dir = cache.bin();
    std::fs::create_dir_all(&bin_dir)?;

    let tools_to_install: Vec<(String, &Tool)> = if let Some(ref name) = name {
        let tool = config
            .tools
            .get(name)
            .ok_or_else(|| format!("tool '{}' not found in unified.toml [tools]", name))?;
        vec![(name.clone(), tool)]
    } else {
        config.tools.iter().map(|(n, t)| (n.clone(), t)).collect()
    };

    for (tool_name, tool) in &tools_to_install {
        let (cache_dir, version) = ensure_tool_downloaded(tool_name, tool, &cache)?;
        let exe = find_executable(&cache_dir, tool_name)?;

        let link_path = bin_dir.join(tool_name);
        #[cfg(unix)]
        {
            // Remove existing symlink if present
            let _ = std::fs::remove_file(&link_path);
            std::os::unix::fs::symlink(&exe, &link_path)?;
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_file(&link_path);
            std::fs::copy(&exe, &link_path)?;
        }

        println!("  Installed {} v{} → {}", tool_name, version, link_path.display());
    }

    println!(
        "\nAdd {} to your PATH to use installed tools directly.",
        bin_dir.display()
    );
    Ok(())
}

fn cmd_tool_list() -> Result<(), Box<dyn std::error::Error>> {
    let cache = Cache::new()?;
    let tools_dir = cache.tools();
    let bin_dir = cache.bin();

    if !tools_dir.exists() {
        println!("No tools cached.");
        return Ok(());
    }

    println!("Cached tools:");
    for entry in std::fs::read_dir(&tools_dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            let tool_name = entry.file_name().to_string_lossy().to_string();
            let mut versions = Vec::new();
            for ver_entry in std::fs::read_dir(entry.path())? {
                let ver_entry = ver_entry?;
                if ver_entry.path().is_dir() {
                    versions.push(ver_entry.file_name().to_string_lossy().to_string());
                }
            }
            let installed = bin_dir.join(&tool_name).exists();
            let marker = if installed { " (installed)" } else { "" };
            println!("  {} — {}{}", tool_name, versions.join(", "), marker);
        }
    }

    Ok(())
}

fn cmd_app(app_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
    let app = config
        .apps
        .get(app_name)
        .ok_or_else(|| format!("app '{}' not found in unified.toml [apps]", app_name))?;

    let cache = Cache::new()?;
    let engine = DownloadEngine::new();
    let source = app.source().ok_or_else(|| format!("app '{}' has no source configured", app_name))?;

    let (cache_dir, version) = match source {
        DownloadSource::GitHub { owner_repo } => {
            let version_req_str = app.version.as_deref().unwrap_or("*");
            let version_req = semver::VersionReq::parse(version_req_str)?;
            let releases = GitHubProvider::get_releases(&engine, &owner_repo)?;
            let resolved =
                DownloadEngine::choose_asset(&releases, &version_req, &std::collections::HashMap::new())
                    .ok_or_else(|| {
                        format!("no compatible release found for {} ({})", app_name, owner_repo)
                    })?;
            let dir = DownloadEngine::cache_path(&cache, "apps", app_name, &resolved.version);
            if !dir.exists() {
                println!("  Downloading {} v{}...", app_name, resolved.version);
                let data = engine.download_bytes(&resolved.url)?;
                un_download::extract_archive(&data, &resolved.asset_name, &dir)?;
            }
            (dir, resolved.version)
        }
        DownloadSource::Artifactory { path } => {
            let base_url = std::env::var("ARTIFACTORY_URL")
                .unwrap_or_else(|_| "https://artifactory.example.com".to_string());
            let version_req_str = app.version.as_deref().unwrap_or("*");
            let version_req = semver::VersionReq::parse(version_req_str)?;
            let releases = ArtifactoryProvider::get_releases(&engine, &base_url, &path)?;
            let resolved =
                DownloadEngine::choose_asset(&releases, &version_req, &std::collections::HashMap::new())
                    .ok_or_else(|| {
                        format!("no compatible release found for {} ({})", app_name, path)
                    })?;
            let dir = DownloadEngine::cache_path(&cache, "apps", app_name, &resolved.version);
            if !dir.exists() {
                println!("  Downloading {} v{}...", app_name, resolved.version);
                let data = ArtifactoryProvider::download_asset(&engine, &resolved.url)?;
                un_download::extract_archive(&data, &resolved.asset_name, &dir)?;
            }
            (dir, resolved.version)
        }
        DownloadSource::Url { url } => {
            let dir = DownloadEngine::cache_path(&cache, "apps", app_name, "latest");
            if !dir.exists() {
                println!("  Downloading {}...", app_name);
                let data = HttpProvider::download(&engine, &url, None)?;
                let filename = url.rsplit('/').next().unwrap_or("download");
                un_download::extract_archive(&data, filename, &dir)?;
            }
            (dir, "latest".to_string())
        }
    };

    // Try to find and launch the app executable
    let exe = find_executable(&cache_dir, app_name)?;
    println!("Launching {} v{}...", app_name, version);
    let mut cmd = std::process::Command::new(&exe);
    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn cmd_task(task_name: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    match task_name {
        None => {
            // List all tasks
            if config.tasks.is_empty() {
                println!("No tasks defined in unified.toml.");
                return Ok(());
            }
            println!("Tasks:");
            let mut names: Vec<&String> = config.tasks.keys().collect();
            names.sort();
            for name in names {
                let task = &config.tasks[name];
                let desc = task
                    .description
                    .as_deref()
                    .unwrap_or(&task.cmd);
                println!("  {:<20} {}", name, desc);
            }
            Ok(())
        }
        Some(name) => {
            // Run a specific task (with dependency resolution)
            run_task(&config, name, &mut std::collections::HashSet::new())
        }
    }
}

/// Run a task with topological dependency resolution. Tracks visited tasks to detect cycles.
fn run_task(
    config: &Config,
    name: &str,
    visited: &mut std::collections::HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !visited.insert(name.to_string()) {
        return Err(format!("circular dependency detected for task '{}'", name).into());
    }

    let task = config
        .tasks
        .get(name)
        .ok_or_else(|| format!("task '{}' not found in unified.toml [tasks]", name))?;

    // Run dependencies first
    for dep in &task.depends {
        run_task(config, dep, visited)?;
    }

    println!("▶ Running task: {}", name);
    let status = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", &task.cmd])
            .status()?
    } else {
        std::process::Command::new("sh")
            .args(["-c", &task.cmd])
            .status()?
    };

    if !status.success() {
        return Err(format!("task '{}' failed with exit code {:?}", name, status.code()).into());
    }

    Ok(())
}

fn cmd_setup() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    let setup = config
        .setup
        .as_ref()
        .ok_or("no [setup] section found in unified.toml")?;

    println!("Running setup commands...");
    for (i, cmd_str) in setup.run.iter().enumerate() {
        println!("  [{}/{}] {}", i + 1, setup.run.len(), cmd_str);
        let status = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", cmd_str])
                .status()?
        } else {
            std::process::Command::new("sh")
                .args(["-c", cmd_str])
                .status()?
        };
        if !status.success() {
            eprintln!(
                "  Warning: setup command failed (exit {:?}): {}",
                status.code(),
                cmd_str
            );
            // Continue with remaining commands — setup is best-effort
        }
    }

    println!("Setup complete.");
    Ok(())
}

fn cmd_launch() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;

    let launcher = config
        .launcher
        .as_ref()
        .ok_or("no [launcher] section found in unified.toml")?;

    if launcher.entries.is_empty() {
        println!("No launcher entries defined.");
        return Ok(());
    }

    println!("Launcher Menu:");
    println!();
    for (i, entry) in launcher.entries.iter().enumerate() {
        let icon = entry.icon.as_deref().unwrap_or("▸");
        println!("  {} [{}] {}", icon, i + 1, entry.name);
    }
    println!();
    print!("Select entry (1-{}): ", launcher.entries.len());
    std::io::Write::flush(&mut std::io::stdout())?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let choice: usize = input
        .trim()
        .parse()
        .map_err(|_| "invalid selection")?;

    if choice == 0 || choice > launcher.entries.len() {
        return Err("selection out of range".into());
    }

    let entry = &launcher.entries[choice - 1];

    if let Some(ref app_name) = entry.app {
        cmd_app(app_name)?;
    } else if let Some(ref task_name) = entry.task {
        cmd_task(Some(task_name))?;
    } else if let Some(ref cmd_str) = entry.cmd {
        let status = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", cmd_str])
                .status()?
        } else {
            std::process::Command::new("sh")
                .args(["-c", cmd_str])
                .status()?
        };
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
    } else {
        println!("Entry '{}' has no action configured.", entry.name);
    }

    Ok(())
}

/// Generate launch.sh and launch.bat scripts from launcher config.
fn generate_launcher_scripts(
    launcher: &un_core::Launcher,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    // Generate launch.sh
    let mut sh = String::from("#!/usr/bin/env bash\n");
    sh.push_str("# Generated by `un sync` — do not edit\n");
    sh.push_str("set -e\n\n");
    sh.push_str("echo \"Launcher Menu:\"\n");
    sh.push_str("echo\n");

    for (i, entry) in launcher.entries.iter().enumerate() {
        let icon = entry.icon.as_deref().unwrap_or("▸");
        sh.push_str(&format!(
            "echo \"  {} [{}] {}\"\n",
            icon,
            i + 1,
            entry.name
        ));
    }

    sh.push_str("echo\n");
    sh.push_str(&format!(
        "read -p \"Select entry (1-{}): \" choice\n",
        launcher.entries.len()
    ));
    sh.push_str("case $choice in\n");

    for (i, entry) in launcher.entries.iter().enumerate() {
        sh.push_str(&format!("  {})\n", i + 1));
        if let Some(ref app_name) = entry.app {
            sh.push_str(&format!("    un app {}\n", app_name));
        } else if let Some(ref task_name) = entry.task {
            sh.push_str(&format!("    un task {}\n", task_name));
        } else if let Some(ref cmd_str) = entry.cmd {
            sh.push_str(&format!("    {}\n", cmd_str));
        }
        sh.push_str("    ;;\n");
    }

    sh.push_str("  *) echo \"Invalid selection\" ;;\n");
    sh.push_str("esac\n");

    std::fs::write("launch.sh", &sh)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions("launch.sh", std::fs::Permissions::from_mode(0o755))?;
    }

    // Generate launch.bat
    let mut bat = String::from("@echo off\r\n");
    bat.push_str("REM Generated by `un sync` -- do not edit\r\n\r\n");
    bat.push_str("echo Launcher Menu:\r\n");
    bat.push_str("echo.\r\n");

    for (i, entry) in launcher.entries.iter().enumerate() {
        let icon = entry.icon.as_deref().unwrap_or(">");
        bat.push_str(&format!(
            "echo   {} [{}] {}\r\n",
            icon,
            i + 1,
            entry.name
        ));
    }

    bat.push_str("echo.\r\n");
    bat.push_str(&format!(
        "set /p choice=\"Select entry (1-{}): \"\r\n",
        launcher.entries.len()
    ));

    for (i, entry) in launcher.entries.iter().enumerate() {
        bat.push_str(&format!("if \"%choice%\"==\"{}\" (\r\n", i + 1));
        if let Some(ref app_name) = entry.app {
            bat.push_str(&format!("    un app {}\r\n", app_name));
        } else if let Some(ref task_name) = entry.task {
            bat.push_str(&format!("    un task {}\r\n", task_name));
        } else if let Some(ref cmd_str) = entry.cmd {
            bat.push_str(&format!("    {}\r\n", cmd_str));
        }
        bat.push_str(")\r\n");
    }

    std::fs::write("launch.bat", &bat)?;
    println!("Generated launch.sh and launch.bat");

    let _ = config; // Used for future extensions
    Ok(())
}
