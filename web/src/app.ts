// Polls flowd's read API, keeps the last good data in memory and redraws with innerHTML.
import { sessionViews } from "./model.ts";
import type { Pr, Session } from "./model.ts";
import { render } from "./render.ts";

const POLL_MS = 3000;

let sessions: Session[] = [];
let prs: Pr[] = [];
let drawn = "";

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}

async function poll() {
  try {
    [sessions, prs] = await Promise.all([get<Session[]>("/api/sessions"), get<Pr[]>("/api/prs")]);
  } catch (err) {
    console.warn("flow: poll failed, showing the last good data", err);
  }
  // Redraw only on change, so hover tooltips and selections survive quiet polls.
  const html = render(sessionViews(sessions, prs));
  if (html !== drawn) document.getElementById("root")!.innerHTML = drawn = html;
}

poll();
setInterval(poll, POLL_MS);
