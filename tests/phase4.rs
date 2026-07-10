use rust_web_digest::{
    config::PublishingConfig,
    editorial::{
        EditorialMonth, EditorialStatus, extract_editorial_notes, merge_parent_body,
        transition_labels,
    },
};

#[test]
fn editorial_month_parses_and_formats_title() {
    let month = EditorialMonth::parse("2026-07").unwrap();
    assert_eq!(month.key, "2026-07");
    assert_eq!(month.title, "July 2026");
}

#[test]
fn editorial_month_rejects_invalid_input() {
    let error = EditorialMonth::parse("July-2026").unwrap_err().to_string();
    assert!(error.contains("expected YYYY-MM"));
}

#[test]
fn status_transition_preserves_non_status_labels() {
    let publishing = PublishingConfig::default();
    let labels = vec![
        "candidate".to_owned(),
        "category:frameworks".to_owned(),
        publishing.new_status_label.clone(),
    ];

    let next = transition_labels(&labels, EditorialStatus::Selected, &publishing);

    assert!(next.contains(&"candidate".to_owned()));
    assert!(next.contains(&"category:frameworks".to_owned()));
    assert!(next.contains(&publishing.selected_status_label));
    assert!(!next.contains(&publishing.new_status_label));
}

#[test]
fn parent_refresh_preserves_human_editorial_notes() {
    let existing = "<!-- rust-web-digest:month:2026-07 -->\n<!-- rust-web-digest:parent-managed:start -->\nold counts\n<!-- rust-web-digest:parent-managed:end -->\n\n## Editorial notes\n\nKeep this plan.";
    let generated = "<!-- rust-web-digest:month:2026-07 -->\n<!-- rust-web-digest:parent-managed:start -->\nnew counts\n<!-- rust-web-digest:parent-managed:end -->\n\n## Editorial notes\n\nplaceholder";

    let merged = merge_parent_body(existing, generated);
    assert!(merged.contains("new counts"));
    assert!(merged.contains("Keep this plan."));
    assert!(!merged.contains("old counts"));
    assert!(!merged.contains("placeholder"));
}

#[test]
fn extracts_candidate_editorial_notes() {
    let body = "machine content\n\n## Editorial notes\n\nReview migration risk before selection.";
    assert_eq!(
        extract_editorial_notes(body).as_deref(),
        Some("Review migration risk before selection.")
    );
}
