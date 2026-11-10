(() => {
  // src/model.ts
  function parseJira(title) {
    const m = /^\[([A-Z][A-Z0-9]*-\d+)\]\s+(.*)$/s.exec(title);
    return m ? { jira: m[1], title: m[2] } : { jira: null, title };
  }
  var FAILING = /* @__PURE__ */ new Set([
    "FAILURE",
    "ERROR",
    "TIMED_OUT",
    "CANCELLED",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE"
  ]);
  var PENDING = /* @__PURE__ */ new Set(["PENDING", "EXPECTED"]);
  function baseLight(g) {
    if (g.state === "MERGED") return { lamp: "fine", tip: "base: merged" };
    if (g.mergeable === "CONFLICTING") return { lamp: "act", tip: "base: conflict" };
    return { lamp: "fine", tip: "base: no conflict" };
  }
  function ciLight(g) {
    const failing2 = g.checks.filter((c) => c.conclusion && FAILING.has(c.conclusion));
    if (failing2.length) {
      return { lamp: "act", tip: `CI failing: ${failing2.map((c) => c.name).join(", ")}` };
    }
    const running = g.checks.filter((c) => !c.conclusion || PENDING.has(c.conclusion));
    if (running.length) {
      return { lamp: "run", tip: `CI running: ${running.map((c) => c.name).join(", ")}` };
    }
    if (!g.checks.length) return { lamp: "off", tip: "CI: no required checks" };
    return { lamp: "fine", tip: "CI: passing" };
  }
  function feedbackLight(g) {
    const n = g.unresolvedReviewThreads;
    if (g.state !== "MERGED" && n > 0) {
      return { lamp: "act", tip: `feedback: ${n} unresolved thread${n === 1 ? "" : "s"}` };
    }
    return { lamp: "fine", tip: "feedback: none open" };
  }
  function reviewLight(g) {
    const names = g.reviewRequests.map((r) => "login" in r ? r.login : r.team);
    const requested = names.length ? ` \xB7 requested: ${names.join(", ")}` : "";
    if (g.isDraft) return { lamp: "off", tip: `review: draft${requested}` };
    if (g.state === "MERGED") return { lamp: "fine", tip: "review: merged" };
    if (g.reviewDecision === "APPROVED") return { lamp: "fine", tip: `review: approved${requested}` };
    if (g.reviewDecision === "REVIEW_REQUIRED") {
      return { lamp: "run", tip: `review: required${requested}` };
    }
    if (g.reviewDecision === "CHANGES_REQUESTED") {
      return { lamp: "act", tip: `review: changes requested${requested}` };
    }
    return { lamp: "fine", tip: `review: none required${requested}` };
  }
  function prView(g, pos) {
    const base = baseLight(g);
    const ci = ciLight(g);
    const feedback = feedbackLight(g);
    const review = reviewLight(g);
    const open = g.state === "OPEN";
    const needs = open ? [
      base.lamp === "act" && "conflict",
      ci.lamp === "act" && "CI failing",
      feedback.lamp === "act" && "open feedback",
      review.lamp === "act" && "changes requested"
    ].filter((r) => !!r) : [];
    const ready = open && !g.isDraft && [base, ci, feedback, review].every((l) => l.lamp === "fine");
    return {
      pos,
      url: g.url,
      number: g.number,
      ...parseJira(g.title),
      additions: g.additions,
      deletions: g.deletions,
      node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : ready ? "ready" : g.isDraft ? "draft" : "open",
      base,
      ci,
      feedback,
      review,
      needs,
      ready
    };
  }
  function stackOf(session, prs2) {
    const last = Number.MAX_SAFE_INTEGER;
    return prs2.filter((p) => p.owner === session && p.github).map((p) => p.github).sort(
      (a, b) => (a.stack?.position ?? last) - (b.stack?.position ?? last) || a.number - b.number
    );
  }
  var STATUS_LAMP = { running: "run", waiting: "act", idle: "fine", done: "fine" };
  var STATUS_LABEL = {
    running: "running",
    waiting: "waiting for you",
    idle: "idle",
    done: "done"
  };
  function statusLight(state) {
    return state ? { lamp: STATUS_LAMP[state], tip: STATUS_LABEL[state] } : { lamp: "off", tip: "unknown" };
  }
  function activityLine(s) {
    const a = s.activity;
    if (!a) return null;
    return s.status?.state === "running" || a.reply == null ? a.prompt : a.reply;
  }
  function sessionViews(sessions2, prs2) {
    return sessions2.map((s) => {
      const stack = stackOf(s.id, prs2);
      const state = s.status?.state ?? null;
      const cwd = s.origin?.cwd;
      return {
        id: s.id,
        status: { ...statusLight(state), state, since: s.status?.at ?? null },
        title: s.context?.title ?? s.id,
        repo: stack[0]?.repo ?? (cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null),
        cde: s.origin?.cde ?? null,
        started: s.origin?.at ?? null,
        activity: activityLine(s),
        stack: stack.map((g, i) => prView(g, i + 1))
      };
    });
  }
  function age(iso, now = Date.now()) {
    const m = Math.floor((now - Date.parse(iso)) / 6e4);
    if (m < 1) return "just now";
    if (m < 60) return `${m} min`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h} h`;
    return `${Math.floor(h / 24)} d`;
  }
  function stackLight(stack) {
    if (!stack.length) return { lamp: "off", tip: "no pull requests" };
    const needs = stack.filter((p) => p.needs.length).length;
    if (needs) return { lamp: "act", tip: `${needs} of ${stack.length} PRs need attention` };
    const open = stack.filter((p) => p.node !== "merged" && p.node !== "closed");
    if (open.some((p) => [p.base, p.ci, p.feedback, p.review].some((l) => l.lamp === "run"))) {
      return { lamp: "run", tip: "PRs in progress" };
    }
    return { lamp: "fine", tip: "PRs fine" };
  }
  function prDataAge(prs2, now = Date.now()) {
    const ats = prs2.flatMap((p) => p.github?.state === "OPEN" ? [Date.parse(p.github.at)] : []);
    if (!ats.length) return null;
    const ms = now - Math.min(...ats);
    return ms > 3 * 6e4 ? Math.floor(ms / 6e4) : null;
  }

  // src/render.ts
  function esc(s) {
    return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
  }
  var ago = (iso) => {
    const a = age(iso);
    return a === "just now" ? a : `${a} ago`;
  };
  var lamp = (l, tip, sm = false) => `<span class="lamp ${l}${sm ? " sm" : ""}" title="${esc(tip)}"></span>`;
  function prRow(p, j, n) {
    const notes = p.ready ? ["ready to merge"] : p.needs;
    return `<div class="grid grow">
    ${j > 0 ? '<div class="rule"></div>' : ""}
    <div class="gnode">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<a class="node ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer" title="#${p.number} \xB7 ${p.node}">${p.pos}</a></div>
    <div class="gtitle"><a class="t ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer">${esc(p.title)}</a><span class="sub">${p.jira ? `<span class="mono">${esc(p.jira)}</span>` : ""}${notes.length ? `<span class="needs">${esc(notes.join(" \xB7 "))}</span>` : ""}</span></div>
    <div class="glines mono">+${p.additions} \u2212${p.deletions}</div>
    <div class="gcell">${lamp(p.base.lamp, p.base.tip)}</div>
    <div class="gcell">${lamp(p.ci.lamp, p.ci.tip)}</div>
    <div class="gcell">${lamp(p.feedback.lamp, p.feedback.tip)}</div>
    <div class="gcell">${lamp(p.review.lamp, p.review.tip)}</div>
  </div>`;
  }
  function sessionRow(v, i, collapsed2) {
    const idx = String(i + 1).padStart(2, "0");
    const stat = `<div class="sstat ${v.status.lamp}">${lamp(v.status.lamp, v.status.tip, true)}${esc(v.status.tip)}${v.status.since ? ` \xB7 ${age(v.status.since)}` : ""}</div>`;
    if (collapsed2.has(v.id)) {
      const s = stackLight(v.stack);
      return `<section class="srow folded" data-toggle="${esc(v.id)}" title="Show this Session">
      <div class="sidx">${idx}</div>
      <div class="sline">${stat}<b>${esc(v.title)}</b><span class="cde">${esc(v.cde ?? "unknown CDE")}</span>${lamp(s.lamp, `Stack: ${s.tip}`)}</div>
    </section>`;
    }
    const head = `<div class="sidx">${idx}<button class="fold" data-toggle="${esc(v.id)}" title="Collapse this Session">\u2212</button></div>
    <div class="sinfo">
      ${stat}
      <h2>${esc(v.title)}</h2>
      ${v.activity ? `<p class="activity" title="${esc(v.activity)}">${esc(v.activity)}</p>` : ""}
      <div class="smeta">
        ${v.repo ? `<span class="mono">${esc(v.repo)}</span>` : ""}
        <span>${esc(v.cde ?? "unknown CDE")}${v.started ? ` \xB7 started ${ago(v.started)}` : ""}</span>
      </div>
    </div>`;
    const body = v.stack.length ? `<div class="grid ghead"><span></span><span></span><span></span><span>base</span><span>CI</span><span>feedback</span><span>review</span></div>` + v.stack.map((p, j) => prRow(p, j, v.stack.length)).join("") : `<div class="noprs"><span class="lamp off"></span><span>No pull requests yet</span></div>`;
    return `<section class="srow">${head}<div class="sprs">${body}</div></section>`;
  }
  function render(views, collapsed2, notices) {
    return `<div class="page">
    <header class="top"><div class="logo"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    ${notices.map((n) => `<p class="notice">${esc(n)}</p>`).join("")}
    <main class="panel">${views.length ? views.map((v, i) => sessionRow(v, i, collapsed2)).join("") : '<p class="empty">No Sessions.</p>'}</main>
    <footer class="key">
      <span><span class="lamp act"></span>needs action</span>
      <span><span class="lamp run"></span>in progress</span>
      <span><span class="lamp fine"></span>fine</span>
      <span><span class="lamp off"></span>does not apply</span>
      <span>PR <span class="node key"></span>open <span class="node key draft"></span>draft <span class="node key ready"></span>ready to merge <span class="node key merged"></span>merged <span class="node key closed"></span>closed</span>
    </footer>
  </div>`;
  }

  // src/app.ts
  var POLL_MS = 3e3;
  var sessions = [];
  var prs = [];
  var drawn = "";
  var lastGood = null;
  var failing = false;
  var collapsed = /* @__PURE__ */ new Set();
  var root = document.getElementById("root");
  async function get(path) {
    const r = await fetch(path);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }
  function draw() {
    const notices = [];
    if (failing) {
      notices.push(
        lastGood ? `flowd unreachable \xB7 showing data from ${lastGood.toLocaleTimeString()}` : "flowd unreachable"
      );
    }
    const stale = prDataAge(prs);
    if (stale != null) notices.push(`PR data ${stale} min old`);
    const html = render(sessionViews(sessions, prs), collapsed, notices);
    if (html !== drawn) root.innerHTML = drawn = html;
  }
  async function poll() {
    try {
      [sessions, prs] = await Promise.all([get("/api/sessions"), get("/api/prs")]);
      lastGood = /* @__PURE__ */ new Date();
      failing = false;
    } catch (err) {
      failing = true;
      console.warn("flow: poll failed, showing the last good data", err);
    }
    draw();
  }
  root.addEventListener("click", (e) => {
    const el = e.target.closest("[data-toggle]");
    if (!el) return;
    const id = el.dataset.toggle;
    if (!collapsed.delete(id)) collapsed.add(id);
    draw();
  });
  poll();
  setInterval(poll, POLL_MS);
})();
