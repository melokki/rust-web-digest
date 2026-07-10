use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, sleep};

use crate::{
    config::PublishingConfig,
    github_issues::{extract_story_ids, extract_candidate_ids},
};

const GITHUB_API_VERSION: &str = "2026-03-10";
const MONTH_MARKER_PREFIX: &str = "<!-- rust-web-digest:month:";
const MONTH_MARKER_SUFFIX: &str = " -->";
const PARENT_MANAGED_START: &str = "<!-- rust-web-digest:parent-managed:start -->";
const PARENT_MANAGED_END: &str = "<!-- rust-web-digest:parent-managed:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorialStatus {
    New,
    Watch,
    Selected,
    Rejected,
    Published,
    Skipped,
}

impl EditorialStatus {
    pub fn label<'a>(&self, publishing: &'a PublishingConfig) -> &'a str {
        match self {
            Self::New => &publishing.new_status_label,
            Self::Watch => &publishing.watch_status_label,
            Self::Selected => &publishing.selected_status_label,
            Self::Rejected => &publishing.rejected_status_label,
            Self::Published => &publishing.published_status_label,
            Self::Skipped => &publishing.skipped_status_label,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorialStatusFilter {
    All,
    Status(EditorialStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorialMonth {
    pub key: String,
    pub title: String,
}

impl EditorialMonth {
    pub fn parse(value: &str) -> Result<Self> {
        let date = NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
            .with_context(|| format!("invalid month '{value}'; expected YYYY-MM"))?;
        Ok(Self::from_date(date))
    }

    fn from_title(value: &str) -> Result<Self> {
        let date = NaiveDate::parse_from_str(&format!("01 {value}"), "%d %B %Y")
            .with_context(|| format!("invalid editorial milestone title '{value}'"))?;
        Ok(Self::from_date(date))
    }

    fn from_date(date: NaiveDate) -> Self {
        Self {
            key: format!("{:04}-{:02}", date.year(), date.month()),
            title: date.format("%B %Y").to_string(),
        }
    }

    pub fn next_month(&self) -> Self {
        let date = NaiveDate::parse_from_str(&format!("{}-01", self.key), "%Y-%m-%d")
            .expect("EditorialMonth keys are always valid YYYY-MM values");
        let next = if date.month() == 12 {
            NaiveDate::from_ymd_opt(date.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1).unwrap()
        };
        Self::from_date(next)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorialStoryRecord {
    pub issue_number: u64,
    pub issue_url: String,
    pub title: String,
    pub story_id: Option<String>,
    pub candidate_ids: Vec<String>,
    pub status: Option<EditorialStatus>,
    pub category: Option<String>,
    pub kinds: Vec<String>,
    pub labels: Vec<String>,
    pub milestone: String,
    pub body: String,
    pub editorial_notes: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncMonthReport {
    pub candidate_count: usize,
    pub parent_created: bool,
    pub parent_updated: bool,
    pub sub_issues_added: usize,
    pub parent_conflicts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusChange {
    pub issue_number: u64,
    pub previous: Option<EditorialStatus>,
    pub current: EditorialStatus,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveMonthReport {
    pub considered: usize,
    pub published_closed: usize,
    pub rejected_closed: usize,
    pub skipped_closed: usize,
    pub watch_moved: usize,
    pub parent_closed: bool,
    pub milestone_closed: bool,
    pub unchanged: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubIssue {
    id: u64,
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    #[serde(default)]
    labels: Vec<GitHubLabel>,
    milestone: Option<GitHubMilestoneRef>,
    state: Option<String>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubLabel {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubMilestoneRef {
    number: u64,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubMilestone {
    number: u64,
    title: String,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatedIssue {
    id: u64,
    number: u64,
}

#[derive(Debug, Serialize)]
struct IssueLabelsRequest<'a> {
    labels: &'a [String],
}

#[derive(Debug, Serialize)]
struct ArchiveIssueRequest<'a> {
    labels: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_reason: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct CloseMilestoneRequest<'a> {
    state: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateIssueRequest<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    milestone: u64,
}

#[derive(Debug, Serialize)]
struct UpdateParentIssueRequest<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    milestone: u64,
}

#[derive(Debug, Serialize)]
struct CreateLabelRequest<'a> {
    name: &'a str,
    color: &'a str,
    description: &'a str,
}

#[derive(Debug, Serialize)]
struct AddSubIssueRequest {
    sub_issue_id: u64,
    replace_parent: bool,
}

pub struct EditorialClient<'a> {
    client: &'a Client,
    api_url: &'a str,
    token: Option<&'a str>,
    owner: String,
    repo: String,
    publishing: &'a PublishingConfig,
}

impl<'a> EditorialClient<'a> {
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

    pub async fn list(
        &self,
        month: &EditorialMonth,
        filter: EditorialStatusFilter,
    ) -> Result<Vec<EditorialStoryRecord>> {
        let Some(milestone) = self.find_milestone(&month.title).await? else {
            return Ok(Vec::new());
        };
        let issues = self.list_candidate_issues(milestone.number).await?;
        let mut records = issues
            .iter()
            .filter(|issue| match filter {
                EditorialStatusFilter::All => true,
                EditorialStatusFilter::Status(status) => {
                    issue_has_label(issue, status.label(self.publishing))
                }
            })
            .map(|issue| issue_to_record(issue, self.publishing))
            .collect::<Result<Vec<_>>>()?;
        records.sort_by(|left, right| {
            left.category
                .cmp(&right.category)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.issue_number.cmp(&right.issue_number))
        });
        Ok(records)
    }

    pub async fn set_status(
        &self,
        issue_number: u64,
        status: EditorialStatus,
        dry_run: bool,
    ) -> Result<StatusChange> {
        let issue = self.get_issue(issue_number).await?;
        if issue.pull_request.is_some() || !issue_has_label(&issue, &self.publishing.candidate_label) {
            bail!("issue #{issue_number} is not a Rust Web Digest candidate issue");
        }

        let previous = issue_status(&issue, self.publishing)?;
        let target = status.label(self.publishing).to_owned();
        let status_labels = self.publishing.status_labels();
        let mut labels = issue_label_names(&issue)
            .into_iter()
            .filter(|label| !status_labels.contains(label))
            .collect::<Vec<_>>();
        labels.push(target);
        labels.sort();
        labels.dedup();

        let changed = previous != Some(status) || issue_label_names(&issue) != labels;
        if changed && !dry_run {
            self.require_token()?;
            let url = self.repo_url(&format!("issues/{issue_number}"));
            self.request(self.client.patch(url))
                .json(&IssueLabelsRequest { labels: &labels })
                .send()
                .await
                .with_context(|| format!("failed to update editorial status for issue #{issue_number}"))?
                .error_for_status()
                .with_context(|| format!("GitHub returned an error updating issue #{issue_number}"))?;
            pause_after_mutation().await;

            if let Some(milestone) = issue.milestone.as_ref() {
                let month = EditorialMonth::from_title(&milestone.title)?;
                self.sync_month(&month, false).await?;
            }
        }

        Ok(StatusChange {
            issue_number,
            previous,
            current: status,
            changed,
        })
    }

    pub async fn sync_month(
        &self,
        month: &EditorialMonth,
        dry_run: bool,
    ) -> Result<SyncMonthReport> {
        let mut report = SyncMonthReport::default();
        if !dry_run {
            self.require_token()?;
        }

        let Some(milestone) = self.find_milestone(&month.title).await? else {
            return Ok(report);
        };

        let candidate_issues = self.list_candidate_issues(milestone.number).await?;
        report.candidate_count = candidate_issues.len();

        if !dry_run {
            self.ensure_label(
                &self.publishing.monthly_parent_label,
                "5319e7",
                "Monthly editorial workspace for the Rust Web Digest",
            )
            .await?;
        }

        let parent_matches = self.list_month_parent_issues(month).await?;
        if parent_matches.len() > 1 {
            bail!(
                "month {} matches multiple editorial parent issues: {:?}",
                month.key,
                parent_matches
                    .iter()
                    .map(|issue| issue.number)
                    .collect::<Vec<_>>()
            );
        }

        let generated_body = render_month_parent_body(month, &candidate_issues, self.publishing)?;
        let title = format!(
            "{} — {}",
            self.publishing.monthly_parent_title_prefix, month.title
        );

        let parent = match parent_matches.into_iter().next() {
            Some(parent) => {
                let body = merge_parent_body(
                    parent.body.as_deref().unwrap_or_default(),
                    &generated_body,
                );
                let labels = merge_required_label(
                    issue_label_names(&parent),
                    &self.publishing.monthly_parent_label,
                );
                let milestone_changed = parent
                    .milestone
                    .as_ref()
                    .map(|item| item.number)
                    != Some(milestone.number);
                let changed = parent.title != title
                    || parent.body.as_deref().unwrap_or_default() != body
                    || issue_label_names(&parent) != labels
                    || milestone_changed;
                if changed {
                    report.parent_updated = true;
                    if !dry_run {
                        self.update_parent_issue(
                            parent.number,
                            &title,
                            &body,
                            &labels,
                            milestone.number,
                        )
                        .await?;
                    }
                }
                GitHubIssue {
                    title,
                    body: Some(body),
                    labels: labels
                        .into_iter()
                        .map(|name| GitHubLabel { name })
                        .collect(),
                    milestone: Some(GitHubMilestoneRef {
                        number: milestone.number,
                        title: milestone.title.clone(),
                    }),
                    ..parent
                }
            }
            None if dry_run => {
                report.parent_created = true;
                GitHubIssue {
                    id: 0,
                    number: 0,
                    html_url: String::new(),
                    title,
                    body: Some(generated_body.clone()),
                    labels: vec![GitHubLabel {
                        name: self.publishing.monthly_parent_label.clone(),
                    }],
                    milestone: Some(GitHubMilestoneRef {
                        number: milestone.number,
                        title: milestone.title.clone(),
                    }),
                    state: Some("open".to_owned()),
                    pull_request: None,
                }
            }
            None => {
                report.parent_created = true;
                let created = self
                    .create_parent_issue(&title, &generated_body, milestone.number)
                    .await?;
                GitHubIssue {
                    id: created.id,
                    number: created.number,
                    html_url: String::new(),
                    title,
                    body: Some(generated_body.clone()),
                    labels: vec![GitHubLabel {
                        name: self.publishing.monthly_parent_label.clone(),
                    }],
                    milestone: Some(GitHubMilestoneRef {
                        number: milestone.number,
                        title: milestone.title.clone(),
                    }),
                    state: Some("open".to_owned()),
                    pull_request: None,
                }
            }
        };

        if dry_run && parent.number == 0 {
            report.sub_issues_added = candidate_issues.len();
            return Ok(report);
        }

        let existing_sub_issue_ids = self
            .list_sub_issues(parent.number)
            .await?
            .into_iter()
            .map(|issue| issue.id)
            .collect::<HashSet<_>>();

        for issue in candidate_issues {
            if existing_sub_issue_ids.contains(&issue.id) {
                continue;
            }

            match self.get_parent_issue(issue.number).await? {
                Some(existing_parent) if existing_parent.number != parent.number => {
                    eprintln!(
                        "Issue #{} already belongs to parent #{}; leaving it unchanged",
                        issue.number, existing_parent.number
                    );
                    report.parent_conflicts += 1;
                }
                Some(_) => {}
                None => {
                    report.sub_issues_added += 1;
                    if !dry_run {
                        self.add_sub_issue(parent.number, issue.id).await?;
                    }
                }
            }
        }

        Ok(report)
    }

    pub async fn export_selected(
        &self,
        month: &EditorialMonth,
        output: impl AsRef<Path>,
    ) -> Result<Vec<EditorialStoryRecord>> {
        let records = self
            .list(
                month,
                EditorialStatusFilter::Status(EditorialStatus::Selected),
            )
            .await?;
        let output = output.as_ref();
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let payload = serde_json::to_string_pretty(&records)
            .context("failed to serialize selected editorial stories")?;
        fs::write(output, format!("{payload}\n"))
            .with_context(|| format!("failed to write {}", output.display()))?;
        Ok(records)
    }


    pub async fn archive_month(
        &self,
        month: &EditorialMonth,
        dry_run: bool,
    ) -> Result<ArchiveMonthReport> {
        let mut report = ArchiveMonthReport::default();
        if !dry_run {
            self.require_token()?;
        }

        let Some(milestone) = self.find_milestone(&month.title).await? else {
            return Ok(report);
        };

        if !dry_run {
            self.ensure_label(
                &self.publishing.published_status_label,
                "0E8A16",
                "Published in a Rust Web Digest issue",
            )
            .await?;
            self.ensure_label(
                &self.publishing.skipped_status_label,
                "BFD4F2",
                "Skipped when the month was archived",
            )
            .await?;
        }

        let next_month = month.next_month();
        let next_milestone = if self.publishing.ensure_milestones {
            match self.find_milestone(&next_month.title).await? {
                Some(item) => Some(item.number),
                None if dry_run => Some(0),
                None => Some(self.create_milestone(&next_month.title).await?),
            }
        } else {
            None
        };

        let issues = self.list_candidate_issues(milestone.number).await?;
        report.considered = issues.len();

        for issue in issues {
            let status = issue_status(&issue, self.publishing)?;
            let state = issue.state.as_deref().unwrap_or("open");

            match status {
                Some(EditorialStatus::Watch) => {
                    let desired_milestone = next_milestone.or_else(|| issue.milestone.as_ref().map(|item| item.number));
                    let unchanged = state == "open"
                        && issue.milestone.as_ref().map(|item| item.number) == desired_milestone;
                    if unchanged {
                        report.unchanged += 1;
                    } else {
                        report.watch_moved += 1;
                        if !dry_run {
                            let labels = issue_label_names(&issue);
                            self.archive_issue(
                                issue.number,
                                &labels,
                                desired_milestone,
                                Some("open"),
                                None,
                            )
                            .await?;
                        }
                    }
                }
                Some(EditorialStatus::Published) => {
                    if state == "closed" {
                        report.unchanged += 1;
                    } else {
                        report.published_closed += 1;
                        if !dry_run {
                            let labels = issue_label_names(&issue);
                            self.archive_issue(
                                issue.number,
                                &labels,
                                issue.milestone.as_ref().map(|item| item.number),
                                Some("closed"),
                                Some("completed"),
                            )
                            .await?;
                        }
                    }
                }
                Some(EditorialStatus::Selected) => {
                    report.published_closed += 1;
                    if !dry_run {
                        let labels = transition_labels(
                            &issue_label_names(&issue),
                            EditorialStatus::Published,
                            self.publishing,
                        );
                        self.archive_issue(
                            issue.number,
                            &labels,
                            issue.milestone.as_ref().map(|item| item.number),
                            Some("closed"),
                            Some("completed"),
                        )
                        .await?;
                    }
                }
                Some(EditorialStatus::Rejected) => {
                    if state == "closed" {
                        report.unchanged += 1;
                    } else {
                        report.rejected_closed += 1;
                        if !dry_run {
                            let labels = issue_label_names(&issue);
                            self.archive_issue(
                                issue.number,
                                &labels,
                                issue.milestone.as_ref().map(|item| item.number),
                                Some("closed"),
                                Some("not_planned"),
                            )
                            .await?;
                        }
                    }
                }
                Some(EditorialStatus::Skipped) => {
                    if state == "closed" {
                        report.unchanged += 1;
                    } else {
                        report.skipped_closed += 1;
                        if !dry_run {
                            let labels = issue_label_names(&issue);
                            self.archive_issue(
                                issue.number,
                                &labels,
                                issue.milestone.as_ref().map(|item| item.number),
                                Some("closed"),
                                Some("not_planned"),
                            )
                            .await?;
                        }
                    }
                }
                Some(EditorialStatus::New) | None => {
                    report.skipped_closed += 1;
                    if !dry_run {
                        let labels = transition_labels(
                            &issue_label_names(&issue),
                            EditorialStatus::Skipped,
                            self.publishing,
                        );
                        self.archive_issue(
                            issue.number,
                            &labels,
                            issue.milestone.as_ref().map(|item| item.number),
                            Some("closed"),
                            Some("not_planned"),
                        )
                        .await?;
                    }
                }
            }
        }

        for parent in self.list_month_parent_issues(month).await? {
            if parent.state.as_deref() == Some("closed") {
                continue;
            }
            report.parent_closed = true;
            if !dry_run {
                let labels = issue_label_names(&parent);
                self.archive_issue(
                    parent.number,
                    &labels,
                    parent.milestone.as_ref().map(|item| item.number),
                    Some("closed"),
                    Some("completed"),
                )
                .await?;
            }
        }

        if milestone.state.as_deref() != Some("closed") {
            report.milestone_closed = true;
            if !dry_run {
                self.close_milestone(milestone.number).await?;
            }
        }

        Ok(report)
    }

    async fn get_issue(&self, number: u64) -> Result<GitHubIssue> {
        let url = self.repo_url(&format!("issues/{number}"));
        self.request(self.client.get(url))
            .send()
            .await
            .with_context(|| format!("failed to fetch issue #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error fetching issue #{number}"))?
            .json::<GitHubIssue>()
            .await
            .with_context(|| format!("invalid GitHub response for issue #{number}"))
    }

    async fn find_milestone(&self, title: &str) -> Result<Option<GitHubMilestone>> {
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
                .context("failed to list milestones")?
                .error_for_status()
                .context("GitHub returned an error listing milestones")?
                .json::<Vec<GitHubMilestone>>()
                .await
                .context("invalid GitHub milestones response")?;
            let page_len = response.len();
            if let Some(milestone) = response.into_iter().find(|item| item.title == title) {
                return Ok(Some(milestone));
            }
            if page_len < 100 {
                break;
            }
        }
        Ok(None)
    }

    async fn list_candidate_issues(&self, milestone: u64) -> Result<Vec<GitHubIssue>> {
        let mut issues = Vec::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let milestone_string = milestone.to_string();
            let url = self.repo_url("issues");
            let response = self
                .request(self.client.get(url))
                .query(&[
                    ("state", "all"),
                    ("labels", self.publishing.candidate_label.as_str()),
                    ("milestone", milestone_string.as_str()),
                    ("per_page", "100"),
                    ("page", page_string.as_str()),
                ])
                .send()
                .await
                .context("failed to list monthly candidate issues")?
                .error_for_status()
                .context("GitHub returned an error listing monthly candidate issues")?
                .json::<Vec<GitHubIssue>>()
                .await
                .context("invalid GitHub candidate Issues response")?;
            let page_len = response.len();
            issues.extend(response.into_iter().filter(|issue| issue.pull_request.is_none()));
            if page_len < 100 {
                break;
            }
        }
        Ok(issues)
    }

    async fn list_month_parent_issues(&self, month: &EditorialMonth) -> Result<Vec<GitHubIssue>> {
        let marker = month_marker(&month.key);
        let mut matches = Vec::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let url = self.repo_url("issues");
            let response = self
                .request(self.client.get(url))
                .query(&[
                    ("state", "all"),
                    ("labels", self.publishing.monthly_parent_label.as_str()),
                    ("per_page", "100"),
                    ("page", page_string.as_str()),
                ])
                .send()
                .await
                .context("failed to list editorial parent issues")?
                .error_for_status()
                .context("GitHub returned an error listing editorial parent issues")?
                .json::<Vec<GitHubIssue>>()
                .await
                .context("invalid GitHub editorial parent response")?;
            let page_len = response.len();
            matches.extend(response.into_iter().filter(|issue| {
                issue.pull_request.is_none()
                    && issue
                        .body
                        .as_deref()
                        .is_some_and(|body| body.contains(&marker))
            }));
            if page_len < 100 {
                break;
            }
        }
        Ok(matches)
    }

    async fn ensure_label(&self, name: &str, color: &str, description: &str) -> Result<()> {
        let url = self.repo_url(&format!("labels/{name}"));
        let response = self.request(self.client.get(url)).send().await?;
        match response.status() {
            StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => {
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
            _ => {
                response
                    .error_for_status()
                    .with_context(|| format!("GitHub returned an error fetching label '{name}'"))?;
                Ok(())
            }
        }
    }

    async fn create_parent_issue(
        &self,
        title: &str,
        body: &str,
        milestone: u64,
    ) -> Result<CreatedIssue> {
        let labels = vec![self.publishing.monthly_parent_label.clone()];
        let url = self.repo_url("issues");
        let issue = self
            .request(self.client.post(url))
            .json(&CreateIssueRequest {
                title,
                body,
                labels: &labels,
                milestone,
            })
            .send()
            .await
            .context("failed to create monthly editorial parent issue")?
            .error_for_status()
            .context("GitHub returned an error creating monthly editorial parent issue")?
            .json::<CreatedIssue>()
            .await
            .context("invalid GitHub response creating monthly editorial parent issue")?;
        pause_after_mutation().await;
        Ok(issue)
    }

    async fn update_parent_issue(
        &self,
        number: u64,
        title: &str,
        body: &str,
        labels: &[String],
        milestone: u64,
    ) -> Result<()> {
        let url = self.repo_url(&format!("issues/{number}"));
        self.request(self.client.patch(url))
            .json(&UpdateParentIssueRequest {
                title,
                body,
                labels,
                milestone,
            })
            .send()
            .await
            .with_context(|| format!("failed to update monthly parent issue #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error updating parent issue #{number}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn list_sub_issues(&self, parent_number: u64) -> Result<Vec<GitHubIssue>> {
        let mut issues = Vec::new();
        for page in 1..=self.publishing.github_max_pages {
            let page_string = page.to_string();
            let url = self.repo_url(&format!("issues/{parent_number}/sub_issues"));
            let response = self
                .request(self.client.get(url))
                .query(&[("per_page", "100"), ("page", page_string.as_str())])
                .send()
                .await
                .with_context(|| format!("failed to list sub-issues of #{parent_number}"))?
                .error_for_status()
                .with_context(|| format!("GitHub returned an error listing sub-issues of #{parent_number}"))?
                .json::<Vec<GitHubIssue>>()
                .await
                .context("invalid GitHub sub-issues response")?;
            let page_len = response.len();
            issues.extend(response);
            if page_len < 100 {
                break;
            }
        }
        Ok(issues)
    }

    async fn get_parent_issue(&self, issue_number: u64) -> Result<Option<GitHubIssue>> {
        let url = self.repo_url(&format!("issues/{issue_number}/parent"));
        let response = self
            .request(self.client.get(url))
            .send()
            .await
            .with_context(|| format!("failed to check parent of issue #{issue_number}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let issue = response
            .error_for_status()
            .with_context(|| format!("GitHub returned an error checking parent of issue #{issue_number}"))?
            .json::<GitHubIssue>()
            .await
            .context("invalid GitHub parent issue response")?;
        Ok(Some(issue))
    }

    async fn add_sub_issue(&self, parent_number: u64, sub_issue_id: u64) -> Result<()> {
        let url = self.repo_url(&format!("issues/{parent_number}/sub_issues"));
        self.request(self.client.post(url))
            .json(&AddSubIssueRequest {
                sub_issue_id,
                replace_parent: false,
            })
            .send()
            .await
            .with_context(|| format!("failed to attach sub-issue to parent #{parent_number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error attaching sub-issue to parent #{parent_number}"))?;
        pause_after_mutation().await;
        Ok(())
    }


    async fn create_milestone(&self, title: &str) -> Result<u64> {
        let url = self.repo_url("milestones");
        let response = self
            .request(self.client.post(url))
            .json(&serde_json::json!({
                "title": title,
                "description": "Editorial inbox for Rust Web Digest stories published in this month."
            }))
            .send()
            .await
            .with_context(|| format!("failed to create milestone '{title}'"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error creating milestone '{title}'"))?
            .json::<GitHubMilestone>()
            .await
            .with_context(|| format!("invalid GitHub milestone response for '{title}'"))?;
        pause_after_mutation().await;
        Ok(response.number)
    }

    async fn archive_issue(
        &self,
        number: u64,
        labels: &[String],
        milestone: Option<u64>,
        state: Option<&str>,
        state_reason: Option<&str>,
    ) -> Result<()> {
        let url = self.repo_url(&format!("issues/{number}"));
        self.request(self.client.patch(url))
            .json(&ArchiveIssueRequest {
                labels,
                milestone,
                state,
                state_reason,
            })
            .send()
            .await
            .with_context(|| format!("failed to archive issue #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error archiving issue #{number}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    async fn close_milestone(&self, number: u64) -> Result<()> {
        let url = self.repo_url(&format!("milestones/{number}"));
        self.request(self.client.patch(url))
            .json(&CloseMilestoneRequest { state: "closed" })
            .send()
            .await
            .with_context(|| format!("failed to close milestone #{number}"))?
            .error_for_status()
            .with_context(|| format!("GitHub returned an error closing milestone #{number}"))?;
        pause_after_mutation().await;
        Ok(())
    }

    fn require_token(&self) -> Result<()> {
        if self.token.is_none() {
            bail!("GITHUB_TOKEN is required for editorial mutations");
        }
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

fn issue_to_record(
    issue: &GitHubIssue,
    publishing: &PublishingConfig,
) -> Result<EditorialStoryRecord> {
    let body = issue.body.clone().unwrap_or_default();
    let labels = issue_label_names(issue);
    let story_id = extract_story_ids(&body).into_iter().next();
    let category = labels
        .iter()
        .find_map(|label| label.strip_prefix("category:").map(ToOwned::to_owned));
    let mut kinds = labels
        .iter()
        .filter_map(|label| label.strip_prefix("type:").map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    kinds.sort();
    let status = issue_status(issue, publishing)?;

    Ok(EditorialStoryRecord {
        issue_number: issue.number,
        issue_url: issue.html_url.clone(),
        title: issue.title.clone(),
        story_id,
        candidate_ids: extract_candidate_ids(&body),
        status,
        category,
        kinds,
        labels,
        milestone: issue
            .milestone
            .as_ref()
            .map(|item| item.title.clone())
            .unwrap_or_default(),
        editorial_notes: extract_editorial_notes(&body),
        body,
    })
}

fn issue_status(
    issue: &GitHubIssue,
    publishing: &PublishingConfig,
) -> Result<Option<EditorialStatus>> {
    let statuses = [
        EditorialStatus::New,
        EditorialStatus::Watch,
        EditorialStatus::Selected,
        EditorialStatus::Rejected,
        EditorialStatus::Published,
        EditorialStatus::Skipped,
    ]
    .into_iter()
    .filter(|status| issue_has_label(issue, status.label(publishing)))
    .collect::<Vec<_>>();

    match statuses.as_slice() {
        [] => Ok(None),
        [status] => Ok(Some(*status)),
        _ => bail!("issue #{} has multiple editorial status labels", issue.number),
    }
}

pub fn transition_labels(
    labels: &[String],
    status: EditorialStatus,
    publishing: &PublishingConfig,
) -> Vec<String> {
    let status_labels = publishing.status_labels();
    let mut next = labels
        .iter()
        .filter(|label| !status_labels.contains(*label))
        .cloned()
        .collect::<Vec<_>>();
    next.push(status.label(publishing).to_owned());
    next.sort();
    next.dedup();
    next
}

fn render_month_parent_body(
    month: &EditorialMonth,
    issues: &[GitHubIssue],
    publishing: &PublishingConfig,
) -> Result<String> {
    let mut new = 0usize;
    let mut watch = 0usize;
    let mut selected = 0usize;
    let mut rejected = 0usize;
    let mut published = 0usize;
    let mut skipped = 0usize;
    let mut unset = 0usize;

    for issue in issues {
        match issue_status(issue, publishing)? {
            Some(EditorialStatus::New) => new += 1,
            Some(EditorialStatus::Watch) => watch += 1,
            Some(EditorialStatus::Selected) => selected += 1,
            Some(EditorialStatus::Rejected) => rejected += 1,
            Some(EditorialStatus::Published) => published += 1,
            Some(EditorialStatus::Skipped) => skipped += 1,
            None => unset += 1,
        }
    }

    Ok(format!(
        "{}\n{}\n\
         ## Editorial overview\n\n\
         This issue is the editorial workspace for **{}**. Candidate stories are attached as sub-issues.\n\n\
         | Status | Count |\n\
         |---|---:|\n\
         | New | {new} |\n\
         | Watch | {watch} |\n\
         | Selected | {selected} |\n\
         | Rejected | {rejected} |\n\
         | Published | {published} |\n\
         | Skipped | {skipped} |\n\
         | Unset | {unset} |\n\
         | **Total** | **{}** |\n{}\n\n\
         ## Editorial notes\n\n\
         _Use this area for issue-level planning notes. The status summary is regenerated by `editorial sync-month`._",
        month_marker(&month.key),
        PARENT_MANAGED_START,
        month.title,
        issues.len(),
        PARENT_MANAGED_END,
    ))
}


pub fn merge_parent_body(existing: &str, generated: &str) -> String {
    let Some(new_managed) = parent_managed_section(generated) else {
        return generated.to_owned();
    };

    if let Some((start, end)) = parent_managed_bounds(existing) {
        return format!("{}{}{}", &existing[..start], new_managed, &existing[end..]);
    }

    if let Some(notes_start) = existing.find("## Editorial notes") {
        let marker = generated
            .lines()
            .next()
            .unwrap_or_default();
        return format!("{marker}\n{new_managed}\n\n{}", &existing[notes_start..]);
    }

    generated.to_owned()
}

fn parent_managed_section(body: &str) -> Option<&str> {
    let (start, end) = parent_managed_bounds(body)?;
    Some(&body[start..end])
}

fn parent_managed_bounds(body: &str) -> Option<(usize, usize)> {
    let start = body.find(PARENT_MANAGED_START)?;
    let end_start = body[start..].find(PARENT_MANAGED_END)? + start;
    let end = end_start + PARENT_MANAGED_END.len();
    Some((start, end))
}

pub fn extract_editorial_notes(body: &str) -> Option<String> {
    let (_, notes) = body.split_once("## Editorial notes")?;
    let notes = notes.trim();
    if notes.is_empty() {
        None
    } else {
        Some(notes.to_owned())
    }
}

fn month_marker(month: &str) -> String {
    format!("{MONTH_MARKER_PREFIX}{month}{MONTH_MARKER_SUFFIX}")
}

fn issue_has_label(issue: &GitHubIssue, label: &str) -> bool {
    issue.labels.iter().any(|item| item.name == label)
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

fn merge_required_label(mut labels: Vec<String>, required: &str) -> Vec<String> {
    labels.push(required.to_owned());
    labels.sort();
    labels.dedup();
    labels
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

async fn pause_after_mutation() {
    sleep(Duration::from_secs(1)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, status_label: &str) -> GitHubIssue {
        GitHubIssue {
            id: number + 1000,
            number,
            html_url: format!("https://github.com/example/digest/issues/{number}"),
            title: format!("Candidate {number}"),
            body: Some(format!(
                "<!-- rust-web-digest:story-id:story-{number} -->\n\n## Editorial notes\n\nHuman note {number}."
            )),
            labels: vec![
                GitHubLabel {
                    name: "candidate".to_owned(),
                },
                GitHubLabel {
                    name: status_label.to_owned(),
                },
            ],
            milestone: Some(GitHubMilestoneRef {
                number: 7,
                title: "July 2026".to_owned(),
            }),
            state: Some("open".to_owned()),
            pull_request: None,
        }
    }

    #[test]
    fn parent_body_counts_statuses() {
        let publishing = PublishingConfig::default();
        let month = EditorialMonth::parse("2026-07").unwrap();
        let issues = vec![
            issue(1, &publishing.new_status_label),
            issue(2, &publishing.selected_status_label),
            issue(3, &publishing.selected_status_label),
        ];

        let body = render_month_parent_body(&month, &issues, &publishing).unwrap();
        assert!(body.contains("| New | 1 |"));
        assert!(body.contains("| Selected | 2 |"));
        assert!(body.contains("| **Total** | **3** |"));
        assert!(body.contains("<!-- rust-web-digest:month:2026-07 -->"));
    }

    #[test]
    fn issue_record_rejects_multiple_status_labels() {
        let publishing = PublishingConfig::default();
        let mut candidate = issue(1, &publishing.new_status_label);
        candidate.labels.push(GitHubLabel {
            name: publishing.selected_status_label.clone(),
        });

        let error = issue_to_record(&candidate, &publishing)
            .unwrap_err()
            .to_string();
        assert!(error.contains("multiple editorial status labels"));
    }
}
