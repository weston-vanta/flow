// The "flow — sessions" design canvas as HTML strings: one panel row per Session, its Stack
// beside it, the legend behind `h` and notices in a strip fixed to the bottom.
import { age, attentionLight, isFolded } from "./model.ts";
import type { Light, PrView, ReviewLight, SessionView } from "./model.ts";

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

const lamp = (l: Light, sm = false) =>
  `<span class="lamp ${l.lamp}${sm ? " sm" : ""}" aria-hidden="true"></span>`;

/** A table cell holding a lamp, or the review count while reviews are requested. */
const cell = (l: Light | ReviewLight) => {
  const count = "count" in l && l.count != null;
  return `<div class="gcell" role="cell" title="${esc(l.tip)}" aria-label="${esc(l.tip)}">${
    count ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l)
  }</div>`;
};

const NODE_TIP = { merged: "merged", closed: "closed", ready: "mergeable" };

function prRow(p: PrView, j: number, n: number): string {
  const tip = `#${p.number} · ${p.node === "open" ? (p.draft ? "draft" : "not yet mergeable") : NODE_TIP[p.node]}`;
  return `<div class="grid grow" role="row">
    ${j > 0 ? '<div class="rule" aria-hidden="true"></div>' : ""}
    <div class="gnode" role="cell">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<a class="node ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer" title="${esc(tip)}" aria-label="Pull request ${esc(tip)}">${n > 1 ? p.pos : ""}</a></div>
    <div class="gtitle" role="cell"><a class="t ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer">${esc(p.title)}</a>${p.jira ? `<span class="jira mono">${esc(p.jira)}</span>` : ""}</div>
    <div class="glines mono" role="cell">+${p.additions} −${p.deletions}</div>
    ${cell(p.base)}${cell(p.ci)}${cell(p.feedback)}${cell(p.review)}
  </div>`;
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

function sessionRow(v: SessionView, toggled: Set<string>): string {
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
  const body = v.stack.length
    ? `<div class="scroll"><div role="table" aria-label="Pull requests">
        <div class="grid ghead" role="row"><span></span><span></span><span></span><span role="columnheader">base</span><span role="columnheader">CI</span><span role="columnheader">feedback</span><span role="columnheader">review</span></div>
        ${v.stack.map((p, j) => prRow(p, j, v.stack.length)).join("")}
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
 * `notices` are warnings shown in the bottom strip; `legend` is whether the legend is open. */
export function render(views: SessionView[], toggled: Set<string>, notices: string[], legend: boolean): string {
  return `<div class="page${notices.length ? " noticed" : ""}">
    <header class="top"><div class="logo" aria-hidden="true"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    <main class="panel">${views.length ? views.map((v) => sessionRow(v, toggled)).join("") : '<p class="empty">No Sessions.</p>'}</main>
    <footer><button type="button" class="hint" data-legend aria-expanded="${legend}" aria-keyshortcuts="h"><kbd class="${legend ? "down" : ""}">h</kbd><span>legend</span></button></footer>
  </div>
  <div class="dock">
    ${legend ? LEGEND : ""}
    ${notices.length ? `<div class="notices" role="status">${notices.map((n) => `<p>${esc(n)}</p>`).join("")}</div>` : ""}
  </div>`;
}
