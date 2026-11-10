// The view model: pure functions from flowd's API JSON (`flow session list`, `flow pr list`)
// to what the dashboard shows. No DOM here, so `node --test` covers it directly.

/** A facet's fields with the time its fact was observed; `null` if never written. */
type Facet<T> = (T & { at: string }) | null;

export type State = "running" | "waiting" | "idle" | "done";

export interface Session {
  id: string;
  origin: Facet<{
    cde: string | null;
    cdeId: string | null;
    pane: string | null;
    herdrSocket: string | null;
    cwd: string | null;
    transcriptPath: string | null;
  }>;
  status: Facet<{ state: State }>;
  context: Facet<{ title: string | null; branch: string | null }>;
  activity: Facet<{ prompt: string | null; reply: string | null }>;
}

export interface Check {
  name: string;
  status: string | null;
  conclusion: string | null;
}

export interface Github {
  repo: string;
  number: number;
  url: string;
  title: string;
  state: "OPEN" | "CLOSED" | "MERGED";
  isDraft: boolean;
  headRefName: string;
  headRefOid: string;
  baseRefName: string;
  mergeable: string;
  reviewDecision: string | null;
  reviewRequests: ({ login: string } | { team: string })[];
  checks: Check[];
  stack: { position: number } | null;
  updatedAt: string;
  additions: number;
  deletions: number;
  unresolvedReviewThreads: number;
}

export interface Pr {
  key: string;
  owner: string | null;
  github: Facet<Github>;
}

/** act: the user must do something; run: in progress; fine; off: does not apply. */
export type Lamp = "act" | "fine" | "run" | "off";

export interface Light {
  lamp: Lamp;
  tip: string;
}

export interface PrView {
  /** 1-based place in the Stack, bottom first. */
  pos: number;
  url: string;
  number: number;
  /** The title without its Jira prefix. */
  title: string;
  jira: string | null;
  additions: number;
  deletions: number;
  node: "merged" | "closed" | "ready" | "draft" | "open";
  base: Light;
  ci: Light;
  feedback: Light;
  review: Light;
  /** Why the PR needs the user's attention; empty if it does not. */
  needs: string[];
  ready: boolean;
}

export interface SessionView {
  id: string;
  status: Light & { state: State | null; since: string | null };
  title: string;
  repo: string | null;
  cde: string | null;
  started: string | null;
  activity: string | null;
  stack: PrView[];
}

/** `[ABC-123] Title` → key and bare title. */
export function parseJira(title: string): { jira: string | null; title: string } {
  const m = /^\[([A-Z][A-Z0-9]*-\d+)\]\s+(.*)$/s.exec(title);
  return m ? { jira: m[1], title: m[2] } : { jira: null, title };
}

const FAILING = new Set([
  "FAILURE",
  "ERROR",
  "TIMED_OUT",
  "CANCELLED",
  "ACTION_REQUIRED",
  "STARTUP_FAILURE",
]);
const PENDING = new Set(["PENDING", "EXPECTED"]);

export function baseLight(g: Github): Light {
  if (g.state === "MERGED") return { lamp: "fine", tip: "base: merged" };
  if (g.mergeable === "CONFLICTING") return { lamp: "act", tip: "base: conflict" };
  return { lamp: "fine", tip: "base: no conflict" };
}

/** From the required checks only, as the PR Writer stores them. */
export function ciLight(g: Github): Light {
  const failing = g.checks.filter((c) => c.conclusion && FAILING.has(c.conclusion));
  if (failing.length) {
    return { lamp: "act", tip: `CI failing: ${failing.map((c) => c.name).join(", ")}` };
  }
  const running = g.checks.filter((c) => !c.conclusion || PENDING.has(c.conclusion));
  if (running.length) {
    return { lamp: "run", tip: `CI running: ${running.map((c) => c.name).join(", ")}` };
  }
  if (!g.checks.length) return { lamp: "off", tip: "CI: no required checks" };
  return { lamp: "fine", tip: "CI: passing" };
}

export function feedbackLight(g: Github): Light {
  const n = g.unresolvedReviewThreads;
  if (g.state !== "MERGED" && n > 0) {
    return { lamp: "act", tip: `feedback: ${n} unresolved thread${n === 1 ? "" : "s"}` };
  }
  return { lamp: "fine", tip: "feedback: none open" };
}

/** `reviewDecision` alone sets the lamp; the tooltip lists the raw review requests. */
export function reviewLight(g: Github): Light {
  const names = g.reviewRequests.map((r) => ("login" in r ? r.login : r.team));
  const requested = names.length ? ` · requested: ${names.join(", ")}` : "";
  if (g.isDraft) return { lamp: "off", tip: `review: draft${requested}` };
  if (g.state === "MERGED") return { lamp: "fine", tip: "review: merged" };
  if (g.reviewDecision === "APPROVED") return { lamp: "fine", tip: `review: approved${requested}` };
  if (g.reviewDecision === "REVIEW_REQUIRED") {
    return { lamp: "run", tip: `review: required${requested}` };
  }
  if (g.reviewDecision === "CHANGES_REQUESTED") {
    return { lamp: "act", tip: `review: changes requested${requested}` };
  }
  return { lamp: "fine", tip: `review: none required${requested}` };
}

export function prView(g: Github, pos: number): PrView {
  const base = baseLight(g);
  const ci = ciLight(g);
  const feedback = feedbackLight(g);
  const review = reviewLight(g);
  const open = g.state === "OPEN";
  const needs = open
    ? [
        base.lamp === "act" && "conflict",
        ci.lamp === "act" && "CI failing",
        feedback.lamp === "act" && "open feedback",
        review.lamp === "act" && "changes requested",
      ].filter((r): r is string => !!r)
    : [];
  const ready = open && !g.isDraft && [base, ci, feedback, review].every((l) => l.lamp === "fine");
  return {
    pos,
    url: g.url,
    number: g.number,
    ...parseJira(g.title),
    additions: g.additions,
    deletions: g.deletions,
    node:
      g.state === "MERGED"
        ? "merged"
        : g.state === "CLOSED"
          ? "closed"
          : ready
            ? "ready"
            : g.isDraft
              ? "draft"
              : "open",
    base,
    ci,
    feedback,
    review,
    needs,
    ready,
  };
}

/** The Session's PRs by GitHub stack position, bottom first; missing positions last. */
export function stackOf(session: string, prs: Pr[]): Github[] {
  const last = Number.MAX_SAFE_INTEGER;
  return prs
    .filter((p) => p.owner === session && p.github)
    .map((p) => p.github as Github)
    .sort(
      (a, b) =>
        (a.stack?.position ?? last) - (b.stack?.position ?? last) || a.number - b.number,
    );
}

const STATUS_LAMP: Record<State, Lamp> = { running: "run", waiting: "act", idle: "fine", done: "fine" };
const STATUS_LABEL: Record<State, string> = {
  running: "running",
  waiting: "waiting for you",
  idle: "idle",
  done: "done",
};

export function statusLight(state: State | null): Light {
  return state ? { lamp: STATUS_LAMP[state], tip: STATUS_LABEL[state] } : { lamp: "off", tip: "unknown" };
}

/** The prompt while running (or while no reply exists yet), else the reply. */
export function activityLine(s: Session): string | null {
  const a = s.activity;
  if (!a) return null;
  return s.status?.state === "running" || a.reply == null ? a.prompt : a.reply;
}

export function sessionViews(sessions: Session[], prs: Pr[]): SessionView[] {
  return sessions.map((s) => {
    const stack = stackOf(s.id, prs);
    const state = s.status?.state ?? null;
    const cwd = s.origin?.cwd;
    return {
      id: s.id,
      status: { ...statusLight(state), state, since: s.status?.at ?? null },
      title: s.context?.title ?? s.id,
      repo: stack[0]?.repo ?? (cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null),
      cde: s.origin?.cde ?? null,
      started: s.origin?.at ?? null,
      activity: activityLine(s),
      stack: stack.map((g, i) => prView(g, i + 1)),
    };
  });
}

/** "just now", "5 min", "3 h", "2 d": how long ago `iso` was. */
export function age(iso: string, now = Date.now()): string {
  const m = Math.floor((now - Date.parse(iso)) / 60_000);
  if (m < 1) return "just now";
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h`;
  return `${Math.floor(h / 24)} d`;
}
