import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activityLine,
  isFolded,
  baseLight,
  ciLight,
  feedbackLight,
  jiraDataAge,
  jiraView,
  lanes,
  prDataAge,
  prView,
  reviewLight,
  sessionPanel,
  sessionViews,
  shortUrl,
  stacksOf,
  statusLight,
  stepPr,
  trayView,
} from "./model.ts";
import type { Branch, Check, Github, Jira, Pr, Session, SessionView, Task, Thread } from "./model.ts";

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
  jira: null,
});

const session = (id: string, o: Partial<Session> = {}): Session => ({
  id,
  origin: null,
  status: null,
  context: null,
  activity: null,
  task: null,
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
  assert.deepEqual(t.base, { fine: "no conflict, up to date with main", items: null });
  assert.deepEqual(t.ci, { fine: "passing", items: null });
  assert.deepEqual(t.feedback, { fine: "no open threads", items: null });
  assert.deepEqual(t.review, { fine: "approved", items: null });
  assert.equal(trayView(gh(), 9918, NOW).base.fine, "no conflict");
  assert.equal(trayView(gh({ checks: [check("IN_PROGRESS", null)] }), null, NOW).ci.fine, "running");
  assert.equal(trayView(gh({ reviewDecision: "REVIEW_REQUIRED" }), null, NOW).review.fine, "required, none requested");
  assert.equal(trayView(gh({ reviewDecision: null }), null, NOW).review.fine, "not required");
  // Behind for less than a day is not a base problem yet.
  assert.equal(trayView(gh({ behindSince: hoursAgo(23) }), null, NOW).base.fine, "no conflict, up to date with main");
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
  const [, top] = sessionViews([session("s")], prs, [], NOW)[0].groups[0].stacks[0];
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
  // A stacked PR's base requires no review, so GitHub gives no decision; the review still counts.
  const changes = { login: "ana", state: "CHANGES_REQUESTED" as const, teams: [] };
  assert.deepEqual(r({ reviewDecision: null, reviews: [changes] }), ["act", null]);
  assert.deepEqual(r({ reviewDecision: null, reviews: [changes], reviewRequests: [{ login: "ana", asCodeOwner: false }] }), ["act", null]);
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

test("queued: an open PR in the merge queue is never mergeable", () => {
  const mergeQueue = { position: 37, size: 214 };
  assert.equal(prView(gh({ mergeQueue }), 1).node, "queued");
  assert.equal(prView(gh({ mergeQueue, threads: [thread()] }), 1).node, "queued");
  // A leftover entry on a merged PR doesn't matter.
  assert.equal(prView(gh({ mergeQueue, state: "MERGED" }), 1).node, "merged");
  assert.equal(prView(gh({ mergeQueue, state: "MERGED" }), 1).queue, null);
  assert.equal(prView(gh(), 1).queue, null);
});

test("queue: the text, and the ring's fill, empty at the back (0) to full at the front (1)", () => {
  const queue = (position: number, size: number) => prView(gh({ mergeQueue: { position, size } }), 1).queue;
  assert.deepEqual(queue(37, 214), { text: "merge queue · 37 of 214", fill: 177 / 213 });
  assert.equal(queue(1, 214)?.fill, 1);
  assert.equal(queue(214, 214)?.fill, 0);
  assert.equal(queue(1, 1)?.fill, 1);
  // Position and size can disagree for a moment; the fill stays on the ring.
  assert.equal(queue(5, 3)?.fill, 0);
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
    v.groups.map((g) => g.stacks[0].map((p) => [p.number, p.pos])),
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
    one.groups.map((g) => g.stacks[0].map((p) => [p.pos, p.number, p.title])),
    [[[null, 8, "[ABC-12] Add widgets"]]],
  );
  // Until its first Stop writes a title, a Session shows a placeholder, not its id.
  assert.equal(two.title, "New session");
  assert.equal(two.untitled, true);
  assert.equal(two.repo, null);
  assert.deepEqual(two.groups, []);
  // Without PRs the repo falls back to the cwd's last segment.
  assert.equal(sessionViews([sessions[0]], [])[0].repo, "obsidian");
});

/** Each group as [head, Stacks by PR number]: the head is "#n" for a feature PR, a Branch's
 * name, or null for an ordinary Stack. */
const shape = (v: SessionView) =>
  v.groups.map((g) => [g.kind === "branch" ? g.head.name : g.kind === "feature" ? `#${g.head.number}` : null, g.stacks.map((s) => s.map((p) => p.number))]);

const branch = (owner: string, name: string, at: string, repo = "acme/widgets"): Branch => ({
  key: `${repo}:${name}`,
  repo,
  branch: name,
  owner,
  at,
  jira: null,
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
  assert.deepEqual(shape(v), [
    [null, [[7]]],
    [null, [[8]]],
    ["feat/old", []],
    ["feat/x", []],
    ["feat/new", []],
  ]);
  // Branch rows don't count as PRs.
  assert.equal(v.prs.length, 2);
  // A Session with only a Branch takes its repo from it.
  const [only] = sessionViews([session("o")], [], [branch("o", "feat/y", AT, "acme/api")]);
  assert.equal(only.repo, "acme/api");
  assert.deepEqual(shape(only), [["feat/y", []]]);
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

test("done Sessions and Sessions with no PRs start folded; a toggle flips the default", () => {
  const views = sessionViews(
    [
      session("d", { status: { at: AT, state: "done" } }),
      session("i", { status: { at: AT, state: "idle" } }),
      session("e", { status: { at: AT, state: "running" } }),
    ],
    [pr("d", { number: 1 }), pr("i", { number: 2 })],
    [{ key: "acme/widgets:wip", repo: "acme/widgets", branch: "wip", owner: "e", at: AT }],
  );
  const done = views.find((v) => v.id === "d")!;
  const idle = views.find((v) => v.id === "i")!;
  // A Branch with no PR doesn't count as a PR.
  const empty = views.find((v) => v.id === "e")!;
  assert.equal(isFolded(done, new Set()), true);
  assert.equal(isFolded(idle, new Set()), false);
  assert.equal(isFolded(empty, new Set()), true);
  assert.equal(isFolded(done, new Set(["d"])), false);
  assert.equal(isFolded(idle, new Set(["i"])), true);
  assert.equal(isFolded(empty, new Set(["e"])), false);
});

test("PR data age: minutes since the oldest open PR was fetched, only when over 3 minutes", () => {
  const now = Date.parse(AT);
  const fetched = (min: number, state: Github["state"] = "OPEN"): Pr => ({
    key: "k",
    owner: null,
    github: { ...gh({ state }), at: new Date(now - min * 60_000).toISOString() },
    jira: null,
  });
  assert.equal(prDataAge([], now), null);
  assert.equal(prDataAge([fetched(1), fetched(3)], now), null);
  assert.equal(prDataAge([fetched(1), fetched(7.5)], now), 7);
  // Closed and merged PRs are fetched once, so their age says nothing.
  assert.equal(prDataAge([fetched(1), fetched(60, "MERGED"), fetched(60, "CLOSED")], now), null);
  assert.equal(prDataAge([{ key: "k", owner: null, github: null, jira: null }], now), null);
});

/** PR `n` owned by `owner`, at `url` "#n"; `stack` places it in a GitHub stack. */
const stepped = (owner: string, n: number, stack: { number: number; position: number } | null = null) =>
  pr(owner, { number: n, url: `#${n}`, headRefName: `b${n}`, stack });

/** Session a: a Stack of #2 on #1, then #3 alone; Session b: #4 and a Branch with no PR;
 * Session c: no PRs. The page draws #1 #2 #3 #4 and the Branch. */
function stepViews() {
  const prs = [stepped("a", 2, { number: 1, position: 2 }), stepped("a", 1, { number: 1, position: 1 }), stepped("a", 3), stepped("b", 4)];
  return sessionViews([session("a"), session("b"), session("c")], prs, [branch("b", "loose", AT)]);
}
const step = (url: string | null, dir: 1 | -1, toggled = new Set<string>()) => stepPr(stepViews(), toggled, url, dir);

const LOOSE = "acme/widgets:loose";

test("step order: from nothing selected, ↓ picks the first node and ↑ the last", () => {
  assert.equal(step(null, 1), "#1");
  assert.equal(step(null, -1), LOOSE);
});

test("step order: next and previous within a Stack, across Stacks and across Sessions", () => {
  assert.equal(step("#1", 1), "#2");
  assert.equal(step("#2", -1), "#1");
  assert.equal(step("#2", 1), "#3");
  assert.equal(step("#3", -1), "#2");
  assert.equal(step("#3", 1), "#4");
  assert.equal(step("#4", -1), "#3");
  assert.equal(step("#4", 1), LOOSE);
  assert.equal(step(LOOSE, -1), "#4");
});

test("step order: wraps at both ends", () => {
  assert.equal(step(LOOSE, 1), "#1");
  assert.equal(step("#1", -1), LOOSE);
});

test("step order: skips folded Sessions", () => {
  const folded = new Set(["a"]);
  assert.equal(step(null, 1, folded), "#4");
  assert.equal(step(LOOSE, 1, folded), "#4");
  // A selected PR that has been folded away steps as if nothing were selected.
  assert.equal(stepPr(stepViews(), new Set(["b"]), "#4", -1), "#3");
});

test("step order: null with nothing visible", () => {
  assert.equal(step(null, 1, new Set(["a", "b"])), null);
  assert.equal(stepPr([], new Set(), null, -1), null);
});

test("feature branch: a PR on the default branch heads the Stacks its owner based on its head", () => {
  const prs = [
    pr("s", { number: 20, headRefName: "feature/x", baseRefName: "main", isDraft: true }),
    pr("s", { number: 21, headRefName: "a", baseRefName: "feature/x", stack: { number: 21, position: 1 } }),
    pr("s", { number: 22, headRefName: "b", baseRefName: "a", stack: { number: 21, position: 2 } }),
    pr("s", { number: 23, headRefName: "c", baseRefName: "feature/x" }),
    pr("s", { number: 30, headRefName: "other", baseRefName: "main" }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(shape(v), [
    ["#20", [[21, 22], [23]]],
    [null, [[30]]],
  ]);
  // Stepping runs through the group as drawn: the feature PR, then its Stacks.
  assert.deepEqual(v.prs.map((p) => p.number), [20, 21, 22, 23, 30]);
});

test("feature branch: before its feature PR exists, the Branch heads its group", () => {
  const prs = [
    pr("s", { number: 21, headRefName: "a", baseRefName: "feature/x" }),
    pr("s", { number: 22, headRefName: "b", baseRefName: "a" }),
  ];
  const [v] = sessionViews([session("s")], prs, [branch("s", "feature/x", AT), branch("s", "a", AT)], NOW);
  // #21 is based on a Branch, not the default branch, so #22 on its head stays an ordinary
  // Stack. Branch groups follow the Stacks, as Branches with no PR do.
  assert.deepEqual(shape(v), [
    [null, [[22]]],
    ["feature/x", [[21]]],
  ]);
  assert.deepEqual(v.prs.map((p) => p.number), [22, 21]);
});

test("feature branch: gathers only its owner's Stacks; another Session's Stack names it as its base", () => {
  const prs = [
    pr("s", { number: 20, headRefName: "feature/x", baseRefName: "main" }),
    pr("s", { number: 21, headRefName: "a", baseRefName: "feature/x" }),
    pr("t", { number: 22, headRefName: "b", baseRefName: "feature/x", mergeable: "CONFLICTING" }),
  ];
  const [s, t] = sessionViews([session("s"), session("t")], prs, [], NOW);
  assert.deepEqual(shape(s), [["#20", [[21]]]]);
  assert.deepEqual(shape(t), [[null, [[22]]]]);
  assert.deepEqual(t.prs[0].tray.base.items, ["Merge conflict with feature/x"]);
  const healthy = sessionViews([session("t")], [{ ...prs[2], github: { ...prs[2].github!, mergeable: "MERGEABLE" } }], [], NOW)[0];
  assert.equal(healthy.prs[0].tray.base.fine, "no conflict, up to date with feature/x");
  // Another Session's Stack on it alone doesn't make it a Feature branch.
  const [lone] = sessionViews([session("s"), session("t")], [prs[0], prs[2]], [], NOW);
  assert.deepEqual(shape(lone), [[null, [[20]]]]);
});

test("feature branch: its Stacks in base-first order, oldest bottom PR first", () => {
  const prs = [
    pr("s", { number: 25, headRefName: "c", baseRefName: "feature/x" }),
    pr("s", { number: 24, headRefName: "b2", baseRefName: "b1", stack: { number: 9, position: 2 } }),
    pr("s", { number: 22, headRefName: "b1", baseRefName: "feature/x", stack: { number: 9, position: 1 } }),
    pr("s", { number: 23, headRefName: "a", baseRefName: "feature/x" }),
    pr("s", { number: 20, headRefName: "feature/x", baseRefName: "main" }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(shape(v), [["#20", [[22, 24], [23], [25]]]]);
});

test("feature branch: a feature PR with nothing based on it is an ordinary draft PR", () => {
  const prs = [pr("s", { number: 20, headRefName: "feature/x", baseRefName: "main", isDraft: true })];
  const [v] = sessionViews([session("s")], prs, [branch("s", "feature/x", AT)], NOW);
  assert.deepEqual(shape(v), [[null, [[20]]]]);
  assert.equal(v.prs[0].draft, true);
});

test("feature branch: a hand-made branching stack reads as one", () => {
  // Two native stacks based on #10's head, which is based on the default branch.
  const prs = [
    pr("s", { number: 10, headRefName: "base-work", baseRefName: "main" }),
    pr("s", { number: 11, headRefName: "x1", baseRefName: "base-work", stack: { number: 11, position: 1 } }),
    pr("s", { number: 12, headRefName: "x2", baseRefName: "x1", stack: { number: 11, position: 2 } }),
    pr("s", { number: 13, headRefName: "y1", baseRefName: "base-work", stack: { number: 13, position: 1 } }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(shape(v), [["#10", [[11, 12], [13]]]]);
});

/** A Branch's Jira issue as `flow` prints it: fetched with `o`, or never fetched. */
const jira = (o: Partial<Jira> = {}): Jira => ({
  key: "ABC-12",
  url: "https://vanta.atlassian.net/browse/ABC-12",
  found: true,
  at: AT,
  summary: "Add widgets",
  status: "To Do",
  category: "new",
  type: "Story",
  ...o,
});
const unfetched = jira({ found: null, at: null, summary: null, status: null, category: null, type: null });
const missing = jira({ found: false, summary: null, status: null, category: null, type: null });

test("Jira state: by status category, in review by status name, then not fetched and not found", () => {
  const state = (j: Jira) => jiraView(j)!.state;
  assert.equal(state(jira({ status: "To Do", category: "new" })), "to do");
  assert.equal(state(jira({ status: "In Progress", category: "indeterminate" })), "in progress");
  assert.equal(state(jira({ status: "Code Review", category: "indeterminate" })), "in review");
  assert.equal(state(jira({ status: "Review", category: "indeterminate" })), "in review");
  assert.equal(state(jira({ status: "Done", category: "done" })), "done");
  // "Reviewed" in the done category is done, not in review.
  assert.equal(state(jira({ status: "Reviewed", category: "done" })), "done");
  assert.equal(state(unfetched), "not fetched");
  assert.equal(state(missing), "not found");
  assert.equal(jiraView(null), null);
});

test("Jira view: the square and link, drawn as to do before the first fetch, bare text when not found", () => {
  const url = "https://vanta.atlassian.net/browse/ABC-12";
  assert.deepEqual(jiraView(jira({ status: "Code Review", category: "indeterminate" })), {
    key: "ABC-12",
    url,
    state: "in review",
    square: "review",
    status: "Code Review",
    tip: "ABC-12 · Code Review · open in Jira",
  });
  assert.equal(jiraView(jira())!.square, "todo");
  assert.equal(jiraView(jira({ status: "In Progress", category: "indeterminate" }))!.square, "progress");
  assert.equal(jiraView(jira({ status: "Done", category: "done" }))!.square, "done");
  assert.deepEqual(jiraView(unfetched), {
    key: "ABC-12",
    url,
    state: "not fetched",
    square: "todo",
    status: null,
    tip: "not fetched yet",
  });
  assert.deepEqual(jiraView(missing), {
    key: "ABC-12",
    url: null,
    state: "not found",
    square: null,
    status: null,
    tip: "not found in Jira",
  });
});

test("Session views carry each PR's and each Branch's Jira issue", () => {
  const prs = [{ ...pr("s", { headRefName: "feat/x" }), jira: jira({ key: "ABC-1" }) }, pr("s", { number: 8, url: "#8", headRefName: "feat/y" })];
  const branches = [{ ...branch("s", "feat/z", AT), jira: missing }, branch("s", "feat/w", hoursAgo(1))];
  const [v] = sessionViews([session("s")], prs, branches, NOW);
  assert.deepEqual(v.prs.map((p) => p.jira?.key ?? null), ["ABC-1", null]);
  const heads = v.groups.flatMap((g) => (g.kind === "branch" ? [[g.head.name, g.head.jira?.state ?? null]] : []));
  assert.deepEqual(heads, [["feat/w", null], ["feat/z", "not found"]]);
});

test("Jira data age: minutes since the oldest fetch among listed Branches, only when over 15 minutes", () => {
  const ago = (min: number) => jira({ at: new Date(NOW - min * 60_000).toISOString() });
  const onPr = (j: Jira | null): Pr => ({ ...pr("s"), jira: j });
  const onBranch = (j: Jira | null): Branch => ({ ...branch("s", "b", AT), jira: j });
  assert.equal(jiraDataAge([], [], NOW), null);
  assert.equal(jiraDataAge([onPr(ago(1)), onPr(null)], [onBranch(ago(15))], NOW), null);
  assert.equal(jiraDataAge([onPr(ago(1)), onPr(ago(20.5))], [onBranch(ago(3))], NOW), 20);
  assert.equal(jiraDataAge([onPr(ago(1))], [onBranch(ago(42))], NOW), 42);
  // A key not found in Jira was still fetched, so its age counts.
  assert.equal(jiraDataAge([onPr({ ...missing, at: new Date(NOW - 30 * 60_000).toISOString() })], [], NOW), 30);
  // Never-fetched keys don't count.
  assert.equal(jiraDataAge([onPr(unfetched)], [onBranch(unfetched)], NOW), null);
});

const task = (slug: string, sessions: string[], resources: Task["resources"] = []): Task => ({
  slug,
  dir: `/efs-home/flow/tasks/${slug}`,
  sessions,
  resources,
});

test("session panel: counts PRs and Branches with no PR, or says there are none", () => {
  const prs = [pr("s", { number: 7, headRefName: "feat/x" }), pr("s", { number: 8, headRefName: "feat/y" })];
  const branches = [branch("s", "feat/z", AT), branch("one", "feat/w", AT)];
  const views = sessionViews([session("s"), session("one"), session("none")], prs, branches, NOW);
  const counts = (id: string) => sessionPanel(id, views, [])!.counts;
  assert.equal(counts("s"), "2 pull requests · 1 branch");
  assert.equal(counts("one"), "1 branch");
  assert.equal(counts("none"), "no pull requests");
  const [solo] = sessionViews([session("p")], [pr("p")], [], NOW);
  assert.equal(sessionPanel("p", [solo], [])!.counts, "1 pull request");
  // A Feature branch with no PR yet heading Stacks counts as a Branch.
  const [f] = sessionViews([session("f")], [pr("f", { number: 9, headRefName: "s1", baseRefName: "feature/a" })], [branch("f", "feature/a", AT)], NOW);
  assert.equal(sessionPanel("f", [f], [])!.counts, "1 pull request · 1 branch");
});

test("session panel: a Session on no Task, or on one not listed, is not part of a task", () => {
  const views = sessionViews([session("a"), session("b", { task: "gone" })], [], [], NOW);
  assert.equal(sessionPanel("a", views, [task("other", ["x"])])!.task, null);
  assert.equal(sessionPanel("b", views, [task("other", ["x"])])!.task, null);
  assert.equal(sessionPanel("missing", views, []), null);
});

test("session panel: resource and session counts, every Resource in order", () => {
  const res = Array.from({ length: 7 }, (_, i) => ({ url: `https://example.com/${i}`, title: null }));
  const views = sessionViews([session("a", { task: "w" }), session("b", { task: "w" })], [], [], NOW);
  const one = sessionPanel("a", views, [task("w", ["a"], res.slice(0, 1))])!;
  assert.equal(one.task!.counts, "1 resource");
  const many = sessionPanel("a", views, [task("w", ["a", "b"], res)])!;
  assert.equal(many.task!.counts, "7 resources · 2 sessions");
  assert.deepEqual(many.task!.resources.map((r) => r.url), res.map((r) => r.url));
  assert.equal(sessionPanel("a", views, [task("w", ["a"])])!.task!.counts, "0 resources");
});

test("resource: its title, else its URL shortened in mono; the host; file:// is path text", () => {
  const views = sessionViews([session("a", { task: "w" })], [], [], NOW);
  const resources = [
    { url: "https://docs.google.com/document/d/abc/edit?tab=t.0", title: "Design doc" },
    { url: "https://www.example.com/path/edit?x=1#y", title: null },
    { url: "file:///efs-home/weston/plan.md", title: null },
  ];
  const [titled, bare, file] = sessionPanel("a", views, [task("w", ["a"], resources)])!.task!.resources;
  assert.deepEqual(titled, { url: resources[0].url, label: "Design doc", mono: false, host: "docs.google.com", link: true });
  assert.deepEqual(bare, { url: resources[1].url, label: "example.com/path", mono: true, host: "example.com", link: true });
  assert.deepEqual(file, { url: resources[2].url, label: "/efs-home/weston/plan.md", mono: true, host: "", link: false });
});

test("short URL: drops the scheme, www., query and a trailing /edit", () => {
  assert.equal(shortUrl("https://www.notion.so/acme/Plan-123?pvs=4"), "notion.so/acme/Plan-123");
  assert.equal(shortUrl("http://acme.atlassian.net/browse/ABC-12"), "acme.atlassian.net/browse/ABC-12");
  assert.equal(shortUrl("https://docs.google.com/document/d/abc/edit"), "docs.google.com/document/d/abc");
  assert.equal(shortUrl("https://example.com/edits"), "example.com/edits");
});

test("session panel: also on this task lists the Task's other listed Sessions in rack order", () => {
  const views = sessionViews(
    [
      session("a", { task: "w", origin: { at: hoursAgo(3), cde: null, cdeId: null, pane: null, herdrSocket: null, cwd: null, transcriptPath: null } }),
      session("b", { task: "w", origin: { at: hoursAgo(1), cde: null, cdeId: null, pane: null, herdrSocket: null, cwd: null, transcriptPath: null } }),
      session("c", { task: "other" }),
      session("d", { task: "w", status: { at: AT, state: "done" } }),
    ],
    [],
    [],
    NOW,
  );
  const tasks = [task("w", ["a", "b", "d", "unlisted"]), task("other", ["c"])];
  assert.deepEqual(sessionPanel("a", views, tasks)!.task!.others.map((v) => v.id), ["b", "d"]);
  assert.deepEqual(sessionPanel("c", views, tasks)!.task!.others, []);
});

/** Each row as `#n` or the Branch name, its stage, and F for a Feature branch's head, ↳ and the
 * head's row for a Stack based on one; each edge as `from>to` (feat edges `from=>to`). */
function laneShape(v: SessionView) {
  const { rows, edges } = lanes(v);
  return {
    rows: rows.map((r) => `${r.head != null ? `↳${r.head}` : ""}${r.feature ? "F" : ""}${r.pr ? `#${r.pr.number}` : r.branch.name} ${r.stage}`),
    edges: edges.map((e) => `${e.from}${e.kind === "feat" ? "=>" : ">"}${e.to}`),
  };
}

test("lanes: each PR's stage; open and drafts in Open, mergeable in Ready, queued, merged and closed in Merge", () => {
  const prs = [
    pr("s", { number: 1, headRefName: "a" }),
    pr("s", { number: 2, headRefName: "b", isDraft: true }),
    pr("s", { number: 3, headRefName: "c", mergeable: "CONFLICTING" }),
    pr("s", { number: 4, headRefName: "d", mergeQueue: { position: 2, size: 5 } }),
    pr("s", { number: 5, headRefName: "e", state: "MERGED" }),
    pr("s", { number: 6, headRefName: "f", state: "CLOSED" }),
  ];
  const [v] = sessionViews([session("s")], prs, [branch("s", "wip", AT)], NOW);
  assert.deepEqual(laneShape(v), {
    rows: ["#1 ready", "#2 open", "#3 open", "#4 merge", "#5 merge", "#6 merge", "wip open"],
    edges: [],
  });
  // A row's id is the PR's URL or the Branch's key.
  assert.deepEqual(lanes(v).rows.map((r) => r.id).slice(-2), [prs[5].github!.url, "acme/widgets:wip"]);
});

test("lanes: a Stack's PRs bottom first, each joined to the one stacked on it", () => {
  const prs = [
    pr("s", { number: 2, headRefName: "b", baseRefName: "a", stack: { number: 1, position: 2 }, mergeable: "CONFLICTING" }),
    pr("s", { number: 1, headRefName: "a", stack: { number: 1, position: 1 } }),
    pr("s", { number: 3, headRefName: "c" }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(laneShape(v), { rows: ["#1 ready", "#2 open", "#3 ready"], edges: ["0>1"] });
});

test("lanes: a Feature branch heads its Stacks, joined to the bottom PR of each", () => {
  const prs = [
    pr("s", { number: 20, headRefName: "feature/x", baseRefName: "main", isDraft: true }),
    pr("s", { number: 21, headRefName: "a", baseRefName: "feature/x", stack: { number: 21, position: 1 } }),
    pr("s", { number: 22, headRefName: "b", baseRefName: "a", stack: { number: 21, position: 2 } }),
    pr("s", { number: 23, headRefName: "c", baseRefName: "feature/x" }),
  ];
  const [v] = sessionViews([session("s")], prs, [], NOW);
  assert.deepEqual(laneShape(v), {
    rows: ["F#20 open", "↳0#21 ready", "↳0#22 ready", "↳0#23 ready"],
    edges: ["0=>1", "1>2", "0=>3"],
  });
  // Before its feature PR exists, the Branch heads the group the same way.
  const [w] = sessionViews([session("s")], prs.slice(1), [branch("s", "feature/x", AT)], NOW);
  assert.deepEqual(laneShape(w), {
    rows: ["Ffeature/x open", "↳0#21 ready", "↳0#22 ready", "↳0#23 ready"],
    edges: ["0=>1", "1>2", "0=>3"],
  });
});
