//! Fixed limits enforced by the sandbox crate.
//!
//! These are compile-time constants.  They are never derived from user input.

pub const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 30;
pub const TEST_TIMEOUT_SECS: u64 = 120;
pub const ABSOLUTE_MAX_TIMEOUT_SECS: u64 = 300;

pub const MAX_OUTPUT_BYTES: usize = 64 * 1024; // combined stdout+stderr
pub const MAX_FILE_READ_BYTES: usize = 64 * 1024;
pub const MAX_SEARCH_MATCHES: usize = 100;
pub const MAX_FILE_LIST_ENTRIES: usize = 500;

pub const MAX_FILE_LINES: usize = 2000;
pub const MAX_FILE_READ_LINES: usize = 2000;

pub const MAX_CLONE_BRANCH_LENGTH: usize = 256;
pub const MAX_URL_LENGTH: usize = 2048;
pub const MAX_COMMAND_LENGTH: usize = 4096;
pub const MAX_PATH_DEPTH: usize = 64;
pub const MAX_GLOB_LENGTH: usize = 256;
pub const MAX_QUERY_LENGTH: usize = 512;
