//! Creating worktrees of a configured repo on a host, with the repo's worktree tool or plain git.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};

use crate::runner::Runner;
use crate::state::config::{Host, Repo, WorktreeTool};

const BRANCH_PLACEHOLDER: &str = "{branch}";
/// `sh -c` script: `$1` is the directory, the remaining arguments the command to run there.
const IN_DIR_SCRIPT: &str = r#"cd "$1" && shift && exec "$@""#;
/// Appended to a repo's path to name the directory plain-git worktrees go in by default.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// Creates a worktree of `repo` for the new branch `branch` on `host` and returns its path: with
/// the repo's worktree tool when it has one, else with `git worktree add`.
pub fn create(
    runner: &dyn Runner,
    host: &Host,
    repo: &Repo,
    branch: &str,
) -> anyhow::Result<PathBuf> {
    match &repo.worktree_tool {
        Some(tool) => create_with_tool(runner, host, &repo.path, tool, branch),
        None => create_with_git(runner, host, repo, branch),
    }
}

/// `git -C <repo> worktree add <dir>/<branch> -b <branch>`, where `<dir>` is the repo's
/// `worktrees_dir` or the sibling `<repo>-worktrees`.
fn create_with_git(
    runner: &dyn Runner,
    host: &Host,
    repo: &Repo,
    branch: &str,
) -> anyhow::Result<PathBuf> {
    let dir = repo.worktrees_dir.clone().unwrap_or_else(|| {
        let mut sibling = repo.path.clone().into_os_string();
        sibling.push(WORKTREES_SUFFIX);
        PathBuf::from(sibling)
    });
    let path = dir.join(branch);
    let argv: Vec<String> = [
        "git",
        "-C",
        &repo.path.display().to_string(),
        "worktree",
        "add",
        &path.display().to_string(),
        "-b",
        branch,
    ]
    .map(str::to_owned)
    .to_vec();
    let output = runner.run(host, &argv, None)?;
    if output.status != 0 {
        bail!(
            "`{}` on {} exited {}: {}",
            argv.join(" "),
            host.name,
            output.status,
            output.stderr.trim()
        );
    }
    Ok(path)
}

/// Runs `tool`'s create template for `branch` inside `repo_path` on `host` and returns the
/// worktree path, which the tool prints as its last non-empty stdout line.
fn create_with_tool(
    runner: &dyn Runner,
    host: &Host,
    repo_path: &Path,
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
    let output = runner.run(host, &wrapped_argv(repo_path, &argv), None)?;
    let command = format!("{} (in {})", argv.join(" "), repo_path.display());
    if output.status != 0 {
        bail!(
            "`{command}` on {} exited {}: {}",
            host.name,
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
        .ok_or_else(|| anyhow!("`{command}` on {} printed no worktree path", host.name))?;
    if !path.is_absolute() {
        bail!(
            "`{command}` on {} printed {}, not an absolute worktree path",
            host.name,
            path.display()
        );
    }
    Ok(path)
}

/// `tool` run with `dir` as its working directory. Worktree tools and `gh` act on the repository
/// they run in, and the runner has no cwd, so the directory change travels inside the command and
/// works the same locally and over ssh.
pub fn wrapped_argv(dir: &Path, tool: &[String]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        IN_DIR_SCRIPT.to_owned(),
        "sh".to_owned(),
        dir.display().to_string(),
    ];
    argv.extend_from_slice(tool);
    argv
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::runner::{FakeRunner, LocalSshRunner, Scripted};

    fn host() -> Host {
        Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    const REPO_PATH: &str = "/work/repo";

    fn repo(tool: Option<WorktreeTool>) -> Repo {
        Repo {
            name: "example-repo".to_owned(),
            path: PathBuf::from(REPO_PATH),
            default_branch: "main".to_owned(),
            worktrees_dir: None,
            worktree_tool: tool,
        }
    }

    fn with_tool() -> Repo {
        repo(Some(WorktreeTool {
            create: vec!["wt".to_owned(), "create".to_owned(), "{branch}".to_owned()],
            list: Vec::new(),
        }))
    }

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn wrapped_argv_changes_into_the_directory_first() {
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
    fn wrapped_argv_runs_the_tool_in_the_directory() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("it's a repo");
        std::fs::create_dir(&repo).unwrap();
        let control = tempfile::tempdir().unwrap();
        let runner =
            LocalSshRunner::new(Some("env-0001".to_owned()), control.path().to_owned()).unwrap();

        let output = runner
            .run(
                &host(),
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

        let path = create(&fake, &host(), &with_tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme-worktrees/feature-x"));
        assert_eq!(
            fake.calls()[0].argv,
            wrapped_argv(Path::new(REPO_PATH), &words(&["wt", "create", "feature-x"]))
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
                    create(&fake, &host(), &with_tool(), "feature-x").unwrap_err()
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

    #[test]
    fn worktree_create_falls_back_to_git() {
        let fake = FakeRunner::new(vec![
            Scripted {
                stderr: "Preparing worktree (new branch 'feature-x')\n".to_owned(),
                ..Scripted::default()
            },
            Scripted::default(),
            Scripted {
                status: 128,
                stderr: "fatal: a branch named 'feature-x' already exists\n".to_owned(),
                ..Scripted::default()
            },
        ]);
        let in_dir = Repo {
            worktrees_dir: Some(PathBuf::from("/work/trees")),
            ..repo(None)
        };

        let sibling = create(&fake, &host(), &repo(None), "feature-x").unwrap();
        let configured = create(&fake, &host(), &in_dir, "feature-x").unwrap();
        let err = create(&fake, &host(), &repo(None), "feature-x").unwrap_err();

        assert_eq!(sibling, Path::new("/work/repo-worktrees/feature-x"));
        assert_eq!(configured, Path::new("/work/trees/feature-x"));
        let calls = fake.calls();
        assert_eq!(
            calls[0].argv,
            words(&[
                "git",
                "-C",
                "/work/repo",
                "worktree",
                "add",
                "/work/repo-worktrees/feature-x",
                "-b",
                "feature-x"
            ])
        );
        assert_eq!(
            calls[1].argv,
            words(&[
                "git",
                "-C",
                "/work/repo",
                "worktree",
                "add",
                "/work/trees/feature-x",
                "-b",
                "feature-x"
            ])
        );
        let err = format!("{err:#}");
        assert!(err.contains("already exists"), "{err}");
    }
}
