use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tokio::time::{Duration, sleep};

use crate::{
    collectors::CollectionWindow,
    config::CratesIoConfig,
    domain::{Candidate, CandidateKind},
};

#[derive(Debug, Deserialize)]
struct CrateResponse {
    versions: Option<Vec<CrateVersion>>,
}

#[derive(Debug, Deserialize)]
struct CrateVersion {
    id: u64,
    num: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    yanked: bool,
}

pub struct CratesIoCollector<'a> {
    client: &'a Client,
    config: &'a CratesIoConfig,
    user_agent: &'a str,
}

impl<'a> CratesIoCollector<'a> {
    pub fn new(client: &'a Client, config: &'a CratesIoConfig, user_agent: &'a str) -> Self {
        Self {
            client,
            config,
            user_agent,
        }
    }

    pub async fn collect_crate(
        &self,
        crate_name: &str,
        project_id: Option<&str>,
        category: &str,
        window: &CollectionWindow,
        discovered_at: &DateTime<Utc>,
    ) -> Result<Vec<Candidate>> {
        let url = format!(
            "{}/crates/{}",
            self.config.api_url.trim_end_matches('/'),
            crate_name
        );
        let response = self
            .client
            .get(url)
            .query(&[("include", "versions")])
            .header("User-Agent", self.user_agent)
            .send()
            .await
            .with_context(|| format!("crates.io request failed for '{crate_name}'"))?
            .error_for_status()
            .with_context(|| format!("crates.io returned an error for '{crate_name}'"))?
            .json::<CrateResponse>()
            .await
            .with_context(|| format!("invalid crates.io response for '{crate_name}'"));

        sleep(Duration::from_millis(self.config.request_delay_ms)).await;
        let response = response?;

        Ok(response
            .versions
            .unwrap_or_default()
            .into_iter()
            .filter_map(|version| {
                version_to_candidate(
                    crate_name,
                    project_id,
                    category,
                    version,
                    window,
                    discovered_at,
                    &self.config.web_url,
                )
            })
            .collect())
    }
}

fn version_to_candidate(
    crate_name: &str,
    project_id: Option<&str>,
    category: &str,
    version: CrateVersion,
    window: &CollectionWindow,
    discovered_at: &DateTime<Utc>,
    web_url: &str,
) -> Option<Candidate> {
    if !window.contains(&version.created_at) {
        return None;
    }

    let mut metadata = BTreeMap::new();
    metadata.insert("crate".to_owned(), crate_name.to_owned());
    metadata.insert("version".to_owned(), version.num.clone());
    metadata.insert("yanked".to_owned(), version.yanked.to_string());

    Some(Candidate {
        id: format!("crate-release:{crate_name}:{}", version.id),
        kind: CandidateKind::CrateRelease,
        title: format!("{crate_name} {} published to crates.io", version.num),
        url: format!(
            "{}/{}/{}",
            web_url.trim_end_matches('/'),
            crate_name,
            version.num
        ),
        source_id: format!("crates-io:{crate_name}"),
        project_id: project_id.map(ToOwned::to_owned),
        category: category.to_owned(),
        published_at: version.created_at,
        discovered_at: discovered_at.clone(),
        summary: Some(format!(
            "Version {} of crate {crate_name} was published to crates.io.",
            version.num
        )),
        raw_content: None,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn converts_crate_version_fixture() {
        let response: CrateResponse = serde_json::from_str(include_str!(
            "../../tests/fixtures/crates_io_axum.json"
        ))
        .unwrap();
        let window = CollectionWindow {
            since: Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
            until: Utc.with_ymd_and_hms(2026, 7, 31, 23, 59, 59).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
        let candidate = version_to_candidate(
            "axum",
            Some("axum"),
            "frameworks",
            response.versions.unwrap().into_iter().next().unwrap(),
            &window,
            &now,
            "https://crates.io/crates",
        )
        .unwrap();

        assert_eq!(candidate.kind, CandidateKind::CrateRelease);
        assert_eq!(candidate.metadata.get("version").unwrap(), "1.2.3");
        assert_eq!(candidate.project_id.as_deref(), Some("axum"));
    }
}
