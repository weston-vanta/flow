//! Adoption of sessions the user started by hand in herdr: each herdr agent is matched to the
//! task whose worker runs in its pane, or shown as adopted, with the Claude Code session it runs
//! and that session's latest summary, read from Claude Code's session registry and transcripts.
//! Reading is all flow does there; it never writes Claude Code's state. The follow list,
//! `state/supervisor/follow.json`, names the adopted sessions whose state changes should wake
//! the supervisor.
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

use crate::herdr::{self, Agent};
use crate::paths::StateDir;
use crate::state::fs::{with_lock, write_atomic};
use crate::state::task::{Status, TaskRecord};

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
    #[serde(default)]
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
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

/// A herdr agent with what flow knows about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Adopted {
    pub host: String,
    pub agent: Agent,
    /// The task whose worker runs in the agent's pane; `None` for a hand-started session.
    pub task: Option<String>,
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

/// `state/supervisor/follow.json`: the adopted sessions whose state changes should wake the
/// supervisor.
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
    claude_home
        .join("projects")
        .join(project_slug(&session.cwd))
        .join(format!("{}.jsonl", session.session_id))
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

/// Matches each agent of `hosts`, herdr's agents per host, to the task whose worker runs in its
/// pane and to a registry session. A task's agent gets the task's own Claude Code session when
/// the registry has it. Any other agent herdr reports a session id for gets the registry
/// session with that id, or none when the registry lacks it. An agent with neither is matched
/// by cwd only when no other such agent on any host shares its cwd: it gets the most recently
/// updated session started there that no task or reported id holds. Agents sharing a cwd get
/// none, as any pick between them would be a guess. Summaries are left for the caller to read.
pub fn match_agents(
    hosts: &[(&str, &[Agent])],
    tasks: &[(TaskRecord, Status)],
    registry: &[RegistrySession],
) -> Vec<Adopted> {
    let by_id = |id: &str| registry.iter().find(|s| s.session_id == id);
    let found: Vec<Found> = hosts
        .iter()
        .flat_map(|(host, agents)| agents.iter().map(move |agent| (*host, agent)))
        .map(|(host, agent)| {
            let task = task_in_pane(host, agent, tasks);
            let task_session = task
                .and_then(|(_, status)| status.claude_session.as_ref())
                .and_then(|claude| by_id(&claude.session_id));
            Found {
                host,
                agent,
                task,
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
            Adopted {
                host: f.host.to_owned(),
                agent: f.agent.clone(),
                task: f.task.map(|(record, _)| record.id.clone()),
                session: session.cloned(),
                matched_by: session.map(|_| matched_by),
                summary: None,
            }
        })
        .collect()
}

/// An agent on its host, with the task whose worker runs in its pane and that task's registry
/// session.
struct Found<'a> {
    host: &'a str,
    agent: &'a Agent,
    task: Option<&'a (TaskRecord, Status)>,
    task_session: Option<&'a RegistrySession>,
}

impl Found<'_> {
    /// Whether neither a task nor herdr names the agent's session, leaving only its cwd.
    fn unidentified(&self) -> bool {
        self.task_session.is_none() && self.agent.agent_session_id.is_none()
    }
}

/// The task whose worker runs in `agent`'s pane on `host`, preferring an open task, then the
/// newest. Workers run only in flow's herdr session.
fn task_in_pane<'a>(
    host: &str,
    agent: &Agent,
    tasks: &'a [(TaskRecord, Status)],
) -> Option<&'a (TaskRecord, Status)> {
    if agent.session != herdr::SESSION {
        return None;
    }
    tasks
        .iter()
        .filter(|(_, status)| {
            status
                .herdr
                .as_ref()
                .is_some_and(|target| target.host == host && target.pane_id == agent.pane_id)
        })
        .max_by_key(|(record, status)| (status.state.is_open(), record.created))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::herdr::{AgentStatus, SESSION};
    use crate::state::task::{ClaudeSession, Harness, HerdrTarget};

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude");
    const FIXTURE_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";

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

    fn session_ids(adopted: &[Adopted]) -> Vec<Option<&str>> {
        adopted
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
        };
        let mut status = Status::pending(Timestamp::UNIX_EPOCH);
        status.herdr = Some(HerdrTarget {
            host: host.to_owned(),
            workspace_id: "w1".to_owned(),
            pane_id: pane_id.to_owned(),
            agent: "claude".to_owned(),
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
            agent(SESSION, "w1:p1", "/work/repo"),
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

        let adopted = match_agents(&[("host-a", &agents)], &tasks, &registry);

        let summary: Vec<(Option<&str>, Option<&str>)> = adopted
            .iter()
            .map(|a| {
                (
                    a.task.as_deref(),
                    a.session.as_ref().map(|s| s.session_id.as_str()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some("aaaaaaab"), Some("s-task")),
                (None, Some("s-hand")),
                (None, None),
            ]
        );
        assert_eq!(
            matched_by(&adopted),
            [Some(MatchedBy::Task), Some(MatchedBy::Cwd), None]
        );
        assert!(adopted.iter().all(|a| a.host == "host-a"));
        assert_eq!(adopted[1].agent, agents[1]);
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

        let adopted = match_agents(&[("host-a", &agents)], &[], &registry);

        assert_eq!(session_ids(&adopted), [Some("s1"), Some("s2")]);
    }

    #[test]
    fn match_leaves_a_reported_session_the_registry_lacks_unmatched() {
        let agents = [reporting("s-gone", agent("default", "w1:p1", "/work/repo"))];
        let registry = [registered("s1", "/work/repo", 100)];

        let adopted = match_agents(&[("host-a", &agents)], &[], &registry);

        assert_eq!(session_ids(&adopted), [None]);
    }

    fn matched_by(adopted: &[Adopted]) -> Vec<Option<MatchedBy>> {
        adopted.iter().map(|a| a.matched_by).collect()
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

        let adopted = match_agents(
            &[("host-a", &agents), ("host-b", &elsewhere)],
            &[],
            &registry,
        );

        assert_eq!(session_ids(&adopted), [Some("s2"), Some("s3")]);
        assert_eq!(
            matched_by(&adopted),
            [Some(MatchedBy::Cwd), Some(MatchedBy::Id)]
        );
        assert_eq!(adopted[1].host, "host-b");
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

        let adopted = match_agents(
            &[("host-a", &agents), ("host-b", &elsewhere)],
            &[],
            &registry,
        );

        assert_eq!(session_ids(&adopted), [None, Some("s3"), None]);
        assert_eq!(matched_by(&adopted), [None, Some(MatchedBy::Cwd), None]);
    }
}
