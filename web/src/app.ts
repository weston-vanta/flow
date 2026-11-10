// Polls flowd's read API, keeps the last good data in memory and redraws with innerHTML.
import { prDataAge, sessionViews } from "./model.ts";
import type { Pr, Session } from "./model.ts";
import { render } from "./render.ts";

const POLL_MS = 3000;

let sessions: Session[] = [];
let prs: Pr[] = [];
let drawn = "";
/** When the last poll succeeded, and whether the latest one failed. */
let lastGood: Date | null = null;
let failing = false;
/** Ids of collapsed Sessions; in memory only, so a reload reopens them all. */
const collapsed = new Set<string>();
const root = document.getElementById("root")!;

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}

function draw() {
  const notices: string[] = [];
  if (failing) {
    notices.push(
      lastGood
        ? `flowd unreachable · showing data from ${lastGood.toLocaleTimeString()}`
        : "flowd unreachable",
    );
  }
  const stale = prDataAge(prs);
  if (stale != null) notices.push(`PR data ${stale} min old`);
  // Redraw only on change, so hover tooltips and selections survive quiet polls.
  const html = render(sessionViews(sessions, prs), collapsed, notices);
  if (html !== drawn) root.innerHTML = drawn = html;
}

async function poll() {
  try {
    [sessions, prs] = await Promise.all([get<Session[]>("/api/sessions"), get<Pr[]>("/api/prs")]);
    lastGood = new Date();
    failing = false;
  } catch (err) {
    failing = true;
    console.warn("flow: poll failed, showing the last good data", err);
  }
  draw();
}

root.addEventListener("click", (e) => {
  const el = (e.target as Element).closest<HTMLElement>("[data-toggle]");
  if (!el) return;
  const id = el.dataset.toggle!;
  if (!collapsed.delete(id)) collapsed.add(id);
  draw();
});

poll();
setInterval(poll, POLL_MS);
