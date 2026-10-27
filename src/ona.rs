//! Ona environments, listed through the `gitpod` CLI on this machine.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::runner;

const LIST_ARGV: [&str; 5] = ["gitpod", "environment", "list", "-o", "json"];
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Names a JSON file `list_environments` reads in place of running the Ona CLI.
const FAKE_ONA_VAR: &str = "FLOW_FAKE_ONA";
const RUNNING_PHASE: &str = "ENVIRONMENT_PHASE_RUNNING";
/// The name of a host whose Ona name has no letters or digits.
const FALLBACK_SLUG: &str = "host";

/// One Ona environment as `gitpod environment list -o json` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnaEnvironment {
    pub id: String,
    /// The free-text name the user gave the environment.
    pub name: String,
    /// Ona's phase name, such as `ENVIRONMENT_PHASE_RUNNING`.
    pub phase: String,
    /// Where the environment's repository is checked out, when Ona reports it.
    pub content_location: Option<PathBuf>,
}

impl OnaEnvironment {
    pub fn running(&self) -> bool {
        is_running(&self.phase)
    }
}

/// Whether `phase` is Ona's running phase.
pub fn is_running(phase: &str) -> bool {
    phase == RUNNING_PHASE
}

/// The ssh alias `gitpod environment ssh-config` writes for the environment `env_id`.
pub fn ssh_alias(env_id: &str) -> String {
    format!("{env_id}.ona.environment")
}

/// The fields of one listed environment flow reads; the listing has many more.
#[derive(Debug, Deserialize)]
struct Listed {
    id: String,
    #[serde(default)]
    metadata: Metadata,
    #[serde(default)]
    status: ListedStatus,
}

#[derive(Debug, Default, Deserialize)]
struct Metadata {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ListedStatus {
    #[serde(default)]
    phase: String,
    #[serde(default)]
    content: Content,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Content {
    #[serde(default)]
    content_location_in_machine: Option<PathBuf>,
}

/// The environments the Ona CLI's login can see, from `gitpod environment list -o json` run
/// on this machine; killed after 30 seconds. With `$FLOW_FAKE_ONA` set, the listing is read
/// from the file it names instead, and under the fake runner it must be set.
pub fn list_environments() -> anyhow::Result<Vec<OnaEnvironment>> {
    let text = match std::env::var_os(FAKE_ONA_VAR) {
        Some(path) if path.is_empty() => bail!("${FAKE_ONA_VAR} is empty"),
        Some(path) => {
            let path = Path::new(&path);
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None if runner::fake_runner_active() => {
            bail!("${FAKE_ONA_VAR} is unset under the fake runner; not running the Ona CLI")
        }
        None => run_list()?,
    };
    parse(&text)
}

/// Parses the JSON array `gitpod environment list -o json` prints.
pub fn parse(text: &str) -> anyhow::Result<Vec<OnaEnvironment>> {
    let listed: Vec<Listed> =
        serde_json::from_str(text).context("parsing the Ona environment list")?;
    Ok(listed
        .into_iter()
        .map(|env| OnaEnvironment {
            id: env.id,
            name: env.metadata.name,
            phase: env.status.phase,
            content_location: env.status.content.content_location_in_machine,
        })
        .collect())
}

/// `name` lowercased, each run of characters other than letters and digits replaced by one
/// `-`, with no `-` at either end; `host` when nothing is left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        FALLBACK_SLUG.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Runs the Ona CLI's listing and returns its stdout, failing on a non-zero exit or after
/// `LIST_TIMEOUT`.
fn run_list() -> anyhow::Result<String> {
    let command = LIST_ARGV.join(" ");
    let (program, args) = LIST_ARGV
        .split_first()
        .expect("the listing names a program");
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + LIST_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("waiting for `{command}`"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            // The child may have exited since the last check; either way it is reaped.
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "`{command}` did not finish within {}s",
                LIST_TIMEOUT.as_secs()
            );
        }
        thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout.join().unwrap_or_default();
    if !status.success() {
        let stderr = stderr.join().unwrap_or_default();
        bail!("`{command}` failed ({status}): {}", stderr.trim());
    }
    Ok(stdout)
}

/// Reads `pipe` to the end on its own thread, so a child filling one pipe cannot block on it.
fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = include_str!("../tests/fixtures/ona/environment_list.json");

    #[test]
    fn ona_listing_parses_fixture() {
        let environments = parse(LISTING).unwrap();

        assert_eq!(
            environments,
            [
                OnaEnvironment {
                    id: "00000000-0000-4000-8000-000000000001".to_owned(),
                    name: "Lab".to_owned(),
                    phase: "ENVIRONMENT_PHASE_RUNNING".to_owned(),
                    content_location: Some(PathBuf::from("/workspaces/example-repo")),
                },
                OnaEnvironment {
                    id: "00000000-0000-4000-8000-000000000002".to_owned(),
                    name: "Side Project".to_owned(),
                    phase: "ENVIRONMENT_PHASE_RUNNING".to_owned(),
                    content_location: None,
                },
                OnaEnvironment {
                    id: "00000000-0000-4000-8000-000000000003".to_owned(),
                    name: "Scratch Box".to_owned(),
                    phase: "ENVIRONMENT_PHASE_STOPPED".to_owned(),
                    content_location: Some(PathBuf::from("/workspaces/example-repo")),
                },
            ]
        );
        assert!(environments[0].running());
        assert!(!environments[2].running());
        assert!(parse("not json").is_err());
        assert_eq!(parse("[]").unwrap(), []);
    }

    #[test]
    fn slug_normalizes_names() {
        assert_eq!(slug("Lab"), "lab");
        assert_eq!(slug("Side Project"), "side-project");
        assert_eq!(slug("  Scratch   Box!! "), "scratch-box");
        assert_eq!(slug("Feature: login/form"), "feature-login-form");
        assert_eq!(slug("v2.0__beta"), "v2-0-beta");
        assert_eq!(slug("---"), "host");
        assert_eq!(slug(""), "host");
    }
}
