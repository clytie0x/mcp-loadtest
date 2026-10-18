//! Analysis toolkit (split across core, engine, and output).
//!
//! - `coverage` — pure data, in `mcp-loadtest-core`.
//! - `leak_detect` / `race_detector` — mid-run analyzers, in
//!   `mcp-loadtest-engine`.
//! - `regression` — post-run analyzer, in `mcp-loadtest-output`.
pub use mcp_loadtest_core::coverage;
pub use mcp_loadtest_engine::{leak_detect, race_detector};
pub use mcp_loadtest_output::regression;
