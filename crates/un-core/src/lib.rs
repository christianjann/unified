use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum GitReference {
    Branch(String),
    Tag(String),
    Rev(String),
    DefaultBranch,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub workspace: Workspace,
    pub settings: Option<Settings>,
    #[serde(default)]
    pub repos: HashMap<String, Repo>,
    // Add artifacts, tools, etc. later
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Workspace {
    pub name: String,
    pub members: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub git_fetch_with_cli: Option<bool>,
    pub parallel: Option<usize>,
    pub cache_dir: Option<String>,
    pub shallow: Option<bool>,
    pub manage_gitignore: Option<bool>,
    pub manage_vscode: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Repo {
    pub url: String,
    pub path: String,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    pub checkout: Option<String>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub shallow: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LockFile {
    pub version: u32,
    #[serde(default)]
    pub config_hash: Option<String>,
    #[serde(default)]
    pub repos: HashMap<String, LockedRepo>,
    // Add artifacts, tools, etc. later
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LockedRepo {
    pub url: String,
    pub oid: String,
    pub reference: GitReference,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            git_fetch_with_cli: Some(false),
            parallel: Some(4),
            cache_dir: None,
            shallow: Some(false),
            manage_gitignore: Some(true),
            manage_vscode: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_round_trip() {
        let config = Config {
            workspace: Workspace {
                name: "test-workspace".to_string(),
                members: Some(vec!["components/*".to_string()]),
                exclude: Some(vec!["components/legacy".to_string()]),
            },
            settings: Some(Settings {
                git_fetch_with_cli: Some(true),
                parallel: Some(8),
                cache_dir: Some("~/.custom".to_string()),
                shallow: Some(true),
                manage_gitignore: Some(false),
                manage_vscode: Some(false),
            }),
            repos: HashMap::new(),
        };

        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.workspace.name, deserialized.workspace.name);
        assert_eq!(config.workspace.members, deserialized.workspace.members);
        assert_eq!(config.workspace.exclude, deserialized.workspace.exclude);
        assert_eq!(
            config.settings.as_ref().unwrap().git_fetch_with_cli,
            deserialized.settings.as_ref().unwrap().git_fetch_with_cli
        );
    }

    #[test]
    fn test_lock_file_serialization() {
        let lock = LockFile {
            version: 1,
            config_hash: None,
            repos: HashMap::new(),
        };
        let toml_str = toml::to_string(&lock).unwrap();
        let deserialized: LockFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(lock.version, deserialized.version);
    }
}
