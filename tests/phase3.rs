use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use rust_web_digest::{
    config::{PublishingConfig, ReconciliationConfig},
    domain::{Candidate, CandidateKind},
    github_issues::{build_story_issue_draft, extract_candidate_ids, extract_story_ids, merge_managed_body},
    reconcile::reconcile_candidates,
};

#[test]
fn github_release_and_crate_publication_become_one_story() {
    let release = candidate(
        "github-release:tokio-rs/axum:100",
        CandidateKind::GitHubRelease,
        "Axum 1.2.3 released",
        10,
        BTreeMap::from([("tag_name".to_owned(), "axum-v1.2.3".to_owned())]),
    );
    let crate_release = candidate(
        "crate-release:axum:200",
        CandidateKind::CrateRelease,
        "axum 1.2.3 published to crates.io",
        10,
        BTreeMap::from([("version".to_owned(), "1.2.3".to_owned())]),
    );

    let stories = reconcile_candidates(
        &[release, crate_release],
        &ReconciliationConfig::default(),
    );

    assert_eq!(stories.len(), 1);
    assert_eq!(stories[0].id, "release:axum:1.2.3");
    assert_eq!(stories[0].candidates.len(), 2);
}

#[test]
fn matching_version_article_is_attached_to_release_story() {
    let release = candidate(
        "release",
        CandidateKind::GitHubRelease,
        "Axum 1.2.3 released",
        10,
        BTreeMap::from([("tag_name".to_owned(), "v1.2.3".to_owned())]),
    );
    let mut article = candidate(
        "article",
        CandidateKind::FeedArticle,
        "Migrating to Axum 1.2.3",
        12,
        BTreeMap::new(),
    );
    article.url = "https://example.com/axum-1-2-3".to_owned();

    let stories = reconcile_candidates(
        &[release, article],
        &ReconciliationConfig::default(),
    );

    assert_eq!(stories.len(), 1);
    assert_eq!(stories[0].candidates.len(), 2);
}

#[test]
fn unrelated_same_project_article_stays_separate() {
    let release = candidate(
        "release",
        CandidateKind::GitHubRelease,
        "Axum 1.2.3 released",
        10,
        BTreeMap::from([("tag_name".to_owned(), "v1.2.3".to_owned())]),
    );
    let article = candidate(
        "article",
        CandidateKind::FeedArticle,
        "Understanding extractors in Axum",
        12,
        BTreeMap::new(),
    );

    let stories = reconcile_candidates(
        &[release, article],
        &ReconciliationConfig::default(),
    );

    assert_eq!(stories.len(), 2);
}

#[test]
fn same_version_in_different_projects_does_not_merge() {
    let left = candidate_for_project(
        "axum-release",
        CandidateKind::GitHubRelease,
        "axum",
        "Axum 1.0.0",
        10,
        BTreeMap::from([("tag_name".to_owned(), "v1.0.0".to_owned())]),
    );
    let right = candidate_for_project(
        "sqlx-release",
        CandidateKind::GitHubRelease,
        "sqlx",
        "SQLx 1.0.0",
        10,
        BTreeMap::from([("tag_name".to_owned(), "v1.0.0".to_owned())]),
    );

    let stories = reconcile_candidates(&[left, right], &ReconciliationConfig::default());
    assert_eq!(stories.len(), 2);
}

#[test]
fn story_issue_contains_story_and_all_source_markers() {
    let release = candidate(
        "release",
        CandidateKind::GitHubRelease,
        "Axum 1.2.3 released",
        10,
        BTreeMap::from([("tag_name".to_owned(), "v1.2.3".to_owned())]),
    );
    let crate_release = candidate(
        "crate",
        CandidateKind::CrateRelease,
        "axum 1.2.3 published to crates.io",
        10,
        BTreeMap::from([("version".to_owned(), "1.2.3".to_owned())]),
    );
    let stories = reconcile_candidates(
        &[release, crate_release],
        &ReconciliationConfig::default(),
    );

    let draft = build_story_issue_draft(
        &stories[0],
        Some("Axum"),
        &PublishingConfig::default(),
    );

    assert_eq!(extract_story_ids(&draft.body), vec!["release:axum:1.2.3"]);
    assert_eq!(extract_candidate_ids(&draft.body).len(), 2);
    assert!(draft.labels.contains(&"type:release".to_owned()));
    assert!(draft.labels.contains(&"type:crate".to_owned()));
}

#[test]
fn machine_refresh_does_not_destroy_editorial_notes() {
    let existing = "<!-- rust-web-digest:managed:start -->\nold sources\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nThis is my human analysis.";
    let generated = "<!-- rust-web-digest:managed:start -->\nnew sources\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nplaceholder";

    let result = merge_managed_body(existing, generated);
    assert!(result.contains("new sources"));
    assert!(result.contains("This is my human analysis."));
    assert!(!result.contains("placeholder"));
}

#[test]
fn phase2_issue_migration_preserves_editorial_notes() {
    let existing = "## Candidate\n\nold machine content\n\n## Editorial notes\n\nImportant human note.\n\n<!-- rust-web-digest:candidate-id:release -->";
    let generated = "<!-- rust-web-digest:managed:start -->\nnew story content\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nplaceholder";

    let result = merge_managed_body(existing, generated);
    assert!(result.contains("new story content"));
    assert!(result.contains("Important human note."));
    assert!(!result.contains("old machine content"));
    assert!(!result.contains("placeholder"));
}

fn candidate(
    id: &str,
    kind: CandidateKind,
    title: &str,
    day: u32,
    metadata: BTreeMap<String, String>,
) -> Candidate {
    candidate_for_project(id, kind, "axum", title, day, metadata)
}

fn candidate_for_project(
    id: &str,
    kind: CandidateKind,
    project: &str,
    title: &str,
    day: u32,
    metadata: BTreeMap<String, String>,
) -> Candidate {
    Candidate {
        id: id.to_owned(),
        kind,
        title: title.to_owned(),
        url: format!("https://example.com/{id}"),
        source_id: "fixture".to_owned(),
        project_id: Some(project.to_owned()),
        category: "frameworks".to_owned(),
        published_at: Utc.with_ymd_and_hms(2026, 7, day, 8, 0, 0).unwrap(),
        discovered_at: Utc.with_ymd_and_hms(2026, 7, day, 9, 0, 0).unwrap(),
        summary: None,
        raw_content: None,
        metadata,
    }
}
