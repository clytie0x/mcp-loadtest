//! Report renderers (Markdown / JSON / JUnit / Prometheus /
//! terminal). The `Report` data
//! model, the `Reporter` trait, and `format_iso8601_utc` live in
//! `mcp-loadtest-core` and are re-exported here for the renderers' internal
//! convenience.

pub mod json;
pub mod junit;
pub mod markdown;
pub mod prometheus;
pub mod terminal;
pub mod wire;

pub(crate) mod common;

pub use common::failure_reasons;

pub use mcp_loadtest_core::report::*;
