use std::{fs, path::{Component, Path, PathBuf}};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{Duration, sleep};

use crate::composer::NewsletterManifest;

const GITHUB_API_VERSION: &str = "2026-03-10";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseState {
    Draft,
    Published,
}

impl ReleaseState {
    fn draft(self) -> bool {
        matches!(self, Self::Draft)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationInput {
    pub manifest_path: PathBuf,
    pub manifest: NewsletterManifest,
    pub markdown_path: PathBuf,
    pub markdown: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicationReport {
    pub content_created: bool,
    pub content_updated: bool,
    pub content_unchanged: bool,
    pub release_created: bool,
    pub release_updated: bool,
    pub release_unchanged: bool,
    pub asset_uploaded: bool,
    pub asset_replaced: bool,
    pub asset_unchanged: bool,
    pub tag_updated: bool,
    pub release_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepositoryInfo {
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct ContentFile {
    sha: String,
    content: String,
    encoding: String,
}

#[derive(Debug, Deserialize)]
struct ContentWriteResponse {
    commit: CommitRef,
}

#[derive(Debug, Deserialize)]
struct CommitRef {
    sha: String,
}

#[derive(Debug, Serialize)]
struct PutContentRequest<'a> {
    message: &'a str,
    content: String,
    branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    id: u64,
    html_url: String,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    #[serde(default)]
    immutable: bool,
    upload_url: String,
}

#[derive(Debug, Serialize)]
struct CreateReleaseRequest<'a> {
    tag_name: &'a str,
    target_commitish: &'a str,
    name: &'a str,
    body: &'a str,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug, Serialize)]
struct UpdateReleaseRequest<'a> {
    target_commitish: &'a str,
    name: &'a str,
    body: &'a str,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    id: u64,
    name: String,
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitReference {
    object: GitObject,
}

#[derive(Debug, Deserialize)]
struct GitObject {
    sha: String,
}

#[derive(Debug, Serialize)]
struct UpdateReferenceRequest<'a> {
    sha: &'a str,
    force: bool,
}

pub struct GitHubNewsletterPublisher<'a> {
    client: &'a Client,
    api_url: &'a str,
    token: &'a str,
    owner: String,
    repo: String,
}

impl<'a> GitHubNewsletterPublisher<'a> {
    pub fn new(
        client: &'a Client,
        api_url: &'a str,
        token: &'a str,
        repository: &str,
    ) -> Result<Self> {
        let (owner, repo) = parse_repository(repository)?;
        Ok(Self {
            client,
            api_url,
            token,
            owner,
            repo,
        })
    }

    pub async fn publish(
        &self,
        input: &PublicationInput,
        state: ReleaseState,
        commit_message_prefix: &str,
        sync_release_tag: bool,
        dry_run: bool,
    ) -> Result<PublicationReport> {
        let mut report = PublicationReport::default();
        let repository = self.get_repository().await?;
        let path = repository_path(&input.manifest.markdown_path)?;
        let existing_content = self.get_content(&path, &repository.default_branch).await?;
        let content_changed = match &existing_content {
            Some(existing) => decode_content(existing)? != input.markdown.as_bytes(),
            None => true,
        };

        if dry_run {
            match existing_content {
                None => report.content_created = true,
                Some(_) if content_changed => report.content_updated = true,
                Some(_) => report.content_unchanged = true,
            }

            match self.get_release_by_tag(&input.manifest.release_tag).await? {
                None => {
                    report.release_created = true;
                    report.asset_uploaded = true;
                }
                Some(release) => {
                    if release_metadata_matches(
                        &release,
                        &input.manifest.release_name,
                        &input.markdown,
                        state,
                    ) {
                        report.release_unchanged = true;
                    } else {
                        report.release_updated = true;
                    }
                    let expected_digest = sha256_digest(input.markdown.as_bytes());
                    let existing_asset = self
                        .list_release_assets(release.id)
                        .await?
                        .into_iter()
                        .find(|asset| asset.name == input.manifest.release_asset_name);
                    match existing_asset {
                        Some(asset) if asset.digest.as_deref() == Some(expected_digest.as_str()) => {
                            report.asset_unchanged = true;
                        }
                        Some(_) => {
                            report.asset_replaced = true;
                            report.asset_uploaded = true;
                        }
                        None => report.asset_uploaded = true,
                    }
                    report.release_url = Some(release.html_url);
                }
            }
            return Ok(report);
        }

        let commit_sha = if content_changed {
            let sha = self
                .put_content(
                    &path,
                    &repository.default_branch,
                    existing_content.as_ref().map(|content| content.sha.as_str()),
                    input.markdown.as_bytes(),
                    &format!("{} {}", commit_message_prefix.trim(), input.manifest.month),
                )
                .await?;
            if existing_content.is_some() {
                report.content_updated = true;
            } else {
                report.content_created = true;
            }
            sha
        } else {
            report.content_unchanged = true;
            self.get_branch_head(&repository.default_branch).await?
        };

        let release = match self.get_release_by_tag(&input.manifest.release_tag).await? {
            Some(existing) => {
                let metadata_matches = release_metadata_matches(
                    &existing,
                    &input.manifest.release_name,
                    &input.markdown,
                    state,
                );
                if metadata_matches && !(content_changed && existing.draft) {
                    report.release_unchanged = true;
                    existing
                } else {
                    if existing.immutable {
                        bail!(
                            "release '{}' is immutable and its metadata differs from the composed digest",
                            input.manifest.release_tag
                        );
                    }
                    let release = self
                        .update_release(
                            existing.id,
                            &commit_sha,
                            &input.manifest.release_name,
                            &input.markdown,
                            state,
                        )
                        .await?;
                    report.release_updated = true;
                    release
                }
            }
            None => {
                let release = self
                    .create_release(
                        &input.manifest.release_tag,
                        &commit_sha,
                        &input.manifest.release_name,
                        &input.markdown,
                        state,
                    )
                    .await?;
                report.release_created = true;
                release
            }
        };

        if sync_release_tag
            && content_changed
            && !report.release_created
            && !release.draft
        {
            report.tag_updated = self
                .sync_tag(&release.tag_name, &commit_sha)
                .await?;
        }

        let expected_digest = sha256_digest(input.markdown.as_bytes());
        let existing_asset = self
            .list_release_assets(release.id)
            .await?
            .into_iter()
            .find(|asset| asset.name == input.manifest.release_asset_name);
        match existing_asset {
            Some(asset) if asset.digest.as_deref() == Some(expected_digest.as_str()) => {
                report.asset_unchanged = true;
            }
            Some(asset) => {
                if release.immutable {
                    bail!(
                        "release '{}' is immutable and its Markdown asset differs from the composed digest",
                        input.manifest.release_tag
                    );
                }
                self.delete_release_asset(asset.id).await?;
                report.asset_replaced = true;
                self.upload_release_asset(
                    &release.upload_url,
                    &input.manifest.release_asset_name,
                    input.markdown.as_bytes(),
                )
                .await?;
                report.asset_uploaded = true;
            }
            None => {
                if release.immutable {
                    bail!(
                        "release '{}' is immutable and the Markdown asset is missing",
                        input.manifest.release_tag
                    );
                }
                self.upload_release_asset(
                    &release.upload_url,
                    &input.manifest.release_asset_name,
                    input.markdown.as_bytes(),
                )
                .await?;
                report.asset_uploaded = true;
            }
        }
        report.release_url = Some(release.html_url);

        Ok(report)
    }

    async fn get_repository(&self) -> Result<RepositoryInfo> {
        let response = self
            .request(self.client.get(self.repo_base_url()))
            .send()
            .await
            .context("failed to query repository metadata")?
            .error_for_status()
            .context("GitHub returned an error querying repository metadata")?;
        response
            .json()
            .await
            .context("invalid GitHub repository response")
    }

    async fn get_content(&self, path: &str, branch: &str) -> Result<Option<ContentFile>> {
        let url = format!("{}/contents/{}", self.repo_base_url(), path);
        let response = self
            .request(self.client.get(url))
            .query(&[("ref", branch)])
            .send()
            .await
            .with_context(|| format!("failed to query repository content '{path}'"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .with_context(|| format!("GitHub returned an error querying '{path}'"))?;
        Ok(Some(
            response
                .json()
                .await
                .with_context(|| format!("invalid GitHub content response for '{path}'"))?,
        ))
    }

    async fn put_content(
        &self,
        path: &str,
        branch: &str,
        sha: Option<&str>,
        bytes: &[u8],
        message: &str,
    ) -> Result<String> {
        let url = format!("{}/contents/{}", self.repo_base_url(), path);
        let response = self
            .request(self.client.put(url))
            .json(&PutContentRequest {
                message,
                content: BASE64.encode(bytes),
                branch,
                sha,
            })
            .send()
            .await
            .with_context(|| format!("failed to write repository content '{path}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error writing '{path}'"))?
            .json::<ContentWriteResponse>()
            .await
            .with_context(|| format!("invalid GitHub write response for '{path}'"))?;
        pause_after_mutation().await;
        Ok(response.commit.sha)
    }

    async fn get_branch_head(&self, branch: &str) -> Result<String> {
        let url = format!("{}/commits/{}", self.repo_base_url(), branch);
        let response = self
            .request(self.client.get(url))
            .send()
            .await
            .with_context(|| format!("failed to query branch head '{branch}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error querying branch '{branch}'"))?
            .json::<CommitRef>()
            .await
            .context("invalid GitHub commit response")?;
        Ok(response.sha)
    }

    async fn get_release_by_tag(&self, tag: &str) -> Result<Option<GitHubRelease>> {
        let url = format!("{}/releases/tags/{}", self.repo_base_url(), tag);
        let response = self
            .request(self.client.get(url))
            .send()
            .await
            .with_context(|| format!("failed to query published release tag '{tag}'"))?;
        if response.status() != StatusCode::NOT_FOUND {
            let response = response
                .error_for_status()
                .with_context(|| format!("GitHub returned an error querying release tag '{tag}'"))?;
            return Ok(Some(
                response
                    .json()
                    .await
                    .with_context(|| format!("invalid GitHub release response for '{tag}'"))?,
            ));
        }

        // The tag lookup endpoint only returns published releases. Authenticated
        // release listings include drafts, so use the listing as a fallback.
        for page in 1..=10_u32 {
            let list_url = format!("{}/releases", self.repo_base_url());
            let releases = self
                .request(self.client.get(list_url))
                .query(&[("per_page", 100_u32), ("page", page)])
                .send()
                .await
                .with_context(|| format!("failed to list releases while looking for '{tag}'"))?
                .error_for_status()
                .with_context(|| format!("GitHub returned an error listing releases for '{tag}'"))?
                .json::<Vec<GitHubRelease>>()
                .await
                .with_context(|| format!("invalid GitHub release listing while looking for '{tag}'"))?;
            let count = releases.len();
            if let Some(release) = releases.into_iter().find(|release| release.tag_name == tag) {
                return Ok(Some(release));
            }
            if count < 100 {
                break;
            }
        }
        Ok(None)
    }

    async fn create_release(
        &self,
        tag: &str,
        target_commitish: &str,
        name: &str,
        body: &str,
        state: ReleaseState,
    ) -> Result<GitHubRelease> {
        let url = format!("{}/releases", self.repo_base_url());
        let release = self.request(self.client.post(url))
            .json(&CreateReleaseRequest {
                tag_name: tag,
                target_commitish,
                name,
                body,
                draft: state.draft(),
                prerelease: false,
            })
            .send()
            .await
            .with_context(|| format!("failed to create release '{tag}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error creating release '{tag}'"))?
            .json()
            .await
            .with_context(|| format!("invalid create-release response for '{tag}'"))?;
        pause_after_mutation().await;
        Ok(release)
    }

    async fn update_release(
        &self,
        release_id: u64,
        target_commitish: &str,
        name: &str,
        body: &str,
        state: ReleaseState,
    ) -> Result<GitHubRelease> {
        let url = format!("{}/releases/{release_id}", self.repo_base_url());
        let release = self.request(self.client.patch(url))
            .json(&UpdateReleaseRequest {
                target_commitish,
                name,
                body,
                draft: state.draft(),
                prerelease: false,
            })
            .send()
            .await
            .with_context(|| format!("failed to update release #{release_id}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error updating release #{release_id}"))?
            .json()
            .await
            .with_context(|| format!("invalid update-release response for #{release_id}"))?;
        pause_after_mutation().await;
        Ok(release)
    }

    async fn list_release_assets(&self, release_id: u64) -> Result<Vec<ReleaseAsset>> {
        let url = format!("{}/releases/{release_id}/assets", self.repo_base_url());
        self.request(self.client.get(url))
            .query(&[("per_page", "100")])
            .send()
            .await
            .with_context(|| format!("failed to list assets for release #{release_id}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error listing assets for release #{release_id}"))?
            .json()
            .await
            .with_context(|| format!("invalid asset-list response for release #{release_id}"))
    }

    async fn delete_release_asset(&self, asset_id: u64) -> Result<()> {
        let url = format!("{}/releases/assets/{asset_id}", self.repo_base_url());
        self.request(self.client.delete(url))
            .send()
            .await
            .with_context(|| format!("failed to delete release asset #{asset_id}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error deleting release asset #{asset_id}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn upload_release_asset(&self, upload_url: &str, name: &str, bytes: &[u8]) -> Result<()> {
        let url = upload_url.split('{').next().unwrap_or(upload_url);
        self.request(self.client.post(url))
            .query(&[("name", name)])
            .header("Content-Type", "text/markdown; charset=utf-8")
            .body(bytes.to_vec())
            .send()
            .await
            .with_context(|| format!("failed to upload release asset '{name}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error uploading release asset '{name}'"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn sync_tag(&self, tag: &str, commit_sha: &str) -> Result<bool> {
        let get_url = format!("{}/git/ref/tags/{}", self.repo_base_url(), tag);
        let response = self
            .request(self.client.get(&get_url))
            .send()
            .await
            .with_context(|| format!("failed to query tag reference '{tag}'"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let reference = response
            .error_for_status()
            .with_context(|| format!("GitHub returned an error querying tag '{tag}'"))?
            .json::<GitReference>()
            .await
            .with_context(|| format!("invalid tag reference response for '{tag}'"))?;
        if reference.object.sha == commit_sha {
            return Ok(false);
        }

        let update_url = format!("{}/git/refs/tags/{}", self.repo_base_url(), tag);
        self.request(self.client.patch(update_url))
            .json(&UpdateReferenceRequest {
                sha: commit_sha,
                force: true,
            })
            .send()
            .await
            .with_context(|| format!("failed to update tag reference '{tag}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error updating tag '{tag}'"))?;
        pause_after_mutation().await;
        Ok(true)
    }

    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .bearer_auth(self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
    }

    fn repo_base_url(&self) -> String {
        format!(
            "{}/repos/{}/{}",
            self.api_url.trim_end_matches('/'),
            self.owner,
            self.repo
        )
    }
}

async fn pause_after_mutation() {
    sleep(Duration::from_secs(1)).await;
}

pub fn load_publication_input(manifest_path: impl AsRef<Path>) -> Result<PublicationInput> {
    let manifest_path = manifest_path.as_ref().to_path_buf();
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: NewsletterManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let markdown_path = PathBuf::from(&manifest.markdown_path);
    repository_path(&manifest.markdown_path)?;
    let markdown = fs::read_to_string(&markdown_path)
        .with_context(|| format!("failed to read {}", markdown_path.display()))?;
    if markdown.trim().is_empty() {
        bail!("newsletter Markdown cannot be empty");
    }
    Ok(PublicationInput {
        manifest_path,
        manifest,
        markdown_path,
        markdown,
    })
}

pub fn repository_path(path: &str) -> Result<String> {
    let path = Path::new(path);
    if path.is_absolute() || path.as_os_str().is_empty() {
        bail!("newsletter markdown_path must be a non-empty relative repository path");
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => bail!("newsletter markdown_path must not contain '.', '..', roots, or prefixes"),
        }
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn decode_content(content: &ContentFile) -> Result<Vec<u8>> {
    if content.encoding != "base64" {
        bail!("unsupported GitHub content encoding '{}'", content.encoding);
    }
    let compact = content.content.lines().collect::<String>();
    BASE64
        .decode(compact)
        .context("invalid base64 content returned by GitHub")
}

pub fn release_asset_name(prefix: &str, month: &str) -> String {
    format!("{}-{}.md", prefix.trim_end_matches('-'), month)
}

fn release_metadata_matches(
    release: &GitHubRelease,
    name: &str,
    body: &str,
    state: ReleaseState,
) -> bool {
    release.name.as_deref().unwrap_or_default() == name
        && release.body.as_deref().unwrap_or_default() == body
        && release.draft == state.draft()
}

pub fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{hex}")
}

fn parse_repository(repository: &str) -> Result<(String, String)> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        bail!("repository must be in owner/repo form: {repository}");
    }
    Ok((owner.to_owned(), repo.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{release_asset_name, repository_path};

    #[test]
    fn validates_repository_relative_paths() {
        assert_eq!(
            repository_path("content/issues/2026-07.md").unwrap(),
            "content/issues/2026-07.md"
        );
        assert!(repository_path("../secrets.txt").is_err());
        assert!(repository_path("/absolute.md").is_err());
    }

    #[test]
    fn builds_stable_release_asset_name() {
        assert_eq!(
            release_asset_name("rust-web-digest", "2026-07"),
            "rust-web-digest-2026-07.md"
        );
    }
}
