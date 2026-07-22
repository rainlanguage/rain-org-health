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

// Selector match for the stub: supports "[data-t]", ".class", "#id", and a bare tag
// — the only forms the render code uses (querySelector / querySelectorAll).
function matchesSel(node, sel) {
  if (!node || typeof node !== "object") return false;
  if (sel === "[data-t]") return node.dataset != null && node.dataset.t != null;
  if (sel[0] === ".") {
    return typeof node.className === "string" &&
      node.className.split(" ").includes(sel.slice(1));
  }
  if (sel[0] === "#") return node.id === sel.slice(1);
  return node.tagName === String(sel).toLowerCase();
}

// A DOM element stub covering what the render functions touch: className, a separate
// classList (add/remove/contains), textContent (setting it clears children, like the
// DOM), dataset, and enough tree API to exercise the click path — parent tracking,
// document-fragment spread, append/appendChild/replaceChildren, querySelector(All),
// and addEventListener + a click() that fires stored handlers.
function makeEl(tag) {
  const el = {
    tagName: String(tag).toLowerCase(),
    className: "",
    _text: undefined,
    _isFragment: false,
    _ev: {},
    parent: null,
    children: [],
    style: {},
    dataset: {},
    classList: {
      _s: new Set(),
      add(c) {
        this._s.add(c);
      },
      remove(c) {
        this._s.delete(c);
      },
      contains(c) {
        return this._s.has(c);
      },
      // Two-arg form (`toggle(c, force)`) is what the render code uses to drive
      // a control's on/off state, so it must set-or-clear rather than flip.
      toggle(c, force) {
        const on = force === undefined ? !this._s.has(c) : !!force;
        if (on) this._s.add(c);
        else this._s.delete(c);
        return on;
      },
    },
    // Adopt one node, spreading a document fragment's children (like the real DOM).
    _adopt(n) {
      if (n && typeof n === "object" && n._isFragment) {
        for (const c of n.children) {
          el.children.push(c);
          if (c && typeof c === "object") c.parent = el;
        }
        n.children = [];
      } else {
        el.children.push(n);
        if (n && typeof n === "object") n.parent = el;
      }
    },
    append(...ns) {
      for (const n of ns) el._adopt(n);
    },
    appendChild(n) {
      el._adopt(n);
      return n;
    },
    replaceChildren(...ns) {
      el.children = [];
      for (const n of ns) el._adopt(n);
    },
    querySelectorAll(sel) {
      const out = [];
      const walk = (n) => {
        for (const c of n.children || []) {
          if (c && typeof c === "object") {
            if (matchesSel(c, sel)) out.push(c);
            walk(c);
          }
        }
      };
      walk(el);
      return out;
    },
    querySelector(sel) {
      return el.querySelectorAll(sel)[0] || null;
    },
    // Attributes the render code sets for accessibility (role, aria-pressed).
    // Stored rather than ignored so a test can assert the state a control
    // announces, not just the class it happens to carry.
    attrs: {},
    setAttribute(k, v) {
      el.attrs[k] = String(v);
    },
    getAttribute(k) {
      return Object.prototype.hasOwnProperty.call(el.attrs, k)
        ? el.attrs[k]
        : null;
    },
    addEventListener(type, fn) {
      (el._ev[type] = el._ev[type] || []).push(fn);
    },
    click() {
      (el._ev.click || []).forEach((fn) => fn());
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
  const re = new RegExp(
    "^function " + name + "\\([^)]*\\)[\\s\\S]*?^\\}$",
    "m",
  );
  const m = re.exec(script);
  if (!m) throw new Error("could not extract function " + name);
  return m[0];
}

function pageScript(file) {
  const html = Deno.readTextFileSync(
    new URL("../site/" + file, import.meta.url),
  );
  return html
    .replace(/^[\s\S]*?<script type="module">/, "")
    .replace(/<\/script>[\s\S]*$/, "");
}

// All descendant elements whose className carries `cls`.
function collect(root, cls, out = []) {
  for (const c of root.children || []) {
    if (c && typeof c === "object") {
      if (
        typeof c.className === "string" && c.className.split(" ").includes(cls)
      ) out.push(c);
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
    else if (c && typeof c === "object") {
      s += c._text !== undefined ? c._text : textOf(c);
    }
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
  bind("audit.html", "renderAudit", ["document", "$", "data"], [
    document,
    $,
    data,
  ])();
  return box;
}

function fsmBox(hq) {
  const box = makeEl("div");
  const document = {
    createElement: (t) => makeEl(t),
    createDocumentFragment: () => {
      const f = makeEl("#fragment");
      f._isFragment = true;
      return f;
    },
  };
  const $ = (id) => (id === "fsm" ? box : makeEl("div"));
  bind("pipeline.html", "renderFsm", ["document", "$"], [document, $])(hq);
  return box;
}

const standsOn = bind("audit.html", "standsOn", [], []);
const ground = (edges, root) => [...standsOn(edges, root)].sort().join(",");

Deno.test("graph trace: follows consumer->dependency edges forward, root included", () => {
  // c stands on b, b stands on a. So c stands on b and a; a stands on nothing.
  const edges = [
    { from: "b", to: "a" },
    { from: "c", to: "b" },
  ];
  assert(ground(edges, "c") === "a,b,c", "c transitively stands on b and a");
  assert(ground(edges, "b") === "a,b", "b stands on a");
  assert(ground(edges, "a") === "a", "a is foundation: it stands on nothing");
});

Deno.test("graph trace: a dependency cycle terminates", () => {
  const edges = [
    { from: "a", to: "b" },
    { from: "b", to: "a" },
  ];
  assert(
    ground(edges, "a") === "a,b",
    "a cycle resolves to both repos, not a hang",
  );
});

Deno.test("graph trace: consumers are NOT traced, only dependencies", () => {
  // b stands on a. Tracing a must not light b — b is above a, not beneath it.
  assert(
    ground([{ from: "b", to: "a" }], "a") === "a",
    "a does not stand on its consumer",
  );
});

// The node's name is the graph's handle on a repo, so it links there. The scan is
// multi-org, so the org is per-node and only falls back to the scan's own.
function graphNode(n, data) {
  const el = (tag, cls, text) => {
    const node = makeEl(tag);
    if (cls) node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  return bind("audit.html", "nodeEl", ["el", "data"], [el, data])(n, null);
}

// A node with its protofire record attached, so the drift figures render.
function graphNodeP(n, p, data) {
  const el = (tag, cls, text) => {
    const node = makeEl(tag);
    if (cls) node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  return bind("audit.html", "nodeEl", ["el", "data"], [el, data])(n, p);
}

Deno.test("graph node: leads with code drift, not the undifferentiated total", () => {
  // rain.math.binary: +8/-2 of NatSpec and no code change. The node must not
  // show a non-zero figure while the row beside it reads CURRENT.
  const box = graphNodeP({ repo: "rain.math.binary", audit: "current" }, {
    auditedRef: "c7ebb6cb99e1648d18594f888331bd584c6a911d",
    anchorKind: "commit",
    sourceLocAddedSinceAudit: 7,
    sourceLocRemovedSinceAudit: 1,
    codeLocAddedSinceAudit: 0,
    codeLocRemovedSinceAudit: 0,
    commentLocAddedSinceAudit: 8,
    commentLocRemovedSinceAudit: 2,
  }, { org: "rainlanguage" });
  const t = textOf(box);
  assert(t.includes("+0"), "code additions should read +0, got: " + t);
  assert(
    !t.includes("+7"),
    "the undifferentiated total must not be the headline figure: " + t,
  );
  assert(t.includes("cmt"), "comment churn should still be shown: " + t);
  assert(t.includes("8"), "comment additions should appear: " + t);
});

Deno.test("graph node: pre-split scan data falls back to the old total", () => {
  // No codeLoc* fields (older health.json): show the undifferentiated figure
  // rather than fabricating a code number or rendering nothing.
  const box = graphNodeP({ repo: "legacy", audit: "stale" }, {
    auditedRef: "v0.1.0",
    anchorKind: "tag",
    sourceLocAddedSinceAudit: 42,
    sourceLocRemovedSinceAudit: 3,
  }, { org: "rainlanguage" });
  const t = textOf(box);
  assert(t.includes("+42"), "expected the legacy total, got: " + t);
  assert(!t.includes("cmt"), "no comment figure without a split: " + t);
});

Deno.test("graph node: a code-only change shows no comment figure", () => {
  const box = graphNodeP({ repo: "codeonly", audit: "stale" }, {
    auditedRef: "v0.1.0",
    anchorKind: "tag",
    sourceLocAddedSinceAudit: 5,
    sourceLocRemovedSinceAudit: 5,
    codeLocAddedSinceAudit: 5,
    codeLocRemovedSinceAudit: 5,
    commentLocAddedSinceAudit: 0,
    commentLocRemovedSinceAudit: 0,
  }, { org: "rainlanguage" });
  const t = textOf(box);
  assert(t.includes("+5"), "expected code additions: " + t);
  assert(!t.includes("cmt"), "zero comment churn should not render: " + t);
});

Deno.test("graph node: the repo name links to the repo, using the node's own org", () => {
  const box = graphNode(
    { repo: "cyclo.sol", org: "cyclofinance", audit: "never" },
    { org: "rainlanguage" },
  );
  const [name] = collect(box, "gn-repo");
  assert(name.tagName === "a", "the name is an anchor, not inert text");
  assert(
    name.href === "https://github.com/cyclofinance/cyclo.sol",
    "links the node's OWN org, not the scan's: " + name.href,
  );
  assert(textOf(box).includes("cyclo.sol"), "still reads as the repo name");
  // Leaving the dashboard would cost the reader their pan/zoom and trace, so an
  // external link opens in a new tab — and _blank without noopener hands the
  // opened page a live handle on window.opener.
  assert(name.target === "_blank", "external links open in a new tab");
  assert(
    String(name.rel).includes("noopener"),
    "a _blank link must carry noopener: " + name.rel,
  );
});

Deno.test("graph node: the repo name falls back to the scan's org", () => {
  const box = graphNode({ repo: "rainlang", audit: "never" }, {
    org: "rainlanguage",
  });
  const [name] = collect(box, "gn-repo");
  assert(
    name.href === "https://github.com/rainlanguage/rainlang",
    "a node with no org of its own uses the scan's: " + name.href,
  );
});

Deno.test("graph node: with no org resolvable the name stays plain text", () => {
  // Never a dead link — the same rule the audit anchor follows when no compareUrl
  // resolves. A link to nowhere is worse than no link.
  const box = graphNode({ repo: "orphan", audit: "never" }, {});
  const [name] = collect(box, "gn-repo");
  assert(
    name.tagName === "span",
    "no org anywhere -> plain span, not a broken href",
  );
  assert(name.href === undefined, "and carries no href at all");
});

Deno.test("graph node: shows the audit skill's last run and open-findings backlog", () => {
  const box = graphNode({
    repo: "rain.math.binary",
    org: "rainlanguage",
    audit: "current",
    depsKnown: true,
    staleDeps: [],
    lastAudit: {
      auditedAt: "2026-07-17T16:34:10Z",
      skillVersion: "0.14.0",
      stale: true,
    },
    openAuditIssues: 7,
  }, null);
  const t = textOf(box);
  assert(
    t.includes("skill 2026-07-17"),
    "shows the last audit-skill run date: " + t,
  );
  assert(t.includes("stale"), "flags a stamp whose source has since changed");
  assert(t.includes("7 open"), "shows the open audit-issue backlog: " + t);
});

Deno.test("graph node: a never-run skill says so, and an unknown backlog is not a zero", () => {
  // openAuditIssues absent = that org's issue search failed. Rendering "0 open"
  // would claim a clean backlog the scan never saw.
  const box = graphNode({
    repo: "unscanned",
    org: "o",
    audit: "never",
    depsKnown: true,
    staleDeps: [],
    lastAudit: null,
  }, null);
  const t = textOf(box);
  assert(
    t.includes("no audit-skill run"),
    "says the skill has never run: " + t,
  );
  assert(
    !t.includes("open"),
    "an absent count renders nothing, not zero: " + t,
  );
});

Deno.test("audit report: each row carries the audit skill's run + open findings", () => {
  const data = {
    ...auditData([
      auditRow({ name: "audited-repo" }),
      { name: "never-repo", hasProtofireAudit: false },
    ], 1),
    audits: [
      {
        name: "audited-repo",
        org: "testorg",
        openAuditIssues: 7,
        lastAudit: {
          auditedAt: "2026-07-17T16:34:10Z",
          skillVersion: "0.14.0",
          stale: true,
        },
      },
      {
        name: "never-repo",
        org: "testorg",
        lastAudit: null,
        openAuditIssues: 0,
      },
    ],
  };
  const t = textOf(auditBox(data));
  // The externally-audited row shows the SKILL's own run, version and backlog —
  // a separate signal from the protofire audit the row is built from.
  assert(
    t.includes("audit skill 2026-07-17"),
    "row shows the skill run date: " + t,
  );
  assert(t.includes("v0.14.0"), "row shows the skill version");
  assert(t.includes("7 open"), "row shows the open-findings backlog");
  // A repo the skill never ran on says so, on its row, rather than silently blank.
  assert(t.includes("audit skill: never run"), "never-run repo says so: " + t);
  assert(
    t.includes("0 open"),
    "a searched org with no findings shows a real zero",
  );
});

Deno.test("graph node: shows the newest adversarial-mutation run and its commit", () => {
  const box = graphNode({
    repo: "rain.math.binary",
    org: "rainlanguage",
    audit: "current",
    depsKnown: true,
    staleDeps: [],
    lastAudit: null,
    lastMutation: {
      timestamp: "2026-07-18T01:50:46Z",
      commit: "208336a29fc53b74226e385594f02703336974d5",
      skillVersion: "0.27.0",
      scope: "change-only",
    },
  }, null);
  const t = textOf(box);
  assert(
    t.includes("mutation 2026-07-18"),
    "shows the mutation run date: " + t,
  );
  assert(
    t.includes("@208336a"),
    "shows the commit it ran against, abbreviated: " + t,
  );
});

Deno.test("graph node: a repo with no mutation run says so rather than going blank", () => {
  const box = graphNode({
    repo: "unmutated",
    org: "o",
    audit: "never",
    depsKnown: true,
    staleDeps: [],
    lastAudit: null,
  }, null);
  assert(
    textOf(box).includes("no mutation run"),
    "says the mutation skill has never run: " + textOf(box),
  );
});

Deno.test("audit report: each row carries the newest mutation run + commit", () => {
  const data = {
    ...auditData([
      auditRow({ name: "mutated-repo" }),
      { name: "unmutated-repo", hasProtofireAudit: false },
    ], 1),
    audits: [
      {
        name: "mutated-repo",
        org: "testorg",
        lastAudit: null,
        openAuditIssues: 0,
        lastMutation: {
          timestamp: "2026-07-18T01:50:46Z",
          commit: "208336a29fc53b74226e385594f02703336974d5",
          skillVersion: "0.27.0",
          scope: "change-only",
        },
      },
      {
        name: "unmutated-repo",
        org: "testorg",
        lastAudit: null,
        openAuditIssues: 0,
      },
    ],
  };
  const t = textOf(auditBox(data));
  assert(
    t.includes("mutation 2026-07-18"),
    "row shows the mutation run date: " + t,
  );
  assert(t.includes("v0.27.0"), "row shows the mutation skill version");
  assert(t.includes("@208336a"), "row shows the commit it ran against");
  assert(
    t.includes("mutation test: never run"),
    "a repo with no mutation record says so: " + t,
  );
});

Deno.test("audit report: a repo's stale dependency pins render on its own row", () => {
  const data = {
    ...auditData([
      auditRow({
        name: "consumer",
        sourceLocAddedSinceAudit: 1,
        sourceLocRemovedSinceAudit: 0,
        filesChangedSinceAudit: 1,
        commitsSinceAudit: 1,
      }),
      { name: "clean", hasProtofireAudit: false },
    ], 1),
    auditGraph: {
      nodes: [
        {
          repo: "consumer",
          staleDeps: [
            { repo: "dep-a", pinned: "0.1.7", latest: "0.2.0" },
            { repo: "dep-b", pinned: "0.1.2", latest: "0.1.5" },
          ],
        },
        { repo: "clean", staleDeps: [] },
      ],
    },
  };
  const stale = collect(auditBox(data), "au-staledeps");
  // Exactly the one repo with stale pins gets a line — on its own row, not a summary.
  assert(stale.length === 1, `expected one stale row, got ${stale.length}`);
  const t = textOf(stale[0]);
  assert(t.includes("2 stale deps"), `count shown: ${t}`);
  assert(
    t.includes("dep-a 0.1.7→0.2.0") && t.includes("dep-b 0.1.2→0.1.5"),
    `both pins listed pinned->latest: ${t}`,
  );
});

Deno.test("audit report: a repo's link uses its own org, not the joined display org", () => {
  const data = {
    // data.org is the joined display string across orgs; a row must link via its
    // OWN org so cross-org repos get correct GitHub URLs.
    ...auditData([{
      name: "issuer-repo",
      hasProtofireAudit: false,
      org: "S01-Issuer",
    }], 1),
    org: "rainlanguage, S01-Issuer",
  };
  const link = auditBox(data).querySelectorAll("a").find((a) =>
    (a.href || "").includes("issuer-repo")
  );
  assert(
    link && link.href === "https://github.com/S01-Issuer/issuer-repo",
    `link should use the repo's own org: ${link && link.href}`,
  );
});

Deno.test("audit report: a repo with no stale deps gets no stale line", () => {
  const data = {
    ...auditData([{ name: "clean", hasProtofireAudit: false }], 1),
    auditGraph: { nodes: [{ repo: "clean", staleDeps: [] }] },
  };
  assert(
    collect(auditBox(data), "au-staledeps").length === 0,
    "no stale deps means no stale line on the row",
  );
});

Deno.test("audit row: an unknown verdict renders as unknown, never as current", () => {
  const box = auditBox(
    auditData([
      auditRow({ name: "unfetchable-repo", externalAudit: "unknown" }),
    ]),
  );
  const statuses = collect(box, "au-status");
  const text = statuses.map((s) => s._text).join(" ");
  assert(
    text.includes("unknown"),
    "the unknown verdict must be shown: " + text,
  );
  assert(
    !text.includes("current"),
    "a scan that established nothing must not render clean: " + text,
  );
  // The badge carries the state as a class, so it is styleable and not silently
  // indistinguishable from a confirmed verdict.
  assert(
    statuses.some((s) => s.className.split(" ").includes("unknown")),
    "unknown needs its own class for the dotted treatment",
  );
});

Deno.test("audit row: an unknown verdict is not counted as stale", () => {
  const box = auditBox(auditData([
    auditRow({ name: "a", externalAudit: "unknown" }),
    auditRow({ name: "b", externalAudit: "stale" }),
  ]));
  // Exactly one repo is confirmed stale; the indeterminate one must not pad
  // that count, or a broken scan would read as a worsening audit backlog.
  const staleBadges = collect(box, "au-status").filter((s) =>
    s.className.split(" ").includes("stale")
  );
  assert(
    staleBadges.length === 1,
    "expected 1 stale badge, got " + staleBadges.length,
  );
});

function auditRow(over) {
  return {
    name: "r",
    hasProtofireAudit: true,
    externalAudit: "stale",
    ...over,
  };
}

function auditData(rows, neverN = 0) {
  return {
    org: "testorg",
    reposNeverExternallyAudited: neverN,
    protofireAudits: rows,
  };
}

Deno.test("audit drift is red ONLY when the line drift is unenumerable", () => {
  const box = auditBox(auditData([
    auditRow({
      name: "unenumerable",
      sourceDriftTruncated: true,
      filesChangedSinceAudit: 47,
      commitsSinceAudit: 687,
      compareUrl: "https://h/x/compare/a...b",
    }),
    auditRow({
      name: "enumerated-large",
      sourceLocAddedSinceAudit: 9000,
      sourceLocRemovedSinceAudit: 8000,
      filesChangedSinceAudit: 200,
      commitsSinceAudit: 500,
      compareUrl: "https://h/x/compare/v...b",
    }),
    auditRow({
      name: "nodrift",
      sourceDriftTruncated: true,
      filesChangedSinceAudit: 0,
      commitsSinceAudit: 5,
    }),
  ]));
  const drifts = collect(box, "au-drift");
  assert(drifts.length === 3, `expected 3 drift cells, got ${drifts.length}`);
  assert(
    drifts.filter((d) => d.classList.contains("big")).length === 1,
    "exactly one red cell",
  );
  const unenum = drifts.filter((d) =>
    textOf(d).includes("line drift too large to size")
  );
  assert(
    unenum.length === 1 && unenum[0].classList.contains("big"),
    "unenumerable cell is red",
  );
  const enumerated = drifts.filter((d) => textOf(d).includes("src LOC"));
  assert(
    enumerated.length === 1 && !enumerated[0].classList.contains("big"),
    "enumerated diff is NOT red",
  );
  const zero = drifts.filter((d) => textOf(d).includes("no Solidity drift"));
  assert(
    zero.length === 1 && !zero[0].classList.contains("big"),
    "zero drift is NOT red",
  );
});

Deno.test("audit enumerated drift shows +added / -removed and file + commit counts", () => {
  const box = auditBox(auditData([
    auditRow({
      sourceLocAddedSinceAudit: 328,
      sourceLocRemovedSinceAudit: 37,
      filesChangedSinceAudit: 10,
      commitsSinceAudit: 36,
    }),
  ]));
  const cell = collect(box, "au-drift")[0];
  const add = collect(cell, "add");
  const del = collect(cell, "del");
  assert(
    add.length === 1 && add[0].textContent === "+328",
    `add span = ${add[0] && add[0].textContent}`,
  );
  assert(
    del.length === 1 && del[0].textContent === "−37",
    `del span = ${del[0] && del[0].textContent}`,
  );
  assert(textOf(cell).includes("10 files"), "shows the changed-file count");
  assert(textOf(cell).includes("36 commits"), "shows the commit count");
});

Deno.test("audit drift: comment LOC is counted apart and does not read as code drift", () => {
  // A NatSpec-only edit: zero code churn, real comment churn, still CURRENT.
  const box = auditBox(auditData([
    auditRow({
      name: "comments-only",
      externalAudit: "current",
      sourceLocAddedSinceAudit: 3,
      sourceLocRemovedSinceAudit: 1,
      codeLocAddedSinceAudit: 0,
      codeLocRemovedSinceAudit: 0,
      commentLocAddedSinceAudit: 3,
      commentLocRemovedSinceAudit: 1,
      driftFullyClassified: true,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
  ]));
  const t = textOf(box);
  assert(
    t.includes("+0 / −0 code LOC"),
    "headline diffstat is code-only: " + t,
  );
  assert(t.includes("+3 / −1 comment"), "comment churn shown separately: " + t);
  assert(
    t.includes("comments only"),
    "a comment-only drift is marked as such, not stale: " + t,
  );
  // and it is NOT presented as a stale row
  assert(collect(box, "stale").length === 0, "comment-only drift is not stale");
});

Deno.test("audit drift: code churn still shows as code LOC alongside comment churn", () => {
  const box = auditBox(auditData([
    auditRow({
      name: "mixed",
      externalAudit: "stale",
      sourceLocAddedSinceAudit: 5,
      sourceLocRemovedSinceAudit: 2,
      codeLocAddedSinceAudit: 4,
      codeLocRemovedSinceAudit: 1,
      commentLocAddedSinceAudit: 1,
      commentLocRemovedSinceAudit: 1,
      driftFullyClassified: true,
      filesChangedSinceAudit: 2,
      commitsSinceAudit: 3,
    }),
  ]));
  const t = textOf(box);
  assert(t.includes("+4 / −1 code LOC"), "code figure excludes comments: " + t);
  assert(t.includes("+1 / −1 comment"), "comment figure shown apart: " + t);
  assert(!t.includes("comments only"), "real code churn is not comments-only");
});

Deno.test("audit drift: an unclassifiable diff is disclosed, not passed off as classified", () => {
  const box = auditBox(auditData([
    auditRow({
      name: "toobig",
      externalAudit: "stale",
      sourceLocAddedSinceAudit: 40,
      sourceLocRemovedSinceAudit: 3,
      codeLocAddedSinceAudit: 40,
      codeLocRemovedSinceAudit: 3,
      commentLocAddedSinceAudit: 0,
      commentLocRemovedSinceAudit: 0,
      driftFullyClassified: false,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
  ]));
  const t = textOf(box);
  assert(
    t.includes("some diffs unclassified"),
    "says the split is incomplete: " + t,
  );
});

Deno.test("audit drift: without a split, the old undifferentiated src LOC is shown", () => {
  // Back-compat with health.json produced before the split existed.
  const box = auditBox(auditData([
    auditRow({
      name: "legacy",
      externalAudit: "stale",
      sourceLocAddedSinceAudit: 7,
      sourceLocRemovedSinceAudit: 2,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
  ]));
  const t = textOf(box);
  assert(
    t.includes("+7 / −2 src LOC"),
    "falls back to src LOC, not a fake code figure: " + t,
  );
  assert(
    !t.includes("comment"),
    "no comment figure invented when absent: " + t,
  );
});

Deno.test("audit up-to-date marker only when current AND zero drift", () => {
  const upToDate = auditBox(auditData([
    auditRow({
      externalAudit: "current",
      sourceLocAddedSinceAudit: 0,
      sourceLocRemovedSinceAudit: 0,
      filesChangedSinceAudit: 0,
      commitsSinceAudit: 0,
    }),
  ]));
  assert(
    textOf(collect(upToDate, "au-drift")[0]).includes("up to date"),
    "current + 0 drift is up to date",
  );
  const stale = auditBox(auditData([
    auditRow({
      externalAudit: "stale",
      sourceLocAddedSinceAudit: 0,
      sourceLocRemovedSinceAudit: 0,
      filesChangedSinceAudit: 0,
      commitsSinceAudit: 1,
    }),
  ]));
  assert(
    !textOf(collect(stale, "au-drift")[0]).includes("up to date"),
    "stale is never up to date",
  );
});

Deno.test("audit anchor flags: commit-anchored vs no-tag-in-name", () => {
  const box = auditBox(auditData([
    auditRow({
      name: "commitrepo",
      anchorKind: "commit",
      sourceLocAddedSinceAudit: 1,
      sourceLocRemovedSinceAudit: 1,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
    auditRow({
      name: "unrepo",
      anchorKind: "unanchored",
      sourceLocAddedSinceAudit: 1,
      sourceLocRemovedSinceAudit: 1,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
  ]));
  const flags = collect(box, "au-flag").map((f) => f.textContent);
  assert(
    flags.includes("commit-anchored"),
    `commit-anchored flag missing: ${JSON.stringify(flags)}`,
  );
  assert(
    flags.includes("no tag in PDF name"),
    `no-tag flag missing: ${JSON.stringify(flags)}`,
  );
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
  assert(
    !t.includes("repo.v0.1.0-r1.jan-2026.pdf"),
    "does NOT list the older PDF",
  );
  assert(t.includes("+1 older"), "summarises the older PDF count");
});

Deno.test("audit never-audited: headline count + one enumerated row per uncovered repo", () => {
  const box = auditBox(auditData([
    auditRow({
      name: "covered",
      sourceLocAddedSinceAudit: 1,
      sourceLocRemovedSinceAudit: 1,
      filesChangedSinceAudit: 1,
      commitsSinceAudit: 1,
    }),
    { name: "gap1", hasProtofireAudit: false },
    { name: "gap2", hasProtofireAudit: false },
    { name: "gap3", hasProtofireAudit: false },
  ], 3));
  const alarmNum = collect(box, "an")[0];
  assert(
    alarmNum && alarmNum.textContent === "3",
    `headline count = ${alarmNum && alarmNum.textContent}`,
  );
  // Each uncovered repo is ENUMERATED as its own row with a "never" status badge,
  // not collapsed into a chip cloud (#48).
  const neverBadges = collect(box, "never");
  assert(
    neverBadges.length === 3,
    `expected 3 never rows, got ${neverBadges.length}`,
  );
  assert(
    neverBadges.every((b) => b.textContent === "never"),
    "each gap row carries a never badge",
  );
  const text = textOf(box);
  assert(
    ["gap1", "gap2", "gap3"].every((n) => text.includes(n)),
    "each gap repo is named",
  );
});

Deno.test("pipeline FSM: leak alarm shows the count; OK alarm when zero", () => {
  const box = fsmBox({
    counts: { leaks: 3, ready: 5, closeCandidateIssues: 0 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 5, prs: [] } } },
  });
  const bad = collect(box, "fsm-alarm").filter((a) =>
    a.className.split(" ").includes("bad")
  );
  assert(bad.length === 1, "a leak alarm is rendered when leaks > 0");
  assert(
    collect(bad[0], "an")[0].textContent === "3",
    "leak alarm shows the leak count",
  );
  assert(
    textOf(box).includes("not in any modeled state"),
    "leak alarm copy present",
  );

  const clean = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {},
  });
  const ok = collect(clean, "fsm-alarm").filter((a) =>
    a.className.split(" ").includes("ok")
  );
  assert(ok.length === 1, "a zero-leak run shows the OK alarm");
  assert(textOf(clean).includes("fully conformant"), "conformant copy present");
});

Deno.test("pipeline FSM: unwired queue renders the not-wired-yet empty state", () => {
  assert(
    textOf(fsmBox(null)).includes("not wired yet"),
    "null human-queue → not-wired-yet copy",
  );
});

// #66: the close-candidate PR STATE and the close-candidate ISSUES group must not
// share the identical "ai:close-candidate" label (they read as one contradictory
// control otherwise).
Deno.test("pipeline FSM: the two close-candidate states carry distinct labels", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 18 },
    lanes: {},
  });
  const labels = collect(box, "sk").map((s) => s.textContent);
  assert(
    labels.includes("ai:close-candidate (PRs)"),
    `PR label missing: ${JSON.stringify(labels)}`,
  );
  assert(
    labels.includes("ai:close-candidate (issues)"),
    `issues label missing: ${JSON.stringify(labels)}`,
  );
  assert(
    !labels.includes("ai:close-candidate"),
    "the bare, ambiguous ai:close-candidate label must be gone",
  );
});

// #66 (primary): the shared detail panel stays at the bottom, but clicking a state
// opens it with a HEADER naming THAT state and a count == its own list length — so
// which state the list belongs to is unambiguous (not attached to the last lane).
Deno.test("pipeline FSM: clicking a state opens the bottom panel with a header naming it + matching count", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 2 },
    lanes: {
      "vetter-verdicts": {
        "ai:close-candidate": {
          count: 3,
          prs: [
            { repo: "o/a", number: 1, title: "pr one" },
            { repo: "o/b", number: 2, title: "pr two" },
            { repo: "o/c", number: 3, title: "pr three" },
          ],
        },
      },
    },
    closeCandidateIssues: [
      { repo: "o/x", number: 10, title: "issue ten" },
      { repo: "o/y", number: 11, title: "issue eleven" },
    ],
  });
  const boxByT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  const detail = box.querySelectorAll("#fsmdetail")[0];
  assert(detail, "detail panel exists");
  const detailHost = detail.parent; // the panel stays a child of this throughout

  // Click the close-candidate PRs box (3 PRs).
  const prBox = boxByT("ai:close-candidate");
  prBox.click();
  assert(detail.classList.contains("open"), "detail is open");
  assert(
    detail.parent === detailHost,
    "the panel stays put (not relocated per click)",
  );
  assert(
    collect(detail, "dhl")[0].textContent === "ai:close-candidate (PRs)",
    "header names the clicked state",
  );
  assert(
    collect(detail, "dhc")[0].textContent === "3 items",
    "header count == the PR list length",
  );
  assert(collect(detail, "li").length === 3, "renders exactly the 3 PRs");

  // Click the close-candidate ISSUES box (2 issues) — the SAME bottom panel re-populates
  // with the issues state's own header + count, so the two never blur together.
  boxByT("closeCandidateIssues").click();
  assert(detail.parent === detailHost, "still the same panel, still in place");
  assert(
    collect(detail, "dhl")[0].textContent === "ai:close-candidate (issues)",
    "header re-names to the issues state",
  );
  assert(
    collect(detail, "dhc")[0].textContent === "2 items",
    "header count == the issue list length",
  );
  assert(collect(detail, "li").length === 2, "renders exactly the 2 issues");
});

// ---- deployments.html: known owners ----

// renderDeployments takes (document, $, data) as its own params, so bind with no
// injected free vars and call the returned function with the stubs.
function deploymentsBox(data) {
  const box = makeEl("div");
  const document = {
    createElement: (t) => makeEl(t),
    createTextNode: (t) => t,
  };
  const $ = (id) => (id === "deployments" ? box : makeEl("div"));
  bind("deployments.html", "renderDeployments", [], [])(document, $, data);
  return box;
}

const OWNERS = {
  deploymentOwners: {
    repo: "st0x.deploy",
    org: "S01-Issuer",
    threshold: 3,
    signerCount: 6,
    groups: [
      {
        id: "safe",
        title: "Upgrade authority — token-owner Safe",
        note: "n",
        entries: [
          {
            role: "Base Safe",
            address: "0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
            network: "base",
            status: "active",
            note: "beacon owner",
          },
          {
            role: "Ethereum Safe",
            address: "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329",
            network: "ethereum",
            status: "active",
            note: "",
          },
        ],
      },
      {
        id: "signers",
        title: "Safe signers (3-of-6)",
        note: "",
        verification: {
          reachable: true,
          network: "base",
          safe: "0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
          rpcHost: "mainnet.base.org",
          onChainCount: 6,
          match: true,
          threshold: { declared: 3, onChain: 3, match: true },
        },
        entries: [1, 2, 3, 4, 5, 6].map((i) => ({
          role: "Signer " + i,
          address: "0x" + String(i).repeat(40).slice(0, 40),
          network: "",
          status: "active",
          note: "",
          onChain: "match",
        })),
      },
      {
        id: "authoriser",
        title: "Operational access — authoriser",
        note: "",
        entries: [
          {
            role: "V4 pending-swap clone",
            address: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
            network: "base",
            status: "pending",
            note: "swap",
          },
        ],
      },
      {
        id: "historical",
        title: "Historical & bricked",
        note: "",
        entries: [
          {
            role: "V2 receipt beacon owner",
            address: "0xbAB0E6b7B5dDA86FB8ba81c00aEA0Ceb8b73686b",
            network: "base",
            status: "bricked",
            note: "dead",
          },
        ],
      },
    ],
  },
};

Deno.test("deployments: renders every owner group, six signers, and per-entry status pills", () => {
  const box = deploymentsBox(OWNERS);
  assert(collect(box, "own-group").length === 4, "four owner groups");
  const signers = collect(box, "own-role").filter((r) =>
    (r.textContent || "").startsWith("Signer ")
  );
  assert(signers.length === 6, "six signer rows, got " + signers.length);
  assert(
    collect(box, "own-status-pending").length === 1,
    "one pending status pill",
  );
  assert(
    collect(box, "own-status-bricked").length === 1,
    "one bricked status pill",
  );
});

Deno.test("deployments: addresses link to the network's explorer (base default, ethereum→etherscan)", () => {
  const box = deploymentsBox(OWNERS);
  const addrs = collect(box, "own-addr");
  const baseSafe = addrs.find((a) =>
    a.textContent === "0xe70d821f3462a074e63b42d0AaC6523faAe1d611"
  );
  assert(baseSafe, "base safe address rendered");
  assert(
    baseSafe.href ===
      "https://basescan.org/address/0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
    "base-network address links to basescan, got " + baseSafe.href,
  );
  const ethSafe = addrs.find((a) =>
    a.textContent === "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329"
  );
  assert(
    ethSafe &&
      ethSafe.href ===
        "https://etherscan.io/address/0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329",
    "ethereum-network address links to etherscan, got " +
      (ethSafe && ethSafe.href),
  );
});

Deno.test("deployments: an unresolved address renders as not-found, not dropped", () => {
  const data = {
    deploymentOwners: {
      repo: "st0x.deploy",
      org: "S01-Issuer",
      threshold: 3,
      signerCount: 6,
      groups: [{
        id: "safe",
        title: "t",
        note: "",
        entries: [
          {
            role: "Ethereum Safe",
            address: null,
            network: "ethereum",
            status: "active",
            note: "",
          },
        ],
      }],
    },
  };
  const box = deploymentsBox(data);
  const missing = collect(box, "own-missing");
  assert(missing.length === 1, "one not-found placeholder");
  assert(
    (missing[0].textContent || "").includes("not found"),
    "labels it not found",
  );
  // The row survives even though its address didn't resolve.
  assert(
    collect(box, "own-role").length === 1,
    "the role row is still rendered",
  );
});

Deno.test("deployments: no owner data shows an empty state and no groups", () => {
  const box = deploymentsBox({ deploymentOwners: null });
  assert(collect(box, "empty").length === 1, "empty-state message shown");
  assert(collect(box, "own-group").length === 0, "no owner groups rendered");
});

Deno.test("deployments: verified signers show constant + on-chain provenance side by side", () => {
  const box = deploymentsBox(OWNERS);
  assert(
    collect(box, "own-verify-ok").length === 1,
    "an on-chain-verified banner",
  );
  // each of the six signers renders BOTH a 'constant ✓' and an 'on-chain ✓' chip
  const labels = collect(box, "own-chip").map((c) => c.textContent);
  assert(
    labels.filter((l) => l === "constant ✓").length === 6,
    "six constant ✓ chips",
  );
  assert(
    labels.filter((l) => l === "on-chain ✓").length === 6,
    "six on-chain ✓ chips",
  );
});

Deno.test("deployments: on-chain drift shows a drift banner, a missing chip, and an unexpected row", () => {
  const data = {
    deploymentOwners: {
      repo: "st0x.deploy",
      org: "S01-Issuer",
      threshold: 3,
      signerCount: 2,
      groups: [{
        id: "signers",
        title: "Safe signers",
        note: "",
        verification: {
          reachable: true,
          network: "base",
          safe: "0xe70dSafe",
          rpcHost: "mainnet.base.org",
          onChainCount: 2,
          match: false,
          threshold: { declared: 3, onChain: 2, match: false },
        },
        entries: [
          {
            role: "Signer 1",
            address: "0x1111111111111111111111111111111111111111",
            network: "",
            status: "active",
            note: "",
            onChain: "match",
          },
          {
            role: "Signer 2",
            address: "0x2222222222222222222222222222222222222222",
            network: "",
            status: "active",
            note: "",
            onChain: "missing",
          },
          {
            role: "Unexpected on-chain owner",
            address: "0xdead000000000000000000000000000000000001",
            network: "base",
            status: "extra",
            note: "not declared",
            onChain: "extra",
          },
        ],
      }],
    },
  };
  const box = deploymentsBox(data);
  assert(collect(box, "own-verify-drift").length === 1, "a drift banner");
  const labels = collect(box, "own-chip").map((c) => c.textContent);
  // the declared-but-absent signer reads on-chain ✗
  assert(
    labels.filter((l) => l === "on-chain ✗").length === 1,
    "one on-chain ✗ (missing signer)",
  );
  // the on-chain-only owner reads constant ✗ (its on-chain source is still ✓)
  assert(
    labels.filter((l) => l === "constant ✗").length === 1,
    "one constant ✗ (unexpected owner)",
  );
  assert(
    collect(box, "own-extra").length === 1,
    "the unexpected owner is its own flagged row",
  );
  const banner = collect(box, "own-verify-drift")[0];
  assert(
    textOf(banner).includes("3 (constant) · 2 (on-chain)"),
    "threshold mismatch shown: " + textOf(banner),
  );
});

Deno.test("deployments: 0.1.1 suite health renders per-contract code + keccak checks", () => {
  const data = {
    deploymentOwners: null, // health must render even without owners
    deploymentHealth: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      version: "0.1.1",
      network: "base",
      rpcHost: "mainnet.base.org",
      total: 3,
      healthy: 2,
      contracts: [
        {
          name: "StoxReceipt",
          address: "0x2dF5cFE6d688EF9fF1B7c59A499D254b1527b286",
          status: "healthy",
          codeMatch: true,
          hashMatch: true,
          erc165: "conformant",
        },
        {
          name: "StoxReceiptVault",
          address: "0x2BCcEd626566Ef1e65F922DD03748C5C7aa2d748",
          status: "healthy",
          codeMatch: true,
          hashMatch: true,
          erc165: "absent",
        },
        {
          name: "StoxGone",
          address: "0xdead000000000000000000000000000000000001",
          status: "missing",
          codeMatch: false,
          hashMatch: false,
          erc165: "nonconformant",
        },
      ],
    },
  };
  const box = deploymentsBox(data);
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  assert(chips.filter((l) => l === "code ✓").length === 2, "two code ✓");
  assert(chips.filter((l) => l === "keccak ✓").length === 2, "two keccak ✓");
  assert(
    chips.filter((l) => l === "code ✗").length === 1,
    "one code ✗ (missing contract)",
  );
  assert(
    chips.includes("missing"),
    "the unhealthy contract shows its status pill",
  );
  assert(
    collect(box, "own-verify-drift").length === 1,
    "a not-all-healthy summary banner",
  );
  assert(
    collect(box, "hlth-missing").length === 1,
    "the missing contract's row is flagged",
  );
  // ERC-165 conformance chip per contract: conformant ✓ / absent — / nonconformant ✗
  assert(
    chips.filter((l) => l === "erc165 ✓").length === 1,
    "one erc165 ✓ (conformant)",
  );
  assert(
    chips.filter((l) => l === "erc165 —").length === 1,
    "one erc165 — (absent)",
  );
  assert(
    chips.filter((l) => l === "erc165 ✗").length === 1,
    "one erc165 ✗ (nonconformant)",
  );
});

Deno.test("deployments: beacons resolve owner (Safe/legacy) + impl version and flag behind-target", () => {
  const TARGET = "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286";
  const data = {
    deploymentOwners: null,
    deploymentHealth: null,
    deploymentBeacons: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      network: "base",
      rpcHost: "mainnet.base.org",
      safeOwner: "0xe70d821f3462a074e63b42d0aac6523faae1d611",
      targetVersion: "0.1.1",
      total: 2,
      healthy: 0,
      beacons: [
        {
          name: "Receipt beacon",
          address: "0x86e93c39B095be0B0054C8488E26466Ee027D79a",
          owner: "0xe70d821f3462a074e63b42d0aac6523faae1d611",
          ownerLabel: "safe",
          implementation: "0xe7573879d73455dc92cb4087fa8177594387cbcd",
          implVersion: "V1",
          targetImpl: TARGET,
          targetVersion: "0.1.1",
          atTarget: false,
          status: "behind",
        },
        {
          name: "Vault beacon",
          address: "0xEa084c8F4331CDF3328E772781b59F8A24F28F1A",
          owner: "0x8e4bdeec7ceb9570d440676345da1dce10329f5b",
          ownerLabel: "legacy",
          implementation: TARGET,
          implVersion: "0.1.1",
          targetImpl: TARGET,
          targetVersion: "0.1.1",
          atTarget: true,
          status: "drift",
        },
      ],
    },
  };
  const box = deploymentsBox(data);
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  // owners labelled by identity, not a bare tick
  assert(chips.includes("Safe"), "owner labelled Safe");
  assert(chips.includes("legacy EOA"), "owner labelled legacy EOA");
  // the behind beacon shows NOW (V1) and the should-be TARGET (0.1.1), each with an address
  assert(chips.includes("V1"), "now impl labelled V1: " + chips.join(","));
  assert(collect(box, "own-chip-target").length >= 1, "a target-impl chip");
  const addrs = collect(box, "own-addr").map((a) => a.textContent);
  assert(
    addrs.includes(TARGET),
    "the target impl address is shown for checking a proposed upgrade",
  );
  assert(chips.includes("0.1.1 ✓"), "the at-target beacon confirms 0.1.1");
  // statuses
  assert(chips.includes("behind"), "a behind status");
  assert(chips.includes("drift"), "a drift status (legacy owner)");
  assert(
    collect(box, "own-verify-drift").length === 1,
    "not-all-healthy beacon banner",
  );
});

Deno.test("deployments: tokens check registry identity + asset wiring, flag mismatch/wiring", () => {
  const UNWRAP = "0x7271b5e7ff0f74f5e7e6c8b8c8a1b3c4d5e6f7a8";
  const WRONG = "0xbeef000000000000000000000000000000000002";
  const CUR = "0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456"; // current prod authoriser
  const TGT = "0x315b16faa6ee413fabca877d3851b3818369f0cd"; // V4-clone target
  const data = {
    deploymentOwners: null,
    deploymentHealth: null,
    deploymentBeacons: null,
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      rpcHost: "mainnet.base.org",
      total: 3,
      ok: 1,
      wrappedCount: 3,
      atAuthoriserTarget: 1,
      authoriser: { current: CUR, target: TGT, targetDeployed: true },
      tokens: [
        // fully wired AND already at the V4-clone authoriser target
        {
          symbol: "wtNVDA",
          name: "Wrapped NVIDIA Corporation ST0x",
          address: "0xFb5B41acdbA20a3230F84BE995173CFb98b8D6E7",
          status: "ok",
          wrapped: true,
          nameOk: true,
          symbolOk: true,
          decimalsOk: true,
          assetOk: true,
          asset: UNWRAP,
          unwrapped: UNWRAP,
          legacy: "0xaaa1",
          receipt: "0xbbb1",
          unwrappedDeployed: true,
          legacyDeployed: true,
          receiptDeployed: true,
          authoriser: TGT,
          authoriserLabel: "target",
          authoriserTarget: TGT,
          atAuthoriserTarget: true,
          inMigrationSet: true,
        },
        // asset() points at the wrong underlying → wiring; authoriser still at current
        {
          symbol: "wtAMZN",
          name: "Wrapped Amazon ST0x",
          address: "0xAAAA000000000000000000000000000000000001",
          status: "wiring",
          wrapped: true,
          nameOk: true,
          symbolOk: true,
          decimalsOk: true,
          assetOk: false,
          asset: WRONG,
          unwrapped: UNWRAP,
          legacy: "0xaaa2",
          receipt: "0xbbb2",
          unwrappedDeployed: true,
          legacyDeployed: true,
          receiptDeployed: true,
          authoriser: CUR,
          authoriserLabel: "current",
          authoriserTarget: TGT,
          atAuthoriserTarget: false,
          inMigrationSet: true,
        },
        // on-chain symbol disagrees with the registry → mismatch; authoriser at current
        {
          symbol: "wtTSLA",
          name: "Wrapped Tesla ST0x",
          address: "0xBBBB000000000000000000000000000000000003",
          status: "mismatch",
          wrapped: true,
          nameOk: true,
          symbolOk: false,
          decimalsOk: true,
          assetOk: true,
          asset: UNWRAP,
          unwrapped: UNWRAP,
          legacy: "0xaaa3",
          receipt: "0xbbb3",
          unwrappedDeployed: true,
          legacyDeployed: true,
          receiptDeployed: true,
          authoriser: CUR,
          authoriserLabel: "current",
          authoriserTarget: TGT,
          atAuthoriserTarget: false,
          inMigrationSet: true,
        },
        // NOTE: USDC is deliberately NOT here. The main list is the intersection
        // (registry tokens the migration governs); a registry token with no governed
        // receipt vault is a reconciliation discrepancy and belongs in
        // reconcile.missingFromMigration below.
      ],
      // cross-check vs the migration's authoritative vault set, BOTH directions:
      // one governed vault (tIBHG) is in the bundle but not the registry, and one
      // registry token (USDC — plain collateral, no vault) is in the registry but
      // not the bundle.
      reconcile: {
        source: "S01-Issuer/st0x.deploy",
        function: "LibTokenInvariants.productionReceiptVaults()",
        governedCount: 4,
        registryTokenCount: 4,
        extraVaults: [
          {
            address: "0x3c0F093aa1eD511910279b2C8d56eF5c96f1a6cF",
            name: "iShares iBonds 2027 Term High Yield ST0x",
            symbol: "tIBHG",
            deployed: true,
            authoriser: CUR,
            authoriserLabel: "current",
            authoriserTarget: TGT,
            atAuthoriserTarget: false,
          },
        ],
        missingFromMigration: [
          {
            symbol: "USDC",
            name: "USD Coin",
            address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            receiptVault: null,
            wrapped: false,
            reason: "no receipt vault (collateral)",
          },
        ],
      },
    },
  };
  const box = deploymentsBox(data);
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  // identity chips read the three registry fields back off the token contract
  assert(chips.includes("name ✓"), "name identity chip");
  assert(chips.includes("symbol ✗"), "the mismatched token's symbol chip is ✗");
  assert(chips.includes("decimals ✓"), "decimals identity chip");
  // per-token status pills
  assert(
    chips.filter((c) => c === "ok").length === 1,
    "the fully-wired token shows ok",
  );
  assert(chips.includes("wiring"), "the bad-asset token shows wiring");
  assert(
    chips.includes("mismatch"),
    "the symbol-mismatch token shows mismatch",
  );
  const addrs = collect(box, "own-addr").map((a) => a.textContent);
  assert(addrs.includes(WRONG), "the actual (wrong) asset address is shown");
  assert(addrs.includes(UNWRAP), "the expected unwrapped address is shown");
  // the main list is the intersection: USDC is NOT a row here (only in the
  // cross-check below), so it contributes no main-list status pill.
  assert(
    collect(box, "own-table")[0].children.length === 3,
    "the main token table holds only the three governed tokens",
  );
  // Authoriser provenance: the migrated vault shows the V4 clone confirmed; the
  // two pre-migration vaults show the current authoriser NOW + the V4-clone target
  // (address linked), so the setAuthorizer bundle can be checked per vault.
  assert(
    chips.includes("V4 clone ✓"),
    "the migrated vault confirms the V4 clone",
  );
  assert(
    chips.filter((c) => c === "current prod authoriser").length === 3,
    "two registry vaults + the extra governed vault show the current prod authoriser",
  );
  assert(
    chips.filter((c) => c === "V4 clone").length === 3,
    "three → target lines point at the V4 clone (two registry + the extra vault)",
  );
  assert(addrs.includes(TGT), "the V4-clone target address is linked");
  assert(addrs.includes(CUR), "the current authoriser address is linked");
  // a mismatched identity field shows the on-chain value NOW next to the registry value
  assert(
    chips.includes("on-chain"),
    "a mismatched identity field shows its on-chain value",
  );
  assert(
    chips.includes("registry"),
    "…next to the registry value it should be",
  );
  // exactly one expected-unwrapped target chip (the wiring token)
  assert(
    chips.filter((c) => c === "unwrapped").length === 1,
    "one expected-unwrapped target chip (the wiring token)",
  );
  // section-level migration banner states the target + progress
  const banners = collect(box, "own-verify").map((b) => textOf(b));
  assert(
    banners.some((m) =>
      m.includes("Authoriser migration") && m.includes("1 of 3")
    ),
    "the authoriser migration banner states progress + target",
  );
  // not-all-ok summaries: wired + authoriser + cross-check(governed) + cross-check(missing)
  assert(
    collect(box, "own-verify-drift").length === 4,
    "wired + authoriser + cross-check-governed + cross-check-missing banners",
  );
  assert(collect(box, "hlth-wiring").length === 1, "the wiring row is flagged");
  assert(
    collect(box, "hlth-mismatch").length === 1,
    "only the identity-mismatch token row is red-flagged",
  );
  // registry→migration (per token): every token in the main list confirms it is in
  // the setAuthorizer bundle (the main list IS the intersection).
  const tokVals = collect(box, "tok-val").map((v) => v.textContent);
  assert(
    tokVals.filter((v) => v === "in setAuthorizer bundle").length === 3,
    "all three main-list tokens show they are in the migration bundle",
  );
  assert(
    !tokVals.some((v) =>
      (v || "").includes("no receipt vault (collateral)") &&
      (v || "").includes("not in the migration")
    ),
    "no not-in-migration token appears in the main list — those live in the cross-check",
  );
  // Migration-set cross-check, BOTH directions.
  assert(
    collect(box, "tok-h3").length === 1,
    "a migration-set cross-check heading",
  );
  assert(
    banners.some((m) =>
      m.includes("4 governed receipt vaults") &&
      m.includes("4 registry tokens") &&
      m.includes("1 governed vault(s) not in the registry") &&
      m.includes("1 registry token(s) not in the migration")
    ),
    "the cross-check banner reconciles both directions at the entry level",
  );
  const roles = collect(box, "own-role").map((r) => r.textContent);
  const notes = collect(box, "own-note").map((n) => n.textContent);
  // migration→registry: the governed vault not in the registry (tIBHG) is surfaced.
  assert(roles.includes("tIBHG"), "the unlisted governed vault is surfaced");
  assert(
    notes.some((n) => (n || "").includes("not in registry")),
    "the extra vault is labelled not-in-registry",
  );
  assert(
    chips.includes("unlisted"),
    "the extra vault carries an unlisted pill",
  );
  assert(
    addrs.includes("0x3c0F093aa1eD511910279b2C8d56eF5c96f1a6cF"),
    "the unlisted vault address is linked for cross-checking the Safe tx",
  );
  // registry→migration: USDC (in registry, no governed vault) is surfaced as a row
  // with a `collateral` pill (expected, not a red gap).
  assert(
    roles.filter((r) => r === "USDC").length >= 1,
    "USDC is surfaced in the migration cross-check",
  );
  assert(
    notes.some((n) => (n || "").includes("no receipt vault (collateral)")),
    "USDC is labelled as collateral with no vault",
  );
  assert(
    chips.includes("collateral"),
    "the collateral token carries a collateral pill",
  );
  // tIBHG (extra vault) + USDC (collateral, no-vault) both use the non-red extra style.
  assert(
    collect(box, "hlth-extra").length === 2,
    "the unlisted governed vault and the collateral token are both flagged (not red)",
  );
});

// A render function reaching for the global `document` removes its branch from
// the reachable-under-test set: the harness injects `el` and a document stub per
// bind, so anything a test does not inject throws when that line is reached, and
// the paths that avoid it stay green. That is how the graph node's diffstat drifted
// from the row beside it while the suite passed. `append()` accepts strings, so
// `createTextNode` is never needed — this pins that, rather than trusting review.
Deno.test("site: no render code calls document.createTextNode", () => {
  const offenders = [];
  for (const f of Deno.readDirSync(new URL("../site", import.meta.url))) {
    if (!f.name.endsWith(".html")) continue;
    const src = Deno.readTextFileSync(
      new URL("../site/" + f.name, import.meta.url),
    );
    const n = (src.match(/document\.createTextNode/g) || []).length;
    if (n) offenders.push(`${f.name} (${n})`);
  }
  assert(
    offenders.length === 0,
    "append() takes strings; pass the string instead of createTextNode — " +
      offenders.join(", "),
  );
});

// ---------------------------------------------------------------------------
// Renderers that had no coverage at all. A global at least throws when reached;
// an untested renderer just drifts — which is how the graph node came to
// contradict the row beside it.
// ---------------------------------------------------------------------------

// Bind a renderer to a stub `$` returning one box, plus whatever else it closes
// over. Returns [invoke, box].
function boxBind(file, name, extraParams, extraValues, boxId) {
  const box = makeEl("div");
  const $ = (id) => (id === boxId ? box : makeEl("div"));
  const el = (tag, cls, text) => {
    const n = makeEl(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  };
  const fn = bind(
    file,
    name,
    ["$", "el", ...extraParams],
    [$, el, ...extraValues],
  );
  return [fn, box];
}

const AUDIT_ORDER_REAL = ["current", "stale", "never", "na", "unknown"];
const AUDIT_HELP_REAL = {
  current: "Protofire-audited at the current tag",
  stale: "Protofire-audited, source moved since",
  never: "no Protofire PDF — a repo may still be audited by someone else",
  unknown: "audit lookup FAILED — indeterminate, not a confirmed gap",
  na: "Protofire-audited but no tags to date it against",
};

Deno.test("graph legend: shows only the states actually present", () => {
  const [render, box] = boxBind(
    "audit.html",
    "renderGraphLegend",
    ["AUDIT_ORDER", "AUDIT_HELP"],
    [AUDIT_ORDER_REAL, AUDIT_HELP_REAL],
    "graphlegend",
  );
  render(new Set(["stale", "current"]));
  const t = textOf(box);
  assert(t.includes("stale"), "present state missing: " + t);
  assert(t.includes("current"), "present state missing: " + t);
  // A key for a state no repo is in would claim the graph shows something it
  // does not.
  assert(!t.includes("no Protofire PDF"), "absent state was listed: " + t);
  assert(!t.includes("indeterminate"), "absent state was listed: " + t);
});

Deno.test("graph legend: follows AUDIT_ORDER, not the caller's set order", () => {
  const [render, box] = boxBind(
    "audit.html",
    "renderGraphLegend",
    ["AUDIT_ORDER", "AUDIT_HELP"],
    [AUDIT_ORDER_REAL, AUDIT_HELP_REAL],
    "graphlegend",
  );
  // Insertion order deliberately reversed against AUDIT_ORDER.
  render(new Set(["unknown", "never", "current"]));
  const t = textOf(box);
  const order = ["current", "never", "unknown"].map((s) => t.indexOf(s));
  assert(
    order[0] < order[1] && order[1] < order[2],
    "legend order should be stable regardless of Set order: " + t,
  );
});

function summaryBind() {
  return boxBind("audit.html", "renderGraphSummary", [], [], "graphsum");
}

Deno.test("graph summary: a current audit is not ready-to-audit work", () => {
  const [render, box] = summaryBind();
  render([{ repo: "done", audit: "current", blockedBy: [] }], new Map());
  assert(
    textOf(box).includes("Nothing is unblocked"),
    "an already-current repo must not be offered as work: " + textOf(box),
  );
});

Deno.test("graph summary: a repo with unknown deps is not called clear ground", () => {
  const [render, box] = summaryBind();
  render(
    [{ repo: "unreadable", audit: "never", blockedBy: [], depsKnown: false }],
    new Map(),
  );
  assert(
    textOf(box).includes("Nothing is unblocked"),
    "cannot claim clear ground when the manifest would not parse: " +
      textOf(box),
  );
});

Deno.test("graph summary: orders by how many repos inherit the gap", () => {
  const [render, box] = summaryBind();
  const nodes = [
    { repo: "few", audit: "never", blockedBy: [] },
    { repo: "many", audit: "never", blockedBy: [] },
    { repo: "c1", audit: "never", blockedBy: ["many"] },
    { repo: "c2", audit: "never", blockedBy: ["many"] },
    { repo: "c3", audit: "never", blockedBy: ["few"] },
  ];
  render(nodes, new Map());
  const t = textOf(box);
  assert(
    t.indexOf("many") < t.indexOf("few"),
    "the most-inherited gap should lead: " + t,
  );
  assert(t.includes("2 inherit"), "inheritor count should be shown: " + t);
});

Deno.test("graph summary: leads with code drift, not the undifferentiated total", () => {
  const [render, box] = summaryBind();
  const pf = new Map([["natspec-only", {
    sourceLocAddedSinceAudit: 7,
    sourceLocRemovedSinceAudit: 1,
    codeLocAddedSinceAudit: 0,
    codeLocRemovedSinceAudit: 0,
  }]]);
  render([{ repo: "natspec-only", audit: "stale", blockedBy: [] }], pf);
  const t = textOf(box);
  assert(t.includes("+0"), "expected code drift, got: " + t);
  assert(
    !t.includes("+7"),
    "the undifferentiated total contradicts the rows and nodes: " + t,
  );
});

Deno.test("graph summary: pre-split data falls back to the old total", () => {
  const [render, box] = summaryBind();
  const pf = new Map([["legacy", {
    sourceLocAddedSinceAudit: 42,
    sourceLocRemovedSinceAudit: 3,
  }]]);
  render([{ repo: "legacy", audit: "stale", blockedBy: [] }], pf);
  assert(
    textOf(box).includes("+42"),
    "legacy scan data should still show its figure: " + textOf(box),
  );
});

// --- metrics.html -----------------------------------------------------------
// startupMin and median are bound from the page too, so these exercise the real
// arithmetic rather than a reimplementation of it in the test.
const startupMinReal = bind("metrics.html", "startupMin", [], []);
const medianReal = bind("metrics.html", "median", [], []);

function pmBind(name, boxId, pmMode, extraParams = [], extraValues = []) {
  const box = makeEl("div");
  const $ = (id) => (id === boxId ? box : makeEl("div"));
  const document = { createElement: (t) => makeEl(t) };
  const fn = bind(
    "metrics.html",
    name,
    ["$", "document", "pmMode", "startupMin", "median", ...extraParams],
    [$, document, pmMode, startupMinReal, medianReal, ...extraValues],
  );
  return [fn, box];
}

Deno.test("metrics tiles: latest skips a trailing run with no value", () => {
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  // The newest run has no startup figure. Taking runs[last] blindly would
  // report "—" and hide the last real measurement.
  render([{ startupPct: 10 }, { startupPct: 20 }, { startupPct: null }]);
  const t = textOf(box);
  assert(
    t.includes("20.0%"),
    "latest should be the newest run WITH a value: " + t,
  );
  assert(!t.includes("—"), "a trailing gap should not blank the tile: " + t);
});

Deno.test("metrics tiles: median ignores gaps and counts every run", () => {
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  render([{ startupPct: 10 }, { startupPct: null }, { startupPct: 20 }]);
  const t = textOf(box);
  // Median over [10, 20] is 15 — a gap counted as 0 would give 10.
  assert(t.includes("15.0%"), "median should skip gaps, not zero them: " + t);
  // …but the run count is every run, gaps included.
  assert(t.includes("3"), "runs recorded should count all runs: " + t);
});

Deno.test("metrics controls: absent startup data renders no toggle", () => {
  const [render, box] = pmBind("renderPmControls", "pmcontrols", "pct", [
    "pmRuns",
    "renderPmTiles",
    "renderPmChart",
  ], [[], () => {}, () => {}]);
  // No run carries startupMs, so absolute mode is unavailable and offering the
  // switch would produce an empty chart.
  render([{ startupPct: 5 }, { startupPct: 6 }]);
  assert(
    collect(box, "pm-toggle").length === 0,
    "no toggle should render without absolute data",
  );
});

Deno.test("metrics controls: the active mode is announced, not just styled", () => {
  const [render, box] = pmBind("renderPmControls", "pmcontrols", "abs", [
    "pmRuns",
    "renderPmTiles",
    "renderPmChart",
  ], [[], () => {}, () => {}]);
  render([{ startupPct: 5, startupMs: 60000 }]);
  const buttons = collect(box, "pm-toggle")[0].children;
  assert(
    buttons.length === 2,
    "expected two mode buttons, got " + buttons.length,
  );
  const pressed = buttons.filter((b) =>
    b.getAttribute("aria-pressed") === "true"
  );
  assert(
    pressed.length === 1 && pressed[0]._text.includes("absolute"),
    "exactly the active mode should read aria-pressed=true",
  );
});

// --- repositories.html ------------------------------------------------------
function repoSummaryBind(summary) {
  const box = makeEl("div");
  const $ = (id) => (id === "summary" ? box : makeEl("div"));
  const document = { createElement: (t) => makeEl(t) };
  const fn = bind(
    "repositories.html",
    "renderSummary",
    ["$", "document", "data", "activeSignal", "setSignal"],
    [$, document, { summary }, null, () => {}],
  );
  return [fn, box];
}

Deno.test("repo summary: bar width is proportional to the largest signal", () => {
  const [render, box] = repoSummaryBind({ big: 10, half: 5 });
  render();
  const fills = collect(box, "fill");
  assert(fills.length === 2, "expected a bar per signal, got " + fills.length);
  assert(
    fills[0].style.width === "100%",
    "largest should fill: " + fills[0].style.width,
  );
  assert(
    fills[1].style.width === "50%",
    "half the count should be half the bar: " + fills[1].style.width,
  );
});

Deno.test("repo summary: each row carries its signal and count", () => {
  const [render, box] = repoSummaryBind({ "old-actions-checkout": 7 });
  render();
  const row = collect(box, "srow")[0];
  assert(
    row.dataset.sig === "old-actions-checkout",
    "the row must carry its signal for filtering: " + row.dataset.sig,
  );
  assert(textOf(row).includes("7"), "count should render: " + textOf(row));
});

Deno.test("repo summary: no debt renders an empty state, not a blank panel", () => {
  const [render, box] = repoSummaryBind({});
  render();
  assert(
    String(box.innerHTML).includes("No modernization debt"),
    "an empty summary should say so: " + box.innerHTML,
  );
});

// The producer runs every 4 hours, so a date-only axis label cannot identify
// which of six daily runs a point is. These render the REAL chart and assert on
// its emitted axis markup — asserting on the formatter alone would pass even if
// the chart never called it, which is exactly how a renderer drifts from its
// helper unnoticed.
function pmChart(runs, pmMode = "pct") {
  const wrap = makeEl("div");
  const svg = makeEl("svg");
  const nodes = {
    pmwrap: wrap,
    pmsvg: svg,
    pmtip: makeEl("div"),
    pmcursor: makeEl("div"),
  };
  const $ = (id) => nodes[id] || makeEl("div");
  const parseRunId = bind("metrics.html", "parseRunId", [], []);
  const MON = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  const fmtDayTime = bind("metrics.html", "fmtDayTime", ["MON", "parseRunId"], [
    MON,
    parseRunId,
  ]);
  const fmtRunTime = bind("metrics.html", "fmtRunTime", ["MON", "parseRunId"], [
    MON,
    parseRunId,
  ]);
  const startupMin = bind("metrics.html", "startupMin", [], []);
  const plotMin = (r) => startupMin(r);
  bind(
    "metrics.html",
    "renderPmChart",
    [
      "$",
      "pmMode",
      "parseRunId",
      "fmtDayTime",
      "fmtRunTime",
      "startupMin",
      "plotMin",
    ],
    [$, pmMode, parseRunId, fmtDayTime, fmtRunTime, startupMin, plotMin],
  )(runs);
  return String(wrap.innerHTML);
}

const RUN_A = {
  runId: "20260720T010001Z",
  startupPct: 4.3,
  startupMs: 590693,
  toolCalls: 529,
  startupToolCalls: 23,
  numTurns: 66,
  outcome: "ok",
};
const RUN_B = {
  runId: "20260720T170002Z",
  startupPct: 61.5,
  startupMs: 700000,
  toolCalls: 431,
  startupToolCalls: 200,
  numTurns: 128,
  outcome: "ok",
};

Deno.test("metrics chart: the axis shows an absolute time, not just a date", () => {
  const svg = pmChart([RUN_A, RUN_B]);
  assert(
    svg.includes("Jul 20"),
    "expected the date in the axis: " + svg.slice(-400),
  );
  assert(
    svg.includes("01:00"),
    "expected the first run's time: " + svg.slice(-400),
  );
  assert(
    svg.includes("17:00"),
    "expected the last run's time: " + svg.slice(-400),
  );
  assert(svg.includes("UTC"), "expected an explicit zone: " + svg.slice(-400));
});

Deno.test("metrics chart: two runs on the same day get distinct axis labels", () => {
  const svg = pmChart([RUN_A, RUN_B]);
  // Before absolute times both endpoints rendered as the bare date "Jul 20".
  const labels = [...svg.matchAll(/<text[^>]*>([^<]*Jul 20[^<]*)<\/text>/g)]
    .map((m) => m[1]);
  assert(
    labels.length >= 2,
    "expected two dated axis labels, got: " + JSON.stringify(labels),
  );
  assert(
    labels[0] !== labels[1],
    "same-day endpoints must not share a label: " + JSON.stringify(labels),
  );
});

Deno.test("metrics chart: a single run still gets an absolute axis label", () => {
  const svg = pmChart([RUN_A]);
  assert(
    svg.includes("01:00"),
    "single-run axis should carry its time: " + svg.slice(-400),
  );
  assert(svg.includes("UTC"), "single-run axis should carry the zone");
});

// The chart opens on absolute minutes. A percentage answers "what share of the
// run was startup"; the question the chart gets opened for is "how long did I
// wait". The fallback matters more than the default: with no absolute data an
// "abs" chart would render empty, so renderPmControls downgrades to "pct" —
// and it runs before the tiles and chart, which is what makes that safe.
Deno.test("metrics: the chart opens on absolute minutes", () => {
  const src = Deno.readTextFileSync(
    new URL("../site/metrics.html", import.meta.url),
  );
  assert(
    /let pmMode = "abs";/.test(src),
    "expected the initial mode to be absolute",
  );
});

Deno.test("metrics controls: no absolute data downgrades the mode to proportion", () => {
  // Runs with startupPct but no startupMs — absolute mode has nothing to plot.
  const [render, box] = pmBind("renderPmControls", "pmcontrols", "abs", [
    "pmRuns",
    "renderPmTiles",
    "renderPmChart",
  ], [[], () => {}, () => {}]);
  render([{ startupPct: 5 }, { startupPct: 6 }]);
  assert(
    collect(box, "pm-toggle").length === 0,
    "no toggle should render when absolute data is absent",
  );
});

Deno.test("metrics controls: with absolute data the toggle opens on absolute", () => {
  const [render, box] = pmBind("renderPmControls", "pmcontrols", "abs", [
    "pmRuns",
    "renderPmTiles",
    "renderPmChart",
  ], [[], () => {}, () => {}]);
  render([{ startupPct: 5, startupMs: 60000 }]);
  const buttons = collect(box, "pm-toggle")[0].children;
  const pressed = buttons.filter((b) =>
    b.getAttribute("aria-pressed") === "true"
  );
  assert(
    pressed.length === 1 && pressed[0]._text.includes("absolute"),
    "absolute should be the pressed mode on open",
  );
});
