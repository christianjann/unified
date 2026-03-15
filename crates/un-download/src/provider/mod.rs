pub mod artifactory;
pub mod github;
pub mod http;

use anyhow::Result;
use semver::Version;
use sha2::{Digest, Sha256};

use crate::platform;
use un_cache::Cache;

/// A release from a provider.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag_name: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

/// A single asset within a release.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub url: String,
    pub name: String,
}

/// A fully resolved download — ready to fetch.
#[derive(Debug, Clone)]
pub struct ResolvedDownload {
    pub version: String,
    pub url: String,
    pub asset_name: String,
}

/// Main download engine — handles resolving, downloading, verifying, and caching.
pub struct DownloadEngine {
    client: reqwest::blocking::Client,
    github_token: Option<String>,
    artifactory_token: Option<String>,
}

impl Default for DownloadEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadEngine {
    pub fn new() -> Self {
        let github_token = std::env::var("GITHUB_TOKEN").ok();
        let artifactory_token = std::env::var("ARTIFACTORY_TOKEN").ok();

        let client = reqwest::blocking::Client::builder()
            .user_agent("unified/0.1")
            .build()
            .expect("failed to create HTTP client");

        Self {
            client,
            github_token,
            artifactory_token,
        }
    }

    pub fn client(&self) -> &reqwest::blocking::Client {
        &self.client
    }

    pub fn github_token(&self) -> Option<&str> {
        self.github_token.as_deref()
    }

    pub fn artifactory_token(&self) -> Option<&str> {
        self.artifactory_token.as_deref()
    }

    /// Download bytes from a URL, returning the raw data.
    pub fn download_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut builder = self.client.get(url);

        // Auto-attach auth tokens based on URL domain
        if url.contains("api.github.com") || url.contains("github.com") {
            if let Some(token) = &self.github_token {
                builder = builder.header("Authorization", format!("token {}", token));
            }
            // GitHub API requires Accept header for binary downloads
            builder = builder.header("Accept", "application/octet-stream");
        }

        let response = builder.send()?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("HTTP {} downloading {}", status, url);
        }
        let bytes = response.bytes()?;
        Ok(bytes.to_vec())
    }

    /// Download and verify SHA-256 checksum.
    pub fn download_verified(&self, url: &str, expected_sha256: Option<&str>) -> Result<Vec<u8>> {
        let data = self.download_bytes(url)?;

        if let Some(expected) = expected_sha256 {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            let actual = format!("{:x}", hasher.finalize());
            if actual != expected {
                anyhow::bail!(
                    "SHA-256 checksum mismatch for {}\n  expected: {}\n  actual:   {}",
                    url,
                    expected,
                    actual
                );
            }
        }

        Ok(data)
    }

    /// Compute SHA-256 of bytes.
    pub fn sha256(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Resolve the best asset from a set of releases for the current platform.
    /// Uses custom platform mappings if provided, otherwise auto-detects.
    pub fn choose_asset(
        releases: &[Release],
        version_req: &semver::VersionReq,
        platform_map: &std::collections::HashMap<String, String>,
    ) -> Option<ResolvedDownload> {
        // Build sorted list of (version, asset_index, release_index)
        let mut candidates: Vec<(Version, usize, usize)> = releases
            .iter()
            .enumerate()
            .filter_map(|(ri, release)| {
                if release.prerelease {
                    return None;
                }
                let version = parse_version(&release.tag_name)?;
                if !version_req.matches(&version) {
                    return None;
                }
                let asset_idx = find_platform_asset(&release.assets, platform_map)?;
                Some((version, asset_idx, ri))
            })
            .collect();

        // Sort descending by version — pick the newest matching
        candidates.sort_by(|a, b| b.0.cmp(&a.0));

        candidates.first().map(|(version, asset_idx, release_idx)| {
            let release = &releases[*release_idx];
            let asset = &release.assets[*asset_idx];
            ResolvedDownload {
                version: version.to_string(),
                url: asset.url.clone(),
                asset_name: asset.name.clone(),
            }
        })
    }

    /// Check if an item is already cached at the expected path.
    pub fn is_cached(cache: &Cache, category: &str, name: &str, version: &str) -> bool {
        let dir = match category {
            "artifacts" => cache.artifacts(),
            "tools" => cache.tools(),
            "apps" => cache.apps(),
            _ => return false,
        };
        let version_dir = dir.join(name).join(version);
        version_dir.exists()
    }

    /// Return the cache path for a category/name/version.
    pub fn cache_path(
        cache: &Cache,
        category: &str,
        name: &str,
        version: &str,
    ) -> std::path::PathBuf {
        let dir = match category {
            "artifacts" => cache.artifacts(),
            "tools" => cache.tools(),
            "apps" => cache.apps(),
            _ => cache.tmp(),
        };
        dir.join(name).join(version)
    }
}

/// Parse a version string, stripping leading 'v' if present.
fn parse_version(tag: &str) -> Option<Version> {
    let s = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(s).ok()
}

/// Find the best asset index for the current platform.
/// If `platform_map` has a mapping for the current platform, use that keyword.
/// Otherwise, fall back to built-in platform keywords.
fn find_platform_asset(
    assets: &[ReleaseAsset],
    platform_map: &std::collections::HashMap<String, String>,
) -> Option<usize> {
    let current = platform::current_platform();

    // Check user-provided platform mapping first
    if let Some(keyword) = platform_map.get(current)
        && let Some(idx) = assets.iter().position(|a| {
            a.name.to_lowercase().contains(&keyword.to_lowercase())
        }) {
            return Some(idx);
        }

    // Fall back to built-in platform keywords
    let keywords = platform::platform_keywords();
    for keyword in keywords {
        if let Some(idx) = assets.iter().position(|a| {
            a.name.to_lowercase().contains(&keyword.to_lowercase())
        }) {
            return Some(idx);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_version_with_v_prefix() {
        assert_eq!(parse_version("v1.2.3"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn parse_version_without_prefix() {
        assert_eq!(parse_version("1.2.3"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn parse_version_invalid() {
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn choose_asset_picks_latest_matching() {
        let releases = vec![
            Release {
                tag_name: "v1.0.0".to_string(),
                prerelease: false,
                assets: vec![ReleaseAsset {
                    name: "tool-linux-x86_64.tar.gz".to_string(),
                    url: "https://example.com/1.0.0".to_string(),
                }],
            },
            Release {
                tag_name: "v2.0.0".to_string(),
                prerelease: false,
                assets: vec![ReleaseAsset {
                    name: "tool-linux-x86_64.tar.gz".to_string(),
                    url: "https://example.com/2.0.0".to_string(),
                }],
            },
        ];

        let req = semver::VersionReq::parse(">=1.0").unwrap();
        let result = DownloadEngine::choose_asset(&releases, &req, &HashMap::new());
        assert!(result.is_some());
        assert_eq!(result.unwrap().version, "2.0.0");
    }

    #[test]
    fn choose_asset_respects_version_req() {
        let releases = vec![
            Release {
                tag_name: "v1.5.0".to_string(),
                prerelease: false,
                assets: vec![ReleaseAsset {
                    name: "tool-linux-x86_64.tar.gz".to_string(),
                    url: "https://example.com/1.5.0".to_string(),
                }],
            },
            Release {
                tag_name: "v2.0.0".to_string(),
                prerelease: false,
                assets: vec![ReleaseAsset {
                    name: "tool-linux-x86_64.tar.gz".to_string(),
                    url: "https://example.com/2.0.0".to_string(),
                }],
            },
        ];

        let req = semver::VersionReq::parse("1.*").unwrap();
        let result = DownloadEngine::choose_asset(&releases, &req, &HashMap::new());
        assert!(result.is_some());
        assert_eq!(result.unwrap().version, "1.5.0");
    }

    #[test]
    fn choose_asset_skips_prerelease() {
        let releases = vec![Release {
            tag_name: "v1.0.0-beta.1".to_string(),
            prerelease: true,
            assets: vec![ReleaseAsset {
                name: "tool-linux-x86_64.tar.gz".to_string(),
                url: "https://example.com/beta".to_string(),
            }],
        }];

        let req = semver::VersionReq::parse(">=1.0").unwrap();
        let result = DownloadEngine::choose_asset(&releases, &req, &HashMap::new());
        assert!(result.is_none());
    }

    #[test]
    fn sha256_computation() {
        let hash = DownloadEngine::sha256(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
