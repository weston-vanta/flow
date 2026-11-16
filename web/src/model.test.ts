import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activityLine,
  attentionLight,
  isFolded,
  baseLight,
  ciLight,
  feedbackLight,
  prDataAge,
  prView,
  reviewLight,
  sessionViews,
  stacksOf,
  statusLight,
  stepPr,
  trayView,
} from "./model.ts";
import type { Branch, Check, Github, Pr, Session, Thread } from "./model.ts";

const AT = "2026-11-10T12:00:00.000000Z";

function gh(o: Partial<Github> = {}): Github {
  return {
    repo: "acme/widgets",
    number: 7,
    url: "https://github.com/acme/widgets/pull/7",
    title: "[ABC-12] Add widgets",
    state: "OPEN",
    isDraft: false,
    headRefName: "feat/x",
    headRefOid: "0123456789abcdef0123456789abcdef01234567",
    baseRefName: "main",
    mergeable: "MERGEABLE",
    reviewDecision: "APPROVED",
    reviewRequests: [],
    checks: [{ name: "build", status: "COMPLETED", conclusion: "SUCCESS", url: null }],
    stack: null,
    updatedAt: AT,
    additions: 10,
    deletions: 2,
    changedFiles: 1,
    reviews: [],
    threads: [],
    behindSince: null,
    mergeQueue: null,
    ...o,
  };
}

const pr = (owner: string | null, o: Partial<Github> = {}): Pr => ({
  key: `acme/widgets#${o.number ?? 7}`,
  owner,
  github: { at: AT, ...gh(o) },
});

const session = (id: string, o: Partial<Session> = {}): Session => ({
  id,
  origin: null,
  status: null,
  context: null,
  activity: null,
  ...o,
});

const check = (status: string | null, conclusion: string | null, name = "build"): Check => ({
  name,
  status,
  conclusion,
  url: `https://ci.example/${name}`,
});

const NOW = Date.parse(AT);
const hoursAgo = (h: number) => new Date(NOW - h * 3_600_000).toISOString();

const thread = (o: Partial<Thread> = {}): Thread => ({
  login: "ana",
  path: "src/a.ts",
  line: 3,
  body: "Rename this.",
  url: "https://github.com/acme/widgets/pull/7#discussion_r1",
  createdAt: AT,
  ...o,
});

test("base: merged → fine; conflicting → act; else fine", () => {
  assert.equal(baseLight(gh({ state: "MERGED", mergeable: "CONFLICTING" })).lamp, "fine");
  assert.equal(baseLight(gh({ mergeable: "CONFLICTING" })).lamp, "act");
  assert.equal(baseLight(gh({ mergeable: "MERGEABLE" })).lamp, "fine");
  assert.equal(baseLight(gh({ mergeable: "UNKNOWN" })).lamp, "fine");
});

test("base: behind the default branch for more than 24h → act; the tip names each reason", () => {
  const base = (o: Partial<Github>, below: number | null = null) => baseLight(gh(o), below, NOW);
  assert.equal(base({ behindSince: hoursAgo(23) }).lamp, "fine");
  assert.equal(base({ behindSince: hoursAgo(25) }).lamp, "act");
  assert.equal(base({ behindSince: hoursAgo(25), state: "CLOSED" }).lamp, "fine");
  assert.equal(base({ mergeable: "CONFLICTING" }).tip, "base: merge conflict with main");
  assert.equal(base({ mergeable: "CONFLICTING" }, 9918).tip, "base: merge conflict with #9918");
  assert.equal(base({ behindSince: hoursAgo(25) }).tip, "base: behind main since 1 d ago");
  assert.equal(
    base({ mergeable: "CONFLICTING", behindSince: hoursAgo(49) }).tip,
    "base: merge conflict with main · behind main since 2 d ago",
  );
});

test("CI: failing → act; not completed → run; else fine, even with no checks", () => {
  const ok = check("COMPLETED", "SUCCESS", "lint");
  assert.equal(ciLight(gh({ checks: [ok, check("COMPLETED", "FAILURE")] })).lamp, "act");
  assert.equal(ciLight(gh({ checks: [check("COMPLETED", "TIMED_OUT")] })).lamp, "act");
  // A failure wins over a check still running.
  assert.equal(ciLight(gh({ checks: [check("IN_PROGRESS", null, "a"), check("COMPLETED", "FAILURE")] })).lamp, "act");
  assert.equal(ciLight(gh({ checks: [ok, check("IN_PROGRESS", null)] })).lamp, "run");
  assert.equal(ciLight(gh({ checks: [check("QUEUED", null)] })).lamp, "run");
  assert.equal(ciLight(gh({ checks: [] })).lamp, "fine");
  assert.equal(ciLight(gh({ checks: [ok, check("COMPLETED", "SKIPPED")] })).lamp, "fine");
  assert.match(ciLight(gh({ checks: [ok, check("COMPLETED", "FAILURE", "e2e")] })).tip, /e2e/);
});

test("feedback: open threads → act unless merged or closed; the tip counts them", () => {
  assert.equal(feedbackLight(gh({ threads: [thread(), thread()] })).lamp, "act");
  assert.match(feedbackLight(gh({ threads: [thread(), thread()] })).tip, /2 unresolved threads/);
  assert.equal(feedbackLight(gh({ threads: [thread(), thread()], state: "MERGED" })).lamp, "fine");
  assert.equal(feedbackLight(gh({ threads: [] })).lamp, "fine");
});

test("merged and closed PRs show all four lamps fine", () => {
  const bad: Partial<Github> = {
    mergeable: "CONFLICTING",
    behindSince: hoursAgo(48),
    checks: [check("COMPLETED", "FAILURE")],
    threads: [thread()],
    reviewDecision: "CHANGES_REQUESTED",
  };
  for (const state of ["MERGED", "CLOSED"] as const) {
    const v = prView(gh({ ...bad, state }), 1, null, NOW);
    assert.deepEqual([v.base.lamp, v.ci.lamp, v.feedback.lamp, v.review.lamp], ["fine", "fine", "fine", "fine"]);
  }
});

test("size tip: files changed, with the lines added and removed", () => {
  const v = prView(gh({ changedFiles: 3, additions: 500, deletions: 40 }), 1);
  assert.equal(v.sizeTip, "3 files, +500 −40");
  assert.equal(prView(gh({ changedFiles: 1, additions: 2, deletions: 0 }), 1).sizeTip, "1 file, +2 −0");
});

test("tray: every section gives its fine statement when nothing is wrong", () => {
  const t = trayView(gh(), null, NOW);
  assert.deepEqual(t.base, { fine: "no conflict, up to date", items: null });
  assert.deepEqual(t.ci, { fine: "passing", items: null });
  assert.deepEqual(t.feedback, { fine: "no open threads", items: null });
  assert.deepEqual(t.review, { fine: "approved", items: null });
  assert.equal(trayView(gh(), 9918, NOW).base.fine, "no conflict");
  assert.equal(trayView(gh({ checks: [check("IN_PROGRESS", null)] }), null, NOW).ci.fine, "running");
  assert.equal(trayView(gh({ reviewDecision: "REVIEW_REQUIRED" }), null, NOW).review.fine, "required, none requested");
  assert.equal(trayView(gh({ reviewDecision: null }), null, NOW).review.fine, "not required");
  // Behind for less than a day is not a base problem yet.
  assert.equal(trayView(gh({ behindSince: hoursAgo(23) }), null, NOW).base.fine, "no conflict, up to date");
});

test("tray: merged and closed PRs give every section's fine statement", () => {
  const bad: Partial<Github> = {
    mergeable: "CONFLICTING",
    checks: [check("COMPLETED", "FAILURE")],
    threads: [thread()],
    reviewRequests: [{ login: "ana", asCodeOwner: false }],
  };
  assert.deepEqual(
    Object.values(trayView(gh({ ...bad, state: "MERGED" }), null, NOW)).map((s) => s.fine),
    ["merged", "passed", "no open threads", "merged"],
  );
  assert.deepEqual(
    Object.values(trayView(gh({ ...bad, state: "CLOSED" }), null, NOW)).map((s) => s.fine),
    ["closed", "passed", "no open threads", "closed"],
  );
});

test("tray base: conflict named by the PR below or the base branch, and behind since", () => {
  const g = gh({ mergeable: "CONFLICTING", behindSince: hoursAgo(26) });
  assert.deepEqual(trayView(g, null, NOW).base.items, ["Merge conflict with main", "Behind main since 1 d ago"]);
  assert.deepEqual(trayView(gh({ mergeable: "CONFLICTING" }), 9918, NOW).base.items, ["Merge conflict with #9918"]);
});

test("tray CI: only failing required checks, with their links", () => {
  const g = gh({
    checks: [check("COMPLETED", "SUCCESS", "lint"), check("COMPLETED", "FAILURE", "e2e"), check("IN_PROGRESS", null, "unit")],
  });
  assert.deepEqual(trayView(g, null, NOW).ci.items, [{ name: "e2e", url: "https://ci.example/e2e" }]);
});

test("tray feedback: threads oldest first, by path:line or the path alone", () => {
  const g = gh({
    threads: [
      thread({ login: "bo", createdAt: hoursAgo(1), path: "b.ts", line: null }),
      thread({ login: "ana", createdAt: hoursAgo(3), path: "a.ts", line: 12 }),
    ],
  });
  const items = trayView(g, null, NOW).feedback.items!;
  assert.deepEqual(
    items.map((t) => [t.author, t.where, t.ago]),
    [
      ["ana", "a.ts:12", "3 h"],
      ["bo", "b.ts", "1 h"],
    ],
  );
  assert.equal(items[0].body, "Rename this.");
  assert.equal(items[0].url, "https://github.com/acme/widgets/pull/7#discussion_r1");
});

test("tray feedback: a body over 280 characters or 4 lines is long", () => {
  const long = (body: string) => trayView(gh({ threads: [thread({ body })] }), null, NOW).feedback.items![0].long;
  assert.equal(long("Rename this."), false);
  assert.equal(long("x".repeat(280)), false);
  assert.equal(long("x".repeat(281)), true);
  assert.equal(long("a\nb\nc\nd"), false);
  assert.equal(long("a\nb\nc\nd\ne"), true);
});

test("tray review: each reviewer once; a pending request wins over an earlier review", () => {
  const g = gh({
    reviewDecision: "CHANGES_REQUESTED",
    reviewRequests: [
      { login: "ana", asCodeOwner: false },
      { team: "frontend", asCodeOwner: true },
      { team: "docs", asCodeOwner: false },
    ],
    reviews: [
      { login: "ana", state: "CHANGES_REQUESTED", teams: [] },
      { login: "rina", state: "APPROVED", teams: ["platform-infra", "node-runtime"] },
      { login: "sam", state: "CHANGES_REQUESTED", teams: [] },
    ],
  });
  assert.deepEqual(trayView(g, null, NOW).review.items, [
    { name: "ana", team: false, state: "requested", codeOwner: false, teams: [] },
    { name: "frontend", team: true, state: "requested", codeOwner: true, teams: [] },
    { name: "docs", team: true, state: "requested", codeOwner: false, teams: [] },
    { name: "rina", team: false, state: "approved", codeOwner: false, teams: ["platform-infra", "node-runtime"] },
    { name: "sam", team: false, state: "changes requested", codeOwner: false, teams: [] },
  ]);
});

test("session views name a stacked PR's base by the PR below it", () => {
  const prs = [
    pr("s", { number: 2, stack: { number: 1, position: 2 }, headRefName: "b", baseRefName: "a", mergeable: "CONFLICTING" }),
    pr("s", { number: 1, stack: { number: 1, position: 1 }, headRefName: "a", baseRefName: "main" }),
  ];
  const [[, top]] = sessionViews([session("s")], prs, [], NOW)[0].stacks;
  assert.equal(top.number, 2);
  assert.deepEqual(top.tray.base.items, ["Merge conflict with #1"]);
});

test("review: changes requested or nobody requested → act; pending requests → count; else fine", () => {
  const r = (o: Partial<Github>) => {
    const l = reviewLight(gh(o));
    return [l.lamp, l.count];
  };
  assert.deepEqual(r({ state: "MERGED", reviewDecision: "CHANGES_REQUESTED" }), ["fine", null]);
  assert.deepEqual(r({ state: "CLOSED", reviewDecision: "REVIEW_REQUIRED" }), ["fine", null]);
  assert.deepEqual(r({ reviewDecision: "CHANGES_REQUESTED", reviewRequests: [{ login: "ana", asCodeOwner: false }] }), ["act", null]);
  assert.deepEqual(r({ reviewDecision: "REVIEW_REQUIRED", reviewRequests: [{ login: "ana", asCodeOwner: false }, { team: "acme/web", asCodeOwner: false }] }), ["fine", 2]);
  assert.deepEqual(r({ reviewDecision: null, reviewRequests: [{ login: "ana", asCodeOwner: false }] }), ["fine", 1]);
  assert.deepEqual(r({ reviewDecision: "REVIEW_REQUIRED" }), ["act", null]);
  // Drafts follow the same rules.
  assert.deepEqual(r({ isDraft: true, reviewDecision: "REVIEW_REQUIRED" }), ["act", null]);
  assert.deepEqual(r({ reviewDecision: "APPROVED" }), ["fine", null]);
  assert.deepEqual(r({ reviewDecision: null }), ["fine", null]);
});

test("review tooltip names the requested people and teams", () => {
  const tip = reviewLight(
    gh({ reviewDecision: "REVIEW_REQUIRED", reviewRequests: [{ login: "ana", asCodeOwner: false }, { team: "acme/web", asCodeOwner: false }] }),
  ).tip;
  assert.match(tip, /ana, acme\/web/);
});

test("Session status lamp: running → run, waiting → act, idle and done → fine", () => {
  assert.equal(statusLight("running").lamp, "run");
  assert.equal(statusLight("waiting").lamp, "act");
  assert.equal(statusLight("idle").lamp, "fine");
  assert.equal(statusLight("done").lamp, "fine");
  assert.equal(statusLight(null).lamp, "off");
});

test("a PR's view title is its GitHub title unchanged, prefix and all", () => {
  assert.equal(prView(gh({ title: "[ABC-123] Fix it" }), null).title, "[ABC-123] Fix it");
});

test("attention: an open PR that is mergeable or has a lit lamp", () => {
  assert.equal(prView(gh(), 1).attention, true);
  assert.equal(prView(gh({ mergeable: "CONFLICTING" }), 1).attention, true);
  assert.equal(prView(gh({ reviewDecision: "REVIEW_REQUIRED" }), 1).attention, true);
  assert.equal(prView(gh({ reviewRequests: [{ login: "ana", asCodeOwner: false }] }), 1).attention, false);
  assert.equal(prView(gh({ checks: [check("IN_PROGRESS", null)] }), 1).attention, false);
  assert.equal(prView(gh({ isDraft: true }), 1).attention, false);
  assert.equal(prView(gh({ state: "CLOSED", threads: [thread(), thread()] }), 1).attention, false);
  assert.equal(prView(gh({ state: "MERGED" }), 1).attention, false);
});

test("mergeable: open, not a draft, no review outstanding, all four lamps fine", () => {
  const node = (o: Partial<Github>) => prView(gh(o), 1).node;
  assert.equal(node({}), "ready");
  assert.equal(node({ reviewDecision: null }), "ready");
  assert.equal(node({ checks: [] }), "ready");
  assert.equal(node({ isDraft: true }), "open");
  assert.equal(node({ reviewRequests: [{ login: "ana", asCodeOwner: false }] }), "open");
  assert.equal(node({ reviewDecision: "REVIEW_REQUIRED" }), "open");
  assert.equal(node({ checks: [check("IN_PROGRESS", null)] }), "open");
  assert.equal(node({ threads: [thread()] }), "open");
  assert.equal(node({ state: "MERGED" }), "merged");
  assert.equal(node({ state: "CLOSED" }), "closed");
});

test("queued: an open PR in the merge queue is never mergeable and needs attention only for a lit lamp", () => {
  const mergeQueue = { position: 37, size: 214 };
  assert.equal(prView(gh({ mergeQueue }), 1).node, "queued");
  assert.equal(prView(gh({ mergeQueue }), 1).attention, false);
  assert.equal(prView(gh({ mergeQueue, threads: [thread()] }), 1).node, "queued");
  assert.equal(prView(gh({ mergeQueue, threads: [thread()] }), 1).attention, true);
  // A leftover entry on a merged PR doesn't matter.
  assert.equal(prView(gh({ mergeQueue, state: "MERGED" }), 1).node, "merged");
  assert.equal(prView(gh({ mergeQueue, state: "MERGED" }), 1).queue, null);
  assert.equal(prView(gh(), 1).queue, null);
});

test("queue: the label, the text, and the needle's place from the front (0) to the back (1)", () => {
  const queue = (position: number, size: number) => prView(gh({ mergeQueue: { position, size } }), 1).queue;
  assert.deepEqual(queue(37, 214), { label: "37/214", text: "in merge queue · 37 of 214", at: 36 / 213 });
  assert.equal(queue(1, 214)?.at, 0);
  assert.equal(queue(214, 214)?.at, 1);
  assert.equal(queue(1, 1)?.at, 0);
  // Position and size can disagree for a moment; the needle stays on the scale.
  assert.equal(queue(5, 3)?.at, 1);
});

const nums = (stacks: { number: number }[][]) => stacks.map((s) => s.map((g) => g.number));

test("Stacks: a Session's PRs grouped by stack number, each bottom first", () => {
  const prs = [
    pr("s1", { number: 10, stack: { number: 41, position: 2 } }),
    pr("s1", { number: 12 }),
    pr("s1", { number: 9, stack: { number: 50, position: 2 } }),
    pr("s1", { number: 4, stack: { number: 41, position: 1 } }),
    pr("s1", { number: 5 }),
    pr("s1", { number: 6, stack: { number: 50, position: 1 } }),
    pr("s2", { number: 3, stack: { number: 41, position: 3 } }),
    pr(null, { number: 1, stack: { number: 41, position: 1 } }),
  ];
  // Oldest first by bottom PR number, with Stacks of one among them.
  assert.deepEqual(nums(stacksOf("s1", prs)), [[4, 10], [5], [6, 9], [12]]);
  assert.deepEqual(nums(stacksOf("s2", prs)), [[3]]);
});

test("Stacks keep their place as PRs are added on top or the bottom PR merges", () => {
  const before = [
    pr("s", { number: 4, stack: { number: 41, position: 1 } }),
    pr("s", { number: 8, stack: { number: 41, position: 2 } }),
    pr("s", { number: 6 }),
  ];
  assert.deepEqual(nums(stacksOf("s", before)), [[4, 8], [6]]);
  const after = [
    pr("s", { number: 4, state: "MERGED", stack: { number: 41, position: 1 } }),
    pr("s", { number: 8, stack: { number: 41, position: 2 } }),
    pr("s", { number: 11, stack: { number: 41, position: 3 } }),
    pr("s", { number: 6 }),
  ];
  assert.deepEqual(nums(stacksOf("s", after)), [[4, 8, 11], [6]]);
});

test("each Stack numbers its own PRs; a Stack of one has no number", () => {
  const prs = [
    pr("s", { number: 4, stack: { number: 41, position: 1 } }),
    pr("s", { number: 8, stack: { number: 41, position: 2 } }),
    pr("s", { number: 5 }),
    pr("s", { number: 6, stack: { number: 50, position: 1 } }),
    pr("s", { number: 7, stack: { number: 50, position: 2 } }),
    pr("s", { number: 9, stack: { number: 50, position: 3 } }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(
    v.stacks.map((s) => s.map((p) => [p.number, p.pos])),
    [
      [[4, 1], [8, 2]],
      [[5, null]],
      [[6, 1], [7, 2], [9, 3]],
    ],
  );
});

test("activity line: the prompt while running, else the reply", () => {
  const activity = { at: AT, prompt: "do it", reply: "done it" };
  const running = session("a", { status: { at: AT, state: "running" }, activity });
  const idle = session("a", { status: { at: AT, state: "idle" }, activity });
  const waiting = session("a", {
    status: { at: AT, state: "waiting" },
    activity: { at: AT, prompt: "do it", reply: null },
  });
  assert.equal(activityLine(running), "do it");
  assert.equal(activityLine(idle), "done it");
  assert.equal(activityLine(waiting), "do it");
  assert.equal(activityLine(session("a")), null);
});

test("session views join Sessions to their Stacks and tolerate null facets", () => {
  const sessions = [
    session("s1", {
      origin: {
        at: AT,
        cde: "flow",
        cdeId: "x",
        pane: "p",
        herdrSocket: null,
        cwd: "/workspaces/obsidian",
        transcriptPath: null,
      },
      status: { at: AT, state: "waiting" },
      context: { at: AT, title: "Fix the flaky test", branch: "fix" },
    }),
    session("s2"),
  ];
  const [one, two] = sessionViews(sessions, [pr("s1", { number: 8, repo: "acme/api" })]);
  assert.equal(one.title, "Fix the flaky test");
  assert.equal(one.untitled, false);
  assert.equal(one.status.lamp, "act");
  assert.equal(one.cde, "flow");
  assert.equal(one.repo, "acme/api");
  assert.deepEqual(
    one.stacks.map((s) => s.map((p) => [p.pos, p.number, p.title])),
    [[[null, 8, "[ABC-12] Add widgets"]]],
  );
  // Until its first Stop writes a title, a Session shows a placeholder, not its id.
  assert.equal(two.title, "New session");
  assert.equal(two.untitled, true);
  assert.equal(two.repo, null);
  assert.deepEqual(two.stacks, []);
  // Without PRs the repo falls back to the cwd's last segment.
  assert.equal(sessionViews([sessions[0]], [])[0].repo, "obsidian");
});

const branch = (owner: string, name: string, at: string, repo = "acme/widgets"): Branch => ({
  key: `${repo}:${name}`,
  repo,
  branch: name,
  owner,
  at,
});

test("a Session's Branches without a PR follow its Stacks, oldest first", () => {
  const branches = [
    branch("s", "feat/new", hoursAgo(1)),
    branch("s", "feat/old", hoursAgo(5)),
    // Heads of a PR, open or merged: the PR shows instead.
    branch("s", "feat/x", hoursAgo(9)),
    branch("s", "feat/done", hoursAgo(9)),
    // Same branch name in another repo has no PR.
    branch("s", "feat/x", hoursAgo(3), "acme/api"),
    branch("other", "feat/theirs", hoursAgo(2)),
  ];
  const prs = [pr("s", { number: 7, headRefName: "feat/x", isDraft: true }), pr("s", { number: 8, headRefName: "feat/done", state: "MERGED" })];
  const [v] = sessionViews([session("s")], prs, branches, NOW);
  assert.deepEqual(v.stacks.flat().map((p) => p.number), [7, 8]);
  assert.deepEqual(v.branches, ["feat/old", "feat/x", "feat/new"]);
  // Branch rows count towards neither the folded light nor its PR count.
  assert.deepEqual(attentionLight(v), { lamp: "fine", tip: "2 pull requests: nothing needs action" });
  // A Session with only a Branch takes its repo from it.
  const [only] = sessionViews([session("o")], [], [branch("o", "feat/y", AT, "acme/api")]);
  assert.equal(only.repo, "acme/api");
  assert.deepEqual(only.branches, ["feat/y"]);
});

test("done Sessions go last; each group newest first, unstarted last", () => {
  const S = (id: string, state: "running" | "done" | "idle", startedMin: number | null) =>
    session(id, {
      status: { at: AT, state },
      origin:
        startedMin == null
          ? null
          : {
              at: new Date(Date.parse(AT) - startedMin * 60_000).toISOString(),
              cde: null,
              cdeId: null,
              pane: null,
              herdrSocket: null,
              cwd: null,
              transcriptPath: null,
            },
    });
  const ids = sessionViews(
    [S("a", "done", 5), S("b", "running", 30), S("c", "done", 1), S("d", "idle", 2), S("e", "running", null)],
    [],
  ).map((v) => v.id);
  assert.deepEqual(ids, ["d", "b", "e", "c", "a"]);
});

test("folded Session light: act if waiting or any PR needs attention, else fine", () => {
  const view = (state: Session["status"], ...gs: Partial<Github>[]) =>
    sessionViews([session("s", { status: state })], gs.map((g, i) => pr("s", { number: i + 1, ...g })))[0];
  const st = (state: "running" | "waiting" | "idle" | "done") => ({ at: AT, state });
  assert.equal(attentionLight(view(st("waiting"))).lamp, "act");
  assert.equal(attentionLight(view(st("running"))).lamp, "fine");
  assert.equal(attentionLight(view(st("running"), { reviewRequests: [{ login: "a", asCodeOwner: false }] })).lamp, "fine");
  assert.equal(attentionLight(view(st("idle"), { reviewRequests: [{ login: "a", asCodeOwner: false }] }, {})).lamp, "act");
  assert.equal(attentionLight(view(st("idle"), { mergeable: "CONFLICTING", reviewRequests: [{ login: "a", asCodeOwner: false }] })).lamp, "act");
  // Over all of a Session's Stacks: the act is in a second Stack.
  const two = view(st("idle"), { reviewRequests: [{ login: "a", asCodeOwner: false }] }, { stack: { number: 9, position: 1 }, mergeable: "CONFLICTING" });
  assert.equal(two.stacks.length, 2);
  assert.deepEqual(attentionLight(two), { lamp: "act", tip: "2 pull requests: something needs action" });
});

test("done Sessions start folded; a toggle flips the default", () => {
  const views = sessionViews(
    [session("d", { status: { at: AT, state: "done" } }), session("i", { status: { at: AT, state: "idle" } })],
    [],
  );
  const done = views.find((v) => v.id === "d")!;
  const idle = views.find((v) => v.id === "i")!;
  assert.equal(isFolded(done, new Set()), true);
  assert.equal(isFolded(idle, new Set()), false);
  assert.equal(isFolded(done, new Set(["d"])), false);
  assert.equal(isFolded(idle, new Set(["i"])), true);
});

test("PR data age: minutes since the oldest open PR was fetched, only when over 3 minutes", () => {
  const now = Date.parse(AT);
  const fetched = (min: number, state: Github["state"] = "OPEN"): Pr => ({
    key: "k",
    owner: null,
    github: { ...gh({ state }), at: new Date(now - min * 60_000).toISOString() },
  });
  assert.equal(prDataAge([], now), null);
  assert.equal(prDataAge([fetched(1), fetched(3)], now), null);
  assert.equal(prDataAge([fetched(1), fetched(7.5)], now), 7);
  // Closed and merged PRs are fetched once, so their age says nothing.
  assert.equal(prDataAge([fetched(1), fetched(60, "MERGED"), fetched(60, "CLOSED")], now), null);
  assert.equal(prDataAge([{ key: "k", owner: null, github: null }], now), null);
});

/** PR `n` owned by `owner`, at `url` "#n"; `stack` places it in a GitHub stack. */
const stepped = (owner: string, n: number, stack: { number: number; position: number } | null = null) =>
  pr(owner, { number: n, url: `#${n}`, headRefName: `b${n}`, stack });

/** Session a: a Stack of #2 on #1, then #3 alone; Session b: #4 and a Branch with no PR;
 * Session c: no PRs. The page draws #1 #2 #3 #4. */
function stepViews() {
  const prs = [stepped("a", 2, { number: 1, position: 2 }), stepped("a", 1, { number: 1, position: 1 }), stepped("a", 3), stepped("b", 4)];
  return sessionViews([session("a"), session("b"), session("c")], prs, [branch("b", "loose", AT)]);
}
const step = (url: string | null, dir: 1 | -1, toggled = new Set<string>()) => stepPr(stepViews(), toggled, url, dir);

test("step order: from nothing selected, ↓ picks the first PR and ↑ the last", () => {
  assert.equal(step(null, 1), "#1");
  assert.equal(step(null, -1), "#4");
});

test("step order: next and previous within a Stack, across Stacks and across Sessions", () => {
  assert.equal(step("#1", 1), "#2");
  assert.equal(step("#2", -1), "#1");
  assert.equal(step("#2", 1), "#3");
  assert.equal(step("#3", -1), "#2");
  assert.equal(step("#3", 1), "#4");
  assert.equal(step("#4", -1), "#3");
});

test("step order: wraps at both ends, never onto a Branch with no PR", () => {
  assert.equal(step("#4", 1), "#1");
  assert.equal(step("#1", -1), "#4");
});

test("step order: skips folded Sessions", () => {
  const folded = new Set(["a"]);
  assert.equal(step(null, 1, folded), "#4");
  assert.equal(step("#4", 1, folded), "#4");
  // A selected PR that has been folded away steps as if nothing were selected.
  assert.equal(stepPr(stepViews(), new Set(["b"]), "#4", -1), "#3");
});

test("step order: null with no visible PR", () => {
  assert.equal(step(null, 1, new Set(["a", "b"])), null);
  assert.equal(stepPr([], new Set(), null, -1), null);
});
