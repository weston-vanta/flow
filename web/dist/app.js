(() => {
  // src/model.ts
  function parseJira(title2) {
    const m = /^\[([A-Z][A-Z0-9]*-\d+)\]\s+(.*)$/s.exec(title2);
    return m ? { jira: m[1], title: m[2] } : { jira: null, title: title2 };
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
    if (!g.checks.length) return { lamp: "fine", tip: "CI: no required checks" };
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
    const light = (lamp2, tip, count = null) => ({ lamp: lamp2, tip, count });
    if (g.state !== "OPEN") return light("fine", `review: ${g.state === "MERGED" ? "merged" : "closed"}`);
    if (g.reviewDecision === "CHANGES_REQUESTED") return light("act", "review: changes requested");
    const names = g.reviewRequests.map((r) => "login" in r ? r.login : r.team);
    if (names.length) return light("fine", `review needed: ${names.join(", ")}`, names.length);
    if (g.reviewDecision === "REVIEW_REQUIRED") return light("act", "review: no reviewers requested");
    return light("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
  }
  function prView(g, pos) {
    const base = baseLight(g);
    const ci = ciLight(g);
    const feedback = feedbackLight(g);
    const review = reviewLight(g);
    const lamps = [base, ci, feedback, review];
    const open = g.state === "OPEN";
    const ready = open && !g.isDraft && review.count == null && lamps.every((l) => l.lamp === "fine");
    return {
      pos,
      url: g.url,
      number: g.number,
      ...parseJira(g.title),
      additions: g.additions,
      deletions: g.deletions,
      node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : ready ? "ready" : "open",
      draft: g.isDraft,
      base,
      ci,
      feedback,
      review,
      attention: open && (ready || lamps.some((l) => l.lamp === "act"))
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
      const title2 = s.context?.title ?? null;
      return {
        id: s.id,
        status: { ...statusLight(state), state, since: s.status?.at ?? null },
        title: title2 ?? "New session",
        untitled: title2 == null,
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
  function attentionLight(v) {
    const n = v.stack.length;
    const prs2 = n ? `${n} pull request${n === 1 ? "" : "s"}` : "No pull requests";
    if (v.status.state === "waiting") return { lamp: "act", tip: `${prs2}: waiting for you` };
    if (v.stack.some((p) => p.attention)) return { lamp: "act", tip: `${prs2}: something needs action` };
    return { lamp: "fine", tip: `${prs2}: nothing needs action` };
  }
  function isFolded(v, toggled2) {
    return v.status.state === "done" !== toggled2.has(v.id);
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
  var lamp = (l, sm = false) => `<span class="lamp ${l.lamp}${sm ? " sm" : ""}" aria-hidden="true"></span>`;
  var cell = (l) => {
    const count = "count" in l && l.count != null;
    return `<div class="gcell" role="cell" title="${esc(l.tip)}" aria-label="${esc(l.tip)}">${count ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l)}</div>`;
  };
  var NODE_TIP = { merged: "merged", closed: "closed", ready: "mergeable" };
  function prRow(p, j, n) {
    const tip = `#${p.number} \xB7 ${p.node === "open" ? p.draft ? "draft" : "not yet mergeable" : NODE_TIP[p.node]}`;
    return `<div class="grid grow" role="row">
    ${j > 0 ? '<div class="rule" aria-hidden="true"></div>' : ""}
    <div class="gnode" role="cell">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<a class="node ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer" title="${esc(tip)}" aria-label="Pull request ${esc(tip)}">${p.pos}</a></div>
    <div class="gtitle" role="cell"><a class="t ${p.node}" href="${esc(p.url)}" target="_blank" rel="noreferrer">${esc(p.title)}</a>${p.jira ? `<span class="jira mono">${esc(p.jira)}</span>` : ""}</div>
    <div class="glines mono" role="cell">+${p.additions} \u2212${p.deletions}</div>
    ${cell(p.base)}${cell(p.ci)}${cell(p.feedback)}${cell(p.review)}
  </div>`;
  }
  var key = (v, idx, folded) => `<button type="button" class="skey${folded ? " up" : ""}" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" aria-label="${folded ? "Expand" : "Collapse"} session ${idx}"><span>${idx}</span></button>`;
  var title = (v, tag) => `<${tag}${v.untitled ? ' class="untitled"' : ""}>${esc(v.title)}</${tag}>`;
  function status(v) {
    const since = v.status.since ? ` \xB7 ${age(v.status.since)}` : "";
    return `<div class="sstat">${lamp(v.status, true)}<span><b class="${v.status.lamp}">${esc(v.status.tip)}</b>${since}</span></div>`;
  }
  function sessionRow(v, i, toggled2) {
    const idx = String(i + 1).padStart(2, "0");
    const cde = esc(v.cde ?? "unknown CDE");
    if (isFolded(v, toggled2)) {
      const a = attentionLight(v);
      return `<section class="srow folded" data-toggle="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="sidx">${key(v, idx, true)}</div>
      <div class="sline">${title(v, "h2")}${status(v)}<span class="cde">${cde}</span><span class="attn" title="${esc(a.tip)}" aria-label="${esc(a.tip)}">${lamp(a)}</span></div>
    </section>`;
    }
    const activity = v.activity?.replace(/\s+/g, " ");
    const info = `<div class="sinfo">
      <div class="smeta">
        ${activity ? `<span class="activity" title="${esc(v.activity)}">${esc(activity)}</span>` : ""}
        <span>${cde}${v.repo ? ` \xB7 <span class="mono">${esc(v.repo)}</span>` : ""}${v.started ? ` \xB7 started ${ago(v.started)}` : ""}</span>
      </div>
    </div>`;
    const body = v.stack.length ? `<div class="scroll"><div role="table" aria-label="Pull requests">
        <div class="grid ghead" role="row"><span></span><span></span><span></span><span role="columnheader">base</span><span role="columnheader">CI</span><span role="columnheader">feedback</span><span role="columnheader">review</span></div>
        ${v.stack.map((p, j) => prRow(p, j, v.stack.length)).join("")}
      </div></div>` : `<div class="noprs"><span class="ring" aria-hidden="true"></span><span>No pull requests yet</span></div>`;
    return `<section class="srow" aria-label="${esc(v.title)}"><div class="sidx">${key(v, idx, false)}</div>
    <div class="sbody"><div class="shead">${title(v, "h2")}${status(v)}</div><div class="sdetail">${info}<div class="sprs">${body}</div></div></div>
  </section>`;
  }
  var LEGEND = `<div class="legend" role="note" aria-label="Legend">
    <div><span class="lh">lights</span>
      <span class="li act"><span class="lamp act"></span>needs action</span>
      <span class="li"><span class="lamp run"></span>in progress</span>
      <span class="li"><span class="lamp fine"></span>fine</span>
      <span class="li"><span class="count">2</span>reviews needed</span>
    </div>
    <div><span class="lh">pull requests</span>
      <span class="li"><span class="node key"></span>open or draft</span>
      <span class="li"><span class="node key ready"></span>mergeable</span>
      <span class="li"><span class="node key merged"></span>merged</span>
      <span class="li"><span class="node key closed"></span>closed</span>
    </div>
  </div>`;
  function render(views, toggled2, notices, legend2) {
    return `<div class="page${notices.length ? " noticed" : ""}">
    <header class="top"><div class="logo" aria-hidden="true"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    <main class="panel">${views.length ? views.map((v, i) => sessionRow(v, i, toggled2)).join("") : '<p class="empty">No Sessions.</p>'}</main>
    <footer><button type="button" class="hint" data-legend aria-expanded="${legend2}" aria-keyshortcuts="h"><kbd class="${legend2 ? "down" : ""}">h</kbd><span>legend</span></button></footer>
  </div>
  <div class="dock">
    ${legend2 ? LEGEND : ""}
    ${notices.length ? `<div class="notices" role="status">${notices.map((n) => `<p>${esc(n)}</p>`).join("")}</div>` : ""}
  </div>`;
  }

  // src/app.ts
  var POLL_MS = 3e3;
  var sessions = [];
  var prs = [];
  var drawn = "";
  var lastGood = null;
  var failing = false;
  var toggled = /* @__PURE__ */ new Set();
  var legend = false;
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
    const html = render(sessionViews(sessions, prs), toggled, notices, legend);
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
    const target = e.target;
    if (target.closest("[data-legend]")) {
      legend = !legend;
      return draw();
    }
    const el = target.closest("[data-toggle]");
    if (!el) return;
    const id = el.dataset.toggle;
    if (!toggled.delete(id)) toggled.add(id);
    draw();
  });
  document.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
    if (e.key === "h" || e.key === "H") legend = !legend;
    else if (e.key === "Escape" && legend) legend = false;
    else return;
    draw();
  });
  poll();
  setInterval(poll, POLL_MS);
})();
