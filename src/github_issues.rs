use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, sleep};

use crate::{
    ai::EditorialDraft,
    config::{AppConfig, PublishingConfig},
    domain::{Candidate, CandidateKind, Story},
    reconcile::reconcile_candidates,
};

const GITHUB_API_VERSION: &str = "2026-03-10";
const CANDIDATE_MARKER_PREFIX: &str = "<!-- rust-web-digest:candidate-id:";
const CANDIDATE_MARKER_SUFFIX: &str = " -->";
const STORY_MARKER_PREFIX: &str = "<!-- rust-web-digest:story-id:";
const STORY_MARKER_SUFFIX: &str = " -->";
const MANAGED_START: &str = "<!-- rust-web-digest:managed:start -->";
const MANAGED_END: &str = "<!-- rust-web-digest:managed:end -->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDraft {
    pub story_id: String,
    pub candidate_ids: Vec<String>,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub milestone_title: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishReport {
    pub considered: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub conflicts: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubIssue {
    number: u64,
    title: String,
    body: Option<String>,
    #[serde(default)]
    labels: Vec<GitHubIssueLabel>,
    milestone: Option<GitHubIssueMilestone>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubIssueLabel {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubIssueMilestone {
    number: u64,
}

#[derive(Debug, Deserialize)]
struct GitHubLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitHubMilestone {
    number: u64,
    title: String,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatedIssue {
    number: u64,
}

#[derive(Debug, Serialize)]
struct CreateLabelRequest<'a> {
    name: &'a str,
    color: &'a str,
    description: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateMilestoneRequest<'a> {
    title: &'a str,
    description: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateIssueRequest<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<u64>,
}

#[derive(Debug, Serialize)]
struct UpdateIssueRequest<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<u64>,
}

#[derive(Debug, Serialize)]
struct CreateCommentRequest<'a> {
    body: &'a str,
}

pub struct GitHubIssuePublisher<'a> {
    client: &'a Client,
    api_url: &'a str,
    token: Option<&'a str>,
    owner: String,
    repo: String,
    publishing: &'a PublishingConfig,
}

impl<'a> GitHubIssuePublisher<'a> {
    pub fn new(
        client: &'a Client,
        api_url: &'a str,
        token: Option<&'a str>,
        repository: &str,
        publishing: &'a PublishingConfig,
    ) -> Result<Self> {
        let (owner, repo) = parse_repository(repository)?;
        Ok(Self {
            client,
            api_url,
            token,
            owner,
            repo,
            publishing,
        })
    }

    pub async fn publish(
        &self,
        config: &AppConfig,
        candidates: &[Candidate],
        since: DateTime<Utc>,
        ai_drafts: Option<&HashMap<String, EditorialDraft>>,
        dry_run: bool,
    ) -> Result<PublishReport> {
        if !dry_run && self.token.is_none() {
            bail!("GITHUB_TOKEN is required to create or update GitHub Issues");
        }

        let project_names = config
            .projects
            .iter()
            .map(|project| (project.id.as_str(), project.name.as_str()))
            .collect::<HashMap<_, _>>();

        let stories = reconcile_candidates(candidates, &config.reconciliation)
            .into_iter()
            .filter(|story| story.has_candidate_discovered_since(&since))
            .collect::<Vec<_>>();
        let drafts = stories
            .iter()
            .map(|story| {
                let project_name = story
                    .project_id
                    .as_deref()
                    .and_then(|id| project_names.get(id).copied());
                build_story_issue_draft_with_ai(
                    story,
                    project_name,
                    self.publishing,
                    ai_drafts.and_then(|drafts| drafts.get(&story.id)),
                )
            })
            .collect::<Vec<_>>();

        let mut report = PublishReport {
            considered: drafts.len(),
            ..PublishReport::default()
        };
        if drafts.is_empty() {
            return Ok(report);
        }

        let mut issues = self.list_existing_candidate_issues().await?;

        if dry_run {
            for (story, draft) in stories.iter().zip(&drafts) {
                let matches = matching_issue_indices(story, &issues);
                match matches.as_slice() {
                    [] => println!("Would create: {}", draft.title),
                    [index] => {
                        let issue = &issues[*index];
                        let new_sources = new_source_candidates(story, issue);
                        let body = merge_managed_body(
                            issue.body.as_deref().unwrap_or_default(),
                            &draft.body,
                        );
                        let labels = merge_labels(issue, &draft.labels, self.publishing);
                        let changed = issue.title != draft.title
                            || issue.body.as_deref().unwrap_or_default() != body
                            || issue_label_names(issue) != labels;
                        if changed {
                            println!(
                                "Would update #{} with {} new source(s): {}",
                                issue.number,
                                new_sources.len(),
                                draft.title
                            );
                            report.updated += 1;
                        } else {
                            report.unchanged += 1;
                        }
                    }
                    _ => {
                        eprintln!(
                            "Conflict: story '{}' matches multiple existing candidate issues",
                            story.id
                        );
                        report.conflicts += 1;
                    }
                }
            }
            return Ok(report);
        }

        let mut existing_labels = if self.publishing.ensure_labels {
            self.list_labels().await?
        } else {
            HashSet::new()
        };
        let mut milestones = if self.publishing.ensure_milestones {
            self.list_milestones().await?
        } else {
            HashMap::new()
        };

        if self.publishing.ensure_labels {
            let mut required_labels = drafts
                .iter()
                .flat_map(|draft| draft.labels.iter().cloned())
                .collect::<HashSet<_>>();
            required_labels.extend(self.publishing.status_labels());
            required_labels.extend(self.publishing.additional_status_labels.iter().cloned());
            required_labels.insert(self.publishing.late_discovery_label.clone());

            let mut required_labels = required_labels.into_iter().collect::<Vec<_>>();
            required_labels.sort();
            for label in required_labels {
                if !existing_labels.contains(&label) {
                    let spec = label_spec(&label);
                    self.create_label(&label, spec.color, spec.description).await?;
                    existing_labels.insert(label);
                }
            }
        }

        for (story, draft) in stories.iter().zip(&drafts) {
            let matches = matching_issue_indices(story, &issues);
            if matches.len() > 1 {
                eprintln!(
                    "Conflict: story '{}' matches Issues {:?}; skipping automatic reconciliation",
                    story.id,
                    matches
                        .iter()
                        .map(|index| issues[*index].number)
                        .collect::<Vec<_>>()
                );
                report.conflicts += 1;
                continue;
            }

            let mut effective_draft = draft.clone();
            let milestone = if self.publishing.ensure_milestones {
                let target_title = match milestones.get(&effective_draft.milestone_title) {
                    Some(existing) if existing.state.as_deref() == Some("closed") => {
                        effective_draft.labels.push(self.publishing.late_discovery_label.clone());
                        effective_draft.labels.sort();
                        effective_draft.labels.dedup();
                        story.discovered_at.format("%B %Y").to_string()
                    }
                    _ => effective_draft.milestone_title.clone(),
                };

                match milestones.get(&target_title).map(|item| item.number) {
                    Some(number) => Some(number),
                    None => {
                        let created = self.create_milestone(&target_title).await?;
                        let number = created.number;
                        milestones.insert(target_title.clone(), created);
                        Some(number)
                    }
                }
            } else {
                None
            };

            match matches.as_slice() {
                [] => {
                    let number = self.create_issue(&effective_draft, milestone).await?;
                    issues.push(existing_issue_from_draft(number, &effective_draft, milestone));
                    report.created += 1;
                }
                [index] => {
                    let issue = &issues[*index];
                    let issue_number = issue.number;
                    let new_sources = new_source_candidates(story, issue);
                    let body =
                        merge_managed_body(issue.body.as_deref().unwrap_or_default(), &effective_draft.body);
                    let labels = merge_labels(issue, &effective_draft.labels, self.publishing);
                    let existing_milestone = issue.milestone.as_ref().map(|item| item.number);
                    let milestone_changed = self.publishing.ensure_milestones
                        && existing_milestone != milestone;
                    let stored_milestone = if self.publishing.ensure_milestones {
                        milestone
                    } else {
                        existing_milestone
                    };
                    let changed = issue.title != effective_draft.title
                        || issue.body.as_deref().unwrap_or_default() != body
                        || issue_label_names(issue) != labels
                        || milestone_changed;

                    if changed {
                        self.update_issue(issue_number, &effective_draft, &body, &labels, stored_milestone)
                            .await?;
                        if config.reconciliation.comment_on_story_update && !new_sources.is_empty() {
                            let comment = render_source_update_comment(&new_sources);
                            self.create_comment(issue_number, &comment).await?;
                        }

                        issues[*index] = existing_issue_from_updated(
                            issue_number,
                            &effective_draft,
                            body,
                            labels,
                            stored_milestone,
                        );
                        report.updated += 1;
                    } else {
                        report.unchanged += 1;
                    }
                }
                _ => unreachable!("multiple matches are handled above"),
            }
        }

        Ok(report)
    }

    async fn list_existing_candidate_issues(&self) -> Result<Vec<GitHubIssue>> {
        let mut issues = Vec::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let url = self.repo_url("issues");
            let response = self
                .request(self.client.get(url))
                .query(&[
                    ("state", "all"),
                    ("labels", self.publishing.candidate_label.as_str()),
                    ("per_page", "100"),
                    ("page", page_string.as_str()),
                ])
                .send()
                .await
                .context("failed to list candidate issues")?
                .error_for_status()
                .context("GitHub returned an error while listing candidate issues")?
                .json::<Vec<GitHubIssue>>()
                .await
                .context("invalid GitHub Issues response")?;

            let page_len = response.len();
            issues.extend(
                response
                    .into_iter()
                    .filter(|issue| issue.pull_request.is_none()),
            );
            if page_len < 100 {
                break;
            }
        }
        Ok(issues)
    }

    async fn list_labels(&self) -> Result<HashSet<String>> {
        let mut labels = HashSet::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let url = self.repo_url("labels");
            let response = self
                .request(self.client.get(url))
                .query(&[("per_page", "100"), ("page", page_string.as_str())])
                .send()
                .await
                .context("failed to list repository labels")?
                .error_for_status()
                .context("GitHub returned an error while listing labels")?
                .json::<Vec<GitHubLabel>>()
                .await
                .context("invalid GitHub Labels response")?;

            let page_len = response.len();
            labels.extend(response.into_iter().map(|label| label.name));
            if page_len < 100 {
                break;
            }
        }
        Ok(labels)
    }

    async fn list_milestones(&self) -> Result<HashMap<String, GitHubMilestone>> {
        let mut milestones = HashMap::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let url = self.repo_url("milestones");
            let response = self
                .request(self.client.get(url))
                .query(&[
                    ("state", "all"),
                    ("per_page", "100"),
                    ("page", page_string.as_str()),
                ])
                .send()
                .await
                .context("failed to list repository milestones")?
                .error_for_status()
                .context("GitHub returned an error while listing milestones")?
                .json::<Vec<GitHubMilestone>>()
                .await
                .context("invalid GitHub Milestones response")?;

            let page_len = response.len();
            milestones.extend(
                response
                    .into_iter()
                    .map(|milestone| (milestone.title.clone(), milestone)),
            );
            if page_len < 100 {
                break;
            }
        }
        Ok(milestones)
    }

    async fn create_label(&self, name: &str, color: &str, description: &str) -> Result<()> {
        let url = self.repo_url("labels");
        self.request(self.client.post(url))
            .json(&CreateLabelRequest {
                name,
                color,
                description,
            })
            .send()
            .await
            .with_context(|| format!("failed to create label '{name}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error creating label '{name}'"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn create_milestone(&self, title: &str) -> Result<GitHubMilestone> {
        let url = self.repo_url("milestones");
        let response = self
            .request(self.client.post(url))
            .json(&CreateMilestoneRequest {
                title,
                description: "Editorial inbox for Rust Web Digest stories published in this month.",
            })
            .send()
            .await
            .with_context(|| format!("failed to create milestone '{title}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error creating milestone '{title}'"))?
            .json::<GitHubMilestone>()
            .await
            .with_context(|| format!("invalid GitHub milestone response for '{title}'"))?;
        pause_after_mutation().await;
        Ok(response)
    }

    async fn create_issue(&self, draft: &IssueDraft, milestone: Option<u64>) -> Result<u64> {
        let url = self.repo_url("issues");
        let issue = self
            .request(self.client.post(url))
            .json(&CreateIssueRequest {
                title: &draft.title,
                body: &draft.body,
                labels: &draft.labels,
                milestone,
            })
            .send()
            .await
            .with_context(|| format!("failed to create issue '{}'", draft.title))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error creating issue '{}'", draft.title))?
            .json::<CreatedIssue>()
            .await
            .with_context(|| format!("invalid GitHub issue response for '{}'", draft.title))?;
        pause_after_mutation().await;
        Ok(issue.number)
    }

    async fn update_issue(
        &self,
        number: u64,
        draft: &IssueDraft,
        body: &str,
        labels: &[String],
        milestone: Option<u64>,
    ) -> Result<()> {
        let url = self.repo_url(&format!("issues/{number}"));
        self.request(self.client.patch(url))
            .json(&UpdateIssueRequest {
                title: &draft.title,
                body,
                labels,
                milestone,
            })
            .send()
            .await
            .with_context(|| format!("failed to update issue #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error updating issue #{number}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn create_comment(&self, number: u64, body: &str) -> Result<()> {
        let url = self.repo_url(&format!("issues/{number}/comments"));
        self.request(self.client.post(url))
            .json(&CreateCommentRequest { body })
            .send()
            .await
            .with_context(|| format!("failed to comment on issue #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error commenting on issue #{number}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION);
        match self.token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    fn repo_url(&self, resource: &str) -> String {
        format!(
            "{}/repos/{}/{}/{}",
            self.api_url.trim_end_matches('/'),
            self.owner,
            self.repo,
            resource
        )
    }
}

async fn pause_after_mutation() {
    sleep(Duration::from_secs(1)).await;
}

pub fn build_issue_draft(
    candidate: &Candidate,
    project_name: Option<&str>,
    publishing: &PublishingConfig,
) -> IssueDraft {
    let story = Story {
        id: format!("candidate:{}", candidate.id),
        project_id: candidate.project_id.clone(),
        category: candidate.category.clone(),
        title: candidate.title.clone(),
        version: candidate
            .metadata
            .get("version")
            .or_else(|| candidate.metadata.get("tag_name"))
            .cloned(),
        published_at: candidate.published_at.clone(),
        discovered_at: candidate.discovered_at.clone(),
        candidates: vec![candidate.clone()],
    };
    build_story_issue_draft(&story, project_name, publishing)
}

pub fn build_story_issue_draft(
    story: &Story,
    project_name: Option<&str>,
    publishing: &PublishingConfig,
) -> IssueDraft {
    build_story_issue_draft_with_ai(story, project_name, publishing, None)
}

pub fn build_story_issue_draft_with_ai(
    story: &Story,
    project_name: Option<&str>,
    publishing: &PublishingConfig,
    ai_draft: Option<&EditorialDraft>,
) -> IssueDraft {
    let title = match project_name.or(story.project_id.as_deref()) {
        Some(project) => format!("[{project}] {}", story.title),
        None => story.title.clone(),
    };

    let mut labels = vec![
        publishing.candidate_label.clone(),
        publishing.new_status_label.clone(),
        format!("category:{}", story.category),
    ];
    labels.extend(
        story
            .candidates
            .iter()
            .map(|candidate| format!("type:{}", kind_slug(candidate.kind))),
    );
    labels.sort();
    labels.dedup();

    let project = project_name
        .or(story.project_id.as_deref())
        .unwrap_or("Unassigned");
    let version_line = story
        .version
        .as_deref()
        .map(|version| format!("- **Version:** {version}\n"))
        .unwrap_or_default();
    let source_grounded_summary = ai_draft
        .map(render_ai_issue_summary)
        .unwrap_or_default();
    let sources = render_sources(&story.candidates);
    let markers = std::iter::once(story_marker(&story.id))
        .chain(story.candidate_ids().map(candidate_marker))
        .collect::<Vec<_>>()
        .join("\n");

    let managed = format!(
        "{MANAGED_START}\n\
         ## Story\n\n\
         - **Project:** {project}\n\
         - **Category:** {}\n\
         {version_line}\
         - **First published:** {}\n\
         - **Sources:** {}\n\n\
         {source_grounded_summary}\
         ## Sources\n\n\
         {sources}\n\n\
         {markers}\n\
         {MANAGED_END}",
        story.category,
        story.published_at.to_rfc3339(),
        story.candidates.len(),
    );
    let body = format!(
        "{managed}\n\n## Editorial notes\n\n_Add review notes here. Change `status:new` to `status:selected`, `status:rejected`, or `status:watch` during editorial review._"
    );

    IssueDraft {
        story_id: story.id.clone(),
        candidate_ids: story.candidate_ids().map(ToOwned::to_owned).collect(),
        title,
        body,
        labels,
        milestone_title: story.published_at.format("%B %Y").to_string(),
    }
}

fn render_ai_issue_summary(draft: &EditorialDraft) -> String {
    let mut output = String::new();
    output.push_str("## Source-grounded summary\n\n");
    output.push_str("_AI-assisted summary generated only from the source material listed below. Verify important claims against the primary sources before publication._\n\n");
    output.push_str(&format!("### What changed\n\n{}\n\n", draft.what_changed.trim()));
    output.push_str(&format!("### Why it matters\n\n{}\n\n", draft.why_it_matters.trim()));

    if !draft.who_is_affected.trim().is_empty() {
        output.push_str(&format!(
            "### Who should care\n\n{}\n\n",
            draft.who_is_affected.trim()
        ));
    }

    output.push_str(&format!(
        "### Suggested action\n\n**{}** — {}\n\n",
        draft.action_required.label(),
        draft.action.trim()
    ));
    output.push_str(&format!(
        "_Summary confidence: {}_\n\n",
        draft.confidence.label()
    ));
    output
}

fn render_sources(candidates: &[Candidate]) -> String {
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|candidate| source_priority(candidate.kind));

    ordered
        .into_iter()
        .map(|candidate| {
            let context = source_context(candidate);
            let metadata = render_metadata(&candidate.metadata);
            let role = source_role(candidate.kind);
            format!(
                "### {} · {} · {}\n\n[{}]({})\n\n{}\n\n{}",
                kind_slug(candidate.kind),
                role,
                candidate.published_at.format("%Y-%m-%d"),
                candidate.title,
                candidate.url,
                context,
                metadata,
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn source_priority(kind: CandidateKind) -> u8 {
    match kind {
        CandidateKind::GitHubRelease => 0,
        CandidateKind::SecurityAdvisory => 1,
        CandidateKind::FeedArticle => 2,
        CandidateKind::CrateRelease => 3,
    }
}

fn source_role(kind: CandidateKind) -> &'static str {
    match kind {
        CandidateKind::GitHubRelease | CandidateKind::SecurityAdvisory => "primary source",
        CandidateKind::FeedArticle => "related source",
        CandidateKind::CrateRelease => "supporting publication",
    }
}

fn source_context(candidate: &Candidate) -> String {
    if candidate.kind == CandidateKind::CrateRelease {
        let version = candidate
            .metadata
            .get("version")
            .or_else(|| candidate.metadata.get("num"))
            .map(String::as_str)
            .unwrap_or("the tracked version");
        return format!(
            "Publication confirmation: `{}` was published to crates.io. Use the primary release notes or project article to understand what changed.",
            version
        );
    }

    let raw = candidate
        .raw_content
        .as_deref()
        .or(candidate.summary.as_deref())
        .unwrap_or("No source summary was provided.");
    extract_source_excerpt(raw, 2_400).unwrap_or_else(|| "No source summary was provided.".to_owned())
}

pub fn extract_source_excerpt(raw: &str, max_chars: usize) -> Option<String> {
    let normalized = raw.replace("\r\n", "\n");
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
            continue;
        }
        if trimmed.starts_with("<!--") || trimmed.starts_with("![") {
            continue;
        }
        if trimmed.starts_with('#') && !paragraphs.is_empty() {
            break;
        }
        current.push(trimmed.to_owned());
    }

    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }

    let mut output = String::new();
    for paragraph in paragraphs {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        if output.len() + paragraph.len() > max_chars {
            let remaining = max_chars.saturating_sub(output.len());
            if remaining > 80 {
                output.push_str(paragraph.chars().take(remaining).collect::<String>().trim_end());
                output.push('…');
            }
            break;
        }
        output.push_str(paragraph);
    }

    if output.trim().is_empty() {
        None
    } else {
        Some(output)
    }
}

fn render_metadata(metadata: &BTreeMap<String, String>) -> String {
    if metadata.is_empty() {
        return "_No additional metadata._".to_owned();
    }
    metadata
        .iter()
        .map(|(key, value)| format!("- **{key}:** {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn kind_slug(kind: CandidateKind) -> &'static str {
    match kind {
        CandidateKind::GitHubRelease => "release",
        CandidateKind::CrateRelease => "crate",
        CandidateKind::FeedArticle => "article",
        CandidateKind::SecurityAdvisory => "security",
    }
}

fn candidate_marker(candidate_id: &str) -> String {
    format!("{CANDIDATE_MARKER_PREFIX}{candidate_id}{CANDIDATE_MARKER_SUFFIX}")
}

fn story_marker(story_id: &str) -> String {
    format!("{STORY_MARKER_PREFIX}{story_id}{STORY_MARKER_SUFFIX}")
}

pub fn extract_candidate_ids(body: &str) -> Vec<String> {
    extract_markers(body, CANDIDATE_MARKER_PREFIX, CANDIDATE_MARKER_SUFFIX)
}

pub fn extract_story_ids(body: &str) -> Vec<String> {
    extract_markers(body, STORY_MARKER_PREFIX, STORY_MARKER_SUFFIX)
}

fn extract_markers(body: &str, prefix: &str, suffix: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix(prefix)
                .and_then(|value| value.strip_suffix(suffix))
                .map(ToOwned::to_owned)
        })
        .collect()
}

pub fn merge_managed_body(existing: &str, generated: &str) -> String {
    let Some(new_managed) = managed_section(generated) else {
        return generated.to_owned();
    };

    if let Some((start, end)) = managed_bounds(existing) {
        return format!("{}{}{}", &existing[..start], new_managed, &existing[end..]);
    }

    if let Some(editorial_start) = existing.find("## Editorial notes") {
        return format!("{new_managed}\n\n{}", &existing[editorial_start..]);
    }

    generated.to_owned()
}

fn managed_section(body: &str) -> Option<&str> {
    let (start, end) = managed_bounds(body)?;
    Some(&body[start..end])
}

fn managed_bounds(body: &str) -> Option<(usize, usize)> {
    let start = body.find(MANAGED_START)?;
    let end_start = body[start..].find(MANAGED_END)? + start;
    let end = end_start + MANAGED_END.len();
    Some((start, end))
}

fn matching_issue_indices(story: &Story, issues: &[GitHubIssue]) -> Vec<usize> {
    let candidate_ids = story.candidate_ids().collect::<HashSet<_>>();
    issues
        .iter()
        .enumerate()
        .filter(|(_, issue)| {
            let body = issue.body.as_deref().unwrap_or_default();
            extract_story_ids(body).iter().any(|id| id == &story.id)
                || extract_candidate_ids(body)
                    .iter()
                    .any(|id| candidate_ids.contains(id.as_str()))
        })
        .map(|(index, _)| index)
        .collect()
}

fn new_source_candidates<'a>(story: &'a Story, issue: &GitHubIssue) -> Vec<&'a Candidate> {
    let known = extract_candidate_ids(issue.body.as_deref().unwrap_or_default())
        .into_iter()
        .collect::<HashSet<_>>();
    story
        .candidates
        .iter()
        .filter(|candidate| !known.contains(&candidate.id))
        .collect()
}

fn render_source_update_comment(candidates: &[&Candidate]) -> String {
    let sources = candidates
        .iter()
        .map(|candidate| format!("- [{}]({})", candidate.title, candidate.url))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "The collector reconciled {} new source(s) into this story:\n\n{}",
        candidates.len(),
        sources
    )
}

fn merge_labels(
    issue: &GitHubIssue,
    draft_labels: &[String],
    publishing: &PublishingConfig,
) -> Vec<String> {
    let mut labels = issue_label_names(issue);
    let status_labels = publishing.status_labels();
    let has_existing_status = labels.iter().any(|label| status_labels.contains(label));
    labels.extend(
        draft_labels
            .iter()
            .filter(|label| {
                !(has_existing_status && label.as_str() == publishing.new_status_label.as_str())
            })
            .cloned(),
    );
    labels.sort();
    labels.dedup();
    labels
}

fn issue_label_names(issue: &GitHubIssue) -> Vec<String> {
    let mut labels = issue
        .labels
        .iter()
        .map(|label| label.name.clone())
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

fn existing_issue_from_draft(
    number: u64,
    draft: &IssueDraft,
    milestone: Option<u64>,
) -> GitHubIssue {
    existing_issue_from_updated(
        number,
        draft,
        draft.body.clone(),
        draft.labels.clone(),
        milestone,
    )
}

fn existing_issue_from_updated(
    number: u64,
    draft: &IssueDraft,
    body: String,
    labels: Vec<String>,
    milestone: Option<u64>,
) -> GitHubIssue {
    GitHubIssue {
        number,
        title: draft.title.clone(),
        body: Some(body),
        labels: labels
            .into_iter()
            .map(|name| GitHubIssueLabel { name })
            .collect(),
        milestone: milestone.map(|number| GitHubIssueMilestone { number }),
        pull_request: None,
    }
}

fn parse_repository(repository: &str) -> Result<(String, String)> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        bail!("repository must be in owner/repo form: {repository}");
    }
    Ok((owner.to_owned(), repo.to_owned()))
}

struct LabelSpec {
    color: &'static str,
    description: &'static str,
}

fn label_spec(label: &str) -> LabelSpec {
    if label == "candidate" {
        LabelSpec {
            color: "5319E7",
            description: "Automatically collected newsletter story",
        }
    } else if label.starts_with("status:") {
        LabelSpec {
            color: "D4C5F9",
            description: "Editorial workflow status",
        }
    } else if label.starts_with("category:") {
        LabelSpec {
            color: "0E8A16",
            description: "Newsletter editorial category",
        }
    } else {
        LabelSpec {
            color: "1D76DB",
            description: "Story source type",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use super::{
        build_issue_draft, extract_candidate_ids, extract_story_ids, merge_labels,
        merge_managed_body, parse_repository,
    };
    use crate::{
        config::PublishingConfig,
        domain::{Candidate, CandidateKind},
    };

    #[test]
    fn extracts_stable_markers() {
        let body = "<!-- rust-web-digest:story-id:release:axum:1.0.0 -->\n\
                    <!-- rust-web-digest:candidate-id:github-release:org/repo:42 -->";
        assert_eq!(extract_story_ids(body), vec!["release:axum:1.0.0"]);
        assert_eq!(
            extract_candidate_ids(body),
            vec!["github-release:org/repo:42"]
        );
    }

    #[test]
    fn preserves_human_editorial_notes_when_refreshing_managed_body() {
        let existing = "<!-- rust-web-digest:managed:start -->\nold\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nKeep this human note.";
        let generated = "<!-- rust-web-digest:managed:start -->\nnew\n<!-- rust-web-digest:managed:end -->\n\n## Editorial notes\n\nplaceholder";
        let merged = merge_managed_body(existing, generated);
        assert!(merged.contains("\nnew\n"));
        assert!(merged.contains("Keep this human note."));
        assert!(!merged.contains("placeholder"));
    }

    #[test]
    fn builds_legacy_single_candidate_issue_draft() {
        let candidate = Candidate {
            id: "github-release:tokio-rs/axum:42".to_owned(),
            kind: CandidateKind::GitHubRelease,
            title: "axum 1.0".to_owned(),
            url: "https://github.com/tokio-rs/axum/releases/tag/v1".to_owned(),
            source_id: "github:tokio-rs/axum".to_owned(),
            project_id: Some("axum".to_owned()),
            category: "frameworks".to_owned(),
            published_at: Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap(),
            discovered_at: Utc.with_ymd_and_hms(2026, 7, 10, 13, 0, 0).unwrap(),
            summary: Some("Release summary".to_owned()),
            raw_content: None,
            metadata: BTreeMap::from([("tag_name".to_owned(), "v1.0.0".to_owned())]),
        };

        let draft = build_issue_draft(&candidate, Some("Axum"), &PublishingConfig::default());
        assert_eq!(draft.title, "[Axum] axum 1.0");
        assert_eq!(draft.milestone_title, "July 2026");
        assert!(draft.labels.contains(&"type:release".to_owned()));
        assert!(draft.body.contains("github-release:tokio-rs/axum:42"));
    }

    #[test]
    fn rejects_invalid_repository_name() {
        assert!(parse_repository("owner/repo").is_ok());
        assert!(parse_repository("owner").is_err());
        assert!(parse_repository("owner/repo/extra").is_err());
    }
    #[test]
    fn reconciliation_does_not_readd_new_status_to_selected_issue() {
        let publishing = PublishingConfig::default();
        let issue = GitHubIssue {
            number: 42,
            title: "Selected story".to_owned(),
            body: Some(String::new()),
            labels: vec![
                GitHubIssueLabel {
                    name: publishing.candidate_label.clone(),
                },
                GitHubIssueLabel {
                    name: publishing.selected_status_label.clone(),
                },
            ],
            milestone: None,
            pull_request: None,
        };
        let draft = vec![
            publishing.candidate_label.clone(),
            publishing.new_status_label.clone(),
            "type:article".to_owned(),
        ];

        let labels = merge_labels(&issue, &draft, &publishing);
        assert!(labels.contains(&publishing.selected_status_label));
        assert!(!labels.contains(&publishing.new_status_label));
        assert!(labels.contains(&"type:article".to_owned()));
    }

}
