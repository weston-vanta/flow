use crate::paths::StateDir;
use crate::state::Store;

/// Prints the task's `brief.md` verbatim.
pub fn run(state: &StateDir, id: &str) -> anyhow::Result<()> {
    super::print(&Store::open(state).brief(id)?)
}
