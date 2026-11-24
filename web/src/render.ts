// The board as HTML strings: a top bar, a lane per Session with its PRs and Branches as nodes in
// the Open, Ready and Merge columns joined by edges, the panel showing the selected PR, Branch or
// Session, and the legend behind `h`.
import { age, isFolded, lanes } from "./model.ts";
import type { BranchView, Edge, JiraView, Light, PanelView, PrView, ReviewLight, Row, SessionView, Stage } from "./model.ts";

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** The four signals: the key a dot selects its PR's section at, and its heading. */
const COLUMNS = [
  ["base", "base"],
  ["ci", "CI"],
  ["feedback", "feedback"],
  ["review", "review"],
] as const;
export type Column = (typeof COLUMNS)[number][0];

/** The selected node's panel: its id (a PR's URL or a Branch's key) and the threads expanded
 * past their clamp, by URL. */
export interface TrayState {
  id: string;
  expanded: Set<string>;
}

const STAGES: [Stage, string][] = [
  ["open", "Open"],
  ["ready", "Ready"],
  ["merge", "Merge"],
];

/** A signal's look: act, run, count (reviews pending), ok or none. */
const sig = (l: Light | ReviewLight) =>
  "count" in l && l.count != null ? "count" : l.lamp === "fine" ? "ok" : l.lamp === "off" ? "none" : l.lamp;

const NODE_WORD = { merged: "merged", closed: "closed", queued: "in merge queue", ready: "mergeable" };
const nodeWord = (p: PrView) => (p.node === "open" ? (p.draft ? "draft" : "not yet mergeable") : NODE_WORD[p.node]);

/** A PR's badge: its state, a dashed ring on a draft, and the merge queue's progress as an arc. */
const badge = (p: PrView, text: string) =>
  `<span class="bd ${p.node}${p.draft && p.node === "open" ? " draft" : ""}"${p.queue ? ` style="--fill:${p.queue.fill.toFixed(3)}"` : ""}>${esc(text)}</span>`;

/** Pixel geometry of a lane's nodes and edges. Rows are 28px tall at a 36px pitch below 12px of
 * padding; Open is fluid, Ready and Merge 96px each. A Stack based on a Feature branch is
 * indented where it shares the Feature branch's column. */
const PITCH = 36;
const NODE = 28;
const PAD = 12;
const NARROW = 96;
const COL: Record<Stage, number> = { open: 0, ready: 1, merge: 2 };
/** A column's left edge inside the lane's area. */
const xl = (c: number) => (c === 0 ? "0px" : `(100% - ${(3 - c) * NARROW}px)`);
/** A Stack based on a Feature branch is indented only where it shares the head's column, so the
 * two lines run apart there; elsewhere its nodes get the column's full width. */
const indent = (r: Row, rows: Row[]) =>
  r.head != null && rows[r.head].stage === r.stage ? (COL[r.stage] === 0 ? 24 : 16) : 0;
const top = (i: number) => PAD + i * PITCH;

/** An edge as a positioned hairline: straight down within a column, else an elbow that turns
 * into the lower node from its column's side. */
function edge(e: Edge, rows: Row[]): string {
  const a = rows[e.from];
  const b = rows[e.to];
  const [ca, cb, ia, ib] = [COL[a.stage], COL[b.stage], indent(a, rows), indent(b, rows)];
  const y = top(e.from) + NODE;
  const h = top(e.to) + NODE / 2 - y;
  const cls = `edge e-${e.kind}`;
  if (ca === cb && ia === ib) {
    return `<i class="${cls} e-v" style="left:calc(${xl(ca)} + ${20 + ia}px);top:${y}px;height:${top(e.to) - y}px"></i>`;
  }
  if (cb < ca) {
    return `<i class="${cls} e-l" style="left:calc(${xl(cb + 1)} - 6px);width:calc(${xl(ca)} - ${xl(cb + 1)} + ${26 + ia}px);top:${y}px;height:${h}px"></i>`;
  }
  return `<i class="${cls} e-r" style="left:calc(${xl(ca)} + ${20 + ia}px);width:calc(${xl(cb)} - ${xl(ca)} + ${ib - ia - 14}px);top:${y}px;height:${h}px"></i>`;
}

/** A dot: selects its PR at its column's section. */
const dot = (p: PrView, col: Column, label: string) => {
  const l = p[col];
  return `<button type="button" class="t-${sig(l)}" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}"></button>`;
};

/** A node: badge, number, and in Open the title and four dots. A Branch is its name in mono. */
function node(r: Row, i: number, rows: Row[], sel: string | null): string {
  const c = COL[r.stage];
  const full = c === 0;
  const tuned = r.id === sel ? " tuned" : "";
  const style = `grid-row:${i + 1};grid-column:${c + 1};margin-left:${6 + indent(r, rows)}px`;
  if (r.branch) {
    const b = r.branch;
    const tip = `${b.name} · ${r.feature ? "feature branch, " : ""}no pull request yet${b.jira ? ` · ${b.jira.key}` : ""}`;
    return `<div class="n7 local${r.feature ? " feat" : ""}${tuned}" style="${style}" title="${esc(tip)}">
      <span class="bd">${r.feature ? "F" : ""}</span>
      <button type="button" class="tl" data-open="${esc(r.id)}" aria-label="${esc(`Select branch ${b.name}, no pull request yet`)}">${esc(b.name)}</button>
    </div>`;
  }
  const p = r.pr;
  const word = r.feature ? `feature branch · ${nodeWord(p)}` : nodeWord(p);
  const aria = `Select #${p.number}, ${p.title}, ${word}`;
  const tip = `#${p.number} ${p.title} · ${word}${p.queue ? ` · ${p.queue.text}` : ""}`;
  const st = p.node === "closed" ? "merged" : p.node;
  return `<div class="n7 st-${st}${r.feature ? " feat" : ""}${full ? "" : " compact"}${tuned}" style="${style}" title="${esc(tip)}">
      ${badge(p, r.feature ? "F" : String(p.pos ?? ""))}
      <button type="button" class="nm mono" data-open="${esc(p.url)}" aria-label="${esc(aria)}">${p.number}</button>${
        full
          ? `<button type="button" class="tl" data-open="${esc(p.url)}" tabindex="-1">${esc(p.title)}</button><span class="dots">${COLUMNS.map(([col, label]) => dot(p, col, label)).join("")}</span>`
          : ""
      }
    </div>`;
}

const ext = (url: string, cls: string, title: string, text: string) =>
  `<a class="${cls}" href="${esc(url)}" target="_blank" rel="noreferrer" title="${esc(title)}">${esc(text)}</a>`;

/** One panel section's items, by column. */
function items(p: PrView, col: Column, expanded: Set<string>): string {
  const t = p.tray;
  if (col === "base") return t.base.items!.map((s) => `<div class="p-item">${esc(s)}</div>`).join("");
  if (col === "ci") {
    return t.ci.items!.map((c) => `<div class="p-item">${c.url ? ext(c.url, "p-mono", "Open job", c.name) : `<span class="p-mono">${esc(c.name)}</span>`}</div>`).join("");
  }
  if (col === "feedback") {
    return t.feedback.items!.map((th) => {
      const open = expanded.has(th.url);
      return `<article class="p-thread">
        <div class="p-th-h"><b>${esc(th.author)}</b>${ext(th.url, "p-mono p-host", `Open comment: ${th.where}`, th.where)}<span class="p-host p-ago" title="${esc(th.at)}">${esc(th.ago)}</span></div>
        <p class="p-body${th.long && !open ? " clamp" : ""}">${esc(th.body)}</p>
        ${th.long ? `<button type="button" class="p-more" data-thread="${esc(th.url)}" aria-expanded="${open}">${open ? "show less" : "show all"}</button>` : ""}
      </article>`;
    }).join("");
  }
  return t.review.items!.map((r) => `<div class="p-item">
      <span><span class="${r.team ? "p-mono" : ""}">${esc(r.name)}</span>${r.codeOwner ? ' <span class="p-host" title="Requested as code owner">code owner</span>' : ""}${
        r.teams.length ? ` <span class="p-host p-mono" title="Reviewed for ${esc(r.teams.join(", "))}">${esc(r.teams.join(", "))}</span>` : ""
      }</span>
      <span class="p-rw${r.state === "changes requested" ? " p-rw-act" : ""}">${r.state}</span>
    </div>`).join("");
}

const CLOSE = (label: string) =>
  `<button type="button" class="p-x" data-close aria-label="${label}"><svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>`;

/** A Jira issue's line: its key and status, linked to Jira, or the bare key when not found. */
function jiraLine(j: JiraView): string {
  const text = `<span class="p-mono">${esc(j.key)}</span>${j.status ? ` · ${esc(j.status)}` : ` · ${esc(j.state)}`}`;
  return `<div class="p-line p-dim">${j.url ? `<a href="${esc(j.url)}" target="_blank" rel="noreferrer" title="${esc(j.tip)}">${text}</a>` : `<span title="${esc(j.tip)}">${text}</span>`}</div>`;
}

/** The panel for the selected PR: its number linking to GitHub and its size, then a section per signal. */
function prPanel(p: PrView, t: TrayState): string {
  const sections = COLUMNS.map(([col, label]) => {
    const s = p.tray[col];
    const l = p[col];
    const led = "count" in l && l.count != null ? String(l.count) : "";
    return `<section class="p-sec" data-sec="${col}" aria-label="${label}">
      <div class="p-sech"><span class="p-led t-${sig(l)}" title="${esc(l.tip)}">${led}</span><span class="p-key">${label}</span>${
        s.fine != null ? `<span class="p-fine${l.lamp === "act" ? " act" : ""}">${esc(s.fine)}</span>` : ""
      }</div>
      ${s.items ? `<div class="p-items">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
  }).join("");
  return `<div class="p-head">${ext(p.url, "p-mono p-num", `Open #${p.number} on GitHub`, `#${p.number}`)}${p.draft && p.node === "open" ? '<span class="p-draft">draft</span>' : ""}<span class="p-meta p-mono">${esc(p.sizeTip)}</span>${CLOSE("Close details")}</div>
    <h2 class="p-title">${esc(p.title)}</h2>
    ${p.jira ? jiraLine(p.jira) : ""}
    ${p.queue ? `<div class="p-line p-dim">${esc(p.queue.text)}</div>` : ""}
    ${sections}`;
}

/** The panel for a selected Branch with no PR: its name and Jira issue. */
const branchPanel = (b: BranchView, feature: boolean) => `<div class="p-head"><span class="lb lb-l" aria-hidden="true">${feature ? "F" : ""}</span><span>${feature ? "feature branch" : "local branch"}</span><span class="p-meta p-mono">${esc(b.repo)}</span>${CLOSE("Close details")}</div>
    <h2 class="p-title p-mono-title">${esc(b.name)}</h2>
    ${b.jira ? jiraLine(b.jira) : ""}
    <div class="p-sub">no pull request yet</div>`;

const EMPTY = `<div class="p-empty"><div class="p-empty-t">Nothing selected</div><div class="p-empty-s">Select a PR or a session, or press ↑ ↓</div></div>`;

/** The status word and how long the Session has had it: "waiting 31 min". */
const statusFor = (v: SessionView) =>
  `<span class="st">${esc(v.status.state ?? "unknown")}</span>${v.status.since ? ` ${esc(age(v.status.since))}` : ""}`;
const statusTip = (v: SessionView) => (v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip);

/** The panel for the open Session: status, title, Activity, then its Task's Resources and other Sessions. */
function sessionPanel({ session: v, counts, task }: PanelView): string {
  const activity = v.activity?.replace(/\s+/g, " ");
  const resources = task
    ? task.resources.map((r) => {
        const cls = r.mono ? "p-mono" : "";
        const label = r.link
          ? `<a class="${cls}" href="${esc(r.url)}" target="_blank" rel="noreferrer" title="${esc(r.url)}">${esc(r.label)}</a>`
          : `<span class="${cls}" title="${esc(r.url)}">${esc(r.label)}</span>`;
        return `<div class="p-item">${label}${r.host ? `<span class="p-host">${esc(r.host)}</span>` : ""}</div>`;
      }).join("")
    : "";
  const others = task?.others.length
    ? `<div class="p-sub">also on this task</div>${task.others.map((o) => `<button type="button" class="p-item p-btn w-${o.status.state ?? "none"}" data-goto="${esc(o.id)}" title="Show this session"><span>${esc(o.title)}</span><span class="p-host">${statusFor(o)}</span></button>`).join("")}`
    : "";
  return `<div class="p-head w-${v.status.state ?? "none"}"><span title="${esc(statusTip(v))}">${statusFor(v)}</span><span class="p-meta">${esc(counts)}</span>${CLOSE("Close session")}</div>
    <h2 class="p-title">${esc(v.title)}</h2>
    ${activity ? `<div class="p-line p-activity" title="${esc(v.activity!)}">${esc(activity)}</div>` : ""}
    <div class="p-line p-dim">${esc(v.cde ?? "unknown CDE")}${v.repo ? ` · <span class="p-mono">${esc(v.repo)}</span>` : ""}</div>
    <section class="p-sec" aria-label="task">
      <div class="p-sech"><span class="p-key">task</span>${task ? "" : '<span class="p-fine">not part of a task</span>'}</div>
      ${task ? `<div class="p-list">${resources}${others}</div>` : ""}
    </section>`;
}

/** The rail: an L along the lane's top and down beside its title, coloured by status. It folds the lane. */
const rail = (v: SessionView, folded: boolean) =>
  `<button type="button" class="trail" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"></button>`;

function lane(v: SessionView, toggled: Set<string>, sel: string | null, open: boolean): string {
  const folded = isFolded(v, toggled);
  const { rows, edges } = lanes(v);
  const activity = v.activity?.replace(/\s+/g, " ");
  const head = `<div class="lh">
      <div class="lht"><button type="button" class="tt${v.untitled ? " untitled" : ""}" data-session="${esc(v.id)}" aria-pressed="${open}" aria-label="${esc(`${open ? "Close" : "Show"} session details: ${v.title}`)}" title="${esc(v.title)}">${esc(v.title)}</button>${rail(v, folded)}</div>
      ${!folded && activity ? `<div class="lm" title="${esc(v.activity!)}">${esc(activity)}</div>` : ""}
      <div class="lm2"><span title="${esc(statusTip(v))}">${statusFor(v)}</span><span>·</span><span>${esc(v.cde ?? "unknown CDE")}</span>${
        v.repo ? `<span>·</span><span class="mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""
      }</div>
    </div>`;
  const cls = `g lane w-${v.status.state ?? "none"}${folded ? " folded" : ""}${open ? " sel" : ""}`;
  let body: string;
  if (folded) {
    body = STAGES.map(([st], c) => {
      const n = rows.filter((r) => r.stage === st).length;
      return `<div class="fc" style="grid-column:${c + 2}">${n ? `<span class="cnt mono">${n}</span>` : ""}</div>`;
    }).join("");
  } else if (rows.length) {
    body = `<div class="area" style="grid-template-rows:repeat(${rows.length},${NODE}px);row-gap:${PITCH - NODE}px;padding:${PAD}px 0">${edges.map((e) => edge(e, rows)).join("")}${rows.map((r, i) => node(r, i, rows, sel)).join("")}</div>`;
  } else {
    body = '<div class="area none">no pull requests yet</div>';
  }
  return `<section class="${cls}" data-chan="${esc(v.id)}" aria-label="${esc(v.title)}">${head}${body}</section>`;
}

/** The wordmark from the brand kit, on light, 24px tall. */
const WORDMARK = `<svg class="brand" width="45.45" height="24" viewBox="0 0 60.6 32" role="img" aria-label="flow"><defs><clipPath id="wx0"><rect x="0" y="12.2" width="60.6" height="15.0"/></clipPath></defs><g fill="none" stroke="#111110" stroke-width="3.2" stroke-linecap="butt" stroke-linejoin="miter" stroke-miterlimit="10"><path d="M8.6 27.2 V11.4 C8.6 8.1 10.5 6.3 13.7 6.3 H16.2"/><path d="M20.4 4.7 V27.2"/><ellipse cx="31.1" cy="19.75" rx="5.45" ry="6.05"/><path clip-path="url(#wx0)" d="M39.4 8 L44.3 29 L49 10 L53.7 29 L58.6 8"/></g><rect x="3.8" y="12.2" width="10.4" height="3.2" fill="#BF1B1B"/></svg>`;

const LEGEND = `<div class="lg" role="note" aria-label="Legend">
    <div class="lg-col"><span class="lg-h">Sessions</span>
      <span><i class="lr lr-w"></i>waiting for you</span><span><i class="lr lr-r"></i>running</span><span><i class="lr"></i>idle</span><span><i class="lr lr-d"></i>done</span><span><i class="lr lr-f"></i>folded: click the rail</span>
    </div>
    <div class="lg-col"><span class="lg-h">Signals</span><span class="lg-sub">base · CI · feedback · review</span>
      <span><i class="ld t-act"></i>needs you</span><span><i class="ld t-run"></i>running</span><span><i class="ld t-count"></i>review requested</span><span><i class="ld t-ok"></i>fine</span>
    </div>
    <div class="lg-col"><span class="lg-h">Pull requests</span>
      <span><i class="lb"></i>open</span><span><i class="lb">2</i>position in a stack</span><span><i class="lb lb-f">F</i>feature branch</span><span><i class="lb lb-d"></i>draft</span><span><i class="lb lb-m"></i>mergeable</span><span><i class="lb lb-q"></i>in merge queue</span><span><i class="lb lb-x"></i>merged or closed</span><span><i class="lb lb-l"></i>local branch, no PR</span>
    </div>
    <div class="lg-col"><span class="lg-h">Lines</span>
      <span><i class="ll"></i>stacked on the PR above</span><span><i class="ll ll-f"></i>based on a feature branch</span>
      <span class="lg-h lg-gap">Columns</span><span class="lg-txt">Open includes local branches. Ready = mergeable. Merge = queued, merged or closed.</span>
      <span class="lg-h lg-gap">Keys</span><span><kbd>↑</kbd><kbd>↓</kbd>step through the board</span><span><kbd>Esc</kbd>clear selection</span>
    </div>
  </div>`;

/** `toggled` holds the ids of Sessions the user has folded or unfolded against their default;
 * `notices` are warnings shown in the bar, `alarm` true when the first is that flowd is
 * unreachable; `legend` is whether the legend is open; `t` is the selected node's panel, drawn
 * only while its node is in `views`; `panel` is the open Session's panel, shown instead. */
export function render(
  views: SessionView[],
  toggled: Set<string>,
  notices: string[],
  alarm: boolean,
  legend: boolean,
  t: TrayState | null,
  panel: PanelView | null = null,
): string {
  const all = views.flatMap((v) => lanes(v).rows);
  const r = !panel && t ? all.find((x) => x.id === t.id) : undefined;
  const sel = r ? r.id : null;
  const open = panel?.session.id ?? null;
  const body = panel ? sessionPanel(panel) : r && t ? (r.pr ? prPanel(r.pr, t) : branchPanel(r.branch, r.feature)) : EMPTY;
  const heads = STAGES.map(([st, name]) => `<div><span class="hn">${name}</span><span class="hc">${all.filter((x) => x.stage === st).length}</span></div>`).join("");
  return `<div class="app">
    <header class="bar">
      ${WORDMARK}<span class="sp"></span>
      ${notices.length ? `<div class="notices" role="status">${notices.map((n, i) => `<span${i === 0 && alarm ? ' class="alarm"' : ""}>${esc(n)}</span>`).join("")}</div>` : ""}
      <span class="kbd" aria-hidden="true"><kbd>↑</kbd><kbd>↓</kbd> select</span>
      <button type="button" class="kbd" data-legend aria-expanded="${legend}" aria-keyshortcuts="h"><kbd>h</kbd> legend</button>
    </header>
    <div class="wrap">
      <main class="board">
        <div class="g heads"><div>Session</div>${heads}</div>
        ${views.length ? views.map((v) => lane(v, toggled, sel, v.id === open)).join("") : '<p class="empty">No Sessions.</p>'}
      </main>
      <aside><div class="p" role="region" aria-label="${panel ? "Session details" : "Details"}">${body}</div></aside>
    </div>
  </div>
  ${legend ? LEGEND : ""}`;
}
