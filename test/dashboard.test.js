// Dashboard render-logic test harness (deno test).
//
// The dashboard pages are self-contained inline JS (CLAUDE.md: no external
// scripts), so there is nothing to `import`. Instead this harness EXTRACTS the
// real render functions from the shipped `site/*.html` and runs them against
// fixtures under a minimal DOM stub — so the tests track the deployed code
// without a browser or any third-party dependency.
//
// Run: `deno test --allow-read test/` (see .github/workflows/site-test.yml).

function assert(cond, msg) {
  if (!cond) throw new Error("assertion failed: " + msg);
}

// A DOM element stub covering only what the render functions touch: className, a
// separate classList, textContent (setting it clears children, like the DOM),
// dataset, append/appendChild, and querySelectorAll (returns [] — the click
// wiring runs over its result and is a no-op at render time).
function makeEl(tag) {
  const el = {
    tagName: String(tag).toLowerCase(),
    className: "",
    _text: undefined,
    children: [],
    style: {},
    dataset: {},
    classList: {
      _s: new Set(),
      add(c) {
        this._s.add(c);
      },
      contains(c) {
        return this._s.has(c);
      },
    },
    append(...ns) {
      for (const n of ns) el.children.push(n);
    },
    appendChild(n) {
      el.children.push(n);
      return n;
    },
    querySelectorAll() {
      return [];
    },
  };
  Object.defineProperty(el, "textContent", {
    get() {
      return el._text;
    },
    set(v) {
      el._text = v;
      el.children = [];
    },
  });
  return el;
}

// Pull a top-level `function NAME(...) { ... }` out of a page's inline script by
// column-0 brace matching (nested braces are indented, so the first `^}$` after
// the header is the function's own close).
function extractFn(script, name) {
  const re = new RegExp("^function " + name + "\\([^)]*\\)[\\s\\S]*?^\\}$", "m");
  const m = re.exec(script);
  if (!m) throw new Error("could not extract function " + name);
  return m[0];
}

function pageScript(file) {
  const html = Deno.readTextFileSync(new URL("../site/" + file, import.meta.url));
  return html
    .replace(/^[\s\S]*?<script type="module">/, "")
    .replace(/<\/script>[\s\S]*$/, "");
}

// All descendant elements whose className carries `cls`.
function collect(root, cls, out = []) {
  for (const c of root.children || []) {
    if (c && typeof c === "object") {
      if (typeof c.className === "string" && c.className.split(" ").includes(cls)) out.push(c);
      collect(c, cls, out);
    }
  }
  return out;
}

// Flattened text of a stub subtree (leaf textContent + interleaved string nodes).
function textOf(node) {
  let s = "";
  for (const c of node.children || []) {
    if (typeof c === "string") s += c;
    else if (c && typeof c === "object") s += (c._text !== undefined ? c._text : textOf(c));
  }
  return s;
}

// Instantiate one extracted render function bound to a stub document/$ (+ data).
function bind(file, name, params, values) {
  const src = extractFn(pageScript(file), name);
  return new Function(...params, src + "\nreturn " + name + ";")(...values);
}

function auditBox(data) {
  const box = makeEl("div");
  const document = { createElement: (t) => makeEl(t) };
  const $ = (id) => (id === "audit" ? box : makeEl("div"));
  bind("audit.html", "renderAudit", ["document", "$", "data"], [document, $, data])();
  return box;
}

function fsmBox(hq) {
  const box = makeEl("div");
  const document = { createElement: (t) => makeEl(t) };
  const $ = (id) => (id === "fsm" ? box : makeEl("div"));
  bind("pipeline.html", "renderFsm", ["document", "$"], [document, $])(hq);
  return box;
}

function auditRow(over) {
  return { name: "r", hasProtofireAudit: true, externalAudit: "stale", ...over };
}

function auditData(rows, neverN = 0) {
  return { org: "testorg", reposNeverExternallyAudited: neverN, protofireAudits: rows };
}

Deno.test("audit drift is red ONLY when the line drift is unenumerable", () => {
  const box = auditBox(auditData([
    auditRow({ name: "unenumerable", sourceDriftTruncated: true, filesChangedSinceAudit: 47, commitsSinceAudit: 687, compareUrl: "https://h/x/compare/a...b" }),
    auditRow({ name: "enumerated-large", sourceLocAddedSinceAudit: 9000, sourceLocRemovedSinceAudit: 8000, filesChangedSinceAudit: 200, commitsSinceAudit: 500, compareUrl: "https://h/x/compare/v...b" }),
    auditRow({ name: "nodrift", sourceDriftTruncated: true, filesChangedSinceAudit: 0, commitsSinceAudit: 5 }),
  ]));
  const drifts = collect(box, "au-drift");
  assert(drifts.length === 3, `expected 3 drift cells, got ${drifts.length}`);
  assert(drifts.filter((d) => d.classList.contains("big")).length === 1, "exactly one red cell");
  const unenum = drifts.filter((d) => textOf(d).includes("line drift too large to size"));
  assert(unenum.length === 1 && unenum[0].classList.contains("big"), "unenumerable cell is red");
  const enumerated = drifts.filter((d) => textOf(d).includes("src LOC"));
  assert(enumerated.length === 1 && !enumerated[0].classList.contains("big"), "enumerated diff is NOT red");
  const zero = drifts.filter((d) => textOf(d).includes("no Solidity drift"));
  assert(zero.length === 1 && !zero[0].classList.contains("big"), "zero drift is NOT red");
});

Deno.test("audit enumerated drift shows +added / -removed and file + commit counts", () => {
  const box = auditBox(auditData([
    auditRow({ sourceLocAddedSinceAudit: 328, sourceLocRemovedSinceAudit: 37, filesChangedSinceAudit: 10, commitsSinceAudit: 36 }),
  ]));
  const cell = collect(box, "au-drift")[0];
  const add = collect(cell, "add");
  const del = collect(cell, "del");
  assert(add.length === 1 && add[0].textContent === "+328", `add span = ${add[0] && add[0].textContent}`);
  assert(del.length === 1 && del[0].textContent === "−37", `del span = ${del[0] && del[0].textContent}`);
  assert(textOf(cell).includes("10 files"), "shows the changed-file count");
  assert(textOf(cell).includes("36 commits"), "shows the commit count");
});

Deno.test("audit up-to-date marker only when current AND zero drift", () => {
  const upToDate = auditBox(auditData([
    auditRow({ externalAudit: "current", sourceLocAddedSinceAudit: 0, sourceLocRemovedSinceAudit: 0, filesChangedSinceAudit: 0, commitsSinceAudit: 0 }),
  ]));
  assert(textOf(collect(upToDate, "au-drift")[0]).includes("up to date"), "current + 0 drift is up to date");
  const stale = auditBox(auditData([
    auditRow({ externalAudit: "stale", sourceLocAddedSinceAudit: 0, sourceLocRemovedSinceAudit: 0, filesChangedSinceAudit: 0, commitsSinceAudit: 1 }),
  ]));
  assert(!textOf(collect(stale, "au-drift")[0]).includes("up to date"), "stale is never up to date");
});

Deno.test("audit anchor flags: commit-anchored vs no-tag-in-name", () => {
  const box = auditBox(auditData([
    auditRow({ name: "commitrepo", anchorKind: "commit", sourceLocAddedSinceAudit: 1, sourceLocRemovedSinceAudit: 1, filesChangedSinceAudit: 1, commitsSinceAudit: 1 }),
    auditRow({ name: "unrepo", anchorKind: "unanchored", sourceLocAddedSinceAudit: 1, sourceLocRemovedSinceAudit: 1, filesChangedSinceAudit: 1, commitsSinceAudit: 1 }),
  ]));
  const flags = collect(box, "au-flag").map((f) => f.textContent);
  assert(flags.includes("commit-anchored"), `commit-anchored flag missing: ${JSON.stringify(flags)}`);
  assert(flags.includes("no tag in PDF name"), `no-tag flag missing: ${JSON.stringify(flags)}`);
});

Deno.test("audit shows only the referenced PDF, summarising older ones", () => {
  const box = auditBox(auditData([
    auditRow({
      referencePdfIndex: 1,
      auditPdfs: [
        { filename: "repo.v0.1.0-r1.jan-2026.pdf" },
        { filename: "repo.v0.1.1-r2.may-2026.pdf" },
      ],
      sourceLocAddedSinceAudit: 1,
      sourceLocRemovedSinceAudit: 1,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
  ]));
  const t = textOf(box);
  assert(t.includes("repo.v0.1.1-r2.may-2026.pdf"), "shows the referenced PDF");
  assert(!t.includes("repo.v0.1.0-r1.jan-2026.pdf"), "does NOT list the older PDF");
  assert(t.includes("+1 older"), "summarises the older PDF count");
});

Deno.test("audit never-audited: headline count + one enumerated row per uncovered repo", () => {
  const box = auditBox(auditData([
    auditRow({ name: "covered", sourceLocAddedSinceAudit: 1, sourceLocRemovedSinceAudit: 1, filesChangedSinceAudit: 1, commitsSinceAudit: 1 }),
    { name: "gap1", hasProtofireAudit: false },
    { name: "gap2", hasProtofireAudit: false },
    { name: "gap3", hasProtofireAudit: false },
  ], 3));
  const alarmNum = collect(box, "an")[0];
  assert(alarmNum && alarmNum.textContent === "3", `headline count = ${alarmNum && alarmNum.textContent}`);
  // Each uncovered repo is ENUMERATED as its own row with a "never" status badge,
  // not collapsed into a chip cloud (#48).
  const neverBadges = collect(box, "never");
  assert(neverBadges.length === 3, `expected 3 never rows, got ${neverBadges.length}`);
  assert(neverBadges.every((b) => b.textContent === "never"), "each gap row carries a never badge");
  const text = textOf(box);
  assert(["gap1", "gap2", "gap3"].every((n) => text.includes(n)), "each gap repo is named");
});

Deno.test("pipeline FSM: leak alarm shows the count; OK alarm when zero", () => {
  const box = fsmBox({
    counts: { leaks: 3, ready: 5, closeCandidateIssues: 0 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 5, prs: [] } } },
  });
  const bad = collect(box, "fsm-alarm").filter((a) => a.className.split(" ").includes("bad"));
  assert(bad.length === 1, "a leak alarm is rendered when leaks > 0");
  assert(collect(bad[0], "an")[0].textContent === "3", "leak alarm shows the leak count");
  assert(textOf(box).includes("not in any modeled state"), "leak alarm copy present");

  const clean = fsmBox({ counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 }, lanes: {} });
  const ok = collect(clean, "fsm-alarm").filter((a) => a.className.split(" ").includes("ok"));
  assert(ok.length === 1, "a zero-leak run shows the OK alarm");
  assert(textOf(clean).includes("fully conformant"), "conformant copy present");
});

Deno.test("pipeline FSM: unwired queue renders the not-wired-yet empty state", () => {
  assert(textOf(fsmBox(null)).includes("not wired yet"), "null human-queue → not-wired-yet copy");
});
