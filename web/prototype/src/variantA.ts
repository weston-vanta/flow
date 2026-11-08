// PROTOTYPE variant A: the design canvas as drawn, adapted to decided data
// (no Open session button, no summaries: title from context, no Jira state).
import { SessionView, ago, esc, lamp, node, reviewCell, statusLabel, statusLamp } from "./model";

export const name = "Panel (the design)";

export function render(views: SessionView[]): string {
  const rows = views.map((v, i) => {
    const s = v.s;
    const head = `
      <div class="sidx">${String(i + 1).padStart(2, "0")}</div>
      <div class="sinfo">
        <div class="sstat ${s.status.value === "waiting" ? "act" : ""}">${lamp(statusLamp[s.status.value], "", true)}${statusLabel[s.status.value]} · ${ago(s.status.at)}</div>
        <h2>${esc(s.context.title)}</h2>
        <div class="smeta">
          <span class="mono">${esc(v.repo)}</span>
          <span>${esc(s.origin.cde)} · started ${ago(s.origin.at)}</span>
        </div>
      </div>`;
    const body = v.prs.length === 0
      ? `<div class="noprs"><span class="lamp"></span><span>No pull requests yet</span></div>`
      : `<div class="grid ghead"><span></span><span></span><span></span><span>base</span><span>CI</span><span>feedback</span><span>review</span></div>` +
        v.prs.map((p, j) => `
          <div class="grid grow">
            ${j > 0 ? '<div class="rule"></div>' : ""}
            <div class="gnode">${j > 0 ? '<div class="up"></div>' : ""}${j < v.prs.length - 1 ? '<div class="down"></div>' : ""}${node(p)}</div>
            <div class="gtitle"><span class="t ${p.node === "merged" ? "merged" : ""}">${esc(p.title)}</span>${p.jira ? `<span class="j mono">${esc(p.jira)}</span>` : ""}</div>
            <div class="glines mono">+${p.pr.github.additions} −${p.pr.github.deletions}</div>
            <div class="gcell">${lamp(p.base.lamp, p.base.tip)}</div>
            <div class="gcell">${lamp(p.ci.lamp, p.ci.tip)}</div>
            <div class="gcell">${lamp(p.feedback.lamp, p.feedback.tip)}</div>
            <div class="gcell">${reviewCell(p)}</div>
          </div>`).join("");
    return `<section class="srow">${head}<div class="sprs">${body}</div></section>`;
  });
  return `<main class="panel">${rows.join("")}</main>`;
}
