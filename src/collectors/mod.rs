pub mod crates_io;
pub mod feed;
pub mod github;
pub mod rustsec;

use std::{collections::BTreeMap, env, time::Duration};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;

use crate::{config::AppConfig, domain::Candidate};

use self::{
    crates_io::CratesIoCollector,
    feed::FeedCollector,
    github::GitHubReleaseCollector,
    rustsec::RustSecCollector,
};

#[derive(Debug, Clone)]
pub struct CollectionWindow {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl CollectionWindow {
    pub fn contains(&self, timestamp: &DateTime<Utc>) -> bool {
        timestamp >= &self.since && timestamp <= &self.until
    }
}

#[derive(Debug, Default)]
pub struct CollectionReport {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
    pub counts: BTreeMap<String, usize>,
}

pub async fn collect_all(
    config: &AppConfig,
    window: CollectionWindow,
    discovered_at: DateTime<Utc>,
) -> Result<CollectionReport> {
    let client = Client::builder()
        .user_agent("rust-web-digest/0.7")
        .timeout(Duration::from_secs(config.collection.request_timeout_seconds))
        .build()
        .context("failed to build HTTP client")?;

    let github_token = env::var("GITHUB_TOKEN").ok();
    let github = GitHubReleaseCollector::new(
        &client,
        &config.collection,
        github_token.as_deref(),
    );
    let feed = FeedCollector::new(&client);
    let rustsec = RustSecCollector::new(&client, &config.security.osv_api_url);

    let mut report = CollectionReport::default();

    for project in &config.projects {
        if project.collect.releases {
            match github.collect_project(project, &window, &discovered_at).await {
                Ok(candidates) => add_candidates(&mut report, "github_releases", candidates),
                Err(error) => report
                    .warnings
                    .push(format!("project '{}': {error:#}", project.id)),
            }
        }
    }

    if config.crates_io.enabled {
        match env::var(&config.crates_io.user_agent_env) {
            Ok(user_agent) if !user_agent.trim().is_empty() => {
                let crates_io = CratesIoCollector::new(&client, &config.crates_io, &user_agent);
                let mut crates = BTreeMap::<String, (Option<String>, String)>::new();
                for project in &config.projects {
                    if project.collect.crate_releases {
                        for crate_name in &project.crates {
                            crates.entry(crate_name.clone()).or_insert_with(|| {
                                (Some(project.id.clone()), project.category.clone())
                            });
                        }
                    }
                }

                for (crate_name, (project_id, category)) in crates {
                    match crates_io
                        .collect_crate(
                            &crate_name,
                            project_id.as_deref(),
                            &category,
                            &window,
                            &discovered_at,
                        )
                        .await
                    {
                        Ok(candidates) => add_candidates(&mut report, "crate_releases", candidates),
                        Err(error) => report
                            .warnings
                            .push(format!("crate '{crate_name}': {error:#}")),
                    }
                }
            }
            _ => report.warnings.push(format!(
                "crates.io collection skipped because environment variable '{}' is missing or empty",
                config.crates_io.user_agent_env
            )),
        }
    }

    for source in &config.feeds {
        match feed.collect_feed(source, &window, &discovered_at).await {
            Ok(candidates) => add_candidates(&mut report, "feeds", candidates),
            Err(error) => report
                .warnings
                .push(format!("feed '{}': {error:#}", source.id)),
        }
    }

    if config.security.enabled {
        let mut crates = BTreeMap::<String, (Option<String>, String)>::new();
        for project in &config.projects {
            if project.collect.security {
                for crate_name in &project.crates {
                    crates.entry(crate_name.clone()).or_insert_with(|| {
                        (Some(project.id.clone()), "security".to_owned())
                    });
                }
            }
        }

        for (crate_name, (project_id, category)) in crates {
            match rustsec
                .collect_crate(
                    &crate_name,
                    project_id.as_deref(),
                    &category,
                    &window,
                    &discovered_at,
                )
                .await
            {
                Ok(candidates) => add_candidates(&mut report, "rustsec", candidates),
                Err(error) => report
                    .warnings
                    .push(format!("crate '{crate_name}': {error:#}")),
            }
        }
    }

    Ok(report)
}

fn add_candidates(report: &mut CollectionReport, key: &str, mut candidates: Vec<Candidate>) {
    *report.counts.entry(key.to_owned()).or_default() += candidates.len();
    report.candidates.append(&mut candidates);
}
