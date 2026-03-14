use std::path::{Path, PathBuf};
use anyhow::Result;

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new() -> Result<Self> {
        let root = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".unified");
        Ok(Cache { root })
    }

    pub fn with_custom_root(root: PathBuf) -> Self {
        Cache { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn git_db(&self) -> PathBuf {
        self.root.join("git").join("db")
    }

    pub fn git_checkouts(&self) -> PathBuf {
        self.root.join("git").join("checkouts")
    }

    pub fn artifacts(&self) -> PathBuf {
        self.root.join("artifacts")
    }

    pub fn tools(&self) -> PathBuf {
        self.root.join("tools")
    }

    pub fn apps(&self) -> PathBuf {
        self.root.join("apps")
    }

    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    // Add cache_key function later
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_cache_paths() {
        let cache = Cache::with_custom_root(PathBuf::from("tests/test_cache"));

        assert_eq!(cache.root(), Path::new("tests/test_cache"));
        assert_eq!(cache.git_db(), PathBuf::from("tests/test_cache/git/db"));
        assert_eq!(cache.git_checkouts(), PathBuf::from("tests/test_cache/git/checkouts"));
        assert_eq!(cache.artifacts(), PathBuf::from("tests/test_cache/artifacts"));
        assert_eq!(cache.tools(), PathBuf::from("tests/test_cache/tools"));
        assert_eq!(cache.apps(), PathBuf::from("tests/test_cache/apps"));
        assert_eq!(cache.bin(), PathBuf::from("tests/test_cache/bin"));
        assert_eq!(cache.tmp(), PathBuf::from("tests/test_cache/tmp"));
    }
}