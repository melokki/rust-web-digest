use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{
    ai::EditorialDraft,
    config::{AppConfig, NewsletterConfig},
    domain::{Candidate, CandidateKind, Story},
    editorial::{EditorialMonth, EditorialStatus, EditorialStoryRecord},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionMode {
    Editorial,
    Automatic,
}

impl CompositionMode {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Editorial => "editorial",
            Self::Automatic => "automatic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsletterSource {
    pub kind: String,
    pub title: String,
    pub url: String,
    pub published_on: String,
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsletterStory {
    pub title: String,
    pub category: String,
    pub project: Option<String>,
    pub version: Option<String>,
    pub published_on: String,
    pub summary: Option<String>,
    pub sources: Vec<NewsletterSource>,
    pub issue_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<EditorialDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsletterSection {
    pub category: String,
    pub title: String,
    pub stories: Vec<NewsletterStory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsletterDocument {
    pub month: String,
    pub title: String,
    pub mode: CompositionMode,
    pub story_count: usize,
    pub sections: Vec<NewsletterSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsletterManifest {
    pub month: String,
    pub title: String,
    pub mode: CompositionMode,
    pub story_count: usize,
    pub ai_drafted_story_count: usize,
    pub markdown_path: String,
    pub release_tag: String,
    pub release_name: String,
    pub release_asset_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenNewsletter {
    pub markdown_path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: NewsletterManifest,
}

pub fn compose_automatic(
    month: &EditorialMonth,
    stories: &[Story],
    config: &AppConfig,
) -> NewsletterDocument {
    let project_names = config
        .projects
        .iter()
        .map(|project| (project.id.as_str(), project.name.as_str()))
        .collect::<HashMap<_, _>>();

    let items = stories
        .iter()
        .filter(|story| story_in_month(story, month))
        .map(|story| story_from_reconciled(story, &project_names))
        .collect::<Vec<_>>();

    build_document(month, CompositionMode::Automatic, items, &config.newsletter)
}

pub fn compose_editorial(
    month: &EditorialMonth,
    records: &[EditorialStoryRecord],
    config: &AppConfig,
) -> Result<NewsletterDocument> {
    let mut items = Vec::with_capacity(records.len());
    for record in records
        .iter()
        .filter(|record| record.status == Some(EditorialStatus::Selected))
    {
        items.push(story_from_editorial(record)?);
    }
    Ok(build_document(
        month,
        CompositionMode::Editorial,
        items,
        &config.newsletter,
    ))
}

pub fn render_markdown(document: &NewsletterDocument, config: &NewsletterConfig) -> String {
    let mut output = String::new();
    output.push_str(&format!("# {}\n\n", document.title));
    output.push_str(&format!(
        "<!-- rust-web-digest:month:{} -->\n<!-- rust-web-digest:composition-mode:{} -->\n\n",
        document.month,
        document.mode.slug()
    ));

    if !config.intro.trim().is_empty() {
        output.push_str(config.intro.trim());
        output.push_str("\n\n");
    }

    let section_count = document.sections.len();
    output.push_str(&format!(
        "_{} stories across {} sections._\n\n",
        document.story_count, section_count
    ));

    for section in &document.sections {
        output.push_str(&format!("## {}\n\n", section.title));
        for story in &section.stories {
            let heading = story
                .draft
                .as_ref()
                .map(|draft| draft.headline.as_str())
                .unwrap_or(&story.title);
            output.push_str(&format!("### {}\n\n", heading));
            output.push_str(&format!("_Published {}", story.published_on));
            if story.sources.len() > 1 {
                output.push_str(&format!(" · {} sources", story.sources.len()));
            }
            output.push_str("_\n\n");

            if let Some(draft) = &story.draft {
                output.push_str(&format!("**What changed:** {}\n\n", draft.what_changed.trim()));
                output.push_str(&format!("**Why it matters:** {}\n\n", draft.why_it_matters.trim()));
                if !draft.who_is_affected.trim().is_empty() {
                    output.push_str(&format!(
                        "**Who should care:** {}\n\n",
                        draft.who_is_affected.trim()
                    ));
                }
                output.push_str(&format!(
                    "**Action:** {} — {}\n\n",
                    draft.action_required.label(),
                    draft.action.trim()
                ));
                output.push_str(&format!(
                    "_Draft confidence: {}_\n\n",
                    draft.confidence.label()
                ));
            } else if let Some(summary) = story.summary.as_deref() {
                output.push_str(summary.trim());
                output.push_str("\n\n");
            }

            if !story.sources.is_empty() {
                output.push_str("**Sources:** ");
                output.push_str(
                    &story
                        .sources
                        .iter()
                        .map(|source| format!("[{}]({})", source_link_label(source), source.url))
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
                output.push_str("\n\n");
            }
        }
    }

    output
}

pub fn write_newsletter(
    document: &NewsletterDocument,
    config: &NewsletterConfig,
    output: Option<&Path>,
    manifest_output: Option<&Path>,
) -> Result<WrittenNewsletter> {
    let markdown_path = output
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| PathBuf::from(&config.output_dir).join(format!("{}.md", document.month)));
    let manifest_path = manifest_output.map(ToOwned::to_owned).unwrap_or_else(|| {
        PathBuf::from(&config.manifest_dir).join(format!("{}.manifest.json", document.month))
    });

    ensure_parent(&markdown_path)?;
    ensure_parent(&manifest_path)?;

    let markdown = render_markdown(document, config);
    fs::write(&markdown_path, markdown)
        .with_context(|| format!("failed to write {}", markdown_path.display()))?;

    let manifest = NewsletterManifest {
        month: document.month.clone(),
        title: document.title.clone(),
        mode: document.mode,
        story_count: document.story_count,
        ai_drafted_story_count: document
            .sections
            .iter()
            .flat_map(|section| &section.stories)
            .filter(|story| story.draft.is_some())
            .count(),
        markdown_path: markdown_path.to_string_lossy().into_owned(),
        release_tag: format!("{}-{}", config.release_tag_prefix, document.month),
        release_name: format!("{} — {}", config.release_name_prefix, month_title(&document.month)?),
        release_asset_name: crate::publication::release_asset_name(
            &config.release_asset_name_prefix,
            &document.month,
        ),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .context("failed to serialize newsletter manifest")?;
    fs::write(&manifest_path, format!("{manifest_json}\n"))
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    Ok(WrittenNewsletter {
        markdown_path,
        manifest_path,
        manifest,
    })
}

fn build_document(
    month: &EditorialMonth,
    mode: CompositionMode,
    stories: Vec<NewsletterStory>,
    config: &NewsletterConfig,
) -> NewsletterDocument {
    let story_count = stories.len();
    let mut grouped = BTreeMap::<String, Vec<NewsletterStory>>::new();
    for story in stories {
        grouped.entry(story.category.clone()).or_default().push(story);
    }

    for stories in grouped.values_mut() {
        stories.sort_by(|left, right| {
            left.published_on
                .cmp(&right.published_on)
                .then_with(|| left.title.cmp(&right.title))
        });
    }

    let mut sections = Vec::new();
    for category in &config.category_order {
        if let Some(stories) = grouped.remove(category) {
            sections.push(NewsletterSection {
                category: category.clone(),
                title: category_title(category),
                stories,
            });
        }
    }

    for (category, stories) in grouped {
        sections.push(NewsletterSection {
            title: category_title(&category),
            category,
            stories,
        });
    }

    NewsletterDocument {
        month: month.key.clone(),
        title: format!("{} — {}", config.title_prefix, month.title),
        mode,
        story_count,
        sections,
    }
}

fn story_from_reconciled(
    story: &Story,
    project_names: &HashMap<&str, &str>,
) -> NewsletterStory {
    let sources = story
        .candidates
        .iter()
        .map(source_from_candidate)
        .collect::<Vec<_>>();
    let summary = story
        .candidates
        .iter()
        .find_map(|candidate| meaningful_summary(candidate.summary.as_deref()));
    let project = story
        .project_id
        .as_deref()
        .map(|id| project_names.get(id).copied().unwrap_or(id).to_owned());

    let title = if story
        .candidates
        .iter()
        .any(|candidate| candidate.kind == CandidateKind::GitHubRelease)
    {
        story.title.clone()
    } else if let (Some(project), Some(version)) = (project.as_deref(), story.version.as_deref()) {
        format!("{project} {version} published")
    } else {
        story.title.clone()
    };

    NewsletterStory {
        title,
        category: story.category.clone(),
        project,
        version: story.version.clone(),
        published_on: story.published_at.format("%B %-d, %Y").to_string(),
        summary,
        sources,
        issue_url: None,
        draft: None,
    }
}

fn story_from_editorial(record: &EditorialStoryRecord) -> Result<NewsletterStory> {
    let sources = extract_sources_from_issue_body(&record.body)?;
    let notes = record
        .editorial_notes
        .as_deref()
        .and_then(meaningful_editorial_notes);
    let fallback = sources
        .iter()
        .find_map(|source| meaningful_summary(source.summary.as_deref()));
    let summary = notes.or(fallback);
    let published_on = extract_first_published(&record.body)
        .unwrap_or_else(|| record.milestone.clone());
    let version = extract_story_field(&record.body, "Version");
    let project = extract_story_field(&record.body, "Project");

    Ok(NewsletterStory {
        title: strip_project_prefix(&record.title),
        category: record
            .category
            .clone()
            .unwrap_or_else(|| "uncategorized".to_owned()),
        project,
        version,
        published_on,
        summary,
        sources,
        issue_url: Some(record.issue_url.clone()),
        draft: None,
    })
}

pub fn extract_sources_from_issue_body(body: &str) -> Result<Vec<NewsletterSource>> {
    let source_section = body
        .split_once("## Sources")
        .map(|(_, tail)| tail)
        .unwrap_or_default();
    let source_section = source_section
        .split("<!-- rust-web-digest:story-id:")
        .next()
        .unwrap_or(source_section);

    let mut sources = Vec::new();
    for block in source_section.split("\n### ").skip(1) {
        let mut lines = block.lines();
        let heading = lines.next().unwrap_or_default().trim();
        let (kind, published_on) = heading
            .split_once(" · ")
            .map(|(kind, date)| (kind.trim().to_owned(), date.trim().to_owned()))
            .unwrap_or_else(|| (heading.to_owned(), String::new()));

        let remaining = lines.collect::<Vec<_>>();
        let link_index = remaining
            .iter()
            .position(|line| parse_markdown_link(line.trim()).is_some());
        let Some(link_index) = link_index else {
            continue;
        };
        let Some((title, url)) = parse_markdown_link(remaining[link_index].trim()) else {
            continue;
        };

        let mut summary_lines = Vec::new();
        for line in remaining.iter().skip(link_index + 1) {
            let trimmed = line.trim();
            if trimmed.starts_with("- **") || trimmed == "_No additional metadata._" {
                break;
            }
            if !trimmed.is_empty() {
                summary_lines.push(trimmed);
            }
        }
        let summary = if summary_lines.is_empty() {
            None
        } else {
            meaningful_summary(Some(&summary_lines.join(" ")))
        };

        sources.push(NewsletterSource {
            kind,
            title,
            url,
            published_on,
            summary,
            content: None,
        });
    }

    Ok(sources)
}

fn source_from_candidate(candidate: &Candidate) -> NewsletterSource {
    NewsletterSource {
        kind: kind_slug(candidate.kind).to_owned(),
        title: candidate.title.clone(),
        url: candidate.url.clone(),
        published_on: candidate.published_at.format("%Y-%m-%d").to_string(),
        summary: meaningful_summary(candidate.summary.as_deref()),
        content: candidate
            .raw_content
            .as_deref()
            .and_then(|value| meaningful_summary(Some(value))),
    }
}

fn story_in_month(story: &Story, month: &EditorialMonth) -> bool {
    let Ok(date) = NaiveDate::parse_from_str(&format!("{}-01", month.key), "%Y-%m-%d") else {
        return false;
    };
    story.published_at.year() == date.year() && story.published_at.month() == date.month()
}

fn category_title(category: &str) -> String {
    match category {
        "frameworks" => "Frameworks".to_owned(),
        "runtime" => "Async Runtime".to_owned(),
        "networking" => "Networking & HTTP".to_owned(),
        "databases" => "Databases".to_owned(),
        "security" => "Security".to_owned(),
        "fullstack" => "Full-stack & Browser".to_owned(),
        "articles" => "Articles Worth Reading".to_owned(),
        "tooling" => "Tooling".to_owned(),
        "uncategorized" => "Other".to_owned(),
        other => title_case_slug(other),
    }
}

fn title_case_slug(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase().collect::<String>(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn source_link_label(source: &NewsletterSource) -> String {
    match source.kind.as_str() {
        "release" => "release notes".to_owned(),
        "crate" => "crates.io".to_owned(),
        "article" => source.title.clone(),
        "security" => "security advisory".to_owned(),
        _ => source.title.clone(),
    }
}

fn meaningful_summary(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value == "No source summary was provided." {
        return None;
    }
    Some(value.to_owned())
}

fn meaningful_editorial_notes(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with("_Add review notes here.")
        || value == "placeholder"
    {
        return None;
    }
    Some(value.to_owned())
}

fn parse_markdown_link(line: &str) -> Option<(String, String)> {
    let body = line.strip_prefix('[')?;
    let split = body.find("](")?;
    let title = &body[..split];
    let url = body[split + 2..].strip_suffix(')')?;
    Some((title.to_owned(), url.to_owned()))
}

fn extract_story_field(body: &str, field: &str) -> Option<String> {
    let prefix = format!("- **{field}:** ");
    body.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "Unassigned")
        .map(ToOwned::to_owned)
}

fn extract_first_published(body: &str) -> Option<String> {
    let value = extract_story_field(body, "First published")?;
    chrono::DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|date| date.format("%B %-d, %Y").to_string())
        .or(Some(value))
}

fn strip_project_prefix(title: &str) -> String {
    if let Some(rest) = title.strip_prefix('[') {
        if let Some((_, title)) = rest.split_once("] ") {
            return title.to_owned();
        }
    }
    title.to_owned()
}

fn kind_slug(kind: CandidateKind) -> &'static str {
    match kind {
        CandidateKind::GitHubRelease => "release",
        CandidateKind::CrateRelease => "crate",
        CandidateKind::FeedArticle => "article",
        CandidateKind::SecurityAdvisory => "security",
    }
}

fn month_title(month: &str) -> Result<String> {
    let date = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .with_context(|| format!("invalid newsletter month '{month}'"))?;
    Ok(date.format("%B %Y").to_string())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

pub fn validate_newsletter_config(config: &NewsletterConfig) -> Result<()> {
    if config.title_prefix.trim().is_empty() {
        bail!("newsletter.title_prefix cannot be empty");
    }
    if config.output_dir.trim().is_empty() {
        bail!("newsletter.output_dir cannot be empty");
    }
    if config.manifest_dir.trim().is_empty() {
        bail!("newsletter.manifest_dir cannot be empty");
    }
    if config.release_tag_prefix.trim().is_empty() {
        bail!("newsletter.release_tag_prefix cannot be empty");
    }
    if config.release_name_prefix.trim().is_empty() {
        bail!("newsletter.release_name_prefix cannot be empty");
    }
    if config.release_asset_name_prefix.trim().is_empty() {
        bail!("newsletter.release_asset_name_prefix cannot be empty");
    }
    if config.commit_message_prefix.trim().is_empty() {
        bail!("newsletter.commit_message_prefix cannot be empty");
    }
    if !config
        .release_tag_prefix
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!(
            "newsletter.release_tag_prefix may contain only ASCII letters, digits, '-', '_', and '.'"
        );
    }

    if !config.release_tag_prefix.chars().any(|ch| ch.is_ascii_alphanumeric()) {
        bail!("newsletter.release_tag_prefix must contain at least one ASCII letter or digit");
    }

    if !config
        .release_asset_name_prefix
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!(
            "newsletter.release_asset_name_prefix may contain only ASCII letters, digits, '-', '_', and '.'"
        );
    }

    if !config
        .release_asset_name_prefix
        .chars()
        .any(|ch| ch.is_ascii_alphanumeric())
    {
        bail!("newsletter.release_asset_name_prefix must contain at least one ASCII letter or digit");
    }

    let mut seen = std::collections::HashSet::new();
    for category in &config.category_order {
        if category.trim().is_empty() {
            bail!("newsletter.category_order cannot contain empty values");
        }
        if !seen.insert(category) {
            bail!("newsletter.category_order contains duplicate category '{category}'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_sources_from_issue_body, parse_markdown_link, strip_project_prefix};

    #[test]
    fn parses_generated_source_blocks() {
        let body = "## Sources\n\n### release · 2026-07-10\n\n[Axum 1.0](https://example.com/release)\n\nStable release summary.\n\n- **tag_name:** v1.0.0\n\n### crate · 2026-07-10\n\n[axum 1.0](https://crates.io/crates/axum/1.0.0)\n\nNo source summary was provided.\n\n_No additional metadata._\n\n<!-- rust-web-digest:story-id:release:axum:1.0.0 -->";
        let sources = extract_sources_from_issue_body(body).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].kind, "release");
        assert_eq!(sources[0].summary.as_deref(), Some("Stable release summary."));
        assert_eq!(sources[1].summary, None);
    }

    #[test]
    fn parses_markdown_link() {
        assert_eq!(
            parse_markdown_link("[Title](https://example.com)"),
            Some(("Title".to_owned(), "https://example.com".to_owned()))
        );
    }

    #[test]
    fn removes_project_prefix_from_issue_title() {
        assert_eq!(strip_project_prefix("[Axum] Axum 1.0 released"), "Axum 1.0 released");
        assert_eq!(strip_project_prefix("No prefix"), "No prefix");
    }
}
