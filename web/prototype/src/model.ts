// PROTOTYPE. The data shape the web app polls from flowd: the JSON of
// `flow session list` / `flow pr list`. Provisional: ticket 15 (Store
// interface) fixes the real facet schemas. Every facet carries `at`.

export type Status = "running" | "waiting" | "idle" | "done";

export interface Session {
  id: string;
  origin: { cde: string; pane: string; cwd: string; at: string };
  status: { value: Status; at: string };
  context: { title: string; branch: string | null; at: string };
  branches: { value: { repo: string; branch: string }[]; at: string };
}

export interface Pr {
  key: string; // owner/repo#number
  owner: string | null; // derived Session id, computed by flow
  github: {
    repo: string;
    number: number;
    url: string;
    title: string;
    isDraft: boolean;
    state: "OPEN" | "MERGED" | "CLOSED";
    headRefName: string;
    additions: number;
    deletions: number;
    mergeable: "MERGEABLE" | "CONFLICTING" | "UNKNOWN";
    ci: "SUCCESS" | "FAILURE" | "PENDING" | "NONE"; // required checks only
    unresolvedThreads: number;
    reviewDecision: "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED" | null;
    pendingCodeOwners: string[]; // approximation, see ticket 12
    stackPosition: number | null; // native GitHub stack position, bottom = 1
    at: string;
  };
}

// ---- derived view state, shared by every variant ----

export type Lamp = "act" | "fine" | "run" | "off";

export interface PrView {
  pr: Pr;
  pos: number;
  title: string;
  jira: string | null;
  node: "merged" | "mergeable" | "draft" | "open";
  base: { lamp: Lamp; tip: string };
  ci: { lamp: Lamp; tip: string };
  feedback: { lamp: Lamp; tip: string };
  review: { lamp: Lamp; count: number; tip: string };
  needs: string[]; // short reasons the user must act
}

export interface SessionView {
  s: Session;
  prs: PrView[];
  repo: string;
}

export function prView(pr: Pr, pos: number): PrView {
  const g = pr.github;
  const merged = g.state === "MERGED";
  const jm = /^\[([A-Z][A-Z0-9]*-\d+)\]\s*(.*)$/.exec(g.title);
  const base: PrView["base"] = merged
    ? { lamp: "fine", tip: "Base: merged" }
    : g.mergeable === "CONFLICTING"
      ? { lamp: "act", tip: "Base: conflict" }
      : { lamp: "fine", tip: "Base: no conflict" }; // behind is ignored: main moves too fast for it to mean anything
  const ci: PrView["ci"] =
    g.ci === "FAILURE" ? { lamp: "act", tip: "CI: failing" }
    : g.ci === "PENDING" ? { lamp: "run", tip: "CI: running" }
    : g.ci === "NONE" ? { lamp: "off", tip: "CI: no required checks" }
    : { lamp: "fine", tip: "CI: passing" };
  const feedback: PrView["feedback"] = !merged && g.unresolvedThreads > 0
    ? { lamp: "act", tip: `Feedback: ${g.unresolvedThreads} unresolved` }
    : { lamp: "fine", tip: "Feedback: none open" };
  // Ticket 12 is open. Approximation: pending code-owner requests, else the bare decision.
  const review: PrView["review"] = merged || g.reviewDecision === "APPROVED"
    ? { lamp: "fine", count: 0, tip: merged ? "Review: done" : "Review: approved" }
    : g.pendingCodeOwners.length > 0
      ? { lamp: "act", count: g.pendingCodeOwners.length, tip: "Review needed: " + g.pendingCodeOwners.join(", ") }
      : g.reviewDecision === "CHANGES_REQUESTED"
        ? { lamp: "act", count: 0, tip: "Review: changes requested" }
        : g.reviewDecision === "REVIEW_REQUIRED"
          ? { lamp: "act", count: 0, tip: "Review: required (reviewers unknown)" }
          : { lamp: "fine", count: 0, tip: "Review: none required" };
  const mergeable = !merged && !g.isDraft && [base, ci, feedback, review].every((x) => x.lamp === "fine");
  const needs: string[] = [];
  if (!merged) {
    if (base.lamp === "act") needs.push(base.tip.replace("Base: ", ""));
    if (ci.lamp === "act") needs.push("CI failing");
    if (feedback.lamp === "act") needs.push("open feedback");
    if (mergeable) needs.push("ready to merge");
  }
  return {
    pr, pos,
    title: jm ? jm[2] : g.title,
    jira: jm ? jm[1] : null,
    node: merged ? "merged" : mergeable ? "mergeable" : g.isDraft ? "draft" : "open",
    base, ci, feedback, review, needs,
  };
}

export function sessionViews(sessions: Session[], prs: Pr[]): SessionView[] {
  return sessions.map((s) => {
    // Stack = the Session's PRs in native stack order; missing positions sort last.
    const own = prs
      .filter((p) => p.owner === s.id)
      .sort((a, b) => (a.github.stackPosition ?? 1e9) - (b.github.stackPosition ?? 1e9));
    const repo = s.branches.value[0]?.repo ?? s.origin.cwd.split("/").slice(-1)[0];
    return { s, prs: own.map((p, i) => prView(p, i + 1)), repo };
  });
}

export function ago(iso: string, now = Date.now()): string {
  const m = Math.round((now - Date.parse(iso)) / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m} m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} d ago`;
}

export const statusLabel: Record<Status, string> = {
  running: "running", waiting: "waiting for you", idle: "idle", done: "done",
};
export const statusLamp: Record<Status, Lamp> = {
  running: "run", waiting: "act", idle: "fine", done: "fine",
};

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}
export const lamp = (l: Lamp, tip = "", sm = false) =>
  `<span class="lamp ${l === "off" ? "hollow" : l}${sm ? " sm" : ""}" title="${esc(tip)}"></span>`;
export const node = (v: PrView, sm = false) =>
  `<a class="node ${v.node}${sm ? " sm" : ""}" href="${esc(v.pr.github.url)}" target="_blank" rel="noreferrer" title="#${v.pr.github.number} · ${v.node}">${sm ? "" : v.pos}</a>`;
export const reviewCell = (v: PrView) =>
  v.review.count > 0
    ? `<span class="count" title="${esc(v.review.tip)}">${v.review.count}</span>`
    : lamp(v.review.lamp, v.review.tip);
