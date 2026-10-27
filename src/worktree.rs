//! Creating worktrees on a CDE with the operator's worktree tool.

use std::path::PathBuf;

use anyhow::{anyhow, bail};

use crate::runner::Runner;
use crate::state::config::{Cde, WorktreeTool};

const BRANCH_PLACEHOLDER: &str = "{branch}";

/// Runs `tool`'s create template for `branch` on `cde` and returns the worktree path, which the
/// tool prints as its last non-empty stdout line.
pub fn create(
    runner: &dyn Runner,
    cde: &Cde,
    tool: &WorktreeTool,
    branch: &str,
) -> anyhow::Result<PathBuf> {
    let argv: Vec<String> = tool
        .create
        .iter()
        .map(|word| word.replace(BRANCH_PLACEHOLDER, branch))
        .collect();
    if argv.is_empty() {
        bail!("worktree_tool.create in config.toml is empty");
    }
    let output = runner.run(cde, &argv, None)?;
    let command = argv.join(" ");
    if output.status != 0 {
        bail!(
            "`{command}` on {} exited {}: {}",
            cde.name,
            output.status,
            output.stderr.trim()
        );
    }
    let path = output
        .stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("`{command}` on {} printed no worktree path", cde.name))?;
    if !path.is_absolute() {
        bail!(
            "`{command}` on {} printed {}, not an absolute worktree path",
            cde.name,
            path.display()
        );
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::runner::{FakeRunner, Scripted};

    fn cde() -> Cde {
        Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn tool() -> WorktreeTool {
        WorktreeTool {
            create: vec!["wt".to_owned(), "create".to_owned(), "{branch}".to_owned()],
        }
    }

    #[test]
    fn create_fills_branch_and_takes_last_printed_line() {
        let fake = FakeRunner::new(vec![Scripted {
            stdout: "Preparing worktree\n/workspaces/acme-worktrees/feature-x\n\n".to_owned(),
            ..Scripted::default()
        }]);

        let path = create(&fake, &cde(), &tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme-worktrees/feature-x"));
        assert_eq!(fake.calls()[0].argv, ["wt", "create", "feature-x"]);
    }

    #[test]
    fn create_fails_on_nonzero_exit_or_no_path() {
        let fake = FakeRunner::new(vec![
            Scripted {
                status: 1,
                stderr: "branch already checked out\n".to_owned(),
                ..Scripted::default()
            },
            Scripted {
                stdout: "\n".to_owned(),
                ..Scripted::default()
            },
            Scripted {
                stdout: "feature-x\n".to_owned(),
                ..Scripted::default()
            },
        ]);

        let errors: Vec<String> = (0..3)
            .map(|_| {
                format!(
                    "{:#}",
                    create(&fake, &cde(), &tool(), "feature-x").unwrap_err()
                )
            })
            .collect();

        assert!(
            errors[0].contains("branch already checked out"),
            "{}",
            errors[0]
        );
        assert!(
            errors[1].contains("printed no worktree path"),
            "{}",
            errors[1]
        );
        assert!(
            errors[2].contains("not an absolute worktree path"),
            "{}",
            errors[2]
        );
    }
}
