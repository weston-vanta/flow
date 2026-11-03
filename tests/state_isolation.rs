//! No test may reach the state `FLOW_STATE` names: a worker session runs `cargo test` with it
//! set to the shared state.

mod common;

use std::fs;
use std::path::Path;

use flow::paths::{STATE_ENV, StateDir};

const REPO: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn fixture_state_and_children_stay_off_the_state_flow_state_names() {
    let decoy = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test in this binary that touches the environment, and nothing
    // else in it reads the environment while it runs.
    unsafe { std::env::set_var(STATE_ENV, decoy.path()) };

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("state")).unwrap();
    assert!(common::state(dir.path()).root().starts_with(dir.path()));
    assert!(
        StateDir::at(dir.path().join("state"))
            .unwrap()
            .root()
            .starts_with(dir.path())
    );

    // A flow child the test does not point at a state finds none, rather than the decoy or the
    // repo's state link.
    let out = common::flow().arg("status").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(common::NO_STATE), "{stderr}");

    let out = common::command("sh")
        .args(["-c", &format!("printf %s \"${{{STATE_ENV}-unset}}\"")])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "unset");

    assert_eq!(fs::read_dir(decoy.path()).unwrap().count(), 0);
}

/// Tests build state with `StateDir::at` and spawn children through `tests/common`, never with
/// the environment-reading `StateDir::resolve` or a bare command.
#[test]
fn tests_never_resolve_state_from_the_environment_or_spawn_bare_children() {
    // Split so this file does not match itself.
    let resolve = ["StateDir::", "resolve("].concat();
    let bare = [["Command::", "new("].concat(), ["cargo_", "bin("].concat()];
    let mut offenders = Vec::new();
    for file in rust_files(&Path::new(REPO).join("src")) {
        let text = fs::read_to_string(&file).unwrap();
        let allowed = file.ends_with("src/main.rs") || file.ends_with("src/paths.rs");
        if !allowed && text.contains(&resolve) {
            offenders.push(format!("{}: {resolve}", file.display()));
        }
    }
    for file in rust_files(&Path::new(REPO).join("tests")) {
        if file.starts_with(Path::new(REPO).join("tests/common")) {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap();
        for needle in std::iter::once(&resolve).chain(&bare) {
            if text.lines().any(|line| contains_unqualified(line, needle)) {
                offenders.push(format!("{}: {needle}", file.display()));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

/// `line` names `needle` other than as `common::<needle>`.
fn contains_unqualified(line: &str, needle: &str) -> bool {
    line.match_indices(needle)
        .any(|(at, _)| !line[..at].ends_with("common::"))
}

fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}
