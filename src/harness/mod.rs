//! How a worker harness is launched with a brief, and the hooks it reports back through.

pub mod brief;
pub mod claude;
pub mod handshake;

use std::path::Path;

use crate::state::task::TaskRecord;

pub use claude::ClaudeAdapter;

/// The command that starts a worker: `argv[0]` is the harness binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub trait HarnessAdapter {
    /// Writes whatever files the harness needs into `task_dir` and returns the command that
    /// starts a worker on the task's `brief.md`. Hooks call `flow_bin`, which must be a path
    /// valid on every host.
    fn prepare(
        &self,
        task: &TaskRecord,
        task_dir: &Path,
        flow_bin: &Path,
    ) -> anyhow::Result<Launch>;

    fn kind(&self) -> &'static str;
}
