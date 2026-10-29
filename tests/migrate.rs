use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::Origin;
use flow::watch::classify::Observation;
use flow::{inventory, supervisor, watch};

const REPO: &str = env!("CARGO_MANIFEST_DIR");

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Every file under `dir` with its contents, keyed by its path relative to `dir`.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_owned();
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

fn migrate(state: &Path, args: &[&str]) -> String {
    let out = Command::new(Path::new(REPO).join("scripts/migrate-2026-10-29.sh"))
        .args(args)
        .arg(state)
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

fn has_jq() -> bool {
    Command::new("jq")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

#[test]
fn migrates_old_state_so_flow_reads_it_and_a_second_run_changes_nothing() {
    if !has_jq() {
        eprintln!("skipping: the migration script needs jq, which is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    copy_dir(
        &Path::new(REPO).join("tests/fixtures/migrate-2026-10-29"),
        &root,
    );
    let state = StateDir::resolve(dir.path()).unwrap();
    let old = snapshot(&root);
    assert!(Store::open(&state).list_tasks().is_err());

    let dry_run = migrate(&root, &["--dry-run"]);
    assert!(dry_run.contains("dry run: nothing written"), "{dry_run}");
    assert!(
        dry_run.contains("tasks/aaaaaaab/task.json: task worktree renamed to cwd"),
        "{dry_run}"
    );
    assert_eq!(snapshot(&root), old);

    let first = migrate(&root, &[]);
    assert_eq!(first, dry_run.replace("dry run: nothing written\n", ""));

    let tasks = Store::open(&state).list_tasks().unwrap();
    let (record, status) = tasks.iter().find(|(r, _)| r.id == "aaaaaaab").unwrap();
    assert_eq!(record.cwd, Path::new("/work/repo.wt/old"));
    assert_eq!(record.origin, Origin::Dispatched);
    assert_eq!(status.herdr.as_ref().unwrap().session, "flow");
    let (adopted, _) = tasks.iter().find(|(r, _)| r.id == "aaaaaaac").unwrap();
    assert_eq!(adopted.origin, Origin::Adopted);

    let lock = supervisor::load(&state).unwrap().unwrap();
    assert_eq!(lock.herdr.session, "flow");

    let hosts = inventory::load_all(&state).unwrap();
    let host_a = hosts.iter().find(|h| h.env_id == "env-0001").unwrap();
    assert_eq!(host_a.ssh_alias, "env-0001.ona.environment");
    assert_eq!(host_a.agents[0].session, "flow");
    assert_eq!(host_a.worktrees[0].repo, "");
    assert_eq!(host_a.prs[0].repo, "");
    let host_b = hosts.iter().find(|h| h.env_id == "env-0002").unwrap();
    assert_eq!(host_b.ssh_alias, "env-0002.ona.environment");

    let wakes = watch::queue::pending(&state).unwrap();
    assert_eq!(
        wakes.iter().map(|wake| wake.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    for wake in &wakes[1..3] {
        let Observation::ObservedChanged { title, .. } = &wake.obs else {
            panic!("{:?}", wake.obs);
        };
        assert_eq!(title, "");
    }
    let migrated = snapshot(&root);
    let watch_state = Path::new("supervisor/watch-state.json");
    assert_eq!(migrated[watch_state], old[watch_state]);
    assert!(migrated[Path::new("supervisor/wake-queue.jsonl")].ends_with(b"\"2026-10-28T"));

    let second = migrate(&root, &[]);
    assert!(
        second.lines().all(|line| line.starts_with("0 ")),
        "{second}"
    );
    assert_eq!(snapshot(&root), migrated);
}
