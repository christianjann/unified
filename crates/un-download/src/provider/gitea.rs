use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Release, ReleaseAsset};

pub struct GiteaProvider;

impl GiteaProvider {
    /// Fetch all releases for a Gitea/Forgejo repo.
    /// `api_url` is the base (e.g. "https://gitea.internal.dev").
    /// `owner_repo` is "owner/repo".
    pub fn get_releases(
        client: &reqwest::blocking::Client,
        api_url: &str,
        owner_repo: &str,
        token: Option<&str>,
    ) -> Result<Vec<Release>> {
        let mut all_releases = Vec::new();
        let mut page = 1u32;

        loop {
            let url = format!(
                "{}/api/v1/repos/{}/releases?limit=50&page={}",
                api_url.trim_end_matches('/'),
                owner_repo,
                page,
            );

            let mut builder = client
                .get(&url)
                .header("User-Agent", "unified/0.1");

            if let Some(t) = token {
                builder = builder.header("Authorization", format!("token {}", t));
            }

            let response = builder
                .send()
                .with_context(|| format!("fetching Gitea releases from {}", url))?;

            let status = response.status();
            let body = response.text().context("reading Gitea API response")?;

            if !status.is_success() {
                anyhow::bail!(
                    "Gitea API returned {} for {}\n{}",
                    status,
                    url,
                    &body[..body.len().min(500)]
                );
            }

            let gt_releases: Vec<GtRelease> = serde_json::from_str(&body)
                .with_context(|| format!("parsing Gitea releases for {}", owner_repo))?;

            if gt_releases.is_empty() {
                break;
            }

            all_releases.extend(gt_releases.into_iter().map(|r| r.into()));
            page += 1;
        }

        Ok(all_releases)
    }

    /// Download an asset from Gitea with token auth.
    pub fn download_asset(
        client: &reqwest::blocking::Client,
        url: &str,
        token: Option<&str>,
    ) -> Result<Vec<u8>> {
        let mut builder = client
            .get(url)
            .header("User-Agent", "unified/0.1");

        if let Some(t) = token {
            builder = builder.header("Authorization", format!("token {}", t));
        }

        let response = builder
            .send()
            .with_context(|| format!("downloading from Gitea: {}", url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Gitea returned {} for {}", status, url);
        }

        let bytes = response.bytes()?;
        Ok(bytes.to_vec())
    }
}

#[derive(Debug, Deserialize)]
struct GtRelease {
    tag_name: String,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GtAsset>,
}

#[derive(Debug, Deserialize)]
struct GtAsset {
    browser_download_url: String,
    name: String,
}

impl From<GtRelease> for Release {
    fn from(r: GtRelease) -> Self {
        Release {
            tag_name: r.tag_name,
            prerelease: r.prerelease,
            assets: r.assets.into_iter().map(|a| a.into()).collect(),
        }
    }
}

impl From<GtAsset> for ReleaseAsset {
    fn from(a: GtAsset) -> Self {
        ReleaseAsset {
            url: a.browser_download_url,
            name: a.name,
        }
    }
}
