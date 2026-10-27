//! Bringing up Claude Code after typing its command line into a herdr pane: waiting for it to
//! start and accepting the folder-trust dialog it shows in a directory it has not seen.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::herdr::{AgentStatus, Herdr, KEY_DOWN, KEY_ENTER, Waited};
use crate::state::config::Host;

/// How long the whole start may take, trust dialog included, counted from the first wait.
const START_TIMEOUT_MS: u64 = 60_000;
/// Overrides how long to wait between asks while herdr does not see the agent yet, in
/// milliseconds.
const POLL_VAR: &str = "FLOW_HANDSHAKE_POLL_MS";
const DEFAULT_POLL: Duration = Duration::from_secs(1);
/// How long herdr may see no agent before the start says what it is waiting for.
const NOTICE_AFTER: Duration = Duration::from_secs(3);
/// Enough of the pane to hold the whole trust dialog, or Claude's last words before exiting.
pub const PANE_LINES: u32 = 40;
/// The trust dialog lists two options, so reaching `Yes` never takes more moves than this.
const MAX_CURSOR_MOVES: usize = 3;

#[derive(Debug)]
pub enum AgentStart {
    /// herdr sees the agent idle or working.
    Running,
    /// The agent stopped on a prompt other than the trust dialog, showing `pane_text`; no keys
    /// were sent.
    BlockedAtStart { pane_text: String },
}

/// Waits for the agent just launched in `pane_id` to come up. herdr does not see an agent in
/// the pane until Claude Code has started there, which can take many seconds, so until then
/// this asks again. Claude shows a folder-trust dialog in a directory it has not seen, and
/// herdr reports it `blocked`; keys sent before then are lost. When the blocked pane shows that
/// dialog, this selects its `Yes` option and waits again. While herdr sees no agent, a pane
/// whose text `stop_early` matches ends the wait with a `StoppedEarly` error.
pub fn await_start(
    herdr: &Herdr,
    host: &Host,
    workspace_id: &str,
    pane_id: &str,
    stop_early: Option<StopEarly>,
) -> anyhow::Result<AgentStart> {
    await_start_with(
        herdr,
        host,
        workspace_id,
        pane_id,
        stop_early,
        &Timing::from_env()?,
    )
}

/// Reads the pane's text while herdr sees no agent there and says whether the agent will not
/// come up, so the start can stop before its timeout.
pub type StopEarly = fn(&str) -> bool;

/// The start stopped before its timeout because the pane showed `pane_text`, which the
/// caller's `StopEarly` matched.
#[derive(Debug, thiserror::Error)]
#[error(
    "agent in pane {pane_id} on {host} will not start, judging by its pane; workspace \
     {workspace_id} is left open for inspection"
)]
pub struct StoppedEarly {
    pub pane_id: String,
    pub host: String,
    pub workspace_id: String,
    pub pane_text: String,
}

#[derive(Debug, Clone, Copy)]
struct Timing {
    timeout: Duration,
    poll: Duration,
}

impl Timing {
    fn from_env() -> anyhow::Result<Timing> {
        let poll = match std::env::var(POLL_VAR) {
            Ok(ms) => Duration::from_millis(
                ms.parse()
                    .with_context(|| format!("parsing ${POLL_VAR} {ms:?} as milliseconds"))?,
            ),
            Err(_) => DEFAULT_POLL,
        };
        Ok(Timing {
            timeout: Duration::from_millis(START_TIMEOUT_MS),
            poll,
        })
    }
}

fn await_start_with(
    herdr: &Herdr,
    host: &Host,
    workspace_id: &str,
    pane_id: &str,
    stop_early: Option<StopEarly>,
    timing: &Timing,
) -> anyhow::Result<AgentStart> {
    let mut clock = Clock::default();
    let mut wait = |until: &[AgentStatus]| -> anyhow::Result<AgentStatus> {
        match wait_for_agent(herdr, host, pane_id, until, stop_early, timing, &mut clock)? {
            Polled::Reached(status) => Ok(status),
            Polled::Stopped(pane_text) => Err(StoppedEarly {
                pane_id: pane_id.to_owned(),
                host: host.name.clone(),
                workspace_id: workspace_id.to_owned(),
                pane_text,
            }
            .into()),
            Polled::TimedOut => {
                let silent = if clock.no_answer_noted {
                    "; herdr gave no answer to at least one wait"
                } else {
                    ""
                };
                bail!(
                    "agent in pane {pane_id} on {} did not start within {} s{silent}; \
                     workspace {workspace_id} is left open for inspection",
                    host.name,
                    timing.timeout.as_secs()
                )
            }
        }
    };
    let status = wait(&[
        AgentStatus::Blocked,
        AgentStatus::Idle,
        AgentStatus::Working,
    ])?;
    if status != AgentStatus::Blocked {
        return Ok(AgentStart::Running);
    }
    let text = herdr.read(host, pane_id, PANE_LINES)?;
    let Some(keys) = trust_dialog_keys(&text) else {
        return Ok(AgentStart::BlockedAtStart { pane_text: text });
    };
    herdr.pane_send_keys(host, pane_id, &keys)?;
    wait(&[AgentStatus::Idle, AgentStatus::Working])?;
    Ok(AgentStart::Running)
}

/// One start's progress across its waits: when the first began, whether the wait for herdr
/// to see the agent has been announced, and whether a wait without an answer has been.
#[derive(Default)]
struct Clock {
    started: Option<Instant>,
    announced: bool,
    no_answer_noted: bool,
}

/// How `wait_for_agent` ended.
enum Polled {
    Reached(AgentStatus),
    /// `StopEarly` matched this pane text.
    Stopped(String),
    TimedOut,
}

/// The status the agent in `pane_id` reaches among `until` before `timing.timeout` has passed
/// since the first wait, which starts `clock`. While herdr sees no agent there this checks the
/// pane against `stop_early` and asks again every `timing.poll`, saying once on stderr what it
/// waits for when that takes a while. A wait that exits without a word is asked again the same
/// way, which is said once on stderr too.
fn wait_for_agent(
    herdr: &Herdr,
    host: &Host,
    pane_id: &str,
    until: &[AgentStatus],
    stop_early: Option<StopEarly>,
    timing: &Timing,
    clock: &mut Clock,
) -> anyhow::Result<Polled> {
    let started = *clock.started.get_or_insert_with(Instant::now);
    let mut left = timing.timeout.saturating_sub(started.elapsed());
    loop {
        if left.is_zero() {
            return Ok(Polled::TimedOut);
        }
        // Rounded up, so the first wait passes the whole timeout; herdr's `--timeout 0` would
        // mean no timeout at all.
        let timeout_ms = u64::try_from(left.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX);
        match herdr.wait_for_agent(host, pane_id, until, Some(timeout_ms))? {
            Waited::Reached(status) => return Ok(Polled::Reached(status)),
            Waited::TimedOut => return Ok(Polled::TimedOut),
            Waited::NoAgent => {}
            Waited::NoAnswer => {
                if !clock.no_answer_noted {
                    eprintln!(
                        "herdr gave no answer for pane {pane_id} on {}; retrying",
                        host.name
                    );
                    clock.no_answer_noted = true;
                }
            }
        }
        if !clock.announced && started.elapsed() >= NOTICE_AFTER {
            eprintln!(
                "waiting for Claude Code in pane {pane_id} on {}…",
                host.name
            );
            clock.announced = true;
        }
        // A pane that cannot be read is no reason to stop.
        if let Some(stop_early) = stop_early
            && let Ok(text) = herdr.pane_read(host, pane_id, PANE_LINES)
            && stop_early(&text)
        {
            return Ok(Polled::Stopped(text));
        }
        thread::sleep(
            timing
                .poll
                .min(timing.timeout.saturating_sub(started.elapsed())),
        );
        left = timing.timeout.saturating_sub(started.elapsed());
    }
}

/// Whether `text`, read from a pane where `claude --resume` did not come up, shows that Claude
/// Code could not find the conversation to resume: a line saying "no conversation found", or
/// one saying "not found" about a resume or a session.
pub fn session_missing(text: &str) -> bool {
    text.lines().map(str::to_lowercase).any(|line| {
        line.contains("no conversation found")
            || (line.contains("not found") && (line.contains("resume") || line.contains("session")))
    })
}

/// The keys that accept Claude's folder-trust dialog shown in `text`: enough `down`s to move
/// the cursor (`❯` or `>` starting a line) onto the option containing `Yes`, then `enter`.
/// `None` when `text` is not that dialog or the cursor cannot reach `Yes` by moving down.
pub fn trust_dialog_keys(text: &str) -> Option<Vec<&'static str>> {
    if !text.to_lowercase().contains("trust") {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // The pane holds earlier output too, so the dialog's cursor is the last one on screen.
    let cursor = lines.iter().rposition(|line| is_cursor_line(line))?;
    let yes = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("Yes"))
        .min_by_key(|(i, _)| i.abs_diff(cursor))?
        .0;
    if yes < cursor {
        return None;
    }
    let moves = lines[cursor + 1..=yes]
        .iter()
        .filter(|line| !line.is_empty())
        .count();
    if moves > MAX_CURSOR_MOVES {
        return None;
    }
    let mut keys = vec![KEY_DOWN; moves];
    keys.push(KEY_ENTER);
    Some(keys)
}

/// A selection cursor: `❯` or `>` alone or followed by a space, so a `>_ claude` prompt line
/// does not count.
fn is_cursor_line(line: &str) -> bool {
    ["❯", ">"].iter().any(|marker| {
        line.strip_prefix(marker)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::runner::{FakeRunner, Scripted};

    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w3:p1 not found"},"id":"cli:agent:wait"}"#;
    const AGENT_GET: &str = include_str!("../../tests/fixtures/herdr/agent_get.json");
    const PANE: &str = "w3:p1";

    fn host() -> Host {
        Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn ok(stdout: &str) -> Scripted {
        Scripted {
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    fn not_found(repeat: bool) -> Scripted {
        Scripted {
            status: 1,
            stderr: NOT_FOUND.to_owned(),
            repeat,
            ..Scripted::default()
        }
    }

    fn wait_reports(status: &str) -> Scripted {
        assert!(AGENT_GET.contains(r#""agent_status":"working""#));
        ok(&AGENT_GET.replace(
            r#""agent_status":"working""#,
            &format!(r#""agent_status":"{status}""#),
        ))
    }

    fn fast() -> Timing {
        Timing {
            timeout: Duration::from_millis(300),
            poll: Duration::from_millis(5),
        }
    }

    /// The `--timeout` value of every `agent wait` call, in order.
    fn wait_timeouts(fake: &FakeRunner) -> Vec<u64> {
        fake.calls()
            .iter()
            .filter(|call| call.argv.get(3..5) == Some(&["agent".to_owned(), "wait".to_owned()]))
            .map(|call| {
                let at = call
                    .argv
                    .iter()
                    .position(|word| word == "--timeout")
                    .unwrap();
                call.argv[at + 1].parse().unwrap()
            })
            .collect()
    }

    #[test]
    fn await_start_waits_for_herdr_to_see_the_agent() {
        let fake = Arc::new(FakeRunner::new(vec![
            not_found(false),
            wait_reports("blocked"),
            ok(DIALOG_ON_NO),
            ok(""),
            wait_reports("idle"),
        ]));
        let herdr = Herdr::new(fake.clone());

        let start = await_start_with(&herdr, &host(), "w3", PANE, None, &fast()).unwrap();

        assert!(matches!(start, AgentStart::Running), "{start:?}");
        let calls = fake.calls();
        let keys: Vec<_> = calls
            .iter()
            .filter(|call| {
                call.argv.get(3..5) == Some(&["pane".to_owned(), "send-keys".to_owned()])
            })
            .collect();
        assert_eq!(keys.len(), 1, "{calls:?}");
        assert_eq!(keys[0].argv[5..], [PANE, KEY_DOWN, KEY_ENTER]);
        let timeouts = wait_timeouts(&fake);
        assert_eq!(timeouts.len(), 3, "{calls:?}");
        assert_eq!(timeouts[0], 300, "the first attempt gets the whole timeout");
        assert!(
            timeouts[1] < 300 && timeouts[2] <= timeouts[1] && timeouts[2] > 0,
            "later waits get what is left: {timeouts:?}"
        );
    }

    #[test]
    fn await_start_gives_up_when_the_agent_never_appears() {
        let fake = Arc::new(FakeRunner::new(vec![not_found(true)]));
        let herdr = Herdr::new(fake.clone());
        let timing = fast();

        let started = std::time::Instant::now();
        let err = await_start_with(&herdr, &host(), "w3", PANE, None, &timing).unwrap_err();

        let err = format!("{err:#}");
        assert!(err.contains("did not start within"), "{err}");
        assert!(err.contains("workspace w3 is left open"), "{err}");
        assert!(
            started.elapsed() >= timing.timeout,
            "{:?}",
            started.elapsed()
        );
        assert!(wait_timeouts(&fake).len() > 1, "it retried");
    }

    #[test]
    fn await_start_stops_early_when_the_pane_shows_the_agent_will_not_start() {
        let fake = Arc::new(FakeRunner::new(vec![
            not_found(false),
            ok("$ claude --resume s9\nNo conversation found with session ID: s9\n$ "),
        ]));
        let herdr = Herdr::new(fake.clone());
        let timing = Timing {
            timeout: Duration::from_secs(60),
            poll: Duration::from_secs(60),
        };

        let started = std::time::Instant::now();
        let err = await_start_with(&herdr, &host(), "w3", PANE, Some(session_missing), &timing)
            .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let stopped = err.downcast_ref::<StoppedEarly>().expect("stopped early");
        assert!(
            stopped.pane_text.contains("No conversation found"),
            "{stopped:?}"
        );
        assert!(
            format!("{err:#}").contains("workspace w3 is left open"),
            "{err:#}"
        );
        let calls = fake.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[1].argv[3..6], ["pane", "read", PANE]);
    }

    #[test]
    fn await_start_fails_on_other_wait_errors() {
        let fake = Arc::new(FakeRunner::new(vec![Scripted {
            status: 1,
            stderr:
                r#"{"error":{"code":"invalid_target","message":"bad pane"},"id":"cli:agent:wait"}"#
                    .to_owned(),
            ..Scripted::default()
        }]));
        let herdr = Herdr::new(fake.clone());

        let err = await_start_with(&herdr, &host(), "w3", PANE, None, &fast()).unwrap_err();

        assert!(format!("{err:#}").contains("invalid_target"), "{err:#}");
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn await_start_retries_a_wait_that_gives_no_answer() {
        let silent = Scripted {
            status: 1,
            ..Scripted::default()
        };
        let fake = Arc::new(FakeRunner::new(vec![silent, wait_reports("idle")]));
        let herdr = Herdr::new(fake.clone());

        let start = await_start_with(&herdr, &host(), "w3", PANE, None, &fast()).unwrap();

        assert!(matches!(start, AgentStart::Running), "{start:?}");
        assert_eq!(wait_timeouts(&fake).len(), 2);
    }

    #[test]
    fn await_start_timeout_names_a_wait_that_gave_no_answer() {
        let fake = Arc::new(FakeRunner::new(vec![Scripted {
            status: 1,
            repeat: true,
            ..Scripted::default()
        }]));
        let herdr = Herdr::new(fake.clone());

        let err = await_start_with(&herdr, &host(), "w3", PANE, None, &fast()).unwrap_err();

        let err = format!("{err:#}");
        assert!(
            err.contains("did not start within 0 s; herdr gave no answer to at least one wait;"),
            "{err}"
        );
        assert!(err.contains("workspace w3 is left open"), "{err}");
    }

    #[test]
    fn await_start_fails_on_a_wait_that_exits_with_text() {
        let fake = Arc::new(FakeRunner::new(vec![Scripted {
            status: 1,
            stderr: "herdr: socket closed\n".to_owned(),
            ..Scripted::default()
        }]));
        let herdr = Herdr::new(fake.clone());

        let err = await_start_with(&herdr, &host(), "w3", PANE, None, &fast()).unwrap_err();

        assert!(format!("{err:#}").contains("socket closed"), "{err:#}");
        assert_eq!(fake.calls().len(), 1);
    }

    const DIALOG_ON_NO: &str = "\
=> /workspaces/acme-worktrees/search-index
>_ claude
 Accessing workspace:

 /workspaces/acme-worktrees/search-index

 Quick safety check: Is this a project you created or one you trust?

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    #[test]
    fn session_missing_reads_a_failed_resume() {
        assert!(session_missing(
            "$ claude --resume s9\nNo conversation found with session ID: s9\n$ "
        ));
        assert!(session_missing("Error: session s9 not found\n"));
        assert!(session_missing("could not RESUME: not found\n"));
        assert!(!session_missing(""));
        assert!(!session_missing(
            "$ claude --resume s9\n Loading conversation…\n"
        ));
        assert!(!session_missing("file not found: CLAUDE.md\n"));
        assert!(!session_missing(
            "resumed session s9\nError: file not found: CLAUDE.md\n"
        ));
    }

    #[test]
    fn trust_dialog_keys_moves_cursor_to_yes() {
        assert_eq!(
            trust_dialog_keys(DIALOG_ON_NO),
            Some(vec![KEY_DOWN, KEY_ENTER])
        );
    }

    #[test]
    fn trust_dialog_keys_confirms_when_cursor_is_on_yes() {
        let text =
            " Do you trust the files in this folder?\n\n > 1. Yes, proceed\n   2. No, exit\n";
        assert_eq!(trust_dialog_keys(text), Some(vec![KEY_ENTER]));
    }

    #[test]
    fn trust_dialog_keys_ignores_other_prompts() {
        assert_eq!(
            trust_dialog_keys("Choose a theme:\n ❯ Dark\n   Light\n"),
            None
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n   Yes\n ❯ No\n"),
            None,
            "Yes above the cursor would need an up key"
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n>_ claude\n   Yes\n"),
            None,
            "a shell prompt is not a cursor"
        );
    }
}
