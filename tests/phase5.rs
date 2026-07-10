use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use rust_web_digest::{
    composer::{
        CompositionMode, compose_automatic, compose_editorial, render_markdown,
        write_newsletter,
    },
    config::AppConfig,
    domain::{Candidate, CandidateKind, Story},
    editorial::{EditorialMonth, EditorialStatus, EditorialStoryRecord},
};

fn config() -> AppConfig {
    AppConfig::load("config/sources.toml").unwrap()
}

fn release_candidate() -> Candidate {
    Candidate {
        id: "github-release:tokio-rs/axum:100".to_owned(),
        kind: CandidateKind::GitHubRelease,
        title: "Axum 1.2.3 released".to_owned(),
        url: "https://github.com/tokio-rs/axum/releases/tag/v1.2.3".to_owned(),
        source_id: "github:tokio-rs/axum".to_owned(),
        project_id: Some("axum".to_owned()),
        category: "frameworks".to_owned(),
        published_at: Utc.with_ymd_and_hms(2026, 7, 10, 8, 0, 0).unwrap(),
        discovered_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap(),
        summary: Some("A stable Axum release with routing improvements.".to_owned()),
        raw_content: None,
        metadata: BTreeMap::from([
            ("tag_name".to_owned(), "v1.2.3".to_owned()),
            ("version".to_owned(), "1.2.3".to_owned()),
        ]),
    }
}

fn release_story() -> Story {
    Story {
        id: "release:axum:1.2.3".to_owned(),
        project_id: Some("axum".to_owned()),
        category: "frameworks".to_owned(),
        title: "Axum 1.2.3 released".to_owned(),
        version: Some("1.2.3".to_owned()),
        published_at: Utc.with_ymd_and_hms(2026, 7, 10, 8, 0, 0).unwrap(),
        discovered_at: Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap(),
        candidates: vec![release_candidate()],
    }
}

#[test]
fn automatic_mode_composes_directly_from_reconciled_stories() {
    let config = config();
    let month = EditorialMonth::parse("2026-07").unwrap();
    let document = compose_automatic(&month, &[release_story()], &config);

    assert_eq!(document.mode, CompositionMode::Automatic);
    assert_eq!(document.story_count, 1);
    assert_eq!(document.sections.len(), 1);
    assert_eq!(document.sections[0].title, "Frameworks");
    assert_eq!(document.sections[0].stories[0].project.as_deref(), Some("Axum"));
}

#[test]
fn automatic_mode_excludes_stories_outside_target_month() {
    let config = config();
    let month = EditorialMonth::parse("2026-08").unwrap();
    let document = compose_automatic(&month, &[release_story()], &config);

    assert_eq!(document.story_count, 0);
    assert!(document.sections.is_empty());
}

#[test]
fn editorial_notes_take_precedence_over_source_summary() {
    let config = config();
    let month = EditorialMonth::parse("2026-07").unwrap();
    let body = "<!-- rust-web-digest:managed:start -->\n## Story\n\n- **Project:** Axum\n- **Category:** frameworks\n- **Version:** 1.2.3\n- **First published:** 2026-07-10T08:00:00+00:00\n- **Sources:** 1\n\n## Sources\n\n### release · 2026-07-10\n\n[Axum 1.2.3 released](https://example.com/release)\n\nAutomatic source summary.\n\n- **tag_name:** v1.2.3\n\n<!-- rust-web-digest:story-id:release:axum:1.2.3 -->\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nThis is the human-written editorial explanation.";
    let record = EditorialStoryRecord {
        issue_number: 42,
        issue_url: "https://github.com/example/repo/issues/42".to_owned(),
        title: "[Axum] Axum 1.2.3 released".to_owned(),
        story_id: Some("release:axum:1.2.3".to_owned()),
        candidate_ids: vec!["github-release:tokio-rs/axum:100".to_owned()],
        status: Some(EditorialStatus::Selected),
        category: Some("frameworks".to_owned()),
        kinds: vec!["release".to_owned()],
        labels: vec!["status:selected".to_owned()],
        milestone: "July 2026".to_owned(),
        body: body.to_owned(),
        editorial_notes: Some("This is the human-written editorial explanation.".to_owned()),
    };

    let document = compose_editorial(&month, &[record], &config).unwrap();
    let story = &document.sections[0].stories[0];
    assert_eq!(
        story.summary.as_deref(),
        Some("This is the human-written editorial explanation.")
    );
    assert_eq!(story.sources.len(), 1);
    assert_eq!(story.sources[0].url, "https://example.com/release");
}

#[test]
fn markdown_and_manifest_are_ready_for_future_release_publication() {
    let config = config();
    let month = EditorialMonth::parse("2026-07").unwrap();
    let document = compose_automatic(&month, &[release_story()], &config);
    let markdown = render_markdown(&document, &config.newsletter);

    assert!(markdown.contains("# Rust Web Monthly — July 2026"));
    assert!(markdown.contains("## Frameworks"));
    assert!(markdown.contains("### Axum 1.2.3 released"));
    assert!(markdown.contains("**Sources:** [release notes]"));

    let directory = tempfile::tempdir().unwrap();
    let markdown_path = directory.path().join("2026-07.md");
    let manifest_path = directory.path().join("2026-07.manifest.json");
    let written = write_newsletter(
        &document,
        &config.newsletter,
        Some(&markdown_path),
        Some(&manifest_path),
    )
    .unwrap();

    assert_eq!(written.manifest.release_tag, "digest-2026-07");
    assert_eq!(written.manifest.release_name, "Rust Web Digest — July 2026");
    assert_eq!(
        written.manifest.release_asset_name,
        "rust-web-digest-2026-07.md"
    );
    assert!(written.markdown_path.exists());
    assert!(written.manifest_path.exists());
}

#[test]
fn newsletter_configuration_rejects_duplicate_category_order_entries() {
    let mut config = config();
    config.newsletter.category_order.push("frameworks".to_owned());
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("duplicate category"));
}

#[test]
fn editorial_composition_ignores_unselected_records_from_offline_input() {
    let config = config();
    let month = EditorialMonth::parse("2026-07").unwrap();
    let record = EditorialStoryRecord {
        issue_number: 99,
        issue_url: "https://github.com/example/repo/issues/99".to_owned(),
        title: "[Axum] Not selected".to_owned(),
        story_id: Some("release:axum:9.9.9".to_owned()),
        candidate_ids: vec![],
        status: Some(EditorialStatus::Watch),
        category: Some("frameworks".to_owned()),
        kinds: vec!["release".to_owned()],
        labels: vec!["status:watch".to_owned()],
        milestone: "July 2026".to_owned(),
        body: "## Sources".to_owned(),
        editorial_notes: None,
    };

    let document = compose_editorial(&month, &[record], &config).unwrap();
    assert_eq!(document.story_count, 0);
}

#[test]
fn newsletter_configuration_rejects_unsafe_release_tag_prefix() {
    let mut config = config();
    config.newsletter.release_tag_prefix = "newsletter monthly".to_owned();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("release_tag_prefix may contain only"));
}
