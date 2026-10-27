use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use assert_cmd::Command;
use serde_json::{Value, json};

const ENV_ID: &str = "env-0001";
const EXISTING: &str = "# edited by hand\n";

const LOADAVG: &str = "0.26 0.38 0.68 2/1501 1003799\n";
const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;

const CHECKLIST: [&str; 5] = [
    "init: still yours to provide:",
    "  - herdr installed, with `herdr integration install claude`",
    "  - Claude Code installed and logged in on every host",
    "  - `gitpod login` and `gitpod environment ssh-config` on the supervisor's machine",
    "  - `gh auth login` on hosts where PR listing matters (optional)",
];

const HOSTS_BLOCK: &str = r#"
[hosts]
source = "ona"
# Ona environment ids that are never hosts:
# exclude = ["<environment id>"]
# Host names to use in place of the slug of an environment's Ona name:
# names = { "<environment id>" = "<host name>" }
"#;

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        for sub in ["workspaces", "bin"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        Fixture { dir, state }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn config_path(&self) -> PathBuf {
        self.state.join("config.toml")
    }

    fn config(&self) -> String {
        fs::read_to_string(self.config_path()).unwrap()
    }

    /// A git repository at `workspaces/<name>`, its `origin/HEAD` pointing at `branch` if given.
    fn repo(&self, name: &str, branch: Option<&str>) -> PathBuf {
        let path = self.path("workspaces").join(name);
        git(&["init", "--quiet", path.to_str().unwrap()]);
        if let Some(branch) = branch {
            let target = format!("refs/remotes/origin/{branch}");
            git(&[
                "-C",
                path.to_str().unwrap(),
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                &target,
            ]);
        }
        path
    }

    /// An executable `wt` on the PATH `flow init` searches.
    fn install_wt(&self) {
        let wt = self.path("bin").join("wt");
        fs::write(&wt, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&wt, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The Ona listing: this machine's environment, running and checked out at `content`.
    fn write_ona(&self, phase: &str, content: Option<&Path>) {
        let content = content.map_or_else(
            || json!({}),
            |path| json!({"contentLocationInMachine": path}),
        );
        let listing = json!([{
            "id": ENV_ID,
            "metadata": {"name": "Primary"},
            "status": {"phase": phase, "content": content},
        }]);
        fs::write(self.path("ona.json"), listing.to_string()).unwrap();
    }

    /// `flow` as the machine `env-0001`, answering runner calls from `script`.
    fn flow(&self, script: &[Value]) -> Command {
        let runner = self.path("runner.json");
        fs::write(&runner, Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", ENV_ID)
            .env("FLOW_FAKE_RUNNER", runner)
            .env("FLOW_FAKE_ONA", self.path("ona.json"))
            .env("FLOW_INIT_WORKSPACES", self.path("workspaces"))
            .env("FLOW_INIT_PATH", self.path("bin"));
        cmd
    }
}

fn git(args: &[&str]) {
    let status = Process::new("git").args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn on(host: &str, stdout: &str) -> Value {
    json!({"host": host, "status": 0, "stdout": stdout})
}

fn init_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("init: "))
        .collect()
}

fn assert_ends_with_checklist(stdout: &str) {
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.ends_with(&CHECKLIST), "{stdout}");
}

#[test]
fn init_writes_config_from_the_local_machine() {
    let fx = Fixture::new();
    let repo = fx.repo("example-repo", Some("trunk"));
    // The Ona listing wins over scanning the workspaces directory.
    fx.repo("other-repo", None);
    fx.install_wt();
    fx.write_ona("ENVIRONMENT_PHASE_RUNNING", Some(&repo));
    let repo_path = repo.display().to_string();
    let porcelain = format!(
        "worktree {repo_path}\nHEAD {}\nbranch refs/heads/trunk\n\n",
        "1".repeat(40)
    );
    let wt_list = json!({"repo": repo_path, "worktrees": []}).to_string();
    let script = [
        on("primary", &porcelain),
        on("primary", &wt_list),
        on("primary", LOADAVG),
        on("primary", SERVER_RUNNING),
        on("primary", NO_AGENTS),
        on("primary", NO_AGENTS),
        on("primary", "[]"),
    ];

    let out = fx
        .flow(&script)
        .arg("init")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "{stderr}");
    let wrote = format!("init: wrote {}", fx.config_path().display());
    assert_eq!(
        init_lines(&stdout)[..6],
        [
            format!("init: repos.path = {repo_path} (Ona content location of $ONA_ID)").as_str(),
            "init: repos.name = example-repo (the path's directory name)",
            "init: repos.default_branch = trunk (origin/HEAD)",
            "init: repos.worktree_tool = wt (wt is on PATH)",
            "init: hosts.source = ona (hosts are discovered from Ona)",
            wrote.as_str(),
        ],
        "{stdout}"
    );
    assert_eq!(
        fx.config(),
        format!(
            "# flow operator config, written by `flow init`.\n\
             \n\
             [[repos]]\n\
             name = \"example-repo\"\n\
             path = \"{repo_path}\"\n\
             default_branch = \"trunk\"\n\
             worktree_tool = {{ create = [\"wt\", \"create\", \"{{branch}}\"], list = [\"wt\", \"--json\", \"list\"] }}\n\
             {HOSTS_BLOCK}"
        )
    );
    assert!(
        stdout.contains("\nprimary  reachable  load 0.26  1 worktrees  0 agents  (seen "),
        "the inventory refresh ran and printed: {stdout}"
    );
    let cached: Value =
        serde_json::from_str(&fs::read_to_string(fx.state.join("hosts/env-0001.json")).unwrap())
            .unwrap();
    assert_eq!(cached["name"], "primary");
    assert_eq!(cached["reachable"], true);
    assert_ends_with_checklist(&stdout);
}

#[test]
fn init_refuses_to_overwrite() {
    let fx = Fixture::new();
    fs::write(fx.config_path(), EXISTING).unwrap();

    let out = fx
        .flow(&[])
        .arg("init")
        .assert()
        .code(2)
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "state/config.toml exists; pass --force to overwrite\n"
    );
    assert!(out.stdout.is_empty());
    assert_eq!(fx.config(), EXISTING);
    assert!(!fx.state.join("hosts").exists());
}

#[test]
fn init_force_overwrites() {
    let fx = Fixture::new();
    fs::write(fx.config_path(), EXISTING).unwrap();
    fx.install_wt();
    fx.write_ona("ENVIRONMENT_PHASE_STOPPED", None);

    let out = fx
        .flow(&[])
        .args(["init", "--force"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let wrote = format!("init: wrote {}", fx.config_path().display());
    assert_eq!(
        init_lines(&stdout)[..3],
        [
            "init: repos = none found; edit state/config.toml",
            "init: hosts.source = ona (hosts are discovered from Ona)",
            wrote.as_str(),
        ],
        "{stdout}"
    );
    assert_eq!(
        fx.config(),
        format!(
            "# flow operator config, written by `flow init`.\n\
             \n\
             # No repository found. Describe each one tasks can get worktrees of:\n\
             # [[repos]]\n\
             # name = \"example-repo\"\n\
             # path = \"/workspaces/example-repo\"\n\
             # default_branch = \"main\"\n\
             # worktree_tool = {{ create = [\"wt\", \"create\", \"{{branch}}\"], list = [\"wt\", \"--json\", \"list\"] }}\n\
             {HOSTS_BLOCK}"
        )
    );
    let cached: Value =
        serde_json::from_str(&fs::read_to_string(fx.state.join("hosts/env-0001.json")).unwrap())
            .unwrap();
    assert_eq!(cached["name"], "primary");
    assert_eq!(cached["reachable"], false);
    assert_ends_with_checklist(&stdout);
}

#[test]
fn init_omits_worktree_tool_without_wt() {
    let fx = Fixture::new();
    // No Ona listing: init falls back to the one repository under the workspaces directory,
    // skipping a plain directory and the flow checkout itself.
    let repo = fx.repo("example-repo", None);
    fs::create_dir(fx.path("workspaces/notes")).unwrap();
    symlink(env!("CARGO_MANIFEST_DIR"), fx.path("workspaces/flow")).unwrap();
    let repo_path = repo.display().to_string();

    let out = fx
        .flow(&[])
        .arg("init")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let workspaces = fx.path("workspaces").display().to_string();
    let wrote = format!("init: wrote {}", fx.config_path().display());
    assert_eq!(
        init_lines(&stdout)[..6],
        [
            format!("init: repos.path = {repo_path} (the only git repository in {workspaces})")
                .as_str(),
            "init: repos.name = example-repo (the path's directory name)",
            "init: repos.default_branch = main (fallback; origin/HEAD is not set)",
            "init: repos.worktree_tool = none (wt is not on PATH; flow will use git worktree)",
            "init: hosts.source = ona (hosts are discovered from Ona)",
            wrote.as_str(),
        ],
        "{stdout}"
    );
    assert_eq!(
        fx.config(),
        format!(
            "# flow operator config, written by `flow init`.\n\
             \n\
             # no worktree tool found; flow will use git worktree\n\
             [[repos]]\n\
             name = \"example-repo\"\n\
             path = \"{repo_path}\"\n\
             default_branch = \"main\"\n\
             {HOSTS_BLOCK}"
        )
    );
    assert_ends_with_checklist(&stdout);
}
