//! Re-export of the report layer (data model in core, renderers in output).
pub use mcp_loadtest_core::report::*;
pub use mcp_loadtest_output::report::{
    failure_reasons, json, junit, markdown, prometheus, terminal, wire,
};
