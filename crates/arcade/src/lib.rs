//! # housebot-arcade
//!
//! An arcade cabinet simulator: a first-person WebGL arena game served by a
//! Rust backend that also owns the high score table.
//!
//! ## Crate boundary
//!
//! - The browser owns rendering and input; it holds no authority over scores.
//! - The backend owns the score table.  Submissions are re-checked against the
//!   game's own scoring rules ([`scores::max_plausible_score`]) before they are
//!   allowed onto the board, so a hand-crafted POST cannot mint a top score.
//!
//! ## No-bloat rule
//!
//! There is no web framework, no bundler and no client-side engine: the HTTP
//! layer is a few hundred lines over `tokio`, and the game is raw WebGL 2.

pub mod assets;
pub mod http;
pub mod nes;
pub mod roms;
pub mod scores;
pub mod server;

pub use nes::Nes;
pub use roms::Shelf;
pub use scores::{Board, Rejected, Score, Submission};
pub use server::serve;
