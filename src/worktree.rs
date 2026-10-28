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

/// `git -C <repo> worktree add <dir>/<branch> -b <branch> <default branch>`, where `<dir>` is
/// the repo's `worktrees_dir` or the sibling `<repo>-worktrees`.
fn create_with_git(
    runner: &dyn Runner,
    host: &Host,
    repo: &Repo,
    branch: &str,
) -> anyhow::Result<PathBuf> {
    let dir = worktrees_dir(repo).ok_or_else(|| {
        anyhow!(
            "repo {} at {} has no sibling worktrees directory; set worktrees_dir in config.toml",
            repo.name,
            repo.path.display()
        )
    })?;
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
        &repo.default_branch,
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

/// Where `repo`'s worktrees live by config: its `worktrees_dir`, else the sibling
/// `<repo>-worktrees`; `None` when the repo's path has no parent to hold a sibling.
pub fn worktrees_dir(repo: &Repo) -> Option<PathBuf> {
    match &repo.worktrees_dir {
        Some(dir) => Some(dir.clone()),
        None => sibling_worktrees_dir(&repo.path),
    }
}

/// `<parent>/<name>-worktrees` for a repo at `<parent>/<name>`, ignoring a trailing slash.
fn sibling_worktrees_dir(repo_path: &Path) -> Option<PathBuf> {
    let mut name = repo_path.file_name()?.to_owned();
    name.push(WORKTREES_SUFFIX);
    Some(repo_path.parent()?.join(name))
}

/// Runs `tool`'s create template for `branch` inside `repo_path` on `host` and returns the
/// worktree path the tool prints, once it is a directory on `host`.
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
            failure_text(&output.stdout, &output.stderr)
        );
    }
    let (printed, path) = printed_path(&output.stdout)
        .map_err(|what| anyhow!("`{command}` on {} printed {what}", host.name))?;
    if !path.is_absolute() {
        bail!(
            "`{command}` on {} printed {printed:?}, not an absolute worktree path",
            host.name
        );
    }
    let test = vec![
        "test".to_owned(),
        "-d".to_owned(),
        path.display().to_string(),
    ];
    if runner.run(host, &test, None)?.status != 0 {
        bail!(
            "`{command}` on {} printed {printed:?}, but {} is not a directory on {}",
            host.name,
            path.display(),
            host.name
        );
    }
    Ok(path)
}

/// What a failed create command said: the `error` field of a JSON object on stdout, which is
/// how `wt --json` refuses, else stdout itself, followed by stderr.
fn failure_text(stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim();
    let reason = serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| stdout.to_owned());
    [reason.as_str(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The worktree path in a create command's `stdout`, with the text it came from: the `path`
/// field when `stdout` is a JSON object, else the first word of the last non-empty line, since
/// tools follow the path with notes such as `(new branch x, 1.2s)`. Fails with what was printed
/// instead.
fn printed_path(stdout: &str) -> Result<(String, PathBuf), String> {
    let text = stdout.trim();
    if text.starts_with('{') {
        let path = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| value.get("path")?.as_str().map(PathBuf::from));
        return match path {
            Some(path) => Ok((text.to_owned(), path)),
            None => Err(format!("{text:?}, which has no \"path\" string")),
        };
    }
    let word = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .and_then(|line| Some((line, line.split_whitespace().next()?)));
    match word {
        Some((line, word)) => Ok((line.to_owned(), PathBuf::from(word))),
        None => Err("no worktree path".to_owned()),
    }
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

    fn printed(stdout: &str) -> Scripted {
        Scripted {
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    #[test]
    fn create_fills_branch_and_takes_last_printed_line() {
        let fake = FakeRunner::new(vec![
            printed("Preparing worktree\n/workspaces/acme-worktrees/feature-x\n\n"),
            Scripted::default(),
        ]);

        let path = create(&fake, &host(), &with_tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme-worktrees/feature-x"));
        let calls = fake.calls();
        assert_eq!(
            calls[0].argv,
            wrapped_argv(Path::new(REPO_PATH), &words(&["wt", "create", "feature-x"]))
        );
        assert_eq!(
            calls[1].argv,
            words(&["test", "-d", "/workspaces/acme-worktrees/feature-x"])
        );
    }

    #[test]
    fn create_takes_the_path_before_the_tools_note() {
        let fake = FakeRunner::new(vec![
            printed("/workspaces/acme.wt/feature-x  (new branch feature-x, 109.6s)\n"),
            Scripted::default(),
        ]);

        let path = create(&fake, &host(), &with_tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme.wt/feature-x"));
        assert_eq!(
            fake.calls()[1].argv,
            words(&["test", "-d", "/workspaces/acme.wt/feature-x"])
        );
    }

    #[test]
    fn create_reads_the_path_from_json() {
        let json = r#"{
  "path": "/workspaces/acme.wt/feature x",
  "branch": "feature-x",
  "created_branch": true,
  "create_seconds": 1.2
}
"#;
        let fake = FakeRunner::new(vec![printed(json), Scripted::default()]);

        let path = create(&fake, &host(), &with_tool(), "feature-x").unwrap();

        assert_eq!(path, Path::new("/workspaces/acme.wt/feature x"));
    }

    #[test]
    fn create_quotes_json_without_a_path() {
        let json = r#"{"branch": "feature-x"}"#;
        let fake = FakeRunner::new(vec![printed(json)]);

        let err = format!(
            "{:#}",
            create(&fake, &host(), &with_tool(), "feature-x").unwrap_err()
        );

        assert!(err.contains(&format!("printed {json:?}")), "{err}");
        assert!(err.contains(r#"no "path""#), "{err}");
    }

    #[test]
    fn create_fails_when_the_printed_path_is_not_a_directory_on_the_host() {
        let line = "/workspaces/acme.wt/feature-x  (new branch feature-x, 2.0s)";
        let fake = FakeRunner::new(vec![
            printed(&format!("{line}\n")),
            Scripted {
                status: 1,
                ..Scripted::default()
            },
        ]);

        let err = format!(
            "{:#}",
            create(&fake, &host(), &with_tool(), "feature-x").unwrap_err()
        );

        assert!(err.contains(&format!("{line:?}")), "{err}");
        assert!(err.contains("not a directory on host-a"), "{err}");
    }

    #[test]
    fn create_failure_reports_what_the_tool_printed_on_stdout() {
        let fake = FakeRunner::new(vec![
            Scripted {
                status: 1,
                stdout: "{\"error\": \"branch already checked out\"}\n".to_owned(),
                ..Scripted::default()
            },
            Scripted {
                status: 2,
                stdout: "refusing: dirty base\n".to_owned(),
                stderr: "wt: see wt guide\n".to_owned(),
                ..Scripted::default()
            },
        ]);

        let errors: Vec<String> = (0..2)
            .map(|_| {
                format!(
                    "{:#}",
                    create(&fake, &host(), &with_tool(), "feature-x").unwrap_err()
                )
            })
            .collect();

        assert!(
            errors[0].contains("exited 1: branch already checked out"),
            "{}",
            errors[0]
        );
        assert!(
            errors[1].contains("refusing: dirty base") && errors[1].contains("wt: see wt guide"),
            "{}",
            errors[1]
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
        let slashed = Repo {
            path: PathBuf::from("/work/repo/"),
            ..repo(None)
        };
        let from_slashed = create(&fake, &host(), &slashed, "feature-x").unwrap();
        let configured = create(&fake, &host(), &in_dir, "feature-x").unwrap();
        assert!(
            create(
                &fake,
                &host(),
                &Repo {
                    path: PathBuf::from("/"),
                    ..repo(None)
                },
                "x"
            )
            .is_err(),
            "the root has no sibling worktrees directory"
        );
        let err = create(&fake, &host(), &repo(None), "feature-x").unwrap_err();

        assert_eq!(sibling, Path::new("/work/repo-worktrees/feature-x"));
        assert_eq!(from_slashed, Path::new("/work/repo-worktrees/feature-x"));
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
                "feature-x",
                "main"
            ])
        );
        assert_eq!(calls[1].argv[5], "/work/repo-worktrees/feature-x");
        assert_eq!(
            calls[2].argv,
            words(&[
                "git",
                "-C",
                "/work/repo",
                "worktree",
                "add",
                "/work/trees/feature-x",
                "-b",
                "feature-x",
                "main"
            ])
        );
        let err = format!("{err:#}");
        assert!(err.contains("already exists"), "{err}");
    }
}
