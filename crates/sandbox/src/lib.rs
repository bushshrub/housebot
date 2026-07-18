//! # housebot-sandbox
//!
//! A safe sandbox crate for basic code inspection and execution.
//!
//! ## Crate boundary
//!
//! - **Housebot** (the main crate) uses the client API (`SandboxClient`, `Sandbox`).
//! - **sandboxd** (the daemon binary from this crate) owns Docker access.
//!
//! This split means Housebot never touches the Docker socket.
//!
//! ## Security principle
//!
//! The sandbox crate constructs all Docker arguments.  User input is validated
//! and passed as command arguments inside the container, never interpolated
//! into Docker flags.

pub mod client;
pub mod docker;
pub mod limits;
pub mod protocol;
pub mod server;
pub mod validation;

pub use client::{Sandbox, SandboxClient};
pub use protocol::NetworkAccess;
