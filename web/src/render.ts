// Variant A of the design canvas as HTML strings: one panel row per Session, its Stack beside it.
import { age, stackLight } from "./model.ts";
import type { Lamp, PrView, SessionView } from "./model.ts";

export function esc(s: string | number): string {
  return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

const ago = (iso: string) => {
  const a = age(iso);
  return a === "just now" ? a : `${a} ago`;
};

const lamp = (l: Lamp, tip: string, sm = false) =>
  `<span class="lamp ${l}${sm ? " sm" : ""}" title="${esc(tip)}"></span>`;

function prRow(p: PrView, j: number, n: number): string {
  const notes = p.ready ? ["ready to merge"] : p.needs;
  return `<div class="grid grow">
    ${j > 0 ? '<div class="rule"></div>' : ""}
    <div class="gnode">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<a class="node ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer" title="#${p.number} · ${p.node}">${p.pos}</a></div>
    <div class="gtitle"><a class="t ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer">${esc(p.title)}</a><span class="sub">${p.jira ? `<span class="mono">${esc(p.jira)}</span>` : ""}${notes.length ? `<span class="needs">${esc(notes.join(" · "))}</span>` : ""}</span></div>
    <div class="glines mono">+${p.additions} −${p.deletions}</div>
    <div class="gcell">${lamp(p.base.lamp, p.base.tip)}</div>
    <div class="gcell">${lamp(p.ci.lamp, p.ci.tip)}</div>
    <div class="gcell">${lamp(p.feedback.lamp, p.feedback.tip)}</div>
    <div class="gcell">${lamp(p.review.lamp, p.review.tip)}</div>
  </div>`;
}

function sessionRow(v: SessionView, i: number, collapsed: Set<string>): string {
  const idx = String(i + 1).padStart(2, "0");
  const stat = `<div class="sstat ${v.status.lamp}">${lamp(v.status.lamp, v.status.tip, true)}${esc(v.status.tip)}${v.status.since ? ` · ${age(v.status.since)}` : ""}</div>`;
  if (collapsed.has(v.id)) {
    const s = stackLight(v.stack);
    return `<section class="srow folded" data-toggle="${esc(v.id)}" title="Show this Session">
      <div class="sidx">${idx}</div>
      <div class="sline">${stat}<b${v.untitled ? ' class="untitled"' : ""}>${esc(v.title)}</b><span class="cde">${esc(v.cde ?? "unknown CDE")}</span>${lamp(s.lamp, `Stack: ${s.tip}`)}</div>
    </section>`;
  }
  const head = `<div class="sidx">${idx}<button class="fold" data-toggle="${esc(v.id)}" title="Collapse this Session">−</button></div>
    <div class="sinfo">
      ${stat}
      <h2${v.untitled ? ' class="untitled"' : ""}>${esc(v.title)}</h2>
      ${v.activity ? `<p class="activity" title="${esc(v.activity)}">${esc(v.activity)}</p>` : ""}
      <div class="smeta">
        ${v.repo ? `<span class="mono">${esc(v.repo)}</span>` : ""}
        <span>${esc(v.cde ?? "unknown CDE")}${v.started ? ` · started ${ago(v.started)}` : ""}</span>
      </div>
    </div>`;
  const body = v.stack.length
    ? `<div class="grid ghead"><span></span><span></span><span></span><span>base</span><span>CI</span><span>feedback</span><span>review</span></div>` +
      v.stack.map((p, j) => prRow(p, j, v.stack.length)).join("")
    : `<div class="noprs"><span class="lamp off"></span><span>No pull requests yet</span></div>`;
  return `<section class="srow">${head}<div class="sprs">${body}</div></section>`;
}

/** `collapsed` holds the ids of Sessions shown as a header line only; `notices` are warnings
 * shown above the panel. */
export function render(views: SessionView[], collapsed: Set<string>, notices: string[]): string {
  return `<div class="page">
    <header class="top"><div class="logo"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    ${notices.map((n) => `<p class="notice">${esc(n)}</p>`).join("")}
    <main class="panel">${views.length ? views.map((v, i) => sessionRow(v, i, collapsed)).join("") : '<p class="empty">No Sessions.</p>'}</main>
    <footer class="key">
      <span><span class="lamp act"></span>needs action</span>
      <span><span class="lamp run"></span>in progress</span>
      <span><span class="lamp fine"></span>fine</span>
      <span><span class="lamp off"></span>does not apply</span>
      <span>PR <span class="node key"></span>open <span class="node key draft"></span>draft <span class="node key ready"></span>ready to merge <span class="node key merged"></span>merged <span class="node key closed"></span>closed</span>
    </footer>
  </div>`;
}
