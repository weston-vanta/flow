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
  url: string | null;
}

export type ReviewRequest = { login: string; asCodeOwner: boolean } | { team: string; asCodeOwner: boolean };

/** A reviewer's latest review that approved or requested changes. */
export interface Review {
  login: string;
  state: "APPROVED" | "CHANGES_REQUESTED";
  /** Slugs of the teams the review was given on behalf of. */
  teams: string[];
}

/** An unresolved, not-outdated review thread, by its first comment. */
export interface Thread {
  login: string;
  path: string;
  line: number | null;
  body: string;
  url: string;
  createdAt: string;
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
  reviewRequests: ReviewRequest[];
  checks: Check[];
  stack: { position: number } | null;
  updatedAt: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  reviews: Review[];
  threads: Thread[];
  /** When the oldest default-branch commit the head lacks landed; null unless open and behind. */
  behindSince: string | null;
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
  /** `files / lines`, e.g. `3 / 540`. */
  size: string;
  /** `3 files, +500 −40`. */
  sizeTip: string;
  /** ready: mergeable (open, not a draft, all four columns fine). */
  node: "merged" | "closed" | "ready" | "open";
  draft: boolean;
  base: Light;
  ci: Light;
  feedback: Light;
  review: ReviewLight;
  /** True if the user should act on this PR: it is mergeable or a lamp is lit. */
  attention: boolean;
  tray: Tray;
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

const DAY = 24 * 60 * 60_000;

/** What the PR's base is called: the PR below it in the Stack, else its base branch. */
const baseName = (g: Github, below: number | null) => (below != null ? `#${below}` : g.baseRefName);

/** The PR's base problems, worded for the tray; empty when there are none or it is not open. */
function baseProblems(g: Github, below: number | null, now: number): string[] {
  if (g.state !== "OPEN") return [];
  const out: string[] = [];
  if (g.mergeable === "CONFLICTING") out.push(`Merge conflict with ${baseName(g, below)}`);
  if (g.behindSince && now - Date.parse(g.behindSince) > DAY) {
    out.push(`Behind ${g.baseRefName} since ${age(g.behindSince, now)} ago`);
  }
  return out;
}

/** A merged or closed PR shows every lamp fine. */
const done = (g: Github, column: string): Light | null =>
  g.state === "OPEN" ? null : { lamp: "fine", tip: `${column}: ${g.state === "MERGED" ? "merged" : "closed"}` };

/** Act on a conflict, or on being behind the default branch for more than a day.
 * `below` is the number of the PR below this one in its Stack, if any. */
export function baseLight(g: Github, below: number | null = null, now = Date.now()): Light {
  const problems = baseProblems(g, below, now);
  if (problems.length) {
    return { lamp: "act", tip: `base: ${problems.map((p) => p[0].toLowerCase() + p.slice(1)).join(" · ")}` };
  }
  return done(g, "base") ?? { lamp: "fine", tip: "base: no conflict" };
}

const failingChecks = (g: Github) => g.checks.filter((c) => c.conclusion && FAILING.has(c.conclusion));

/** From the required checks only, as the PR Writer stores them. */
export function ciLight(g: Github): Light {
  const closed = done(g, "CI");
  if (closed) return closed;
  const failing = failingChecks(g);
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
  const n = g.threads.length;
  if (g.state === "OPEN" && n > 0) {
    return { lamp: "act", tip: `feedback: ${n} unresolved thread${n === 1 ? "" : "s"}` };
  }
  return done(g, "feedback") ?? { lamp: "fine", tip: "feedback: none open" };
}

/** Changes requested or nobody asked to review → act; pending requests → their count;
 * else fine. Drafts follow the same rules. Reviewers who requested changes leave
 * `reviewRequests`, so that decision is shown as a lamp rather than counted. */
export function reviewLight(g: Github): ReviewLight {
  const light = (lamp: Lamp, tip: string, count: number | null = null): ReviewLight => ({ lamp, tip, count });
  const closed = done(g, "review");
  if (closed) return { ...closed, count: null };
  if (g.reviewDecision === "CHANGES_REQUESTED") return light("act", "review: changes requested");
  const names = g.reviewRequests.map((r) => ("login" in r ? r.login : r.team));
  if (names.length) return light("fine", `review needed: ${names.join(", ")}`, names.length);
  if (g.reviewDecision === "REVIEW_REQUIRED") return light("act", "review: no reviewers requested");
  return light("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
}

/** `below` is the number of the PR below this one in its Stack, if any. */
export function prView(g: Github, pos: number, below: number | null = null, now = Date.now()): PrView {
  const base = baseLight(g, below, now);
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
    size: `${g.changedFiles} / ${g.additions + g.deletions}`,
    sizeTip: `${g.changedFiles} file${g.changedFiles === 1 ? "" : "s"}, +${g.additions} −${g.deletions}`,
    node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : ready ? "ready" : "open",
    draft: g.isDraft,
    base,
    ci,
    feedback,
    review,
    attention: open && (ready || lamps.some((l) => l.lamp === "act")),
    tray: trayView(g, below, now),
  };
}

/** One tray section: its one-line fine statement, or the items behind its lamp. */
export type Section<T> = { fine: string; items: null } | { fine: null; items: T[] };

export interface TrayCheck {
  name: string;
  url: string | null;
}

export interface TrayThread {
  author: string;
  /** `path:line`, or the path alone for a file-level comment. */
  where: string;
  body: string;
  url: string;
  /** When the comment was posted, and how long ago that was ("3 h"). */
  at: string;
  ago: string;
}

export interface TrayReviewer {
  /** A login, or a team's slug when `team` is true. */
  name: string;
  team: boolean;
  state: "requested" | "approved" | "changes requested";
  /** True when CODEOWNERS made the pending request. */
  codeOwner: boolean;
  /** Slugs of the teams a review was given on behalf of. */
  teams: string[];
}

/** The tray's four sections, one per lamp. */
export interface Tray {
  /** Each item is a sentence: "Merge conflict with #9918", "Behind main since 2 d ago". */
  base: Section<string>;
  ci: Section<TrayCheck>;
  feedback: Section<TrayThread>;
  review: Section<TrayReviewer>;
}

function section<T>(items: T[], fine: string): Section<T> {
  return items.length ? { fine: null, items } : { fine, items: null };
}

/** Each reviewer once: a pending request wins over an earlier review, since GitHub drops a
 * request once someone reviews, so a person in both was re-requested. */
function reviewers(g: Github): TrayReviewer[] {
  const requested = g.reviewRequests.map((r) => ({
    name: "login" in r ? r.login : r.team,
    team: !("login" in r),
    state: "requested" as const,
    codeOwner: r.asCodeOwner,
    teams: [],
  }));
  const pending = new Set(g.reviewRequests.flatMap((r) => ("login" in r ? [r.login] : [])));
  const reviewed = g.reviews
    .filter((r) => !pending.has(r.login))
    .map((r) => ({
      name: r.login,
      team: false,
      state: r.state === "APPROVED" ? ("approved" as const) : ("changes requested" as const),
      codeOwner: false,
      teams: r.teams,
    }));
  return [...requested, ...reviewed];
}

/** The tray for one PR. `below` is the number of the PR below it in its Stack, if any. A
 * merged or closed PR shows every section's fine statement. */
export function trayView(g: Github, below: number | null = null, now = Date.now()): Tray {
  const word = g.state === "MERGED" ? "merged" : "closed";
  if (g.state !== "OPEN") {
    return {
      base: section([], word),
      ci: section([], "passed"),
      feedback: section([], "no open threads"),
      review: section([], word),
    };
  }
  const running = g.checks.some((c) => !c.conclusion || PENDING.has(c.conclusion));
  const threads = [...g.threads]
    .sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt))
    .map((t) => ({
      author: t.login,
      where: t.line == null ? t.path : `${t.path}:${t.line}`,
      body: t.body,
      url: t.url,
      at: t.createdAt,
      ago: age(t.createdAt, now),
    }));
  const review =
    g.reviewDecision === "REVIEW_REQUIRED"
      ? "required, none requested"
      : g.reviewDecision === "APPROVED"
        ? "approved"
        : "not required";
  return {
    base: section(baseProblems(g, below, now), below == null ? "no conflict, up to date" : "no conflict"),
    ci: section(
      failingChecks(g).map((c) => ({ name: c.name, url: c.url })),
      running ? "running" : "passing",
    ),
    feedback: section(threads, "no open threads"),
    review: section(reviewers(g), review),
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
export function sessionViews(sessions: Session[], prs: Pr[], now = Date.now()): SessionView[] {
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
      activity: activityLine(s),
      stack: stack.map((g, i) => {
        const below = stack.find((o) => o.headRefName === g.baseRefName)?.number ?? null;
        return prView(g, i + 1, below, now);
      }),
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
