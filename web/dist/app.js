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
    const light2 = (lamp2, tip, count = null) => ({ lamp: lamp2, tip, count });
    const closed = done(g, "review");
    if (closed) return { ...closed, count: null };
    if (g.reviewDecision === "CHANGES_REQUESTED" || g.reviews.some((r) => r.state === "CHANGES_REQUESTED")) {
      return light2("act", "review: changes requested");
    }
    const names = g.reviewRequests.map((r) => "login" in r ? r.login : r.team);
    if (names.length) return light2("fine", `review needed: ${names.join(", ")}`, names.length);
    if (g.reviewDecision === "REVIEW_REQUIRED") return light2("act", "review: no reviewers requested");
    return light2("fine", g.reviewDecision === "APPROVED" ? "review: approved" : "review: none required");
  }
  var SQUARE = { "to do": "todo", "in progress": "progress", "in review": "review", done: "done" };
  function jiraView(j) {
    if (!j) return null;
    const { key: key2, url } = j;
    if (j.found == null) return { key: key2, url, state: "not fetched", square: "todo", status: null, tip: "not fetched yet" };
    if (!j.found) return { key: key2, url: null, state: "not found", square: null, status: null, tip: "not found in Jira" };
    const state = j.category === "done" ? "done" : j.category === "indeterminate" ? /review/i.test(j.status ?? "") ? "in review" : "in progress" : "to do";
    const status = j.status ?? state;
    return { key: key2, url, state, square: SQUARE[state], status, tip: `${key2} \xB7 ${status} \xB7 open in Jira` };
  }
  function prView(g, pos, below = null, now = Date.now(), jira2 = null) {
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
        label: `${q.position}/${q.size}`,
        text: `in merge queue \xB7 ${q.position} of ${q.size}`,
        at: q.size > 1 ? Math.min(Math.max((q.position - 1) / (q.size - 1), 0), 1) : 0
      },
      base,
      ci,
      feedback,
      review,
      attention: open && (ready || lamps.some((l) => l.lamp === "act")),
      tray: trayView(g, below, now),
      jira: jiraView(jira2)
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
      const on = (key2) => raw.filter((st) => base(st[0]) === key2).map(view);
      const own = loose.filter((b) => b.owner === s.id);
      const feature = new Set(raw.filter((st) => st.length === 1 && !named.has(base(st[0])) && on(head(st[0])).length).map((st) => head(st[0])));
      const fbranches = /* @__PURE__ */ new Set([...feature, ...own.map((b) => b.key)]);
      const groups = [
        ...raw.flatMap((st) => {
          if (fbranches.has(base(st[0]))) return [];
          if (feature.has(head(st[0]))) return [{ head: view(st)[0], stacks: on(head(st[0])) }];
          return [{ head: null, stacks: [view(st)] }];
        }),
        ...own.map((b) => ({ head: { name: b.branch, jira: jiraView(b.jira) }, stacks: on(b.key) }))
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
        prs: groups.flatMap((g) => [...g.head && !("name" in g.head) ? [g.head] : [], ...g.stacks.flat()])
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
    return (v.status.state === "done" || !v.prs.length) !== toggled2.has(v.id);
  }
  function stepPr(views, toggled2, url, dir) {
    const order = views.filter((v) => !isFolded(v, toggled2)).flatMap((v) => v.prs.map((p) => p.url));
    if (!order.length) return null;
    const i = url == null ? -1 : order.indexOf(url);
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
    const branches2 = v.groups.filter((g) => g.head && "name" in g.head).length;
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
  var lamp = (l) => `<span class="lamp ${l.lamp}" aria-hidden="true"></span>`;
  var light = (l) => "count" in l && l.count != null ? `<span class="count" aria-hidden="true">${l.count}</span>` : lamp(l);
  var COLUMNS = [
    ["base", "base"],
    ["ci", "CI"],
    ["feedback", "feedback"],
    ["review", "review"]
  ];
  var NODE_WORD = { merged: "merged", closed: "closed", queued: "in merge queue", ready: "mergeable" };
  var gauge = (q) => `<span class="gauge" style="--at:${q.at}" aria-hidden="true"><span class="ahead"></span><span class="mark"></span></span>`;
  var nodeClass = (p) => p.node === "open" && p.draft ? "open draft" : p.node;
  var nodeWord = (p) => p.node === "open" ? p.draft ? "draft" : "not yet mergeable" : NODE_WORD[p.node];
  var nodeCell = (j, n, node) => `<span class="ncell">${j > 0 ? '<span class="up"></span>' : ""}${j < n - 1 ? '<span class="down"></span>' : ""}${node}</span>`;
  var led = (p, col, label) => {
    const l = p[col];
    return `<button type="button" class="led" data-open="${esc(p.url)}" data-col="${col}" title="${esc(l.tip)}" aria-label="${esc(`${l.tip}. Show ${label} details for #${p.number}`)}">${light(l)}</button>`;
  };
  var square = (j) => j.square ? `<span class="jsq ${j.square}" aria-hidden="true"></span>` : "";
  function jira(j, cls, status = false) {
    const inner = `${square(j)}<span class="mono">${esc(j.key)}</span>${status && j.status ? `<span>${esc(j.status)}</span>` : ""}`;
    return j.url ? `<a class="${cls}" href="${esc(j.url)}" target="_blank" rel="noreferrer" title="${esc(j.tip)}" aria-label="${esc(`${j.key}: ${j.tip}`)}">${inner}</a>` : `<span class="${cls} missing" title="${esc(j.tip)}">${inner}</span>`;
  }
  var jiraCell = (j) => `<span class="jcell">${j ? jira(j, "jira") : ""}</span>`;
  function strip(p, j, n, selected, feature = false) {
    const word = feature ? `feature branch \xB7 ${nodeWord(p)}` : nodeWord(p);
    return `<div class="strip pr${selected ? " sel" : ""}">
    <span class="needle" aria-hidden="true"></span>
    <button type="button" class="pick" data-open="${esc(p.url)}" title="${esc(`#${p.number} ${p.title} \xB7 ${word}`)}" aria-label="${esc(`Tune in #${p.number}, ${p.title}, ${word}`)}" aria-pressed="${selected}">
      ${nodeCell(j, n, `<span class="node ${nodeClass(p)}">${feature ? "F" : p.pos ?? ""}</span>`)}
      <span class="tline"><span class="title ${p.node}">${esc(p.title)}</span>${p.queue ? `<span class="queue" title="${esc(p.queue.text)}">${gauge(p.queue)}<span class="mono">${p.queue.label}</span></span>` : ""}</span>
    </button>
    ${jiraCell(p.jira)}
    ${COLUMNS.map(([col, label]) => led(p, col, label)).join("")}
  </div>`;
  }
  var branchPlate = ({ name, jira: jira2 }, feature = false) => `<div class="plate branch" role="group" aria-label="${feature ? "Feature branch" : "Branch"} ${esc(name)}, no pull request yet">
    <div class="strip" title="${esc(name)} \xB7 ${feature ? "feature branch, " : ""}no pull request yet">
      <span class="pick">${nodeCell(0, feature ? 2 : 1, `<span class="node branch">${feature ? "F" : ""}</span>`)}<span class="title mono">${esc(name)}</span></span>
      ${jiraCell(jira2)}
      ${'<span class="led"></span>'.repeat(COLUMNS.length)}
    </div>
  </div>`;
  var plate = (stack, url) => `<div class="plate" role="group" aria-label="${stack.length === 1 ? "Pull request" : `Stack of ${stack.length} pull requests`}">${stack.map((p, j) => strip(p, j, stack.length, p.url === url)).join("")}</div>`;
  function group(g, url) {
    if (g.head == null) return plate(g.stacks[0], url);
    if (!g.stacks.length) return branchPlate(g.head);
    const n = g.stacks.length;
    const head = "name" in g.head ? branchPlate(g.head, true) : `<div class="plate" role="group" aria-label="Feature PR">${strip(g.head, 0, 2, g.head.url === url, true)}</div>`;
    const name = "name" in g.head ? g.head.name : `#${g.head.number}`;
    return `<div class="fgroup" role="group" aria-label="${esc(`Feature branch ${name} with ${n} ${n === 1 ? "stack" : "stacks"}`)}">${head}<div class="fstacks">${g.stacks.map((s) => `<div class="fstack">${plate(s, url)}</div>`).join("")}</div></div>`;
  }
  var ext = (url, cls, title, text) => `<a class="${cls}" href="${esc(url)}" target="_blank" rel="noreferrer" title="${esc(title)}">${esc(text)}</a>`;
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
  var CLOSE = `<button type="button" class="tclose" data-close aria-label="Close details"><svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg></button>`;
  function tray(p, t) {
    const word = nodeWord(p);
    const sections = COLUMNS.map(([col, label]) => {
      const s = p.tray[col];
      const l = p[col];
      return `<section class="tsec${t.focus === col ? " focus" : ""}" data-sec="${col}" aria-label="${label}">
      <span class="tlamp" title="${esc(l.tip)}">${light(l)}</span>
      <div class="thd"><h3>${label}</h3>${s.fine != null ? `<span class="${l.lamp === "act" ? "act" : ""}">${esc(s.fine)}</span>` : ""}</div>
      ${s.items ? `<div class="titems ${col}">${items(p, col, t.expanded)}</div>` : ""}
    </section>`;
    }).join("");
    return `<header class="whead">
      <div class="wline">
        <span class="node ${nodeClass(p)}" title="${esc(word)}" aria-hidden="true"></span>
        ${ext(p.url, "num mono", `Open #${p.number} on GitHub`, `#${p.number}`)}
        <span>${word}</span>
        <span class="wsize mono">${esc(p.sizeTip)}</span>
        ${CLOSE}
      </div>
      <h2>${esc(p.title)}</h2>
      ${p.jira ? jira(p.jira, "wjira", true) : ""}
      ${p.queue ? `<div class="wqueue">${gauge(p.queue)}<span>${esc(p.queue.text)}</span></div>` : ""}
    </header>${sections}`;
  }
  var EMPTY = `<header class="whead">
    <div class="wline"><span class="node none" aria-hidden="true"></span><span>no pull request tuned in</span></div>
    <span class="whint">Select a strip or a session title, or press \u2191 \u2193</span>
  </header>${COLUMNS.map(([, label]) => `<div class="tsec idle"><span class="tlamp"><span class="lamp unlit" aria-hidden="true"></span></span><span class="thd">${label}</span></div>`).join("")}`;
  var statusTip = (v) => v.status.since ? `${v.status.tip}, ${age(v.status.since)}` : v.status.tip;
  var key = (v, folded) => `<button type="button" class="skey${folded ? " up" : ""}" data-toggle="${esc(v.id)}" aria-expanded="${!folded}" title="${esc(statusTip(v))}" aria-label="${folded ? "Expand" : "Collapse"} session: ${esc(v.title)} (${esc(statusTip(v))})"><span>${lamp(v.status)}</span></button>`;
  var DOT = '<span class="dot" aria-hidden="true">\xB7</span>';
  function meta(v) {
    const since = v.status.since ? ` ${age(v.status.since)}` : "";
    return `<span class="smeta"><span>${esc(v.cde ?? "unknown CDE")}</span>${v.repo ? `${DOT}<span class="repo mono" title="${esc(v.repo)}">${esc(v.repo)}</span>` : ""}${DOT}<span title="${esc(statusTip(v))}"><span class="${v.status.lamp}">${esc(v.status.state ?? "unknown")}</span>${since}</span></span>`;
  }
  var statusWord = (v) => `<span class="${v.status.lamp === "act" ? "act" : ""}">${esc(v.status.tip)}</span>${v.status.since ? ` ${esc(age(v.status.since))}` : ""}`;
  function panelHtml({ session: v, counts, task }) {
    const activity = v.activity?.replace(/\s+/g, " ");
    const resources = task?.resources.length ? `<div class="pres">${task.resources.map((r) => {
      const cls = `rlabel${r.mono ? " mono" : ""}`;
      const label = r.link ? `<a class="${cls}" href="${esc(r.url)}" target="_blank" rel="noreferrer" title="${esc(r.url)}">${esc(r.label)}</a>` : `<span class="${cls}" title="${esc(r.url)}">${esc(r.label)}</span>`;
      return `<div class="rrow">${label}${r.host ? `<span class="rhost mono">${esc(r.host)}</span>` : ""}</div>`;
    }).join("")}</div>` : "";
    const others = task?.others.length ? `<div class="pothers"><span class="plabel">also on this task</span>${task.others.map((o) => `<button type="button" class="psess" data-goto="${esc(o.id)}" title="Show this session">
          ${lamp(o.status)}<span class="ptitle">${esc(o.title)}</span><span class="pstatus">${statusWord(o)}</span>
        </button>`).join("")}</div>` : "";
    return `<header class="whead">
      <div class="wline">
        <span class="wkey" title="${esc(statusTip(v))}">${lamp(v.status)}</span>
        <span>${statusWord(v)}</span>
        <span class="wsize">${esc(counts)}</span>
        ${CLOSE.replace("Close details", "Close session")}
      </div>
      <h2>${esc(v.title)}</h2>
      ${activity ? `<span class="wactivity" title="${esc(v.activity)}">${esc(activity)}</span>` : ""}
      <span class="wmeta">${esc(v.cde ?? "unknown CDE")}${v.repo ? `${DOT}<span class="mono">${esc(v.repo)}</span>` : ""}</span>
    </header>
    <section class="psec" aria-label="task">
      <div class="thd"><h3>task</h3><span>${task ? esc(task.counts) : "not part of a task"}</span></div>
      ${resources}${others}
    </section>`;
  }
  function channel(v, toggled2, url, open) {
    const h2 = `<h2><button type="button" class="stitle" data-session="${esc(v.id)}" aria-pressed="${open}" aria-label="${esc(`${open ? "Close" : "Show"} session details: ${v.title}`)}" title="${esc(v.title)}">${esc(v.title)}</button></h2>`;
    const cls = `chan${open ? " open" : ""}`;
    const needle = '<span class="needle" aria-hidden="true"></span>';
    if (isFolded(v, toggled2)) {
      const a = attentionLight(v);
      return `<section class="${cls} folded" data-chan="${esc(v.id)}" data-toggle="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="chead">${needle}${key(v, true)}<div class="ctext"><div class="cline">${h2}<span class="attn" title="${esc(a.tip)}" aria-label="${esc(a.tip)}">${lamp(a)}</span></div></div></div>
    </section>`;
    }
    const activity = v.activity?.replace(/\s+/g, " ");
    const body = v.groups.length ? `<div class="plates">${v.groups.map((g) => group(g, url)).join("")}</div>` : '<div class="noprs">no pull requests yet</div>';
    return `<section class="${cls}" data-chan="${esc(v.id)}" aria-label="${esc(v.title)}">
      <div class="chead">${needle}${key(v, false)}<div class="ctext">
        <div class="cline">${h2}</div>
        ${activity ? `<span class="activity" title="${esc(v.activity)}">${esc(activity)}</span>` : ""}
        ${meta(v)}
      </div></div>
      ${body}
    </section>`;
  }
  var WORDMARK = `<svg class="brand" width="45.45" height="24" viewBox="0 0 60.6 32" role="img" aria-label="flow"><defs><clipPath id="wx0"><rect x="0" y="12.2" width="60.6" height="15.0"/></clipPath></defs><g fill="none" stroke="#2A2927" stroke-width="3.2" stroke-linecap="butt" stroke-linejoin="miter" stroke-miterlimit="10"><path d="M8.6 27.2 V11.4 C8.6 8.1 10.5 6.3 13.7 6.3 H16.2"/><path d="M20.4 4.7 V27.2"/><ellipse cx="31.1" cy="19.75" rx="5.45" ry="6.05"/><path clip-path="url(#wx0)" d="M39.4 8 L44.3 29 L49 10 L53.7 29 L58.6 8"/></g><rect x="3.8" y="12.2" width="10.4" height="3.2" fill="#BF1B1B"/></svg>`;
  var LEGEND = `<div class="legend" role="note" aria-label="Legend">
    <div><span class="lh">lights</span>
      <span class="li act"><span class="lamp act"></span>needs action</span>
      <span class="li"><span class="lamp run still"></span>in progress</span>
      <span class="li"><span class="lamp fine"></span>fine</span>
      <span class="li"><span class="count">2</span>pending reviews</span>
    </div>
    <div><span class="lh">pull requests</span>
      <span class="li"><span class="node key open"></span>open</span>
      <span class="li"><span class="node key ready"></span>mergeable</span>
      <span class="li"><span class="node key queued"></span>in merge queue</span>
      <span class="li"><span class="node key merged"></span>merged</span>
      <span class="li"><span class="node key branch"></span>draft, or branch with no PR</span>
      <span class="li"><span class="node key open">F</span>feature branch</span>
    </div>
    <div><span class="lh">jira status</span>
      <span class="li"><span class="jsq todo"></span>to do</span>
      <span class="li"><span class="jsq progress"></span>in progress</span>
      <span class="li"><span class="jsq review"></span>in review</span>
      <span class="li"><span class="jsq done"></span>done</span>
    </div>
  </div>`;
  function render(views, toggled2, notices, live, legend2, t, panel2 = null) {
    const p = !panel2 && t && views.flatMap((v) => v.prs).find((x) => x.url === t.url);
    const open = panel2?.session.id ?? null;
    return `<div class="face">
    <header class="top">
      ${WORDMARK}<span class="tagline">agent sessions and pull requests</span>
      <div class="controls">
        ${notices.length ? `<div class="notices" role="status">${notices.map((n) => `<span>${esc(n)}</span>`).join("")}</div>` : ""}
        <span class="hint" aria-hidden="true"><kbd>\u2191</kbd><kbd>\u2193</kbd><span>tune</span></span>
        <button type="button" class="hint" data-legend aria-expanded="${legend2}" aria-keyshortcuts="h"><kbd class="${legend2 ? "down" : ""}">h</kbd><span>legend</span></button>
        <span class="power${live ? " on" : ""}" title="${live ? "flowd reachable" : "flowd unreachable"}" aria-hidden="true"></span>
      </div>
    </header>
    <div class="deck">
      <main class="rack">
        <div class="scale" aria-hidden="true"><span></span><span></span><span></span>${COLUMNS.map(([, label]) => `<span>${label}</span>`).join("")}</div>
        <div class="chans">${views.length ? views.map((v) => channel(v, toggled2, p ? p.url : null, v.id === open)).join("") : '<p class="empty">No Sessions.</p>'}</div>
      </main>
      <aside class="window"><div class="wbody"><div class="tscroll" role="region" aria-label="${panel2 ? "Session details" : "Pull request details"}">${panel2 ? panelHtml(panel2) : p && t ? tray(p, t) : EMPTY}</div></div></aside>
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
  var tray2 = null;
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
    if (tray2 && !views.some((v) => v.prs.some((p) => p.url === tray2.url))) tray2 = null;
    const pv = panel ? sessionPanel(panel, views, tasks) : null;
    if (!pv) panel = null;
    return render(views, toggled, notices, !failing, legend, tray2, pv);
  }
  var $ = (sel) => root.querySelector(sel);
  function draw() {
    const html = page();
    if (html === drawn) return;
    const top = $(".tscroll")?.scrollTop ?? 0;
    root.innerHTML = drawn = html;
    $(".tscroll").scrollTop = top;
  }
  function openTray(url, col) {
    panel = null;
    tray2 = { url, focus: col, expanded: tray2?.expanded ?? /* @__PURE__ */ new Set() };
    draw();
    $(".tscroll").scrollTop = col ? $(`[data-sec="${col}"]`).offsetTop : 0;
    $(".tclose").focus({ preventScroll: true });
    if (col) {
      setTimeout(() => {
        if (tray2?.focus !== col) return;
        draw();
        tray2.focus = null;
        $(".tsec.focus")?.classList.remove("focus");
        drawn = page();
      }, 1e3);
    }
  }
  function closeTray() {
    tray2 = null;
    panel = null;
    draw();
  }
  function openPanel(id) {
    tray2 = null;
    panel = id;
    draw();
    $(".tscroll").scrollTop = 0;
    $(".tclose").focus({ preventScroll: true });
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
    if (thread && tray2) {
      const url = thread.dataset.thread;
      if (!tray2.expanded.delete(url)) tray2.expanded.add(url);
      return draw();
    }
    const title = target.closest("[data-session]");
    if (title) {
      const id2 = title.dataset.session;
      return panel === id2 ? closeTray() : openPanel(id2);
    }
    const goto = target.closest("[data-goto]");
    if (goto) {
      const id2 = goto.dataset.goto;
      const v = sessionViews(sessions, prs, branches).find((x) => x.id === id2);
      if (v && isFolded(v, toggled) && !toggled.delete(id2)) toggled.add(id2);
      openPanel(id2);
      $(`[data-chan="${id2}"]`)?.scrollIntoView({ block: "nearest" });
      return;
    }
    const pr = target.closest("[data-open]");
    if (pr) {
      const col = pr.dataset.col ?? null;
      if (!col && tray2?.url === pr.dataset.open) return closeTray();
      return openTray(pr.dataset.open, col);
    }
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
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const url = stepPr(sessionViews(sessions, prs, branches), toggled, tray2?.url ?? null, e.key === "ArrowDown" ? 1 : -1);
      if (!url) return;
      e.preventDefault();
      openTray(url, null);
      $(".strip.sel")?.scrollIntoView({ block: "nearest" });
      return;
    }
    if (e.repeat) return;
    if (e.key === "Escape" && (tray2 || panel)) return closeTray();
    if (e.key === "h" || e.key === "H") legend = !legend;
    else if (e.key === "Escape" && legend) legend = false;
    else return;
    draw();
  });
  poll();
  setInterval(poll, POLL_MS);
})();
