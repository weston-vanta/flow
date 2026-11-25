// The view model: pure functions from flowd's API JSON (`flow session list`, `flow pr list`,
// `flow branch list`, `flow task list`) to what the dashboard shows. No DOM here, so `node --test` covers it directly.

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
  /** The slug of the Task it joined, if any. */
  task: string | null;
}

/** Something outside flow a Task points at. */
export interface Resource {
  url: string;
  title: string | null;
}

/** A Task as `flow task list` prints it. */
export interface Task {
  slug: string;
  dir: string;
  /** The ids of its listed Sessions. */
  sessions: string[];
  resources: Resource[];
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
  /** The PR's GitHub stack and place in it; null outside a stack, a Stack of one. */
  stack: { number: number; position: number } | null;
  updatedAt: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  reviews: Review[];
  threads: Thread[];
  /** When the oldest commit on its base that the head lacks landed; null unless open and behind. */
  behindSince: string | null;
  /** Its place in its repository's merge queue (1 merges next); null unless queued. */
  mergeQueue: { position: number; size: number } | null;
}

/** A Branch's Jira issue as `flow` prints it. All but `key` and `url` are null until the Jira
 * poller first fetches it; when `found` is false, only `at` joins them. */
export interface Jira {
  key: string;
  url: string;
  found: boolean | null;
  at: string | null;
  summary: string | null;
  status: string | null;
  /** The status category's key: `new`, `indeterminate` or `done`. */
  category: string | null;
  type: string | null;
}

export interface Pr {
  key: string;
  owner: string | null;
  github: Facet<Github>;
  /** The Jira issue of its head Branch. */
  jira: Jira | null;
}

/** A Branch a Session created a worktree for; `at` is when. */
export interface Branch {
  key: string;
  repo: string;
  branch: string;
  owner: string;
  at: string;
  jira: Jira | null;
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
  /** 1-based place in the Stack, bottom first; null in a Stack of one. */
  pos: number | null;
  url: string;
  number: number;
  /** The PR's GitHub title, unchanged. */
  title: string;
  /** `3 files, +500 −40`. */
  sizeTip: string;
  /** ready: mergeable (open, not a draft, approved or no review required, all four columns fine). queued: open and in the
   * merge queue, so never ready. */
  node: "merged" | "closed" | "queued" | "ready" | "open";
  draft: boolean;
  /** An open PR's place in the merge queue: `merge queue · 37 of 214`, and how much of its
   * badge's ring is filled, 0 at the back to 1 at the front. Null unless queued. */
  queue: { text: string; fill: number } | null;
  base: Light;
  ci: Light;
  feedback: Light;
  review: ReviewLight;
  tray: Tray;
  jira: JiraView | null;
}

export type JiraState = "to do" | "in progress" | "in review" | "done" | "not fetched" | "not found";

/** A Branch's Jira issue: a square plus its key, linked to Jira, or the bare key when Jira
 * didn't return it. */
export interface JiraView {
  key: string;
  /** Null when not found: the key is plain text. */
  url: string | null;
  state: JiraState;
  /** Empty, half-filled, cross-hatched or full; null when not found. Not fetched draws as to do. */
  square: "todo" | "progress" | "review" | "done" | null;
  /** The status's name in Jira, for the window header; null until fetched. */
  status: string | null;
  tip: string;
}

/** A Session's Branch that heads no PR. */
export interface BranchView {
  /** `repo:branch`, as `flow branch list` keys it. */
  key: string;
  repo: string;
  name: string;
  jira: JiraView | null;
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
  /** The slug of its Task, if any. */
  task: string | null;
  /** The Session's Stacks and Branches with no PR, as drawn. */
  groups: Group[];
  /** The PRs of `groups`, flattened in the order they are drawn. */
  prs: PrView[];
}

/** What the rack draws as one unit. A "stack" is an ordinary Stack, the one entry of `stacks`.
 * A "branch" is a Branch with no PR: its BranchView, with no `stacks`, or, as a Feature branch
 * before its feature PR exists, with the Stacks its owner based on it. A "feature" is a Feature
 * branch's feature PR with those Stacks. `stacks` are oldest bottom PR first. */
export type Group =
  | { kind: "stack"; head: null; stacks: PrView[][] }
  | { kind: "branch"; head: BranchView; stacks: PrView[][] }
  | { kind: "feature"; head: PrView; stacks: PrView[][] };

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

/** Act on a conflict, or on being behind its base for more than a day.
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
 * `reviewRequests`, so that decision is shown as a lamp rather than counted. Changes are
 * requested by any such review, as on a stacked PR, whose base requires no review, GitHub
 * gives no `reviewDecision`. */
export function reviewLight(g: Github): ReviewLight {
  const light = (lamp: Lamp, tip: string, count: number | null = null): ReviewLight => ({ lamp, tip, count });
  const closed = done(g, "review");
  if (closed) return { ...closed, count: null };
  if (g.reviewDecision === "CHANGES_REQUESTED" || g.reviews.some((r) => r.state === "CHANGES_REQUESTED")) {
    return light("act", "review: changes requested");
  }
  const names = g.reviewRequests.map((r) => ("login" in r ? r.login : r.team));
  if (names.length) return light("fine", `review needed: ${names.join(", ")}`, names.length);
  if (g.reviewDecision === "REVIEW_REQUIRED") return light("act", "review: no reviewers requested");
  return light("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
}

const SQUARE = { "to do": "todo", "in progress": "progress", "in review": "review", done: "done" } as const;

/** Judged from the status category, with "in review" told apart by the status name. */
export function jiraView(j: Jira | null): JiraView | null {
  if (!j) return null;
  const { key, url } = j;
  if (j.found == null) return { key, url, state: "not fetched", square: "todo", status: null, tip: "not fetched yet" };
  if (!j.found) return { key, url: null, state: "not found", square: null, status: null, tip: "not found in Jira" };
  const state =
    j.category === "done"
      ? "done"
      : j.category === "indeterminate"
        ? /review/i.test(j.status ?? "")
          ? "in review"
          : "in progress"
        : "to do";
  const status = j.status ?? state;
  return { key, url, state, square: SQUARE[state], status, tip: `${key} · ${status} · open in Jira` };
}

/** `below` is the number of the PR below this one in its Stack, if any. */
export function prView(
  g: Github,
  pos: number | null,
  below: number | null = null,
  now = Date.now(),
  jira: Jira | null = null,
): PrView {
  const base = baseLight(g, below, now);
  const ci = ciLight(g);
  const feedback = feedbackLight(g);
  const review = reviewLight(g);
  const lamps = [base, ci, feedback, review];
  const open = g.state === "OPEN";
  const q = open ? g.mergeQueue : null;
  // GitHub's review decision rules: pending requests don't block a PR it counts as approved.
  const ready = !q && open && !g.isDraft && g.reviewDecision !== "REVIEW_REQUIRED" && lamps.every((l) => l.lamp === "fine");
  return {
    pos,
    url: g.url,
    number: g.number,
    title: g.title,
    sizeTip: `${g.changedFiles} file${g.changedFiles === 1 ? "" : "s"}, +${g.additions} −${g.deletions}`,
    node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : q ? "queued" : ready ? "ready" : "open",
    draft: g.isDraft,
    queue: q && {
      text: `merge queue · ${q.position} of ${q.size}`,
      fill: q.size > 1 ? Math.min(Math.max((q.size - q.position) / (q.size - 1), 0), 1) : 1,
    },
    base,
    ci,
    feedback,
    review,
    tray: trayView(g, below, now),
    jira: jiraView(jira),
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
  /** True when the body runs past the tray's clamp (about 4 lines), so it gets a toggle. */
  long: boolean;
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
      long: t.body.length > 280 || t.body.split("\n").length > 4,
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
    base: section(baseProblems(g, below, now), below == null ? `no conflict, up to date with ${g.baseRefName}` : "no conflict"),
    ci: section(
      failingChecks(g).map((c) => ({ name: c.name, url: c.url })),
      running ? "running" : "passing",
    ),
    feedback: section(threads, "no open threads"),
    review: section(reviewers(g), review),
  };
}

/** The Session's PRs grouped by GitHub stack number into Stacks, each bottom first by
 * position. A PR outside a GitHub stack is a Stack of one. Stacks are sorted by their bottom
 * PR's number, so each keeps its place as PRs are added on top or its bottom merges. */
export function stacksOf(session: string, prs: Pr[]): Github[][] {
  const groups = new Map<number | string, Github[]>();
  for (const p of prs) {
    const g = p.github;
    if (p.owner !== session || !g) continue;
    const id = g.stack?.number ?? `#${g.number}`;
    groups.set(id, [...(groups.get(id) ?? []), g]);
  }
  return [...groups.values()]
    .map((s) => s.sort((a, b) => a.stack!.position - b.stack!.position || a.number - b.number))
    .sort((a, b) => a[0].number - b[0].number);
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
export function sessionViews(sessions: Session[], prs: Pr[], branches: Branch[] = [], now = Date.now()): SessionView[] {
  const heads = new Set(prs.flatMap((p) => (p.github ? [`${p.github.repo}:${p.github.headRefName}`] : [])));
  const jiras = new Map(prs.flatMap((p) => (p.github ? [[p.github.url, p.jira]] : [])));
  const loose = branches.filter((b) => !heads.has(b.key)).sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  const done = (s: Session) => (s.status?.state === "done" ? 1 : 0);
  const started = (s: Session) => (s.origin ? Date.parse(s.origin.at) : -Infinity);
  const sorted = [...sessions].sort((a, b) => done(a) - done(b) || started(b) - started(a));
  // Branches that are not the default branch: every PR's head and every listed Branch.
  const named = new Set([...heads, ...branches.map((b) => b.key)]);
  const head = (g: Github) => `${g.repo}:${g.headRefName}`;
  const base = (g: Github) => `${g.repo}:${g.baseRefName}`;
  return sorted.map((s) => {
    const raw = stacksOf(s.id, prs);
    const view = (stack: Github[]) =>
      stack.map((g, i) => {
        const below = stack.find((o) => o.headRefName === g.baseRefName)?.number ?? null;
        return prView(g, stack.length > 1 ? i + 1 : null, below, now, jiras.get(g.url) ?? null);
      });
    /** The Session's Stacks based on the Branch `key`, as Feature-branch Stacks are. */
    const on = (key: string) => raw.filter((st) => base(st[0]) === key).map(view);
    const own = loose.filter((b) => b.owner === s.id);
    // A feature PR: a Stack of one based on the default branch, with Stacks based on its head.
    const feature = new Set(raw.filter((st) => st.length === 1 && !named.has(base(st[0])) && on(head(st[0])).length).map((st) => head(st[0])));
    const fbranches = new Set([...feature, ...own.map((b) => b.key)]);
    const groups: Group[] = [
      ...raw.flatMap((st): Group[] => {
        if (fbranches.has(base(st[0]))) return [];
        if (feature.has(head(st[0]))) return [{ kind: "feature", head: view(st)[0], stacks: on(head(st[0])) }];
        return [{ kind: "stack", head: null, stacks: [view(st)] }];
      }),
      ...own.map((b): Group => ({ kind: "branch", head: { key: b.key, repo: b.repo, name: b.branch, jira: jiraView(b.jira) }, stacks: on(b.key) })),
    ];
    const state = s.status?.state ?? null;
    const cwd = s.origin?.cwd;
    const title = s.context?.title ?? null;
    return {
      id: s.id,
      status: { ...statusLight(state), state, since: s.status?.at ?? null },
      title: title ?? "New session",
      untitled: title == null,
      repo: raw[0]?.[0].repo ?? own[0]?.repo ?? (cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null),
      cde: s.origin?.cde ?? null,
      activity: activityLine(s),
      task: s.task,
      groups,
      prs: groups.flatMap((g) => [...(g.kind === "feature" ? [g.head] : []), ...g.stacks.flat()]),
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

/** Done Sessions and Sessions with no PRs start folded; `toggled` holds the ids the user has
 * flipped since load. */
export function isFolded(v: SessionView, toggled: Set<string>): boolean {
  return (v.status.state === "done" || !v.prs.length) !== toggled.has(v.id);
}

/** The board's three stage columns. */
export type Stage = "open" | "ready" | "merge";

/** One node of a Session's lane: a PR or a Branch with no PR, in its stage column. `id` is the
 * PR's URL or the Branch's key, and selects it. `feature` marks a Feature branch's head,
 * `head`, for a PR in a Stack based on one, is the index of that head's row. */
export type Row = { id: string; stage: Stage; feature: boolean; head: number | null } & (
  | { pr: PrView; branch: null }
  | { pr: null; branch: BranchView }
);

/** A line from row `from` down to row `to`: `stack` joins a PR to the one stacked on it, `feat`
 * a Feature branch's head to the bottom PR of each Stack based on it. */
export interface Edge {
  from: number;
  to: number;
  kind: "stack" | "feat";
}

/** Merged, closed and queued PRs are in Merge, mergeable ones in Ready, the rest and every
 * Branch in Open. */
export const stageOf = (p: PrView | null): Stage =>
  !p ? "open" : p.node === "ready" ? "ready" : p.node === "open" ? "open" : "merge";

/** A Session's groups as rows in the order drawn, one per PR or Branch, and the edges between them. */
export function lanes(v: SessionView): { rows: Row[]; edges: Edge[] } {
  const rows: Row[] = [];
  const edges: Edge[] = [];
  const pr = (p: PrView, feature: boolean, head: number | null) =>
    rows.push({ id: p.url, stage: stageOf(p), feature, head, pr: p, branch: null }) - 1;
  /** Adds a Stack's rows, joined bottom to top. */
  const stack = (s: PrView[], head: number | null) =>
    s.forEach((p, j) => {
      const i = pr(p, false, head);
      if (j) edges.push({ from: i - 1, to: i, kind: "stack" });
    });
  for (const g of v.groups) {
    if (g.kind === "stack") {
      stack(g.stacks[0], null);
      continue;
    }
    const feature = g.stacks.length > 0;
    const head =
      g.kind === "feature"
        ? pr(g.head, feature, null)
        : rows.push({ id: g.head.key, stage: "open", feature, head: null, pr: null, branch: g.head }) - 1;
    for (const s of g.stacks) {
      edges.push({ from: head, to: rows.length, kind: "feat" });
      stack(s, head);
    }
  }
  return { rows, edges };
}

/** The id of the node ↓ (`dir` 1) or ↑ (`dir` -1) selects after the one at `id`, in the order
 * the page draws them: Session by Session, row by row. Nodes in folded Sessions are skipped.
 * Wraps at both ends; from nothing selected (or a node no longer visible), ↓ picks the first and
 * ↑ the last. Null when no node is visible. */
export function stepPr(views: SessionView[], toggled: Set<string>, id: string | null, dir: 1 | -1): string | null {
  const order = views.filter((v) => !isFolded(v, toggled)).flatMap((v) => lanes(v).rows.map((r) => r.id));
  if (!order.length) return null;
  const i = id == null ? -1 : order.indexOf(id);
  if (i < 0) return order[dir > 0 ? 0 : order.length - 1];
  return order[(i + dir + order.length) % order.length];
}

/** Minutes since the oldest `github.at` among open PRs when that is over 3 minutes, else null. */
export function prDataAge(prs: Pr[], now = Date.now()): number | null {
  const ats = prs.flatMap((p) => (p.github?.state === "OPEN" ? [Date.parse(p.github.at)] : []));
  if (!ats.length) return null;
  const ms = now - Math.min(...ats);
  return ms > 3 * 60_000 ? Math.floor(ms / 60_000) : null;
}

/** Minutes since the oldest Jira fetch among the listed PRs' and Branches' Jira issues when that
 * is over 15 minutes, else null. Keys never fetched don't count. */
export function jiraDataAge(prs: Pr[], branches: Branch[], now = Date.now()): number | null {
  const ats = [...prs, ...branches].flatMap((x) => (x.jira?.at ? [Date.parse(x.jira.at)] : []));
  if (!ats.length) return null;
  const ms = now - Math.min(...ats);
  return ms > 15 * 60_000 ? Math.floor(ms / 60_000) : null;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** A URL without its scheme, `www.`, query, fragment and a trailing `/` or `/edit`. */
export const shortUrl = (url: string) =>
  url.replace(/^[a-z]+:\/\//i, "").replace(/^www\./i, "").replace(/[?#].*$/, "").replace(/\/(edit)?$/, "");

/** How the Session panel shows a Resource: its title, else its short URL in mono. A `file://`
 * Resource is its path as text, with no host and no link. */
export interface ResourceView {
  url: string;
  label: string;
  mono: boolean;
  /** The URL's host without `www.`; empty for `file://`. */
  host: string;
  link: boolean;
}

function resourceView(r: Resource): ResourceView {
  const file = /^file:\/\//i.test(r.url);
  const short = file ? r.url.replace(/^file:\/\//i, "") : shortUrl(r.url);
  return {
    url: r.url,
    label: r.title ?? short,
    mono: r.title == null,
    host: file ? "" : short.split("/")[0],
    link: !file,
  };
}

/** The Session panel: the Session, "N pull requests · N branches", and its Task. */
export interface PanelView {
  session: SessionView;
  counts: string;
  /** Null when the Session is not part of a listed Task. */
  task: {
    /** "N resources", plus "· N sessions" when more than one. */
    counts: string;
    resources: ResourceView[];
    /** The Task's other listed Sessions, in rack order. */
    others: SessionView[];
  } | null;
}

/** The panel of Session `id`, or null when it isn't in `views`. */
export function sessionPanel(id: string, views: SessionView[], tasks: Task[]): PanelView | null {
  const v = views.find((w) => w.id === id);
  if (!v) return null;
  const branches = v.groups.filter((g) => g.kind === "branch").length;
  const parts = [v.prs.length && plural(v.prs.length, "pull request"), branches && plural(branches, "branch", "branches")].filter(Boolean);
  const t = v.task == null ? null : tasks.find((x) => x.slug === v.task);
  return {
    session: v,
    counts: parts.join(" · ") || "no pull requests",
    task: t
      ? {
          counts: plural(t.resources.length, "resource") + (t.sessions.length > 1 ? ` · ${t.sessions.length} sessions` : ""),
          resources: t.resources.map(resourceView),
          others: views.filter((w) => w.id !== v.id && t.sessions.includes(w.id)),
        }
      : null,
  };
}
