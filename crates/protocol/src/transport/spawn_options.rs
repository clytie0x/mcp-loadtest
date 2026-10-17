//! Spawn-time options for stdio transport (stderr disposition). Consumed by
//! `StdioTransport::spawn_with` / `Session::spawn_with` (Agent B, Feature 2).
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How a spawned stdio server's stderr is handled.
#[derive(Debug, Clone, Default)]
pub enum StderrMode {
    /// Inherit the parent's stderr (historical default).
    #[default]
    Inherit,
    /// Capture to a file (no console passthrough).
    CaptureToFile(PathBuf),
    /// Capture to a file AND mirror lines to the parent's stderr live.
    TeeToFile(PathBuf),
}

/// Options for spawning a child MCP server over stdio.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SpawnOptions {
    /// Stderr disposition for the spawned child.
    pub stderr: StderrMode,
    /// Extra environment variables merged over the parent's environment.
    pub env: BTreeMap<String, String>,
    /// Working directory for the child. `None` inherits the parent's.
    pub working_dir: Option<PathBuf>,
}

impl SpawnOptions {
    /// Inherit the parent's stderr (default).
    #[must_use]
    pub fn inherit() -> Self {
        Self {
            stderr: StderrMode::Inherit,
            ..Self::default()
        }
    }
    /// Capture the child's stderr to `path`.
    #[must_use]
    pub fn capture_stderr(path: impl Into<PathBuf>) -> Self {
        Self {
            stderr: StderrMode::CaptureToFile(path.into()),
            ..Self::default()
        }
    }
    /// Capture the child's stderr to `path` and also mirror it to the parent's stderr.
    #[must_use]
    pub fn tee_stderr(path: impl Into<PathBuf>) -> Self {
        Self {
            stderr: StderrMode::TeeToFile(path.into()),
            ..Self::default()
        }
    }
    /// Merge `env` over the parent's environment for the spawned child.
    #[must_use]
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }
    /// Run the spawned child in `dir` instead of the parent's working directory.
    #[must_use]
    pub fn with_working_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.working_dir = dir;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_inherit() {
        assert!(matches!(
            SpawnOptions::default().stderr,
            StderrMode::Inherit
        ));
    }
    #[test]
    fn capture_sets_path() {
        let o = SpawnOptions::capture_stderr("a.log");
        assert!(matches!(o.stderr, StderrMode::CaptureToFile(p) if p.ends_with("a.log")));
    }
    #[test]
    fn tee_sets_path() {
        let o = SpawnOptions::tee_stderr("b.log");
        assert!(matches!(o.stderr, StderrMode::TeeToFile(p) if p.ends_with("b.log")));
    }
}
