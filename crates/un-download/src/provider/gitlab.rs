use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Release, ReleaseAsset};

pub struct GitLabProvider;

impl GitLabProvider {
    /// Fetch all releases for a GitLab project.
    /// `api_url` is the base (e.g. "https://gitlab.com" or "https://gitlab.mycompany.com").
    /// `project` is "group/project" (URL-encoded internally) or a numeric project ID.
    pub fn get_releases(
        client: &reqwest::blocking::Client,
        api_url: &str,
        project: &str,
        token: Option<&str>,
    ) -> Result<Vec<Release>> {
        let encoded_project = if project.parse::<u64>().is_ok() {
            project.to_string()
        } else {
            urlencoding::encode(project).into_owned()
        };

        let mut all_releases = Vec::new();
        let mut page = 1u32;

        loop {
            let url = format!(
                "{}/api/v4/projects/{}/releases?per_page=100&page={}",
                api_url.trim_end_matches('/'),
                encoded_project,
                page,
            );

            let mut builder = client
                .get(&url)
                .header("User-Agent", "unified/0.1");

            if let Some(t) = token {
                builder = builder.header("PRIVATE-TOKEN", t);
            }

            let response = builder
                .send()
                .with_context(|| format!("fetching GitLab releases from {}", url))?;

            let status = response.status();
            let body = response.text().context("reading GitLab API response")?;

            if !status.is_success() {
                anyhow::bail!(
                    "GitLab API returned {} for {}\n{}",
                    status,
                    url,
                    &body[..body.len().min(500)]
                );
            }

            let gl_releases: Vec<GlRelease> = serde_json::from_str(&body)
                .with_context(|| format!("parsing GitLab releases for {}", project))?;

            if gl_releases.is_empty() {
                break;
            }

            all_releases.extend(gl_releases.into_iter().map(|r| r.into()));
            page += 1;
        }

        Ok(all_releases)
    }

    /// Download an asset from GitLab with private-token auth.
    pub fn download_asset(
        client: &reqwest::blocking::Client,
        url: &str,
        token: Option<&str>,
    ) -> Result<Vec<u8>> {
        let mut builder = client
            .get(url)
            .header("User-Agent", "unified/0.1");

        if let Some(t) = token {
            builder = builder.header("PRIVATE-TOKEN", t);
        }

        let response = builder
            .send()
            .with_context(|| format!("downloading from GitLab: {}", url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("GitLab returned {} for {}", status, url);
        }

        let bytes = response.bytes()?;
        Ok(bytes.to_vec())
    }
}

#[derive(Debug, Deserialize)]
struct GlRelease {
    tag_name: String,
    #[serde(default)]
    assets: GlAssets,
}

#[derive(Debug, Default, Deserialize)]
struct GlAssets {
    #[serde(default)]
    links: Vec<GlLink>,
}

#[derive(Debug, Deserialize)]
struct GlLink {
    url: String,
    name: String,
}

impl From<GlRelease> for Release {
    fn from(r: GlRelease) -> Self {
        Release {
            tag_name: r.tag_name,
            prerelease: false, // GitLab doesn't have a prerelease flag on releases
            assets: r.assets.links.into_iter().map(|l| l.into()).collect(),
        }
    }
}

impl From<GlLink> for ReleaseAsset {
    fn from(l: GlLink) -> Self {
        ReleaseAsset {
            url: l.url,
            name: l.name,
        }
    }
}
