use std::collections::{BTreeMap, HashSet};

use crate::domain::Candidate;

pub fn normalize_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates
        .into_iter()
        .map(normalize_candidate)
        .collect()
}

pub fn deduplicate_exact(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen_ids = HashSet::new();
    let mut seen_urls = HashSet::new();
    let mut unique = Vec::new();

    for candidate in candidates {
        let normalized_url = normalize_url(&candidate.url);
        if seen_ids.contains(&candidate.id) || seen_urls.contains(&normalized_url) {
            continue;
        }
        seen_ids.insert(candidate.id.clone());
        seen_urls.insert(normalized_url);
        unique.push(candidate);
    }

    unique
}

fn normalize_candidate(mut candidate: Candidate) -> Candidate {
    candidate.title = collapse_whitespace(&candidate.title);
    candidate.url = normalize_url(&candidate.url);
    candidate.summary = candidate.summary.map(|value| collapse_whitespace(&value));
    candidate.raw_content = candidate
        .raw_content
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    candidate.metadata = candidate
        .metadata
        .into_iter()
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    candidate
}

fn normalize_url(value: &str) -> String {
    match url::Url::parse(value.trim()) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            let path = parsed.path().to_owned();
            if path.len() > 1 && path.ends_with('/') {
                parsed.set_path(path.trim_end_matches('/'));
            }
            parsed.to_string()
        }
        Err(_) => value.trim().to_owned(),
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::domain::{Candidate, CandidateKind};

    fn candidate(id: &str, url: &str) -> Candidate {
        Candidate {
            id: id.to_owned(),
            kind: CandidateKind::FeedArticle,
            title: "  A   title  ".to_owned(),
            url: url.to_owned(),
            source_id: "feed".to_owned(),
            project_id: None,
            category: "articles".to_owned(),
            published_at: Utc::now(),
            discovered_at: Utc::now(),
            summary: Some("  summary   text ".to_owned()),
            raw_content: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn normalizes_text_and_urls() {
        let result = normalize_candidates(vec![candidate(
            "1",
            "https://example.com/article/#fragment",
        )]);
        assert_eq!(result[0].title, "A title");
        assert_eq!(result[0].summary.as_deref(), Some("summary text"));
        assert_eq!(result[0].url, "https://example.com/article");
    }

    #[test]
    fn deduplicates_by_id_or_normalized_url() {
        let candidates = normalize_candidates(vec![
            candidate("same", "https://example.com/one"),
            candidate("same", "https://example.com/two"),
            candidate("other", "https://example.com/one/"),
        ]);
        let result = deduplicate_exact(candidates);
        assert_eq!(result.len(), 1);
    }
}
