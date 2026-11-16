// The "flow" receiver face, flat, as HTML strings: a header strip, the Session channels with their
// Stacks as plates of PR strips, the window showing the selected PR, and the legend behind `h`.
import { age, attentionLight, isFolded } from "./model.ts";
import type { Light, PrView, ReviewLight, SessionView } from "./model.ts";

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

const lamp = (l: Light) => `<span class="lamp ${l.lamp}" aria-hidden="true"></span>`;

/** A lamp, or the review count while reviews are requested. */
const light = (l: Light | ReviewLight) =>
  "count" in l && l.count != null ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l);

/** The four columns: the key a lamp selects its PR at, and its heading. */
const COLUMNS = [
  ["base", "base"],
  ["ci", "CI"],
  ["feedback", "feedback"],
  ["review", "review"],
] as const;
export type Column = (typeof COLUMNS)[number][0];

/** The selected PR's tray: whose it is, the section a lamp click selected it at (tinted
 * briefly), and the threads expanded past their clamp, by URL. */
export interface TrayState {
  url: string;
  focus: Column | null;
  expanded: Set<string>;
}

const NODE_WORD = { merged: "merged", closed: "closed", queued: "in merge queue", ready: "mergeable" };

/** The merge-queue gauge: a fixed-length scale, front of the queue at the left, the stretch
 * ahead of the PR drawn darker and an amber mark at its place. */
const gauge = (q: NonNullable<PrView["queue"]>) =>
  `<span class="gauge" style="--at:${q.at}" aria-hidden="true"><span class="ahead"></span><span class="mark"></span></span>`;
/** The node's classes: its state, and a draft PR drawn dashed like a Branch with no PR. */
const nodeClass = (p: PrView) => (p.node === "open" && p.draft ? "open draft" : p.node);
const nodeWord = (p: PrView) => (p.node === "open" ? (p.draft ? "draft" : "not yet mergeable") : NODE_WORD[p.node]);

/** The node column: the line joining a strip to its neighbours in the Stack, and its node. */
const nodeCell = (j: number, n: number, node: string) =>
  `<span class="ncell">${j > 0 ? '<span class="up"></span>' : ""}${j < n - 1 ? '<span class="down"></span>' : ""}${node}</span>`;

/** A lamp button: selects the PR at its column's section. */
const led = (p: PrView, col: Column, label: string) => {
  const l = p[col];
  return `<button type="button" class="led" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}">${light(l)}</button>`;
};

function strip(p: PrView, j: number, n: number, selected: boolean): string {
  const word = nodeWord(p);
  return `<div class="strip pr${selected ? " sel" : ""}">
    <span class="needle" aria-hidden="true"></span>
    <button type="button" class="pick" data-open="${esc(p.url)}" title="${esc(`#${p.number} ${p.title} · ${word}`)}" aria-label="${esc(`Tune in #${p.number}, ${p.title}, ${word}`)}" aria-pressed="${selected}">
      ${nodeCell(j, n, `<span class="node ${nodeClass(p)}">${p.pos ?? ""}</span>`)}
      <span class="tline"><span class="title ${p.node}">${esc(p.title)}</span>${
        p.queue ? `<span class="queue" title="${esc(p.queue.text)}">${gauge(p.queue)}<span class="mono">${p.queue.label}</span></span>` : ""
      }</span>
    </button>
    ${COLUMNS.map(([col, label]) => led(p, col, label)).join("")}
  </div>`;
}

/** A Branch with no PR: its own plate, a dashed node, its name in mono and blank lamps. */
const branchPlate = (name: string) => `<div class="plate branch" role="group" aria-label="Branch ${esc(name)}, no pull request yet">
    <div class="strip" title="${esc(name)} · no pull request yet">
      <span class="pick">${nodeCell(0, 1, '<span class="node branch"></span>')}<span class="title mono">${esc(name)}</span></span>
      ${'<span class="led"></span>'.repeat(COLUMNS.length)}
    </div>
  </div>`;

const plate = (stack: PrView[], url: string | null) =>
  `<div class="plate" role="group" aria-label="${stack.length === 1 ? "Pull request" : `Stack of ${stack.length} pull requests`}">${stack.map((p, j) => strip(p, j, stack.length, p.url === url)).join("")}</div>`;

const ext = (url: string, cls: string, title: string, text: string) =>
  `<a class="${cls}" href="${esc(url)}" target="_blank" rel="noreferrer" title="${esc(title)}">${esc(text)}</a>`;

/** One tray section's items, by column. */
function items(p: PrView, col: Column, expanded: Set<string>): string {
  const t = p.tray;
  if (col === "base") return t.base.items!.map((s) => `<span>${esc(s)}</span>`).join("");
  if (col === "ci") {
    return t.ci.items!.map((c) => (c.url ? ext(c.url, "item mono", "Open job", c.name) : `<span class="item mono">${esc(c.name)}</span>`)).join("");
  }
  if (col === "feedback") {
    return t.feedback.items!.map((th) => {
      const open = expanded.has(th.url);
      return `<article>
        <div class="tby"><b>${esc(th.author)}</b>${ext(th.url, "item mono", `Open comment: ${th.where}`, th.where)}<span title="${esc(th.at)}">${esc(th.ago)}</span></div>
        <p${th.long && !open ? ' class="clamp"' : ""}>${esc(th.body)}</p>
        ${th.long ? `<button type="button" class="plain" data-thread="${esc(th.url)}" aria-expanded="${open}">${open ? "show less" : "show all"}</button>` : ""}
      </article>`;
    }).join("");
  }
  return t.review.items!.map((r) => `<div class="rv">
      <span class="${r.team ? "who mono" : "who"}">${esc(r.name)}</span>
      ${r.codeOwner ? '<span class="co" title="Requested as code owner">code owner</span>' : ""}
      ${r.teams.length ? `<span class="for mono" title="Reviewed for ${esc(r.teams.join(", "))}">${esc(r.teams.join(", "))}</span>` : ""}
      <span class="state${r.state === "changes requested" ? " act" : ""}">${r.state}</span>
    </div>`).join("");
}

const CLOSE = `<button type="button" class="tclose" data-close aria-label="Close details"><svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>`;

/** The window for the selected PR: a header linking to GitHub, then a section per lamp. */
function tray(p: PrView, t: TrayState): string {
  const word = nodeWord(p);
  const sections = COLUMNS.map(([col, label]) => {
    const s = p.tray[col];
    const l = p[col];
    return `<section class="tsec${t.focus === col ? " focus" : ""}" data-sec="${col}" aria-label="${label}">
      <span class="tlamp" title="${esc(l.tip)}">${light(l)}</span>
      <div class="thd"><h3>${label}</h3>${s.fine != null ? `<span class="${l.lamp === "act" ? "act" : ""}">${esc(s.fine)}</span>` : ""}</div>
      ${s.items ? `<div class="titems ${col}">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
  }).join("");
  return `<header class="whead">
      <div class="wline">
        <span class="node ${nodeClass(p)}" title="${esc(word)}" aria-hidden="true"></span>
        ${ext(p.url, "num mono", `Open #${p.number} on GitHub`, `#${p.number}`)}
        <span>${word}</span>
        <span class="wsize mono">${esc(p.sizeTip)}</span>
        ${CLOSE}
      </div>
      <h2>${esc(p.title)}</h2>
      ${p.queue ? `<div class="wqueue">${gauge(p.queue)}<span>${esc(p.queue.text)}</span></div>` : ""}
    </header>${sections}`;
}

/** The window with no PR selected: what it will show and how to fill it. */
const EMPTY = `<header class="whead">
    <div class="wline"><span class="node none" aria-hidden="true"></span><span>no pull request tuned in</span></div>
    <span class="whint">Select a strip, or press ↑ ↓</span>
  </header>${COLUMNS.map(([, label]) => `<div class="tsec idle"><span class="tlamp"><span class="lamp unlit" aria-hidden="true"></span></span><span class="thd">${label}</span></div>`).join("")}`;

/** The status label and how long the Session has had it: "waiting for you, 31 min". */
const statusTip = (v: SessionView) => (v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip);

/** The square push key: it carries the status lamp; an outline while folded, filled while open. */
const key = (v: SessionView, folded: boolean) =>
  `<button type="button" class="skey${folded ? " up" : ""}" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"><span>${lamp(v.status)}</span></button>`;

const DOT = '<span class="dot" aria-hidden="true">·</span>';

/** One line that never wraps: CDE · repo · status word and its duration. The repo gives way. */
function meta(v: SessionView): string {
  const since = v.status.since ? ` ${age(v.status.since)}` : "";
  return `<span class="smeta"><span>${esc(v.cde ?? "unknown CDE")}</span>${
    v.repo ? `${DOT}<span class="repo mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""
  }${DOT}<span title="${esc(statusTip(v))}"><span class="${v.status.lamp}">${esc(v.status.state ?? "unknown")}</span>${since}</span></span>`;
}

function channel(v: SessionView, toggled: Set<string>, url: string | null): string {
  // Untitled and done Sessions show their title muted.
  const h2 = `<h2${v.untitled || v.status.state === "done" ? ' class="muted"' : ""}>${esc(v.title)}</h2>`;
  if (isFolded(v, toggled)) {
    const a = attentionLight(v);
    return `<section class="chan folded" data-toggle="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="chead">${key(v, true)}<div class="ctext"><div class="cline">${h2}<span class="attn" title="${esc(a.tip)}" aria-label="${esc(a.tip)}">${lamp(a)}</span></div></div></div>
    </section>`;
  }
  const activity = v.activity?.replace(/\s+/g, " ");
  const body = v.stacks.length || v.branches.length
    ? `<div class="plates">${v.stacks.map((s) => plate(s, url)).join("")}${v.branches.map(branchPlate).join("")}</div>`
    : '<div class="noprs">no pull requests yet</div>';
  return `<section class="chan" aria-label="${esc(v.title)}">
      <div class="chead">${key(v, false)}<div class="ctext">
        <div class="cline">${h2}</div>
        ${activity ? `<span class="activity" title="${esc(v.activity!)}">${esc(activity)}</span>` : ""}
        ${meta(v)}
      </div></div>
      ${body}
    </section>`;
}

/** The wordmark from the brand kit, on light, 24px tall. */
const WORDMARK = `<svg class="brand" width="45.45" height="24" viewBox="0 0 60.6 32" role="img" aria-label="flow"><defs><clipPath id="wx0"><rect x="0" y="12.2" width="60.6" height="15.0"/></clipPath></defs><g fill="none" stroke="#2A2927" stroke-width="3.2" stroke-linecap="butt" stroke-linejoin="miter" stroke-miterlimit="10"><path d="M8.6 27.2 V11.4 C8.6 8.1 10.5 6.3 13.7 6.3 H16.2"/><path d="M20.4 4.7 V27.2"/><ellipse cx="31.1" cy="19.75" rx="5.45" ry="6.05"/><path clip-path="url(#wx0)" d="M39.4 8 L44.3 29 L49 10 L53.7 29 L58.6 8"/></g><rect x="3.8" y="12.2" width="10.4" height="3.2" fill="#BF1B1B"/></svg>`;

const LEGEND = `<div class="legend" role="note" aria-label="Legend">
    <div><span class="lh">lights</span>
      <span class="li act"><span class="lamp act"></span>needs action</span>
      <span class="li"><span class="lamp run still"></span>in progress</span>
      <span class="li"><span class="lamp fine"></span>fine</span>
      <span class="li"><span class="count">2</span>pending reviews</span>
    </div>
    <div><span class="lh">pull requests</span>
      <span class="li"><span class="node key open"></span>open</span>
      <span class="li"><span class="node key ready"></span>mergeable</span>
      <span class="li"><span class="node key queued"></span>in merge queue</span>
      <span class="li"><span class="node key merged"></span>merged</span>
      <span class="li"><span class="node key branch"></span>draft, or branch with no PR</span>
    </div>
  </div>`;

/** `toggled` holds the ids of Sessions the user has folded or unfolded against their default;
 * `notices` are warnings shown in the header; `live` lights the power lamp; `legend` is whether
 * the legend is open; `t` is the selected PR's tray, drawn only while its PR is in `views`. */
export function render(
  views: SessionView[],
  toggled: Set<string>,
  notices: string[],
  live: boolean,
  legend: boolean,
  t: TrayState | null,
): string {
  const p = t && views.flatMap((v) => v.prs).find((x) => x.url === t.url);
  return `<div class="face">
    <header class="top">
      ${WORDMARK}<span class="tagline">agent sessions and pull requests</span>
      <div class="controls">
        ${notices.length ? `<div class="notices" role="status">${notices.map((n) => `<span>${esc(n)}</span>`).join("")}</div>` : ""}
        <span class="hint" aria-hidden="true"><kbd>↑</kbd><kbd>↓</kbd><span>tune</span></span>
        <button type="button" class="hint" data-legend aria-expanded="${legend}" aria-keyshortcuts="h"><kbd class="${legend ? "down" : ""}">h</kbd><span>legend</span></button>
        <span class="power${live ? " on" : ""}" title="${live ? "flowd reachable" : "flowd unreachable"}" aria-hidden="true"></span>
      </div>
    </header>
    <div class="deck">
      <main class="rack">
        <div class="scale" aria-hidden="true"><span></span><span></span>${COLUMNS.map(([, label]) => `<span>${label}</span>`).join("")}</div>
        <div class="chans">${views.length ? views.map((v) => channel(v, toggled, p ? p.url : null)).join("") : '<p class="empty">No Sessions.</p>'}</div>
      </main>
      <aside class="window"><div class="wbody"><div class="tscroll" role="region" aria-label="Pull request details">${p && t ? tray(p, t) : EMPTY}</div></div></aside>
    </div>
  </div>
  ${legend ? LEGEND : ""}`;
}
