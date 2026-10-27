use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REPO: &str = env!("CARGO_MANIFEST_DIR");

struct Fixture {
    _dir: tempfile::TempDir,
    flow_dir: PathBuf,
    state_dir: PathBuf,
}

impl Fixture {
    /// A fresh clone of this repo, and a state directory path that does not exist yet.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let flow_dir = dir.path().join("flow");
        let status = Command::new("git")
            .args(["clone", "--quiet", REPO])
            .arg(&flow_dir)
            .status()
            .unwrap();
        assert!(status.success(), "cloning {REPO}");
        let state_dir = dir.path().join("efs/flow/state");
        Fixture {
            _dir: dir,
            flow_dir,
            state_dir,
        }
    }

    fn link(&self) -> PathBuf {
        self.flow_dir.join("state")
    }

    fn bin_dir(&self) -> PathBuf {
        self.flow_dir.join("target/release")
    }

    /// Runs this checkout's `scripts/install.sh` on the clone, with `path` as `$PATH`.
    fn install(&self, path: &str) -> Output {
        Command::new("bash")
            .arg(Path::new(REPO).join("scripts/install.sh"))
            .env("FLOW_DIR", &self.flow_dir)
            .env("FLOW_STATE_DIR", &self.state_dir)
            .env("FLOW_SKIP_BUILD", "1")
            .env("PATH", path)
            .output()
            .unwrap()
    }
}

fn system_path() -> String {
    std::env::var("PATH").unwrap()
}

fn lines(out: &Output) -> Vec<String> {
    assert!(
        out.status.success(),
        "install.sh failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn install_sh_is_idempotent() {
    let fx = Fixture::new();

    let first = lines(&fx.install(&system_path()));

    let hint = format!("install: PATH: add {} to your PATH", fx.bin_dir().display());
    assert_eq!(
        first,
        [
            "install: pull: already",
            "install: state dir: done",
            "install: symlink: done",
            hint.as_str(),
        ]
    );
    assert!(fx.state_dir.is_dir());
    assert_eq!(fs::read_link(fx.link()).unwrap(), fx.state_dir);

    let on_path = format!("{}:{}", fx.bin_dir().display(), system_path());
    let second = lines(&fx.install(&on_path));

    assert_eq!(
        second,
        [
            "install: pull: already",
            "install: state dir: already",
            "install: symlink: already",
        ]
    );
    assert_eq!(fs::read_link(fx.link()).unwrap(), fx.state_dir);
}

#[test]
fn install_sh_replaces_a_wrong_symlink_and_refuses_a_directory() {
    let fx = Fixture::new();
    fs::create_dir_all(&fx.state_dir).unwrap();
    symlink("/nonexistent/elsewhere", fx.link()).unwrap();

    let out = lines(&fx.install(&system_path()));

    assert!(
        out.contains(&"install: symlink: done".to_owned()),
        "{out:?}"
    );
    assert_eq!(fs::read_link(fx.link()).unwrap(), fx.state_dir);

    fs::remove_file(fx.link()).unwrap();
    fs::create_dir(fx.link()).unwrap();
    fs::write(fx.link().join("config.toml"), "kept\n").unwrap();

    let out = fx.install(&system_path());

    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "install: {} is a real directory, not a symlink; move its contents to {} and remove it\n",
            fx.link().display(),
            fx.state_dir.display()
        )
    );
    assert!(fx.link().is_dir() && !fx.link().is_symlink());
    assert_eq!(
        fs::read_to_string(fx.link().join("config.toml")).unwrap(),
        "kept\n"
    );
}
