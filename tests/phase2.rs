use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use rust_web_digest::{
    config::PublishingConfig,
    domain::{Candidate, CandidateKind},
    github_issues::{build_issue_draft, extract_candidate_ids},
};

#[test]
fn issue_draft_contains_editorial_labels_month_and_stable_marker() {
    let candidate = candidate();
    let draft = build_issue_draft(&candidate, Some("Axum"), &PublishingConfig::default());

    assert_eq!(draft.title, "[Axum] Axum 1.0 released");
    assert_eq!(draft.milestone_title, "July 2026");
    assert!(draft.labels.contains(&"candidate".to_owned()));
    assert!(draft.labels.contains(&"status:new".to_owned()));
    assert!(draft.labels.contains(&"category:frameworks".to_owned()));
    assert!(draft.labels.contains(&"type:release".to_owned()));

    let ids = extract_candidate_ids(&draft.body);
    assert_eq!(ids, vec![candidate.id]);
}

#[test]
fn issue_draft_uses_source_summary_and_metadata() {
    let draft = build_issue_draft(
        &candidate(),
        Some("Axum"),
        &PublishingConfig::default(),
    );

    assert!(draft.body.contains("A stable release with routing improvements."));
    assert!(draft.body.contains("**tag_name:** v1.0.0"));
    assert!(draft.body.contains("status:selected"));
}

fn candidate() -> Candidate {
    Candidate {
        id: "github-release:tokio-rs/axum:1000".to_owned(),
        kind: CandidateKind::GitHubRelease,
        title: "Axum 1.0 released".to_owned(),
        url: "https://github.com/tokio-rs/axum/releases/tag/v1.0.0".to_owned(),
        source_id: "github:tokio-rs/axum".to_owned(),
        project_id: Some("axum".to_owned()),
        category: "frameworks".to_owned(),
        published_at: Utc.with_ymd_and_hms(2026, 7, 10, 8, 0, 0).unwrap(),
        discovered_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap(),
        summary: Some("A stable release with routing improvements.".to_owned()),
        raw_content: None,
        metadata: BTreeMap::from([("tag_name".to_owned(), "v1.0.0".to_owned())]),
    }
}
