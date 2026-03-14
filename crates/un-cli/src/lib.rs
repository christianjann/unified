use clap::{Parser, Subcommand};
use un_core::{Config, Settings, LockFile, LockedRepo, GitReference};
use un_cache::Cache;
use un_git::{GitRemote, GitDatabase, CheckoutMode, GitCheckout};

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
    Sync,
    /// Update dependencies
    Update,
    /// Show workspace status
    Status,
    /// Add a repository or artifact
    Add,
    /// Remove a repository or artifact
    Remove,
    // Add more commands as stubs
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let config = Config {
                workspace: un_core::Workspace {
                    name: "my-workspace".to_string(),
                    members: None,
                    exclude: None,
                },
                settings: Some(Settings::default()),
                repos: std::collections::HashMap::new(),
            };
            let toml = toml::to_string(&config)?;
            std::fs::write("unified.toml", toml)?;
            println!("Created unified.toml");
        }
        Commands::Sync => {
            let config: Config = toml::from_str(&std::fs::read_to_string("unified.toml")?)?;
            let cache = Cache::new()?;
            let default_settings = Settings::default();
            let settings = config.settings.as_ref().unwrap_or(&default_settings);
            
            let mut locked_repos = std::collections::HashMap::new();
            
            for (name, repo) in &config.repos {
                let remote = GitRemote::new(&repo.url);
                let database = GitDatabase::new(&cache, name, &repo.url)?;
                let reference = if let Some(branch) = &repo.branch {
                    GitReference::Branch(branch.clone())
                } else if let Some(tag) = &repo.tag {
                    GitReference::Tag(tag.clone())
                } else if let Some(rev) = &repo.rev {
                    GitReference::Rev(rev.clone())
                } else {
                    GitReference::DefaultBranch
                };
                let shallow = repo.shallow.unwrap_or(settings.shallow.unwrap_or(false));
                let oid = database.fetch(&remote, &reference, shallow, settings.git_fetch_with_cli.unwrap_or(false))?;
                
                let mode = if let Some(checkout) = &repo.checkout {
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
                        _ => {
                            if repo.include.is_some() || repo.exclude.is_some() {
                                CheckoutMode::SparseWorktree {
                                    includes: repo.include.clone().unwrap_or_default(),
                                    excludes: repo.exclude.clone().unwrap_or_default(),
                                }
                            } else {
                                CheckoutMode::Worktree
                            }
                        }
                    }
                } else {
                    if repo.include.is_some() || repo.exclude.is_some() {
                        CheckoutMode::SparseWorktree {
                            includes: repo.include.clone().unwrap_or_default(),
                            excludes: repo.exclude.clone().unwrap_or_default(),
                        }
                    } else {
                        CheckoutMode::Worktree
                    }
                };
                
                let _checkout = GitCheckout::new(&database, &oid, &std::env::current_dir()?.join(&repo.path), mode)?;
                println!("Checked out {} to {}", name, repo.path);
                
                // Record in lock file
                locked_repos.insert(name.clone(), LockedRepo {
                    url: repo.url.clone(),
                    oid: oid.clone(),
                    reference: reference.clone(),
                });
            }
            
            // Write lock file
            let lock_file = LockFile {
                version: 1,
                repos: locked_repos,
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
        }
        Commands::Update => {
            println!("un update: Not implemented yet");
        }
        Commands::Status => {
            println!("un status: Not implemented yet");
        }
        Commands::Add => {
            println!("un add: Not implemented yet");
        }
        Commands::Remove => {
            println!("un remove: Not implemented yet");
        }
    }
    Ok(())
}

fn update_gitignore(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let gitignore_path = ".gitignore";
    let managed_block_start = "# BEGIN UNIFIED MANAGED BLOCK - DO NOT EDIT\n";
    let managed_block_end = "# END UNIFIED MANAGED BLOCK\n";

    // Collect paths to ignore
    let mut ignore_paths = Vec::new();
    for repo in config.repos.values() {
        ignore_paths.push(repo.path.clone());
    }

    // Read existing .gitignore
    let existing_content = if std::path::Path::new(gitignore_path).exists() {
        std::fs::read_to_string(gitignore_path)?
    } else {
        String::new()
    };

    // Remove all existing managed blocks (there might be multiple due to corruption)
    let mut cleaned_content = existing_content.clone();
    while let Some(start_pos) = cleaned_content.find(managed_block_start) {
        if let Some(end_pos) = cleaned_content[start_pos..].find(managed_block_end) {
            let actual_end_pos = start_pos + end_pos + managed_block_end.len();
            cleaned_content = format!("{}{}",
                &cleaned_content[..start_pos],
                &cleaned_content[actual_end_pos..]
            );
        } else {
            // Malformed block (missing end marker), preserve everything and warn
            eprintln!("Warning: malformed unified managed block in .gitignore (missing end marker), skipping cleanup");
            break;
        }
    }

    // Add new managed block
    let new_content = if cleaned_content.is_empty() {
        format!("{}{}\n{}",
            managed_block_start,
            ignore_paths.join("\n"),
            managed_block_end
        )
    } else {
        format!("{}{}\n{}{}\n{}",
            cleaned_content.trim_end(),
            if cleaned_content.is_empty() || cleaned_content.ends_with('\n') { "" } else { "\n" },
            managed_block_start,
            ignore_paths.join("\n"),
            managed_block_end
        )
    };

    std::fs::write(gitignore_path, new_content)?;
    println!("Updated .gitignore");
    Ok(())
}

fn update_vscode_settings(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Path;
    
    let settings_dir = Path::new(".vscode");
    let settings_path = settings_dir.join("settings.json");
    
    // Create .vscode directory if it doesn't exist
    if !settings_dir.exists() {
        std::fs::create_dir_all(settings_dir)?;
    }
    
    // Read existing settings
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    
    // Collect repo paths to ignore in git
    let mut ignored_repos = Vec::new();
    for repo in config.repos.values() {
        ignored_repos.push(repo.path.clone());
    }
    
    // Update git.ignoredRepositories
    if let serde_json::Value::Object(ref mut map) = settings {
        map.insert("git.ignoredRepositories".to_string(), serde_json::json!(ignored_repos));
    }
    
    // Write back
    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, content)?;
    println!("Updated .vscode/settings.json");
    Ok(())
}