use anyhow::Result;
use std::path::Path;

#[allow(async_fn_in_trait)]
pub trait ArtifactProvider {
    async fn list_versions(&self, spec: &ArtifactSpec) -> Result<Vec<Version>>;
    async fn resolve(
        &self,
        spec: &ArtifactSpec,
        version_req: &VersionReq,
    ) -> Result<ResolvedArtifact>;
    async fn download(&self, resolved: &ResolvedArtifact, dest: &Path) -> Result<DownloadResult>;
}

pub struct ArtifactSpec {
    // Stub
}

pub struct Version {
    // Stub
}

pub struct VersionReq {
    // Stub
}

pub struct ResolvedArtifact {
    // Stub
}

pub struct DownloadResult {
    // Stub
}

// Providers
pub struct GitHubProvider;

impl ArtifactProvider for GitHubProvider {
    async fn list_versions(&self, _spec: &ArtifactSpec) -> Result<Vec<Version>> {
        todo!()
    }
    async fn resolve(
        &self,
        _spec: &ArtifactSpec,
        _version_req: &VersionReq,
    ) -> Result<ResolvedArtifact> {
        todo!()
    }
    async fn download(&self, _resolved: &ResolvedArtifact, _dest: &Path) -> Result<DownloadResult> {
        todo!()
    }
}

pub struct ArtifactoryProvider;

impl ArtifactProvider for ArtifactoryProvider {
    async fn list_versions(&self, _spec: &ArtifactSpec) -> Result<Vec<Version>> {
        todo!()
    }
    async fn resolve(
        &self,
        _spec: &ArtifactSpec,
        _version_req: &VersionReq,
    ) -> Result<ResolvedArtifact> {
        todo!()
    }
    async fn download(&self, _resolved: &ResolvedArtifact, _dest: &Path) -> Result<DownloadResult> {
        todo!()
    }
}

pub struct HttpProvider;

impl ArtifactProvider for HttpProvider {
    async fn list_versions(&self, _spec: &ArtifactSpec) -> Result<Vec<Version>> {
        todo!()
    }
    async fn resolve(
        &self,
        _spec: &ArtifactSpec,
        _version_req: &VersionReq,
    ) -> Result<ResolvedArtifact> {
        todo!()
    }
    async fn download(&self, _resolved: &ResolvedArtifact, _dest: &Path) -> Result<DownloadResult> {
        todo!()
    }
}
