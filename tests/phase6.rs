use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, bail};
use rust_web_digest::{
    ai::{
        ActionRequired, AiFailurePolicy, DraftConfidence, DraftGenerator, EditorialDraft,
        SourcedClaim, enrich_document, story_fingerprint,
    },
    composer::{
        CompositionMode, NewsletterDocument, NewsletterSection, NewsletterSource, NewsletterStory,
        render_markdown,
    },
    config::AppConfig,
};

fn config() -> AppConfig {
    AppConfig::load("config/sources.toml").unwrap()
}

fn story() -> NewsletterStory {
    NewsletterStory {
        title: "Axum 1.2.3 released".to_owned(),
        category: "frameworks".to_owned(),
        project: Some("Axum".to_owned()),
        version: Some("1.2.3".to_owned()),
        published_on: "July 10, 2026".to_owned(),
        summary: Some("Source-backed release summary.".to_owned()),
        sources: vec![NewsletterSource {
            kind: "release".to_owned(),
            title: "Axum 1.2.3 released".to_owned(),
            url: "https://example.com/axum-1.2.3".to_owned(),
            published_on: "2026-07-10".to_owned(),
            summary: Some("Routing behavior was improved.".to_owned()),
            content: Some("Full release notes source material.".to_owned()),
        }],
        issue_url: None,
        draft: None,
    }
}

fn document() -> NewsletterDocument {
    NewsletterDocument {
        month: "2026-07".to_owned(),
        title: "Rust Web Monthly — July 2026".to_owned(),
        mode: CompositionMode::Automatic,
        story_count: 1,
        sections: vec![NewsletterSection {
            category: "frameworks".to_owned(),
            title: "Frameworks".to_owned(),
            stories: vec![story()],
        }],
    }
}

fn draft(source_url: &str) -> EditorialDraft {
    EditorialDraft {
        headline: "Axum 1.2.3 sharpens routing behavior".to_owned(),
        what_changed: "The release includes routing behavior improvements described by the source.".to_owned(),
        why_it_matters: "Applications depending on the affected routing behavior may want to review the release notes.".to_owned(),
        who_is_affected: "Teams maintaining Axum services.".to_owned(),
        action_required: ActionRequired::ConsiderUpdate,
        action: "Review the release notes before updating production services.".to_owned(),
        confidence: DraftConfidence::Medium,
        sourced_claims: vec![SourcedClaim {
            claim: "The release changes routing behavior.".to_owned(),
            source_urls: vec![source_url.to_owned()],
        }],
    }
}

#[derive(Clone)]
struct FakeGenerator {
    calls: Arc<AtomicUsize>,
    fail: bool,
    source_url: String,
}

impl DraftGenerator for FakeGenerator {
    fn provider(&self) -> &str {
        "fake"
    }

    fn model(&self) -> &str {
        "fake-model"
    }

    fn generate<'a>(
        &'a self,
        _story: &'a NewsletterStory,
    ) -> impl Future<Output = Result<EditorialDraft>> + Send + 'a {
        async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                bail!("synthetic drafting failure");
            }
            Ok(draft(&self.source_url))
        }
    }
}

#[tokio::test]
async fn ai_enrichment_generates_and_then_reuses_cache() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let generator = FakeGenerator {
        calls: calls.clone(),
        fail: false,
        source_url: "https://example.com/axum-1.2.3".to_owned(),
    };

    let mut first = document();
    let first_report = enrich_document(
        &mut first,
        &generator,
        directory.path(),
        false,
        AiFailurePolicy::Fail,
    )
    .await
    .unwrap();

    assert_eq!(first_report.generated, 1);
    assert_eq!(first_report.cached, 0);
    assert!(first.sections[0].stories[0].draft.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let mut second = document();
    let second_report = enrich_document(
        &mut second,
        &generator,
        directory.path(),
        false,
        AiFailurePolicy::Fail,
    )
    .await
    .unwrap();

    assert_eq!(second_report.generated, 0);
    assert_eq!(second_report.cached, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fallback_policy_keeps_deterministic_story_when_ai_fails() {
    let directory = tempfile::tempdir().unwrap();
    let generator = FakeGenerator {
        calls: Arc::new(AtomicUsize::new(0)),
        fail: true,
        source_url: "https://example.com/axum-1.2.3".to_owned(),
    };
    let mut document = document();

    let report = enrich_document(
        &mut document,
        &generator,
        directory.path(),
        false,
        AiFailurePolicy::Fallback,
    )
    .await
    .unwrap();

    assert_eq!(report.failed, 1);
    assert!(document.sections[0].stories[0].draft.is_none());
}

#[tokio::test]
async fn unknown_ai_source_url_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let generator = FakeGenerator {
        calls: Arc::new(AtomicUsize::new(0)),
        fail: false,
        source_url: "https://invented.example/not-a-source".to_owned(),
    };
    let mut document = document();

    let error = enrich_document(
        &mut document,
        &generator,
        directory.path(),
        true,
        AiFailurePolicy::Fail,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("unknown source URL"));
}

#[test]
fn fingerprint_changes_when_source_material_changes() {
    let first = story();
    let mut second = story();
    second.sources[0].summary = Some("Different source material.".to_owned());

    assert_ne!(
        story_fingerprint(&first).unwrap(),
        story_fingerprint(&second).unwrap()
    );
}

#[test]
fn markdown_renders_structured_ai_draft_without_hiding_sources() {
    let config = config();
    let mut document = document();
    document.sections[0].stories[0].draft = Some(draft("https://example.com/axum-1.2.3"));

    let markdown = render_markdown(&document, &config.newsletter);
    assert!(markdown.contains("### Axum 1.2.3 sharpens routing behavior"));
    assert!(markdown.contains("**What changed:**"));
    assert!(markdown.contains("**Why it matters:**"));
    assert!(markdown.contains("**Who should care:**"));
    assert!(markdown.contains("**Action:** Consider updating"));
    assert!(markdown.contains("_Draft confidence: medium_"));
    assert!(markdown.contains("**Sources:** [release notes](https://example.com/axum-1.2.3)"));
}

#[test]
fn ai_configuration_rejects_unknown_provider() {
    let mut config = config();
    config.ai.provider = "unknown".to_owned();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("supports only 'openai'"));
}
