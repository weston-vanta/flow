// PROTOTYPE dev server, a stand-in for flowd: serves the static frontend and
// GET /api/sessions, GET /api/prs from in-memory sample data that changes every
// few seconds so polling can be judged. GET /__down toggles a simulated outage.
import * as esbuild from "esbuild";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, "dist");
const PORT = Number(process.env.PORT ?? 7878);

const ctx = await esbuild.context({
  entryPoints: [path.join(here, "src/main.ts")],
  bundle: true, format: "iife", target: "es2022", outfile: path.join(out, "app.js"), logLevel: "info",
});
await ctx.watch();

const min = (m) => new Date(Date.now() - m * 60000).toISOString();
const S = (id, cde, title, status, startedMin, statusMin, branches, cwd) => ({
  id,
  origin: { cde, pane: "w1-p" + id.slice(0, 1), cwd, at: min(startedMin) },
  status: { value: status, at: min(statusMin) },
  context: { title, branch: branches[0]?.branch ?? null, at: min(statusMin) },
  branches: { value: branches, at: min(startedMin) },
});
const P = (repo, number, owner, title, o = {}) => ({
  key: `${repo}#${number}`, owner,
  github: {
    repo, number, url: `https://github.com/${repo}/pull/${number}`, title, isDraft: false, state: "OPEN",
    headRefName: "b" + number, additions: 40, deletions: 10, mergeable: "MERGEABLE", ci: "SUCCESS",
    unresolvedThreads: 0, reviewDecision: "APPROVED", pendingCodeOwners: [], stackPosition: 1, at: min(1), ...o,
  },
});

const sessions = [
  S("7f3a", "secondary", "Find root cause of flaky policy-engine timeout test", "running", 12, 0, [], "/workspaces/obsidian"),
  S("2c9e", "oncall", "Map new Okta MFA checks to SOC 2 controls", "waiting", 48, 3,
    [{ repo: "vanta/controls-catalog", branch: "grc-219-okta" }], "/workspaces/controls-catalog"),
  S("b81d", "flow", "Move evidence uploads to presigned S3 URLs", "running", 130, 0,
    [{ repo: "vanta/web", branch: "evd-412" }, { repo: "vanta/web", branch: "evd-413" }, { repo: "vanta/web", branch: "evd-414" }], "/workspaces/web"),
  S("e044", "secondary", "Upgrade services to Node 22", "idle", 1500, 95,
    [{ repo: "vanta/platform", branch: "plat-88" }], "/workspaces/platform"),
  S("91fd", "oncall", "Split notification preferences API by channel", "done", 6000, 2900,
    [{ repo: "vanta/api", branch: "notif-301" }], "/workspaces/api"),
];
const prs = [
  P("vanta/controls-catalog", 1307, "2c9e", "[GRC-219] Okta MFA check → CC6.1, CC6.6 mappings", { isDraft: true, additions: 64, deletions: 3, reviewDecision: null }),
  P("vanta/web", 4821, "b81d", "[EVD-412] Add upload_sessions table and model", { additions: 212, deletions: 18, stackPosition: 1 }),
  P("vanta/web", 4822, "b81d", "[EVD-413] Presign service with per-tenant key scoping", { additions: 486, deletions: 102, stackPosition: 2, ci: "FAILURE", unresolvedThreads: 3, reviewDecision: "CHANGES_REQUESTED", pendingCodeOwners: ["Jordan Okafor"] }),
  P("vanta/web", 4823, "b81d", "[EVD-414] Uploader UI: direct-to-S3 with progress", { additions: 331, deletions: 207, stackPosition: 3, mergeable: "CONFLICTING", ci: "PENDING", reviewDecision: "REVIEW_REQUIRED", pendingCodeOwners: ["Ana Lopez"] }),
  P("vanta/platform", 9918, "e044", "[PLAT-88] Bump .nvmrc, Dockerfiles and CI images to 22", { additions: 38, deletions: 38, stackPosition: 1, reviewDecision: "REVIEW_REQUIRED", pendingCodeOwners: ["Sam Patel"] }),
  P("vanta/platform", 9921, "e044", "Replace deprecated fs.rmdir recursive calls", { additions: 22, deletions: 27, stackPosition: 2, unresolvedThreads: 1, reviewDecision: "REVIEW_REQUIRED" }),
  P("vanta/platform", 9930, "e044", "[PLAT-91] Drop Node 18 polyfills", { additions: 5, deletions: 120, stackPosition: 3 }),
  P("vanta/api", 7702, "91fd", "[NOTIF-301] Channel enum and migration", { state: "MERGED", additions: 90, deletions: 12, stackPosition: 1 }),
  P("vanta/api", 7705, "91fd", "[NOTIF-302] GET/PUT /preferences/:channel endpoints", { state: "MERGED", additions: 274, deletions: 151, stackPosition: 2 }),
];

// Live changes so polling is visible: a Session flips running <-> waiting, CI finishes and restarts.
let tick = 0;
setInterval(() => {
  tick++;
  const now = new Date().toISOString();
  const s = sessions[0];
  s.status = { value: s.status.value === "running" ? "waiting" : "running", at: now };
  const p = prs.find((x) => x.github.number === 4823);
  p.github.ci = ["PENDING", "FAILURE", "PENDING", "SUCCESS"][tick % 4];
  p.github.at = now;
}, 6000);

let down = false;
const types = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };
http.createServer((req, res) => {
  const url = new URL(req.url, "http://x");
  const json = (v) => { res.writeHead(down ? 503 : 200, { "content-type": "application/json" }); res.end(down ? "{}" : JSON.stringify(v)); };
  if (url.pathname === "/api/sessions") return json(sessions);
  if (url.pathname === "/api/prs") return json(prs);
  if (url.pathname === "/__down") { down = !down; res.end(`down=${down}\n`); return; }
  const file = url.pathname === "/" ? path.join(here, "index.html")
    : url.pathname === "/app.js" ? path.join(out, "app.js")
    : url.pathname === "/style.css" ? path.join(here, "style.css") : null;
  if (!file || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { "content-type": types[path.extname(file)] });
  fs.createReadStream(file).pipe(res);
}).listen(PORT, "127.0.0.1", () => console.log(`PROTOTYPE: http://127.0.0.1:${PORT}/?variant=A  (toggle outage: curl 127.0.0.1:${PORT}/__down)`));
