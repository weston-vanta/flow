// PROTOTYPE variant C: attention first. What needs the user, then what is moving, then what is quiet.
import { SessionView, ago, esc, lamp, node, statusLabel, statusLamp } from "./model";

export const name = "Attention (needs you first)";

export function render(views: SessionView[]): string {
  const needs: string[] = [];
  for (const v of views) {
    if (v.s.status.value === "waiting")
      needs.push(item("act", "waiting for you", esc(v.s.context.title), `${esc(v.s.origin.cde)} · ${ago(v.s.status.at)}`));
    for (const p of v.prs)
      for (const why of p.needs)
        needs.push(item(why === "ready to merge" ? "ok" : "act", why, `${node(p, true)} ${esc(p.title)}`, `#${p.pr.github.number} · ${esc(v.s.context.title)}`));
  }
  const moving = views.filter((v) => v.s.status.value === "running" || v.s.status.value === "waiting");
  const quiet = views.filter((v) => v.s.status.value === "idle" || v.s.status.value === "done");
  return `
    <section class="zone"><h3>Needs you · ${needs.length}</h3><div class="list">${needs.join("") || '<div class="zero">Nothing needs you.</div>'}</div></section>
    <section class="zone"><h3>Working · ${moving.length}</h3><div class="list">${moving.map(line).join("") || '<div class="zero">No Session is working.</div>'}</div></section>
    <section class="zone"><h3>Quiet · ${quiet.length}</h3><div class="list">${quiet.map(line).join("")}</div></section>`;
}

function item(kind: "act" | "ok", why: string, what: string, who: string): string {
  return `<div class="need">${lamp("act")}<span class="why ${kind}">${esc(why)}</span><span class="what">${what}</span><span class="who">${who}</span></div>`;
}

function line(v: SessionView): string {
  const strip = v.prs.map((p, i) => (i ? '<span class="bar"></span>' : "") + node(p, true)).join("");
  return `<div class="fly">${lamp(statusLamp[v.s.status.value], statusLabel[v.s.status.value])}
    <span><b style="font-weight:500">${esc(v.s.context.title)}</b> <span class="muted mono" style="font-size:12px">· ${esc(v.repo)}</span></span>
    <span class="strip" title="${v.prs.length} PRs">${strip || '<span class="muted" style="font-size:12px">no PRs</span>'}</span>
    <span class="muted" style="font-size:12px">${statusLabel[v.s.status.value]} · ${ago(v.s.status.at)}</span></div>`;
}
