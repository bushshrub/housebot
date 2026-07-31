//! The cartridge shelf.
//!
//! Only `.nes` files sitting directly in the ROM directory are offered, and a
//! request may only name one of them — never a path.  No ROMs are shipped with
//! this crate beyond the built-in homebrew demo.

use std::path::{Path, PathBuf};

use crate::nes::demo;

pub const BUILT_IN: &str = "housebot-demo";
pub const MAX_ROM_BYTES: u64 = 4 * 1024 * 1024;

pub struct Shelf {
    directory: PathBuf,
}

impl Shelf {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The built-in demo first, then whatever is on disk, alphabetically.
    pub async fn titles(&self) -> Vec<String> {
        let mut titles = vec![BUILT_IN.to_string()];
        let Ok(mut entries) = tokio::fs::read_dir(&self.directory).await else {
            return titles;
        };
        let mut found = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_rom_name(&name) {
                found.push(name);
            }
        }
        found.sort();
        titles.extend(found);
        titles
    }

    pub async fn load(&self, title: &str) -> Result<Vec<u8>, String> {
        if title == BUILT_IN {
            return Ok(demo::rom());
        }
        if !is_rom_name(title) {
            return Err("not a cartridge name".into());
        }
        let path = self.directory.join(title);
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(|_| "no such cartridge".to_string())?
            .len();
        if size > MAX_ROM_BYTES {
            return Err("cartridge is too large".into());
        }
        tokio::fs::read(&path)
            .await
            .map_err(|_| "could not read cartridge".into())
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// A bare `*.nes` filename with no path components and no leading dot.
fn is_rom_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".nes")
        && !name.starts_with('.')
        && name.len() > 4
        && !name.contains(['/', '\\', '\0'])
        && name != BUILT_IN
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn only_plain_nes_filenames_are_accepted() {
        assert!(is_rom_name("Homebrew.nes"));
        assert!(is_rom_name("game.NES"));
        assert!(!is_rom_name(".nes"));
        assert!(!is_rom_name("../../etc/passwd"));
        assert!(!is_rom_name("sub/dir/game.nes"));
        assert!(!is_rom_name("..\\windows\\game.nes"));
        assert!(!is_rom_name("notes.txt"));
    }

    #[tokio::test]
    async fn the_demo_is_always_on_the_shelf() {
        let shelf = Shelf::new("/nonexistent-arcade-directory");
        assert_eq!(shelf.titles().await, vec![BUILT_IN]);
        assert!(!shelf.load(BUILT_IN).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lists_and_loads_cartridges_from_disk() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("beta.nes"), b"NES\x1Adata")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("alpha.nes"), b"NES\x1Amore")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("readme.txt"), b"ignore me")
            .await
            .unwrap();

        let shelf = Shelf::new(dir.path());
        assert_eq!(
            shelf.titles().await,
            vec![BUILT_IN.to_string(), "alpha.nes".into(), "beta.nes".into()]
        );
        assert_eq!(shelf.load("alpha.nes").await.unwrap(), b"NES\x1Amore");
    }

    #[tokio::test]
    async fn refuses_to_walk_out_of_the_rom_directory() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("secret.nes"), b"NES\x1A")
            .await
            .unwrap();
        let shelf = Shelf::new(dir.path().join("roms"));
        assert!(shelf.load("../secret.nes").await.is_err());
        assert!(shelf.load("/etc/passwd").await.is_err());
    }
}
