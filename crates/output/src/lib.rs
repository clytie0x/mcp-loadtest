//! Report renderers and regression policy for mcp-loadtest.
//!
//! Everything here is post-run presentation over the data model in
//! `mcp-loadtest-core`; this crate depends only on core.

pub mod regression;
pub mod report;
