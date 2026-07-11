//! Agent tools: each exposes a JSON `definition()` (name/description/input_schema) and
//! an async implementation invoked by the agent's dispatch loop.

pub mod duckduckgo;
pub mod feature_request;
pub mod remind;
pub mod summarize_url;
pub mod translate;
