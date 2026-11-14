// Polls flowd's read API, keeps the last good data in memory and redraws with innerHTML.
import { prDataAge, sessionViews, stepPr } from "./model.ts";
import type { Branch, Pr, Session } from "./model.ts";
import { render } from "./render.ts";
import type { Column, TrayState } from "./render.ts";

const POLL_MS = 3000;

let sessions: Session[] = [];
let prs: Pr[] = [];
let branches: Branch[] = [];
let drawn = "";
/** When the last poll succeeded, and whether the latest one failed. */
let lastGood: Date | null = null;
let failing = false;
/** Ids of Sessions folded or unfolded against their default (done Sessions start folded);
 * in memory only, so a reload resets them. */
const toggled = new Set<string>();
let legend = false;
/** The selected PR's tray, or null while the window shows its empty state. */
let tray: TrayState | null = null;
const root = document.getElementById("root")!;

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}

/** The page for the current state. Drops the tray once its PR leaves the list. */
function page(): string {
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
  const views = sessionViews(sessions, prs, branches);
  if (tray && !views.some((v) => v.prs.some((p) => p.url === tray!.url))) tray = null;
  return render(views, toggled, notices, !failing, legend, tray);
}

const $ = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel);

function draw() {
  // Redraw only on change, so hover tooltips and selections survive quiet polls.
  const html = page();
  if (html === drawn) return;
  const top = $(".tscroll")?.scrollTop ?? 0;
  root.innerHTML = drawn = html;
  $(".tscroll")!.scrollTop = top;
}

/** Selects the PR at `url`, with the window scrolled to `col`'s section (tinted briefly) or the top. */
function openTray(url: string, col: Column | null) {
  tray = { url, focus: col, expanded: tray?.expanded ?? new Set() };
  draw();
  $(".tscroll")!.scrollTop = col ? $(`[data-sec="${col}"]`)!.offsetTop : 0;
  $(".tclose")!.focus({ preventScroll: true });
  if (col) {
    setTimeout(() => {
      if (tray?.focus !== col) return;
      // Clear the tint on the drawn page rather than redrawing, so its fade runs.
      draw();
      tray.focus = null;
      $(".tsec.focus")?.classList.remove("focus");
      drawn = page();
    }, 1000);
  }
}

function closeTray() {
  tray = null;
  draw();
}

async function poll() {
  try {
    [sessions, prs, branches] = await Promise.all([
      get<Session[]>("/api/sessions"),
      get<Pr[]>("/api/prs"),
      get<Branch[]>("/api/branches"),
    ]);
    lastGood = new Date();
    failing = false;
  } catch (err) {
    failing = true;
    console.warn("flow: poll failed, showing the last good data", err);
  }
  draw();
}

root.addEventListener("click", (e) => {
  const target = e.target as Element;
  if (target.closest("[data-close]")) return closeTray();
  const thread = target.closest<HTMLElement>("[data-thread]");
  if (thread && tray) {
    const url = thread.dataset.thread!;
    if (!tray.expanded.delete(url)) tray.expanded.add(url);
    return draw();
  }
  const pr = target.closest<HTMLElement>("[data-open]");
  if (pr) {
    const col = (pr.dataset.col as Column | undefined) ?? null;
    // Clicking the selected strip's node or title again deselects it; its lamps still select.
    if (!col && tray?.url === pr.dataset.open) return closeTray();
    return openTray(pr.dataset.open!, col);
  }
  if (target.closest("[data-legend]")) {
    legend = !legend;
    return draw();
  }
  const el = target.closest<HTMLElement>("[data-toggle]");
  if (!el) return;
  const id = el.dataset.toggle!;
  if (!toggled.delete(id)) toggled.add(id);
  draw();
});

// ↑ ↓ step through the visible PRs; `h` toggles the legend; Esc deselects the PR, else closes
// the legend. Listening on the document means the page needs no focus first.
document.addEventListener("keydown", (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    const url = stepPr(sessionViews(sessions, prs, branches), toggled, tray?.url ?? null, e.key === "ArrowDown" ? 1 : -1);
    if (!url) return;
    e.preventDefault();
    openTray(url, null);
    $(".strip.sel")?.scrollIntoView({ block: "nearest" });
    return;
  }
  if (e.repeat) return;
  if (e.key === "Escape" && tray) return closeTray();
  if (e.key === "h" || e.key === "H") legend = !legend;
  else if (e.key === "Escape" && legend) legend = false;
  else return;
  draw();
});

poll();
setInterval(poll, POLL_MS);
