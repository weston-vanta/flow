//! `flow down`: stops the supervisor and leaves every worker and task as it is. It finds the
//! supervisor in the pane its lock records, telling it as `flow up` does, since herdr reuses pane
//! ids, and closes that pane, or the workspace it sits in when it is that workspace's only pane.
//! When another session runs in that pane, or one it cannot tell from the supervisor, it refuses,
//! changing nothing; `--force` closes the lock's pane or workspace without checking what runs
//! there. When the pane holds no agent, the supervisor's workspace is closed only while herdr lists
//! it under the supervisor's label. It then marks the lock stopped, keeping the Claude Code session
//! to resume, and clears the watch lock, stopping a watcher still running on this machine; a
//! watcher on another host ends on its own at its next cycle once its lock is gone. The next `flow
//! up` starts the supervisor again on the host it runs on, resuming that session. When the
//! supervisor ran in a herdr session that is neither the configured one nor herdr's `default`, and
//! that session has no agents and no workspaces left, it offers to stop that session's server.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::Arc;

use anyhow::Context;

use super::{Consent, Refused};
use crate::herdr::{self, Herdr, Workspace};
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::supervisor::{self, Identity, Lock, SUPERVISOR_NAME};
use crate::watch::lock as watch_lock;

/// Stops the supervisor `state/supervisor/lock.json` records, printing a line per step and how
/// many open tasks keep running. With `yes`, stops an emptied herdr session's server without
/// asking; with `force`, closes the lock's pane or workspace without checking what runs there.
/// Fails, changing nothing, when the supervisor's host cannot be reached, since the supervisor
/// may be alive there, and refuses when another session runs in its pane or one flow cannot
/// tell from the supervisor.
pub fn run(state: &StateDir, yes: bool, force: bool) -> anyhow::Result<ExitCode> {
    let Some(lock) = supervisor::load(state)? else {
        super::print("no supervisor recorded; nothing to stop\n")?;
        return Ok(ExitCode::SUCCESS);
    };
    if lock.stopped.is_some() {
        super::print("the supervisor is already stopped; nothing to stop\n")?;
        return Ok(ExitCode::SUCCESS);
    }
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let herdr = Herdr::with_session(runner, &lock.herdr.session);
    let host = reachable_host(state, &lock)?;
    if let Some(host) = &host {
        close_supervisor(&herdr, host, &lock, force)?;
    }
    let marked = supervisor::mark_stopped(state)?;
    super::print(&format!(
        "{}\n",
        stopped_line(marked, lock.claude_session.as_deref())
    ))?;
    for line in clear_watch_lock(state, &Proc)? {
        super::print(&format!("{line}\n"))?;
    }
    if let Some(host) = &host
        && may_offer_server_stop(&lock, &config)
    {
        offer_server_stop(&herdr, host, yes)?;
    }
    let open = Store::open(state)
        .list_tasks()?
        .iter()
        .filter(|(_, status)| status.state.is_open())
        .count();
    super::print(&format!(
        "{open} open tasks keep running; flow up resumes the supervisor\n"
    ))?;
    Ok(ExitCode::SUCCESS)
}

/// What marking the lock stopped did: `marked` is false when the lock was gone by then.
fn stopped_line(marked: bool, session: Option<&str>) -> String {
    if !marked {
        return "state/supervisor/lock.json was removed meanwhile, so nothing was marked stopped \
                and flow up starts a fresh supervisor session"
            .to_owned();
    }
    let kept = session
        .map(|session| format!(", keeping session {session} to resume"))
        .unwrap_or_default();
    format!("marked the supervisor stopped in state/supervisor/lock.json{kept}")
}

/// Whether stopping the server of the supervisor's herdr session may be offered: never the
/// configured session's, which flow keeps using, nor herdr's `default`, the user's own.
fn may_offer_server_stop(lock: &Lock, config: &Config) -> bool {
    let session = &lock.herdr.session;
    *session != config.herdr_session && session != herdr::DEFAULT_SESSION
}

/// The host the supervisor runs on, or `None`, saying so, when it is unknown, gone, or not
/// running in Ona: the supervisor went with it.
fn reachable_host(state: &StateDir, lock: &Lock) -> anyhow::Result<Option<Host>> {
    let gone = |why: String| {
        super::print(&format!(
            "the supervisor's pane {} on {} is already gone: {why}\n",
            lock.herdr.pane_id, lock.herdr.host
        ))
    };
    let Some(record) = inventory::load(state, &lock.host_env_id)? else {
        gone(format!("{} is not a known host", lock.host_env_id))?;
        return Ok(None);
    };
    if record.gone {
        gone(format!("{} is gone", record.name))?;
        return Ok(None);
    }
    if !record.should_probe() {
        gone(format!("{} is not running", record.name))?;
        return Ok(None);
    }
    Ok(Some(record.host()))
}

/// Closes the supervisor on `host`: its pane, or its whole workspace when the pane is the
/// workspace's only one. The pane is the lock's when the agent there is confirmed to be the
/// supervisor (see `Lock::identify`), or, with no agent there, while herdr lists the lock's
/// workspace under the supervisor's label; otherwise it says the pane is already gone. Refuses,
/// closing nothing, when the agent there is another or cannot be told from the supervisor.
/// With `force`, closes the lock's pane or workspace without asking what runs there.
fn close_supervisor(herdr: &Herdr, host: &Host, lock: &Lock, force: bool) -> anyhow::Result<()> {
    let unreachable = |err: anyhow::Error| {
        err.context(format!(
            "closing the supervisor's workspace on {}; nothing stopped, and the lock is kept",
            host.name
        ))
    };
    let gone = || {
        super::print(&format!(
            "the supervisor's pane {} on {} is already gone\n",
            lock.herdr.pane_id, host.name
        ))
    };
    if force {
        let workspaces = herdr.workspaces_if_running(host).map_err(unreachable)?;
        let Some(workspace) = workspaces
            .unwrap_or_default()
            .into_iter()
            .find(|workspace| workspace.workspace_id == lock.herdr.workspace_id)
        else {
            return gone();
        };
        let Some(closed) = close_pane_or_workspace(herdr, host, &workspace, &lock.herdr.pane_id)
            .map_err(unreachable)?
        else {
            return gone();
        };
        return super::print(&format!(
            "closed the lock's {} on {} (herdr session {}) without checking what runs there \
             (--force)\n",
            closed.what(&workspace, &lock.herdr.pane_id),
            host.name,
            herdr.session()
        ));
    }
    let Some(mut agents) = herdr.agents_if_running(host).map_err(unreachable)? else {
        return gone();
    };
    let workspaces = herdr
        .workspaces_if_running(host)
        .map_err(unreachable)?
        .unwrap_or_default();
    supervisor::label_workspaces(&mut agents, &workspaces);
    let in_pane = agents
        .iter()
        .find(|agent| agent.pane_id == lock.herdr.pane_id);
    let workspace = match in_pane {
        Some(agent) => match lock.identify(agent) {
            Identity::Supervisor => workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == agent.workspace_id)
                .cloned()
                .unwrap_or_else(|| Workspace {
                    workspace_id: agent.workspace_id.clone(),
                    label: String::new(),
                    pane_count: None,
                }),
            Identity::Unconfirmed => {
                return Err(Refused(format!(
                    "{}. flow down stopped nothing and kept the lock; if it is not the \
                     supervisor, `flow down --force` stops it",
                    supervisor::unconfirmed(lock, &host.name, agent)
                ))
                .into());
            }
            Identity::Other => {
                return Err(Refused(format!(
                    "pane {} on {} runs {}; flow down stopped nothing and kept the lock",
                    agent.pane_id,
                    host.name,
                    supervisor::not_the_supervisor(lock, agent)
                ))
                .into());
            }
        },
        None => match workspaces
            .iter()
            .find(|workspace| lock.names_workspace(workspace))
        {
            Some(workspace) => workspace.clone(),
            None => return gone(),
        },
    };
    let Some(closed) = close_pane_or_workspace(herdr, host, &workspace, &lock.herdr.pane_id)
        .map_err(unreachable)?
    else {
        return gone();
    };
    let relabelled = !workspace.label.is_empty() && workspace.label != SUPERVISOR_NAME;
    let relabelled = |before: &str, after: &str| {
        if relabelled {
            format!("{before}\"{}\"{after}", workspace.label)
        } else {
            String::new()
        }
    };
    let line = match closed {
        Closed::Workspace => format!(
            "closed the supervisor's workspace {} on {} (herdr session {}){}\n",
            workspace.workspace_id,
            host.name,
            herdr.session(),
            relabelled(", relabelled ", "")
        ),
        Closed::Pane => format!(
            "closed the supervisor's pane {} on {} (herdr session {}); workspace {}{} keeps its \
             {}\n",
            lock.herdr.pane_id,
            host.name,
            herdr.session(),
            workspace.workspace_id,
            relabelled(" (", ")"),
            other_panes(&workspace)
        ),
    };
    super::print(&line)
}

/// What `close_pane_or_workspace` closed.
enum Closed {
    Pane,
    Workspace,
}

impl Closed {
    fn what(&self, workspace: &Workspace, pane_id: &str) -> String {
        match self {
            Closed::Pane => format!("pane {pane_id}"),
            Closed::Workspace => format!("workspace {}", workspace.workspace_id),
        }
    }
}

/// "other N panes" or "other pane" of `workspace` beside the closed one, or "other panes" when
/// herdr did not count them.
fn other_panes(workspace: &Workspace) -> String {
    match workspace.pane_count {
        Some(2) => "other pane".to_owned(),
        Some(count) => format!("other {} panes", count.saturating_sub(1)),
        None => "other panes".to_owned(),
    }
}

/// Closes `workspace` when `pane_id` is its only pane, else that pane alone, which herdr also
/// does when it does not count the workspace's panes; `None` when herdr no longer knows it.
fn close_pane_or_workspace(
    herdr: &Herdr,
    host: &Host,
    workspace: &Workspace,
    pane_id: &str,
) -> anyhow::Result<Option<Closed>> {
    let (closed, what) = if workspace.pane_count == Some(1) {
        (
            herdr.close_workspace(host, &workspace.workspace_id)?,
            Closed::Workspace,
        )
    } else {
        (herdr.close_pane(host, pane_id)?, Closed::Pane)
    };
    Ok(closed.then_some(what))
}

/// This machine's processes, as `flow down` needs them, so tests can stand in for `/proc` and
/// signals.
pub(crate) trait Processes {
    /// The command line of process `pid`, or `None` when it cannot be read.
    fn cmdline(&self, pid: u32) -> Option<Vec<String>>;
    /// The processes `pid` started that still run, such as a watcher's herdr waits and ssh
    /// connections.
    fn children(&self, pid: u32) -> Vec<u32>;
    /// Sends SIGTERM to `pid`.
    fn terminate(&self, pid: u32) -> anyhow::Result<()>;
}

struct Proc;

impl Processes for Proc {
    fn cmdline(&self, pid: u32) -> Option<Vec<String>> {
        let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(
            bytes
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect(),
        )
    }

    fn children(&self, pid: u32) -> Vec<u32> {
        let Ok(threads) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return Vec::new();
        };
        threads
            .filter_map(|thread| std::fs::read_to_string(thread.ok()?.path().join("children")).ok())
            .flat_map(|children| {
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect()
    }

    fn terminate(&self, pid: u32) -> anyhow::Result<()> {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .context("running kill")?;
        anyhow::ensure!(status.success(), "kill exited {status}");
        Ok(())
    }
}

/// Whether `cmdline` is a flow binary running `flow watch`.
fn is_flow_watch(cmdline: &[String]) -> bool {
    let program = cmdline.first().map(Path::new);
    program
        .and_then(Path::file_name)
        .is_some_and(|name| name == "flow")
        && cmdline.get(1).is_some_and(|arg| arg == "watch")
}

/// Removes the watch lock, first stopping its holder, and the processes it started, when it runs
/// on this machine, holds a fresh lock, and is a `flow watch`: an old lock's pid may name an
/// unrelated process by now. Returns the lines to print.
fn clear_watch_lock(state: &StateDir, procs: &dyn Processes) -> anyhow::Result<Vec<String>> {
    let mut lines = Vec::new();
    let owner = watch_lock::owner(state)?;
    if let Some(owner) = owner.as_ref().filter(|owner| owner.is_running_here()) {
        let pid = owner.pid;
        let cmdline = procs.cmdline(pid);
        if !watch_lock::is_fresh(state)? {
            lines.push(format!(
                "left pid {pid} running: state/supervisor/watch.lock was not touched in the last \
                 {} minutes, so the pid may name another process by now",
                watch_lock::STALE_MINUTES
            ));
        } else if !cmdline.as_deref().is_some_and(is_flow_watch) {
            let runs = cmdline
                .filter(|cmdline| !cmdline.is_empty())
                .map(|cmdline| format!("`{}`", cmdline.join(" ")))
                .unwrap_or_else(|| "a command flow cannot read".to_owned());
            lines.push(format!(
                "left pid {pid} running: it runs {runs}, not flow watch"
            ));
        } else {
            // Read before the watcher ends, after which its children are no longer its own.
            let children = procs.children(pid);
            match procs.terminate(pid) {
                Ok(()) => {
                    let stopped = children
                        .iter()
                        .filter(|child| match procs.terminate(**child) {
                            Ok(()) => true,
                            Err(err) => {
                                eprintln!(
                                    "could not stop pid {child}, which the watcher started: \
                                     {err:#}"
                                );
                                false
                            }
                        })
                        .count();
                    lines.push(match stopped {
                        0 => format!("stopped the watcher (pid {pid})"),
                        1 => format!("stopped the watcher (pid {pid}) and the process it started"),
                        n => format!(
                            "stopped the watcher (pid {pid}) and the {n} processes it started"
                        ),
                    });
                }
                Err(err) => eprintln!("could not stop the watcher (pid {pid}): {err:#}"),
            }
        }
    }
    if !watch_lock::clear(state)? {
        return Ok(lines);
    }
    lines.push(match owner.filter(|owner| !owner.is_on_this_host()) {
        Some(owner) => format!(
            "removed state/supervisor/watch.lock; the watcher on {} ends at its next cycle",
            owner.host
        ),
        None => "removed state/supervisor/watch.lock".to_owned(),
    });
    Ok(lines)
}

/// Offers to stop `herdr`'s session's server on `host` when it has no agents and no workspaces
/// left.
fn offer_server_stop(herdr: &Herdr, host: &Host, yes: bool) -> anyhow::Result<()> {
    let session = herdr.session();
    let emptied = herdr.agents_if_running(host).and_then(|agents| {
        if agents.is_none_or(|agents| !agents.is_empty()) {
            return Ok(false);
        }
        let workspaces = herdr.workspaces_if_running(host)?;
        Ok(workspaces.is_some_and(|workspaces| workspaces.is_empty()))
    });
    match emptied {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(err) => {
            eprintln!(
                "could not list herdr session {session} on {}: {err:#}",
                host.name
            );
            return Ok(());
        }
    }
    super::print(&format!(
        "Herdr:\n - stop the server of herdr session {session} on {}, which has no agents and \
         no workspaces left.\n",
        host.name
    ))?;
    let left = format!(
        "left herdr session {session}'s server running on {}",
        host.name
    );
    match super::confirm(yes)? {
        Consent::Given => {
            let output = herdr.stop_server(host)?;
            super::print(&output.stdout)?;
            eprint!("{}", output.stderr);
            super::print(&format!(
                "stopped herdr session {session}'s server on {}\n",
                host.name
            ))
        }
        Consent::Declined => super::print(&format!("{left}\n")),
        Consent::NoTerminal => super::print(&format!(
            "{left}; to stop it, run there: {}\n",
            herdr::stop_server_argv(session).join(" ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::*;

    struct FakeProcs {
        cmdline: Option<Vec<String>>,
        children: Vec<u32>,
        terminated: RefCell<Vec<u32>>,
    }

    impl FakeProcs {
        fn running(cmdline: Option<&[&str]>) -> FakeProcs {
            FakeProcs {
                cmdline: cmdline.map(|words| words.iter().map(|w| (*w).to_owned()).collect()),
                children: Vec::new(),
                terminated: RefCell::new(Vec::new()),
            }
        }
    }

    impl Processes for FakeProcs {
        fn cmdline(&self, _pid: u32) -> Option<Vec<String>> {
            self.cmdline.clone()
        }

        fn children(&self, _pid: u32) -> Vec<u32> {
            self.children.clone()
        }

        fn terminate(&self, pid: u32) -> anyhow::Result<()> {
            self.terminated.borrow_mut().push(pid);
            Ok(())
        }
    }

    const WATCH: &[&str] = &["/workspaces/flow/target/release/flow", "watch"];

    /// A state dir whose watch lock this test process holds, as a watcher on this machine would.
    fn held() -> (tempfile::TempDir, StateDir, u32) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("state")).unwrap();
        let state = StateDir::resolve(dir.path()).unwrap();
        let pid = std::process::id();
        std::mem::forget(watch_lock::acquire(&state, pid).unwrap().unwrap());
        (dir, state, pid)
    }

    fn lock_dir(state: &StateDir) -> std::path::PathBuf {
        state.root().join("supervisor/watch.lock")
    }

    #[test]
    fn a_fresh_lock_held_by_flow_watch_here_stops_the_watcher() {
        let (_dir, state, pid) = held();
        let procs = FakeProcs::running(Some(WATCH));

        let lines = clear_watch_lock(&state, &procs).unwrap();

        assert_eq!(*procs.terminated.borrow(), [pid]);
        assert_eq!(
            lines,
            [
                format!("stopped the watcher (pid {pid})"),
                "removed state/supervisor/watch.lock".to_owned(),
            ]
        );
        assert!(!lock_dir(&state).exists());
    }

    #[test]
    fn stopping_the_watcher_also_stops_the_processes_it_started() {
        let (_dir, state, pid) = held();
        let procs = FakeProcs {
            children: vec![7001, 7002],
            ..FakeProcs::running(Some(WATCH))
        };

        let lines = clear_watch_lock(&state, &procs).unwrap();

        assert_eq!(*procs.terminated.borrow(), [pid, 7001, 7002]);
        assert_eq!(
            lines[0],
            format!("stopped the watcher (pid {pid}) and the 2 processes it started")
        );
    }

    #[test]
    fn the_stopped_line_says_when_the_lock_vanished_meanwhile() {
        assert_eq!(
            stopped_line(true, Some("s9")),
            "marked the supervisor stopped in state/supervisor/lock.json, keeping session s9 to \
             resume"
        );
        assert_eq!(
            stopped_line(true, None),
            "marked the supervisor stopped in state/supervisor/lock.json"
        );
        assert_eq!(
            stopped_line(false, Some("s9")),
            "state/supervisor/lock.json was removed meanwhile, so nothing was marked stopped and \
             flow up starts a fresh supervisor session"
        );
    }

    #[test]
    fn a_pid_running_something_else_is_left_alone_and_only_the_lock_cleared() {
        for (cmdline, runs) in [
            (Some(&["sleep", "30"][..]), "`sleep 30`"),
            (
                Some(&["/workspaces/flow/target/release/flow", "guard"][..]),
                "`/workspaces/flow/target/release/flow guard`",
            ),
            (None, "a command flow cannot read"),
        ] {
            let (_dir, state, pid) = held();
            let procs = FakeProcs::running(cmdline);

            let lines = clear_watch_lock(&state, &procs).unwrap();

            assert!(procs.terminated.borrow().is_empty());
            assert_eq!(
                lines,
                [
                    format!("left pid {pid} running: it runs {runs}, not flow watch"),
                    "removed state/supervisor/watch.lock".to_owned(),
                ]
            );
            assert!(!lock_dir(&state).exists());
        }
    }

    #[test]
    fn a_stale_lock_never_signals_its_pid() {
        let (_dir, state, pid) = held();
        let old = SystemTime::now() - Duration::from_secs((watch_lock::STALE_MINUTES + 1) * 60);
        File::open(lock_dir(&state))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let procs = FakeProcs::running(Some(WATCH));

        let lines = clear_watch_lock(&state, &procs).unwrap();

        assert!(procs.terminated.borrow().is_empty());
        assert_eq!(
            lines,
            [
                format!(
                    "left pid {pid} running: state/supervisor/watch.lock was not touched in the \
                     last 10 minutes, so the pid may name another process by now"
                ),
                "removed state/supervisor/watch.lock".to_owned(),
            ]
        );
        assert!(!lock_dir(&state).exists());
    }

    #[test]
    fn proc_reads_a_process_command_line_and_children() {
        let mut parent = Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .spawn()
            .unwrap();
        let pid = parent.id();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut children = Proc.children(pid);
        while children.len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            children = Proc.children(pid);
        }

        assert_eq!(children.len(), 2, "{children:?}");
        assert_eq!(
            Proc.cmdline(children[0]),
            Some(vec!["sleep".to_owned(), "30".to_owned()])
        );
        assert_eq!(
            Proc.cmdline(pid),
            Some(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "sleep 30 & sleep 30 & wait".to_owned()
            ])
        );
        for child in children {
            Proc.terminate(child).unwrap();
        }
        parent.wait().unwrap();
    }

    #[test]
    fn flow_watch_is_a_flow_binary_whose_first_argument_is_watch() {
        let argv = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        for yes in [
            &["/home/dev/.local/bin/flow", "watch"][..],
            &["/workspaces/flow/target/release/flow", "watch", "--once"],
            &["flow", "watch"],
        ] {
            assert!(is_flow_watch(&argv(yes)), "{yes:?}");
        }
        for no in [
            &["flow", "guard"][..],
            &["/bin/sh", "/workspaces/flow/target/release/flow", "watch"],
            &["flowx", "watch"],
            &["sleep", "watch"],
            &[],
        ] {
            assert!(!is_flow_watch(&argv(no)), "{no:?}");
        }
    }
}
