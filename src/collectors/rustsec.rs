use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{
    collectors::CollectionWindow,
    domain::{Candidate, CandidateKind},
};

#[derive(Debug, Serialize)]
struct OsvQuery<'a> {
    package: OsvPackage<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_token: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct OsvPackage<'a> {
    name: &'a str,
    ecosystem: &'static str,
}

#[derive(Debug, Deserialize)]
struct OsvQueryResponse {
    #[serde(default)]
    vulns: Vec<OsvVulnerability>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvVulnerability {
    id: String,
    summary: Option<String>,
    details: Option<String>,
    published: Option<DateTime<Utc>>,
    modified: Option<DateTime<Utc>>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    references: Vec<OsvReference>,
}

#[derive(Debug, Deserialize)]
struct OsvReference {
    #[serde(rename = "type")]
    kind: Option<String>,
    url: String,
}

pub struct RustSecCollector<'a> {
    client: &'a Client,
    api_url: &'a str,
}

impl<'a> RustSecCollector<'a> {
    pub fn new(client: &'a Client, api_url: &'a str) -> Self {
        Self { client, api_url }
    }

    pub async fn collect_crate(
        &self,
        crate_name: &str,
        project_id: Option<&str>,
        category: &str,
        window: &CollectionWindow,
        discovered_at: &DateTime<Utc>,
    ) -> Result<Vec<Candidate>> {
        let url = format!("{}/v1/query", self.api_url.trim_end_matches('/'));
        let mut page_token: Option<String> = None;
        let mut output = Vec::new();

        loop {
            let response = self
                .client
                .post(&url)
                .json(&OsvQuery {
                    package: OsvPackage {
                        name: crate_name,
                        ecosystem: "crates.io",
                    },
                    page_token: page_token.as_deref(),
                })
                .send()
                .await
                .with_context(|| format!("OSV request failed for crate '{crate_name}'"))?
                .error_for_status()
                .with_context(|| format!("OSV returned an error for crate '{crate_name}'"))?
                .json::<OsvQueryResponse>()
                .await
                .with_context(|| format!("invalid OSV response for crate '{crate_name}'"))?;

            output.extend(response.vulns.into_iter().filter_map(|vulnerability| {
                vulnerability_to_candidate(
                    crate_name,
                    project_id,
                    category,
                    vulnerability,
                    window,
                    discovered_at,
                )
            }));

            match response.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        Ok(output)
    }
}

fn vulnerability_to_candidate(
    crate_name: &str,
    project_id: Option<&str>,
    category: &str,
    vulnerability: OsvVulnerability,
    window: &CollectionWindow,
    discovered_at: &DateTime<Utc>,
) -> Option<Candidate> {
    let rustsec_id = if vulnerability.id.starts_with("RUSTSEC-") {
        Some(vulnerability.id.clone())
    } else {
        vulnerability
            .aliases
            .iter()
            .find(|alias| alias.starts_with("RUSTSEC-"))
            .cloned()
    }?;

    let published_at = vulnerability.published.or(vulnerability.modified)?;
    if !window.contains(&published_at) {
        return None;
    }

    let url = vulnerability
        .references
        .iter()
        .find(|reference| reference.kind.as_deref() == Some("ADVISORY"))
        .or_else(|| vulnerability.references.first())
        .map(|reference| reference.url.clone())
        .unwrap_or_else(|| format!("https://rustsec.org/advisories/{rustsec_id}.html"));

    let title = vulnerability
        .summary
        .clone()
        .unwrap_or_else(|| format!("RustSec advisory {rustsec_id} for {crate_name}"));
    let mut metadata = BTreeMap::new();
    metadata.insert("crate".to_owned(), crate_name.to_owned());
    metadata.insert("rustsec_id".to_owned(), rustsec_id.clone());
    metadata.insert("osv_id".to_owned(), vulnerability.id);

    Some(Candidate {
        id: format!("rustsec:{rustsec_id}"),
        kind: CandidateKind::SecurityAdvisory,
        title,
        url,
        source_id: "rustsec".to_owned(),
        project_id: project_id.map(ToOwned::to_owned),
        category: category.to_owned(),
        published_at,
        discovered_at: discovered_at.clone(),
        summary: vulnerability.summary,
        raw_content: vulnerability.details,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn converts_rustsec_fixture_to_candidate() {
        let response: OsvQueryResponse =
            serde_json::from_str(include_str!("../../tests/fixtures/osv_query.json")).unwrap();
        let window = CollectionWindow {
            since: Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
            until: Utc.with_ymd_and_hms(2026, 7, 31, 23, 59, 59).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();

        let candidate = vulnerability_to_candidate(
            "axum",
            Some("axum"),
            "security",
            response.vulns.into_iter().next().unwrap(),
            &window,
            &now,
        )
        .unwrap();

        assert_eq!(candidate.id, "rustsec:RUSTSEC-2026-0001");
        assert_eq!(candidate.project_id.as_deref(), Some("axum"));
    }
}
