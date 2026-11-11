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

/** The review column: a lamp, or, while reviews are requested, how many with `count`. */
export type ReviewLight = Light & { count: number | null };

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
  /** ready: mergeable (open, not a draft, all four columns fine). */
  node: "merged" | "closed" | "ready" | "open";
  draft: boolean;
  base: Light;
  ci: Light;
  feedback: Light;
  review: ReviewLight;
  /** True if the user should act on this PR: it is mergeable or a lamp is lit. */
  attention: boolean;
}

export interface SessionView {
  id: string;
  status: Light & { state: State | null; since: string | null };
  /** The Session's title, or "New session" until its first Stop writes one. */
  title: string;
  /** True while `title` is the placeholder. */
  untitled: boolean;
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
  if (!g.checks.length) return { lamp: "fine", tip: "CI: no required checks" };
  return { lamp: "fine", tip: "CI: passing" };
}

export function feedbackLight(g: Github): Light {
  const n = g.unresolvedReviewThreads;
  if (g.state !== "MERGED" && n > 0) {
    return { lamp: "act", tip: `feedback: ${n} unresolved thread${n === 1 ? "" : "s"}` };
  }
  return { lamp: "fine", tip: "feedback: none open" };
}

/** Changes requested or nobody asked to review → act; pending requests → their count;
 * else fine. Drafts follow the same rules. Reviewers who requested changes leave
 * `reviewRequests`, so that decision is shown as a lamp rather than counted. */
export function reviewLight(g: Github): ReviewLight {
  const light = (lamp: Lamp, tip: string, count: number | null = null): ReviewLight => ({ lamp, tip, count });
  if (g.state !== "OPEN") return light("fine", `review: ${g.state === "MERGED" ? "merged" : "closed"}`);
  if (g.reviewDecision === "CHANGES_REQUESTED") return light("act", "review: changes requested");
  const names = g.reviewRequests.map((r) => ("login" in r ? r.login : r.team));
  if (names.length) return light("fine", `review needed: ${names.join(", ")}`, names.length);
  if (g.reviewDecision === "REVIEW_REQUIRED") return light("act", "review: no reviewers requested");
  return light("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
}

export function prView(g: Github, pos: number): PrView {
  const base = baseLight(g);
  const ci = ciLight(g);
  const feedback = feedbackLight(g);
  const review = reviewLight(g);
  const lamps = [base, ci, feedback, review];
  const open = g.state === "OPEN";
  const ready = open && !g.isDraft && review.count == null && lamps.every((l) => l.lamp === "fine");
  return {
    pos,
    url: g.url,
    number: g.number,
    ...parseJira(g.title),
    additions: g.additions,
    deletions: g.deletions,
    node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : ready ? "ready" : "open",
    draft: g.isDraft,
    base,
    ci,
    feedback,
    review,
    attention: open && (ready || lamps.some((l) => l.lamp === "act")),
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

/** Done Sessions last; within each group the most recently started first, then those with no
 * start time. */
export function sessionViews(sessions: Session[], prs: Pr[]): SessionView[] {
  const done = (s: Session) => (s.status?.state === "done" ? 1 : 0);
  const started = (s: Session) => (s.origin ? Date.parse(s.origin.at) : -Infinity);
  const sorted = [...sessions].sort((a, b) => done(a) - done(b) || started(b) - started(a));
  return sorted.map((s) => {
    const stack = stackOf(s.id, prs);
    const state = s.status?.state ?? null;
    const cwd = s.origin?.cwd;
    const title = s.context?.title ?? null;
    return {
      id: s.id,
      status: { ...statusLight(state), state, since: s.status?.at ?? null },
      title: title ?? "New session",
      untitled: title == null,
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

/** The one light on a folded Session: act if it is waiting for you or any PR needs
 * attention, else fine. */
export function attentionLight(v: SessionView): Light {
  const n = v.stack.length;
  const prs = n ? `${n} pull request${n === 1 ? "" : "s"}` : "No pull requests";
  if (v.status.state === "waiting") return { lamp: "act", tip: `${prs}: waiting for you` };
  if (v.stack.some((p) => p.attention)) return { lamp: "act", tip: `${prs}: something needs action` };
  return { lamp: "fine", tip: `${prs}: nothing needs action` };
}

/** Done Sessions start folded; `toggled` holds the ids the user has flipped since load. */
export function isFolded(v: SessionView, toggled: Set<string>): boolean {
  return (v.status.state === "done") !== toggled.has(v.id);
}

/** Minutes since the oldest `github.at` among open PRs when that is over 3 minutes, else null. */
export function prDataAge(prs: Pr[], now = Date.now()): number | null {
  const ats = prs.flatMap((p) => (p.github?.state === "OPEN" ? [Date.parse(p.github.at)] : []));
  if (!ats.length) return null;
  const ms = now - Math.min(...ats);
  return ms > 3 * 60_000 ? Math.floor(ms / 60_000) : null;
}
