//! `scripts/install`, run for real (frontend and release build included) into a temp target.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use common::TestStore;

fn install(dest: &Path) {
    Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install"))
        .arg(dest)
        .timeout(Duration::from_secs(600))
        .assert()
        .success();
}

#[test]
fn installs_both_binaries_and_replaces_them_under_a_running_flowd() {
    let ts = TestStore::new();
    let dest = ts.scratch("dest").join("bin");

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
}
