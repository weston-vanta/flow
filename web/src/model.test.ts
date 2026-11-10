import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activityLine,
  attentionLight,
  isFolded,
  baseLight,
  ciLight,
  feedbackLight,
  parseJira,
  prDataAge,
  prView,
  reviewLight,
  sessionViews,
  stackOf,
  statusLight,
} from "./model.ts";
import type { Check, Github, Pr, Session } from "./model.ts";

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
    checks: [{ name: "build", status: "COMPLETED", conclusion: "SUCCESS" }],
    stack: null,
    updatedAt: AT,
    additions: 10,
    deletions: 2,
    unresolvedReviewThreads: 0,
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
});

test("base: merged → fine; conflicting → act; else fine", () => {
  assert.equal(baseLight(gh({ state: "MERGED", mergeable: "CONFLICTING" })).lamp, "fine");
  assert.equal(baseLight(gh({ mergeable: "CONFLICTING" })).lamp, "act");
  assert.equal(baseLight(gh({ mergeable: "MERGEABLE" })).lamp, "fine");
  assert.equal(baseLight(gh({ mergeable: "UNKNOWN" })).lamp, "fine");
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

test("feedback: unresolved threads → act unless merged", () => {
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 2 })).lamp, "act");
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 2, state: "MERGED" })).lamp, "fine");
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 0 })).lamp, "fine");
});

test("review: changes requested or nobody requested → act; pending requests → count; else fine", () => {
  const r = (o: Partial<Github>) => {
    const l = reviewLight(gh(o));
    return [l.lamp, l.count];
  };
  assert.deepEqual(r({ state: "MERGED", reviewDecision: "CHANGES_REQUESTED" }), ["fine", null]);
  assert.deepEqual(r({ state: "CLOSED", reviewDecision: "REVIEW_REQUIRED" }), ["fine", null]);
  assert.deepEqual(r({ reviewDecision: "CHANGES_REQUESTED", reviewRequests: [{ login: "ana" }] }), ["act", null]);
  assert.deepEqual(r({ reviewDecision: "REVIEW_REQUIRED", reviewRequests: [{ login: "ana" }, { team: "acme/web" }] }), ["fine", 2]);
  assert.deepEqual(r({ reviewDecision: null, reviewRequests: [{ login: "ana" }] }), ["fine", 1]);
  assert.deepEqual(r({ reviewDecision: "REVIEW_REQUIRED" }), ["act", null]);
  // Drafts follow the same rules.
  assert.deepEqual(r({ isDraft: true, reviewDecision: "REVIEW_REQUIRED" }), ["act", null]);
  assert.deepEqual(r({ reviewDecision: "APPROVED" }), ["fine", null]);
  assert.deepEqual(r({ reviewDecision: null }), ["fine", null]);
});

test("review tooltip names the requested people and teams", () => {
  const tip = reviewLight(
    gh({ reviewDecision: "REVIEW_REQUIRED", reviewRequests: [{ login: "ana" }, { team: "acme/web" }] }),
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

test("Jira key is parsed from a `[ABC-123] ` prefix and removed from the title", () => {
  assert.deepEqual(parseJira("[ABC-123] Fix it"), { jira: "ABC-123", title: "Fix it" });
  assert.deepEqual(parseJira("[PLAT2-9] Bump"), { jira: "PLAT2-9", title: "Bump" });
  assert.deepEqual(parseJira("Fix it [ABC-123]"), { jira: null, title: "Fix it [ABC-123]" });
  assert.deepEqual(parseJira("[abc-123] lower"), { jira: null, title: "[abc-123] lower" });
  assert.deepEqual(parseJira("[ABC] no number"), { jira: null, title: "[ABC] no number" });
  assert.deepEqual(parseJira("[ABC-123]no space"), { jira: null, title: "[ABC-123]no space" });
});

test("attention: an open PR that is mergeable or has a lit lamp", () => {
  assert.equal(prView(gh(), 1).attention, true);
  assert.equal(prView(gh({ mergeable: "CONFLICTING" }), 1).attention, true);
  assert.equal(prView(gh({ reviewDecision: "REVIEW_REQUIRED" }), 1).attention, true);
  assert.equal(prView(gh({ reviewRequests: [{ login: "ana" }] }), 1).attention, false);
  assert.equal(prView(gh({ checks: [check("IN_PROGRESS", null)] }), 1).attention, false);
  assert.equal(prView(gh({ isDraft: true }), 1).attention, false);
  assert.equal(prView(gh({ state: "CLOSED", unresolvedReviewThreads: 2 }), 1).attention, false);
  assert.equal(prView(gh({ state: "MERGED" }), 1).attention, false);
});

test("mergeable: open, not a draft, no review outstanding, all four lamps fine", () => {
  const node = (o: Partial<Github>) => prView(gh(o), 1).node;
  assert.equal(node({}), "ready");
  assert.equal(node({ reviewDecision: null }), "ready");
  assert.equal(node({ checks: [] }), "ready");
  assert.equal(node({ isDraft: true }), "open");
  assert.equal(node({ reviewRequests: [{ login: "ana" }] }), "open");
  assert.equal(node({ reviewDecision: "REVIEW_REQUIRED" }), "open");
  assert.equal(node({ checks: [check("IN_PROGRESS", null)] }), "open");
  assert.equal(node({ unresolvedReviewThreads: 1 }), "open");
  assert.equal(node({ state: "MERGED" }), "merged");
  assert.equal(node({ state: "CLOSED" }), "closed");
});

test("Stack: the Session's PRs by stack position, missing positions last", () => {
  const prs = [
    pr("s1", { number: 4 }),
    pr("s1", { number: 3, stack: { position: 2 } }),
    pr("s2", { number: 9, stack: { position: 1 } }),
    pr("s1", { number: 1 }),
    pr("s1", { number: 5, stack: { position: 1 } }),
    pr(null, { number: 6, stack: { position: 1 } }),
  ];
  assert.deepEqual(
    stackOf("s1", prs).map((g) => g.number),
    [5, 3, 1, 4],
  );
  assert.deepEqual(
    stackOf("s2", prs).map((g) => g.number),
    [9],
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
    one.stack.map((p) => [p.pos, p.number, p.jira, p.title]),
    [[1, 8, "ABC-12", "Add widgets"]],
  );
  // Until its first Stop writes a title, a Session shows a placeholder, not its id.
  assert.equal(two.title, "New session");
  assert.equal(two.untitled, true);
  assert.equal(two.repo, null);
  assert.deepEqual(two.stack, []);
  // Without PRs the repo falls back to the cwd's last segment.
  assert.equal(sessionViews([sessions[0]], [])[0].repo, "obsidian");
});

test("folded Session light: act if waiting or any PR needs attention, else fine", () => {
  const view = (state: Session["status"], ...gs: Partial<Github>[]) =>
    sessionViews([session("s", { status: state })], gs.map((g, i) => pr("s", { number: i + 1, ...g })))[0];
  const st = (state: "running" | "waiting" | "idle" | "done") => ({ at: AT, state });
  assert.equal(attentionLight(view(st("waiting"))).lamp, "act");
  assert.equal(attentionLight(view(st("running"))).lamp, "fine");
  assert.equal(attentionLight(view(st("running"), { reviewRequests: [{ login: "a" }] })).lamp, "fine");
  assert.equal(attentionLight(view(st("idle"), { reviewRequests: [{ login: "a" }] }, {})).lamp, "act");
  assert.equal(attentionLight(view(st("idle"), { mergeable: "CONFLICTING", reviewRequests: [{ login: "a" }] })).lamp, "act");
});

test("done Sessions start folded; a toggle flips the default", () => {
  const [done, idle] = sessionViews(
    [session("d", { status: { at: AT, state: "done" } }), session("i", { status: { at: AT, state: "idle" } })],
    [],
  );
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
