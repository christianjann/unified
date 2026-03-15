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
    #[serde(default)]
    pub collections: HashMap<String, Collection>,
    // Add artifacts, tools, etc. later
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Collection {
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// User-local config stored in `.unified/user.toml` (git-ignored).
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct UserConfig {
    #[serde(rename = "default-collection", skip_serializing_if = "Option::is_none")]
    pub default_collection: Option<String>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

impl Config {
    /// Validate that all names referenced by collections exist in the corresponding
    /// top-level sections. Returns a list of errors (empty = valid).
    pub fn validate_collections(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for (coll_name, coll) in &self.collections {
            for repo_name in &coll.repos {
                if !self.repos.contains_key(repo_name) {
                    let suggestion = find_similar(repo_name, self.repos.keys());
                    let mut msg = format!(
                        "collection \"{}\" references unknown repo \"{}\"",
                        coll_name, repo_name
                    );
                    if let Some(s) = suggestion {
                        msg.push_str(&format!("\n  → did you mean \"{}\"?", s));
                    }
                    errors.push(msg);
                }
            }
            // artifacts and tools validation will be added when those sections exist
        }
        errors
    }

    /// Return a filtered view of repos based on the active collection.
    /// If `collection_name` is None, returns all repos.
    pub fn repos_for_collection(
        &self,
        collection_name: Option<&str>,
    ) -> Result<HashMap<String, Repo>, String> {
        match collection_name {
            None => Ok(self.repos.clone()),
            Some(name) => {
                let coll = self
                    .collections
                    .get(name)
                    .ok_or_else(|| format!("collection \"{}\" not found in unified.toml", name))?;
                let filtered: HashMap<String, Repo> = coll
                    .repos
                    .iter()
                    .filter_map(|repo_name| {
                        self.repos
                            .get(repo_name)
                            .map(|r| (repo_name.clone(), r.clone()))
                    })
                    .collect();
                Ok(filtered)
            }
        }
    }
}

impl Collection {
    pub fn member_count(&self) -> usize {
        self.repos.len() + self.artifacts.len() + self.tools.len()
    }
}

/// Find the most similar string from candidates (simple edit-distance heuristic).
fn find_similar<'a, I>(target: &str, candidates: I) -> Option<String>
where
    I: Iterator<Item = &'a String>,
{
    let target_lower = target.to_lowercase();
    candidates
        .filter_map(|c| {
            let c_lower = c.to_lowercase();
            let dist = levenshtein(&target_lower, &c_lower);
            if dist <= 2 {
                Some((dist, c.clone()))
            } else {
                None
            }
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, name)| name)
}

/// Simple Levenshtein distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in dp.iter_mut().enumerate().take(a.len() + 1) {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate().take(b.len() + 1) {
        *val = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a.len()][b.len()]
}

impl UserConfig {
    /// Load from `.unified/user.toml`, returning default if file doesn't exist.
    pub fn load(workspace_root: &std::path::Path) -> Self {
        let path = workspace_root.join(".unified").join("user.toml");
        if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| toml::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Save to `.unified/user.toml`, creating the directory if needed.
    pub fn save(&self, workspace_root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let dir = workspace_root.join(".unified");
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
        }
        let content = toml::to_string(self)?;
        std::fs::write(dir.join("user.toml"), content)?;
        Ok(())
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
            collections: HashMap::new(),
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

    #[test]
    fn test_collection_parsing() {
        let toml_str = r#"
            [workspace]
            name = "test"

            [repos.firmware]
            url = "https://example.com/firmware.git"
            path = "firmware"

            [repos.protocol]
            url = "https://example.com/protocol.git"
            path = "protocol"

            [repos.web-ui]
            url = "https://example.com/web-ui.git"
            path = "web-ui"

            [collections.embedded]
            repos = ["firmware", "protocol"]

            [collections.frontend]
            repos = ["web-ui"]
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.collections.len(), 2);
        assert_eq!(
            config.collections["embedded"].repos,
            vec!["firmware", "protocol"]
        );
        assert_eq!(config.collections["frontend"].repos, vec!["web-ui"]);
    }

    #[test]
    fn test_collection_validation_valid() {
        let mut repos = HashMap::new();
        repos.insert(
            "firmware".to_string(),
            Repo {
                url: "https://example.com/firmware.git".to_string(),
                path: "firmware".to_string(),
                branch: None,
                tag: None,
                rev: None,
                checkout: None,
                include: None,
                exclude: None,
                shallow: None,
            },
        );
        let mut collections = HashMap::new();
        collections.insert(
            "team".to_string(),
            Collection {
                repos: vec!["firmware".to_string()],
                artifacts: vec![],
                tools: vec![],
            },
        );
        let config = Config {
            workspace: Workspace {
                name: "test".to_string(),
                members: None,
                exclude: None,
            },
            settings: None,
            repos,
            collections,
        };
        assert!(config.validate_collections().is_empty());
    }

    #[test]
    fn test_collection_validation_unknown_repo() {
        let mut repos = HashMap::new();
        repos.insert(
            "firmware".to_string(),
            Repo {
                url: "https://example.com/firmware.git".to_string(),
                path: "firmware".to_string(),
                branch: None,
                tag: None,
                rev: None,
                checkout: None,
                include: None,
                exclude: None,
                shallow: None,
            },
        );
        let mut collections = HashMap::new();
        collections.insert(
            "team".to_string(),
            Collection {
                repos: vec!["firmwrae".to_string()], // typo
                artifacts: vec![],
                tools: vec![],
            },
        );
        let config = Config {
            workspace: Workspace {
                name: "test".to_string(),
                members: None,
                exclude: None,
            },
            settings: None,
            repos,
            collections,
        };
        let errors = config.validate_collections();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("unknown repo \"firmwrae\""));
        assert!(errors[0].contains("did you mean \"firmware\""));
    }

    #[test]
    fn test_repos_for_collection() {
        let mut repos = HashMap::new();
        repos.insert(
            "a".to_string(),
            Repo {
                url: "u".to_string(),
                path: "a".to_string(),
                branch: None,
                tag: None,
                rev: None,
                checkout: None,
                include: None,
                exclude: None,
                shallow: None,
            },
        );
        repos.insert(
            "b".to_string(),
            Repo {
                url: "u".to_string(),
                path: "b".to_string(),
                branch: None,
                tag: None,
                rev: None,
                checkout: None,
                include: None,
                exclude: None,
                shallow: None,
            },
        );
        let mut collections = HashMap::new();
        collections.insert(
            "partial".to_string(),
            Collection {
                repos: vec!["a".to_string()],
                artifacts: vec![],
                tools: vec![],
            },
        );
        let config = Config {
            workspace: Workspace {
                name: "t".to_string(),
                members: None,
                exclude: None,
            },
            settings: None,
            repos,
            collections,
        };

        // No collection → all repos
        let all = config.repos_for_collection(None).unwrap();
        assert_eq!(all.len(), 2);

        // With collection → filtered
        let filtered = config.repos_for_collection(Some("partial")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert!(filtered.contains_key("a"));

        // Unknown collection → error
        assert!(config.repos_for_collection(Some("nope")).is_err());
    }

    #[test]
    fn test_collection_member_count() {
        let coll = Collection {
            repos: vec!["a".to_string(), "b".to_string()],
            artifacts: vec!["x".to_string()],
            tools: vec![],
        };
        assert_eq!(coll.member_count(), 3);
    }

    #[test]
    fn test_user_config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let uc = UserConfig {
            default_collection: Some("my-team".to_string()),
        };
        uc.save(dir.path()).unwrap();

        let loaded = UserConfig::load(dir.path());
        assert_eq!(loaded.default_collection.as_deref(), Some("my-team"));
    }

    #[test]
    fn test_user_config_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = UserConfig::load(dir.path());
        assert!(loaded.default_collection.is_none());
    }

    #[test]
    fn test_user_config_clear() {
        let dir = tempfile::tempdir().unwrap();
        let uc = UserConfig {
            default_collection: Some("team".to_string()),
        };
        uc.save(dir.path()).unwrap();

        let cleared = UserConfig {
            default_collection: None,
        };
        cleared.save(dir.path()).unwrap();

        let loaded = UserConfig::load(dir.path());
        assert!(loaded.default_collection.is_none());
    }
}
