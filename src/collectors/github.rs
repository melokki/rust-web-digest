use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::{
    collectors::CollectionWindow,
    config::{CollectionConfig, ProjectConfig},
    domain::{Candidate, CandidateKind},
};

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    id: u64,
    html_url: String,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    published_at: Option<DateTime<Utc>>,
}

pub struct GitHubReleaseCollector<'a> {
    client: &'a Client,
    config: &'a CollectionConfig,
    token: Option<&'a str>,
}

impl<'a> GitHubReleaseCollector<'a> {
    pub fn new(
        client: &'a Client,
        config: &'a CollectionConfig,
        token: Option<&'a str>,
    ) -> Self {
        Self {
            client,
            config,
            token,
        }
    }

    pub async fn collect_project(
        &self,
        project: &ProjectConfig,
        window: &CollectionWindow,
        discovered_at: &DateTime<Utc>,
    ) -> Result<Vec<Candidate>> {
        let repository = project
            .github
            .as_deref()
            .context("GitHub release collection requires a repository")?;
        let mut output = Vec::new();

        for page in 1..=self.config.github_max_pages {
            let url = format!(
                "{}/repos/{}/releases?per_page=100&page={}",
                self.config.github_api_url.trim_end_matches('/'),
                repository,
                page
            );
            let mut request = self
                .client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2026-03-10");
            if let Some(token) = self.token {
                request = request.bearer_auth(token);
            }

            let releases = request
                .send()
                .await
                .with_context(|| format!("GitHub request failed for {repository}"))?
                .error_for_status()
                .with_context(|| format!("GitHub returned an error for {repository}"))?
                .json::<Vec<GitHubRelease>>()
                .await
                .with_context(|| format!("invalid GitHub release response for {repository}"))?;

            let page_len = releases.len();
            output.extend(releases.into_iter().filter_map(|release| {
                release_to_candidate(project, repository, release, window, discovered_at)
            }));

            if page_len < 100 {
                break;
            }
        }

        Ok(output)
    }
}

fn release_to_candidate(
    project: &ProjectConfig,
    repository: &str,
    release: GitHubRelease,
    window: &CollectionWindow,
    discovered_at: &DateTime<Utc>,
) -> Option<Candidate> {
    if release.draft {
        return None;
    }

    let published_at = release.published_at?;
    if !window.contains(&published_at) {
        return None;
    }

    let title = release
        .name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&release.tag_name)
        .to_owned();
    let mut metadata = BTreeMap::new();
    metadata.insert("repository".to_owned(), repository.to_owned());
    metadata.insert("tag_name".to_owned(), release.tag_name.clone());
    metadata.insert("prerelease".to_owned(), release.prerelease.to_string());

    Some(Candidate {
        id: format!("github-release:{repository}:{}", release.id),
        kind: CandidateKind::GitHubRelease,
        title,
        url: release.html_url,
        source_id: format!("github:{repository}"),
        project_id: Some(project.id.clone()),
        category: project.category.clone(),
        published_at,
        discovered_at: discovered_at.clone(),
        summary: release.body.as_deref().and_then(first_non_empty_line),
        raw_content: release.body,
        metadata,
    })
}

fn first_non_empty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{GitHubRelease, first_non_empty_line, release_to_candidate};
    use crate::{
        collectors::CollectionWindow,
        config::{ProjectCollectConfig, ProjectConfig},
    };

    #[test]
    fn parses_release_fixture() {
        let releases: Vec<GitHubRelease> =
            serde_json::from_str(include_str!("../../tests/fixtures/github_releases.json")).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag_name, "axum-v1.2.3");
        assert!(releases[1].prerelease);
    }

    #[test]
    fn converts_release_fixture_to_candidate() {
        let mut releases: Vec<GitHubRelease> =
            serde_json::from_str(include_str!("../../tests/fixtures/github_releases.json")).unwrap();
        let project = ProjectConfig {
            id: "axum".to_owned(),
            name: "Axum".to_owned(),
            category: "frameworks".to_owned(),
            github: Some("tokio-rs/axum".to_owned()),
            crates: vec!["axum".to_owned()],
            collect: ProjectCollectConfig {
                releases: true,
                security: true,
                crate_releases: true,
            },
        };
        let window = CollectionWindow {
            since: Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
            until: Utc.with_ymd_and_hms(2026, 7, 31, 23, 59, 59).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();

        let candidate = release_to_candidate(
            &project,
            "tokio-rs/axum",
            releases.remove(0),
            &window,
            &now,
        )
        .unwrap();

        assert_eq!(candidate.id, "github-release:tokio-rs/axum:101");
        assert_eq!(candidate.project_id.as_deref(), Some("axum"));
        assert_eq!(candidate.summary.as_deref(), Some("A useful release"));
    }

    #[test]
    fn picks_first_non_empty_release_note_line() {
        assert_eq!(
            first_non_empty_line("\n\nFirst meaningful line\nSecond"),
            Some("First meaningful line".to_owned())
        );
    }
}
