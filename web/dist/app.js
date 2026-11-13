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
  var DAY = 24 * 60 * 6e4;
  var baseName = (g, below) => below != null ? `#${below}` : g.baseRefName;
  function baseProblems(g, below, now) {
    if (g.state !== "OPEN") return [];
    const out = [];
    if (g.mergeable === "CONFLICTING") out.push(`Merge conflict with ${baseName(g, below)}`);
    if (g.behindSince && now - Date.parse(g.behindSince) > DAY) {
      out.push(`Behind ${g.baseRefName} since ${age(g.behindSince, now)} ago`);
    }
    return out;
  }
  var done = (g, column) => g.state === "OPEN" ? null : { lamp: "fine", tip: `${column}: ${g.state === "MERGED" ? "merged" : "closed"}` };
  function baseLight(g, below = null, now = Date.now()) {
    const problems = baseProblems(g, below, now);
    if (problems.length) {
      return { lamp: "act", tip: `base: ${problems.map((p) => p[0].toLowerCase() + p.slice(1)).join(" \xB7 ")}` };
    }
    return done(g, "base") ?? { lamp: "fine", tip: "base: no conflict" };
  }
  var failingChecks = (g) => g.checks.filter((c) => c.conclusion && FAILING.has(c.conclusion));
  function ciLight(g) {
    const closed = done(g, "CI");
    if (closed) return closed;
    const failing2 = failingChecks(g);
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
    const n = g.threads.length;
    if (g.state === "OPEN" && n > 0) {
      return { lamp: "act", tip: `feedback: ${n} unresolved thread${n === 1 ? "" : "s"}` };
    }
    return done(g, "feedback") ?? { lamp: "fine", tip: "feedback: none open" };
  }
  function reviewLight(g) {
    const light2 = (lamp2, tip, count = null) => ({ lamp: lamp2, tip, count });
    const closed = done(g, "review");
    if (closed) return { ...closed, count: null };
    if (g.reviewDecision === "CHANGES_REQUESTED") return light2("act", "review: changes requested");
    const names = g.reviewRequests.map((r) => "login" in r ? r.login : r.team);
    if (names.length) return light2("fine", `review needed: ${names.join(", ")}`, names.length);
    if (g.reviewDecision === "REVIEW_REQUIRED") return light2("act", "review: no reviewers requested");
    return light2("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
  }
  function prView(g, pos, below = null, now = Date.now()) {
    const base = baseLight(g, below, now);
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
      size: `${g.changedFiles} / ${g.additions + g.deletions}`,
      sizeTip: `${g.changedFiles} file${g.changedFiles === 1 ? "" : "s"}, +${g.additions} \u2212${g.deletions}`,
      node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : ready ? "ready" : "open",
      draft: g.isDraft,
      base,
      ci,
      feedback,
      review,
      attention: open && (ready || lamps.some((l) => l.lamp === "act")),
      tray: trayView(g, below, now)
    };
  }
  function section(items2, fine) {
    return items2.length ? { fine: null, items: items2 } : { fine, items: null };
  }
  function reviewers(g) {
    const requested = g.reviewRequests.map((r) => ({
      name: "login" in r ? r.login : r.team,
      team: !("login" in r),
      state: "requested",
      codeOwner: r.asCodeOwner,
      teams: []
    }));
    const pending = new Set(g.reviewRequests.flatMap((r) => "login" in r ? [r.login] : []));
    const reviewed = g.reviews.filter((r) => !pending.has(r.login)).map((r) => ({
      name: r.login,
      team: false,
      state: r.state === "APPROVED" ? "approved" : "changes requested",
      codeOwner: false,
      teams: r.teams
    }));
    return [...requested, ...reviewed];
  }
  function trayView(g, below = null, now = Date.now()) {
    const word = g.state === "MERGED" ? "merged" : "closed";
    if (g.state !== "OPEN") {
      return {
        base: section([], word),
        ci: section([], "passed"),
        feedback: section([], "no open threads"),
        review: section([], word)
      };
    }
    const running = g.checks.some((c) => !c.conclusion || PENDING.has(c.conclusion));
    const threads = [...g.threads].sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt)).map((t) => ({
      author: t.login,
      where: t.line == null ? t.path : `${t.path}:${t.line}`,
      body: t.body,
      long: t.body.length > 280 || t.body.split("\n").length > 4,
      url: t.url,
      at: t.createdAt,
      ago: age(t.createdAt, now)
    }));
    const review = g.reviewDecision === "REVIEW_REQUIRED" ? "required, none requested" : g.reviewDecision === "APPROVED" ? "approved" : "not required";
    return {
      base: section(baseProblems(g, below, now), below == null ? "no conflict, up to date" : "no conflict"),
      ci: section(
        failingChecks(g).map((c) => ({ name: c.name, url: c.url })),
        running ? "running" : "passing"
      ),
      feedback: section(threads, "no open threads"),
      review: section(reviewers(g), review)
    };
  }
  function stacksOf(session, prs2) {
    const groups = /* @__PURE__ */ new Map();
    for (const p of prs2) {
      const g = p.github;
      if (p.owner !== session || !g) continue;
      const id = g.stack?.number ?? `#${g.number}`;
      groups.set(id, [...groups.get(id) ?? [], g]);
    }
    return [...groups.values()].map((s) => s.sort((a, b) => a.stack.position - b.stack.position || a.number - b.number)).sort((a, b) => a[0].number - b[0].number);
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
  function sessionViews(sessions2, prs2, branches2 = [], now = Date.now()) {
    const heads = new Set(prs2.flatMap((p) => p.github ? [`${p.github.repo}:${p.github.headRefName}`] : []));
    const loose = branches2.filter((b) => !heads.has(b.key)).sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
    const done2 = (s) => s.status?.state === "done" ? 1 : 0;
    const started = (s) => s.origin ? Date.parse(s.origin.at) : -Infinity;
    const sorted = [...sessions2].sort((a, b) => done2(a) - done2(b) || started(b) - started(a));
    return sorted.map((s) => {
      const raw = stacksOf(s.id, prs2);
      const stacks = raw.map(
        (stack) => stack.map((g, i) => {
          const below = stack.find((o) => o.headRefName === g.baseRefName)?.number ?? null;
          return prView(g, stack.length > 1 ? i + 1 : null, below, now);
        })
      );
      const own = loose.filter((b) => b.owner === s.id);
      const state = s.status?.state ?? null;
      const cwd = s.origin?.cwd;
      const title2 = s.context?.title ?? null;
      return {
        id: s.id,
        status: { ...statusLight(state), state, since: s.status?.at ?? null },
        title: title2 ?? "New session",
        untitled: title2 == null,
        repo: raw[0]?.[0].repo ?? own[0]?.repo ?? (cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null),
        cde: s.origin?.cde ?? null,
        activity: activityLine(s),
        stacks,
        prs: stacks.flat(),
        branches: own.map((b) => b.branch)
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
    const n = v.prs.length;
    const prs2 = n ? `${n} pull request${n === 1 ? "" : "s"}` : "No pull requests";
    if (v.status.state === "waiting") return { lamp: "act", tip: `${prs2}: waiting for you` };
    if (v.prs.some((p) => p.attention)) return { lamp: "act", tip: `${prs2}: something needs action` };
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
  var lamp = (l, sm = false) => `<span class="lamp ${l.lamp}${sm ? " sm" : ""}" aria-hidden="true"></span>`;
  var light = (l) => "count" in l && l.count != null ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l);
  var COLUMNS = [
    ["base", "base"],
    ["ci", "CI"],
    ["feedback", "feedback"],
    ["review", "review"]
  ];
  var cell = (p, col, label) => {
    const l = p[col];
    return `<div class="gcell" role="cell"><button type="button" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}">${light(l)}</button></div>`;
  };
  var NODE_TIP = { merged: "merged", closed: "closed", ready: "mergeable" };
  var nodeWord = (p) => p.node === "open" ? p.draft ? "draft" : "not yet mergeable" : NODE_TIP[p.node];
  function prRow(p, j, n, selected) {
    const tip = `#${p.number} \xB7 ${nodeWord(p)}`;
    return `<div class="grid grow${selected ? " sel" : ""}" role="row">
    ${j > 0 ? '<div class="rule" aria-hidden="true"></div>' : ""}
    <div class="gnode" role="cell">${j > 0 ? '<div class="up"></div>' : ""}${j < n - 1 ? '<div class="down"></div>' : ""}<button type="button" class="node ${p.node}" data-open="${esc(p.url)}" title="${esc(tip)}" aria-label="Pull request ${esc(tip)}: show details">${p.pos ?? ""}</button></div>
    <div class="gtitle" role="cell"><button type="button" class="t ${p.node}" data-open="${esc(p.url)}">${esc(p.title)}</button>${p.jira ? `<span class="jira mono">${esc(p.jira)}</span>` : ""}</div>
    <div class="glines mono" role="cell" title="${esc(p.sizeTip)}" aria-label="${esc(p.sizeTip)}">${esc(p.size)}</div>
    ${COLUMNS.map(([col, label]) => cell(p, col, label)).join("")}
  </div>`;
  }
  var branchRow = (name) => `<div class="sgroup" role="rowgroup"><div class="grid grow" role="row">
    <div class="gnode" role="cell"></div>
    <div class="gtitle" role="cell"><span class="branch mono muted" title="Branch with no pull request">${esc(name)}</span></div>
    <div class="glines" role="cell"></div>${'<div class="gcell" role="cell"></div>'.repeat(COLUMNS.length)}
  </div></div>`;
  var ext = (url, cls, title2, text) => `<a class="${cls}" href="${esc(url)}" target="_blank" rel="noreferrer" title="${esc(title2)}">${esc(text)}</a>`;
  function items(p, col, expanded) {
    const t = p.tray;
    if (col === "base") return t.base.items.map((s) => `<span>${esc(s)}</span>`).join("");
    if (col === "ci") {
      return t.ci.items.map((c) => c.url ? ext(c.url, "item mono", "Open job", c.name) : `<span class="item mono">${esc(c.name)}</span>`).join("");
    }
    if (col === "feedback") {
      return t.feedback.items.map((th) => {
        const open = expanded.has(th.url);
        return `<article>
        <div class="tby"><b>${esc(th.author)}</b>${ext(th.url, "item mono", `Open comment: ${th.where}`, th.where)}<span title="${esc(th.at)}">${esc(th.ago)}</span></div>
        <p${th.long && !open ? ' class="clamp"' : ""}>${esc(th.body)}</p>
        ${th.long ? `<button type="button" class="plain" data-thread="${esc(th.url)}" aria-expanded="${open}">${open ? "show less" : "show all"}</button>` : ""}
      </article>`;
      }).join("");
    }
    return t.review.items.map((r) => `<div class="rv">
      <span class="${r.team ? "who mono" : "who"}">${esc(r.name)}</span>
      ${r.codeOwner ? '<span class="co" title="Requested as code owner">code owner</span>' : ""}
      ${r.teams.length ? `<span class="for mono" title="Reviewed for ${esc(r.teams.join(", "))}">${esc(r.teams.join(", "))}</span>` : ""}
      <span class="state${r.state === "changes requested" ? " act" : ""}">${r.state}</span>
    </div>`).join("");
  }
  function tray(p, t) {
    const word = nodeWord(p);
    const state = p.draft || p.node === "closed" ? `<span>${p.node === "closed" ? "closed" : "draft"}</span>` : "";
    const sections = COLUMNS.map(([col, label]) => {
      const s = p.tray[col];
      const l = p[col];
      return `<section class="tsec${t.focus === col ? " focus" : ""}" data-sec="${col}" aria-label="${label}">
      <div class="tlamp" title="${esc(l.tip)}">${light(l)}</div>
      <div class="thd"><h4>${label}</h4>${s.fine != null ? `<span class="${l.lamp === "act" ? "act" : ""}">${esc(s.fine)}</span>` : ""}</div>
      ${s.items ? `<div class="titems ${col}">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
    }).join("");
    return `<header class="thead">
      <span class="node ${p.node}" title="${esc(word)}" aria-hidden="true"></span>
      <div class="tmeta">${ext(p.url, "plain mono", `Open #${p.number} on GitHub`, `#${p.number}`)}${state}${p.jira ? `<span class="mono">${esc(p.jira)}</span>` : ""}</div>
      <button type="button" class="tclose" data-close aria-label="Close details"><svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>
      <h3>${esc(p.title)}</h3>
    </header>${sections}`;
  }
  var statusTip = (v) => v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip;
  var key = (v, folded) => `<button type="button" class="skey${folded ? " up" : ""}" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"><span>${lamp(v.status, true)}</span></button>`;
  var title = (v, tag) => `<${tag}${v.untitled || v.status.state === "done" ? ' class="muted"' : ""}>${esc(v.title)}</${tag}>`;
  var DOT = '<span class="dot" aria-hidden="true">\xB7</span>';
  function meta(v) {
    const since = v.status.since ? ` ${age(v.status.since)}` : "";
    return `<span class="smeta"><span>${esc(v.cde ?? "unknown CDE")}</span>${v.repo ? `${DOT}<span class="repo mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""}${DOT}<span title="${esc(statusTip(v))}"><span class="${v.status.lamp}">${esc(v.status.state ?? "unknown")}</span>${since}</span></span>`;
  }
  function sessionRow(v, toggled2, trayUrl) {
    if (isFolded(v, toggled2)) {
      const a = attentionLight(v);
      return `<section class="srow folded" data-toggle="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="sidx">${key(v, true)}</div>
      <div class="sline">${title(v, "h2")}<span class="attn" title="${esc(a.tip)}" aria-label="${esc(a.tip)}">${lamp(a)}</span></div>
    </section>`;
    }
    const activity = v.activity?.replace(/\s+/g, " ");
    const info = `<div class="sess">${title(v, "h2")}<div class="sctx">
      ${activity ? `<span class="activity" title="${esc(v.activity)}">${esc(activity)}</span>` : ""}
      ${meta(v)}
    </div></div>`;
    const body = v.stacks.length || v.branches.length ? `<div class="scroll"><div role="table" aria-label="Pull requests and branches">
        <div class="grid ghead" role="row"><span></span><span></span><span></span><span role="columnheader">base</span><span role="columnheader">CI</span><span role="columnheader">feedback</span><span role="columnheader">review</span></div>
        ${v.stacks.map((stack) => `<div class="sgroup" role="rowgroup">${stack.map((p, j) => prRow(p, j, stack.length, p.url === trayUrl)).join("")}</div>`).join("")}
        ${v.branches.map(branchRow).join("")}
      </div></div>` : `<div class="noprs"><span class="ring" aria-hidden="true"></span><span>No pull requests yet</span></div>`;
    return `<section class="srow" aria-label="${esc(v.title)}"><div class="sidx">${key(v, false)}</div>
    ${info}<div class="sprs">${body}</div>
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
  function render(views, toggled2, notices, legend2, t) {
    const p = t && views.flatMap((v) => v.prs).find((x) => x.url === t.url);
    return `<div class="page${notices.length ? " noticed" : ""}">
    <header class="top"><div class="logo" aria-hidden="true"><i></i></div><div class="brand"><b>flow</b><span>agent sessions and pull requests</span></div></header>
    <div class="deck${p && t?.open ? " open" : ""}">
      <main class="panel">${views.length ? views.map((v) => sessionRow(v, toggled2, p ? p.url : null)).join("") : '<p class="empty">No Sessions.</p>'}</main>
      <div class="scrim" data-close aria-hidden="true"></div>
      <aside class="tray" role="dialog" aria-modal="false" aria-label="Pull request details"><div class="tscroll">${p && t ? tray(p, t) : ""}</div></aside>
    </div>
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
  var branches = [];
  var drawn = "";
  var lastGood = null;
  var failing = false;
  var toggled = /* @__PURE__ */ new Set();
  var legend = false;
  var tray2 = null;
  var root = document.getElementById("root");
  async function get(path) {
    const r = await fetch(path);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }
  function page() {
    const notices = [];
    if (failing) {
      notices.push(
        lastGood ? `flowd unreachable \xB7 showing data from ${lastGood.toLocaleTimeString()}` : "flowd unreachable"
      );
    }
    const stale = prDataAge(prs);
    if (stale != null) notices.push(`PR data ${stale} min old`);
    const views = sessionViews(sessions, prs, branches);
    if (tray2 && !views.some((v) => v.prs.some((p) => p.url === tray2.url))) tray2 = null;
    return render(views, toggled, notices, legend, tray2);
  }
  var $ = (sel) => root.querySelector(sel);
  function draw() {
    const html = page();
    if (html === drawn) return;
    const top = $(".tscroll")?.scrollTop ?? 0;
    root.innerHTML = drawn = html;
    $(".tscroll").scrollTop = top;
  }
  function morph(change, dom) {
    draw();
    change();
    dom();
    drawn = page();
  }
  function openTray(url, col) {
    tray2 = { url, open: false, focus: col, expanded: tray2?.expanded ?? /* @__PURE__ */ new Set() };
    draw();
    const deck = $(".deck");
    void deck.offsetWidth;
    morph(() => tray2.open = true, () => deck.classList.add("open"));
    $(".tscroll").scrollTop = col ? $(`[data-sec="${col}"]`).offsetTop : 0;
    $(".tclose").focus({ preventScroll: true });
    if (col) {
      setTimeout(() => {
        if (tray2?.focus === col) morph(() => tray2.focus = null, () => $(".tsec.focus")?.classList.remove("focus"));
      }, 1e3);
    }
  }
  function closeTray() {
    morph(() => tray2.open = false, () => $(".deck").classList.remove("open"));
  }
  async function poll() {
    try {
      [sessions, prs, branches] = await Promise.all([
        get("/api/sessions"),
        get("/api/prs"),
        get("/api/branches")
      ]);
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
    if (target.closest("[data-close]")) return closeTray();
    const thread = target.closest("[data-thread]");
    if (thread && tray2) {
      const url = thread.dataset.thread;
      if (!tray2.expanded.delete(url)) tray2.expanded.add(url);
      return draw();
    }
    const pr = target.closest("[data-open]");
    if (pr) return openTray(pr.dataset.open, pr.dataset.col ?? null);
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
    if (e.key === "Escape" && tray2?.open) return closeTray();
    if (e.key === "h" || e.key === "H") legend = !legend;
    else if (e.key === "Escape" && legend) legend = false;
    else return;
    draw();
  });
  poll();
  setInterval(poll, POLL_MS);
})();
