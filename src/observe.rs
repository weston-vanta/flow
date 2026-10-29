//! Observed sessions, the ones the user started by hand in herdr: each herdr agent is matched to
//! its owner (see `owner::owner`), whatever herdr session it is in, with the Claude Code session
//! it runs and that session's latest summary, read from Claude Code's session registry and
//! transcripts. Reading is all flow does there; it never writes Claude Code's state. Any observed
//! session turning `blocked` wakes the supervisor; the follow list,
//! `state/supervisor/follow.json`, names the followed ones, whose other state changes should
//! wake it too.
//!
//! herdr can report an agent's Claude Code session id, which names its registry session
//! exactly. An agent it reports none for is matched by cwd, and only when no other such agent
//! shares that cwd. Known limitation of that fallback: Claude Code's home directory is one
//! directory shared by every machine, so the registry lists sessions from all of them. The main
//! checkout's cwd exists on every machine, so such an agent there can be matched to a sibling
//! machine's session and show its summary.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::Agent;
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::state::fs::{with_lock, write_atomic};
use crate::state::task::{Status, TaskRecord};
use crate::supervisor::Lock;

/// The most of a transcript's end `latest_summary` reads.
const SUMMARY_WINDOW: u64 = 1024 * 1024;
const AWAY_SUMMARY: &str = "away_summary";
const FOLLOW_FILE: &str = "supervisor/follow.json";

/// One Claude Code session as its registry file under `<claude home>/sessions/` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistrySession {
    pub session_id: String,
    pub cwd: PathBuf,
    /// The session's display name; `None` when it has none.
    pub name: Option<String>,
    /// Claude Code's status name, such as `busy` or `idle`.
    pub status: String,
    pub updated: Timestamp,
    pub pid_domain: String,
}

/// A registry file. Only the fields flow reads are declared.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    session_id: String,
    cwd: PathBuf,
    name: Option<String>,
    #[serde(default)]
    status: String,
    /// Unix milliseconds.
    updated_at: i64,
    #[serde(default)]
    pid_domain: String,
}

/// One transcript line, as far as `latest_summary` reads it.
#[derive(Deserialize)]
struct TranscriptLine {
    subtype: Option<String>,
    content: Option<String>,
}

/// A herdr agent with what flow knows about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchedAgent {
    pub host: String,
    pub agent: Agent,
    pub owner: Owner,
    /// Whether the agent is an observed session in the follow list.
    pub followed: bool,
    pub session: Option<RegistrySession>,
    /// How `session` was found; `None` when there is no session.
    pub matched_by: Option<MatchedBy>,
    pub summary: Option<String>,
}

/// How an agent's registry session was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchedBy {
    /// The session of the task whose worker runs in the agent's pane.
    Task,
    /// The session id herdr reports for the agent.
    Id,
    /// The newest session started in the agent's cwd: a guess by directory.
    Cwd,
}

/// `state/supervisor/follow.json`: the hand-started sessions whose every state change should
/// wake the supervisor, not only turning `blocked`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowList {
    pub follow: Vec<Followed>,
}

/// An agent's pane, named by its host, herdr session, and pane id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Followed {
    pub host: String,
    pub pane_id: String,
    pub session: String,
    /// The agent's title when it was followed, for display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Followed {
    /// Whether this names the pane `pane_id` in herdr session `session` on `host`.
    pub fn is(&self, host: &str, session: &str, pane_id: &str) -> bool {
        self.host == host && self.session == session && self.pane_id == pane_id
    }
}

impl FollowList {
    /// The follow list, empty when the file does not exist.
    pub fn load(state: &StateDir) -> anyhow::Result<FollowList> {
        let path = follow_path(state);
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(FollowList::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Applies `f` to the follow list under its lock, writing the list back when `f` changed
    /// it.
    pub fn update<T>(state: &StateDir, f: impl FnOnce(&mut FollowList) -> T) -> anyhow::Result<T> {
        let path = follow_path(state);
        let dir = path
            .parent()
            .expect("the follow file has a parent directory");
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        with_lock(&path, || {
            let mut list = FollowList::load(state).map_err(io::Error::other)?;
            let before = list.clone();
            let result = f(&mut list);
            if list != before {
                let mut bytes = serde_json::to_vec_pretty(&list).map_err(io::Error::other)?;
                bytes.push(b'\n');
                write_atomic(&path, &bytes)?;
            }
            Ok(result)
        })
        .with_context(|| format!("updating {}", path.display()))
    }
}

fn follow_path(state: &StateDir) -> PathBuf {
    state.root().join(FOLLOW_FILE)
}

/// Claude Code's home directory: `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`.
pub fn claude_home() -> anyhow::Result<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    set("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".claude")))
        .ok_or_else(|| anyhow!("neither $CLAUDE_CONFIG_DIR nor $HOME is set"))
}

/// Every parsable session in `<claude_home>/sessions/*.json`, sorted by session id. Files that
/// do not parse, or vanish while being read as their sessions end, are skipped.
pub fn registry_sessions(claude_home: &Path) -> anyhow::Result<Vec<RegistrySession>> {
    let dir = claude_home.join("sessions");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        if let Some(session) = parse_registry_entry(&text) {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(sessions)
}

fn parse_registry_entry(text: &str) -> Option<RegistrySession> {
    let entry: RegistryEntry = serde_json::from_str(text).ok()?;
    Some(RegistrySession {
        session_id: entry.session_id,
        cwd: entry.cwd,
        name: entry.name,
        status: entry.status,
        updated: Timestamp::from_millisecond(entry.updated_at).ok()?,
        pid_domain: entry.pid_domain,
    })
}

/// `cwd` with every character that is not ASCII alphanumeric replaced by `-`: the name of the
/// directory under `<claude home>/projects/` holding transcripts of sessions started in `cwd`.
pub fn project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Where Claude Code keeps `session`'s transcript.
pub fn transcript_path(claude_home: &Path, session: &RegistrySession) -> PathBuf {
    transcript_in(claude_home, &session.cwd, &session.session_id)
}

/// Where Claude Code keeps the transcript of session `session_id` started in `cwd`.
pub fn transcript_in(claude_home: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    claude_home
        .join("projects")
        .join(project_slug(cwd))
        .join(format!("{session_id}.jsonl"))
}

/// The `content` of the transcript's last `away_summary` line, reading at most its last
/// `SUMMARY_WINDOW` bytes; `None` when there is none or the transcript does not exist.
pub fn latest_summary(transcript: &Path) -> anyhow::Result<Option<String>> {
    let context = || format!("reading {}", transcript.display());
    let mut file = match File::open(transcript) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(context),
    };
    let start = file
        .metadata()
        .with_context(context)?
        .len()
        .saturating_sub(SUMMARY_WINDOW);
    // Inside the file, read one byte before the window too: everything up to the first newline
    // is then the tail of a line that starts before the window, or nothing when the window starts
    // on a line boundary.
    let read_from = start.saturating_sub(1);
    file.seek(SeekFrom::Start(read_from))
        .with_context(context)?;
    let mut window = Vec::new();
    file.take(SUMMARY_WINDOW + (start - read_from))
        .read_to_end(&mut window)
        .with_context(context)?;
    let mut lines = window.split(|&byte| byte == b'\n');
    if start > 0 {
        lines.next();
    }
    Ok(lines.rev().find_map(away_summary))
}

fn away_summary(line: &[u8]) -> Option<String> {
    let line = std::str::from_utf8(line).ok()?;
    if !line.contains(AWAY_SUMMARY) {
        return None;
    }
    let parsed: TranscriptLine = serde_json::from_str(line).ok()?;
    (parsed.subtype.as_deref() == Some(AWAY_SUMMARY))
        .then_some(parsed.content)
        .flatten()
}

/// Matches each agent of `hosts`, herdr's agents per host, to its owner, given `supervisor`, the
/// running lock, and to a registry session. A task's agent gets the task's own Claude Code
/// session when the registry has it. Any other agent herdr reports a session id for gets the
/// registry session with that id, or none when the registry lacks it. An agent with neither is
/// matched by cwd only when no other such agent on any host shares its cwd: it gets the most
/// recently updated session started there that no task or reported id holds. Agents sharing a cwd
/// get none, as any pick between them would be a guess. Summaries are left for the caller to read.
pub fn match_agents(
    hosts: &[(&str, &[Agent])],
    tasks: &[(TaskRecord, Status)],
    supervisor: Option<&Lock>,
    registry: &[RegistrySession],
) -> Vec<MatchedAgent> {
    let by_id = |id: &str| registry.iter().find(|s| s.session_id == id);
    let found: Vec<Found> = hosts
        .iter()
        .flat_map(|(host, agents)| agents.iter().map(move |agent| (*host, agent)))
        .map(|(host, agent)| {
            let owner = owner::owner(host, agent, supervisor, tasks);
            let task_session = match &owner {
                Owner::Task(id) => tasks
                    .iter()
                    .find(|(record, _)| record.id == *id)
                    .and_then(|(_, status)| status.claude_session.as_ref())
                    .and_then(|claude| by_id(&claude.session_id)),
                _ => None,
            };
            Found {
                host,
                agent,
                owner,
                task_session,
            }
        })
        .collect();
    let held: Vec<&str> = tasks
        .iter()
        .filter_map(|(_, status)| status.claude_session.as_ref())
        .map(|claude| claude.session_id.as_str())
        .chain(
            found
                .iter()
                .filter_map(|f| f.agent.agent_session_id.as_deref()),
        )
        .collect();
    let by_cwd = |agent: &Agent| {
        let sharing = found
            .iter()
            .filter(|f| f.unidentified() && f.agent.cwd == agent.cwd)
            .count();
        if sharing != 1 {
            return None;
        }
        registry
            .iter()
            .filter(|s| s.cwd == agent.cwd && !held.contains(&s.session_id.as_str()))
            .max_by_key(|s| s.updated)
    };
    found
        .iter()
        .map(|f| {
            let (session, matched_by) = match (f.task_session, &f.agent.agent_session_id) {
                (Some(session), _) => (Some(session), MatchedBy::Task),
                (None, Some(id)) => (by_id(id), MatchedBy::Id),
                (None, None) => (by_cwd(f.agent), MatchedBy::Cwd),
            };
            MatchedAgent {
                host: f.host.to_owned(),
                agent: f.agent.clone(),
                owner: f.owner.clone(),
                followed: false,
                session: session.cloned(),
                matched_by: session.map(|_| matched_by),
                summary: None,
            }
        })
        .collect()
}

/// An agent on its host, with its owner and, for a task's worker, the task's registry session.
struct Found<'a> {
    host: &'a str,
    agent: &'a Agent,
    owner: Owner,
    task_session: Option<&'a RegistrySession>,
}

impl Found<'_> {
    /// Whether neither a task nor herdr names the agent's session, leaving only its cwd.
    fn unidentified(&self) -> bool {
        self.task_session.is_none() && self.agent.agent_session_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::herdr::AgentStatus;
    use crate::state::task::{ClaudeSession, Harness, HerdrTarget, Origin};
    use crate::supervisor::Lock;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude");
    const FIXTURE_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn an_untitled_follow_entry_parses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        fs::create_dir_all(root.join("supervisor")).unwrap();
        fs::write(
            root.join(FOLLOW_FILE),
            r#"{"follow": [{"host": "host-b", "pane_id": "w5:p1", "session": "default"}]}"#,
        )
        .unwrap();
        let state = StateDir::resolve(dir.path()).unwrap();
        assert_eq!(
            FollowList::load(&state).unwrap().follow,
            [Followed {
                host: "host-b".to_owned(),
                pane_id: "w5:p1".to_owned(),
                session: "default".to_owned(),
                title: None,
            }]
        );
    }

    #[test]
    fn registry_parses_fixture() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        fs::create_dir(&sessions).unwrap();
        fs::copy(
            Path::new(FIXTURES).join("sessions/1.json"),
            sessions.join("1.json"),
        )
        .unwrap();
        fs::write(sessions.join("1.0123abcd.key"), "not a registry entry").unwrap();
        fs::write(sessions.join("2.json"), "{\"truncated\": ").unwrap();

        let registry = registry_sessions(home.path()).unwrap();

        assert_eq!(
            registry,
            [RegistrySession {
                session_id: FIXTURE_SESSION_ID.to_owned(),
                cwd: PathBuf::from("/work/repo"),
                name: Some("tidy-docs".to_owned()),
                status: "idle".to_owned(),
                updated: Timestamp::from_millisecond(1_793_000_600_000).unwrap(),
                pid_domain: "machine-0001".to_owned(),
            }]
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(registry_sessions(empty.path()).unwrap(), []);
    }

    #[test]
    fn latest_summary_finds_last_away_summary() {
        let summary = latest_summary(&Path::new(FIXTURES).join("transcript.jsonl")).unwrap();
        assert_eq!(
            summary.as_deref(),
            Some(
                "Moved the guides into one folder and fixed the links between them; waiting \
                 for a review of the new index page."
            )
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            latest_summary(&dir.path().join("absent.jsonl")).unwrap(),
            None
        );
    }

    #[test]
    fn latest_summary_reads_only_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.jsonl");
        let old = r#"{"type":"system","subtype":"away_summary","content":"too old"}"#;
        let filler = format!("{{\"type\":\"user\",\"text\":\"{}\"}}\n", "x".repeat(1000));
        let mut text = format!("{old}\n");
        while text.len() < SUMMARY_WINDOW as usize + 10 {
            text.push_str(&filler);
        }
        fs::write(&path, &text).unwrap();
        assert_eq!(latest_summary(&path).unwrap(), None);

        // The window starts inside the `recent` line, which is dropped as partial.
        let recent = r#"{"type":"system","subtype":"away_summary","content":"cut off"}"#;
        let mut text = format!("{recent}\n");
        text.push_str(&filler.repeat(SUMMARY_WINDOW as usize / filler.len()));
        fs::write(&path, &text).unwrap();
        assert!(text.len() as u64 > SUMMARY_WINDOW);
        assert!(text.len() as u64 - SUMMARY_WINDOW < recent.len() as u64);
        assert_eq!(latest_summary(&path).unwrap(), None);
    }

    #[test]
    fn latest_summary_keeps_a_line_starting_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boundary.jsonl");
        let first = r#"{"type":"system","subtype":"away_summary","content":"at the start"}"#;
        let filler = format!("{{\"type\":\"user\",\"text\":\"{}\"}}\n", "x".repeat(1000));
        let mut window = format!("{first}\n");
        window.push_str(&filler.repeat((SUMMARY_WINDOW as usize - window.len()) / filler.len()));
        let pad = SUMMARY_WINDOW as usize - window.len() - r#"{"text":""}"#.len() - 1;
        window.push_str(&format!("{{\"text\":\"{}\"}}\n", "y".repeat(pad)));
        assert_eq!(window.len() as u64, SUMMARY_WINDOW);
        fs::write(&path, format!("{{\"type\":\"user\"}}\n{window}")).unwrap();

        assert_eq!(
            latest_summary(&path).unwrap().as_deref(),
            Some("at the start")
        );
    }

    #[test]
    fn project_slug_replaces_non_alphanumerics() {
        assert_eq!(project_slug(Path::new("/work/repo")), "-work-repo");
        assert_eq!(
            project_slug(Path::new("/work/repo.wt/fix_cache-2")),
            "-work-repo-wt-fix-cache-2"
        );
    }

    fn agent(session: &str, pane_id: &str, cwd: &str) -> Agent {
        Agent {
            pane_id: pane_id.to_owned(),
            workspace_id: pane_id.split(':').next().unwrap().to_owned(),
            agent: "claude".to_owned(),
            status: AgentStatus::Idle,
            cwd: PathBuf::from(cwd),
            title: String::new(),
            session: session.to_owned(),
            agent_session_id: None,
        }
    }

    fn reporting(id: &str, agent: Agent) -> Agent {
        Agent {
            agent_session_id: Some(id.to_owned()),
            ..agent
        }
    }

    fn session_ids(matched: &[MatchedAgent]) -> Vec<Option<&str>> {
        matched
            .iter()
            .map(|a| a.session.as_ref().map(|s| s.session_id.as_str()))
            .collect()
    }

    fn task(id: &str, host: &str, pane_id: &str, session_id: &str) -> (TaskRecord, Status) {
        let record = TaskRecord {
            id: id.to_owned(),
            parent: None,
            title: "a task".to_owned(),
            host: host.to_owned(),
            cwd: PathBuf::from("/work/repo.wt/feature"),
            repo: Some("example-repo".to_owned()),
            harness: Harness::Claude,
            created: Timestamp::UNIX_EPOCH,
            owner: "supervisor".to_owned(),
            origin: Origin::Dispatched,
        };
        let mut status = Status::pending(Timestamp::UNIX_EPOCH);
        status.herdr = Some(HerdrTarget {
            host: host.to_owned(),
            workspace_id: "w1".to_owned(),
            pane_id: pane_id.to_owned(),
            agent: "claude".to_owned(),
            session: "flow".to_owned(),
        });
        status.claude_session = Some(ClaudeSession {
            session_id: session_id.to_owned(),
            transcript_path: PathBuf::from("/home/dev/.claude/projects/x/y.jsonl"),
        });
        (record, status)
    }

    fn registered(session_id: &str, cwd: &str, updated_s: i64) -> RegistrySession {
        RegistrySession {
            session_id: session_id.to_owned(),
            cwd: PathBuf::from(cwd),
            name: None,
            status: "idle".to_owned(),
            updated: Timestamp::from_second(updated_s).unwrap(),
            pid_domain: "machine-0001".to_owned(),
        }
    }

    #[test]
    fn match_prefers_task_pane_over_cwd() {
        let agents = [
            // The task's worker, in the same cwd as a newer hand session.
            reporting("s-task", agent("flow", "w1:p1", "/work/repo")),
            // A hand session in the default herdr session, whose pane ids are its own.
            agent("default", "w1:p1", "/work/repo"),
            agent("default", "w2:p1", "/work/elsewhere"),
        ];
        let tasks = [
            task("aaaaaaab", "host-a", "w1:p1", "s-task"),
            task("aaaaaaac", "host-b", "w2:p1", "s-other"),
        ];
        let registry = [
            registered("s-task", "/work/repo", 100),
            registered("s-hand-old", "/work/repo", 200),
            registered("s-hand", "/work/repo", 300),
        ];

        let matched = match_agents(&[("host-a", &agents)], &tasks, None, &registry);

        let summary: Vec<(&Owner, Option<&str>)> = matched
            .iter()
            .map(|a| (&a.owner, a.session.as_ref().map(|s| s.session_id.as_str())))
            .collect();
        assert_eq!(
            summary,
            [
                (&Owner::Task("aaaaaaab".to_owned()), Some("s-task")),
                (&Owner::Observed, Some("s-hand")),
                (&Owner::Observed, None),
            ]
        );
        assert_eq!(
            matched_by(&matched),
            [Some(MatchedBy::Task), Some(MatchedBy::Cwd), None]
        );
        assert!(matched.iter().all(|a| a.host == "host-a"));
        assert_eq!(matched[1].agent, agents[1]);
    }

    #[test]
    fn match_finds_an_adopted_task_in_the_herdr_session_its_target_names() {
        let agents = [
            agent("flow", "w1:p1", "/work/repo"),
            agent("default", "w1:p1", "/work/repo"),
        ];
        let mut adopted = in_default(task("aaaaaaab", "host-a", "w1:p1", "s-task"));
        adopted.1.claude_session = None;

        let matched = match_agents(&[("host-a", &agents)], &[adopted], None, &[]);

        let owners: Vec<&Owner> = matched.iter().map(|m| &m.owner).collect();
        assert_eq!(
            owners,
            [&Owner::Observed, &Owner::Task("aaaaaaab".to_owned())]
        );
    }

    #[test]
    fn match_takes_the_session_herdr_reports() {
        let agents = [
            reporting("s1", agent("default", "w1:p1", "/work/repo")),
            reporting("s2", agent("default", "w1:p2", "/work/repo")),
        ];
        let registry = [
            registered("s1", "/work/repo", 100),
            registered("s2", "/work/repo", 200),
            registered("s3", "/work/repo", 300),
        ];

        let matched = match_agents(&[("host-a", &agents)], &[], None, &registry);

        assert_eq!(session_ids(&matched), [Some("s1"), Some("s2")]);
    }

    #[test]
    fn match_leaves_a_reported_session_the_registry_lacks_unmatched() {
        let agents = [reporting("s-gone", agent("default", "w1:p1", "/work/repo"))];
        let registry = [registered("s1", "/work/repo", 100)];

        let matched = match_agents(&[("host-a", &agents)], &[], None, &registry);

        assert_eq!(session_ids(&matched), [None]);
    }

    fn matched_by(matched: &[MatchedAgent]) -> Vec<Option<MatchedBy>> {
        matched.iter().map(|a| a.matched_by).collect()
    }

    #[test]
    fn match_by_cwd_takes_the_newest_session_no_other_agent_holds() {
        let agents = [agent("default", "w1:p1", "/work/repo")];
        let elsewhere = [reporting("s3", agent("default", "w1:p1", "/work/repo"))];
        let registry = [
            registered("s1", "/work/repo", 100),
            registered("s2", "/work/repo", 200),
            registered("s3", "/work/repo", 300),
        ];

        let matched = match_agents(
            &[("host-a", &agents), ("host-b", &elsewhere)],
            &[],
            None,
            &registry,
        );

        assert_eq!(session_ids(&matched), [Some("s2"), Some("s3")]);
        assert_eq!(
            matched_by(&matched),
            [Some(MatchedBy::Cwd), Some(MatchedBy::Id)]
        );
        assert_eq!(matched[1].host, "host-b");
    }

    #[test]
    fn match_by_cwd_leaves_agents_sharing_a_cwd_unmatched() {
        let agents = [
            agent("default", "w1:p1", "/work/repo"),
            agent("default", "w2:p1", "/work/app"),
        ];
        let elsewhere = [agent("default", "w1:p1", "/work/repo")];
        let registry = [
            registered("s1", "/work/repo", 100),
            registered("s2", "/work/repo", 200),
            registered("s3", "/work/app", 300),
        ];

        let matched = match_agents(
            &[("host-a", &agents), ("host-b", &elsewhere)],
            &[],
            None,
            &registry,
        );

        assert_eq!(session_ids(&matched), [None, Some("s3"), None]);
        assert_eq!(matched_by(&matched), [None, Some(MatchedBy::Cwd), None]);
    }

    #[test]
    fn workers_the_supervisor_and_hand_sessions_share_one_herdr_session() {
        let agents = [
            reporting("s-sup", agent("default", "w1:p1", "/workspaces/flow")),
            reporting(
                "s-worker",
                agent("default", "w2:p1", "/work/repo.wt/feature"),
            ),
            agent("default", "w3:p1", "/work/repo"),
            reporting("s-adopted", agent("default", "w4:p1", "/work/other")),
        ];
        let mut worker = task("aaaaaaab", "host-a", "w2:p1", "s-worker");
        let mut adopted = task("aaaaaaac", "host-a", "w4:p1", "s-adopted");
        adopted.0.origin = Origin::Adopted;
        for (_, status) in [&mut worker, &mut adopted] {
            if let Some(target) = &mut status.herdr {
                target.session = "default".to_owned();
            }
        }
        let supervisor = supervisor_lock("w1:p1", Some("s-sup"));

        let matched = match_agents(
            &[("host-a", &agents)],
            &[worker, adopted],
            Some(&supervisor),
            &[],
        );

        let owners: Vec<&Owner> = matched.iter().map(|m| &m.owner).collect();
        assert_eq!(
            owners,
            [
                &Owner::Supervisor,
                &Owner::Task("aaaaaaab".to_owned()),
                &Owner::Observed,
                &Owner::Task("aaaaaaac".to_owned()),
            ]
        );
    }

    /// The supervisor's lock: its pane `pane_id` in herdr's `default` session on `host-a`, and
    /// the Claude Code session it recorded.
    fn supervisor_lock(pane_id: &str, claude_session: Option<&str>) -> Lock {
        Lock {
            host_env_id: "env-0001".to_owned(),
            herdr: HerdrTarget {
                host: "host-a".to_owned(),
                workspace_id: "w1".to_owned(),
                pane_id: pane_id.to_owned(),
                agent: "claude".to_owned(),
                session: "default".to_owned(),
            },
            claude_session: claude_session.map(str::to_owned),
            since: Timestamp::UNIX_EPOCH,
            stopped: None,
        }
    }

    fn in_default(mut task: (TaskRecord, Status)) -> (TaskRecord, Status) {
        if let Some(target) = &mut task.1.herdr {
            target.session = "default".to_owned();
        }
        task
    }
}
