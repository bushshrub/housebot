//! High score tables, one per cabinet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const BOARD_SIZE: usize = 10;
pub const MAX_NAME_LEN: usize = 12;

/// The cabinets that keep score, with the highest total the game can produce.
/// A submission above its cabinet's ceiling did not come from playing.
const CABINETS: [(&str, u32); 3] = [("rush", 999_000), ("blocks", 99_999), ("snake", 9_999)];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    pub name: String,
    pub score: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Submission {
    pub cabinet: String,
    pub name: String,
    pub score: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    UnknownCabinet,
    EmptyName,
    ScoreTooHigh,
}

impl Rejected {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnknownCabinet => "no such cabinet",
            Self::EmptyName => "name must contain at least one letter or digit",
            Self::ScoreTooHigh => "score is beyond what that cabinet can produce",
        }
    }
}

pub fn ceiling(cabinet: &str) -> Option<u32> {
    CABINETS
        .iter()
        .find(|(name, _)| *name == cabinet)
        .map(|(_, ceiling)| *ceiling)
}

/// Strips a submitted name down to the arcade character set: uppercase ASCII
/// alphanumerics and single spaces, truncated to [`MAX_NAME_LEN`].
pub fn sanitize_name(raw: &str) -> String {
    let mut name = String::with_capacity(MAX_NAME_LEN);
    let mut pending_space = false;
    for ch in raw.chars() {
        if name.len() == MAX_NAME_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            if pending_space && !name.is_empty() {
                name.push(' ');
            }
            pending_space = false;
            name.push(ch.to_ascii_uppercase());
        } else {
            pending_space = true;
        }
    }
    name
}

#[derive(Debug)]
pub struct Board {
    path: PathBuf,
    cabinets: BTreeMap<String, Vec<Score>>,
}

impl Board {
    /// A missing or unreadable file is treated as an empty board, so a damaged
    /// score file can never stop the arcade from opening.
    pub async fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let cabinets = read_board(&path).await.unwrap_or_default();
        Self { path, cabinets }
    }

    pub fn all(&self) -> &BTreeMap<String, Vec<Score>> {
        &self.cabinets
    }

    pub fn cabinet(&self, cabinet: &str) -> &[Score] {
        self.cabinets.get(cabinet).map_or(&[], Vec::as_slice)
    }

    /// Validates a submission, inserts it in rank order and persists the board.
    /// A write failure is logged rather than propagated: the run still counts
    /// for the players in the room.
    pub async fn submit(&mut self, submission: Submission) -> Result<usize, Rejected> {
        let Some(ceiling) = ceiling(&submission.cabinet) else {
            return Err(Rejected::UnknownCabinet);
        };
        let name = sanitize_name(&submission.name);
        if name.is_empty() {
            return Err(Rejected::EmptyName);
        }
        if submission.score > ceiling {
            return Err(Rejected::ScoreTooHigh);
        }

        let entry = Score {
            name,
            score: submission.score,
        };
        let entries = self.cabinets.entry(submission.cabinet).or_default();
        let rank = entries
            .iter()
            .position(|existing| existing.score < entry.score)
            .unwrap_or(entries.len());
        entries.insert(rank, entry);
        entries.truncate(BOARD_SIZE);

        if let Err(error) = write_board(&self.path, &self.cabinets).await {
            tracing::warn!(%error, path = %self.path.display(), "failed to persist arcade scores");
        }
        Ok(rank + 1)
    }
}

type Cabinets = BTreeMap<String, Vec<Score>>;

async fn read_board(path: &Path) -> Option<Cabinets> {
    let raw = tokio::fs::read(path).await.ok()?;
    let mut cabinets: Cabinets = serde_json::from_slice(&raw).ok()?;
    for entries in cabinets.values_mut() {
        entries.sort_by(|a, b| b.score.cmp(&a.score));
        entries.truncate(BOARD_SIZE);
    }
    Some(cabinets)
}

async fn write_board(path: &Path, cabinets: &Cabinets) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    tokio::fs::write(path, serde_json::to_vec(cabinets)?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn submission(cabinet: &str, name: &str, score: u32) -> Submission {
        Submission {
            cabinet: cabinet.into(),
            name: name.into(),
            score,
        }
    }

    #[test]
    fn names_are_reduced_to_the_arcade_character_set() {
        assert_eq!(sanitize_name("  ada  lovelace  "), "ADA LOVELACE");
        assert_eq!(sanitize_name("<b>hi</b>"), "B HI B");
        assert_eq!(sanitize_name("!!!"), "");
        assert_eq!(sanitize_name("abcdefghijklmnop").len(), MAX_NAME_LEN);
    }

    #[tokio::test]
    async fn each_cabinet_keeps_its_own_top_ten() {
        let dir = TempDir::new().unwrap();
        let mut board = Board::load(dir.path().join("scores.json")).await;
        for i in 0..15u32 {
            board
                .submit(submission("snake", &format!("p{i}"), i * 10))
                .await
                .unwrap();
        }
        board
            .submit(submission("blocks", "ada", 500))
            .await
            .unwrap();

        assert_eq!(board.cabinet("snake").len(), BOARD_SIZE);
        assert_eq!(board.cabinet("snake")[0].score, 140);
        assert!(board
            .cabinet("snake")
            .windows(2)
            .all(|w| w[0].score >= w[1].score));
        assert_eq!(board.cabinet("blocks").len(), 1);
        assert!(board.cabinet("rush").is_empty());
    }

    #[tokio::test]
    async fn reports_the_rank_a_run_earned() {
        let dir = TempDir::new().unwrap();
        let mut board = Board::load(dir.path().join("scores.json")).await;
        assert_eq!(board.submit(submission("rush", "a", 100)).await, Ok(1));
        assert_eq!(board.submit(submission("rush", "b", 300)).await, Ok(1));
        assert_eq!(board.submit(submission("rush", "c", 200)).await, Ok(2));
    }

    #[tokio::test]
    async fn refuses_unknown_cabinets_and_impossible_scores() {
        let dir = TempDir::new().unwrap();
        let mut board = Board::load(dir.path().join("scores.json")).await;
        assert_eq!(
            board.submit(submission("nes", "ada", 10)).await,
            Err(Rejected::UnknownCabinet)
        );
        assert_eq!(
            board.submit(submission("snake", "ada", 10_000)).await,
            Err(Rejected::ScoreTooHigh)
        );
        assert_eq!(
            board.submit(submission("snake", "***", 10)).await,
            Err(Rejected::EmptyName)
        );
        assert!(board.all().is_empty());
    }

    #[tokio::test]
    async fn survives_a_reload_and_a_corrupt_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/scores.json");

        let mut board = Board::load(&path).await;
        board.submit(submission("blocks", "ada", 60)).await.unwrap();
        assert_eq!(Board::load(&path).await.all(), board.all());

        tokio::fs::write(&path, b"not json at all").await.unwrap();
        assert!(Board::load(&path).await.all().is_empty());
    }
}
