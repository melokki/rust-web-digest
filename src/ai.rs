use std::{
    collections::HashSet,
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::{
    composer::{NewsletterDocument, NewsletterStory},
    config::AiConfig,
};

const DRAFT_SCHEMA_VERSION: &str = "phase6-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFailurePolicy {
    Fail,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRequired {
    None,
    ConsiderUpdate,
    MigrationRequired,
    SecurityUpdate,
    Investigate,
}

impl ActionRequired {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::ConsiderUpdate => "Consider updating",
            Self::MigrationRequired => "Migration required",
            Self::SecurityUpdate => "Security update",
            Self::Investigate => "Investigate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftConfidence {
    High,
    Medium,
    Low,
}

impl DraftConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcedClaim {
    pub claim: String,
    pub source_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorialDraft {
    pub headline: String,
    pub what_changed: String,
    pub why_it_matters: String,
    pub who_is_affected: String,
    pub action_required: ActionRequired,
    pub action: String,
    pub confidence: DraftConfidence,
    pub sourced_claims: Vec<SourcedClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedDraft {
    pub fingerprint: String,
    pub provider: String,
    pub model: String,
    pub generated_at: String,
    pub draft: EditorialDraft,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiDraftingReport {
    pub generated: usize,
    pub cached: usize,
    pub failed: usize,
    pub failures: Vec<String>,
}

pub trait DraftGenerator {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn generate<'a>(
        &'a self,
        story: &'a NewsletterStory,
    ) -> impl Future<Output = Result<EditorialDraft>> + Send + 'a;
}

pub struct OpenAiDraftGenerator<'a> {
    client: &'a Client,
    config: &'a AiConfig,
    api_key: &'a str,
}

impl<'a> OpenAiDraftGenerator<'a> {
    pub fn new(client: &'a Client, config: &'a AiConfig, api_key: &'a str) -> Self {
        Self {
            client,
            config,
            api_key,
        }
    }

    async fn request(&self, story: &NewsletterStory) -> Result<EditorialDraft> {
        let payload = build_openai_request(
            story,
            &self.config.model,
            self.config.max_source_chars,
        )?;

        for attempt in 0..=self.config.max_retries {
            let response = self
                .client
                .post(&self.config.api_url)
                .bearer_auth(self.api_key)
                .json(&payload)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let body: ResponsesApiResponse = response
                        .json()
                        .await
                        .context("failed to decode OpenAI Responses API response")?;
                    let text = extract_output_text(&body)?;
                    let draft: EditorialDraft = serde_json::from_str(&text)
                        .context("OpenAI structured output was not valid EditorialDraft JSON")?;
                    validate_draft_sources(&draft, story)?;
                    validate_draft_content(&draft)?;
                    return Ok(draft);
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let message = format!(
                        "OpenAI Responses API returned {status}: {}",
                        truncate(&body, 800)
                    );
                    if !retryable_status(status) || attempt == self.config.max_retries {
                        bail!(message);
                    }
                }
                Err(error) => {
                    let message = format!("OpenAI request failed: {error}");
                    if attempt == self.config.max_retries {
                        bail!(message);
                    }
                }
            }

            sleep(backoff_delay(
                self.config.initial_backoff_ms,
                attempt,
            ))
            .await;
        }

        bail!("OpenAI retry loop exhausted unexpectedly")
    }
}

impl DraftGenerator for OpenAiDraftGenerator<'_> {
    fn provider(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn generate<'a>(
        &'a self,
        story: &'a NewsletterStory,
    ) -> impl Future<Output = Result<EditorialDraft>> + Send + 'a {
        async move { self.request(story).await }
    }
}

pub async fn enrich_document<G: DraftGenerator>(
    document: &mut NewsletterDocument,
    generator: &G,
    cache_dir: &Path,
    refresh: bool,
    failure_policy: AiFailurePolicy,
) -> Result<AiDraftingReport> {
    let mut report = AiDraftingReport::default();
    let month_cache = cache_dir.join(&document.month);

    for section in &mut document.sections {
        for story in &mut section.stories {
            let fingerprint = story_fingerprint(story)?;
            let cache_path = month_cache.join(format!("{fingerprint}.json"));

            let cached = if refresh {
                None
            } else {
                load_cached_draft(&cache_path)?
            };
            if let Some(cached) = cached {
                let cache_matches = cached.fingerprint == fingerprint
                    && cached.provider == generator.provider()
                    && cached.model == generator.model();
                if cache_matches {
                    story.draft = Some(cached.draft);
                    report.cached += 1;
                    continue;
                }
            }

            match generator.generate(story).await {
                Ok(draft) => {
                    let validation = validate_draft_sources(&draft, story)
                        .and_then(|_| validate_draft_content(&draft));
                    if let Err(error) = validation {
                        let message = format!("{}: {error:#}", story.title);
                        match failure_policy {
                            AiFailurePolicy::Fail => {
                                return Err(error).with_context(|| {
                                    format!("AI draft validation failed for story '{}'", story.title)
                                });
                            }
                            AiFailurePolicy::Fallback => {
                                report.failed += 1;
                                report.failures.push(message);
                                continue;
                            }
                        }
                    }

                    let record = CachedDraft {
                        fingerprint: fingerprint.clone(),
                        provider: generator.provider().to_owned(),
                        model: generator.model().to_owned(),
                        generated_at: Utc::now().to_rfc3339(),
                        draft: draft.clone(),
                    };
                    write_cached_draft(&cache_path, &record)?;
                    story.draft = Some(draft);
                    report.generated += 1;
                }
                Err(error) => {
                    let message = format!("{}: {error:#}", story.title);
                    match failure_policy {
                        AiFailurePolicy::Fail => return Err(error).with_context(|| {
                            format!("AI drafting failed for story '{}'", story.title)
                        }),
                        AiFailurePolicy::Fallback => {
                            report.failed += 1;
                            report.failures.push(message);
                        }
                    }
                }
            }
        }
    }

    Ok(report)
}

pub fn story_fingerprint(story: &NewsletterStory) -> Result<String> {
    #[derive(Serialize)]
    struct FingerprintSource<'a> {
        kind: &'a str,
        title: &'a str,
        url: &'a str,
        published_on: &'a str,
        summary: &'a Option<String>,
        content: &'a Option<String>,
    }

    #[derive(Serialize)]
    struct FingerprintStory<'a> {
        schema_version: &'static str,
        title: &'a str,
        category: &'a str,
        project: &'a Option<String>,
        version: &'a Option<String>,
        published_on: &'a str,
        summary: &'a Option<String>,
        sources: Vec<FingerprintSource<'a>>,
    }

    let mut sources = story.sources.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.url.cmp(&right.url));
    let value = FingerprintStory {
        schema_version: DRAFT_SCHEMA_VERSION,
        title: &story.title,
        category: &story.category,
        project: &story.project,
        version: &story.version,
        published_on: &story.published_on,
        summary: &story.summary,
        sources: sources
            .into_iter()
            .map(|source| FingerprintSource {
                kind: &source.kind,
                title: &source.title,
                url: &source.url,
                published_on: &source.published_on,
                summary: &source.summary,
                content: &source.content,
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&value).context("failed to serialize story fingerprint input")?;
    Ok(format!("{:016x}", fnv1a64(&bytes)))
}

fn build_openai_request(
    story: &NewsletterStory,
    model: &str,
    max_source_chars: usize,
) -> Result<Value> {
    let packet = story_packet(story, max_source_chars)?;
    Ok(json!({
        "model": model,
        "store": false,
        "input": [
            {
                "role": "system",
                "content": "You draft a technical Rust web ecosystem newsletter. Use only the supplied story packet. Never invent release details, impact, migration requirements, affected users, security severity, or actions. Every factual claim in sourced_claims must cite one or more source_urls exactly as provided. If the material is insufficient to establish impact, say so and use low confidence. Write concise, neutral engineering prose."
            },
            {
                "role": "user",
                "content": packet
            }
        ],
        "text": {
            "format": editorial_draft_schema()
        }
    }))
}

fn story_packet(story: &NewsletterStory, max_source_chars: usize) -> Result<String> {
    #[derive(Serialize)]
    struct PacketSource<'a> {
        kind: &'a str,
        title: &'a str,
        url: &'a str,
        published_on: &'a str,
        summary: &'a Option<String>,
        content_excerpt: Option<String>,
    }

    #[derive(Serialize)]
    struct Packet<'a> {
        title: &'a str,
        category: &'a str,
        project: &'a Option<String>,
        version: &'a Option<String>,
        published_on: &'a str,
        editorial_context: &'a Option<String>,
        sources: Vec<PacketSource<'a>>,
    }

    let sources = story
        .sources
        .iter()
        .map(|source| PacketSource {
            kind: &source.kind,
            title: &source.title,
            url: &source.url,
            published_on: &source.published_on,
            summary: &source.summary,
            content_excerpt: source
                .content
                .as_deref()
                .map(|content| truncate(content, max_source_chars)),
        })
        .collect();

    serde_json::to_string_pretty(&Packet {
        title: &story.title,
        category: &story.category,
        project: &story.project,
        version: &story.version,
        published_on: &story.published_on,
        editorial_context: &story.summary,
        sources,
    })
    .context("failed to serialize AI story packet")
}

fn editorial_draft_schema() -> Value {
    json!({
        "type": "json_schema",
        "name": "rust_web_newsletter_draft",
        "strict": true,
        "schema": {
            "type": "object",
            "properties": {
                "headline": { "type": "string" },
                "what_changed": { "type": "string" },
                "why_it_matters": { "type": "string" },
                "who_is_affected": { "type": "string" },
                "action_required": {
                    "type": "string",
                    "enum": ["none", "consider_update", "migration_required", "security_update", "investigate"]
                },
                "action": { "type": "string" },
                "confidence": {
                    "type": "string",
                    "enum": ["high", "medium", "low"]
                },
                "sourced_claims": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "claim": { "type": "string" },
                            "source_urls": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["claim", "source_urls"],
                        "additionalProperties": false
                    }
                }
            },
            "required": [
                "headline",
                "what_changed",
                "why_it_matters",
                "who_is_affected",
                "action_required",
                "action",
                "confidence",
                "sourced_claims"
            ],
            "additionalProperties": false
        }
    })
}

fn validate_draft_sources(draft: &EditorialDraft, story: &NewsletterStory) -> Result<()> {
    let allowed = story
        .sources
        .iter()
        .map(|source| source.url.as_str())
        .collect::<HashSet<_>>();

    for claim in &draft.sourced_claims {
        if claim.claim.trim().is_empty() {
            bail!("AI draft contains an empty sourced claim");
        }
        if claim.source_urls.is_empty() {
            bail!("AI draft claim '{}' has no source URLs", claim.claim);
        }
        for url in &claim.source_urls {
            if !allowed.contains(url.as_str()) {
                bail!("AI draft cited unknown source URL '{url}'");
            }
        }
    }
    Ok(())
}

fn validate_draft_content(draft: &EditorialDraft) -> Result<()> {
    for (field, value) in [
        ("headline", draft.headline.as_str()),
        ("what_changed", draft.what_changed.as_str()),
        ("why_it_matters", draft.why_it_matters.as_str()),
        ("action", draft.action.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("AI draft field '{field}' cannot be empty");
        }
    }
    Ok(())
}

fn extract_output_text(response: &ResponsesApiResponse) -> Result<String> {
    if let Some(error) = &response.error {
        bail!("OpenAI response error: {}", error.message);
    }

    for item in &response.output {
        for content in &item.content {
            if content.kind == "refusal" {
                bail!(
                    "OpenAI refused the drafting request: {}",
                    content.refusal.as_deref().unwrap_or("no refusal text")
                );
            }
            if content.kind == "output_text" {
                if let Some(text) = content.text.as_deref() {
                    return Ok(text.to_owned());
                }
            }
        }
    }

    bail!("OpenAI response contained no output_text content")
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
}

fn backoff_delay(initial_ms: u64, attempt: u32) -> Duration {
    let multiplier = 1_u64.checked_shl(attempt.min(6)).unwrap_or(64);
    let base = initial_ms.saturating_mul(multiplier).min(60_000);
    let jitter_span = (base / 4).max(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(base.saturating_add(nanos % jitter_span))
}

fn load_cached_draft(path: &Path) -> Result<Option<CachedDraft>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read AI draft cache {}", path.display()))?;
    let draft = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse AI draft cache {}", path.display()))?;
    Ok(Some(draft))
}

fn write_cached_draft(path: &Path, draft: &CachedDraft) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create AI cache directory {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(draft).context("failed to serialize AI draft cache")?;
    let temp = temp_path(path);
    fs::write(&temp, format!("{raw}\n"))
        .with_context(|| format!("failed to write AI cache temp file {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| {
        format!(
            "failed to replace AI cache {} with {}",
            path.display(),
            temp.display()
        )
    })?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(".tmp");
    PathBuf::from(value)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[derive(Debug, Deserialize)]
struct ResponsesApiResponse {
    #[serde(default)]
    output: Vec<ResponseOutputItem>,
    error: Option<ResponseApiError>,
}

#[derive(Debug, Deserialize)]
struct ResponseOutputItem {
    #[serde(default)]
    content: Vec<ResponseOutputContent>,
}

#[derive(Debug, Deserialize)]
struct ResponseOutputContent {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    refusal: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseApiError {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{
        ResponsesApiResponse, build_openai_request, editorial_draft_schema, extract_output_text,
    };
    use crate::composer::{NewsletterSource, NewsletterStory};

    #[test]
    fn structured_schema_is_strict_at_root_and_claim_level() {
        let format = editorial_draft_schema();
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["strict"], true);
        assert_eq!(format["schema"]["additionalProperties"], false);
        assert_eq!(
            format["schema"]["properties"]["sourced_claims"]["items"]
                ["additionalProperties"],
            false
        );
    }

    #[test]
    fn extracts_output_text_from_responses_api_shape() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "output": [
                { "type": "reasoning", "summary": [] },
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "{\"headline\":\"test\"}",
                            "annotations": []
                        }
                    ]
                }
            ],
            "error": null
        }))
        .unwrap();

        assert_eq!(
            extract_output_text(&response).unwrap(),
            "{\"headline\":\"test\"}"
        );
    }

    #[test]
    fn refusal_is_not_treated_as_output_text() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "refusal",
                            "refusal": "cannot comply"
                        }
                    ]
                }
            ],
            "error": null
        }))
        .unwrap();

        assert!(
            extract_output_text(&response)
                .unwrap_err()
                .to_string()
                .contains("refused")
        );
    }
    #[test]
    fn responses_request_uses_text_format_structured_output() {
        let story = NewsletterStory {
            title: "Axum release".to_owned(),
            category: "frameworks".to_owned(),
            project: Some("Axum".to_owned()),
            version: Some("1.2.3".to_owned()),
            published_on: "July 10, 2026".to_owned(),
            summary: None,
            sources: vec![NewsletterSource {
                kind: "release".to_owned(),
                title: "Axum 1.2.3".to_owned(),
                url: "https://example.com/release".to_owned(),
                published_on: "2026-07-10".to_owned(),
                summary: Some("Release summary".to_owned()),
                content: Some("Longer release content".to_owned()),
            }],
            issue_url: None,
            draft: None,
        };

        let request = build_openai_request(&story, "test-model", 1000).unwrap();
        assert_eq!(request["model"], "test-model");
        assert_eq!(request["store"], false);
        assert_eq!(request["text"]["format"]["type"], "json_schema");
        assert_eq!(request["text"]["format"]["strict"], true);
        assert!(request.get("response_format").is_none());
    }

}
