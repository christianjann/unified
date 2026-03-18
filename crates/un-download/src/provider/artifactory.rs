use anyhow::{Context, Result};
use indicatif::ProgressBar;
use serde::Deserialize;

use super::{Release, ReleaseAsset, read_response_with_progress};

pub struct ArtifactoryProvider;

impl ArtifactoryProvider {
    /// Fetch all releases from an Artifactory storage path.
    /// Uses the Artifactory Storage API: `GET /artifactory/api/storage/{path}?list&deep=1`
    pub fn get_releases(
        client: &reqwest::blocking::Client,
        base_url: &str,
        repo_path: &str,
        token: Option<&str>,
    ) -> Result<Vec<Release>> {
        let url = format!(
            "{}/artifactory/api/storage/{}",
            base_url.trim_end_matches('/'),
            repo_path
        );

        let mut builder = client
            .get(&url)
            .query(&[("list", ""), ("deep", "1")])
            .header("User-Agent", "unified/0.1");

        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {}", t));
        }

        let response = builder
            .send()
            .with_context(|| format!("fetching Artifactory releases from {}", url))?;

        let status = response.status();
        let body = response.text().context("reading Artifactory response")?;

        if !status.is_success() {
            anyhow::bail!(
                "Artifactory API returned {} for {}\n{}",
                status,
                url,
                &body[..body.len().min(500)]
            );
        }

        let listing: ArtifactoryListing = serde_json::from_str(&body)
            .with_context(|| format!("parsing Artifactory listing for {}", repo_path))?;

        // Group files by version directory: /<version>/<asset-name>
        let mut version_map: std::collections::HashMap<String, Vec<ReleaseAsset>> =
            std::collections::HashMap::new();

        for file in &listing.files {
            let parts: Vec<&str> = file.uri.split('/').collect();
            // Expected: ["", version, asset_name]
            if parts.len() != 3 || !parts[0].is_empty() {
                continue;
            }
            let version = parts[1].to_string();
            let asset_name = parts[2].to_string();

            let asset_url = format!(
                "{}/artifactory/{}/{}/{}",
                base_url.trim_end_matches('/'),
                repo_path,
                version,
                asset_name
            );

            version_map.entry(version).or_default().push(ReleaseAsset {
                url: asset_url,
                name: asset_name,
            });
        }

        let releases = version_map
            .into_iter()
            .map(|(version, assets)| Release {
                tag_name: version,
                prerelease: false,
                assets,
            })
            .collect();

        Ok(releases)
    }

    /// Download an asset from Artifactory with bearer auth.
    pub fn download_asset(
        client: &reqwest::blocking::Client,
        url: &str,
        token: Option<&str>,
        pb: Option<&ProgressBar>,
    ) -> Result<Vec<u8>> {
        let mut builder = client.get(url).header("User-Agent", "unified/0.1");

        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {}", t));
        }

        let response = builder
            .send()
            .with_context(|| format!("downloading from Artifactory: {}", url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Artifactory returned {} for {}", status, url);
        }

        read_response_with_progress(response, pb)
    }
}

#[derive(Debug, Deserialize)]
struct ArtifactoryListing {
    files: Vec<ArtifactoryFile>,
}

#[derive(Debug, Deserialize)]
struct ArtifactoryFile {
    uri: String,
}
