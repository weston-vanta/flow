# flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `flow`, a Rust CLI plus Claude Code instructions that turn one herdr-hosted Claude Code session into a supervisor that dispatches, watches, and reports on worker sessions across the user's Ona CDEs, with all durable state on EFS.

**Architecture:** One binary, `flow`, owns a plain-file state directory (`state/`, a symlink to EFS) and three adapters: a command runner that executes locally or over ssh to another CDE, a herdr adapter for panes and agent state, and a Claude Code harness adapter for launching workers and receiving their hooks. `flow watch` and `flow guard` are Stop hooks of the supervisor session; workers report through hooks and `flow task` subcommands that append events. The supervisor's instructions live in `CLAUDE.md` and skills in the same repo.

**Tech Stack:** Rust 1.96 (edition 2024), `clap` (derive), `serde`/`serde_json`/`toml`, `anyhow`, `jiff` (time), `rand`; dev: `assert_cmd`, `predicates`, `tempfile`. External processes: `ssh`, `herdr`, `claude`, `wt`, `gh`, `git`.

**Spec:** `/efs-home/flow/docs/2026-10-26-flow-design.md` (copied into the repo as `docs/superpowers/specs/2026-10-26-flow-design.md` in Task 1).

## Global Constraints

- Repo `weston-vanta/flow` is public; `main` is reset to a fresh history. Nothing committed may name the user's employer, monorepo, tickets, PRs, hosts, or environment ids. Tests use invented names (`cde-a`, `cde-b`, `example-repo`).
- Clone lives at `/workspaces/flow`; durable state at `/efs-home/flow/state`, reached via symlink `/workspaces/flow/state`. `state/` is gitignored. The binary resolves the state dir as `$FLOW_STATE` if set, else `<repo root>/state`.
- The supervisor always runs with cwd `/workspaces/flow`.
- Agents read and write state only through the CLI. Every read command renders a compact view; raw JSON only behind `--json`.
- No undocumented harness internals. Allowed: Claude Code CLI flags, hooks (via `--settings` files or the repo's `.claude/settings.json`), read-only reads of `~/.claude/sessions/*.json` and `~/.claude/projects/<slug>/<id>.jsonl`. Never write into `~/.claude`.
- Everything that runs on another CDE goes through the runner as `ssh <alias> zsh -lc '<command>'`. Nothing polls over ssh in the steady state except `herdr agent wait`, which blocks.
- Files on EFS are written atomically (temp file in the same directory, then rename) under a mkdir lock directory `<path>.lock`. Readers reopen files on every poll; inotify is not used (NFS does not deliver remote events).
- Status vocabulary, exactly: `pending`, `working`, `blocked`, `needs-decision`, `done`, `orphaned`.
- Task ids: 8 lowercase base32 characters using the ASCII-sorted alphabet `234567abcdefghijklmnopqrstuvwxyz`, so string order equals time order: 6 encoding seconds since 2026-01-01T00:00:00Z, big-endian, then 2 random. Never reused.
- herdr: named session `flow` on every CDE; minimum protocol 22; one workspace per task labeled with the task id. Every herdr call passes `--session flow`.
- Supervisor Stop hooks: `flow guard` (blocking) and `flow watch` (`asyncRewake: true`, `timeout: 7200`). Watch heartbeat default 3600 s, always less than the hook timeout.
- No `flow doctor`. Login failures surface as plain error text from the command that hit them.

## Review Focus

1. A worker CDE's ssh alias stops resolving mid-watch (login expired). Expected: `flow watch` reports `cde-unreachable: <cde>` once, does not mark tasks orphaned by itself, and keeps watching the others. Test in Task 11.
2. Two `flow watch` processes start from overlapping Stop hook firings. Expected: the second exits 0 immediately after finding a live lock holder; the first keeps running. Test in Task 11.
3. The state directory symlink is missing or EFS is not mounted. Expected: every command fails with one line naming the expected path, exit 3, and writes nothing. Test in Task 1.
4. A worker writes `done` while the supervisor is mid-turn. Expected: the event is appended, the watcher's queue records it, and the supervisor sees it on its next drain even if the watcher restarted in between. Test in Task 11.
5. Dispatch targets a worktree whose cwd matches a live herdr agent. Expected: refusal with the agent's name and `--allow-shared-worktree` named as the override; no task directory created. Test in Task 8.

---

### Task 1: Repo reset and binary skeleton

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/cli.rs`, `src/paths.rs`, `.gitignore`, `README.md`, `docs/superpowers/specs/2026-10-26-flow-design.md`, `docs/superpowers/plans/2026-10-26-flow-implementation-plan.md`
- Test: `tests/cli_paths.rs`

**Interfaces:**
- Produces: `paths::StateDir::resolve(repo_root: &Path) -> Result<StateDir, PathError>`; `StateDir::root(&self) -> &Path`; `PathError::Missing(PathBuf)` rendered as `state directory not found: <path> (expected a symlink to /efs-home/flow/state)`; exit code 3 for `PathError`.

- [ ] **Step 1: Reset the repo.** Clone `weston-vanta/flow` to `/workspaces/flow`, `git checkout --orphan main-reset`, `git rm -rf .`, `cargo init --name flow`, commit `chore: start flow from an empty tree`, then `git branch -M main` and `git push --force-with-lease origin main`. (Force push is authorized by the design, decision 12.)
- [ ] **Step 2: Copy the two docs** from `/efs-home/flow/docs/` into `docs/superpowers/specs/` and `docs/superpowers/plans/`. Add `.gitignore` with `/target`, `/state`.
- [ ] **Step 3: Write the failing test** `tests/cli_paths.rs::missing_state_dir_exits_3`: run the built binary with `FLOW_STATE=/nonexistent/path status`; assert exit code 3 and stderr contains `state directory not found: /nonexistent/path`.
- [ ] **Step 4: Run** `cargo test --test cli_paths`; expected: FAIL (no `status` subcommand).
- [ ] **Step 5: Implement** `cli.rs` with clap derive: subcommands `status`, `task`, `dispatch`, `cde`, `inventory`, `sessions`, `watch`, `wake`, `guard`, `hook`, `up`, `attach` as stubs returning `anyhow::bail!("not implemented")` except `status`, which resolves `StateDir` and prints `state: <path>`. Implement `paths.rs` per Interfaces. Map `PathError` to exit 3 in `main.rs`.
- [ ] **Step 6: Run** `cargo test`; expected: PASS.
- [ ] **Step 7: Commit** `feat: flow binary skeleton with state dir resolution`.

### Task 2: State layer

**Files:**
- Create: `src/state/mod.rs`, `src/state/fs.rs`, `src/state/ids.rs`, `src/state/task.rs`, `src/state/events.rs`, `src/state/config.rs`
- Test: `src/state/fs.rs` (unit), `src/state/ids.rs` (unit), `tests/state_tasks.rs`

**Interfaces:**
- Produces:
  - `fs::write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>`; `fs::with_lock<T>(path: &Path, f: impl FnOnce() -> io::Result<T>) -> io::Result<T>` (mkdir `<path>.lock`, retry every 50 ms up to 10 s, stale lock older than 60 s is removed).
  - `ids::new_task_id(now: jiff::Timestamp, rng: &mut impl rand::Rng) -> String`.
  - `task::TaskRecord { id, parent: Option<String>, title, cde, worktree: PathBuf, harness: Harness, created: Timestamp, owner: String }`; `Harness::Claude`.
  - `task::Status { state: State, last_event: Timestamp, hold: Option<Hold>, herdr: Option<HerdrTarget> }`; `State` enum of the six states; `Hold { on: HoldOn, what: String, since: Timestamp }`; `HoldOn::{User, Supervisor}`; `HerdrTarget { cde: String, workspace_id: String, pane_id: String, agent: String }`.
  - `events::Event { ts, source: String, kind: String, data: serde_json::Value }`; `events::append(dir: &Path, ev: &Event) -> Result<()>`; `events::read_all(dir: &Path) -> Result<Vec<Event>>`.
  - `Store::open(state: &StateDir) -> Store`; `Store::create_task(&self, rec: TaskRecord, brief: &str) -> Result<()>` (writes `task.json`, `status.json` with `pending`, `brief.md`, first event `task.created`); `Store::task(&self, id) -> Result<(TaskRecord, Status)>`; `Store::list_tasks(&self) -> Result<Vec<(TaskRecord, Status)>>`; `Store::set_status(&self, id, f: impl FnOnce(&mut Status)) -> Result<()>` (locked read-modify-write, appends event `status` with `{from, to}` when `state` changes).
  - `config::Config { cdes: Vec<Cde>, monorepo: Monorepo, worktree_tool: WorktreeTool }`; `Cde { name, env_id, ssh_alias, local_marker_env: Option<(String, String)> }`; `Monorepo { path: PathBuf, default_branch: String }`; `WorktreeTool { create: Vec<String> }` (argv template with `{branch}`); `Config::load(state: &StateDir) -> Result<Config>` from `state/config.toml`.

- [ ] **Step 1: Write failing unit tests** in `ids.rs`: `ids_are_8_base32_chars`, `ids_sort_by_time` (two timestamps one second apart produce ids with `a < b`). In `fs.rs`: `write_atomic_leaves_no_temp_file`, `with_lock_serializes` (two threads incrementing a counter file 100 times each end at 200), `stale_lock_is_broken` (pre-create `<path>.lock` with mtime 120 s old; `with_lock` succeeds).
- [ ] **Step 2: Run** `cargo test state::`; expected: FAIL (modules missing).
- [ ] **Step 3: Implement** `fs.rs` and `ids.rs` per Interfaces.
- [ ] **Step 4: Run** `cargo test state::`; expected: PASS.
- [ ] **Step 5: Write failing integration tests** `tests/state_tasks.rs`: `create_then_read_roundtrip` (fields equal, status `pending`, `brief.md` content equal, exactly one event of kind `task.created`); `set_status_appends_transition_event` (pending → working appends `status` event with `data == {"from":"pending","to":"working"}`); `config_loads_sample` (write a sample `config.toml` with two CDEs `cde-a` and `cde-b`, monorepo `/workspaces/example-repo`, worktree tool `["wt","create","{branch}"]`; assert parsed values).
- [ ] **Step 6: Run** `cargo test --test state_tasks`; expected: FAIL.
- [ ] **Step 7: Implement** `task.rs`, `events.rs`, `config.rs`, `Store` in `mod.rs`. Serde with `rename_all = "kebab-case"` for enums so files read `needs-decision`.
- [ ] **Step 8: Run** `cargo test`; expected: PASS.
- [ ] **Step 9: Commit** `feat: plain-file state layer with atomic writes and task records`.

### Task 3: Read commands with compact rendering

**Files:**
- Create: `src/render.rs`, `src/cmd/status.rs`, `src/cmd/task_show.rs`, `src/cmd/task_list.rs`, `src/cmd/task_events.rs`
- Modify: `src/cli.rs`
- Test: `tests/cli_read.rs`

**Interfaces:**
- Consumes: `Store`, `Config` (Task 2).
- Produces: `render::relative(ts: Timestamp, now: Timestamp) -> String` (`3m`, `2h`, `5d`); `render::table(rows: &[Vec<String>]) -> String` (left-aligned columns, two-space gutter, no borders). Commands: `flow status [--json]` (one line per non-done task: `id  state  cde  age  title`, then `holds:` lines, then `adopted sessions:` placeholder count from Task 10), `flow task show <id> [--json]`, `flow task list [--all] [--json]`, `flow task events <id> [--last N]`.

- [ ] **Step 1: Write failing tests** `tests/cli_read.rs`: `status_lists_open_tasks_compactly` (create two tasks via `Store` in a temp state dir, one `done`; assert stdout has exactly one task line and it contains the id, `pending`, `cde-a`); `task_show_elides_brief` (brief of 2000 chars; stdout shows first 10 lines then `… (<N> more lines; flow task brief <id>)`); `task_events_last_limits` (append 30 events; `--last 5` prints 5 lines); `json_flag_prints_raw_json` (`flow task show <id> --json` parses as JSON with key `task`).
- [ ] **Step 2: Run** `cargo test --test cli_read`; expected: FAIL.
- [ ] **Step 3: Implement** `render.rs` and the four commands; add `flow task brief <id>` printing `brief.md` verbatim.
- [ ] **Step 4: Run** `cargo test`; expected: PASS.
- [ ] **Step 5: Commit** `feat: status, task show/list/events with compact rendering`.

### Task 4: Worker-side task commands

**Files:**
- Create: `src/cmd/task_write.rs`
- Modify: `src/cli.rs`
- Test: `tests/cli_task_write.rs`

**Interfaces:**
- Consumes: `Store::set_status`, `events::append`.
- Produces: `flow task status <id> <state> [--message TEXT]` (worker reports `working|blocked|needs-decision`; appends event kind `status.report` with `{message}` and transitions state; `needs-decision` and `blocked` set `hold = Some(Hold{on: Supervisor, what: message, since: now})`); `flow task done <id> --report <file>` (copies file to `report.md`, state `done`, event `report`); `flow task request-dispatch <id> --title T --brief <file> [--cde NAME]` (writes `requests/<new-id>.json` with `{title, brief_path, cde, requested_by: id}` and event `request.dispatch`); `flow task decide <id> --by user|supervisor --answer TEXT` (appends to `decisions.jsonl`, clears hold, event `decision`); `flow task hold <id> --on user --what TEXT` (sets hold, event `hold.set`); `flow task orphan <id> --reason TEXT` (supervisor-only: state `orphaned`, event `orphaned`); `flow requests [--json]` (lists every `requests/*.json` across tasks not yet consumed, one line `req-id  from-task  cde  title`; `flow requests consume <req-id>` renames it to `.consumed` after the supervisor dispatches it).

- [ ] **Step 1: Write failing tests**: `needs_decision_sets_supervisor_hold`, `done_copies_report_and_transitions`, `request_dispatch_writes_request_file` (file exists, JSON has `requested_by`), `decide_clears_hold_and_logs` (`decisions.jsonl` has one line with `by == "supervisor"`; `status.hold` is null), `requests_lists_then_consume_hides` , `orphan_transitions_with_reason_event`, `invalid_state_is_rejected` (`flow task status <id> finished` exits 2 listing the six valid states).
- [ ] **Step 2: Run**; expected: FAIL.
- [ ] **Step 3: Implement** `task_write.rs`.
- [ ] **Step 4: Run** `cargo test`; expected: PASS.
- [ ] **Step 5: Commit** `feat: worker-facing task status, done, decide, hold, request-dispatch`.

### Task 5: Command runner and CDE adapter

**Files:**
- Create: `src/runner.rs`, `src/cde.rs`, `src/cmd/cde.rs`
- Modify: `src/cli.rs`
- Test: `src/runner.rs` (unit, `FakeRunner`), `tests/cli_cde.rs`

**Interfaces:**
- Consumes: `Config`, `Cde` (Task 2).
- Produces: `trait Runner { fn run(&self, cde: &Cde, argv: &[String], stdin: Option<&[u8]>) -> Result<Output>; fn spawn(&self, cde: &Cde, argv: &[String]) -> Result<Child>; }`; `Output { status: i32, stdout: String, stderr: String }`. `LocalSshRunner::new(local_env_id: Option<String>)`: when `cde.env_id == local_env_id` runs argv directly, else runs `ssh -o BatchMode=yes -o ControlMaster=auto -o ControlPath=<state>/supervisor/ssh-%C -o ControlPersist=600 <alias> zsh -lc <shell-quoted argv>`. `FakeRunner` records `(cde, argv)` calls and returns scripted `Output`s in order. `cde::local_env_id() -> Option<String>` reads `$ONA_ID`. Test support: when `FLOW_FAKE_RUNNER=<file>` is set, `runner::from_env()` returns a `FakeRunner` whose scripted outputs are read from that JSON file (a list of `{status, stdout, stderr}`) and which appends every call's `{cde, argv}` to `<file>.calls.jsonl`; later tasks' CLI tests use this. Command: `flow cde list` (name, env id, `local` marker), `flow cde run <name> -- <argv...>` (exit code passes through).

- [ ] **Step 1: Write failing unit tests** in `runner.rs`: `local_cde_runs_directly` (argv built without `ssh`), `remote_cde_wraps_in_ssh_zsh_lc` (argv starts with `ssh`, contains alias, last element starts with `zsh -lc`), `shell_quoting_survives_spaces_and_quotes` (argv `["echo", "a b", "it's"]` round-trips through the quoting function to the same tokens when parsed by `sh -c 'printf "%s\n" ...'`).
- [ ] **Step 2: Run**; expected: FAIL.
- [ ] **Step 3: Implement** `runner.rs` with the command construction as a pure function `build_argv(cde, local_env_id, argv) -> Vec<String>` so it is testable without executing.
- [ ] **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Write failing test** `tests/cli_cde.rs::cde_run_local_echo` (config with one CDE whose env id equals `ONA_ID` set in the test env; `flow cde run cde-a -- echo hi` prints `hi`).
- [ ] **Step 6: Run**; expected: FAIL. **Step 7: Implement** `cmd/cde.rs`. **Step 8: Run**; expected: PASS.
- [ ] **Step 9: Commit** `feat: runner that executes locally or over ssh, flow cde list/run`.

### Task 6: herdr adapter

**Files:**
- Create: `src/herdr.rs`, `tests/fixtures/herdr/agent_list.json`, `tests/fixtures/herdr/workspace_list.json`, `tests/fixtures/herdr/agent_get.json`
- Test: `src/herdr.rs` (unit against fixtures through `FakeRunner`)

**Interfaces:**
- Consumes: `Runner`, `Cde`.
- Produces: `Herdr::new(runner: Arc<dyn Runner>)`; `Herdr::ensure_server(&self, cde) -> Result<()>` (`herdr --session flow status server`; if not running, `herdr --session flow server` detached via `spawn`, then poll `status` up to 10 s; fail if protocol < 22); `Herdr::agents(&self, cde) -> Result<Vec<Agent>>` where `Agent { pane_id, workspace_id, agent: String, status: AgentStatus, cwd: PathBuf, title: String }` and `AgentStatus::{Idle, Working, Blocked, Done, Unknown}`; `Herdr::create_workspace(&self, cde, cwd, label) -> Result<(workspace_id, pane_id)>`; `Herdr::start_agent(&self, cde, pane_id, name, kind: &str, args: &[String]) -> Result<()>` (`herdr --session flow agent start <name> --kind <kind> --pane <id> -- <args>`); `Herdr::wait(&self, cde, pane_id, until: &[AgentStatus], timeout_ms: Option<u64>) -> Result<AgentStatus>` (blocking); `Herdr::prompt(&self, cde, pane_id, text) -> Result<AgentStatus>` (`agent prompt ... --wait`); `Herdr::read(&self, cde, pane_id, lines) -> Result<String>`; `Herdr::attach_argv(cde, pane_id) -> Vec<String>` (for `flow attach`).

- [ ] **Step 1: Capture fixtures** on this CDE: `herdr agent list`, `herdr workspace list`, `herdr agent get <pane>` saved verbatim, then edit cwds and titles to invented names.
- [ ] **Step 2: Write failing unit tests**: `agents_parses_fixture` (two agents, statuses `idle` and `working`, cwd paths), `wait_passes_until_flags` (FakeRunner call argv contains `--until`, `blocked`, `--until`, `done`), `ensure_server_fails_on_old_protocol` (fixture with `private_protocol: 21` → error mentions `protocol 22`), `every_call_passes_session_flow` (all recorded argv contain `--session` then `flow`).
- [ ] **Step 3: Run**; expected: FAIL. **Step 4: Implement** `herdr.rs`. For `create_workspace`, run `herdr --session flow workspace create --cwd <cwd> --label <label> --no-focus` and parse the returned workspace id, then `workspace get` to find its first pane id; confirm the exact response fields against `herdr api schema` and record them in a comment above the parser. **Step 5: Run**; expected: PASS.
- [ ] **Step 6: Integration check (manual, not a test):** with `FLOW_HERDR_LIVE=1`, run a small `examples/herdr_smoke.rs` that creates a workspace in a temp dir, starts `--kind claude` with `--version`, waits for `done`, and closes the workspace. Record the output in the commit message.
- [ ] **Step 7: Commit** `feat: herdr adapter over the runner`.

### Task 7: Claude Code harness adapter and hooks

**Files:**
- Create: `src/harness/mod.rs`, `src/harness/claude.rs`, `src/harness/brief.rs`, `templates/brief.md`, `templates/claude-worker-settings.json`, `src/cmd/hook.rs`
- Modify: `src/cli.rs`
- Test: `src/harness/brief.rs` (unit), `tests/cli_hook.rs`

**Interfaces:**
- Consumes: `TaskRecord`, `Store`, `events::append`.
- Produces: `trait HarnessAdapter { fn prepare(&self, task: &TaskRecord, task_dir: &Path, flow_bin: &Path) -> Result<Launch>; fn kind(&self) -> &'static str; }`; `Launch { argv: Vec<String>, env: Vec<(String, String)> }`. `ClaudeAdapter::prepare` writes `<task_dir>/claude-settings.json` from the template with hooks: `SessionStart` → `flow hook claude --task <id>`, `Stop` → `flow hook claude --task <id>`, both `timeout: 30`; returns argv `["--settings", "<task_dir>/claude-settings.json", "--name", "<id>", "Read <task_dir>/brief.md and follow it exactly."]` with kind `claude` (the binary name is supplied by `herdr agent start --kind claude`). `brief::render(task: &TaskRecord, body: &str, criteria: &str, flow_bin: &Path) -> String` fills `templates/brief.md`, which contains: the task id, the worktree path and the rule to work only inside it, the user's ask (`body`), completion criteria (`criteria`), and the reporting contract: run `flow task status <id> working --message ...` at start and on each phase change, `flow task status <id> needs-decision --message <question>` then stop and wait for a reply in the terminal, `flow task status <id> blocked --message ...`, `flow task request-dispatch ...` for sibling work, and `flow task done <id> --report <path>` when the criteria are met. Workers never address the user. Command `flow hook claude --task <id>` reads hook JSON from stdin and appends event `hook.<hook_event_name lowercased>` with `{session_id, transcript_path, cwd}`; on `SessionStart` also writes `session_id` and `transcript_path` into `status.json` (`claude_session: Option<ClaudeSession>` added to `Status`).

- [ ] **Step 1: Write failing tests**: `brief_contains_id_worktree_and_criteria`, `brief_has_no_user_address` (rendered text does not contain the string `captain` or `the user said`), `prepare_writes_settings_with_two_hooks` (parse the JSON; `hooks.SessionStart[0].hooks[0].command` ends with `--task <id>`; same for `Stop`), `hook_session_start_records_session` (pipe `{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t/s1.jsonl","cwd":"/w"}`; status has `claude_session.session_id == "s1"`; one event `hook.sessionstart`).
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement.** **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Commit** `feat: Claude Code worker adapter, brief template, hook receiver`.

### Task 8: Dispatch

**Files:**
- Create: `src/cmd/dispatch.rs`, `src/worktree.rs`
- Modify: `src/cli.rs`, `src/cmd/status.rs` (show herdr target column)
- Test: `tests/cli_dispatch.rs` (uses `FLOW_FAKE_RUNNER` from Task 5)

**Interfaces:**
- Consumes: `Store`, `Config`, `Herdr`, `ClaudeAdapter`, `Runner`.
- Produces: `flow dispatch --cde NAME --title T (--brief-file F | --brief TEXT) --criteria TEXT (--worktree PATH | --branch NAME) [--parent ID] [--allow-shared-worktree]`. Sequence: resolve CDE; if `--branch`, run the worktree tool template on that CDE and take its printed path; list herdr agents on that CDE and refuse if any `cwd` equals the worktree unless `--allow-shared-worktree` (exit 2, message names the agent); create task (id from `ids`); `ensure_server`; `create_workspace(cwd=worktree, label=id)`; `start_agent(pane, name=id, kind="claude", args=Launch.argv)`; set status `working` with `herdr = Some(target)`; append `worker.started`; print `dispatched <id> on <cde> (<pane_id>)`. `worktree::create(runner, cde, tool, branch) -> Result<PathBuf>` (takes the last non-empty stdout line as the path).

- [ ] **Step 1: Write failing tests**: `dispatch_refuses_shared_worktree` (scripted agent list with cwd equal to `--worktree`; exit 2; no `state/tasks/*` created; message contains the agent name and `--allow-shared-worktree`), `dispatch_creates_task_and_starts_agent` (scripted outputs for status, agents (empty), workspace create, workspace get, agent start; asserts recorded argv includes `agent start <id> --kind claude --pane`, and task status is `working` with a herdr target), `dispatch_with_branch_runs_worktree_tool` (recorded argv contains `wt create feature-x`; task worktree equals the scripted path).
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement.** **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Commit** `feat: flow dispatch`.

### Task 9: Inventory

**Files:**
- Create: `src/cmd/inventory.rs`, `src/inventory.rs`
- Modify: `src/cli.rs`
- Test: `tests/cli_inventory.rs` (FakeRunner script)

**Interfaces:**
- Consumes: `Runner`, `Herdr`, `Config`.
- Produces: `inventory::CdeInventory { name, env_id, seen: Timestamp, reachable: bool, load1: Option<f32>, worktrees: Vec<Worktree { path, branch }>, agents: Vec<Agent>, prs: Vec<Pr { branch, number, url, state }> }`; `inventory::refresh(cde) -> Result<CdeInventory>` runs on the CDE: `git -C <monorepo> worktree list --porcelain` plus `wt list --json`, `cat /proc/loadavg`, herdr `agents`; then locally once: `gh pr list --author @me --state open --json number,url,headRefName,state --limit 100` and joins PRs to worktrees by branch. Writes `state/cdes/<env_id>.json`. Command `flow inventory [--refresh] [--json]` renders one block per CDE: `name  reachable  load  N worktrees  N agents`, then worktree lines `branch  path  [PR #n state]  [agent: name status]`.

- [ ] **Step 1: Write failing tests**: `refresh_parses_worktrees_load_and_joins_prs` (scripted outputs; one worktree branch `feature-x` matched to PR 12), `unreachable_cde_is_recorded_not_fatal` (scripted ssh failure for `cde-b`; exit 0; `cde-b` block shows `unreachable`), `inventory_without_refresh_reads_cache`.
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement.** **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Commit** `feat: per-CDE inventory`.

### Task 10: Adoption of hand-started sessions

**Files:**
- Create: `src/adopt.rs`, `src/cmd/sessions.rs`, `tests/fixtures/claude/sessions/1.json`, `tests/fixtures/claude/transcript.jsonl`
- Modify: `src/cli.rs`, `src/cmd/status.rs`
- Test: `src/adopt.rs` (unit), `tests/cli_sessions.rs`

**Interfaces:**
- Consumes: `CdeInventory.agents` (Task 9), `Store`.
- Produces: `adopt::registry_sessions(claude_home: &Path) -> Result<Vec<RegistrySession { session_id, cwd, name, status, updated: Timestamp, pid_domain }>>` (reads `<claude_home>/sessions/*.json`); `adopt::latest_summary(transcript: &Path) -> Result<Option<String>>` (last line whose JSON has `"subtype":"away_summary"`, its `content`; scans from the end, reads at most the last 1 MiB); `adopt::match_agents(agents: &[Agent], tasks: &[(TaskRecord, Status)], registry: &[RegistrySession]) -> Vec<Adopted { cde, agent, task: Option<String>, session: Option<RegistrySession>, summary: Option<String> }>` (agent ↔ task by `status.herdr.pane_id`; agent ↔ registry by `cwd` equality and most recent `updated`). Command `flow sessions [--json]`: one line per agent across CDEs: `cde  pane  status  task-or-'adopted'  title  summary(first 80 chars)`. `flow status` gains a trailing `adopted: N sessions` line. `flow follow <cde> <pane_id>` / `flow unfollow <cde> <pane_id>` maintain `state/supervisor/follow.json`, the list of adopted sessions whose state changes should wake the supervisor.

- [ ] **Step 1: Write failing tests**: `registry_parses_fixture`, `latest_summary_finds_last_away_summary` (fixture with two summaries; returns the later), `match_prefers_task_pane_over_cwd`, `sessions_lists_adopted_with_summary`.
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement.** **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Commit** `feat: flow sessions, adoption of herdr sessions`.

### Task 11: Watch and the wake queue

**Files:**
- Create: `src/watch/mod.rs`, `src/watch/classify.rs`, `src/watch/queue.rs`, `src/watch/lock.rs`, `src/cmd/watch.rs`, `src/cmd/wake.rs`
- Modify: `src/cli.rs`
- Test: `src/watch/classify.rs` (unit), `src/watch/queue.rs` (unit), `tests/cli_watch.rs`

**Interfaces:**
- Consumes: `Store`, `Herdr::wait`, `Herdr::agents`, `Runner`.
- Produces:
  - `classify::Observation` enum: `StatusChanged { task, from, to }`, `EventAppended { task, kind }`, `AgentState { task, status: AgentStatus }`, `AgentGone { task }`, `CdeUnreachable { cde }`, `RequestAppeared { task, request }`, `Quiet { task, since_secs, worktree_written: bool }`, `AdoptedChanged { cde, pane_id, status }`, `Heartbeat`.
  - `classify::classify(obs: &Observation, thresholds: &Thresholds) -> Verdict` with `Verdict::{Absorb, Wake(String)}`; `Thresholds { quiet_secs: u64 (default 1800) }`. Rules: `StatusChanged` to `done|blocked|needs-decision` → Wake(`<to>: <task>`); to `working` → Absorb; `EventAppended` kind `request.dispatch` → Wake(`request: <task>`); `AgentState` `Blocked|Done` → Wake(`agent-<status>: <task>`), `Working|Idle` → Absorb; `AgentGone` → Wake(`agent-gone: <task>`); `CdeUnreachable` → Wake(`cde-unreachable: <cde>`) once per cde per watch run; `Quiet` with `worktree_written` → Absorb, else if `since_secs >= quiet_secs` → Wake(`quiet: <task> <since>s`); `AdoptedChanged` → Wake(`adopted-<status>: <cde>/<pane>`) only when the pane is in `follow.json`, else Absorb; `Heartbeat` → Wake(`heartbeat`). Followed panes get a `Herdr::wait` like tasks do.
  - `queue::push(state, reason: &str, obs: &Observation) -> Result<u64>` appends to `state/supervisor/wake-queue.jsonl` with a sequence number; `queue::pending(state) -> Result<Vec<Wake>>`; `queue::ack_through(state, seq) -> Result<()>` writes `state/supervisor/wake-ack`.
  - `lock::acquire(state, pid) -> Result<Option<Guard>>` (mkdir `state/supervisor/watch.lock` containing pid and host; `None` if held by a live pid on this host; stale if pid dead).
  - `flow watch [--heartbeat SECS] [--once]`: acquire lock or exit 0 silently; load open tasks; spawn one `Herdr::wait(until=[blocked, done])` per task with a herdr target (through the runner; a failed spawn becomes `CdeUnreachable`); poll `state/tasks/*/status.json` and `events.jsonl` sizes every 2 s; emit `Quiet` for tasks whose last event is older than `quiet_secs` (worktree write check: newest mtime under the worktree via the runner, once per quiet task per 10 min); classify each observation; absorbed ones are logged to `state/supervisor/watch.log`; the first `Wake` is pushed to the queue, printed as the single stdout line, and the process exits 2. Heartbeat fires after `--heartbeat` seconds (default 3600). `--once` runs one poll cycle and exits 0 (for tests).
  - `flow wake drain`: prints pending wakes as `seq  age  reason` plus `WAKE_ACK_REQUIRED: flow wake ack --through <max seq>`; `flow wake ack --through N`.

- [ ] **Step 1: Write failing unit tests** in `classify.rs`, one per rule above, named `classify_<rule>` (at least 10 tests), plus `cde_unreachable_wakes_once_per_run`. In `queue.rs`: `push_increments_seq`, `pending_excludes_acked`.
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement** `classify.rs`, `queue.rs`, `lock.rs`. **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Write failing integration tests** `tests/cli_watch.rs`: `watch_exits_2_on_done_status` (start `flow watch` with no herdr tasks and a 200 ms poll override `FLOW_WATCH_POLL_MS=200`; from the test, `flow task done` on a task; assert exit code 2 within 5 s and stdout `done: <id>`; queue has one entry), `second_watch_exits_0_when_locked` (hold the lock dir with the test's own pid; `flow watch --once` exits 0 with empty stdout), `queued_wake_survives_restart` (push a wake, run `flow wake drain`, assert the entry prints and `WAKE_ACK_REQUIRED` line is present; `ack --through`; `drain` prints nothing), `unreachable_cde_reported_once` (FakeRunner script fails every call for `cde-b`; two tasks on `cde-b`; `watch` exits 2 with `cde-unreachable: cde-b`; the second `watch --once` after acking does not exit 2 for the same reason within one run).
- [ ] **Step 6: Run**; expected: FAIL. **Step 7: Implement** `cmd/watch.rs` and `cmd/wake.rs`. **Step 8: Run**; expected: PASS.
- [ ] **Step 9: Commit** `feat: flow watch with classification, durable wake queue, single-flight lock`.

### Task 12: Guard

**Files:**
- Create: `src/cmd/guard.rs`
- Modify: `src/cli.rs`
- Test: `tests/cli_guard.rs`

**Interfaces:**
- Consumes: `Store::list_tasks`, `watch::lock`, `queue::pending`.
- Produces: `flow guard`: reads the Stop hook JSON from stdin; exit 0 when there are no tasks in `working|blocked|needs-decision|pending`; otherwise exit 0 if the watch lock is held by a live pid (grace: retry for up to 5 s, since the async watch hook starts concurrently) or `stop_hook_active` is true in the input; else exit 2 printing `flow: work under way (<N> tasks) and no watcher is live; run \`flow watch\` in the background before ending the turn`. If `queue::pending` is non-empty, exit 2 printing `flow: <N> unhandled wakes; run flow wake drain`.

- [ ] **Step 1: Write failing tests**: `guard_allows_when_no_open_tasks`, `guard_blocks_without_watcher`, `guard_allows_when_watch_lock_live`, `guard_blocks_on_pending_wakes`, `guard_respects_stop_hook_active`.
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement.** **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Commit** `feat: flow guard turn-end backstop`.

### Task 13: Supervisor session: up, attach, settings, instructions

**Files:**
- Create: `.claude/settings.json`, `CLAUDE.md`, `.claude/skills/flow-dispatch/SKILL.md`, `.claude/skills/flow-wake/SKILL.md`, `.claude/skills/flow-escalate/SKILL.md`, `src/cmd/up.rs`, `src/cmd/attach.rs`, `src/supervisor.rs`
- Modify: `src/cli.rs`
- Test: `tests/cli_up.rs` (FakeRunner script)

**Interfaces:**
- Consumes: `Herdr`, `Runner`, `Config`, `adopt::registry_sessions`.
- Produces: `supervisor::Lock { host_env_id, herdr: HerdrTarget, claude_session: Option<String>, since }` at `state/supervisor/lock.json`. `flow up`: if lock exists and its herdr agent is live on that CDE (`Herdr::agents`), exec attach; else `ensure_server` locally, `create_workspace(cwd=/workspaces/flow, label="flow-supervisor")`, `start_agent(pane, "flow-supervisor", "claude", args)` where args are `["--resume", <id>]` when `lock.claude_session` is known (preserved across `up` runs) else `[]`, write the lock, then exec attach. `flow attach`: `exec` the local or ssh+herdr attach argv for the lock's target. `.claude/settings.json`: `Stop` hooks `[ {command: "\"$CLAUDE_PROJECT_DIR\"/target/release/flow guard"}, {command: "\"$CLAUDE_PROJECT_DIR\"/target/release/flow watch", asyncRewake: true, timeout: 7200} ]`, `SessionStart` hook `flow hook supervisor` (records the supervisor's `session_id` into the lock so `--resume` works next time). `CLAUDE.md` (under 120 lines): role, the five hard rules from the design (workers never address the user; never merge; never tear down unlanded work; report faithfully; agents use the CLI not the files), the escalation rule verbatim from design decision 21, session start procedure (`flow wake drain`, `flow status`, `flow sessions`, `flow requests`, then read `state/operator.md` if it exists for operator-specific conventions that are never committed), what to do on a `Stop hook feedback` wake, and pointers to the three skills. Skills: `flow-dispatch` (how to choose a CDE from `flow inventory`, write a brief with completion criteria, call `flow dispatch`), `flow-wake` (drain, handle each reason, ack, end turn), `flow-escalate` (decide vs. hold, `flow task decide`, `flow task hold`, steer via `flow steer <id> <text>`).
- Also produces `flow steer <id> <text>`: `Herdr::prompt` to the task's pane; appends event `steer`.

- [ ] **Step 1: Write failing tests**: `up_creates_supervisor_when_no_lock` (recorded argv includes `workspace create --cwd /workspaces/flow` and `agent start flow-supervisor --kind claude`; lock file written), `up_resumes_known_session` (lock with `claude_session: "s9"`; argv contains `--resume s9`), `up_attaches_when_lock_live` (scripted agents include the lock's pane; no `workspace create` recorded), `steer_prompts_pane_and_logs_event`.
- [ ] **Step 2: Run**; expected: FAIL. **Step 3: Implement** code, then write `.claude/settings.json`, `CLAUDE.md`, and the three skills. **Step 4: Run**; expected: PASS.
- [ ] **Step 5: Verify hook registration manually:** from `/workspaces/flow`, start `claude`, run `/hooks`, confirm both Stop hooks and the SessionStart hook are listed. Note the result in the commit message.
- [ ] **Step 6: Commit** `feat: flow up/attach/steer, supervisor hooks and instructions`.

### Task 14: CDE bootstrap (dotfiles)

**Files (in `~/dotfiles`, separate repo):**
- Modify: `post-start-install.sh`
- Modify: `agents/claude/settings.json` (herdr hook path)

**Interfaces:**
- Produces: on CDE start: clone `weston-vanta/flow` to `/workspaces/flow` if absent (else `git pull --ff-only`); `ln -sfn /efs-home/flow/state /workspaces/flow/state`; `mkdir -p /efs-home/flow/state`; add `/workspaces/flow/target/release` to PATH in the shell rc; install `herdr` if `command -v herdr` fails (follow herdr.dev's documented install command); run `cargo build --release` in `/workspaces/flow` when the binary is missing or older than `HEAD`. Herdr hook path becomes `"$HOME/.claude/hooks/herdr-agent-state.sh"` instead of the macOS absolute path.

- [ ] **Step 1: Make the edits** with idempotency (every step safe to rerun).
- [ ] **Step 2: Verify** by running `post-start-install.sh` on this CDE twice; second run prints only "already" lines. Run `herdr integration status` and confirm `claude: current`.
- [ ] **Step 3: Verify on a second CDE** over ssh: `ssh <alias> zsh -lc 'bash ~/dotfiles/post-start-install.sh && flow --version && readlink /workspaces/flow/state'`; expected: version printed, symlink target `/efs-home/flow/state`.
- [ ] **Step 4: Commit** in dotfiles: `feat: bootstrap flow clone, state symlink, herdr, and platform-aware herdr hook path`.

### Task 15: End-to-end verification against a second CDE

**Files:**
- Create: `docs/verification/2026-10-e2e.md` (checklist with recorded results, invented names only)

- [ ] **Step 1: Write `state/config.toml`** on EFS with the real CDEs (never committed).
- [ ] **Step 2: Start the supervisor:** `flow up` on this CDE; confirm `flow status` prints no tasks and `flow sessions` lists this session as adopted.
- [ ] **Step 3: Dispatch** a trivial task to another CDE with criteria "create a file named hello.txt in the worktree containing the task id, then report done"; confirm `dispatched <id>`; `flow sessions` shows the worker `working`; end the supervisor turn; confirm `flow watch` is running (`pgrep -f 'flow watch'`).
- [ ] **Step 4: Wake:** wait for the worker's `done`; confirm the supervisor received `Stop hook feedback` containing `done: <id>` and drained it.
- [ ] **Step 5: Escalate:** dispatch a task whose brief forces a `needs-decision`; confirm the wake, answer with `flow task decide` and `flow steer`; confirm the worker continues and finishes.
- [ ] **Step 6: Recycle drill:** stop the supervisor's herdr workspace; from a different CDE run `flow up`; confirm it resumes the same Claude session (`--resume`) and `flow wake drain` shows anything that happened in between.
- [ ] **Step 7: Record** results in the checklist and commit `docs: end-to-end verification record`.
