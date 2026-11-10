//! `scripts/install`, run for real (frontend and release build included) into a temp target.
//! The plugin's marketplace lands in `claude` beside the target bin directory.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use common::TestStore;
use serde_json::Value;

fn install(dest: &Path) {
    Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install"))
        .arg(dest)
        .timeout(Duration::from_secs(600))
        .assert()
        .success();
}

#[test]
// One test, so two installs never run `npm ci` in `web/` at once.
fn installs_binaries_and_hooks_plugin_and_replaces_them_under_a_running_flowd() {
    let ts = TestStore::new();
    let root = ts.scratch("dest");
    let dest = root.join("bin");

    install(&dest);
    for bin in ["flow", "flowd"] {
        Command::new(dest.join(bin))
            .arg("--help")
            .assert()
            .success();
    }

    let old = ts.flowd_at(&dest.join("flowd"));
    assert_eq!(old.get("/api/sessions").0, 200);
    let old_inode = fs::metadata(dest.join("flowd")).unwrap().ino();

    install(&dest);

    // The running flowd still serves, and the path now holds a new file it never opened.
    assert_eq!(old.get("/api/sessions").0, 200);
    assert_ne!(fs::metadata(dest.join("flowd")).unwrap().ino(), old_inode);
    let new = ts.flowd_at(&dest.join("flowd"));
    assert_eq!(new.get("/api/sessions").0, 200);
    let mut left: Vec<_> = fs::read_dir(&dest)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["flow", "flowd"], "no temp files are left behind");

    // The plugin marketplace sits beside the bin directory, replaced with no temp left over.
    let mut left: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["bin", "claude"], "no temp directories are left behind");

    let marketplace = root.join("claude");
    let manifest = read_json(&marketplace.join(".claude-plugin/marketplace.json"));
    assert_eq!(manifest["name"], "flow");
    let entry = &manifest["plugins"][0];
    assert_eq!(entry["name"], "flow");
    let plugin = marketplace.join(entry["source"].as_str().unwrap());

    let hooks = read_json(&plugin.join("hooks/hooks.json"));
    let hooks = hooks["hooks"].as_object().unwrap();
    let mut events: Vec<_> = hooks.keys().map(String::as_str).collect();
    events.sort();
    assert_eq!(
        events,
        [
            "Notification",
            "PermissionRequest",
            "PostToolUse",
            "SessionEnd",
            "SessionStart",
            "Stop",
            "StopFailure",
            "UserPromptSubmit",
        ]
    );
    for (event, groups) in hooks {
        let hook = &groups[0]["hooks"][0];
        assert_eq!(hook["command"], "/efs-home/flow/bin/flow hook || true", "{event}");
        let is_async = hook["async"].as_bool().unwrap_or(false);
        assert_eq!(is_async, event == "PostToolUse", "{event} async");
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}
