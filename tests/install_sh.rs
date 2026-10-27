use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REPO: &str = env!("CARGO_MANIFEST_DIR");

/// A fake `cargo` that logs its arguments and, like cargo's fingerprinting, rewrites the binary
/// only when the checkout's HEAD differs from the one it was last built at.
const FAKE_CARGO: &str = r#"#!/bin/sh
echo "$*" >> "$CARGO_LOG"
while [ $# -gt 0 ]; do
  case $1 in
    --manifest-path) manifest=$2; shift ;;
    --target-dir) target=$2; shift ;;
  esac
  shift
done
head=$(git -C "$(dirname "$manifest")" rev-parse HEAD)
mkdir -p "$target/release"
if [ "$(cat "$target/release/flow" 2>/dev/null)" != "$head" ]; then
  printf '%s' "$head" > "$target/release/flow"
  chmod +x "$target/release/flow"
fi
"#;

struct Fixture {
    dir: tempfile::TempDir,
    /// A bare repository the clone pulls from, on its own `fixture` branch.
    origin: PathBuf,
    flow_dir: PathBuf,
    state_dir: PathBuf,
}

impl Fixture {
    /// A clone of this repo's HEAD tracking a `fixture` branch in a bare origin of its own, so
    /// the checkout's branch, or a detached HEAD, does not matter; and a state directory path
    /// that does not exist yet.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let flow_dir = dir.path().join("flow");
        let head = git_stdout(Path::new(REPO), &["rev-parse", "HEAD"]);
        git(
            dir.path(),
            &["clone", "--quiet", "--bare", REPO, "origin.git"],
        );
        git(&origin, &["update-ref", "refs/heads/fixture", head.trim()]);
        git(&origin, &["symbolic-ref", "HEAD", "refs/heads/fixture"]);
        git(
            dir.path(),
            &[
                "clone",
                "--quiet",
                "--branch",
                "fixture",
                "origin.git",
                "flow",
            ],
        );
        let state_dir = dir.path().join("efs/flow/state");
        Fixture {
            dir,
            origin,
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

    /// Runs this checkout's `scripts/install.sh` on the clone, with `path` as `$PATH` and the
    /// build step skipped.
    fn install(&self, path: &str) -> Output {
        self.script(path)
            .env("FLOW_SKIP_BUILD", "1")
            .output()
            .unwrap()
    }

    /// The install script on the clone, with `path` as `$PATH`.
    fn script(&self, path: &str) -> Command {
        let mut cmd = Command::new(find_on(&system_path(), "bash"));
        cmd.arg(Path::new(REPO).join("scripts/install.sh"))
            .env("FLOW_DIR", &self.flow_dir)
            .env("FLOW_STATE_DIR", &self.state_dir)
            .env_remove("FLOW_SKIP_BUILD")
            .env("PATH", path);
        cmd
    }

    /// A directory holding the fake `cargo`, which logs to `cargo.log`.
    fn fake_cargo(&self) -> PathBuf {
        let bin = self.dir.path().join("fake-bin");
        fs::create_dir_all(&bin).unwrap();
        let cargo = bin.join("cargo");
        fs::write(&cargo, FAKE_CARGO).unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn cargo_log(&self) -> PathBuf {
        self.dir.path().join("cargo.log")
    }

    fn cargo_calls(&self) -> usize {
        fs::read_to_string(self.cargo_log())
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// Pushes a commit to the origin's `fixture` branch, committed at `date`.
    fn push_commit_dated(&self, date: &str) {
        let work = self.dir.path().join("work");
        git(
            self.dir.path(),
            &[
                "clone",
                "--quiet",
                "--branch",
                "fixture",
                "origin.git",
                "work",
            ],
        );
        fs::write(work.join("NOTES"), "a later change\n").unwrap();
        git(&work, &["add", "NOTES"]);
        let status = Command::new("git")
            .current_dir(&work)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
            ])
            .args(["commit", "--quiet", "-m", "add notes"])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .status()
            .unwrap();
        assert!(status.success(), "committing in {}", work.display());
        git(&work, &["push", "--quiet", "origin", "fixture"]);
        assert!(self.origin.is_dir());
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} in {}", dir.display());
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} in {}", dir.display());
    String::from_utf8(out.stdout).unwrap()
}

/// The first `program` in the `:`-separated `path`.
fn find_on(path: &str, program: &str) -> PathBuf {
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{program} is not on PATH"))
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

#[test]
fn install_sh_rebuilds_after_pulling_an_older_dated_commit() {
    let fx = Fixture::new();
    let path = format!("{}:{}", fx.fake_cargo().display(), system_path());
    let run = || {
        let out = fx
            .script(&path)
            .env("CARGO_LOG", fx.cargo_log())
            .output()
            .unwrap();
        lines(&out)
    };

    assert!(run().contains(&"install: build: done".to_owned()));
    assert!(run().contains(&"install: build: already".to_owned()));
    assert_eq!(fx.cargo_calls(), 2, "cargo decides staleness on every run");

    // Committed long before the binary was built, as a commit made elsewhere and pushed later.
    fx.push_commit_dated("2001-01-01T00:00:00Z");
    let out = run();

    assert!(out.contains(&"install: pull: done".to_owned()), "{out:?}");
    assert!(out.contains(&"install: build: done".to_owned()), "{out:?}");
    assert_eq!(fx.cargo_calls(), 3);
    let built = fs::read_to_string(fx.bin_dir().join("flow")).unwrap();
    assert_eq!(
        built,
        git_stdout(&fx.flow_dir, &["rev-parse", "HEAD"]).trim()
    );
}

#[test]
fn install_sh_refuses_to_build_without_cargo() {
    let fx = Fixture::new();
    let bin = fx.dir.path().join("no-cargo-bin");
    fs::create_dir(&bin).unwrap();
    for tool in ["git", "grep", "mkdir", "readlink", "ln", "cksum"] {
        symlink(find_on(&system_path(), tool), bin.join(tool)).unwrap();
    }

    let out = fx.script(bin.to_str().unwrap()).output().unwrap();

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "install: build: cargo not found; install a Rust toolchain (https://rustup.rs) or set \
         FLOW_SKIP_BUILD=1\n"
    );
    assert!(!fx.bin_dir().exists());
}

#[test]
fn install_sh_links_a_relative_state_dir_absolutely() {
    let fx = Fixture::new();
    let run = || {
        let out = fx
            .script(&system_path())
            .env("FLOW_SKIP_BUILD", "1")
            .env("FLOW_STATE_DIR", "efs/flow/state/")
            .current_dir(fx.dir.path())
            .output()
            .unwrap();
        lines(&out)
    };

    let first = run();
    let second = run();

    assert!(
        first.contains(&"install: symlink: done".to_owned()),
        "{first:?}"
    );
    assert!(
        second.contains(&"install: symlink: already".to_owned()),
        "{second:?}"
    );
    assert_eq!(fs::read_link(fx.link()).unwrap(), fx.state_dir);
    assert!(fx.link().join(".").is_dir());
}
