// The "flow — sessions" design canvas as HTML strings: one panel row per Session, its Stacks
// beside it, the legend behind `h` and notices in a strip fixed to the bottom.
import { age, attentionLight, isFolded } from "./model.ts";
import type { Light, PrView, ReviewLight, SessionView } from "./model.ts";

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

const lamp = (l: Light, sm = false) =>
  `<span class="lamp ${l.lamp}${sm ? " sm" : ""}" aria-hidden="true"></span>`;

/** A lamp, or the review count while reviews are requested. */
const light = (l: Light | ReviewLight) =>
  "count" in l && l.count != null ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l);

/** The four columns: the key a lamp cell opens the tray at, and its heading. */
const COLUMNS = [
  ["base", "base"],
  ["ci", "CI"],
  ["feedback", "feedback"],
  ["review", "review"],
] as const;
export type Column = (typeof COLUMNS)[number][0];

/** The open (or last opened) tray: whose it is, whether it is showing, the section a lamp
 * click opened it at (tinted briefly), and the threads expanded past their clamp, by URL. */
export interface TrayState {
  url: string;
  open: boolean;
  focus: Column | null;
  expanded: Set<string>;
}

/** A lamp cell: a button that opens the tray at its column's section. */
const cell = (p: PrView, col: Column, label: string) => {
  const l = p[col];
  return `<div class="gcell" role="cell"><button type="button" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}">${light(l)}</button></div>`;
};

const NODE_TIP = { merged: "merged", closed: "closed", ready: "mergeable" };
const nodeWord = (p: PrView) => (p.node === "open" ? (p.draft ? "draft" : "not yet mergeable") : NODE_TIP[p.node]);

function prRow(p: PrView, j: number, n: number, selected: boolean): string {
  const tip = `#${p.number} · ${nodeWord(p)}`;
  return `<div class="grid grow${selected ? " sel" : ""}" role="row">
    ${j > 0 ? '<div class="rule" aria-hidden="true"></div>' : ""}
    <div class="gnode" role="cell">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<button type="button" class="node ${p.node}" data-open="${esc(p.url)}" title="${esc(tip)}" aria-label="Pull request ${esc(tip)}: show details">${p.pos ?? ""}</button></div>
    <div class="gtitle" role="cell"><button type="button" class="t ${p.node}" data-open="${esc(p.url)}">${esc(p.title)}</button>${p.jira ? `<span class="jira mono">${esc(p.jira)}</span>` : ""}</div>
    <div class="glines mono" role="cell" title="${esc(p.sizeTip)}" aria-label="${esc(p.sizeTip)}">${esc(p.size)}</div>
    ${COLUMNS.map(([col, label]) => cell(p, col, label)).join("")}
  </div>`;
}

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

/** The tray for one PR: a header linking to GitHub, then a section per lamp. */
function tray(p: PrView, t: TrayState): string {
  const word = nodeWord(p);
  // The node already says open, mergeable or merged; only draft and closed need a word.
  const state = p.draft || p.node === "closed" ? `<span>${p.node === "closed" ? "closed" : "draft"}</span>` : "";
  const sections = COLUMNS.map(([col, label]) => {
    const s = p.tray[col];
    const l = p[col];
    return `<section class="tsec${t.focus === col ? " focus" : ""}" data-sec="${col}" aria-label="${label}">
      <div class="tlamp" title="${esc(l.tip)}">${light(l)}</div>
      <div class="thd"><h4>${label}</h4>${s.fine != null ? `<span class="${l.lamp === "act" ? "act" : ""}">${esc(s.fine)}</span>` : ""}</div>
      ${s.items ? `<div class="titems ${col}">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
  }).join("");
  return `<header class="thead">
      <span class="node ${p.node}" title="${esc(word)}" aria-hidden="true"></span>
      <div class="tmeta">${ext(p.url, "plain mono", `Open #${p.number} on GitHub`, `#${p.number}`)}${state}${p.jira ? `<span class="mono">${esc(p.jira)}</span>` : ""}</div>
      <button type="button" class="tclose" data-close aria-label="Close details"><svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>
      <h3>${esc(p.title)}</h3>
    </header>${sections}`;
}

/** The status label and how long the Session has had it: "waiting for you, 31 min". */
const statusTip = (v: SessionView) => (v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip);

/** The Session key: its dish carries the status lamp. */
const key = (v: SessionView, folded: boolean) =>
  `<button type="button" class="skey${folded ? " up" : ""}" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"><span>${lamp(v.status, true)}</span></button>`;

/** Untitled and done Sessions show their title muted. */
const title = (v: SessionView, tag: string) =>
  `<${tag}${v.untitled || v.status.state === "done" ? ' class="muted"' : ""}>${esc(v.title)}</${tag}>`;

const DOT = '<span class="dot" aria-hidden="true">·</span>';

/** One line that never wraps: CDE · repo · status word and its duration. The repo gives way. */
function meta(v: SessionView): string {
  const since = v.status.since ? ` ${age(v.status.since)}` : "";
  return `<span class="smeta"><span>${esc(v.cde ?? "unknown CDE")}</span>${
    v.repo ? `${DOT}<span class="repo mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""
  }${DOT}<span title="${esc(statusTip(v))}"><span class="${v.status.lamp}">${esc(v.status.state ?? "unknown")}</span>${since}</span></span>`;
}

function sessionRow(v: SessionView, toggled: Set<string>, trayUrl: string | null): string {
  if (isFolded(v, toggled)) {
    const a = attentionLight(v);
    return `<section class="srow folded" data-toggle="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="sidx">${key(v, true)}</div>
      <div class="sline">${title(v, "h2")}<span class="attn" title="${esc(a.tip)}" aria-label="${esc(a.tip)}">${lamp(a)}</span></div>
    </section>`;
  }
  const activity = v.activity?.replace(/\s+/g, " ");
  const info = `<div class="sess">${title(v, "h2")}<div class="sctx">
      ${activity ? `<span class="activity" title="${esc(v.activity!)}">${esc(activity)}</span>` : ""}
      ${meta(v)}
    </div></div>`;
  const body = v.stacks.length
    ? `<div class="scroll"><div role="table" aria-label="Pull requests">
        <div class="grid ghead" role="row"><span></span><span></span><span></span><span role="columnheader">base</span><span role="columnheader">CI</span><span role="columnheader">feedback</span><span role="columnheader">review</span></div>
        ${v.stacks.map((stack) => `<div class="sgroup" role="rowgroup">${stack.map((p, j) => prRow(p, j, stack.length, p.url === trayUrl)).join("")}</div>`).join("")}
      </div></div>`
    : `<div class="noprs"><span class="ring" aria-hidden="true"></span><span>No pull requests yet</span></div>`;
  return `<section class="srow" aria-label="${esc(v.title)}"><div class="sidx">${key(v, false)}</div>
    ${info}<div class="sprs">${body}</div>
  </section>`;
}

const LEGEND = `<div class="legend" role="note" aria-label="Legend">
    <div><span class="lh">lights</span>
      <span class="li act"><span class="lamp act"></span>needs action</span>
      <span class="li"><span class="lamp run"></span>in progress</span>
      <span class="li"><span class="lamp fine"></span>fine</span>
      <span class="li"><span class="count">2</span>reviews needed</span>
    </div>
    <div><span class="lh">pull requests</span>
      <span class="li"><span class="node key"></span>open or draft</span>
      <span class="li"><span class="node key ready"></span>mergeable</span>
      <span class="li"><span class="node key merged"></span>merged</span>
      <span class="li"><span class="node key closed"></span>closed</span>
    </div>
  </div>`;

/** `toggled` holds the ids of Sessions the user has folded or unfolded against their default;
 * `notices` are warnings shown in the bottom strip; `legend` is whether the legend is open;
 * `t` is the tray, drawn only while its PR is in `views`. */
export function render(
  views: SessionView[],
  toggled: Set<string>,
  notices: string[],
  legend: boolean,
  t: TrayState | null,
): string {
  const p = t && views.flatMap((v) => v.stacks.flat()).find((x) => x.url === t.url);
  return `<div class="page${notices.length ? " noticed" : ""}">
    <header class="top"><div class="logo" aria-hidden="true"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    <div class="deck${p && t?.open ? " open" : ""}">
      <main class="panel">${views.length ? views.map((v) => sessionRow(v, toggled, p ? p.url : null)).join("") : '<p class="empty">No Sessions.</p>'}</main>
      <div class="scrim" data-close aria-hidden="true"></div>
      <aside class="tray" role="dialog" aria-modal="false" aria-label="Pull request details"><div class="tscroll">${p && t ? tray(p, t) : ""}</div></aside>
    </div>
    <footer><button type="button" class="hint" data-legend aria-expanded="${legend}" aria-keyshortcuts="h"><kbd class="${legend ? "down" : ""}">h</kbd><span>legend</span></button></footer>
  </div>
  <div class="dock">
    ${legend ? LEGEND : ""}
    ${notices.length ? `<div class="notices" role="status">${notices.map((n) => `<p>${esc(n)}</p>`).join("")}</div>` : ""}
  </div>`;
}
