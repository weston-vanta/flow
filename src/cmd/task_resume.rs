//! `flow task resume`: reopens a `done` or `orphaned` dispatched task and relaunches the Claude
//! Code session its worker ran in, in a new tab on the task's host and in its working directory,
//! with the task's settings, so its hooks and flow reporting work as they did. The first
//! message reminds the worker that its brief's Reporting section still applies, then carries
//! the follow-up when one is given.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use serde_json::json;

use super::Refused;
use super::dispatch::{self, HAND_DISPATCH_NOTE};
use crate::harness::handshake::AgentStart;
use crate::harness::{ClaudeAdapter, HarnessAdapter};
use crate::herdr::Herdr;
use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::Host;
use crate::state::task::{ClaudeSession, Origin, State, Status, TaskRecord};
use crate::supervisor;

pub struct ResumeArgs {
    pub id: String,
    /// The follow-up the worker should act on; without one it waits for the next message.
    pub prompt: Option<String>,
    pub allow_shared_dir: bool,
}

/// Reopens task `args.id` to `working`, launches `claude --settings <task settings> --name <id>
/// --resume <session> <message>` in a new tab labelled like a dispatched worker's, records the
/// tab as the task's herdr target, and prints `resumed <id> on <host> (<pane_id>)`, with a note
/// when run outside the supervisor's session. Refuses, changing nothing, a task that is open,
/// archived, adopted, still has a herdr tab, recorded no session, or whose transcript or working
/// directory is missing on its host. A failure once the tab is opening logs `resume.failed` and
/// returns the task to `done` or `orphaned`.
pub fn run(state: &StateDir, args: &ResumeArgs) -> anyhow::Result<ExitCode> {
    // Task paths go to the worker under the physical shared-volume path, as for `flow dispatch`.
    let state = &state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let store = Store::open(state);
    let id = &args.id;
    if store.is_archived(id)? {
        return Err(Refused(format!(
            "task {id} is archived; flow does not resume archived tasks (dispatch a new task, \
             pointing its brief at the archived report under state/archive/tasks/{id}/)"
        ))
        .into());
    }
    let (rec, status) = store.task(id)?;
    let session = refusal(&rec, &status)?.clone();
    let hosts = inventory::hosts(state)?;
    let host = host::find(&hosts, &rec.host).map_err(|err| Refused(format!("{err:#}")))?;
    if !inventory::live_hosts(state)?.contains(host) {
        return Err(Refused(format!(
            "task {id}'s host {} is gone, and its session with it; dispatch a new task instead",
            host.name
        ))
        .into());
    }
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    check_on_host(runner.as_ref(), host, &rec, &session)?;
    let herdr = Herdr::with_session(runner, &supervisor::herdr_session(state)?);
    herdr.ensure_server(host)?;
    if !args.allow_shared_dir {
        dispatch::refuse_shared_dir(&herdr.agents(host)?, host, &rec.cwd)?;
    }

    let flow_bin = std::env::current_exe().context("locating the flow binary")?;
    let message = first_message(
        &rec,
        &store.brief_path(id)?,
        &flow_bin,
        args.prompt.as_deref(),
    );
    let launch = ClaudeAdapter.prepare_resume(
        &rec,
        &store.task_dir(id)?,
        &flow_bin,
        &session.session_id,
        &message,
    )?;
    dispatch::append_event(
        &store,
        id,
        "resume",
        json!({
            "from": status.state,
            "session_id": session.session_id,
            "prompt": args.prompt,
        }),
    )?;
    match dispatch::start_worker(&store, state.root(), &herdr, host, &rec, &launch) {
        Ok((pane_id, start)) => {
            if let AgentStart::BlockedAtStart { .. } = start {
                eprintln!(
                    "worker in pane {pane_id} is blocked on something other than the trust \
                     dialog; see `flow task events {id}`"
                );
            }
            super::print(&format!("resumed {id} on {} ({pane_id})\n", host.name))?;
            if !supervisor::is_supervisor_session() {
                super::print(HAND_DISPATCH_NOTE)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            if let Err(record_err) = record_failure(&store, id, status.state, &err) {
                eprintln!(
                    "could not return task {id} to {}: {record_err:#}",
                    status.state
                );
            }
            Err(err)
        }
    }
}

/// Logs `resume.failed` and returns the task to `from`, so it can be resumed again once the
/// failure is dealt with. A herdr target the start recorded stays, so `flow task close` can
/// retire a tab left open.
fn record_failure(store: &Store, id: &str, from: State, err: &anyhow::Error) -> anyhow::Result<()> {
    dispatch::append_event(
        store,
        id,
        "resume.failed",
        json!({ "error": format!("{err:#}") }),
    )?;
    store.set_status(id, |status| {
        status.state = from;
        status.hold = None;
    })
}

/// The task's recorded Claude Code session when the task's record allows resuming it, else a
/// `Refused` saying what to do instead.
pub(super) fn refusal<'a>(
    rec: &TaskRecord,
    status: &'a Status,
) -> anyhow::Result<&'a ClaudeSession> {
    let id = &rec.id;
    if rec.origin == Origin::Adopted {
        return Err(Refused(format!(
            "task {id} was adopted from a session started by hand, whose pane flow leaves to its \
             owner and which may still be running there; resume it there by hand, or dispatch a \
             new task"
        ))
        .into());
    }
    if status.state.is_open() {
        return Err(Refused(format!(
            "task {id} is {}, still open; flow task resume reopens done or orphaned tasks (use \
             `flow steer {id}` to message its worker)",
            status.state
        ))
        .into());
    }
    if let Some(target) = &status.herdr {
        return Err(Refused(format!(
            "task {id}'s tab {} on {} is still open; run `flow task close {id}` first, or use \
             `flow steer {id}` if its worker is still running there",
            target.tab_id, target.host
        ))
        .into());
    }
    status.claude_session.as_ref().ok_or_else(|| {
        Refused(format!(
            "task {id} recorded no Claude Code session to resume; dispatch a new task instead"
        ))
        .into()
    })
}

/// Refuses when the session's transcript is missing or empty on `host`, since `claude --resume`
/// of one exits at once, or when the task's working directory is gone.
fn check_on_host(
    runner: &dyn Runner,
    host: &Host,
    rec: &TaskRecord,
    session: &ClaudeSession,
) -> anyhow::Result<()> {
    let exists = |test: &str, path: &Path| -> anyhow::Result<bool> {
        let path = path
            .to_str()
            .with_context(|| format!("path is not UTF-8: {}", path.display()))?;
        let argv = ["test".to_owned(), test.to_owned(), path.to_owned()];
        let out = runner.run(host, &argv, None)?;
        match out.status {
            0 => Ok(true),
            1 => Ok(false),
            code => anyhow::bail!(
                "`test {test} {path}` on {} exited {code}: {}",
                host.name,
                out.stderr.trim()
            ),
        }
    };
    let id = &rec.id;
    if !exists("-s", &session.transcript_path)? {
        return Err(Refused(format!(
            "no transcript of task {id}'s session {} at {} on {}; dispatch a new task instead, \
             pointing its brief at the task's report",
            session.session_id,
            session.transcript_path.display(),
            host.name
        ))
        .into());
    }
    if !exists("-d", &rec.cwd)? {
        return Err(Refused(format!(
            "task {id}'s working directory {} is gone from {}; dispatch a new task instead",
            rec.cwd.display(),
            host.name
        ))
        .into());
    }
    Ok(())
}

/// The resumed session's first message: that the task was reopened, that the brief's Reporting
/// section still applies, then `prompt`, or, without one, to wait for the next message.
fn first_message(rec: &TaskRecord, brief: &Path, flow_bin: &Path, prompt: Option<&str>) -> String {
    let id = &rec.id;
    let flow = flow_bin.display();
    let reminder = format!(
        "Task {id} was reopened with `flow task resume` and reads `working` again. The \
         Reporting section of your brief ({}) still applies: report through those commands, \
         and when this follow-up is finished, write a new report and run `{flow} task done {id} \
         --report <path>` again.",
        brief.display()
    );
    match prompt {
        Some(prompt) => format!("{reminder}\n\n{prompt}"),
        None => format!(
            "{reminder}\n\nNo follow-up was given yet: reply in one line that you are ready, then \
             wait for the next message."
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::task::{Harness, HerdrTarget};

    fn rec(origin: Origin) -> TaskRecord {
        TaskRecord {
            id: "aaaaaaab".to_owned(),
            parent: None,
            title: "Add retry".to_owned(),
            host: "host-a".to_owned(),
            cwd: PathBuf::from("/work/retry"),
            repo: None,
            harness: Harness::Claude,
            created: "2026-10-26T12:00:00Z".parse().unwrap(),
            owner: "supervisor".to_owned(),
            origin,
        }
    }

    fn closed(state: State) -> Status {
        let mut status = Status::pending("2026-10-26T12:00:00Z".parse().unwrap());
        status.state = state;
        status.claude_session = Some(ClaudeSession {
            session_id: "s1".to_owned(),
            transcript_path: PathBuf::from("/home/dev/.claude/projects/-work-retry/s1.jsonl"),
        });
        status
    }

    fn refused(rec: &TaskRecord, status: &Status) -> String {
        let err = refusal(rec, status).unwrap_err();
        assert!(err.downcast_ref::<Refused>().is_some(), "{err:#}");
        format!("{err:#}")
    }

    #[test]
    fn done_and_orphaned_tasks_with_a_session_resume() {
        for state in [State::Done, State::Orphaned] {
            let status = closed(state);
            assert_eq!(
                refusal(&rec(Origin::Dispatched), &status)
                    .unwrap()
                    .session_id,
                "s1"
            );
        }
    }

    #[test]
    fn open_tasks_are_refused() {
        for state in [
            State::Pending,
            State::Working,
            State::Blocked,
            State::NeedsDecision,
        ] {
            let err = refused(&rec(Origin::Dispatched), &closed(state));
            assert!(err.contains("still open"), "{err}");
            assert!(err.contains("flow steer aaaaaaab"), "{err}");
        }
    }

    #[test]
    fn a_task_whose_tab_is_open_is_refused() {
        let mut status = closed(State::Done);
        status.herdr = Some(HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: "w1".to_owned(),
            tab_id: "w1:t2".to_owned(),
            pane_id: "w1:p2".to_owned(),
            agent: "claude".to_owned(),
            session: "flow".to_owned(),
        });
        let err = refused(&rec(Origin::Dispatched), &status);
        assert!(err.contains("tab w1:t2 on host-a is still open"), "{err}");
        assert!(err.contains("flow task close aaaaaaab"), "{err}");
    }

    #[test]
    fn a_task_without_a_session_is_refused() {
        let mut status = closed(State::Done);
        status.claude_session = None;
        let err = refused(&rec(Origin::Dispatched), &status);
        assert!(err.contains("recorded no Claude Code session"), "{err}");
    }

    #[test]
    fn adopted_tasks_are_refused() {
        let err = refused(&rec(Origin::Adopted), &closed(State::Done));
        assert!(err.contains("adopted"), "{err}");
    }

    #[test]
    fn first_message_reminds_of_reporting_then_carries_the_prompt() {
        let brief = Path::new("/state/tasks/aaaaaaab/brief.md");
        let flow = Path::new("/opt/flow/bin/flow");
        let with = first_message(
            &rec(Origin::Dispatched),
            brief,
            flow,
            Some("Address the review comments."),
        );
        assert!(with.contains("Reporting section of your brief (/state/tasks/aaaaaaab/brief.md)"));
        assert!(with.contains("/opt/flow/bin/flow task done aaaaaaab --report <path>"));
        assert!(with.ends_with("\n\nAddress the review comments."), "{with}");
        let without = first_message(&rec(Origin::Dispatched), brief, flow, None);
        assert!(without.ends_with("wait for the next message."), "{without}");
    }
}
