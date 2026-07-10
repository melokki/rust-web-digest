use std::{collections::BTreeMap, fs};

use chrono::{TimeZone, Utc};
use feed_rs::parser;
use rust_web_digest::{
    collectors::{CollectionWindow, feed::entry_to_candidate},
    config::{AppConfig, FeedConfig},
    domain::{Candidate, CandidateKind},
    normalize::{deduplicate_exact, normalize_candidates},
    storage::JsonlStore,
};


#[test]
fn starter_source_registry_is_valid_and_curated() {
    let config = AppConfig::load("config/sources.toml").unwrap();
    assert_eq!(config.projects.len(), 20);
    assert!(config.projects.iter().any(|project| project.id == "axum"));
    assert!(config.projects.iter().any(|project| project.id == "sqlx"));
    assert!(config.projects.iter().any(|project| project.id == "leptos"));
}

#[test]
fn rss_fixture_is_parsed_and_keyword_filtered() {
    let bytes = fs::read("tests/fixtures/rss.xml").unwrap();
    let parsed = parser::parse(bytes.as_slice()).unwrap();
    let config = FeedConfig {
        id: "example".to_owned(),
        name: "Example".to_owned(),
        url: "https://example.com/feed.xml".to_owned(),
        category: "articles".to_owned(),
        project_id: None,
        required_any: vec!["axum".to_owned(), "tokio".to_owned()],
    };
    let window = CollectionWindow {
        since: Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        until: Utc.with_ymd_and_hms(2026, 7, 31, 23, 59, 59).unwrap(),
    };
    let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();

    let candidates = parsed
        .entries
        .iter()
        .filter_map(|entry| entry_to_candidate(&config, entry, &window, &now))
        .collect::<Vec<_>>();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].title, "Building services with Axum");
}

#[test]
fn atom_fixture_is_parsed() {
    let bytes = fs::read("tests/fixtures/atom.xml").unwrap();
    let parsed = parser::parse(bytes.as_slice()).unwrap();
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(
        parsed.entries[0].title.as_ref().unwrap().content,
        "Tokio networking changes"
    );
}

#[test]
fn jsonl_store_merges_without_duplicate_ids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("candidates.jsonl");
    let store = JsonlStore::new(&path);

    let first = make_candidate("id-1", "https://example.com/one");
    let second = make_candidate("id-2", "https://example.com/two");

    let result = store.merge_and_save(vec![first.clone()]).unwrap();
    assert_eq!(result.added, 1);
    let result = store
        .merge_and_save(vec![first.clone(), second.clone()])
        .unwrap();
    assert_eq!(result.added, 1);
    assert_eq!(result.total, 2);
    assert_eq!(store.load().unwrap().len(), 2);
}


#[test]
fn jsonl_store_refreshes_existing_candidate_by_stable_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("candidates.jsonl");
    let store = JsonlStore::new(&path);

    let original = make_candidate("id-1", "https://example.com/one");
    store.merge_and_save(vec![original]).unwrap();

    let mut updated = make_candidate("id-1", "https://example.com/one");
    updated.title = "Updated title".to_owned();
    let result = store.merge_and_save(vec![updated]).unwrap();

    assert_eq!(result.added, 0);
    assert_eq!(store.load().unwrap()[0].title, "Updated title");
}

#[test]
fn normalization_and_exact_deduplication_are_deterministic() {
    let candidates = vec![
        make_candidate("id-1", "https://example.com/article/#section"),
        make_candidate("id-2", "https://example.com/article"),
    ];
    let result = deduplicate_exact(normalize_candidates(candidates));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].url, "https://example.com/article");
}

fn make_candidate(id: &str, url: &str) -> Candidate {
    Candidate {
        id: id.to_owned(),
        kind: CandidateKind::FeedArticle,
        title: "Example".to_owned(),
        url: url.to_owned(),
        source_id: "test".to_owned(),
        project_id: None,
        category: "articles".to_owned(),
        published_at: Utc.with_ymd_and_hms(2026, 7, 10, 0, 0, 0).unwrap(),
        discovered_at: Utc.with_ymd_and_hms(2026, 7, 10, 1, 0, 0).unwrap(),
        summary: None,
        raw_content: None,
        metadata: BTreeMap::new(),
    }
}
