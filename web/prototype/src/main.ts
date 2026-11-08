// PROTOTYPE entry. Polls flowd's read API, keeps the last good data in memory,
// re-renders the chosen variant. Switch variants with ?variant=, the bar, or ← →.
import { Pr, Session, sessionViews } from "./model";
import * as A from "./variantA";
import * as B from "./variantB";
import * as C from "./variantC";

const VARIANTS: Record<string, { name: string; render: typeof A.render }> = { A, B, C };
const KEYS = Object.keys(VARIANTS);
const POLL_MS = 3000;

let sessions: Session[] = [];
let prs: Pr[] = [];
let lastOk = 0;
let failing = false;

function variant(): string {
  const v = new URLSearchParams(location.search).get("variant") ?? "A";
  return KEYS.includes(v) ? v : "A";
}

function setVariant(k: string) {
  const u = new URL(location.href);
  u.searchParams.set("variant", k);
  history.replaceState(null, "", u);
  draw();
}

function draw() {
  const k = variant();
  const views = sessionViews(sessions, prs);
  const conn = failing
    ? `<span class="conn down">flowd unreachable · showing data from ${new Date(lastOk).toLocaleTimeString()}</span>`
    : `<span class="conn">${sessions.length} sessions · ${prs.length} PRs</span>`;
  document.getElementById("root")!.innerHTML = `<div class="page">
    <header class="top"><div class="logo"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div>${conn}</header>
    ${VARIANTS[k].render(views)}
    <footer class="key">
      <span><span class="lamp act"></span>needs action</span>
      <span><span class="lamp run"></span>in progress</span>
      <span><span class="lamp"></span>fine</span>
      <span><span class="count">2</span>reviews needed</span>
      <span>PR <span class="node" style="width:14px;height:14px"></span>open <span class="node draft" style="width:14px;height:14px"></span>draft <span class="node mergeable" style="width:14px;height:14px"></span>mergeable <span class="node merged" style="width:14px;height:14px"></span>merged</span>
    </footer></div>`;
  document.getElementById("switcher")!.innerHTML = `<div class="sw">
    <button data-d="-1" aria-label="previous variant">←</button>
    <span class="lbl">${k} · ${VARIANTS[k].name}</span>
    <button data-d="1" aria-label="next variant">→</button>
    <span class="st">poll ${POLL_MS / 1000}s · ${lastOk ? new Date(lastOk).toLocaleTimeString() : "never"}</span></div>`;
}

function step(d: number) {
  const i = KEYS.indexOf(variant());
  setVariant(KEYS[(i + d + KEYS.length) % KEYS.length]);
}

document.addEventListener("click", (e) => {
  const b = (e.target as HTMLElement).closest(".sw button") as HTMLElement | null;
  if (b) step(Number(b.dataset.d));
});
document.addEventListener("keydown", (e) => {
  const t = e.target as HTMLElement;
  if (t.closest("input,textarea,[contenteditable]")) return;
  if (e.key === "ArrowLeft") step(-1);
  if (e.key === "ArrowRight") step(1);
});

async function poll() {
  try {
    const [s, p] = await Promise.all([
      fetch("/api/sessions").then((r) => { if (!r.ok) throw r.status; return r.json(); }),
      fetch("/api/prs").then((r) => { if (!r.ok) throw r.status; return r.json(); }),
    ]);
    sessions = s; prs = p; lastOk = Date.now(); failing = false;
  } catch {
    failing = true;
  }
  draw();
}

poll();
setInterval(poll, POLL_MS);
