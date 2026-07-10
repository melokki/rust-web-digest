use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    GitHubRelease,
    CrateRelease,
    FeedArticle,
    SecurityAdvisory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub kind: CandidateKind,
    pub title: String,
    pub url: String,
    pub source_id: String,
    pub project_id: Option<String>,
    pub category: String,
    pub published_at: DateTime<Utc>,
    pub discovered_at: DateTime<Utc>,
    pub summary: Option<String>,
    pub raw_content: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    pub id: String,
    pub project_id: Option<String>,
    pub category: String,
    pub title: String,
    pub version: Option<String>,
    pub published_at: DateTime<Utc>,
    pub discovered_at: DateTime<Utc>,
    pub candidates: Vec<Candidate>,
}

impl Story {
    pub fn candidate_ids(&self) -> impl Iterator<Item = &str> {
        self.candidates.iter().map(|candidate| candidate.id.as_str())
    }

    pub fn has_candidate_discovered_since(&self, since: &DateTime<Utc>) -> bool {
        self.candidates
            .iter()
            .any(|candidate| &candidate.discovered_at >= since)
    }
}
