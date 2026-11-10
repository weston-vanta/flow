import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activityLine,
  baseLight,
  ciLight,
  feedbackLight,
  parseJira,
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

test("CI: failing → act; not completed → run; none → off; else fine", () => {
  const ok = check("COMPLETED", "SUCCESS", "lint");
  assert.equal(ciLight(gh({ checks: [ok, check("COMPLETED", "FAILURE")] })).lamp, "act");
  assert.equal(ciLight(gh({ checks: [check("COMPLETED", "TIMED_OUT")] })).lamp, "act");
  // A failure wins over a check still running.
  assert.equal(ciLight(gh({ checks: [check("IN_PROGRESS", null, "a"), check("COMPLETED", "FAILURE")] })).lamp, "act");
  assert.equal(ciLight(gh({ checks: [ok, check("IN_PROGRESS", null)] })).lamp, "run");
  assert.equal(ciLight(gh({ checks: [check("QUEUED", null)] })).lamp, "run");
  assert.equal(ciLight(gh({ checks: [] })).lamp, "off");
  assert.equal(ciLight(gh({ checks: [ok, check("COMPLETED", "SKIPPED")] })).lamp, "fine");
  assert.match(ciLight(gh({ checks: [ok, check("COMPLETED", "FAILURE", "e2e")] })).tip, /e2e/);
});

test("feedback: unresolved threads → act unless merged", () => {
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 2 })).lamp, "act");
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 2, state: "MERGED" })).lamp, "fine");
  assert.equal(feedbackLight(gh({ unresolvedReviewThreads: 0 })).lamp, "fine");
});

test("review: draft → off; merged, approved or null → fine; required → run; changes → act", () => {
  assert.equal(reviewLight(gh({ isDraft: true, reviewDecision: "CHANGES_REQUESTED" })).lamp, "off");
  assert.equal(reviewLight(gh({ state: "MERGED", reviewDecision: "REVIEW_REQUIRED" })).lamp, "fine");
  assert.equal(reviewLight(gh({ reviewDecision: "APPROVED" })).lamp, "fine");
  assert.equal(reviewLight(gh({ reviewDecision: null })).lamp, "fine");
  assert.equal(reviewLight(gh({ reviewDecision: "REVIEW_REQUIRED" })).lamp, "run");
  assert.equal(reviewLight(gh({ reviewDecision: "CHANGES_REQUESTED" })).lamp, "act");
});

test("review tooltip lists the requested people and teams", () => {
  const tip = reviewLight(
    gh({ reviewDecision: "REVIEW_REQUIRED", reviewRequests: [{ login: "ana" }, { team: "acme/web" }] }),
  ).tip;
  assert.match(tip, /requested: ana, acme\/web/);
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

test("needs attention: conflict, failing CI, open feedback, changes requested; never review required", () => {
  const all = prView(
    gh({
      mergeable: "CONFLICTING",
      checks: [check("COMPLETED", "FAILURE")],
      unresolvedReviewThreads: 1,
      reviewDecision: "CHANGES_REQUESTED",
    }),
    1,
  );
  assert.deepEqual(all.needs, ["conflict", "CI failing", "open feedback", "changes requested"]);
  assert.deepEqual(prView(gh({ reviewDecision: "REVIEW_REQUIRED" }), 1).needs, []);
  assert.deepEqual(prView(gh({ checks: [check("IN_PROGRESS", null)] }), 1).needs, []);
});

test("ready to merge: open, not a draft, all four lamps fine", () => {
  const ready = prView(gh(), 1);
  assert.equal(ready.ready, true);
  assert.equal(ready.node, "ready");
  assert.equal(prView(gh({ reviewDecision: null }), 1).ready, true);
  assert.equal(prView(gh({ state: "MERGED" }), 1).ready, false);
  assert.equal(prView(gh({ state: "CLOSED" }), 1).ready, false);
  assert.equal(prView(gh({ isDraft: true }), 1).ready, false);
  assert.equal(prView(gh({ reviewDecision: "REVIEW_REQUIRED" }), 1).ready, false);
  assert.equal(prView(gh({ checks: [check("IN_PROGRESS", null)] }), 1).ready, false);
  assert.equal(prView(gh({ checks: [] }), 1).ready, false);
  assert.equal(prView(gh({ unresolvedReviewThreads: 1 }), 1).ready, false);
});

test("PR nodes: draft dashed, merged, closed, open", () => {
  assert.equal(prView(gh({ isDraft: true }), 1).node, "draft");
  assert.equal(prView(gh({ state: "MERGED" }), 1).node, "merged");
  assert.equal(prView(gh({ state: "CLOSED" }), 1).node, "closed");
  assert.equal(prView(gh({ reviewDecision: "REVIEW_REQUIRED" }), 1).node, "open");
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
  assert.equal(one.status.lamp, "act");
  assert.equal(one.cde, "flow");
  assert.equal(one.repo, "acme/api");
  assert.deepEqual(
    one.stack.map((p) => [p.pos, p.number, p.jira, p.title]),
    [[1, 8, "ABC-12", "Add widgets"]],
  );
  assert.equal(two.title, "s2");
  assert.equal(two.repo, null);
  assert.deepEqual(two.stack, []);
  // Without PRs the repo falls back to the cwd's last segment.
  assert.equal(sessionViews([sessions[0]], [])[0].repo, "obsidian");
});
