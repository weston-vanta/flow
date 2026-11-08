// PROTOTYPE variant B: one dense ledger. A row per PR, grouped under a row per Session.
import { SessionView, ago, esc, lamp, node, reviewCell, statusLabel, statusLamp } from "./model";

export const name = "Ledger (dense table)";

export function render(views: SessionView[]): string {
  const body = views.map((v) => {
    const s = v.s;
    const srow = `<tr class="s">
      <td>${lamp(statusLamp[s.status.value], statusLabel[s.status.value], true)}</td>
      <td class="title" title="${esc(s.context.title)}">${esc(s.context.title)}</td>
      <td class="small ${s.status.value === "waiting" ? "act-text" : ""}">${statusLabel[s.status.value]} · ${ago(s.status.at)}</td>
      <td class="small mono">${esc(s.origin.cde)}</td>
      <td class="small mono">${esc(v.repo)}</td>
      <td colspan="5" class="small">${v.prs.length === 0 ? "no pull requests yet" : ""}</td></tr>`;
    const prows = v.prs.map((p) => `<tr class="p">
      <td></td>
      <td class="title" title="${esc(p.pr.github.title)}">${node(p)}${esc(p.title)}</td>
      <td class="small mono">${p.jira ? esc(p.jira) : ""}</td>
      <td class="small mono">#${p.pr.github.number}</td>
      <td class="small mono">+${p.pr.github.additions} −${p.pr.github.deletions}</td>
      <td class="c">${lamp(p.base.lamp, p.base.tip)}</td>
      <td class="c">${lamp(p.ci.lamp, p.ci.tip)}</td>
      <td class="c">${lamp(p.feedback.lamp, p.feedback.tip)}</td>
      <td class="c">${reviewCell(p)}</td>
      <td class="small">${ago(p.pr.github.at)}</td></tr>`).join("");
    return srow + prows;
  }).join("");
  return `<main class="ledger"><table>
    <thead><tr><th></th><th>session / pull request</th><th></th><th></th><th></th>
    <th class="c">base</th><th class="c">CI</th><th class="c">feedback</th><th class="c">review</th><th>checked</th></tr></thead>
    <tbody>${body}</tbody></table></main>`;
}
