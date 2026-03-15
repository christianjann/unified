use anyhow::{Context, Result};
use indicatif::ProgressBar;
use sha2::{Digest, Sha256};

use super::{read_response_with_progress, DownloadEngine};

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

    /// Download a file from a direct URL with progress and optional SHA-256 verification.
    pub fn download_with_progress(
        engine: &DownloadEngine,
        url: &str,
        expected_sha256: Option<&str>,
        pb: &ProgressBar,
    ) -> Result<Vec<u8>> {
        let response = engine
            .client()
            .get(url)
            .send()
            .with_context(|| format!("downloading {}", url))?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("HTTP {} downloading {}", status, url);
        }

        let data = read_response_with_progress(response, Some(pb))?;

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
}
