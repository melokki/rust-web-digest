use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use feed_rs::{model::Entry, parser};
use reqwest::Client;

use crate::{
    collectors::CollectionWindow,
    config::FeedConfig,
    domain::{Candidate, CandidateKind},
};

pub struct FeedCollector<'a> {
    client: &'a Client,
}

impl<'a> FeedCollector<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    pub async fn collect_feed(
        &self,
        config: &FeedConfig,
        window: &CollectionWindow,
        discovered_at: &DateTime<Utc>,
    ) -> Result<Vec<Candidate>> {
        let bytes = self
            .client
            .get(&config.url)
            .send()
            .await
            .with_context(|| format!("feed request failed for '{}'", config.id))?
            .error_for_status()
            .with_context(|| format!("feed '{}' returned an error", config.id))?
            .bytes()
            .await
            .with_context(|| format!("failed reading feed '{}'", config.id))?;

        let feed = parser::parse(bytes.as_ref())
            .with_context(|| format!("failed parsing feed '{}'", config.id))?;
        Ok(feed
            .entries
            .iter()
            .filter_map(|entry| entry_to_candidate(config, entry, window, discovered_at))
            .collect())
    }
}

pub fn entry_to_candidate(
    config: &FeedConfig,
    entry: &Entry,
    window: &CollectionWindow,
    discovered_at: &DateTime<Utc>,
) -> Option<Candidate> {
    let published_at = entry.published.clone().or_else(|| entry.updated.clone());
    if let Some(timestamp) = published_at.as_ref() {
        if !window.contains(timestamp) {
            return None;
        }
    }

    let title = entry
        .title
        .as_ref()
        .map(|text| text.content.trim().to_owned())
        .filter(|value| !value.is_empty())?;
    let summary = entry
        .summary
        .as_ref()
        .map(|text| text.content.trim().to_owned())
        .filter(|value| !value.is_empty());
    let raw_content = entry
        .content
        .as_ref()
        .and_then(|content| content.body.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if !matches_keywords(config, &title, summary.as_deref(), raw_content.as_deref()) {
        return None;
    }

    let url = entry
        .links
        .iter()
        .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or_else(|| entry.links.first())?
        .href
        .clone();
    let stable_entry_id = if entry.id.trim().is_empty() {
        &url
    } else {
        entry.id.trim()
    };

    let mut metadata = BTreeMap::new();
    metadata.insert("feed_name".to_owned(), config.name.clone());
    metadata.insert("entry_id".to_owned(), stable_entry_id.to_owned());

    Some(Candidate {
        id: format!("feed:{}:{stable_entry_id}", config.id),
        kind: CandidateKind::FeedArticle,
        title,
        url,
        source_id: format!("feed:{}", config.id),
        project_id: config.project_id.clone(),
        category: config.category.clone(),
        published_at: published_at.unwrap_or_else(|| discovered_at.clone()),
        discovered_at: discovered_at.clone(),
        summary,
        raw_content,
        metadata,
    })
}

fn matches_keywords(
    config: &FeedConfig,
    title: &str,
    summary: Option<&str>,
    content: Option<&str>,
) -> bool {
    let haystack = format!(
        "{} {} {}",
        title,
        summary.unwrap_or_default(),
        content.unwrap_or_default()
    )
    .to_lowercase();

    if config
        .excluded_any
        .iter()
        .any(|keyword| haystack.contains(&keyword.to_lowercase()))
    {
        return false;
    }

    config.required_any.is_empty()
        || config
            .required_any
            .iter()
            .any(|keyword| haystack.contains(&keyword.to_lowercase()))
}
