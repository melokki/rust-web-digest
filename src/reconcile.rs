use std::collections::{BTreeMap, HashMap};

use chrono::Duration;
use semver::Version;

use crate::{
    config::ReconciliationConfig,
    domain::{Candidate, CandidateKind, Story},
};

pub fn reconcile_candidates(
    candidates: &[Candidate],
    config: &ReconciliationConfig,
) -> Vec<Story> {
    let mut stories = Vec::<Story>::new();
    let mut release_story_by_key = HashMap::<(String, String), usize>::new();
    let mut deferred_articles = Vec::<Candidate>::new();

    let mut ordered = candidates.to_vec();
    ordered.sort_by(|left, right| {
        left.published_at
            .cmp(&right.published_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    for candidate in ordered {
        match candidate.kind {
            CandidateKind::GitHubRelease | CandidateKind::CrateRelease => {
                if let Some((project_id, version)) = release_key(&candidate) {
                    let key = (project_id, version.clone());
                    if let Some(index) = release_story_by_key.get(&key).copied() {
                        attach_candidate(&mut stories[index], candidate);
                    } else {
                        let story = release_story(candidate, version);
                        let index = stories.len();
                        release_story_by_key.insert(key, index);
                        stories.push(story);
                    }
                } else {
                    stories.push(single_candidate_story(candidate));
                }
            }
            CandidateKind::FeedArticle => deferred_articles.push(candidate),
            CandidateKind::SecurityAdvisory => stories.push(single_candidate_story(candidate)),
        }
    }

    for article in deferred_articles {
        if let Some(index) = matching_release_story(&article, &stories, config) {
            attach_candidate(&mut stories[index], article);
        } else {
            stories.push(single_candidate_story(article));
        }
    }

    for story in &mut stories {
        sort_story_candidates(story);
        refresh_story_projection(story);
    }

    stories.sort_by(|left, right| {
        right
            .published_at
            .cmp(&left.published_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    stories
}

fn release_key(candidate: &Candidate) -> Option<(String, String)> {
    let project_id = candidate.project_id.clone()?;
    let version = candidate_version(candidate)?;
    Some((project_id, version))
}

fn release_story(candidate: Candidate, version: String) -> Story {
    let project_id = candidate
        .project_id
        .clone()
        .expect("release_key guarantees a project id");
    Story {
        id: format!("release:{project_id}:{version}"),
        project_id: Some(project_id),
        category: candidate.category.clone(),
        title: candidate.title.clone(),
        version: Some(version),
        published_at: candidate.published_at.clone(),
        discovered_at: candidate.discovered_at.clone(),
        candidates: vec![candidate],
    }
}

fn single_candidate_story(candidate: Candidate) -> Story {
    let prefix = match candidate.kind {
        CandidateKind::SecurityAdvisory => "security",
        CandidateKind::FeedArticle => "article",
        CandidateKind::GitHubRelease | CandidateKind::CrateRelease => "candidate",
    };
    Story {
        id: format!("{prefix}:{}", candidate.id),
        project_id: candidate.project_id.clone(),
        category: candidate.category.clone(),
        title: candidate.title.clone(),
        version: candidate_version(&candidate),
        published_at: candidate.published_at.clone(),
        discovered_at: candidate.discovered_at.clone(),
        candidates: vec![candidate],
    }
}

fn matching_release_story(
    article: &Candidate,
    stories: &[Story],
    config: &ReconciliationConfig,
) -> Option<usize> {
    let project_id = article.project_id.as_deref()?;
    let article_text = candidate_text(article);
    let article_version = candidate_version(article).or_else(|| extract_semver(&article_text));
    let window = Duration::days(config.article_window_days as i64);

    let matches = stories
        .iter()
        .enumerate()
        .filter(|(_, story)| story.project_id.as_deref() == Some(project_id))
        .filter(|(_, story)| story.version.is_some())
        .filter(|(_, story)| {
            let distance = article
                .published_at
                .signed_duration_since(story.published_at.clone())
                .abs();
            distance <= window
        })
        .filter(|(_, story)| {
            let explicit_url_reference = story
                .candidates
                .iter()
                .any(|candidate| article_text.contains(&candidate.url));
            let version_reference = article_version
                .as_deref()
                .zip(story.version.as_deref())
                .is_some_and(|(article_version, story_version)| article_version == story_version);
            explicit_url_reference || version_reference
        })
        .map(|(index, story)| {
            let distance = article
                .published_at
                .signed_duration_since(story.published_at.clone())
                .abs();
            (index, distance)
        })
        .collect::<Vec<_>>();

    matches
        .into_iter()
        .min_by_key(|(_, distance)| *distance)
        .map(|(index, _)| index)
}

fn attach_candidate(story: &mut Story, candidate: Candidate) {
    if story
        .candidates
        .iter()
        .any(|existing| existing.id == candidate.id)
    {
        return;
    }
    story.candidates.push(candidate);
    sort_story_candidates(story);
    refresh_story_projection(story);
}

fn sort_story_candidates(story: &mut Story) {
    story.candidates.sort_by(|left, right| {
        source_priority(left.kind)
            .cmp(&source_priority(right.kind))
            .then_with(|| left.published_at.cmp(&right.published_at))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn refresh_story_projection(story: &mut Story) {
    if let Some(primary) = story.candidates.first() {
        story.category = primary.category.clone();
        if let Some(github_release) = story
            .candidates
            .iter()
            .find(|candidate| candidate.kind == CandidateKind::GitHubRelease)
        {
            story.title = github_release.title.clone();
        } else if let (Some(project_id), Some(version)) =
            (story.project_id.as_deref(), story.version.as_deref())
        {
            story.title = format!("{project_id} {version} published");
        } else {
            story.title = primary.title.clone();
        }
    }
    if let Some(first_published) = story.candidates.iter().map(|item| item.published_at.clone()).min() {
        story.published_at = first_published;
    }
    if let Some(last_discovered) = story
        .candidates
        .iter()
        .map(|item| item.discovered_at.clone())
        .max()
    {
        story.discovered_at = last_discovered;
    }
}

fn source_priority(kind: CandidateKind) -> u8 {
    match kind {
        CandidateKind::SecurityAdvisory => 0,
        CandidateKind::GitHubRelease => 1,
        CandidateKind::CrateRelease => 2,
        CandidateKind::FeedArticle => 3,
    }
}

pub fn candidate_version(candidate: &Candidate) -> Option<String> {
    candidate
        .metadata
        .get("version")
        .and_then(|value| normalize_semver(value))
        .or_else(|| {
            candidate
                .metadata
                .get("tag_name")
                .and_then(|value| extract_semver(value))
        })
        .or_else(|| extract_semver(&candidate.title))
}

pub fn extract_semver(value: &str) -> Option<String> {
    for (start, ch) in value.char_indices() {
        if !ch.is_ascii_digit() {
            continue;
        }

        let token = value[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
            .collect::<String>();
        let token = token.trim_end_matches(|ch: char| matches!(ch, '.' | '-' | '+'));
        if let Some(version) = normalize_semver(token) {
            return Some(version);
        }
    }
    None
}

fn normalize_semver(value: &str) -> Option<String> {
    Version::parse(value.trim().trim_start_matches('v'))
        .ok()
        .map(|version| version.to_string())
}

fn candidate_text(candidate: &Candidate) -> String {
    let mut parts = vec![candidate.title.as_str(), candidate.url.as_str()];
    if let Some(summary) = candidate.summary.as_deref() {
        parts.push(summary);
    }
    if let Some(content) = candidate.raw_content.as_deref() {
        parts.push(content);
    }
    parts.join("\n")
}

pub fn story_candidate_map(stories: &[Story]) -> BTreeMap<String, Vec<String>> {
    stories
        .iter()
        .map(|story| {
            (
                story.id.clone(),
                story
                    .candidates
                    .iter()
                    .map(|candidate| candidate.id.clone())
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use super::*;

    fn candidate(
        id: &str,
        kind: CandidateKind,
        project: &str,
        title: &str,
        day: u32,
        metadata: BTreeMap<String, String>,
    ) -> Candidate {
        Candidate {
            id: id.to_owned(),
            kind,
            title: title.to_owned(),
            url: format!("https://example.com/{id}"),
            source_id: "test".to_owned(),
            project_id: Some(project.to_owned()),
            category: "frameworks".to_owned(),
            published_at: Utc.with_ymd_and_hms(2026, 7, day, 12, 0, 0).unwrap(),
            discovered_at: Utc.with_ymd_and_hms(2026, 7, day, 13, 0, 0).unwrap(),
            summary: None,
            raw_content: None,
            metadata,
        }
    }

    #[test]
    fn extracts_prefixed_semver() {
        assert_eq!(extract_semver("axum-v1.2.3"), Some("1.2.3".to_owned()));
        assert_eq!(extract_semver("Release 0.8.0-rc.1!"), Some("0.8.0-rc.1".to_owned()));
    }

    #[test]
    fn merges_release_and_crate_publication() {
        let release = candidate(
            "release",
            CandidateKind::GitHubRelease,
            "axum",
            "Axum 1.2.3",
            10,
            BTreeMap::from([("tag_name".to_owned(), "axum-v1.2.3".to_owned())]),
        );
        let crate_release = candidate(
            "crate",
            CandidateKind::CrateRelease,
            "axum",
            "axum 1.2.3 published",
            10,
            BTreeMap::from([("version".to_owned(), "1.2.3".to_owned())]),
        );

        let stories = reconcile_candidates(
            &[release, crate_release],
            &ReconciliationConfig::default(),
        );
        assert_eq!(stories.len(), 1);
        assert_eq!(stories[0].id, "release:axum:1.2.3");
        assert_eq!(stories[0].candidates.len(), 2);
    }
}
