use std::{env, fs, path::PathBuf, time::Duration as StdDuration};

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::Client;
use rust_web_digest::{
    ai::{AiFailurePolicy, OpenAiDraftGenerator, enrich_document},
    collectors::{CollectionWindow, collect_all},
    composer::{CompositionMode, compose_automatic, compose_editorial, render_markdown, write_newsletter},
    config::AppConfig,
    editorial::{
        EditorialClient, EditorialMonth, EditorialStatus, EditorialStatusFilter,
    },
    github_issues::GitHubIssuePublisher,
    normalize::{deduplicate_exact, normalize_candidates},
    publication::{
        GitHubNewsletterPublisher, ReleaseState, load_publication_input,
    },
    reconcile::reconcile_candidates,
    storage::JsonlStore,
};

#[derive(Debug, Parser)]
#[command(name = "rust-web-digest")]
#[command(about = "Collect Rust web ecosystem news and manage a GitHub-native editorial workflow")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Collect {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long, default_value = "data/candidates.jsonl")]
        output: PathBuf,
        #[arg(long, default_value_t = 72)]
        since_hours: i64,
    },
    PublishIssues {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long, default_value = "data/candidates.jsonl")]
        input: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, default_value_t = 96)]
        since_hours: i64,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    Editorial {
        #[command(subcommand)]
        command: EditorialCommand,
    },
    Compose {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        month: String,
        #[arg(long, value_enum, default_value = "editorial")]
        mode: ComposeModeArg,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, default_value = "data/candidates.jsonl")]
        input: PathBuf,
        #[arg(long)]
        selected_input: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        ai: bool,
        #[arg(long, default_value_t = false)]
        refresh_ai: bool,
        #[arg(long, value_enum, default_value = "fail")]
        ai_failure_policy: AiFailurePolicyArg,
        #[arg(long, default_value_t = false)]
        stdout: bool,
    },
    Publish {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, value_enum, default_value = "published")]
        state: ReleaseStateArg,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum EditorialCommand {
    List {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        month: String,
        #[arg(long, value_enum, default_value = "all")]
        status: EditorialFilterArg,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    SetStatus {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        issue: u64,
        #[arg(long, value_enum)]
        status: EditorialStatusArg,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    SyncMonth {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        month: String,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    ExportSelected {
        #[arg(long, default_value = "config/sources.toml")]
        config: PathBuf,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        month: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ComposeModeArg {
    Editorial,
    Automatic,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AiFailurePolicyArg {
    Fail,
    Fallback,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReleaseStateArg {
    Draft,
    Published,
}

impl From<ReleaseStateArg> for ReleaseState {
    fn from(value: ReleaseStateArg) -> Self {
        match value {
            ReleaseStateArg::Draft => Self::Draft,
            ReleaseStateArg::Published => Self::Published,
        }
    }
}

impl From<AiFailurePolicyArg> for AiFailurePolicy {
    fn from(value: AiFailurePolicyArg) -> Self {
        match value {
            AiFailurePolicyArg::Fail => Self::Fail,
            AiFailurePolicyArg::Fallback => Self::Fallback,
        }
    }
}

impl From<ComposeModeArg> for CompositionMode {
    fn from(value: ComposeModeArg) -> Self {
        match value {
            ComposeModeArg::Editorial => Self::Editorial,
            ComposeModeArg::Automatic => Self::Automatic,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EditorialStatusArg {
    New,
    Watch,
    Selected,
    Rejected,
}

impl From<EditorialStatusArg> for EditorialStatus {
    fn from(value: EditorialStatusArg) -> Self {
        match value {
            EditorialStatusArg::New => Self::New,
            EditorialStatusArg::Watch => Self::Watch,
            EditorialStatusArg::Selected => Self::Selected,
            EditorialStatusArg::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EditorialFilterArg {
    All,
    New,
    Watch,
    Selected,
    Rejected,
}

impl From<EditorialFilterArg> for EditorialStatusFilter {
    fn from(value: EditorialFilterArg) -> Self {
        match value {
            EditorialFilterArg::All => Self::All,
            EditorialFilterArg::New => Self::Status(EditorialStatus::New),
            EditorialFilterArg::Watch => Self::Status(EditorialStatus::Watch),
            EditorialFilterArg::Selected => Self::Status(EditorialStatus::Selected),
            EditorialFilterArg::Rejected => Self::Status(EditorialStatus::Rejected),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Collect {
            config,
            output,
            since_hours,
        } => collect(config, output, since_hours).await,
        Command::PublishIssues {
            config,
            input,
            repository,
            since_hours,
            dry_run,
        } => publish_issues(config, input, repository, since_hours, dry_run).await,
        Command::Editorial { command } => run_editorial(command).await,
        Command::Compose {
            config,
            month,
            mode,
            repository,
            input,
            selected_input,
            output,
            manifest,
            ai,
            refresh_ai,
            ai_failure_policy,
            stdout,
        } => compose(
            config,
            month,
            mode.into(),
            repository,
            input,
            selected_input,
            output,
            manifest,
            ai,
            refresh_ai,
            ai_failure_policy.into(),
            stdout,
        )
        .await,
        Command::Publish {
            config,
            manifest,
            repository,
            state,
            dry_run,
        } => publish_newsletter(config, manifest, repository, state.into(), dry_run).await,
    }
}

async fn collect(config_path: PathBuf, output: PathBuf, since_hours: i64) -> Result<()> {
    validate_positive_hours(since_hours)?;

    let config = AppConfig::load(config_path)?;
    let now = Utc::now();
    let window = CollectionWindow {
        since: now.clone() - Duration::hours(since_hours),
        until: now.clone(),
    };

    println!("Collecting candidates since {}", window.since.to_rfc3339());
    let report = collect_all(&config, window, now).await?;

    for (source, count) in &report.counts {
        println!("  {source}: {count}");
    }

    let raw_count = report.candidates.len();
    let normalized = normalize_candidates(report.candidates);
    let unique = deduplicate_exact(normalized);
    let unique_count = unique.len();

    let store = JsonlStore::new(output);
    let merge = store.merge_and_save(unique)?;

    println!("Raw candidates: {raw_count}");
    println!("Unique candidates this run: {unique_count}");
    println!("New candidates persisted: {}", merge.added);
    println!("Total candidates persisted: {}", merge.total);

    if !report.warnings.is_empty() {
        eprintln!("Warnings:");
        for warning in report.warnings {
            eprintln!("  - {warning}");
        }
    }

    Ok(())
}

async fn publish_issues(
    config_path: PathBuf,
    input: PathBuf,
    repository: Option<String>,
    since_hours: i64,
    dry_run: bool,
) -> Result<()> {
    validate_positive_hours(since_hours)?;

    let config = AppConfig::load(config_path)?;
    let repository = resolve_repository(repository)?;
    let token = env::var("GITHUB_TOKEN").ok();
    let client = build_http_client(&config)?;

    let candidates = JsonlStore::new(input).load()?;
    let since = Utc::now() - Duration::hours(since_hours);
    let publisher = GitHubIssuePublisher::new(
        &client,
        &config.collection.github_api_url,
        token.as_deref(),
        &repository,
        &config.publishing,
    )?;

    println!(
        "Publishing candidates discovered since {} to {repository}",
        since.to_rfc3339()
    );
    let report = publisher
        .publish(&config, &candidates, since, dry_run)
        .await?;

    println!("Stories considered: {}", report.considered);
    println!("Issues created: {}", report.created);
    println!("Issues updated: {}", report.updated);
    println!("Stories unchanged: {}", report.unchanged);
    println!("Reconciliation conflicts: {}", report.conflicts);

    if dry_run {
        println!("Dry run complete; no GitHub Issues were changed.");
    }

    Ok(())
}

async fn run_editorial(command: EditorialCommand) -> Result<()> {
    match command {
        EditorialCommand::List {
            config,
            repository,
            month,
            status,
            json,
        } => {
            let config = AppConfig::load(config)?;
            let repository = resolve_repository(repository)?;
            let token = env::var("GITHUB_TOKEN").ok();
            let client = build_http_client(&config)?;
            let month = EditorialMonth::parse(&month)?;
            let editorial = EditorialClient::new(
                &client,
                &config.collection.github_api_url,
                token.as_deref(),
                &repository,
                &config.publishing,
            )?;
            let records = editorial.list(&month, status.into()).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                print_editorial_records(&records);
            }
            Ok(())
        }
        EditorialCommand::SetStatus {
            config,
            repository,
            issue,
            status,
            dry_run,
        } => {
            let config = AppConfig::load(config)?;
            let repository = resolve_repository(repository)?;
            let token = env::var("GITHUB_TOKEN").ok();
            let client = build_http_client(&config)?;
            let editorial = EditorialClient::new(
                &client,
                &config.collection.github_api_url,
                token.as_deref(),
                &repository,
                &config.publishing,
            )?;
            let report = editorial.set_status(issue, status.into(), dry_run).await?;
            println!(
                "Issue #{}: {:?} -> {:?}{}",
                report.issue_number,
                report.previous,
                report.current,
                if report.changed { "" } else { " (unchanged)" }
            );
            if dry_run {
                println!("Dry run complete; the issue was not changed.");
            }
            Ok(())
        }
        EditorialCommand::SyncMonth {
            config,
            repository,
            month,
            dry_run,
        } => {
            let config = AppConfig::load(config)?;
            let repository = resolve_repository(repository)?;
            let token = env::var("GITHUB_TOKEN").ok();
            let client = build_http_client(&config)?;
            let month = EditorialMonth::parse(&month)?;
            let editorial = EditorialClient::new(
                &client,
                &config.collection.github_api_url,
                token.as_deref(),
                &repository,
                &config.publishing,
            )?;
            let report = editorial.sync_month(&month, dry_run).await?;
            println!("Candidate issues: {}", report.candidate_count);
            println!("Parent created: {}", report.parent_created);
            println!("Parent updated: {}", report.parent_updated);
            println!("Sub-issues added: {}", report.sub_issues_added);
            println!("Parent conflicts: {}", report.parent_conflicts);
            if dry_run {
                println!("Dry run complete; no GitHub Issues were changed.");
            }
            Ok(())
        }
        EditorialCommand::ExportSelected {
            config,
            repository,
            month,
            output,
        } => {
            let config = AppConfig::load(config)?;
            let repository = resolve_repository(repository)?;
            let token = env::var("GITHUB_TOKEN").ok();
            let client = build_http_client(&config)?;
            let month = EditorialMonth::parse(&month)?;
            let output = output.unwrap_or_else(|| {
                PathBuf::from(format!("data/editorial/{}.selected.json", month.key))
            });
            let editorial = EditorialClient::new(
                &client,
                &config.collection.github_api_url,
                token.as_deref(),
                &repository,
                &config.publishing,
            )?;
            let records = editorial.export_selected(&month, &output).await?;
            println!("Exported {} selected stories to {}", records.len(), output.display());
            Ok(())
        }
    }
}

async fn compose(
    config_path: PathBuf,
    month: String,
    mode: CompositionMode,
    repository: Option<String>,
    input: PathBuf,
    selected_input: Option<PathBuf>,
    output: Option<PathBuf>,
    manifest: Option<PathBuf>,
    ai_enabled: bool,
    refresh_ai: bool,
    ai_failure_policy: AiFailurePolicy,
    stdout: bool,
) -> Result<()> {
    let config = AppConfig::load(config_path)?;
    let month = EditorialMonth::parse(&month)?;

    let mut document = match mode {
        CompositionMode::Editorial => {
            let records = if let Some(path) = selected_input {
                let raw = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                serde_json::from_str(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?
            } else {
                let repository = resolve_repository(repository)?;
                let token = env::var("GITHUB_TOKEN").ok();
                let client = build_http_client(&config)?;
                let editorial = EditorialClient::new(
                    &client,
                    &config.collection.github_api_url,
                    token.as_deref(),
                    &repository,
                    &config.publishing,
                )?;
                editorial
                    .list(
                        &month,
                        EditorialStatusFilter::Status(EditorialStatus::Selected),
                    )
                    .await?
            };
            compose_editorial(&month, &records, &config)?
        }
        CompositionMode::Automatic => {
            let candidates = JsonlStore::new(input).load()?;
            let stories = reconcile_candidates(&candidates, &config.reconciliation);
            compose_automatic(&month, &stories, &config)
        }
    };

    if ai_enabled {
        let api_key = env::var(&config.ai.api_key_env).with_context(|| {
            format!(
                "--ai requires API key environment variable {}",
                config.ai.api_key_env
            )
        })?;
        let ai_client = Client::builder()
            .user_agent("rust-web-digest/0.7")
            .timeout(StdDuration::from_secs(config.ai.request_timeout_seconds))
            .build()
            .context("failed to build AI HTTP client")?;
        let generator = OpenAiDraftGenerator::new(&ai_client, &config.ai, &api_key);
        let cache_dir = PathBuf::from(&config.ai.cache_dir);
        let report = enrich_document(
            &mut document,
            &generator,
            &cache_dir,
            refresh_ai,
            ai_failure_policy,
        )
        .await?;
        println!(
            "AI drafts: {} generated, {} cached, {} failed",
            report.generated, report.cached, report.failed
        );
        for failure in report.failures {
            eprintln!("AI drafting warning: {failure}");
        }
    }

    if document.story_count == 0 {
        bail!(
            "no stories available for {} in {} mode",
            month.key,
            mode.slug()
        );
    }

    if stdout {
        print!("{}", render_markdown(&document, &config.newsletter));
        return Ok(());
    }

    let written = write_newsletter(
        &document,
        &config.newsletter,
        output.as_deref(),
        manifest.as_deref(),
    )?;
    println!("Composed {} stories in {} mode", document.story_count, mode.slug());
    println!("Markdown: {}", written.markdown_path.display());
    println!("Manifest: {}", written.manifest_path.display());
    println!("Future release tag: {}", written.manifest.release_tag);
    Ok(())
}

async fn publish_newsletter(
    config_path: PathBuf,
    manifest_path: PathBuf,
    repository: Option<String>,
    state: ReleaseState,
    dry_run: bool,
) -> Result<()> {
    let config = AppConfig::load(config_path)?;
    let repository = resolve_repository(repository)?;
    let token = env::var("GITHUB_TOKEN")
        .context("GITHUB_TOKEN is required to publish newsletter content and Releases")?;
    let input = load_publication_input(&manifest_path)?;
    let client = build_http_client(&config)?;
    let publisher = GitHubNewsletterPublisher::new(
        &client,
        &config.collection.github_api_url,
        &token,
        &repository,
    )?;

    let report = publisher
        .publish(
            &input,
            state,
            &config.newsletter.commit_message_prefix,
            config.newsletter.sync_release_tag,
            dry_run,
        )
        .await?;

    println!("Repository content created: {}", report.content_created);
    println!("Repository content updated: {}", report.content_updated);
    println!("Repository content unchanged: {}", report.content_unchanged);
    println!("Release created: {}", report.release_created);
    println!("Release updated: {}", report.release_updated);
    println!("Release unchanged: {}", report.release_unchanged);
    println!("Release asset uploaded: {}", report.asset_uploaded);
    println!("Release asset replaced: {}", report.asset_replaced);
    println!("Release asset unchanged: {}", report.asset_unchanged);
    println!("Release tag moved: {}", report.tag_updated);
    if let Some(url) = report.release_url {
        println!("Release: {url}");
    }
    if dry_run {
        println!("Dry run complete; no repository content, release, tag, or asset was changed.");
    }
    Ok(())
}

fn resolve_repository(repository: Option<String>) -> Result<String> {
    repository
        .or_else(|| env::var("GITHUB_REPOSITORY").ok())
        .context("--repository or GITHUB_REPOSITORY is required")
}

fn build_http_client(config: &AppConfig) -> Result<Client> {
    Client::builder()
        .user_agent("rust-web-digest/0.7")
        .timeout(StdDuration::from_secs(
            config.collection.request_timeout_seconds,
        ))
        .build()
        .context("failed to build HTTP client")
}

fn print_editorial_records(records: &[rust_web_digest::editorial::EditorialStoryRecord]) {
    if records.is_empty() {
        println!("No matching editorial stories found.");
        return;
    }

    for record in records {
        let status = record
            .status
            .map(|value| format!("{value:?}").to_lowercase())
            .unwrap_or_else(|| "unset".to_owned());
        let category = record.category.as_deref().unwrap_or("uncategorized");
        println!(
            "#{:<5} {:<10} {:<16} {}",
            record.issue_number, status, category, record.title
        );
    }
}

fn validate_positive_hours(since_hours: i64) -> Result<()> {
    if since_hours <= 0 {
        bail!("--since-hours must be greater than zero");
    }
    Ok(())
}
