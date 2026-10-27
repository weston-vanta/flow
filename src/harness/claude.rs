//! Claude Code workers. Hooks are installed through a per-task `--settings` file, never the
//! user's own settings.

use std::path::Path;

use anyhow::{Context, bail};

use super::{HarnessAdapter, Launch};
use crate::runner::shell_quote;
use crate::state::fs::write_atomic;
use crate::state::ids::is_task_id;
use crate::state::task::TaskRecord;

const SETTINGS_TEMPLATE: &str = include_str!("../../templates/claude-worker-settings.json");
const SETTINGS_FILE: &str = "claude-settings.json";
const BRIEF_FILE: &str = "brief.md";

/// Launches Claude Code workers whose `SessionStart` and `Stop` hooks call `flow hook claude`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeAdapter;

impl HarnessAdapter for ClaudeAdapter {
    /// Writes `<task_dir>/claude-settings.json` and returns
    /// `claude --settings <that file> --name <id> "Read <task_dir>/brief.md and follow it exactly."`.
    fn prepare(
        &self,
        task: &TaskRecord,
        task_dir: &Path,
        flow_bin: &Path,
    ) -> anyhow::Result<Launch> {
        // The id goes unquoted into the hook's shell command.
        if !is_task_id(&task.id) {
            bail!("invalid task id: {:?}", task.id);
        }
        let flow_bin = utf8(flow_bin)?;
        let settings_path = task_dir.join(SETTINGS_FILE);
        let brief_path = task_dir.join(BRIEF_FILE);
        let hook_command = format!("{} hook claude --task {}", shell_quote(flow_bin), task.id);
        let settings = SETTINGS_TEMPLATE.replace(
            "\"{{hook_command}}\"",
            &serde_json::to_string(&hook_command)?,
        );
        write_atomic(&settings_path, settings.as_bytes())
            .with_context(|| format!("writing {}", settings_path.display()))?;
        Ok(Launch {
            argv: vec![
                "claude".to_owned(),
                "--settings".to_owned(),
                utf8(&settings_path)?.to_owned(),
                "--name".to_owned(),
                task.id.clone(),
                format!("Read {} and follow it exactly.", utf8(&brief_path)?),
            ],
            env: Vec::new(),
        })
    }

    fn kind(&self) -> &'static str {
        "claude"
    }
}

fn utf8(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::*;
    use crate::state::task::Harness;

    const FLOW_BIN: &str = "/opt/flow/bin/flow";

    fn task() -> TaskRecord {
        TaskRecord {
            id: "aaaaaaab".to_owned(),
            parent: None,
            title: "Add retry to the example client".to_owned(),
            cde: "cde-a".to_owned(),
            worktree: PathBuf::from("/workspaces/example-repo.wt/retry"),
            harness: Harness::Claude,
            created: "2026-10-26T12:00:00Z".parse().unwrap(),
            owner: "supervisor".to_owned(),
        }
    }

    #[test]
    fn prepare_writes_settings_with_two_hooks() {
        let dir = tempfile::tempdir().unwrap();
        ClaudeAdapter
            .prepare(&task(), dir.path(), Path::new(FLOW_BIN))
            .unwrap();

        let bytes = std::fs::read(dir.path().join("claude-settings.json")).unwrap();
        let settings: Value = serde_json::from_slice(&bytes).unwrap();
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(
            hooks.keys().collect::<Vec<_>>(),
            ["SessionStart", "Stop"],
            "{settings:#}"
        );
        for event in ["SessionStart", "Stop"] {
            let hook = &settings["hooks"][event][0]["hooks"][0];
            assert_eq!(hook["type"], "command", "{settings:#}");
            assert_eq!(hook["timeout"], 30, "{settings:#}");
            let command = hook["command"].as_str().unwrap();
            assert_eq!(command, "'/opt/flow/bin/flow' hook claude --task aaaaaaab");
            assert!(command.ends_with("--task aaaaaaab"), "{command}");
        }
    }

    #[test]
    fn prepare_quotes_flow_bin_for_the_hook_shell() {
        let dir = tempfile::tempdir().unwrap();
        ClaudeAdapter
            .prepare(&task(), dir.path(), Path::new("/opt/it's flow/flow"))
            .unwrap();

        let bytes = std::fs::read(dir.path().join("claude-settings.json")).unwrap();
        let settings: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            r"'/opt/it'\''s flow/flow' hook claude --task aaaaaaab"
        );
    }

    #[test]
    fn launch_argv_starts_with_claude_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let launch = ClaudeAdapter
            .prepare(&task(), dir.path(), Path::new(FLOW_BIN))
            .unwrap();
        let settings = dir.path().join("claude-settings.json");
        let prompt = format!(
            "Read {} and follow it exactly.",
            dir.path().join("brief.md").display()
        );

        assert_eq!(
            launch.argv,
            [
                "claude",
                "--settings",
                settings.to_str().unwrap(),
                "--name",
                "aaaaaaab",
                &prompt,
            ]
        );
        assert!(launch.env.is_empty());
        assert_eq!(ClaudeAdapter.kind(), "claude");
    }
}
