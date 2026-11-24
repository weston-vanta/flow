(() => {
  // src/model.ts
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
    const light = (lamp, tip, count = null) => ({ lamp, tip, count });
    const closed = done(g, "review");
    if (closed) return { ...closed, count: null };
    if (g.reviewDecision === "CHANGES_REQUESTED" || g.reviews.some((r) => r.state === "CHANGES_REQUESTED")) {
      return light("act", "review: changes requested");
    }
    const names = g.reviewRequests.map((r) => "login" in r ? r.login : r.team);
    if (names.length) return light("fine", `review needed: ${names.join(", ")}`, names.length);
    if (g.reviewDecision === "REVIEW_REQUIRED") return light("act", "review: no reviewers requested");
    return light("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
  }
  var SQUARE = { "to do": "todo", "in progress": "progress", "in review": "review", done: "done" };
  function jiraView(j) {
    if (!j) return null;
    const { key, url } = j;
    if (j.found == null) return { key, url, state: "not fetched", square: "todo", status: null, tip: "not fetched yet" };
    if (!j.found) return { key, url: null, state: "not found", square: null, status: null, tip: "not found in Jira" };
    const state = j.category === "done" ? "done" : j.category === "indeterminate" ? /review/i.test(j.status ?? "") ? "in review" : "in progress" : "to do";
    const status = j.status ?? state;
    return { key, url, state, square: SQUARE[state], status, tip: `${key} \xB7 ${status} \xB7 open in Jira` };
  }
  function prView(g, pos, below = null, now = Date.now(), jira = null) {
    const base = baseLight(g, below, now);
    const ci = ciLight(g);
    const feedback = feedbackLight(g);
    const review = reviewLight(g);
    const lamps = [base, ci, feedback, review];
    const open = g.state === "OPEN";
    const q = open ? g.mergeQueue : null;
    const ready = !q && open && !g.isDraft && review.count == null && lamps.every((l) => l.lamp === "fine");
    return {
      pos,
      url: g.url,
      number: g.number,
      title: g.title,
      sizeTip: `${g.changedFiles} file${g.changedFiles === 1 ? "" : "s"}, +${g.additions} \u2212${g.deletions}`,
      node: g.state === "MERGED" ? "merged" : g.state === "CLOSED" ? "closed" : q ? "queued" : ready ? "ready" : "open",
      draft: g.isDraft,
      queue: q && {
        text: `merge queue \xB7 ${q.position} of ${q.size}`,
        fill: q.size > 1 ? Math.min(Math.max((q.size - q.position) / (q.size - 1), 0), 1) : 1
      },
      base,
      ci,
      feedback,
      review,
      tray: trayView(g, below, now),
      jira: jiraView(jira)
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
      base: section(baseProblems(g, below, now), below == null ? `no conflict, up to date with ${g.baseRefName}` : "no conflict"),
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
    const jiras = new Map(prs2.flatMap((p) => p.github ? [[p.github.url, p.jira]] : []));
    const loose = branches2.filter((b) => !heads.has(b.key)).sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
    const done2 = (s) => s.status?.state === "done" ? 1 : 0;
    const started = (s) => s.origin ? Date.parse(s.origin.at) : -Infinity;
    const sorted = [...sessions2].sort((a, b) => done2(a) - done2(b) || started(b) - started(a));
    const named = /* @__PURE__ */ new Set([...heads, ...branches2.map((b) => b.key)]);
    const head = (g) => `${g.repo}:${g.headRefName}`;
    const base = (g) => `${g.repo}:${g.baseRefName}`;
    return sorted.map((s) => {
      const raw = stacksOf(s.id, prs2);
      const view = (stack) => stack.map((g, i) => {
        const below = stack.find((o) => o.headRefName === g.baseRefName)?.number ?? null;
        return prView(g, stack.length > 1 ? i + 1 : null, below, now, jiras.get(g.url) ?? null);
      });
      const on = (key) => raw.filter((st) => base(st[0]) === key).map(view);
      const own = loose.filter((b) => b.owner === s.id);
      const feature = new Set(raw.filter((st) => st.length === 1 && !named.has(base(st[0])) && on(head(st[0])).length).map((st) => head(st[0])));
      const fbranches = /* @__PURE__ */ new Set([...feature, ...own.map((b) => b.key)]);
      const groups = [
        ...raw.flatMap((st) => {
          if (fbranches.has(base(st[0]))) return [];
          if (feature.has(head(st[0]))) return [{ kind: "feature", head: view(st)[0], stacks: on(head(st[0])) }];
          return [{ kind: "stack", head: null, stacks: [view(st)] }];
        }),
        ...own.map((b) => ({ kind: "branch", head: { key: b.key, repo: b.repo, name: b.branch, jira: jiraView(b.jira) }, stacks: on(b.key) }))
      ];
      const state = s.status?.state ?? null;
      const cwd = s.origin?.cwd;
      const title = s.context?.title ?? null;
      return {
        id: s.id,
        status: { ...statusLight(state), state, since: s.status?.at ?? null },
        title: title ?? "New session",
        untitled: title == null,
        repo: raw[0]?.[0].repo ?? own[0]?.repo ?? (cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null),
        cde: s.origin?.cde ?? null,
        activity: activityLine(s),
        task: s.task,
        groups,
        prs: groups.flatMap((g) => [...g.kind === "feature" ? [g.head] : [], ...g.stacks.flat()])
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
  function isFolded(v, toggled2) {
    return (v.status.state === "done" || !v.prs.length) !== toggled2.has(v.id);
  }
  var stageOf = (p) => !p ? "open" : p.node === "ready" ? "ready" : p.node === "open" ? "open" : "merge";
  function lanes(v) {
    const rows = [];
    const edges = [];
    const pr = (p, feature, child) => rows.push({ id: p.url, stage: stageOf(p), feature, child, pr: p, branch: null }) - 1;
    const stack = (s, child) => s.forEach((p, j) => {
      const i = pr(p, false, child);
      if (j) edges.push({ from: i - 1, to: i, kind: "stack" });
    });
    for (const g of v.groups) {
      if (g.kind === "stack") {
        stack(g.stacks[0], false);
        continue;
      }
      const feature = g.stacks.length > 0;
      const head = g.kind === "feature" ? pr(g.head, feature, false) : rows.push({ id: g.head.key, stage: "open", feature, child: false, pr: null, branch: g.head }) - 1;
      for (const s of g.stacks) {
        edges.push({ from: head, to: rows.length, kind: "feat" });
        stack(s, true);
      }
    }
    return { rows, edges };
  }
  function stepPr(views, toggled2, id, dir) {
    const order = views.filter((v) => !isFolded(v, toggled2)).flatMap((v) => lanes(v).rows.map((r) => r.id));
    if (!order.length) return null;
    const i = id == null ? -1 : order.indexOf(id);
    if (i < 0) return order[dir > 0 ? 0 : order.length - 1];
    return order[(i + dir + order.length) % order.length];
  }
  function prDataAge(prs2, now = Date.now()) {
    const ats = prs2.flatMap((p) => p.github?.state === "OPEN" ? [Date.parse(p.github.at)] : []);
    if (!ats.length) return null;
    const ms = now - Math.min(...ats);
    return ms > 3 * 6e4 ? Math.floor(ms / 6e4) : null;
  }
  function jiraDataAge(prs2, branches2, now = Date.now()) {
    const ats = [...prs2, ...branches2].flatMap((x) => x.jira?.at ? [Date.parse(x.jira.at)] : []);
    if (!ats.length) return null;
    const ms = now - Math.min(...ats);
    return ms > 15 * 6e4 ? Math.floor(ms / 6e4) : null;
  }
  var plural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  var shortUrl = (url) => url.replace(/^[a-z]+:\/\//i, "").replace(/^www\./i, "").replace(/[?#].*$/, "").replace(/\/(edit)?$/, "");
  function resourceView(r) {
    const file = /^file:\/\//i.test(r.url);
    const short = file ? r.url.replace(/^file:\/\//i, "") : shortUrl(r.url);
    return {
      url: r.url,
      label: r.title ?? short,
      mono: r.title == null,
      host: file ? "" : short.split("/")[0],
      link: !file
    };
  }
  function sessionPanel(id, views, tasks2) {
    const v = views.find((w) => w.id === id);
    if (!v) return null;
    const branches2 = v.groups.filter((g) => g.kind === "branch").length;
    const parts = [v.prs.length && plural(v.prs.length, "pull request"), branches2 && plural(branches2, "branch", "branches")].filter(Boolean);
    const t = v.task == null ? null : tasks2.find((x) => x.slug === v.task);
    return {
      session: v,
      counts: parts.join(" \xB7 ") || "no pull requests",
      task: t ? {
        counts: plural(t.resources.length, "resource") + (t.sessions.length > 1 ? ` \xB7 ${t.sessions.length} sessions` : ""),
        resources: t.resources.map(resourceView),
        others: views.filter((w) => w.id !== v.id && t.sessions.includes(w.id))
      } : null
    };
  }

  // src/render.ts
  function esc(s) {
    return String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
  }
  var COLUMNS = [
    ["base", "base"],
    ["ci", "CI"],
    ["feedback", "feedback"],
    ["review", "review"]
  ];
  var STAGES = [
    ["open", "Open"],
    ["ready", "Ready"],
    ["merge", "Merge"]
  ];
  var sig = (l) => "count" in l && l.count != null ? "count" : l.lamp === "fine" ? "ok" : l.lamp === "off" ? "none" : l.lamp;
  var NODE_WORD = { merged: "merged", closed: "closed", queued: "in merge queue", ready: "mergeable" };
  var nodeWord = (p) => p.node === "open" ? p.draft ? "draft" : "not yet mergeable" : NODE_WORD[p.node];
  var badge = (p, text) => `<span class="bd ${p.node}${p.draft && p.node === "open" ? " draft" : ""}"${p.queue ? ` style="--fill:${p.queue.fill.toFixed(3)}"` : ""}>${esc(text)}</span>`;
  var PITCH = 36;
  var NODE = 28;
  var PAD = 12;
  var NARROW = 96;
  var COL = { open: 0, ready: 1, merge: 2 };
  var xl = (c) => c === 0 ? "0px" : `(100% - ${(3 - c) * NARROW}px)`;
  var indent = (r) => r.child ? COL[r.stage] === 0 ? 24 : 16 : 0;
  var top = (i) => PAD + i * PITCH;
  function edge(e, rows) {
    const a = rows[e.from];
    const b = rows[e.to];
    const [ca, cb, ia, ib] = [COL[a.stage], COL[b.stage], indent(a), indent(b)];
    const y = top(e.from) + NODE;
    const h = top(e.to) + NODE / 2 - y;
    const cls = `edge e-${e.kind}`;
    if (ca === cb && ia === ib) {
      return `<i class="${cls} e-v" style="left:calc(${xl(ca)} + ${20 + ia}px);top:${y}px;height:${top(e.to) - y}px"></i>`;
    }
    if (cb < ca) {
      return `<i class="${cls} e-l" style="left:calc(${xl(cb + 1)} - 6px);width:calc(${xl(ca)} - ${xl(cb + 1)} + ${26 + ia}px);top:${y}px;height:${h}px"></i>`;
    }
    return `<i class="${cls} e-r" style="left:calc(${xl(ca)} + ${20 + ia}px);width:calc(${xl(cb)} - ${xl(ca)} + ${ib - ia - 14}px);top:${y}px;height:${h}px"></i>`;
  }
  var dot = (p, col, label) => {
    const l = p[col];
    return `<button type="button" class="t-${sig(l)}" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}"></button>`;
  };
  function node(r, i, sel) {
    const c = COL[r.stage];
    const full = c === 0;
    const tuned = r.id === sel ? " tuned" : "";
    const style = `grid-row:${i + 1};grid-column:${c + 1};margin-left:${6 + indent(r)}px`;
    if (r.branch) {
      const b = r.branch;
      const tip2 = `${b.name} \xB7 ${r.feature ? "feature branch, " : ""}no pull request yet${b.jira ? ` \xB7 ${b.jira.key}` : ""}`;
      return `<div class="n7 local${r.feature ? " feat" : ""}${tuned}" style="${style}" title="${esc(tip2)}">
      <span class="bd">${r.feature ? "F" : ""}</span>
      <button type="button" class="tl" data-open="${esc(r.id)}" aria-label="${esc(`Select branch ${b.name}, no pull request yet`)}">${esc(b.name)}</button>
    </div>`;
    }
    const p = r.pr;
    const word = r.feature ? `feature branch \xB7 ${nodeWord(p)}` : nodeWord(p);
    const aria = `Select #${p.number}, ${p.title}, ${word}`;
    const tip = `#${p.number} ${p.title} \xB7 ${word}${p.queue ? ` \xB7 ${p.queue.text}` : ""}`;
    const st = p.node === "closed" ? "merged" : p.node;
    return `<div class="n7 st-${st}${r.feature ? " feat" : ""}${full ? "" : " compact"}${tuned}" style="${style}" title="${esc(tip)}">
      ${badge(p, r.feature ? "F" : String(p.pos ?? ""))}
      <button type="button" class="nm mono" data-open="${esc(p.url)}" aria-label="${esc(aria)}">${p.number}</button>${full ? `<button type="button" class="tl" data-open="${esc(p.url)}" tabindex="-1">${esc(p.title)}</button><span class="dots">${COLUMNS.map(([col, label]) => dot(p, col, label)).join("")}</span>` : ""}
    </div>`;
  }
  var ext = (url, cls, title, text) => `<a class="${cls}" href="${esc(url)}" target="_blank" rel="noreferrer" title="${esc(title)}">${esc(text)}</a>`;
  function items(p, col, expanded) {
    const t = p.tray;
    if (col === "base") return t.base.items.map((s) => `<div class="p-item">${esc(s)}</div>`).join("");
    if (col === "ci") {
      return t.ci.items.map((c) => `<div class="p-item">${c.url ? ext(c.url, "p-mono", "Open job", c.name) : `<span class="p-mono">${esc(c.name)}</span>`}</div>`).join("");
    }
    if (col === "feedback") {
      return t.feedback.items.map((th) => {
        const open = expanded.has(th.url);
        return `<article class="p-thread">
        <div class="p-th-h"><b>${esc(th.author)}</b>${ext(th.url, "p-mono p-host", `Open comment: ${th.where}`, th.where)}<span class="p-host p-ago" title="${esc(th.at)}">${esc(th.ago)}</span></div>
        <p class="p-body${th.long && !open ? " clamp" : ""}">${esc(th.body)}</p>
        ${th.long ? `<button type="button" class="p-more" data-thread="${esc(th.url)}" aria-expanded="${open}">${open ? "show less" : "show all"}</button>` : ""}
      </article>`;
      }).join("");
    }
    return t.review.items.map((r) => `<div class="p-item">
      <span><span class="${r.team ? "p-mono" : ""}">${esc(r.name)}</span>${r.codeOwner ? ' <span class="p-host" title="Requested as code owner">code owner</span>' : ""}${r.teams.length ? ` <span class="p-host p-mono" title="Reviewed for ${esc(r.teams.join(", "))}">${esc(r.teams.join(", "))}</span>` : ""}</span>
      <span class="p-rw${r.state === "changes requested" ? " p-rw-act" : ""}">${r.state}</span>
    </div>`).join("");
  }
  var CLOSE = (label) => `<button type="button" class="p-x" data-close aria-label="${label}"><svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>`;
  function jiraLine(j) {
    const text = `<span class="p-mono">${esc(j.key)}</span>${j.status ? ` \xB7 ${esc(j.status)}` : ` \xB7 ${esc(j.state)}`}`;
    return `<div class="p-line p-dim">${j.url ? `<a href="${esc(j.url)}" target="_blank" rel="noreferrer" title="${esc(j.tip)}">${text}</a>` : `<span title="${esc(j.tip)}">${text}</span>`}</div>`;
  }
  function prPanel(p, t) {
    const sections = COLUMNS.map(([col, label]) => {
      const s = p.tray[col];
      const l = p[col];
      const led = "count" in l && l.count != null ? String(l.count) : "";
      return `<section class="p-sec" data-sec="${col}" aria-label="${label}">
      <div class="p-sech"><span class="p-led t-${sig(l)}" title="${esc(l.tip)}">${led}</span><span class="p-key">${label}</span>${s.fine != null ? `<span class="p-fine${l.lamp === "act" ? " act" : ""}">${esc(s.fine)}</span>` : ""}</div>
      ${s.items ? `<div class="p-items">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
    }).join("");
    return `<div class="p-head">${ext(p.url, "p-mono p-num", `Open #${p.number} on GitHub`, `#${p.number}`)}${p.draft && p.node === "open" ? '<span class="p-draft">draft</span>' : ""}<span class="p-meta p-mono">${esc(p.sizeTip)}</span>${CLOSE("Close details")}</div>
    <h2 class="p-title">${esc(p.title)}</h2>
    ${p.jira ? jiraLine(p.jira) : ""}
    ${p.queue ? `<div class="p-line p-dim">${esc(p.queue.text)}</div>` : ""}
    ${sections}`;
  }
  var branchPanel = (b, feature) => `<div class="p-head"><span class="lb lb-l" aria-hidden="true">${feature ? "F" : ""}</span><span>${feature ? "feature branch" : "local branch"}</span><span class="p-meta p-mono">${esc(b.repo)}</span>${CLOSE("Close details")}</div>
    <h2 class="p-title p-mono-title">${esc(b.name)}</h2>
    ${b.jira ? jiraLine(b.jira) : ""}
    <div class="p-sub">no pull request yet</div>`;
  var EMPTY = `<div class="p-empty"><div class="p-empty-t">Nothing selected</div><div class="p-empty-s">Select a PR or a session, or press \u2191 \u2193</div></div>`;
  var statusFor = (v) => `<span class="st">${esc(v.status.state ?? "unknown")}</span>${v.status.since ? ` ${esc(age(v.status.since))}` : ""}`;
  var statusTip = (v) => v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip;
  function sessionPanel2({ session: v, counts, task }) {
    const activity = v.activity?.replace(/\s+/g, " ");
    const resources = task ? task.resources.map((r) => {
      const cls = r.mono ? "p-mono" : "";
      const label = r.link ? `<a class="${cls}" href="${esc(r.url)}" target="_blank" rel="noreferrer" title="${esc(r.url)}">${esc(r.label)}</a>` : `<span class="${cls}" title="${esc(r.url)}">${esc(r.label)}</span>`;
      return `<div class="p-item">${label}${r.host ? `<span class="p-host">${esc(r.host)}</span>` : ""}</div>`;
    }).join("") : "";
    const others = task?.others.length ? `<div class="p-sub">also on this task</div>${task.others.map((o) => `<button type="button" class="p-item p-btn w-${o.status.state ?? "none"}" data-goto="${esc(o.id)}" title="Show this session"><span>${esc(o.title)}</span><span class="p-host">${statusFor(o)}</span></button>`).join("")}` : "";
    return `<div class="p-head w-${v.status.state ?? "none"}"><span title="${esc(statusTip(v))}">${statusFor(v)}</span><span class="p-meta">${esc(counts)}</span>${CLOSE("Close session")}</div>
    <h2 class="p-title">${esc(v.title)}</h2>
    ${activity ? `<div class="p-line p-activity" title="${esc(v.activity)}">${esc(activity)}</div>` : ""}
    <div class="p-line p-dim">${esc(v.cde ?? "unknown CDE")}${v.repo ? ` \xB7 <span class="p-mono">${esc(v.repo)}</span>` : ""}</div>
    <section class="p-sec" aria-label="task">
      <div class="p-sech"><span class="p-key">task</span>${task ? "" : '<span class="p-fine">not part of a task</span>'}</div>
      ${task ? `<div class="p-list">${resources}${others}</div>` : ""}
    </section>`;
  }
  var rail = (v, folded) => `<button type="button" class="trail" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"></button>`;
  function lane(v, toggled2, sel, open) {
    const folded = isFolded(v, toggled2);
    const { rows, edges } = lanes(v);
    const activity = v.activity?.replace(/\s+/g, " ");
    const head = `<div class="lh">
      <div class="lht"><button type="button" class="tt${v.untitled ? " untitled" : ""}" data-session="${esc(v.id)}" aria-pressed="${open}" aria-label="${esc(`${open ? "Close" : "Show"} session details: ${v.title}`)}" title="${esc(v.title)}">${esc(v.title)}</button>${rail(v, folded)}</div>
      ${!folded && activity ? `<div class="lm" title="${esc(v.activity)}">${esc(activity)}</div>` : ""}
      <div class="lm2"><span title="${esc(statusTip(v))}">${statusFor(v)}</span><span>\xB7</span><span>${esc(v.cde ?? "unknown CDE")}</span>${v.repo ? `<span>\xB7</span><span class="mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""}</div>
    </div>`;
    const cls = `g lane w-${v.status.state ?? "none"}${folded ? " folded" : ""}${open ? " sel" : ""}`;
    let body;
    if (folded) {
      body = STAGES.map(([st], c) => {
        const n = rows.filter((r) => r.stage === st).length;
        return `<div class="fc" style="grid-column:${c + 2}">${n ? `<span class="cnt mono">${n}</span>` : ""}</div>`;
      }).join("");
    } else if (rows.length) {
      body = `<div class="area" style="grid-template-rows:repeat(${rows.length},${NODE}px);row-gap:${PITCH - NODE}px;padding:${PAD}px 0">${edges.map((e) => edge(e, rows)).join("")}${rows.map((r, i) => node(r, i, sel)).join("")}</div>`;
    } else {
      body = '<div class="area none">no pull requests yet</div>';
    }
    return `<section class="${cls}" data-chan="${esc(v.id)}" aria-label="${esc(v.title)}">${head}${body}</section>`;
  }
  var WORDMARK = `<svg class="brand" width="45.45" height="24" viewBox="0 0 60.6 32" role="img" aria-label="flow"><defs><clipPath id="wx0"><rect x="0" y="12.2" width="60.6" height="15.0"/></clipPath></defs><g fill="none" stroke="#111110" stroke-width="3.2" stroke-linecap="butt" stroke-linejoin="miter" stroke-miterlimit="10"><path d="M8.6 27.2 V11.4 C8.6 8.1 10.5 6.3 13.7 6.3 H16.2"/><path d="M20.4 4.7 V27.2"/><ellipse cx="31.1" cy="19.75" rx="5.45" ry="6.05"/><path clip-path="url(#wx0)" d="M39.4 8 L44.3 29 L49 10 L53.7 29 L58.6 8"/></g><rect x="3.8" y="12.2" width="10.4" height="3.2" fill="#BF1B1B"/></svg>`;
  var LEGEND = `<div class="lg" role="note" aria-label="Legend">
    <div class="lg-col"><span class="lg-h">Sessions</span>
      <span><i class="lr lr-w"></i>waiting for you</span><span><i class="lr lr-r"></i>running</span><span><i class="lr"></i>idle</span><span><i class="lr lr-d"></i>done</span><span><i class="lr lr-f"></i>folded: click the rail</span>
    </div>
    <div class="lg-col"><span class="lg-h">Signals</span><span class="lg-sub">base \xB7 CI \xB7 feedback \xB7 review</span>
      <span><i class="ld t-act"></i>needs you</span><span><i class="ld t-run"></i>running</span><span><i class="ld t-count"></i>review requested</span><span><i class="ld t-ok"></i>fine</span>
    </div>
    <div class="lg-col"><span class="lg-h">Pull requests</span>
      <span><i class="lb"></i>open</span><span><i class="lb">2</i>position in a stack</span><span><i class="lb lb-f">F</i>feature branch</span><span><i class="lb lb-d"></i>draft</span><span><i class="lb lb-m"></i>mergeable</span><span><i class="lb lb-q"></i>in merge queue</span><span><i class="lb lb-x"></i>merged or closed</span><span><i class="lb lb-l"></i>local branch, no PR</span>
    </div>
    <div class="lg-col"><span class="lg-h">Lines</span>
      <span><i class="ll"></i>stacked on the PR above</span><span><i class="ll ll-f"></i>based on a feature branch</span>
      <span class="lg-h lg-gap">Columns</span><span class="lg-txt">Open includes local branches. Ready = mergeable. Merge = queued, merged or closed.</span>
      <span class="lg-h lg-gap">Keys</span><span><kbd>\u2191</kbd><kbd>\u2193</kbd>step through the board</span><span><kbd>Esc</kbd>clear selection</span>
    </div>
  </div>`;
  function render(views, toggled2, notices, alarm, legend2, t, panel2 = null) {
    const all = views.flatMap((v) => lanes(v).rows);
    const r = !panel2 && t ? all.find((x) => x.id === t.id) : void 0;
    const sel = r ? r.id : null;
    const open = panel2?.session.id ?? null;
    const body = panel2 ? sessionPanel2(panel2) : r && t ? r.pr ? prPanel(r.pr, t) : branchPanel(r.branch, r.feature) : EMPTY;
    const heads = STAGES.map(([st, name]) => `<div><span class="hn">${name}</span><span class="hc">${all.filter((x) => x.stage === st).length}</span></div>`).join("");
    return `<div class="app">
    <header class="bar">
      ${WORDMARK}<span class="sp"></span>
      ${notices.length ? `<div class="notices" role="status">${notices.map((n, i) => `<span${i === 0 && alarm ? ' class="alarm"' : ""}>${esc(n)}</span>`).join("")}</div>` : ""}
      <span class="kbd" aria-hidden="true"><kbd>\u2191</kbd><kbd>\u2193</kbd> select</span>
      <button type="button" class="kbd" data-legend aria-expanded="${legend2}" aria-keyshortcuts="h"><kbd>h</kbd> legend</button>
    </header>
    <div class="wrap">
      <main class="board">
        <div class="g heads"><div>Session</div>${heads}</div>
        ${views.length ? views.map((v) => lane(v, toggled2, sel, v.id === open)).join("") : '<p class="empty">No Sessions.</p>'}
      </main>
      <aside><div class="p" role="region" aria-label="${panel2 ? "Session details" : "Details"}">${body}</div></aside>
    </div>
  </div>
  ${legend2 ? LEGEND : ""}`;
  }

  // src/app.ts
  var POLL_MS = 3e3;
  var sessions = [];
  var prs = [];
  var branches = [];
  var tasks = [];
  var drawn = "";
  var lastGood = null;
  var failing = false;
  var toggled = /* @__PURE__ */ new Set();
  var legend = false;
  var tray = null;
  var panel = null;
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
    const jiraStale = jiraDataAge(prs, branches);
    if (jiraStale != null) notices.push(`Jira data ${jiraStale} min old`);
    const views = sessionViews(sessions, prs, branches);
    if (tray && !views.some((v) => lanes(v).rows.some((r) => r.id === tray.id))) tray = null;
    const pv = panel ? sessionPanel(panel, views, tasks) : null;
    if (!pv) panel = null;
    return render(views, toggled, notices, failing, legend, tray, pv);
  }
  var $ = (sel) => root.querySelector(sel);
  function draw() {
    const html = page();
    if (html === drawn) return;
    const board = $(".board")?.scrollTop ?? 0;
    const p = $(".p")?.scrollTop ?? 0;
    root.innerHTML = drawn = html;
    $(".board").scrollTop = board;
    $(".p").scrollTop = p;
  }
  function openTray(id, col) {
    panel = null;
    tray = { id, expanded: tray?.expanded ?? /* @__PURE__ */ new Set() };
    draw();
    const p = $(".p");
    p.scrollTop = col ? $(`[data-sec="${col}"]`).offsetTop - $(".p-head").offsetHeight : 0;
    $(".p-x").focus({ preventScroll: true });
  }
  var still = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
  function toggle(id) {
    const sel = `[data-chan="${CSS.escape(id)}"]`;
    const h0 = $(sel)?.offsetHeight;
    if (!toggled.delete(id)) toggled.add(id);
    draw();
    const el = $(sel);
    if (!el || h0 == null || still()) return;
    const T = { duration: 220, easing: "cubic-bezier(.2,.8,.2,1)" };
    const folded = el.classList.contains("folded");
    el.style.overflow = "hidden";
    el.animate([{ height: `${h0}px` }, { height: `${el.offsetHeight}px` }], T).onfinish = () => el.style.overflow = "";
    const leg = [
      { height: "33px", borderLeftWidth: "3px", borderTopLeftRadius: "7px", borderBottomLeftRadius: "0px" },
      { height: "3px", borderLeftWidth: "0px", borderTopLeftRadius: "1.5px", borderBottomLeftRadius: "1.5px" }
    ];
    el.querySelector(".trail")?.animate(folded ? leg : leg.reverse(), { ...T, pseudoElement: "::before" });
  }
  function closeTray() {
    tray = null;
    panel = null;
    draw();
  }
  function openPanel(id) {
    tray = null;
    panel = id;
    draw();
    $(".p").scrollTop = 0;
    $(".p-x").focus({ preventScroll: true });
  }
  async function poll() {
    try {
      [sessions, prs, branches, tasks] = await Promise.all([
        get("/api/sessions"),
        get("/api/prs"),
        get("/api/branches"),
        get("/api/tasks")
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
    if (thread && tray) {
      const url = thread.dataset.thread;
      if (!tray.expanded.delete(url)) tray.expanded.add(url);
      return draw();
    }
    const title = target.closest("[data-session]");
    if (title) {
      const id = title.dataset.session;
      return panel === id ? closeTray() : openPanel(id);
    }
    const goto = target.closest("[data-goto]");
    if (goto) {
      const id = goto.dataset.goto;
      const v = sessionViews(sessions, prs, branches).find((x) => x.id === id);
      if (v && isFolded(v, toggled) && !toggled.delete(id)) toggled.add(id);
      openPanel(id);
      $(`[data-chan="${id}"]`)?.scrollIntoView({ block: "nearest" });
      return;
    }
    const pr = target.closest("[data-open]");
    if (pr) {
      const col = pr.dataset.col ?? null;
      if (!col && tray?.id === pr.dataset.open) return closeTray();
      return openTray(pr.dataset.open, col);
    }
    if (target.closest("[data-legend]")) {
      legend = !legend;
      return draw();
    }
    const el = target.closest("[data-toggle]");
    if (!el) return;
    toggle(el.dataset.toggle);
  });
  document.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const id = stepPr(sessionViews(sessions, prs, branches), toggled, tray?.id ?? null, e.key === "ArrowDown" ? 1 : -1);
      if (!id) return;
      e.preventDefault();
      openTray(id, null);
      $(".n7.tuned")?.scrollIntoView({ block: "nearest" });
      return;
    }
    if (e.repeat) return;
    if (e.key === "Escape" && (tray || panel)) return closeTray();
    if (e.key === "h" || e.key === "H") legend = !legend;
    else if (e.key === "Escape" && legend) legend = false;
    else return;
    draw();
  });
  poll();
  setInterval(poll, POLL_MS);
})();
