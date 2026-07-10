use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{domain::Candidate, normalize::deduplicate_exact};

pub struct JsonlStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeResult {
    pub total: usize,
    pub added: usize,
}

impl JsonlStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<Vec<Candidate>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)
            .with_context(|| format!("failed to open {}", self.path.display()))?;
        let reader = BufReader::new(file);
        let mut candidates = Vec::new();

        for (index, line) in reader.lines().enumerate() {
            let line = line.with_context(|| {
                format!("failed reading line {} from {}", index + 1, self.path.display())
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let candidate = serde_json::from_str(&line).with_context(|| {
                format!("invalid JSON on line {} in {}", index + 1, self.path.display())
            })?;
            candidates.push(candidate);
        }

        Ok(candidates)
    }

    pub fn merge_and_save(&self, incoming: Vec<Candidate>) -> Result<MergeResult> {
        let existing = self.load()?;
        let existing_ids = existing
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<HashSet<_>>();

        let mut combined = existing;
        let mut positions = combined
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.id.clone(), index))
            .collect::<HashMap<_, _>>();

        for candidate in incoming {
            if let Some(index) = positions.get(&candidate.id).copied() {
                combined[index] = candidate;
            } else {
                positions.insert(candidate.id.clone(), combined.len());
                combined.push(candidate);
            }
        }

        let mut merged = deduplicate_exact(combined);
        merged.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let added = merged
            .iter()
            .filter(|candidate| !existing_ids.contains(&candidate.id))
            .count();
        let total = merged.len();
        self.write_atomic(&merged)?;

        Ok(MergeResult { total, added })
    }

    fn write_atomic(&self, candidates: &[Candidate]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let temp_path = temporary_path(&self.path);
        let file = File::create(&temp_path)
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        let mut writer = BufWriter::new(file);

        for candidate in candidates {
            serde_json::to_writer(&mut writer, candidate)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        drop(writer);

        fs::rename(&temp_path, &self.path).with_context(|| {
            format!(
                "failed to replace {} with {}",
                self.path.display(),
                temp_path.display()
            )
        })?;
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".tmp");
    PathBuf::from(value)
}
