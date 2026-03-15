use anyhow::{Context, Result};

use super::DownloadEngine;

pub struct HttpProvider;

impl HttpProvider {
    /// Download a file from a direct URL with optional SHA-256 verification.
    pub fn download(
        engine: &DownloadEngine,
        url: &str,
        expected_sha256: Option<&str>,
    ) -> Result<Vec<u8>> {
        engine
            .download_verified(url, expected_sha256)
            .with_context(|| format!("downloading {}", url))
    }
}
