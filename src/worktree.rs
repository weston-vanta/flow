//! Creating worktrees on a CDE with the operator's worktree tool.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};

use crate::runner::Runner;
use crate::state::config::{Cde, WorktreeTool};

const BRANCH_PLACEHOLDER: &str = "{branch}";
/// `sh -c` script: `$1` is the directory, the remaining arguments the command to run there.
const IN_DIR_SCRIPT: &str = r#"cd "$1" && shift && exec "$@""#;

/// Runs `tool`'s create template for `branch` inside `monorepo` on `cde` and returns the
/// worktree path, which the tool prints as its last non-empty stdout line.
pub fn create(
    runner: &dyn Runner,
    cde: &Cde,
    monorepo: &Path,
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
    let output = runner.run(cde, &wrapped_argv(monorepo, &argv), None)?;
    let command = format!("{} (in {})", argv.join(" "), monorepo.display());
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

/// `tool` run with `monorepo` as its working directory. The worktree tool acts on the repository
/// it runs in, and the runner has no cwd, so the directory change travels inside the command and
/// works the same locally and over ssh.
pub fn wrapped_argv(monorepo: &Path, tool: &[String]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        IN_DIR_SCRIPT.to_owned(),
        "sh".to_owned(),
        monorepo.display().to_string(),
    ];
    argv.extend_from_slice(tool);
    argv
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::runner::{FakeRunner, LocalSshRunner, Scripted};

    fn cde() -> Cde {
        Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    const MONOREPO: &str = "/workspaces/acme";

    fn tool() -> WorktreeTool {
        WorktreeTool {
            create: vec!["wt".to_owned(), "create".to_owned(), "{branch}".to_owned()],
            list: Vec::new(),
        }
    }

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn wrapped_argv_changes_into_the_monorepo_first() {
        assert_eq!(
            wrapped_argv(
                Path::new("/workspaces/acme"),
                &words(&["wt", "create", "feature x"])
            ),
            [
                "sh",
                "-c",
                r#"cd "$1" && shift && exec "$@""#,
                "sh",
                "/workspaces/acme",
                "wt",
                "create",
                "feature x",
            ]
        );
    }

    #[test]
    fn wrapped_argv_runs_the_tool_in_the_monorepo() {
        let monorepo = tempfile::tempdir().unwrap();
        let repo = monorepo.path().join("it's a repo");
        std::fs::create_dir(&repo).unwrap();
        let control = tempfile::tempdir().unwrap();
        let runner =
            LocalSshRunner::new(Some("env-0001".to_owned()), control.path().to_owned()).unwrap();

        let output = runner
            .run(
                &cde(),
                &wrapped_argv(&repo, &words(&["sh", "-c", "pwd"])),
                None,
            )
            .unwrap();

        assert_eq!(output.status, 0, "{output:?}");
        assert_eq!(
            Path::new(output.stdout.trim_end()).canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn create_fills_branch_and_takes_last_printed_line() {
        let fake = FakeRunner::new(vec![Scripted {
            stdout: "Preparing worktree\n/workspaces/acme-worktrees/feature-x\n\n".to_owned(),
            ..Scripted::default()
        }]);

        let path = create(&fake, &cde(), Path::new(MONOREPO), &tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme-worktrees/feature-x"));
        assert_eq!(
            fake.calls()[0].argv,
            wrapped_argv(Path::new(MONOREPO), &words(&["wt", "create", "feature-x"]))
        );
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
                    create(&fake, &cde(), Path::new(MONOREPO), &tool(), "feature-x").unwrap_err()
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
