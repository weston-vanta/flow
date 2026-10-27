//! Runs commands on a host: directly when the host is this machine, over ssh otherwise. Every
//! remote operation goes through a `Runner`.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::host;
use crate::state::config::Host;

/// Names a JSON script file; when set, `from_env` returns a `FakeRunner` reading it.
const FAKE_RUNNER_VAR: &str = "FLOW_FAKE_RUNNER";
const CALLS_LOG_SUFFIX: &str = ".calls.jsonl";

/// A finished command. Output that is not UTF-8 is converted lossily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or 128 + the signal number when a signal killed the process.
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait Runner: Send + Sync {
    /// Runs `argv` on `host` to completion, feeding it `stdin` when given.
    fn run(&self, host: &Host, argv: &[String], stdin: Option<&[u8]>) -> anyhow::Result<Output>;

    /// Starts `argv` on `host` with stdout piped and stdin and stderr closed, so a long-lived
    /// child never blocks on a pipe nobody drains.
    fn spawn(&self, host: &Host, argv: &[String]) -> anyhow::Result<Child>;
}

/// The runner for this process: a `FakeRunner` when `$FLOW_FAKE_RUNNER` names a script,
/// else a `LocalSshRunner` keeping its ssh control sockets in `default_control_dir()`.
pub fn from_env() -> anyhow::Result<Box<dyn Runner>> {
    if let Some(script) = std::env::var_os(FAKE_RUNNER_VAR) {
        return Ok(Box::new(FakeRunner::from_file(Path::new(&script))?));
    }
    Ok(Box::new(LocalSshRunner::new(
        host::local_env_id(),
        default_control_dir(),
    )?))
}

/// Where ssh control sockets live: `$XDG_RUNTIME_DIR/flow`, else `/tmp/flow-<uid>`.
pub fn default_control_dir() -> PathBuf {
    let uid = fs::metadata("/proc/self").map(|meta| meta.uid()).ok();
    control_dir_for(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), uid)
}

/// The control dir rule. It stays on local disk because Unix sockets cannot be bound on NFS
/// and their paths are capped at 108 bytes.
fn control_dir_for(xdg_runtime_dir: Option<&OsStr>, uid: Option<u32>) -> PathBuf {
    match (xdg_runtime_dir.filter(|dir| !dir.is_empty()), uid) {
        (Some(dir), _) => Path::new(dir).join("flow"),
        (None, Some(uid)) => PathBuf::from(format!("/tmp/flow-{uid}")),
        (None, None) => PathBuf::from("/tmp/flow"),
    }
}

/// Whether `$FLOW_FAKE_RUNNER` is set, so `from_env` returns a `FakeRunner`. Commands that
/// would replace this process with an interactive one print what they would run instead.
pub fn fake_runner_active() -> bool {
    std::env::var_os(FAKE_RUNNER_VAR).is_some()
}

/// Creates `control_dir` if missing and sets it to mode 0700.
pub fn ensure_control_dir(control_dir: &Path) -> anyhow::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(control_dir)
        .and_then(|()| fs::set_permissions(control_dir, Permissions::from_mode(0o700)))
        .with_context(|| format!("creating {}", control_dir.display()))
}

/// Runs argv directly on the host whose env id is `local_env_id`, and over ssh on the others.
#[derive(Debug)]
pub struct LocalSshRunner {
    local_env_id: Option<String>,
    control_dir: PathBuf,
}

impl LocalSshRunner {
    /// `control_dir` holds the ssh multiplexing sockets; it is created if missing and set to
    /// mode 0700.
    pub fn new(local_env_id: Option<String>, control_dir: PathBuf) -> anyhow::Result<Self> {
        ensure_control_dir(&control_dir)?;
        Ok(LocalSshRunner {
            local_env_id,
            control_dir,
        })
    }

    fn command(&self, host: &Host, argv: &[String]) -> anyhow::Result<Command> {
        let full = build_argv(host, self.local_env_id.as_deref(), &self.control_dir, argv);
        let (program, args) = full
            .split_first()
            .ok_or_else(|| anyhow!("empty command for host {}", host.name))?;
        let mut command = Command::new(program);
        command.args(args);
        Ok(command)
    }
}

impl Runner for LocalSshRunner {
    fn run(&self, host: &Host, argv: &[String], stdin: Option<&[u8]>) -> anyhow::Result<Output> {
        let mut command = self.command(host, argv)?;
        command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {argv:?} on {}", host.name))?;
        let output = std::thread::scope(|scope| {
            if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
                // Written from its own thread so a child that fills stdout before reading all
                // of stdin cannot deadlock against us. A child that exits without reading gets
                // a broken pipe, which is its business, not an error here.
                scope.spawn(move || {
                    let _ = pipe.write_all(input);
                });
            }
            child.wait_with_output()
        })
        .with_context(|| format!("running {argv:?} on {}", host.name))?;
        Ok(Output {
            status: exit_code(output.status),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn spawn(&self, host: &Host, argv: &[String]) -> anyhow::Result<Child> {
        self.command(host, argv)?
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting {argv:?} on {}", host.name))
    }
}

/// The argv that runs `argv` on `host`: `argv` itself when `host` is local, else an ssh call
/// multiplexed through a control socket in `control_dir`. The remote side runs the command
/// under `zsh -lc`, which reads `.zshenv` and `.zprofile` (not `.zshrc`), so it gets the
/// PATH those files set, `~/.local/bin` included.
pub fn build_argv(
    host: &Host,
    local_env_id: Option<&str>,
    control_dir: &Path,
    argv: &[String],
) -> Vec<String> {
    if host::is_local(host, local_env_id) {
        return argv.to_vec();
    }
    let mut ssh = vec![
        "ssh".to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
    ];
    ssh.extend(ssh_connection_options(control_dir));
    ssh.push(host.ssh_alias.clone());
    ssh.push(remote_command(argv));
    ssh
}

/// The ssh options every connection to a host uses: a short connect timeout and multiplexing
/// through a control socket in `control_dir`.
pub fn ssh_connection_options(control_dir: &Path) -> Vec<String> {
    let control_path = control_dir.join("ssh-%C");
    vec![
        "-o".to_owned(),
        // A stopped host then fails within seconds instead of hanging on the TCP connect.
        "ConnectTimeout=10".to_owned(),
        "-o".to_owned(),
        "ControlMaster=auto".to_owned(),
        "-o".to_owned(),
        format!("ControlPath={}", control_path.display()),
        "-o".to_owned(),
        "ControlPersist=600".to_owned(),
    ]
}

/// The remote command argument that makes ssh run `argv` under `zsh -lc` on the host.
pub fn remote_command(argv: &[String]) -> String {
    // ssh joins its remote arguments into one string for the remote login shell to parse, so
    // the command line zsh receives must be quoted a second time.
    format!("zsh -lc {}", shell_quote(&quote_argv(argv)))
}

/// `word` as one POSIX shell word: single-quoted, with each `'` written as `'\''`.
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `argv` as one shell command line that parses back to the same words.
fn quote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

/// One scripted result for `FakeRunner`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scripted {
    /// Only calls to the host with this name use the entry; absent matches every host.
    pub host: Option<String>,
    /// Only calls whose argv holds these words in a row use the entry; absent matches every
    /// call.
    pub argv: Option<Vec<String>>,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    /// A repeating entry answers every matching call instead of being used up.
    pub repeat: bool,
}

/// One call a `FakeRunner` received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Call {
    pub host: String,
    pub argv: Vec<String>,
}

/// Test runner: answers each call with the first unused scripted entry matching the host and
/// records every call, in memory and optionally as JSON lines in a log file.
#[derive(Debug)]
pub struct FakeRunner {
    script: Mutex<Vec<(Scripted, bool)>>,
    calls: Mutex<Vec<Call>>,
    calls_log: Option<PathBuf>,
}

impl FakeRunner {
    pub fn new(script: Vec<Scripted>) -> FakeRunner {
        FakeRunner {
            script: Mutex::new(script.into_iter().map(|entry| (entry, false)).collect()),
            calls: Mutex::new(Vec::new()),
            calls_log: None,
        }
    }

    /// Reads the script from the JSON list in `path` and logs calls to `<path>.calls.jsonl`.
    pub fn from_file(path: &Path) -> anyhow::Result<FakeRunner> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let script: Vec<Scripted> =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut log = OsString::from(path.as_os_str());
        log.push(CALLS_LOG_SUFFIX);
        Ok(FakeRunner {
            calls_log: Some(PathBuf::from(log)),
            ..FakeRunner::new(script)
        })
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls lock").clone()
    }

    /// Records the call and returns the scripted result for it.
    fn answer(&self, host: &Host, argv: &[String]) -> anyhow::Result<Scripted> {
        let call = Call {
            host: host.name.clone(),
            argv: argv.to_vec(),
        };
        if let Some(log) = &self.calls_log {
            let mut line = serde_json::to_vec(&call)?;
            line.push(b'\n');
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .and_then(|mut file| file.write_all(&line))
                .with_context(|| format!("appending to {}", log.display()))?;
        }
        self.calls.lock().expect("calls lock").push(call);

        let mut script = self.script.lock().expect("script lock");
        let Some((entry, used)) = script.iter_mut().find(|(entry, used)| {
            !*used
                && entry.host.as_deref().is_none_or(|name| name == host.name)
                && entry.argv.as_deref().is_none_or(|words| {
                    words.is_empty() || argv.windows(words.len()).any(|window| window == words)
                })
        }) else {
            bail!(
                "fake runner has no scripted output for {argv:?} on {}",
                host.name
            );
        };
        *used = !entry.repeat;
        Ok(entry.clone())
    }
}

impl Runner for FakeRunner {
    fn run(&self, host: &Host, argv: &[String], _stdin: Option<&[u8]>) -> anyhow::Result<Output> {
        let entry = self.answer(host, argv)?;
        Ok(Output {
            status: entry.status,
            stdout: entry.stdout,
            stderr: entry.stderr,
        })
    }

    fn spawn(&self, host: &Host, argv: &[String]) -> anyhow::Result<Child> {
        let entry = self.answer(host, argv)?;
        Command::new("sh")
            .args(["-c", r#"printf %s "$1"; exit "$2""#, "sh"])
            .arg(&entry.stdout)
            .arg(entry.status.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting the fake runner's child")
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn host(name: &str, env_id: &str) -> Host {
        Host {
            name: name.to_owned(),
            env_id: env_id.to_owned(),
            ssh_alias: format!("{name}.example"),
            local_marker_env: None,
        }
    }

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    /// Runs `command_line` with `sh -c` and splits its stdout on NUL bytes.
    fn sh_words(command_line: &str) -> Vec<String> {
        let out = Command::new("sh")
            .arg("-c")
            .arg(command_line)
            .output()
            .unwrap();
        assert!(out.status.success(), "sh failed: {out:?}");
        let mut words: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split('\0')
            .map(str::to_owned)
            .collect();
        assert_eq!(words.pop().as_deref(), Some(""), "output ends with NUL");
        words
    }

    #[test]
    fn local_host_runs_directly() {
        let argv = words(&["echo", "hi"]);
        let built = build_argv(
            &host("host-a", "env-0001"),
            Some("env-0001"),
            Path::new("/run/user/1000/flow"),
            &argv,
        );
        assert_eq!(built, argv);
    }

    #[test]
    fn remote_host_wraps_in_ssh_zsh_lc() {
        let built = build_argv(
            &host("host-b", "env-0002"),
            Some("env-0001"),
            Path::new("/run/user/1000/flow"),
            &words(&["echo", "a b"]),
        );
        assert_eq!(
            built,
            words(&[
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/run/user/1000/flow/ssh-%C",
                "-o",
                "ControlPersist=600",
                "host-b.example",
                r#"zsh -lc ''\''echo'\'' '\''a b'\'''"#,
            ])
        );
        assert!(built.last().unwrap().starts_with("zsh -lc "));
    }

    #[test]
    fn default_control_dir_prefers_xdg_runtime_dir() {
        let xdg = Some(OsStr::new("/run/user/1000"));
        assert_eq!(
            control_dir_for(xdg, Some(1000)),
            Path::new("/run/user/1000/flow")
        );
        assert_eq!(
            control_dir_for(None, Some(1000)),
            Path::new("/tmp/flow-1000")
        );
        assert_eq!(
            control_dir_for(Some(OsStr::new("")), Some(1000)),
            Path::new("/tmp/flow-1000")
        );
    }

    #[test]
    fn local_ssh_runner_creates_private_control_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("flow");
        LocalSshRunner::new(None, dir.clone()).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn unknown_local_env_id_treats_every_host_as_remote() {
        let built = build_argv(
            &host("host-a", "env-0001"),
            None,
            Path::new("/run/user/1000/flow"),
            &words(&["true"]),
        );
        assert_eq!(built[0], "ssh");
    }

    #[test]
    fn shell_quoting_survives_spaces_and_quotes() {
        let tokens = words(&["echo", "a b", "it's", "", "$HOME", "x\ny"]);
        let quoted: Vec<String> = tokens.iter().map(|t| shell_quote(t)).collect();
        let printed = sh_words(&format!("printf '%s\\0' {}", quoted.join(" ")));
        assert_eq!(printed, tokens);
    }

    #[test]
    fn remote_command_survives_two_shell_parses() {
        // ssh hands the remote login shell one string, which parses it and runs `zsh -lc` on
        // the inner command line; `sh -c` stands in for both shells here.
        let argv = words(&["printf", "%s\\0", "a b", "it's"]);
        let outer = format!("sh -c {}", shell_quote(&quote_argv(&argv)));
        assert_eq!(sh_words(&outer), words(&["a b", "it's"]));
    }

    fn scripted(host: Option<&str>, stdout: &str, repeat: bool) -> Scripted {
        Scripted {
            host: host.map(str::to_owned),
            stdout: stdout.to_owned(),
            repeat,
            ..Scripted::default()
        }
    }

    fn run_stdout(fake: &FakeRunner, target: &Host) -> anyhow::Result<String> {
        fake.run(target, &words(&["true"]), None)
            .map(|out| out.stdout)
    }

    #[test]
    fn fake_answers_with_first_unused_matching_entry() {
        let fake = FakeRunner::new(vec![
            scripted(Some("host-b"), "b1", false),
            scripted(None, "any", false),
            scripted(Some("host-b"), "b2", false),
        ]);
        let (a, b) = (host("host-a", "env-0001"), host("host-b", "env-0002"));
        assert_eq!(run_stdout(&fake, &a).unwrap(), "any");
        assert_eq!(run_stdout(&fake, &b).unwrap(), "b1");
        assert_eq!(run_stdout(&fake, &b).unwrap(), "b2");
        let err = run_stdout(&fake, &a).unwrap_err().to_string();
        assert!(err.contains("host-a") && err.contains("\"true\""), "{err}");
    }

    #[test]
    fn fake_entry_with_argv_answers_only_calls_holding_those_words() {
        let fake = FakeRunner::new(vec![
            Scripted {
                argv: Some(words(&["agent", "read"])),
                stdout: "agent".to_owned(),
                repeat: true,
                ..Scripted::default()
            },
            Scripted {
                stdout: "other".to_owned(),
                ..Scripted::default()
            },
        ]);
        let a = host("host-a", "env-0001");
        let run = |argv: &[&str]| fake.run(&a, &words(argv), None).unwrap().stdout;

        assert_eq!(run(&["herdr", "pane", "read", "w1:p1"]), "other");
        assert_eq!(run(&["herdr", "agent", "read", "w1:p1"]), "agent");
        assert_eq!(run(&["herdr", "agent", "read", "w1:p1"]), "agent");
    }

    #[test]
    fn fake_repeat_entry_is_never_used_up() {
        let fake = FakeRunner::new(vec![scripted(Some("host-b"), "down", true)]);
        let b = host("host-b", "env-0002");
        for _ in 0..3 {
            assert_eq!(run_stdout(&fake, &b).unwrap(), "down");
        }
        assert_eq!(fake.calls().len(), 3);
    }

    #[test]
    fn fake_spawn_emits_scripted_output_and_status() {
        let fake = FakeRunner::new(vec![Scripted {
            status: 3,
            stdout: "it's out".to_owned(),
            ..Scripted::default()
        }]);
        let argv = words(&["herdr", "wait"]);
        let out = fake
            .spawn(&host("host-a", "env-0001"), &argv)
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"it's out");
        assert_eq!(
            fake.calls(),
            [Call {
                host: "host-a".to_owned(),
                argv
            }]
        );
    }
}
