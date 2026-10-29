//! Every environment variable flow reads, named in one place so that a herdr server flow starts
//! runs with none of them set: the panes a user later opens there inherit the server's
//! environment.

/// Names the state directory, in place of `<repo_root>/state`.
pub const STATE: &str = "FLOW_STATE";
/// Set in the supervisor's environment, so its hooks can tell it from other sessions started in
/// the same repo.
pub const SUPERVISOR: &str = "FLOW_SUPERVISOR";
/// Names a JSON script file; when set, the runner is a fake one reading it.
pub const FAKE_RUNNER: &str = "FLOW_FAKE_RUNNER";
/// Names a JSON file read in place of running the Ona CLI.
pub const FAKE_ONA: &str = "FLOW_FAKE_ONA";
/// Under the fake runner, makes commands treat stdin and stdout as a terminal when set.
pub const FAKE_TTY: &str = "FLOW_FAKE_TTY";
/// Overrides how long a start handshake waits between asks while herdr does not see the agent
/// yet, in milliseconds.
pub const HANDSHAKE_POLL_MS: &str = "FLOW_HANDSHAKE_POLL_MS";
/// Overrides how long `flow guard` waits for a watcher to take the lock, in milliseconds.
pub const GUARD_GRACE_MS: &str = "FLOW_GUARD_GRACE_MS";
/// The directory `flow init` searches for the repository when Ona does not name one.
pub const INIT_WORKSPACES: &str = "FLOW_INIT_WORKSPACES";
/// The search path `flow init` looks for the worktree tool in, in place of `$PATH`.
pub const INIT_PATH: &str = "FLOW_INIT_PATH";
/// Overrides the watch poll interval, in milliseconds.
pub const WATCH_POLL_MS: &str = "FLOW_WATCH_POLL_MS";
/// Overrides the delay before a failed watch first retries, in milliseconds.
pub const WATCH_RETRY_MS: &str = "FLOW_WATCH_RETRY_MS";
/// Overrides how long a watch run watching only hand sessions nobody follows lasts, in
/// milliseconds.
pub const WATCH_HAND_MAX_MS: &str = "FLOW_WATCH_HAND_MAX_MS";
/// Overrides how long listing the hosts' agents may take in a watch run, in milliseconds.
pub const WATCH_LIST_TIMEOUT_MS: &str = "FLOW_WATCH_LIST_TIMEOUT_MS";

pub const ALL: [&str; 13] = [
    STATE,
    SUPERVISOR,
    FAKE_RUNNER,
    FAKE_ONA,
    FAKE_TTY,
    HANDSHAKE_POLL_MS,
    GUARD_GRACE_MS,
    INIT_WORKSPACES,
    INIT_PATH,
    WATCH_POLL_MS,
    WATCH_RETRY_MS,
    WATCH_HAND_MAX_MS,
    WATCH_LIST_TIMEOUT_MS,
];

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn sources(dir: &Path, found: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push((
                    path.display().to_string(),
                    fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }

    #[test]
    fn every_flow_variable_is_named_here_only() {
        let mut found = Vec::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        let quoted = format!("\"{}", "FLOW_");
        for (path, text) in found {
            if path.ends_with("src/vars.rs") {
                continue;
            }
            assert!(
                !text.contains(&quoted),
                "{path} names a flow variable; name it in src/vars.rs"
            );
        }
        let mut names = ALL.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }
}
