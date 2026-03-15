use anyhow::{Context, Result};
use serde::Deserialize;

use super::{DownloadEngine, Release, ReleaseAsset};

/// Parses the GitHub `Link` header to extract the "next" page URL.
fn parse_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let mut url = None;
        let mut is_next = false;

        for segment in part.split(';') {
            let segment = segment.trim();
            if segment.starts_with('<') && segment.ends_with('>') {
                url = Some(segment[1..segment.len() - 1].to_string());
            } else if segment == r#"rel="next""# {
                is_next = true;
            }
        }

        if is_next {
            return url;
        }
    }
    None
}

pub struct GitHubProvider;

impl GitHubProvider {
    /// Fetch all releases for a GitHub repo (owner/repo format).
    /// Paginates automatically.
    pub fn get_releases(engine: &DownloadEngine, owner_repo: &str) -> Result<Vec<Release>> {
        let mut all_releases = Vec::new();
        let mut next_url: Option<String> = Some(format!(
            "https://api.github.com/repos/{}/releases?per_page=100",
            owner_repo
        ));

        while let Some(url) = next_url.take() {
            let mut builder = engine
                .client()
                .get(&url)
                .header("User-Agent", "unified/0.1")
                .header("Accept", "application/vnd.github+json");

            if let Some(token) = engine.github_token() {
                builder = builder.header("Authorization", format!("token {}", token));
            }

            let response = builder
                .send()
                .with_context(|| format!("fetching releases from {}", url))?;

            // Parse pagination
            next_url = response
                .headers()
                .get("link")
                .and_then(|h| h.to_str().ok())
                .and_then(parse_next_link);

            let status = response.status();
            let body = response
                .text()
                .context("reading GitHub API response body")?;

            if !status.is_success() {
                anyhow::bail!(
                    "GitHub API returned {} for {}\n{}",
                    status,
                    url,
                    &body[..body.len().min(500)]
                );
            }

            let gh_releases: Vec<GhRelease> = serde_json::from_str(&body)
                .with_context(|| format!("parsing GitHub releases for {}", owner_repo))?;

            all_releases.extend(gh_releases.into_iter().map(|r| r.into()));
        }

        Ok(all_releases)
    }
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    url: String,
    name: String,
}

impl From<GhRelease> for Release {
    fn from(r: GhRelease) -> Self {
        Release {
            tag_name: r.tag_name,
            prerelease: r.prerelease,
            assets: r.assets.into_iter().map(|a| a.into()).collect(),
        }
    }
}

impl From<GhAsset> for ReleaseAsset {
    fn from(a: GhAsset) -> Self {
        ReleaseAsset {
            url: a.url,
            name: a.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_next_link_with_next_and_last() {
        let header = r#"<https://api.github.com/repos/o/r/releases?per_page=100&page=2>; rel="next", <https://api.github.com/repos/o/r/releases?per_page=100&page=5>; rel="last""#;
        assert_eq!(
            parse_next_link(header),
            Some("https://api.github.com/repos/o/r/releases?per_page=100&page=2".to_string())
        );
    }

    #[test]
    fn parse_next_link_last_page() {
        let header = r#"<https://api.github.com/repos/o/r/releases?page=1>; rel="first""#;
        assert_eq!(parse_next_link(header), None);
    }

    #[test]
    fn parse_next_link_empty() {
        assert_eq!(parse_next_link(""), None);
    }
}
