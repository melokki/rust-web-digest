use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub feeds: Vec<FeedConfig>,
    #[serde(default)]
    pub collection: CollectionConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub crates_io: CratesIoConfig,
    #[serde(default)]
    pub reconciliation: ReconciliationConfig,
    #[serde(default)]
    pub publishing: PublishingConfig,
    #[serde(default)]
    pub newsletter: NewsletterConfig,
    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub category: String,
    pub github: Option<String>,
    #[serde(default)]
    pub crates: Vec<String>,
    #[serde(default)]
    pub collect: ProjectCollectConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCollectConfig {
    #[serde(default)]
    pub releases: bool,
    #[serde(default)]
    pub security: bool,
    #[serde(default)]
    pub crate_releases: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub category: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub required_any: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionConfig {
    #[serde(default = "default_github_api_url")]
    pub github_api_url: String,
    #[serde(default = "default_github_max_pages")]
    pub github_max_pages: u32,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            github_api_url: default_github_api_url(),
            github_max_pages: default_github_max_pages(),
            request_timeout_seconds: default_request_timeout_seconds(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    #[serde(default = "default_security_enabled")]
    pub enabled: bool,
    #[serde(default = "default_osv_api_url")]
    pub osv_api_url: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: default_security_enabled(),
            osv_api_url: default_osv_api_url(),
        }
    }
}


#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CratesIoConfig {
    #[serde(default = "default_crates_io_enabled")]
    pub enabled: bool,
    #[serde(default = "default_crates_io_api_url")]
    pub api_url: String,
    #[serde(default = "default_crates_io_web_url")]
    pub web_url: String,
    #[serde(default = "default_crates_io_user_agent_env")]
    pub user_agent_env: String,
    #[serde(default = "default_crates_io_request_delay_ms")]
    pub request_delay_ms: u64,
}

impl Default for CratesIoConfig {
    fn default() -> Self {
        Self {
            enabled: default_crates_io_enabled(),
            api_url: default_crates_io_api_url(),
            web_url: default_crates_io_web_url(),
            user_agent_env: default_crates_io_user_agent_env(),
            request_delay_ms: default_crates_io_request_delay_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationConfig {
    #[serde(default = "default_article_window_days")]
    pub article_window_days: u64,
    #[serde(default = "default_comment_on_story_update")]
    pub comment_on_story_update: bool,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            article_window_days: default_article_window_days(),
            comment_on_story_update: default_comment_on_story_update(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishingConfig {
    #[serde(default = "default_candidate_label")]
    pub candidate_label: String,
    #[serde(default = "default_new_status_label")]
    pub new_status_label: String,
    #[serde(default = "default_additional_status_labels")]
    pub additional_status_labels: Vec<String>,
    #[serde(default = "default_watch_status_label")]
    pub watch_status_label: String,
    #[serde(default = "default_selected_status_label")]
    pub selected_status_label: String,
    #[serde(default = "default_rejected_status_label")]
    pub rejected_status_label: String,
    #[serde(default = "default_published_status_label")]
    pub published_status_label: String,
    #[serde(default = "default_skipped_status_label")]
    pub skipped_status_label: String,
    #[serde(default = "default_late_discovery_label")]
    pub late_discovery_label: String,
    #[serde(default = "default_monthly_parent_label")]
    pub monthly_parent_label: String,
    #[serde(default = "default_monthly_parent_title_prefix")]
    pub monthly_parent_title_prefix: String,
    #[serde(default = "default_ensure_labels")]
    pub ensure_labels: bool,
    #[serde(default = "default_ensure_milestones")]
    pub ensure_milestones: bool,
    #[serde(default = "default_github_max_pages")]
    pub github_max_pages: u32,
}

impl Default for PublishingConfig {
    fn default() -> Self {
        Self {
            candidate_label: default_candidate_label(),
            new_status_label: default_new_status_label(),
            additional_status_labels: default_additional_status_labels(),
            watch_status_label: default_watch_status_label(),
            selected_status_label: default_selected_status_label(),
            rejected_status_label: default_rejected_status_label(),
            published_status_label: default_published_status_label(),
            skipped_status_label: default_skipped_status_label(),
            late_discovery_label: default_late_discovery_label(),
            monthly_parent_label: default_monthly_parent_label(),
            monthly_parent_title_prefix: default_monthly_parent_title_prefix(),
            ensure_labels: default_ensure_labels(),
            ensure_milestones: default_ensure_milestones(),
            github_max_pages: default_github_max_pages(),
        }
    }
}


#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewsletterConfig {
    #[serde(default = "default_newsletter_title_prefix")]
    pub title_prefix: String,
    #[serde(default = "default_newsletter_intro")]
    pub intro: String,
    #[serde(default = "default_newsletter_output_dir")]
    pub output_dir: String,
    #[serde(default = "default_newsletter_manifest_dir")]
    pub manifest_dir: String,
    #[serde(default = "default_newsletter_release_tag_prefix")]
    pub release_tag_prefix: String,
    #[serde(default = "default_newsletter_release_name_prefix")]
    pub release_name_prefix: String,
    #[serde(default = "default_newsletter_release_asset_name_prefix")]
    pub release_asset_name_prefix: String,
    #[serde(default = "default_newsletter_commit_message_prefix")]
    pub commit_message_prefix: String,
    #[serde(default = "default_newsletter_sync_release_tag")]
    pub sync_release_tag: bool,
    #[serde(default = "default_newsletter_category_order")]
    pub category_order: Vec<String>,
}

impl Default for NewsletterConfig {
    fn default() -> Self {
        Self {
            title_prefix: default_newsletter_title_prefix(),
            intro: default_newsletter_intro(),
            output_dir: default_newsletter_output_dir(),
            manifest_dir: default_newsletter_manifest_dir(),
            release_tag_prefix: default_newsletter_release_tag_prefix(),
            release_name_prefix: default_newsletter_release_name_prefix(),
            release_asset_name_prefix: default_newsletter_release_asset_name_prefix(),
            commit_message_prefix: default_newsletter_commit_message_prefix(),
            sync_release_tag: default_newsletter_sync_release_tag(),
            category_order: default_newsletter_category_order(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiConfig {
    #[serde(default = "default_ai_provider")]
    pub provider: String,
    #[serde(default = "default_ai_api_url")]
    pub api_url: String,
    #[serde(default = "default_ai_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_ai_model")]
    pub model: String,
    #[serde(default = "default_ai_cache_dir")]
    pub cache_dir: String,
    #[serde(default = "default_ai_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_ai_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_ai_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_ai_max_source_chars")]
    pub max_source_chars: usize,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_ai_provider(),
            api_url: default_ai_api_url(),
            api_key_env: default_ai_api_key_env(),
            model: default_ai_model(),
            cache_dir: default_ai_cache_dir(),
            request_timeout_seconds: default_ai_request_timeout_seconds(),
            max_retries: default_ai_max_retries(),
            initial_backoff_ms: default_ai_initial_backoff_ms(),
            max_source_chars: default_ai_max_source_chars(),
        }
    }
}

impl PublishingConfig {
    pub fn status_labels(&self) -> HashSet<String> {
        [
            self.new_status_label.clone(),
            self.watch_status_label.clone(),
            self.selected_status_label.clone(),
            self.rejected_status_label.clone(),
            self.published_status_label.clone(),
            self.skipped_status_label.clone(),
        ]
        .into_iter()
        .collect()
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.collection.github_max_pages == 0 {
            bail!("collection.github_max_pages must be greater than zero");
        }
        if self.collection.request_timeout_seconds == 0 {
            bail!("collection.request_timeout_seconds must be greater than zero");
        }
        validate_http_url("collection.github_api_url", &self.collection.github_api_url)?;
        validate_http_url("security.osv_api_url", &self.security.osv_api_url)?;
        validate_http_url("crates_io.api_url", &self.crates_io.api_url)?;
        validate_http_url("crates_io.web_url", &self.crates_io.web_url)?;
        validate_non_empty("crates_io.user_agent_env", &self.crates_io.user_agent_env)?;
        if self.crates_io.request_delay_ms < 1_000 {
            bail!("crates_io.request_delay_ms must be at least 1000");
        }
        if self.reconciliation.article_window_days == 0 {
            bail!("reconciliation.article_window_days must be greater than zero");
        }
        validate_non_empty("publishing.candidate_label", &self.publishing.candidate_label)?;
        validate_non_empty("publishing.new_status_label", &self.publishing.new_status_label)?;
        for label in &self.publishing.additional_status_labels {
            validate_non_empty("publishing.additional_status_labels entry", label)?;
        }
        validate_non_empty("publishing.watch_status_label", &self.publishing.watch_status_label)?;
        validate_non_empty("publishing.selected_status_label", &self.publishing.selected_status_label)?;
        validate_non_empty("publishing.rejected_status_label", &self.publishing.rejected_status_label)?;
        validate_non_empty("publishing.published_status_label", &self.publishing.published_status_label)?;
        validate_non_empty("publishing.skipped_status_label", &self.publishing.skipped_status_label)?;
        validate_non_empty("publishing.late_discovery_label", &self.publishing.late_discovery_label)?;
        validate_non_empty("publishing.monthly_parent_label", &self.publishing.monthly_parent_label)?;
        validate_non_empty(
            "publishing.monthly_parent_title_prefix",
            &self.publishing.monthly_parent_title_prefix,
        )?;
        let status_labels = self.publishing.status_labels();
        if status_labels.len() != 6 {
            bail!("publishing status labels must be unique");
        }
        if self.publishing.monthly_parent_label == self.publishing.candidate_label
            || status_labels.contains(&self.publishing.monthly_parent_label)
        {
            bail!("publishing.monthly_parent_label must be distinct from candidate and status labels");
        }
        if self.publishing.late_discovery_label == self.publishing.candidate_label
            || status_labels.contains(&self.publishing.late_discovery_label)
            || self.publishing.late_discovery_label == self.publishing.monthly_parent_label
        {
            bail!("publishing.late_discovery_label must be distinct from candidate, parent, and status labels");
        }
        if self.publishing.github_max_pages == 0 {
            bail!("publishing.github_max_pages must be greater than zero");
        }
        crate::composer::validate_newsletter_config(&self.newsletter)?;
        validate_ai_config(&self.ai)?;

        let mut ids = HashSet::new();
        let mut project_ids = HashSet::new();
        for project in &self.projects {
            validate_non_empty("project id", &project.id)?;
            validate_non_empty("project name", &project.name)?;
            validate_non_empty("project category", &project.category)?;

            if !project_ids.insert(project.id.as_str()) {
                bail!("duplicate project id: {}", project.id);
            }
            if !ids.insert(format!("project:{}", project.id)) {
                bail!("duplicate project id: {}", project.id);
            }
            if project.collect.releases && project.github.is_none() {
                bail!("project '{}' collects releases but has no github repository", project.id);
            }
            if project.collect.security && project.crates.is_empty() {
                bail!("project '{}' collects security advisories but has no crates", project.id);
            }
            if project.collect.crate_releases && project.crates.is_empty() {
                bail!("project '{}' collects crate releases but has no crates", project.id);
            }
            if let Some(repository) = &project.github {
                validate_github_repository(repository)?;
            }
            for crate_name in &project.crates {
                validate_non_empty("crate name", crate_name)?;
            }
        }

        for feed in &self.feeds {
            validate_non_empty("feed id", &feed.id)?;
            validate_non_empty("feed name", &feed.name)?;
            validate_non_empty("feed category", &feed.category)?;

            if !ids.insert(format!("feed:{}", feed.id)) {
                bail!("duplicate feed id: {}", feed.id);
            }
            validate_http_url(&format!("feed '{}' URL", feed.id), &feed.url)?;

            if let Some(project_id) = &feed.project_id {
                if !project_ids.contains(project_id.as_str()) {
                    bail!("feed '{}' references unknown project '{}'", feed.id, project_id);
                }
            }
        }

        Ok(())
    }
}

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} cannot be empty");
    }
    Ok(())
}

pub fn validate_ai_config(config: &AiConfig) -> Result<()> {
    if config.provider != "openai" {
        bail!("ai.provider currently supports only 'openai'");
    }
    validate_http_url("ai.api_url", &config.api_url)?;
    validate_non_empty("ai.api_key_env", &config.api_key_env)?;
    validate_non_empty("ai.model", &config.model)?;
    validate_non_empty("ai.cache_dir", &config.cache_dir)?;
    if config.request_timeout_seconds == 0 {
        bail!("ai.request_timeout_seconds must be greater than zero");
    }
    if config.initial_backoff_ms == 0 {
        bail!("ai.initial_backoff_ms must be greater than zero");
    }
    if config.max_source_chars == 0 || config.max_source_chars > 100_000 {
        bail!("ai.max_source_chars must be between 1 and 100000");
    }
    Ok(())
}

fn default_ai_provider() -> String { "openai".to_owned() }
fn default_ai_api_url() -> String { "https://api.openai.com/v1/responses".to_owned() }
fn default_ai_api_key_env() -> String { "OPENAI_API_KEY".to_owned() }
fn default_ai_model() -> String { "gpt-5.6".to_owned() }
fn default_ai_cache_dir() -> String { "data/ai-drafts".to_owned() }
fn default_ai_request_timeout_seconds() -> u64 { 120 }
fn default_ai_max_retries() -> u32 { 5 }
fn default_ai_initial_backoff_ms() -> u64 { 1_000 }
fn default_ai_max_source_chars() -> usize { 16_000 }

fn validate_http_url(label: &str, value: &str) -> Result<()> {
    let parsed = url::Url::parse(value).with_context(|| format!("{label} has an invalid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("{label} must use http or https");
    }
    Ok(())
}

fn validate_github_repository(repository: &str) -> Result<()> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        bail!("GitHub repository must be in owner/repo form: {repository}");
    }
    Ok(())
}

fn default_github_api_url() -> String {
    "https://api.github.com".to_owned()
}

fn default_github_max_pages() -> u32 {
    10
}

fn default_request_timeout_seconds() -> u64 {
    30
}

fn default_security_enabled() -> bool {
    true
}

fn default_osv_api_url() -> String {
    "https://api.osv.dev".to_owned()
}

fn default_crates_io_enabled() -> bool {
    true
}

fn default_crates_io_api_url() -> String {
    "https://crates.io/api/v1".to_owned()
}

fn default_crates_io_web_url() -> String {
    "https://crates.io/crates".to_owned()
}

fn default_crates_io_user_agent_env() -> String {
    "CRATES_IO_USER_AGENT".to_owned()
}

fn default_crates_io_request_delay_ms() -> u64 {
    1_100
}

fn default_article_window_days() -> u64 {
    14
}

fn default_comment_on_story_update() -> bool {
    true
}

fn default_candidate_label() -> String {
    "candidate".to_owned()
}

fn default_new_status_label() -> String {
    "status:new".to_owned()
}

fn default_additional_status_labels() -> Vec<String> {
    Vec::new()
}

fn default_watch_status_label() -> String {
    "status:watch".to_owned()
}

fn default_selected_status_label() -> String {
    "status:selected".to_owned()
}

fn default_rejected_status_label() -> String {
    "status:rejected".to_owned()
}

fn default_published_status_label() -> String {
    "status:published".to_owned()
}

fn default_skipped_status_label() -> String {
    "status:skipped".to_owned()
}

fn default_late_discovery_label() -> String {
    "late-discovery".to_owned()
}

fn default_monthly_parent_label() -> String {
    "editorial:month".to_owned()
}

fn default_monthly_parent_title_prefix() -> String {
    "Rust Web Monthly".to_owned()
}

fn default_ensure_labels() -> bool {
    true
}

fn default_ensure_milestones() -> bool {
    true
}


#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> AppConfig {
        AppConfig {
            projects: vec![ProjectConfig {
                id: "axum".to_owned(),
                name: "Axum".to_owned(),
                category: "frameworks".to_owned(),
                github: Some("tokio-rs/axum".to_owned()),
                crates: vec!["axum".to_owned()],
                collect: ProjectCollectConfig {
                    releases: true,
                    security: true,
                    crate_releases: true,
                },
            }],
            feeds: vec![],
            collection: CollectionConfig::default(),
            security: SecurityConfig::default(),
            crates_io: CratesIoConfig::default(),
            reconciliation: ReconciliationConfig::default(),
            publishing: PublishingConfig::default(),
            newsletter: NewsletterConfig::default(),
            ai: AiConfig::default(),
        }
    }

    #[test]
    fn accepts_minimal_valid_config() {
        minimal_config().validate().unwrap();
    }

    #[test]
    fn rejects_feed_with_unknown_project() {
        let mut config = minimal_config();
        config.feeds.push(FeedConfig {
            id: "blog".to_owned(),
            name: "Blog".to_owned(),
            url: "https://example.com/feed.xml".to_owned(),
            category: "articles".to_owned(),
            project_id: Some("missing".to_owned()),
            required_any: vec![],
        });

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("unknown project"));
    }

    #[test]
    fn rejects_security_collection_without_crates() {
        let mut config = minimal_config();
        config.projects[0].crates.clear();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("has no crates"));
    }
    #[test]
    fn rejects_duplicate_editorial_status_labels() {
        let mut config = minimal_config();
        config.publishing.selected_status_label = config.publishing.new_status_label.clone();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("status labels must be unique"));
    }

    #[test]
    fn rejects_parent_label_that_collides_with_candidate_label() {
        let mut config = minimal_config();
        config.publishing.monthly_parent_label = config.publishing.candidate_label.clone();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("monthly_parent_label must be distinct"));
    }

}

fn default_newsletter_title_prefix() -> String {
    "Rust Web Monthly".to_owned()
}

fn default_newsletter_intro() -> String {
    "A monthly briefing on Rust web development: frameworks, async runtimes, networking, databases, security, and full-stack tooling.".to_owned()
}

fn default_newsletter_output_dir() -> String {
    "content/issues".to_owned()
}

fn default_newsletter_manifest_dir() -> String {
    "data/newsletters".to_owned()
}

fn default_newsletter_release_tag_prefix() -> String {
    "digest".to_owned()
}

fn default_newsletter_release_name_prefix() -> String {
    "Rust Web Digest".to_owned()
}

fn default_newsletter_release_asset_name_prefix() -> String {
    "rust-web-digest".to_owned()
}

fn default_newsletter_commit_message_prefix() -> String {
    "publish digest".to_owned()
}

fn default_newsletter_sync_release_tag() -> bool {
    true
}

fn default_newsletter_category_order() -> Vec<String> {
    [
        "frameworks",
        "runtime",
        "networking",
        "databases",
        "security",
        "fullstack",
        "articles",
        "tooling",
        "uncategorized",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

