//! `flow task adopt`: the supervisor takes over a session the user started by hand as a task. The
//! session keeps running where it is, in the herdr session it was started in; flow records a task
//! whose herdr target is its pane, writes it a brief, and tells it so through herdr. No hooks can
//! be added to a running session, so an adopted task reports through its own `flow task` calls and
//! herdr's view of it; `flow watch` logs its turn ends from herdr's `working` to `idle`.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use jiff::Timestamp;
use serde_json::json;

use super::Refused;
use super::dispatch::BriefSource;
use super::steer;
use crate::harness::brief;
use crate::herdr::{Agent, Herdr};
use crate::host;
use crate::inventory;
use crate::observe::{self, FollowList};
use crate::paths::{STATE_ENV, StateDir};
use crate::runner::{self, Runner, shell_quote};
use crate::state::config::Host;
use crate::state::events::Event;
use crate::state::task::{
    ClaudeSession, Harness, HerdrTarget, Hold, HoldOn, Origin, State, TaskRecord,
};
use crate::state::{Store, ids};
use crate::supervisor;

const EVENT_SOURCE: &str = "cli";
const OWNER: &str = "supervisor";
/// The only agent kind flow can brief and take over.
const CLAUDE: &str = "claude";
/// Printed after an adoption from outside the supervisor's session, as after a dispatch.
const HAND_ADOPT_NOTE: &str = "note: no supervisor turn is watching; the supervisor hears of \
     this task when it next ends a turn\n";

pub struct AdoptArgs {
    pub host: String,
    /// The hand session's title as `flow sessions` shows it, or its herdr pane id.
    pub session: String,
    pub title: String,
    pub criteria: String,
    /// The ask; a note to continue the work under way when `None`.
    pub brief: Option<BriefSource>,
    pub parent: Option<String>,
}

/// Creates a task for the hand session `args.session` names on `args.host`, briefs it, and prompts
/// the session to read the brief, then prints `adopted <id> on <host> (<pane_id>)`, with a note
/// when run outside the supervisor's session. The session is resolved as `flow follow` resolves it,
/// among observed sessions only, and is removed from the follow list. Refuses a host that is gone,
/// a pane a task or the supervisor holds, and an agent that is not Claude Code or has no known
/// working directory, all before writing anything. A failure once the task exists leaves it
/// `blocked` with a supervisor hold naming the failure and exits 1. When only the prompt cannot be
/// delivered, the adopted line is still printed.
pub fn run(state: &StateDir, args: &AdoptArgs) -> anyhow::Result<ExitCode> {
    let state = &state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let store = Store::open(state);
    let hosts = inventory::hosts(state)?;
    let host = host::find(&hosts, &args.host).map_err(|err| Refused(format!("{err:#}")))?;
    if !inventory::live_hosts(state)?.contains(host) {
        return Err(Refused(format!("host {} is gone", host.name)).into());
    }
    if let Some(parent) = &args.parent {
        store
            .task(parent)
            .map_err(|err| Refused(format!("--parent: {err:#}")))?;
    }
    let body = match &args.brief {
        Some(BriefSource::File(path)) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        Some(BriefSource::Text(text)) => text.clone(),
        None => format!(
            "Adopted from a session started by hand on {}. The conversation so far is the \
             context; continue the work already under way.",
            host.name
        ),
    };

    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let listed = super::sessions::host_agents(state, &runner, &host.name)?;
    let agent = super::sessions::resolve(&listed, &host.name, &args.session)?;
    refuse_unadoptable(host, agent)?;

    let flow = flow_command(
        state,
        &std::env::current_exe().context("locating the flow binary")?,
    )?;
    let rec = TaskRecord {
        id: ids::new_task_id(Timestamp::now(), &mut rand::rng()),
        parent: args.parent.clone(),
        title: args.title.clone(),
        host: host.name.clone(),
        cwd: agent.cwd.clone(),
        repo: None,
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: OWNER.to_owned(),
        origin: Origin::Adopted,
    };
    let brief_text = brief::render(&rec, &body, &args.criteria, &flow);
    let id = &rec.id;

    store.create_task(rec.clone(), &brief_text)?;
    let taken = take_over(&store, state, host, agent, id).and_then(|()| {
        let message = format!(
            "You are now task {id}, run by a supervisor. Read {} and follow its Reporting \
             section exactly; keep going with the work already under way.",
            store.brief_path(id)?.display()
        );
        let herdr = Herdr::with_session(runner, &agent.session);
        let prompted = herdr.prompt(host, &agent.pane_id, &message, &steer::DELIVERED);
        Ok((message, prompted))
    });
    let code = match taken {
        Ok((message, Ok(_))) => {
            if let Err(err) = store.append_event(
                id,
                &event(steer::EVENT_SOURCE, "steer", json!({ "text": message })),
            ) {
                return Err(fail(&store, id, err));
            }
            ExitCode::SUCCESS
        }
        Ok((_, Err(err))) => {
            hold(
                &store,
                id,
                "adopt.undelivered",
                "adoption message not delivered",
                &err,
            );
            eprintln!("{err:#}");
            ExitCode::FAILURE
        }
        Err(err) => return Err(fail(&store, id, err)),
    };
    super::print(&format!(
        "adopted {id} on {} ({})\n",
        host.name, agent.pane_id
    ))?;
    if !supervisor::is_supervisor_session() {
        super::print(HAND_ADOPT_NOTE)?;
    }
    Ok(code)
}

/// `env FLOW_STATE=<state root> <flow_bin>`, shell-quoted: the session was not started by flow,
/// so its environment may name no state directory, or another one.
fn flow_command(state: &StateDir, flow_bin: &Path) -> anyhow::Result<String> {
    let utf8 = |path: &Path| {
        path.to_str()
            .map(shell_quote)
            .with_context(|| format!("path is not UTF-8: {}", path.display()))
    };
    Ok(format!(
        "env {STATE_ENV}={} {}",
        utf8(state.root())?,
        utf8(flow_bin)?
    ))
}

/// Records task `id` as running in `agent`'s pane, logs the adoption, and drops the pane from
/// the follow list.
fn take_over(
    store: &Store,
    state: &StateDir,
    host: &Host,
    agent: &Agent,
    id: &str,
) -> anyhow::Result<()> {
    store.set_status(id, |status| {
        status.state = State::Working;
        status.herdr = Some(HerdrTarget {
            host: host.name.clone(),
            workspace_id: agent.workspace_id.clone(),
            pane_id: agent.pane_id.clone(),
            agent: agent.agent.clone(),
            session: agent.session.clone(),
        });
        status.claude_session = claude_session(agent);
    })?;
    store.append_event(
        id,
        &event(
            EVENT_SOURCE,
            "adopted",
            json!({
                "host": host.name,
                "session": agent.session,
                "pane_id": agent.pane_id,
                "title": agent.title,
            }),
        ),
    )?;
    FollowList::update(state, |list| {
        list.follow
            .retain(|entry| !entry.is(&host.name, &agent.session, &agent.pane_id));
    })
}

/// Holds task `id` for the supervisor after `err` stopped the adoption part way, and returns
/// `err` naming the task.
fn fail(store: &Store, id: &str, err: anyhow::Error) -> anyhow::Error {
    hold(store, id, "adopt.failed", "adoption failed", &err);
    err.context(format!("adopting the session as task {id}"))
}

/// Fails with `Refused` for an agent that is not Claude Code or whose working directory herdr
/// does not know.
fn refuse_unadoptable(host: &Host, agent: &Agent) -> anyhow::Result<()> {
    if agent.agent != CLAUDE {
        let kind = if agent.agent.is_empty() {
            "not identified by herdr"
        } else {
            &agent.agent
        };
        return Err(Refused(format!(
            "flow adopts Claude Code sessions only; this one is {kind}"
        ))
        .into());
    }
    if agent.cwd.as_os_str().is_empty() {
        return Err(Refused(format!(
            "herdr reports no working directory for pane {} on {}",
            agent.pane_id, host.name
        ))
        .into());
    }
    Ok(())
}

/// The Claude Code session herdr reports for `agent`, its transcript where Claude Code keeps
/// one for a session started in the agent's working directory.
fn claude_session(agent: &Agent) -> Option<ClaudeSession> {
    let session_id = agent.agent_session_id.clone()?;
    let transcript_path = observe::claude_home()
        .map(|home| observe::transcript_in(&home, &agent.cwd, &session_id))
        .unwrap_or_default();
    Some(ClaudeSession {
        session_id,
        transcript_path,
    })
}

/// Logs `err` as a `kind` event on task `id` and holds the task `blocked` on the supervisor
/// with `<what>: <err>`, so a half-adopted task explains itself. A failure to record it is
/// reported on stderr.
fn hold(store: &Store, id: &str, kind: &str, what: &str, err: &anyhow::Error) {
    let message = format!("{err:#}");
    let recorded = store
        .append_event(id, &event(EVENT_SOURCE, kind, json!({ "error": message })))
        .and_then(|()| {
            store.set_status(id, |status| {
                status.state = State::Blocked;
                status.hold = Some(Hold {
                    on: HoldOn::Supervisor,
                    what: format!("{what}: {message}"),
                    since: Timestamp::now(),
                });
            })
        });
    if let Err(record_err) = recorded {
        eprintln!("could not mark task {id} blocked: {record_err:#}");
    }
}

fn event(source: &str, kind: &str, data: serde_json::Value) -> Event {
    Event {
        ts: Timestamp::now(),
        source: source.to_owned(),
        kind: kind.to_owned(),
        data,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::herdr::AgentStatus;

    fn agent(kind: &str, cwd: &str, session_id: Option<&str>) -> Agent {
        Agent {
            pane_id: "w5:p2".to_owned(),
            workspace_id: "w5".to_owned(),
            agent: kind.to_owned(),
            status: AgentStatus::Idle,
            cwd: PathBuf::from(cwd),
            title: "Fix links".to_owned(),
            session: "default".to_owned(),
            agent_session_id: session_id.map(str::to_owned),
        }
    }

    #[test]
    fn claude_session_is_known_only_when_herdr_reports_one() {
        assert_eq!(claude_session(&agent(CLAUDE, "/work/other", None)), None);
        let session = claude_session(&agent(CLAUDE, "/work/other", Some("s1"))).unwrap();
        assert_eq!(session.session_id, "s1");
        assert!(
            session
                .transcript_path
                .ends_with(Path::new("projects/-work-other/s1.jsonl")),
            "{}",
            session.transcript_path.display()
        );
    }
}
