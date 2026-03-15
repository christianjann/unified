mod extract;
mod platform;
mod provider;

pub use extract::extract_archive;
pub use platform::platform_keywords;
pub use provider::artifactory::ArtifactoryProvider;
pub use provider::gitea::GiteaProvider;
pub use provider::github::GitHubProvider;
pub use provider::gitlab::GitLabProvider;
pub use provider::http::HttpProvider;
pub use provider::{DownloadEngine, Release, ReleaseAsset, ResolvedDownload};
