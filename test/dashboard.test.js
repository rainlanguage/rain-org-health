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
    // Attributes the render code sets for accessibility (role, aria-pressed)
    // and for SVG geometry. Stored rather than ignored so a test can assert the
    // state a control announces, not just the class it happens to carry.
    attrs: {},
    setAttribute(k, v) {
      el.attrs[k] = String(v);
      // An SVG element has no writable className, so its class arrives through
      // setAttribute. Mirroring it keeps ONE class view for the selectors here,
      // which is what the real DOM does too.
      if (k === "class") el.className = String(v);
    },
    getAttribute(k) {
      return Object.prototype.hasOwnProperty.call(el.attrs, k)
        ? el.attrs[k]
        : null;
    },
    // Layout: the stub has none, so a test that exercises a pointer path sets
    // the rect it wants the render code to read.
    _rect: { left: 0, top: 0, width: 0, height: 0 },
    getBoundingClientRect() {
      return el._rect;
    },
    addEventListener(type, fn) {
      (el._ev[type] = el._ev[type] || []).push(fn);
    },
    click() {
      (el._ev.click || []).forEach((fn) => fn());
    },
    // Fire the handlers registered for `type` with a synthetic event, so a test
    // can drive hover paths (the tooltip) as well as clicks.
    fire(type, ev) {
      (el._ev[type] || []).forEach((fn) => fn(ev));
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

// Every descendant element with the given tag name.
function tags(root, tag, out = []) {
  for (const c of root.children || []) {
    if (c && typeof c === "object") {
      if (c.tagName === tag) out.push(c);
      tags(c, tag, out);
    }
  }
  return out;
}

// The document surface the render code touches: element creation (namespaced,
// for the SVG chart) and fragments.
function stubDocument() {
  return {
    createElement: (t) => makeEl(t),
    createElementNS: (_ns, t) => makeEl(t),
    createDocumentFragment: () => {
      const f = makeEl("#fragment");
      f._isFragment = true;
      return f;
    },
  };
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

function fsmBox(hq, history) {
  const box = makeEl("div");
  const document = {
    createElement: (t) => makeEl(t),
    createElementNS: (_ns, t) => makeEl(t),
    createDocumentFragment: () => {
      const f = makeEl("#fragment");
      f._isFragment = true;
      return f;
    },
  };
  const $ = (id) => (id === "fsm" ? box : makeEl("div"));
  // renderFsm calls the sibling top-level `sevenDaySlope` (the #32 trend). The harness
  // evals each function standalone, so inject that sibling as a param — in the browser
  // it resolves from module scope. `history` is optional (an older/unfetched rollup).
  const slope = bind("pipeline.html", "sevenDaySlope", [], []);
  bind(
    "pipeline.html",
    "renderFsm",
    ["document", "$", "sevenDaySlope"],
    [document, $, slope],
  )(hq, history);
  return box;
}

// Zoom is BUTTONS ONLY — the repo rejects binding pan/zoom GESTURES in JS
// (CLAUDE.md: touch-action:none suppresses the device's real pinch to reimplement
// it worse). These pin the button arithmetic, not any gesture path.
const graphFit = bind("audit.html", "graphFit", [], []);
const zoomScale = bind(
  "audit.html",
  "zoomScale",
  ["GRAPH_ZOOM_STEP", "GRAPH_ZOOM_MAX"],
  [1.25, 3],
);

const zoomBtnDisabled = bind("audit.html", "zoomBtnDisabled", ["GRAPH_ZOOM_MAX"], [3]);

Deno.test("graph zoom: at the initial fit only zoom-IN is live", () => {
  // What the page shows on first render: fitted, so out/Fit are dead ends and the
  // only useful move is in. A board where every control looks dead reads as broken.
  const off = zoomBtnDisabled(0.5, 0.5);
  assert(off.in === false, "zoom-in is live at the fit");
  assert(off.out === true, "zoom-out is dead at the fit");
  assert(off.fit === true, "Fit is dead when already fitted");
});

Deno.test("graph zoom: at the ceiling only zoom-OUT and Fit are live", () => {
  const off = zoomBtnDisabled(3, 0.5);
  assert(off.in === true, "zoom-in is dead at the max");
  assert(off.out === false, "zoom-out is live at the max");
  assert(off.fit === false, "Fit is live once zoomed off the fit");
});

Deno.test("graph fit: fits the whole graph inside the box padding, never magnifying", () => {
  // 16px padding each side, so a 1032-wide box holds 1000px of graph at 1:1.
  assert(graphFit(1032, 1032, 1000, 1000) === 1, "exactly-fitting graph sits at 1:1");
  // A graph twice the box width fits at half scale, and the TIGHTER axis wins.
  assert(graphFit(532, 1032, 1000, 1000) === 0.5, "width-constrained: " + graphFit(532, 1032, 1000, 1000));
  assert(graphFit(1032, 532, 1000, 1000) === 0.5, "height-constrained");
  // Small graph in a big box is NOT blown up — 1 is the ceiling.
  assert(graphFit(2032, 2032, 100, 100) === 1, "never magnifies past 1:1");
});

Deno.test("graph zoom: steps in and out, clamped by the fit below and the max above", () => {
  const fit = 0.5;
  assert(zoomScale(fit, fit, "in") === 0.625, "one step in: " + zoomScale(fit, fit, "in"));
  assert(zoomScale(1, fit, "out") === 0.8, "one step out: " + zoomScale(1, fit, "out"));
  // Nothing smaller than "the whole graph" is useful, so the fit is the floor.
  assert(zoomScale(fit, fit, "out") === fit, "cannot zoom out past the fit");
  assert(zoomScale(0.55, fit, "out") === fit, "a step that would undershoot clamps to the fit");
  // And a step cannot launch the graph out of its own scrollport.
  assert(zoomScale(3, fit, "in") === 3, "cannot zoom in past the max");
  assert(zoomScale(2.9, fit, "in") === 3, "a step that would overshoot clamps to the max");
});

Deno.test("graph zoom: fit action returns the CURRENT fit, not a remembered one", () => {
  // Reset recomputes, so after a resize it lands on the new fit rather than the
  // scale the graph first rendered at.
  assert(zoomScale(2.4, 0.5, "fit") === 0.5, "resets to the fit passed in");
  assert(zoomScale(2.4, 0.31, "fit") === 0.31, "a resized box resets to its NEW fit");
});

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

// A backlog of open audit findings is a DEFECT the repo is still carrying, not a
// status line, so the count renders in the dashboard's semantic critical token —
// the same `--crit` the pipeline's rising-WIP flag uses. And never on colour alone.
function backlogNode(openAuditIssues) {
  return graphNode({
    repo: "rain.solmem",
    org: "rainlanguage",
    audit: "current",
    depsKnown: true,
    staleDeps: [],
    lastAudit: { auditedAt: "2026-07-17T16:34:10Z" },
    openAuditIssues,
  }, null);
}

Deno.test("graph node: a non-zero open-findings backlog reads critical, with a non-color cue", () => {
  const carrying = backlogNode(3);
  const [count] = collect(carrying, "gn-open");
  assert(count, "3 open findings takes the critical class: " + textOf(carrying));
  assert(
    collect(carrying, "ok").length === 0,
    "and is not also rendered as the recessive/clean one",
  );
  assert(
    textOf(count).includes("3 open"),
    "the count itself still reads: " + textOf(carrying),
  );
  // Never color-alone: a visible ⚑ glyph plus a spelled-out title/aria-label.
  assert(
    collect(carrying, "gn-openflag").length === 1,
    "a red count carries a visible ⚑ cue",
  );
  assert(
    count.getAttribute("title") === "3 open audit findings" &&
      count.getAttribute("aria-label") === "3 open audit findings",
    `and spells the backlog out: ${count.getAttribute("aria-label")}`,
  );
  const [one] = collect(backlogNode(1), "gn-open");
  assert(
    one.getAttribute("aria-label") === "1 open audit finding",
    `a lone finding is singular: ${one.getAttribute("aria-label")}`,
  );
});

Deno.test("graph node: a zero backlog stays recessive — no critical color, no cue", () => {
  // A repo with nothing outstanding must not read as a problem: the red is
  // reserved for a real backlog, or it stops meaning anything.
  const clean = backlogNode(0);
  assert(
    collect(clean, "gn-open").length === 0,
    "0 open never takes the critical class: " + textOf(clean),
  );
  assert(collect(clean, "gn-openflag").length === 0, "and carries no ⚑ cue");
  const [count] = collect(clean, "ok");
  assert(
    count && textOf(count).includes("0 open"),
    "it still reports the zero: " + textOf(clean),
  );
  assert(
    (count.getAttribute("aria-label") || "") === "",
    "and adds no findings aria-label",
  );
});

// The render tests above prove the CLASS lands. The red itself lives in the
// stylesheet, so this pins the other half: the class must resolve to the shared
// semantic critical token — not the accent hue, and not a hue hard-coded past the
// themes, which is what would break the dark render.
Deno.test("audit page: the open-findings count resolves to the semantic --crit token, defined for both themes", () => {
  const css = Deno.readTextFileSync(new URL("../site/audit.html", import.meta.url));
  for (const sel of [".gn-open", ".au-open"]) {
    const rule = new RegExp("^\\s*\\" + sel + "\\s*\\{([^}]*)\\}", "m").exec(css);
    assert(rule, "no rule for " + sel);
    assert(
      /color:\s*var\(--crit\)/.test(rule[1]),
      `${sel} must take the semantic critical token, got: ${rule[1]}`,
    );
    assert(
      !/var\(--accent\)|#[0-9a-f]{3,6}/i.test(rule[1]),
      `${sel} must not use the accent hue or a literal colour: ${rule[1]}`,
    );
  }
  // Light default, the OS-preference dark block, and both explicit toggle
  // overrides — a token missing from any one of them is an unreadable theme.
  const scopes = [
    /:root\s*\{[^}]*--crit:/,
    /@media \(prefers-color-scheme: dark\)\s*\{\s*:root\s*\{[^}]*--crit:/,
    /:root\[data-theme="dark"\]\s*\{[^}]*--crit:/,
    /:root\[data-theme="light"\]\s*\{[^}]*--crit:/,
  ];
  for (const s of scopes) {
    assert(s.test(css), "--crit is undefined in a theme scope: " + s);
  }
});

Deno.test("audit report: a row's open-findings backlog reads critical too, zero stays quiet", () => {
  // The row and the graph node render the same datum on the same page, so they
  // carry the same signal — one of them staying amber would read as a bug.
  const data = {
    ...auditData([
      auditRow({ name: "carrying" }),
      auditRow({ name: "clean" }),
    ], 0),
    audits: [
      { name: "carrying", org: "testorg", lastAudit: null, openAuditIssues: 14 },
      { name: "clean", org: "testorg", lastAudit: null, openAuditIssues: 0 },
    ],
  };
  const box = auditBox(data);
  const red = collect(box, "au-open");
  assert(
    red.length === 1 && textOf(red[0]).includes("14 open"),
    "only the repo with findings takes the critical class: " +
      JSON.stringify(red.map(textOf)),
  );
  assert(
    collect(box, "au-openflag").length === 1,
    "paired with a visible ⚑ cue, never color alone",
  );
  assert(
    red[0].getAttribute("aria-label") === "14 open audit findings",
    `spelled out for a screen reader: ${red[0].getAttribute("aria-label")}`,
  );
  assert(
    textOf(box).includes("0 open"),
    "the clean row still reports its zero: " + textOf(box),
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

// The FSM leak is a STATE the human must clear, not a banner beside the machine. A
// banner reads as an annotation on the diagram and gets skipped; a state box sits in
// the human's actionable set and counts toward their total like every other inbox.
Deno.test("pipeline FSM: unmodelled PRs are a human-owned state, not a banner", () => {
  const box = fsmBox({
    counts: { leaks: 3, ready: 5, closeCandidateIssues: 0 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 5, prs: [] } } },
  });
  // No banner survives — the old one was the only .fsm-alarm on the page.
  assert(
    collect(box, "fsm-alarm").length === 0,
    "the leak banner is gone",
  );
  const leak = collect(box, "fsm-state").find((b) => b.dataset.t === "leak");
  assert(leak, "a leak state box is rendered");
  assert(
    collect(leak, "sk")[0].textContent === "not in any modeled state",
    "the state keeps the banner's label",
  );
  assert(
    collect(leak, "sc")[0].textContent === "3",
    "the state shows the leak count",
  );
  assert(
    collect(leak, "sa")[0].textContent === "model it",
    "the act names what the human must do",
  );
  // It belongs to the human, and its count reaches the human total (3 leaks + 5 ready).
  const human = ownerGroups(box).find((g) => g.title.includes("Human action"));
  assert(
    human.states.includes("not in any modeled state"),
    `leak is human-owned: ${JSON.stringify(human.states)}`,
  );
  assert(
    human.title.includes("8"),
    `human total includes the leaks (3+5=8): ${human.title}`,
  );
});

// A zero leak count is the healthy case and must render as a dimmed zero box — the
// machine's shape stays visible rather than the state vanishing.
Deno.test("pipeline FSM: a zero leak count renders a dimmed state, not a gap", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {},
  });
  const leak = collect(box, "fsm-state").find((b) => b.dataset.t === "leak");
  assert(leak, "the leak state still renders at zero");
  assert(collect(leak, "sc")[0].textContent === "0", "it reads zero");
  assert(
    leak.className.includes("zero"),
    `zero states are dimmed: ${leak.className}`,
  );
});

// An older human-queue.json may not carry `leaks` at all — that must render zero, not
// NaN or a broken box.
Deno.test("pipeline FSM: an absent leaks key renders zero, not NaN", () => {
  const box = fsmBox({
    counts: { ready: 1, closeCandidateIssues: 0 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } } },
  });
  const leak = collect(box, "fsm-state").find((b) => b.dataset.t === "leak");
  assert(leak, "the leak state renders even with no leaks key");
  assert(
    collect(leak, "sc")[0].textContent === "0",
    `absent key reads 0, got ${collect(leak, "sc")[0].textContent}`,
  );
});

// Clicking the state lists the offending PRs from the top-level `leaks` array — the
// click-through the banner had is preserved by keeping the state key `leak`.
Deno.test("pipeline FSM: the leak state lists its PRs on click", () => {
  const box = fsmBox({
    counts: { leaks: 2, ready: 0, closeCandidateIssues: 0 },
    lanes: {},
    leaks: [
      { repo: "rainlanguage/raindex", number: 11, title: "unlabelled one" },
      { repo: "rainlanguage/rain.flare", number: 22, title: "unlabelled two" },
    ],
  });
  const leak = collect(box, "fsm-state").find((b) => b.dataset.t === "leak");
  leak.click();
  const text = textOf(box);
  assert(text.includes("unlabelled one"), "first leaked PR listed");
  assert(text.includes("rain.flare#22"), "second leaked PR listed by repo#number");
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
Deno.test("pipeline FSM: the close-candidate states carry distinct labels", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateUnvetted: 18, closeCandidateUpheld: 4 },
    lanes: {},
  });
  const labels = collect(box, "sk").map((s) => s.textContent);
  // One flag, three distinct boxes: the PR variant, plus the issue variant split by
  // vetting stage (issue-pr-cron#73). None may read as another.
  for (const l of [
    "ai:close-candidate (PRs)",
    "ai:close-candidate (unvetted)",
    "ai:close-candidate (upheld)",
  ]) {
    assert(labels.includes(l), `label missing: ${l} in ${JSON.stringify(labels)}`);
  }
  assert(
    new Set(labels).size === labels.length,
    `no two boxes share a label: ${JSON.stringify(labels)}`,
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
    uncoveredIssues: [
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

  // Click an ISSUE-type box (2 issues) — the SAME bottom panel re-populates with that
  // state's own header + count, so PR and issue states never blur together.
  boxByT("uncoveredIssues").click();
  assert(detail.parent === detailHost, "still the same panel, still in place");
  assert(
    collect(detail, "dhl")[0].textContent === "untouched (no PR)",
    "header re-names to the issues state",
  );
  assert(
    collect(detail, "dhc")[0].textContent === "2 items",
    "header count == the issue list length",
  );
  assert(collect(detail, "li").length === 2, "renders exactly the 2 issues");
});

// #69: the FSM report groups states by WHO MUST ACT NEXT — three actor headings
// (producer / vetter / human), and every modeled state files under exactly one of
// them (no fourth "misc" bucket). Each .fsm-lane is one actor group: its header
// names the actor, its .sk boxes are that actor's states.
function ownerGroups(box) {
  return collect(box, "fsm-lane").map((g) => ({
    title: textOf(collect(g, "fsm-lane-h")[0] || makeEl("div")),
    states: collect(g, "sk").map((s) => s.textContent),
  }));
}

Deno.test("pipeline FSM: states group under the three actor headings, no fourth bucket", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 1, closeCandidateIssues: 4 },
    lanes: {
      "vet-lifecycle": {
        "un-vetted": { count: 2, prs: [] },
        "awaiting-re-vet": { count: 1, prs: [] },
      },
      "vetter-verdicts": {
        "ai:ready": { count: 1, prs: [] },
        "ai:reject": { count: 1, prs: [] },
        "ai:relink": { count: 1, prs: [] },
        "ai:design": { count: 1, prs: [] },
        "ai:close-candidate": { count: 1, prs: [] },
      },
      "producer-blocked": {
        "ai:blocked-deploy": { count: 1, prs: [] },
        "ai:blocked-infra": { count: 1, prs: [] },
        "ai:blocked-on": { count: 1, prs: [] },
      },
      "human-decisions": {
        "human:reject": { count: 1, prs: [] },
        "human:design": { count: 1, prs: [] },
        "human:close-candidate": { count: 1, prs: [] },
      },
    },
  });
  const groups = ownerGroups(box);
  assert(
    groups.length === 3,
    `exactly three actor groups (no misc bucket): ${groups.length}`,
  );
  assert(
    groups[0].title.includes("Producer action") &&
      groups[1].title.includes("Vetter action") &&
      groups[2].title.includes("Human action"),
    `headings are the three actors in order: ${JSON.stringify(groups.map((g) => g.title))}`,
  );
  const [producer, vetter, human] = groups;
  // Vetter owns the ONE vet-lifecycle state. `awaiting-re-vet` is retired
  // (issue-pr-cron#128) and is in the fixture above as a DECOY — asserting only that
  // `un-vetted` is present would pass with the retired box still drawn beside it, so this
  // asserts both directions and a reintroduction fails here.
  assert(
    vetter.states.includes("un-vetted"),
    `vetter states: ${JSON.stringify(vetter.states)}`,
  );
  assert(
    !vetter.states.includes("awaiting-re-vet"),
    `the retired awaiting-re-vet state renders nowhere: ${JSON.stringify(vetter.states)}`,
  );
  // Producer owns the reject rework state + the untouched backlog. ai:relink is NOT
  // here: the verdict is retired (issue-pr-cron#135/#139), the producer has no relink
  // transition to execute, and the residue exits by the vetter re-vetting at current
  // head — so the retired state files under the vetter while its count is non-zero.
  assert(
    ["ai:reject", "untouched (no PR)"].every((s) =>
      producer.states.includes(s)
    ),
    `producer states: ${JSON.stringify(producer.states)}`,
  );
  // ai:relink is fully retired (issue-pr-cron#135/#139; last labelled PR migrated
  // 2026-07-31): a lanes payload still carrying it renders in NO group, and its
  // history folds into reject via HIST_FOLD.
  assert(
    groups.every((g) => !g.states.includes("ai:relink")),
    `retired ai:relink renders nowhere: ${JSON.stringify(groups.map((g) => g.states))}`,
  );
  // Human owns the merge/ruling/close states + both close-candidate variants.
  assert(
    [
      "ai:ready",
      "ai:design",
      "ai:close-candidate (PRs)",
      "ai:close-candidate (upheld)",
      "human:design",
      "human:close-candidate",
    ].every((s) => human.states.includes(s)),
    `human states: ${JSON.stringify(human.states)}`,
  );
  // Every state box sits under exactly one heading (no leaks into a fourth group).
  // 15 = the 15 original states, minus the combined close-candidate issues box, plus the
  // two owner-specific states the vetter verdict introduces (issue-pr-cron#73), plus the
  // FSM leak, promoted from a banner to a human-owned state, minus `awaiting-re-vet`,
  // collapsed into `un-vetted` by issue-pr-cron#128, minus `human:reject`, consolidated
  // into `ai:reject` by issue-pr-cron#133/#138, minus `ai:relink`, retired by
  // issue-pr-cron#135/#139 (a relink IS a reject with a specific note).
  const total = groups.reduce((n, g) => n + g.states.length, 0);
  assert(total === 14, `all 14 states filed once: ${total}`);
  // Both directions, same as awaiting-re-vet: a reintroduction fails here.
  assert(
    groups.every((g) => !g.states.includes("human:reject")),
    `the retired human:reject state renders nowhere: ${JSON.stringify(groups.map((g) => g.states))}`,
  );
});

// issue-pr-cron#128: `awaiting-re-vet` is not a state any more. Vetting is a pure function
// of the PR at its current head, so a moved head just makes a PR un-vetted — there is one
// vet-lifecycle state, handled one way. This page hardcodes the state vocabulary, so the
// retired box would otherwise render at zero, dimmed, FOREVER, describing a machine that no
// longer exists (rain-org-health#145).
Deno.test("pipeline FSM: the retired awaiting-re-vet state draws no box, even from a snapshot that still carries it", () => {
  // A snapshot from a pre-#128 emitter: the retired lane cell AND its counts key, both
  // non-zero. Nothing here may resurrect the box.
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, unvetted: 2, awaitingReVet: 7 },
    lanes: {
      "vet-lifecycle": {
        "un-vetted": { count: 2, prs: [] },
        "awaiting-re-vet": { count: 7, prs: [{ repo: "o/a", number: 1, title: "moved head" }] },
      },
    },
  });
  const boxes = collect(box, "fsm-state");
  assert(
    !boxes.some((b) => b.dataset.t === "awaiting-re-vet"),
    `no box is keyed to the retired state: ${JSON.stringify(boxes.map((b) => b.dataset.t))}`,
  );
  const labels = collect(box, "sk").map((s) => s.textContent);
  assert(
    !labels.includes("awaiting-re-vet"),
    `no box is labelled with the retired state: ${JSON.stringify(labels)}`,
  );
  // The surviving state renders, and reads its OWN lane cell — the retired cell is ignored
  // outright rather than summed in. The live snapshot's shape is the tool's to declare and
  // this page does not own that vocabulary (issue-pr-cron#130), so it renders what the tool
  // says a state holds and nothing else. HISTORY is the deliberate exception, folded below,
  // because a retired key's samples are the only surviving record of the old machine.
  const unvetted = boxes.find((b) => b.dataset.t === "un-vetted");
  assert(unvetted, "the surviving un-vetted box renders");
  assert(
    collect(unvetted, "sc")[0].textContent === "2",
    `un-vetted reads its own count, not the sum: ${collect(unvetted, "sc")[0].textContent}`,
  );
  // Its ACTION describes the collapsed state, not the half that survived the name. "first
  // vet" would say this box holds only never-judged PRs, which is the deleted machine.
  assert(
    collect(unvetted, "sa")[0].textContent === "vet at current head",
    `the action covers a first vet and a re-vet alike: ${collect(unvetted, "sa")[0].textContent}`,
  );
  // The vetter heading's total moves with it: 2, never 9.
  const vetter = ownerGroups(box).find((g) => g.title.includes("Vetter action"));
  assert(vetter.title.endsWith("2"), `vetter total is the un-vetted count: ${vetter.title}`);
});

// rain-org-health#145: deleting the state must not delete its HISTORY. Every rollup line
// written before the collapse carries `counts.awaitingReVet` alongside `counts.unvetted`,
// and they counted disjoint sets of PRs — so the quantity `un-vetted` now measures is their
// SUM at every one of those samples. Reading only the surviving key would draw a cliff at
// the change that the machine never had: it would misrepresent the past, not merely stop.
Deno.test("fsm history: retired awaiting-re-vet samples fold into un-vetted, so the series is continuous across the collapse", () => {
  const now = Date.parse("2026-07-29T16:31:36Z");
  const at = (d) => now - d * DAY;
  // Real inventory held flat at 10 across the collapse. Before it, the tool split that 10
  // across two keys (1 + 9); after it, one key carries all 10. Folded, the series is FLAT.
  // Unfolded, it steps 1 → 10 — a cliff, and a false +2.4/day bottleneck alarm.
  const history = [
    { t: at(4), counts: { unvetted: 1, awaitingReVet: 9 } },
    { t: at(3), counts: { unvetted: 1, awaitingReVet: 9 } },
    { t: at(2), counts: { unvetted: 1, awaitingReVet: 9 } },
    { t: at(1), counts: { unvetted: 10 } },
    { t: at(0), counts: { unvetted: 10 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, unvetted: 10 },
    lanes: { "vet-lifecycle": { "un-vetted": { count: 10, prs: [] } } },
  }, history);
  const unvetted = collect(box, "fsm-state").find((b) => b.dataset.t === "un-vetted");
  assert(unvetted, "un-vetted box rendered");
  const line = tags(unvetted, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) => Number(p.split(",")[1]));
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  // A flat series normalises to one y for every point. Drop the fold and the first three
  // sit at the floor while the last two sit at the ceiling.
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the collapse: ${JSON.stringify(ys)}`,
  );
  // And the collapse is not misread as an accumulating constraint.
  assert(
    !unvetted.classList.contains("rising"),
    "folding a flat inventory raises no bottleneck flag",
  );
});

// Same shape for the second retirement (issue-pr-cron#133/#138): a snapshot written before
// the reject consolidation carries `counts.humanReject` alongside `counts.reject`, counting
// disjoint sets. The quantity `ai:reject` now measures is their SUM at those samples —
// unfolded, the 2026-07-30 migration of 42 PRs would draw as a cliff the inventory never had.
Deno.test("fsm history: retired human-reject samples fold into reject, so the series is continuous across the consolidation", () => {
  const now = Date.parse("2026-07-30T14:01:00Z");
  const at = (d) => now - d * DAY;
  // Real inventory flat at 70 across the migration: split 28 + 42 before, one key after.
  const history = [
    { t: at(4), counts: { reject: 28, humanReject: 42 } },
    { t: at(3), counts: { reject: 28, humanReject: 42 } },
    { t: at(2), counts: { reject: 28, humanReject: 42 } },
    { t: at(1), counts: { reject: 70 } },
    { t: at(0), counts: { reject: 70 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, reject: 70 },
    lanes: { "vetter-verdicts": { "ai:reject": { count: 70, prs: [] } } },
  }, history);
  const reject = collect(box, "fsm-state").find((b) => b.dataset.t === "ai:reject");
  assert(reject, "ai:reject box rendered");
  const line = tags(reject, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) => Number(p.split(",")[1]));
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the consolidation: ${JSON.stringify(ys)}`,
  );
  assert(
    !reject.classList.contains("rising"),
    "folding a flat inventory raises no bottleneck flag",
  );
});

// Same shape a third time (issue-pr-cron#135/#139): a snapshot written before the relink
// retirement carries `counts.relink` beside `counts.reject`; a relink IS a reject with a
// specific note, so the folded series is their SUM and the 2026-07-31 migration of the last
// labelled PR draws no cliff.
Deno.test("fsm history: retired relink samples fold into reject too, so the series is continuous across the second retirement", () => {
  const now = Date.parse("2026-07-31T10:10:00Z");
  const at = (d) => now - d * DAY;
  // Real inventory flat at 70 across the migration: split 28 + 42 before, one key after.
  const history = [
    { t: at(4), counts: { reject: 69, relink: 1 } },
    { t: at(3), counts: { reject: 69, relink: 1 } },
    { t: at(2), counts: { reject: 69, relink: 1 } },
    { t: at(1), counts: { reject: 70 } },
    { t: at(0), counts: { reject: 70 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, reject: 70 },
    lanes: { "vetter-verdicts": { "ai:reject": { count: 70, prs: [] } } },
  }, history);
  const reject = collect(box, "fsm-state").find((b) => b.dataset.t === "ai:reject");
  assert(reject, "ai:reject box rendered");
  const line = tags(reject, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) => Number(p.split(",")[1]));
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the relink retirement: ${JSON.stringify(ys)}`,
  );
  assert(
    !reject.classList.contains("rising"),
    "folding a flat inventory raises no bottleneck flag",
  );
});


// The fold must not INVENT samples: a refresh that carried neither key is still no point,
// not a zero — the pre-existing contract every other state depends on. The rollup is fetched
// from a repo this page does not own, so a line with no `counts` at all is skipped, never
// thrown on.
Deno.test("fsm history: a refresh carrying neither the surviving nor the retired key contributes no point", () => {
  const now = Date.parse("2026-07-29T16:31:36Z");
  const at = (d) => now - d * DAY;
  // The live rollup's real shape: its first 14 lines predate BOTH keys. Plus two malformed
  // ones, because the file is untrusted input.
  const history = [
    { t: at(4), counts: { ready: 5 } },
    { t: at(3) },
    { t: at(2), counts: null },
    { t: at(1), counts: { ready: 5, unvetted: 4, awaitingReVet: 2 } },
    { t: at(0), counts: { ready: 5, unvetted: 6 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 5, closeCandidateIssues: 0, unvetted: 6 },
    lanes: { "vet-lifecycle": { "un-vetted": { count: 6, prs: [] } } },
  }, history);
  const unvetted = collect(box, "fsm-state").find((b) => b.dataset.t === "un-vetted");
  const pts = tags(unvetted, "polyline")[0].getAttribute("points").split(" ");
  assert(pts.length === 2, `only the two carrying refreshes are plotted: ${pts.length}`);
  // Both plotted points are the SUM, so the older one (4+2=6) matches the newer (6) and the
  // line is flat — the fold reads the retired key wherever it appears, not just on a key
  // the newest sample happens to carry.
  const ys = pts.map((p) => Number(p.split(",")[1]));
  assert(ys[0] === ys[1], `4+2 == 6, so the two samples sit level: ${JSON.stringify(ys)}`);
});

// The rollup is an artifact this page does not own, so a count that is not a number is
// skipped exactly as an absent one is. The fold widens that surface — a junk value under the
// RETIRED key would otherwise reach the sum and turn the whole point into NaN, which draws a
// chart with NaN geometry: silence, on the state whose history this change exists to keep.
Deno.test("fsm history: a junk count is skipped like an absent one and cannot poison the folded sum", () => {
  const now = Date.parse("2026-07-29T16:31:36Z");
  const at = (d) => now - d * DAY;
  const history = [
    { t: at(2), counts: { awaitingReVet: "many" } }, // only the retired key, and it is junk
    { t: at(1), counts: { unvetted: 6, awaitingReVet: "many" } },
    { t: at(0), counts: { unvetted: 6 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, unvetted: 6 },
    lanes: { "vet-lifecycle": { "un-vetted": { count: 6, prs: [] } } },
  }, history);
  const unvetted = collect(box, "fsm-state").find((b) => b.dataset.t === "un-vetted");
  const points = tags(unvetted, "polyline")[0].getAttribute("points");
  assert(!points.includes("NaN"), `no NaN reaches the chart geometry: ${points}`);
  const pts = points.split(" ");
  assert(pts.length === 2, `the junk-only refresh contributes no point: ${points}`);
  const ys = pts.map((p) => Number(p.split(",")[1]));
  assert(ys[0] === ys[1], `both samples read 6, so the line is flat: ${JSON.stringify(ys)}`);
});

// #69: the four historically dual-owner states each resolve to ONE actor.
Deno.test("pipeline FSM: ambiguous states resolve to a single owner", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {
      "producer-blocked": {
        "ai:blocked-deploy": { count: 1, prs: [] },
        "ai:blocked-infra": { count: 1, prs: [] },
        "ai:blocked-on": { count: 1, prs: [] },
      },
      "human-decisions": { "human:reject": { count: 1, prs: [] } },
    },
  });
  const groups = ownerGroups(box);
  const owner = (state) => {
    const g = groups.find((g) => g.states.includes(state));
    return g ? g.title : null;
  };
  // human:reject is RETIRED (issue-pr-cron#133/#138: one reject state, ai:reject,
  // whoever ruled). A lanes payload still carrying it — an old snapshot — renders no
  // state row in any group; its history folds into reject via HIST_FOLD.
  assert(
    owner("human:reject") === null,
    `retired human:reject renders in no group: ${owner("human:reject")}`,
  );
  // The three blocked states → human (the actor that actually unblocks each).
  for (const s of ["ai:blocked-deploy", "ai:blocked-infra", "ai:blocked-on"]) {
    assert(
      (owner(s) || "").includes("Human action"),
      `${s} is human-owned: ${owner(s)}`,
    );
  }
});

// ---- pipeline.html: Theory-of-Constraints flow layer (#32) ----
// Per-state inventory sparklines + a red border on any state whose inventory is
// trending UP over the trailing 7 days (the bottleneck signal).

const sevenDaySlope = bind("pipeline.html", "sevenDaySlope", [], []);
const parseHistory = bind("pipeline.html", "parseHistory", [], []);
const DAY = 864e5;

// (c) the 7-day slope logic in isolation: rising / flat / falling / single-point.
Deno.test("fsm trend: a rising 7d series slopes up, flat ≈ 0, falling down, a lone point is null", () => {
  const now = 30 * DAY; // arbitrary anchor well past the epoch
  // one point per day for the 8 days ending at `now` (all inside the trailing-7d window).
  const daily = (vals) =>
    vals.map((v, i) => ({ t: now - (vals.length - 1 - i) * DAY, v }));
  assert(sevenDaySlope(daily([1, 2, 3, 4, 5, 6, 7, 8]), now) > 0.9, "steady rise ≈ +1/day");
  assert(Math.abs(sevenDaySlope(daily([5, 5, 5, 5, 5, 5, 5, 5]), now)) < 1e-9, "flat ≈ 0");
  assert(sevenDaySlope(daily([8, 7, 6, 5, 4, 3, 2, 1]), now) < -0.9, "steady fall ≈ -1/day");
  assert(sevenDaySlope([{ t: now, v: 5 }], now) === null, "single point → null (no trend)");
  assert(sevenDaySlope([], now) === null, "empty series → null");
  // Points whose window is entirely older than 7 days no longer count as a trend.
  const stale = [{ t: now - 20 * DAY, v: 1 }, { t: now - 15 * DAY, v: 9 }];
  assert(sevenDaySlope(stale, now) === null, "all points outside the 7d window → null");
});

// (a) a sparkline renders per state from a history rollup.
Deno.test("fsm sparkline: a state with ≥2 history points renders an inline sparkline (line + endpoint dot)", () => {
  const now = Date.parse("2026-07-25T00:00:00Z");
  const at = (d) => now - d * DAY;
  const history = [6, 5, 7, 8, 9].map((v, i) => ({
    t: at(4 - i),
    counts: { ready: v, reject: 3, leaks: 0 },
  }));
  const hq = {
    counts: { leaks: 0, ready: 9, reject: 3, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 9, prs: [] },
        "ai:reject": { count: 3, prs: [] },
      },
    },
  };
  const box = fsmBox(hq, history);
  const ready = collect(box, "fsm-state").find((b) => b.dataset.t === "ai:ready");
  assert(ready, "ai:ready state box rendered");
  assert(collect(ready, "fsm-spark").length === 1, "one inline sparkline in the state");
  assert(tags(ready, "polyline").length === 1, "sparkline draws a polyline");
  assert(tags(ready, "circle").length === 1, "sparkline has an emphasized endpoint dot");
});

// (a2) a state whose counts key is NEW — carried by only the newest refresh — still gets a
// chart: the lone endpoint dot. This is the real shape of the live rollup whenever the tool
// starts emitting a key (`uncoveredIssues`/"untouched (no PR)" entered `counts` on
// 2026-07-25 and appeared in 1 of 118 history lines), and the state must not silently lose
// its sparkline just because its series is short. No polyline: one vertex paints nothing.
Deno.test("fsm sparkline: a brand-new counts key with ONE history point draws its endpoint dot alone", () => {
  const now = Date.parse("2026-07-26T06:22:06Z");
  const at = (d) => now - d * DAY;
  // 5 refreshes; only the newest carries `uncoveredIssues` — exactly like the live rollup.
  const history = [6, 5, 7, 8, 9].map((v, i) => ({
    t: at(4 - i),
    counts: i === 4 ? { ready: v, uncoveredIssues: 616 } : { ready: v },
  }));
  const hq = {
    counts: { leaks: 0, ready: 9, closeCandidateIssues: 0, uncoveredIssues: 616 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 9, prs: [] } } },
  };
  const box = fsmBox(hq, history);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const untouched = byKey("uncoveredIssues");
  assert(untouched, "untouched (no PR) state box rendered");
  assert(collect(untouched, "fsm-spark").length === 1, "a single-sample state still gets a sparkline");
  assert(tags(untouched, "circle").length === 1, "the single sample renders as an endpoint dot");
  assert(tags(untouched, "polyline").length === 0, "one point draws no line");
  // A single point can't establish a trend, so it is still never flagged as the bottleneck.
  assert(!untouched.classList.contains("rising"), "one sample never flags a bottleneck");
  // Siblings with a full series are untouched by this: line AND dot, exactly as before.
  const ready = byKey("ai:ready");
  assert(tags(ready, "polyline").length === 1, "a multi-sample sibling still draws its line");
  assert(tags(ready, "circle").length === 1, "a multi-sample sibling still draws its dot");
});

// issue-pr-cron#73: the close-candidate ISSUE lifecycle gains a vetter verdict, so the
// single "all flagged issues" figure splits into two inboxes with DIFFERENT owners —
// unvetted (the vetter judges the flag) and upheld (a human closes the issue).
Deno.test("pipeline FSM: close-candidate issues split into a vetter inbox and a human inbox", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 29, closeCandidateUnvetted: 20, closeCandidateUpheld: 9 },
    lanes: {},
  });
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const unvetted = byKey("closeCandidateUnvetted");
  const upheld = byKey("closeCandidateUpheld");
  assert(unvetted, "unvetted close-candidate box rendered");
  assert(upheld, "upheld close-candidate box rendered");
  assert(
    textOf(unvetted).includes("20") && textOf(upheld).includes("9"),
    `each reads its own count: ${textOf(unvetted)} / ${textOf(upheld)}`,
  );
  // Each is exactly one actor's inbox — the whole point of splitting them.
  const groups = ownerGroups(box);
  const owner = (label) => {
    const g = groups.find((g) => g.states.includes(label));
    return g ? g.title : null;
  };
  assert(
    (owner("ai:close-candidate (unvetted)") || "").includes("Vetter action"),
    `unvetted is the vetter's inbox: ${owner("ai:close-candidate (unvetted)")}`,
  );
  assert(
    (owner("ai:close-candidate (upheld)") || "").includes("Human action"),
    `upheld is the human's inbox: ${owner("ai:close-candidate (upheld)")}`,
  );
});

// One flag now produces TWO inboxes with DIFFERENT owners, so their sum is not a state
// anyone acts on. No combined box exists in the lanes view, and each split state counts
// toward its own actor's total — the invariant a combined box could not satisfy.
Deno.test("pipeline FSM: no combined close-candidate box; each split state counts toward its own actor total", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 29, closeCandidateUnvetted: 20, closeCandidateUpheld: 9 },
    lanes: {},
  });
  const groups = ownerGroups(box);
  const all = groups.flatMap((g) => g.states);
  assert(
    !all.includes("ai:close-candidate (issues)"),
    `the combined all-issues box is gone: ${JSON.stringify(all)}`,
  );
  // The heading concatenates title + sub + count, so the total is its trailing number.
  const vetter = groups.find((g) => g.title.includes("Vetter action"));
  const human = groups.find((g) => g.title.includes("Human action"));
  assert(vetter.title.endsWith("20"), `vetter total counts the 20 unvetted: ${vetter.title}`);
  assert(human.title.endsWith("9"), `human total counts the 9 upheld: ${human.title}`);
  // Neither total silently absorbs the 29 that used to be rendered as its own box.
  assert(
    !vetter.title.includes("29") && !human.title.includes("29"),
    `no total carries the combined figure: ${vetter.title} / ${human.title}`,
  );
});

// Rollout ordering: this dashboard change ships BEFORE pr-review-report emits the new
// keys, so live data will not carry them for a while. An absent key must render a zeroed
// box (the machine's whole shape stays visible) with no sparkline — never a broken box.
Deno.test("pipeline FSM: close-candidate split states render zeroed when the tool has not emitted them yet", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 29 },
    lanes: {},
  }, [
    { t: Date.parse("2026-07-25T00:00:00Z"), counts: { closeCandidateIssues: 29 } },
    { t: Date.parse("2026-07-26T00:00:00Z"), counts: { closeCandidateIssues: 29 } },
  ]);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  for (const k of ["closeCandidateUnvetted", "closeCandidateUpheld"]) {
    const b = byKey(k);
    assert(b, `${k} box still rendered with the key absent`);
    // `zero` is baked into the className at construction (only `rising` is added via
    // classList), so read the string the box was built with.
    assert(b.className.includes("zero"), `${k} reads as zero, dimmed`);
    assert(collect(b, "fsm-spark").length === 0, `${k} has no spark with no samples`);
    assert(!b.classList.contains("rising"), `${k} cannot flag a bottleneck with no data`);
  }
});

// The first refresh after the tool starts emitting a split key gives it a single sample.
// Per #129 that must draw its lone endpoint dot rather than vanishing.
Deno.test("pipeline FSM: a freshly-emitted close-candidate split key draws its lone endpoint dot", () => {
  const now = Date.parse("2026-07-26T06:22:06Z");
  const at = (d) => now - d * DAY;
  const history = [0, 1, 2, 3, 4].map((i) => ({
    t: at(4 - i),
    counts: i === 4 ? { closeCandidateIssues: 29, closeCandidateUnvetted: 20 } : { closeCandidateIssues: 29 },
  }));
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 29, closeCandidateUnvetted: 20 },
    lanes: {},
  }, history);
  const unvetted = collect(box, "fsm-state").find((b) => b.dataset.t === "closeCandidateUnvetted");
  assert(collect(unvetted, "fsm-spark").length === 1, "one sample still gets a sparkline");
  assert(tags(unvetted, "circle").length === 1, "the sample renders as an endpoint dot");
  assert(tags(unvetted, "polyline").length === 0, "one point draws no line");
  assert(!unvetted.classList.contains("rising"), "one sample never flags a bottleneck");
});

// (b) the red border applies on an up-trend and NOT on a flat / single-blip series.
Deno.test("fsm trend border: an up-trending state gets the rising warning border + a non-color cue; flat / blip states do not", () => {
  const now = Date.parse("2026-07-25T00:00:00Z");
  const at = (d) => now - d * DAY; // d days before `now`
  // 8 daily samples ending at `now`:
  //   ai:ready   climbs 40→54     (slope ≈ +2/day)          → the constraint, flagged.
  //   ai:reject  flat 10, PLUS the retired relink series (flat 3, lone +1 blip on the last
  //              day) folded in via HIST_FOLD → folded 13..14, slope ≈ 0.11/day, below the
  //              0.15 epsilon → not flagged (anti-flap guard survives the retirement: the
  //              blip rides the fold).
  const ready = [40, 42, 44, 46, 48, 50, 52, 54];
  const relink = [3, 3, 3, 3, 3, 3, 3, 4];
  const history = ready.map((_, i) => ({
    t: at(7 - i),
    counts: { ready: ready[i], reject: 10, relink: relink[i], leaks: 0 },
  }));
  const hq = {
    counts: { leaks: 0, ready: 54, reject: 10, relink: 4, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 54, prs: [] },
        "ai:reject": { count: 10, prs: [] },
        "ai:relink": { count: 4, prs: [] },
      },
    },
  };
  const box = fsmBox(hq, history);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const readyBox = byKey("ai:ready"), rejectBox = byKey("ai:reject"), relinkBox = byKey("ai:relink");
  assert(readyBox.classList.contains("rising"), "up-trending ai:ready gets the rising border");
  assert(!rejectBox.classList.contains("rising"), "the folded blip stays under epsilon, not flagged");
  // (the under-epsilon blip rides reject's FOLDED series above — retired relink renders no box)
  // Never color-alone: a visible ▲ badge + an aria-label spell the flag out.
  assert(collect(readyBox, "fsm-rise").length === 1, "rising state carries a visible ▲ cue");
  assert(
    (readyBox.getAttribute("aria-label") || "").includes("rising"),
    `rising state carries an aria cue: ${readyBox.getAttribute("aria-label")}`,
  );
  // A non-rising state must never ANNOUNCE a trend. It may still LOOK highlighted: ai:reject
  // is producer-owned, that group flags no bottleneck here, so it is legitimately marked as
  // the producer's largest queue — and the fallback wears the bottleneck's exact treatment
  // by design. What must never travel with the mark is the trend CLAIM.
  assert(
    !(rejectBox.getAttribute("aria-label") || "").includes("rising"),
    `a non-rising state never claims a trend: ${rejectBox.getAttribute("aria-label")}`,
  );
  assert(
    !(collect(rejectBox, "fsm-rise")[0]?.getAttribute("title") || "").includes("rising"),
    "the ▲ on a fallback pick does not claim a trend either",
  );
  // ai:relink is fully retired (issue-pr-cron#135/#139): the fixture still carries its
  // lane data and history — an old snapshot — and it must render NO box at all; its
  // history folds into reject's series via HIST_FOLD instead.
  assert(relinkBox === undefined, "retired ai:relink draws no box, even from a snapshot that carries it");
});

// The fallback that keeps every working actor pointed somewhere. `sevenDaySlope` needs two
// samples inside the 7d window AND a positive slope, so a flat or newly-emitted group flags
// no bottleneck at all — and before this, such a group rendered with NOTHING highlighted
// even while holding the biggest pile on the board (the human group, carrying 44 `ready`).
Deno.test("fsm lead fallback: a group with no bottleneck marks its largest non-zero queue instead", () => {
  // No history at all → no sparklines, no slope, so no group can flag a bottleneck.
  const box = fsmBox({
    counts: { leaks: 0, ready: 44, design: 2, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 44, prs: [] },
        "ai:design": { count: 2, prs: [] },
        "ai:reject": { count: 9, prs: [] },
      },
      "vet-lifecycle": { "un-vetted": { count: 3, prs: [] } },
    },
  });
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const ready = byKey("ai:ready");
  assert(ready.classList.contains("lead"), "human's largest queue (44 ai:ready) is marked");
  assert(!ready.classList.contains("rising"), "identical on screen, still distinct in the DOM");
  // The fallback wears the EXISTING highlight, not a second visual language: the same ▲ in
  // the same row, and a `.lead` class whose stylesheet rule is shared verbatim with
  // `.rising`. A reader sees one kind of "look here", never two competing ones.
  const up = collect(ready, "fsm-rise");
  assert(up.length === 1, "the fallback carries the same visible ▲ cue a bottleneck does");
  assert(up[0]._text === "▲", `the same glyph, not a substitute: ${up[0]._text}`);
  assert(
    ready.querySelector(".fsm-scrow").children.includes(up[0]),
    "the ▲ sits in the same row it does on a bottleneck",
  );
  assert(collect(ready, "fsm-lead").length === 0, "no separate chip — nothing new is invented");
  // Only the WORDING differs, because only the wording can lie: this box has no trend.
  assert(
    (ready.getAttribute("aria-label") || "").includes("largest queue"),
    `the fallback states its own reason: ${ready.getAttribute("aria-label")}`,
  );
  assert(
    !(ready.getAttribute("aria-label") || "").includes("rising"),
    "the fallback never claims a trend it does not have",
  );
  // One mark per actor group, and only on that group's own biggest pile.
  assert(!byKey("ai:design").classList.contains("lead"), "a smaller queue in the group is not marked");
  assert(byKey("ai:reject").classList.contains("lead"), "producer's largest queue is marked too");
  assert(byKey("un-vetted").classList.contains("lead"), "vetter's largest queue is marked too");
});

Deno.test("fsm lead fallback: a group that already flags a bottleneck is left alone", () => {
  const now = Date.parse("2026-07-25T00:00:00Z");
  const at = (d) => now - d * DAY;
  // ai:ready climbs 40→54 (≈ +2/day) so the human group flags a bottleneck. ai:design sits
  // flat at a HIGHER count — the fallback must still not fire, and must not steal the mark.
  const ready = [40, 42, 44, 46, 48, 50, 52, 54];
  const history = ready.map((_, i) => ({
    t: at(7 - i),
    counts: { ready: ready[i], design: 99, leaks: 0 },
  }));
  const hq = {
    counts: { leaks: 0, ready: 54, design: 99, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 54, prs: [] },
        "ai:design": { count: 99, prs: [] },
      },
    },
  };
  const box = fsmBox(hq, history);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  assert(byKey("ai:ready").classList.contains("rising"), "the rising flag still fires");
  assert(
    !byKey("ai:ready").classList.contains("lead"),
    "a flagged bottleneck is not also given the fallback mark",
  );
  assert(
    !byKey("ai:design").classList.contains("lead"),
    "99 ai:design is not marked over the flagged bottleneck — it is a fallback, not an addition",
  );
});

Deno.test("fsm lead fallback: an all-zero group gets nothing, and equal counts break toward STATES order", () => {
  // Every human-owned state is zero → nothing to do, so nothing is marked. The producer
  // group holds two EQUAL non-zero queues, which must resolve deterministically.
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, design: 0, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:reject": { count: 7, prs: [] },
        "ai:relink": { count: 7, prs: [] },
      },
      "human-decisions": {
        "human:design": { count: 7, prs: [] },
        "human:close-candidate": { count: 7, prs: [] },
      },
    },
  });
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  assert(!byKey("ai:ready").classList.contains("lead"), "an all-zero group is never marked");
  assert(!byKey("ai:design").classList.contains("lead"), "no work really is nothing to do");
  // ai:relink is fully retired (issue-pr-cron#135/#139): its equal count renders no box
  // and leads nothing — reject leads its group with relink data present but inert.
  assert(byKey("ai:reject").classList.contains("lead"), "reject leads the producer group");
  assert(byKey("ai:relink") === undefined, "retired ai:relink draws no box and leads nothing");
  // Same-group ties still break toward the earlier STATES entry: human-decisions carries
  // two equal states, and human:design precedes human:close-candidate.
  assert(byKey("human:design").classList.contains("lead"), "ties break toward the earlier STATES entry");
  assert(!byKey("human:close-candidate").classList.contains("lead"), "the later of two equal counts is not marked");
});

Deno.test("fsm flow layer: with no history rollup there are no sparklines and no trend borders", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 5, reject: 2, closeCandidateIssues: 0 },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 5, prs: [] },
        "ai:reject": { count: 2, prs: [] },
      },
    },
  });
  assert(collect(box, "fsm-spark").length === 0, "no sparklines without a history rollup");
  assert(
    !collect(box, "fsm-state").some((b) => b.classList.contains("rising")),
    "no trend borders without a history rollup",
  );
});

Deno.test("fsm history: parseHistory keeps well-formed {ts,counts} lines, sorts oldest→newest, drops junk", () => {
  const text = [
    '{"ts":"2026-07-20T00:00:00Z","counts":{"ready":5}}',
    "   ",
    "not json at all",
    '{"ts":"nonsense","counts":{"ready":9}}', // unparseable timestamp → dropped
    '{"ts":"2026-07-18T00:00:00Z","counts":{"ready":3}}',
    '{"ts":"2026-07-19T00:00:00Z"}', // no counts object → dropped
  ].join("\n");
  const pts = parseHistory(text);
  assert(pts.length === 2, `only the two well-formed lines are kept: ${pts.length}`);
  assert(pts[0].t < pts[1].t, "sorted oldest→newest");
  assert(
    pts[0].counts.ready === 3 && pts[1].counts.ready === 5,
    "counts preserved and ordered by time",
  );
});

// ---- rain-org-health#140: OPEN-ISSUE AGE beside the population it measures ----
//
// The recording half (issue-pr-cron#167) emits an OPTIONAL `ages` block beside `counts` —
// snapshot and rollup line alike — over the SAME population `counts.openIssues` counts:
//
//   "counts": { …, "openIssues": 802 },
//   "ages": { "openIssues": { "meanDays": 333.8, "medianDays": 99.0, "oldestDays": 1654.5 } }
//
// That population is EVERY open issue in the pipeline's scope — no coverage filter, no label
// filter, 180 issues wider than the producer backlog (802 vs 617 live) — so the panel draws
// it as its own band above the machine and never inside the `untouched (no PR)` box, whose
// count measures the narrower set. Mean and median are drawn as EQUALS — the same row, the
// same type, and one small trend chart each, on its OWN scale (these quantities move by
// ~1% of their level, so a shared or zero-anchored ruler draws both lines flat); oldest is
// the tail number, in the band's visible detail, never in a tooltip.
//
// Degradation: a snapshot carrying neither the count nor the ages — all 136+ rollup lines
// predating the block, and every refresh whose coverage read FAILED, which omits both rather
// than reporting a population that just went to zero — draws NO band, which is exactly the
// page as it was. Never a fabricated zero, and never a fallback to the retired
// `ages.uncoveredIssues`: that block measured 622 issues and this label says 802.

const agesOf = (mean, median, oldest) => ({
  openIssues: { meanDays: mean, medianDays: median, oldestDays: oldest },
});

// The history fixture the band tests share: one line per refresh, every one carrying the
// population SIZE, only some carrying the ages — the live shape of statistics that start
// mid-history. `stats[i]` is [mean, median, oldest], or null for a refresh that reported none.
function openHistory(now, sizes, stats) {
  return sizes.map((n, i) => {
    const p = { t: now - (sizes.length - 1 - i) * DAY, counts: { openIssues: n } };
    if (stats[i]) p.ages = agesOf(stats[i][0], stats[i][1], stats[i][2]);
    return p;
  });
}

// Band chart geometry, hand-derived from the constants in site/pipeline.html (OI_PAD = 3;
// the size chart is 120x34, each age trend 72x20) rather than read back through the render:
// a series normalized to its own min/max spans [PAD, H - PAD] exactly — [3, 31] for the size
// chart, [3, 17] for an age trend.
const OI_SIZE = { w: 120, h: 34 }, OI_AGE = { w: 72, h: 20 }, OI_PAD = 3;
const bandOf = (box) => collect(box, "fsm-open")[0];
// Every chart is addressed by its OWN class: one quantity per chart is the property under
// test, so a test that found a line by line-class alone could not tell which chart it was in.
const chartOf = (band, which) => collect(band, which)[0];
const ptsOf = (node, cls) => {
  const l = tags(node, "polyline").find((x) => x.className.split(" ").includes(cls));
  return l ? l.getAttribute("points").split(" ").map((p) => p.split(",").map(Number)) : null;
};

Deno.test("fsm ages: parseHistory carries a well-formed ages block and drops a malformed one — without costing the line its counts", () => {
  const text = [
    '{"ts":"2026-07-28T00:00:00Z","counts":{"openIssues":800}}',
    '{"ts":"2026-07-29T00:00:00Z","counts":{"openIssues":801},"ages":{"openIssues":{"meanDays":333.8,"medianDays":99.0,"oldestDays":1654.5}}}',
    '{"ts":"2026-07-30T00:00:00Z","counts":{"openIssues":802},"ages":"not an object"}',
  ].join("\n");
  const pts = parseHistory(text);
  assert(pts.length === 3, `all three lines keep their counts: ${pts.length}`);
  assert(!("ages" in pts[0]), "a line without ages yields a point without the key — absence, not null");
  assert(
    pts[1].ages.openIssues.meanDays === 333.8 &&
      pts[1].ages.openIssues.medianDays === 99.0 &&
      pts[1].ages.openIssues.oldestDays === 1654.5,
    "all three statistics ride through verbatim, as siblings",
  );
  assert(!("ages" in pts[2]), "a malformed ages block is dropped, not forwarded");
  assert(pts[2].counts.openIssues === 802, "…and never invalidates the line's counts");
});

Deno.test("fsm open band: the open-issue population draws its size and BOTH statistics, as equals", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [320, 95, 1620], [333.8, 99.0, 1654.5]]);
  const box = fsmBox(
    { counts: { openIssues: 802, uncoveredIssues: 617 }, lanes: {}, ages: agesOf(333.8, 99.0, 1654.5) },
    history,
  );
  const band = bandOf(box);
  assert(band, "a snapshot carrying the population draws the band");
  // Above the machine: this is the population the machine has to move, and the one figure on
  // the panel that belongs to no actor's inbox.
  assert(box.children[0] === band, "the band leads the panel, ahead of the actor lanes");
  assert(collect(band, "oi-n")[0].textContent === "802", "the population's own size, from counts.openIssues");
  assert(
    textOf(band).includes("every open issue in the pipeline's scope"),
    `the copy says which population this is: "${textOf(band)}"`,
  );
  const stats = collect(band, "oi-stat");
  assert(stats.length === 2, `mean and median, both drawn: ${stats.length}`);
  assert(textOf(stats[0]).includes("mean") && textOf(stats[0]).includes("333.8d"), `mean, unit attached: "${textOf(stats[0])}"`);
  assert(textOf(stats[1]).includes("median") && textOf(stats[1]).includes("99d"), `median, unit attached: "${textOf(stats[1])}"`);
  // EQUALS: same row class, same value class, one trend chart each — neither is a headline
  // and neither is a footnote hidden in a tooltip.
  assert(collect(band, "oi-sv").length === 2, "both values carry the same value class");
  assert(
    collect(band, "oi-mean").length === 1 && collect(band, "oi-med").length === 1,
    "each statistic gets its own trend chart — one is never drawn without the other",
  );
  // The tail number is detail, and VISIBLE detail — a tooltip is unreadable on a touch
  // screen and invisible in a screenshot.
  assert(
    collect(band, "oi-tail")[0].textContent === "oldest 1654.5d",
    `oldest renders as visible text: "${collect(band, "oi-tail").map((n) => n.textContent)}"`,
  );
});

// The reason BOTH statistics are recorded is that they can move apart: a mean running well
// above its median is a long tail doing the ageing. What a CHART can add to two numbers that
// already say that is whether either is MOVING — and these quantities move by ~1% of their
// level (live: a mean of 333.8 days drifting a few days a month), so a shared ruler between
// mean and median, or one anchored at zero, renders both lines flat to within half a pixel
// and shows nothing. Each therefore gets its own chart on its own min/max, which is exactly
// the convention every state sparkline on this panel already uses.
Deno.test("fsm open band: each statistic's trend is drawn on its OWN scale, so a small drift is still visible", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  // The live shape: a mean 3.4x its median, and each drifting by ~1% of its own level.
  const history = openHistory(
    now,
    [800, 801, 802],
    [[330.0, 97.0, 1650], [332.0, 98.0, 1652], [333.8, 99.0, 1654.5]],
  );
  const box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: agesOf(333.8, 99.0, 1654.5) }, history);
  const band = bandOf(box);
  for (const [chart, name] of [["oi-mean", "mean"], ["oi-med", "median"]]) {
    const ys = ptsOf(chartOf(band, chart), "a-l").map((pt) => pt[1]);
    assert(
      Math.min(...ys) === OI_PAD && Math.max(...ys) === OI_AGE.h - OI_PAD,
      `the ${name} trend spans its own full height [${OI_PAD},${OI_AGE.h - OI_PAD}], got [${Math.min(...ys)},${Math.max(...ys)}] — on a ruler set by the OTHER statistic's level it would be a flat smear`,
    );
  }
  // …and the two are drawn identically: same stroke class, same geometry. Nothing in the
  // rendering ranks one above the other.
  const geom = (chart) => {
    const svg = chartOf(band, chart);
    return svg.getAttribute("width") + "x" + svg.getAttribute("height");
  };
  assert(geom("oi-mean") === geom("oi-med"), `same chart geometry for both: ${geom("oi-mean")} vs ${geom("oi-med")}`);
  assert(
    tags(band, "polyline").filter((l) => l.className.split(" ").includes("a-l")).length === 2,
    "both age trends draw in the same stroke — the names tell them apart, not a visual hierarchy",
  );
});

Deno.test("fsm open band: one quantity per chart — no chart mixes days with items", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [320, 95, 1620], [333.8, 99.0, 1654.5]]);
  const box = fsmBox(
    { counts: { openIssues: 802, uncoveredIssues: 617 }, lanes: {}, ages: agesOf(333.8, 99.0, 1654.5) },
    history,
  );
  const band = bandOf(box);
  assert(collect(band, "oi-chart").length === 3, "three quantities, three charts");
  const cls = (n) => tags(n, "polyline").map((l) => l.className);
  for (const [chart, want] of [["oi-size", "q-l"], ["oi-mean", "a-l"], ["oi-med", "a-l"]]) {
    const c = chartOf(band, chart);
    assert(c, `${chart} draws`);
    assert(
      cls(c).length === 1 && cls(c)[0].split(" ").includes(want),
      `${chart} holds exactly one series, its own: ${cls(c)}`,
    );
  }
  assert(cls(chartOf(band, "oi-size")).every((k) => !k.includes("a-l")), "no age line inside the queue-size chart");
  // …and the machine's own sparklines are untouched: no age mark ever enters a state box.
  const b = collect(box, "fsm-state").find((x) => x.dataset.t === "uncoveredIssues");
  assert(collect(b, "sg").length === 0, "no age label in the backlog box");
  assert(
    tags(b, "polyline").every((l) => l.className.split(" ").includes("sl")),
    "the backlog box draws its own count line and nothing else",
  );
});

// THREE charts, ONE time domain: a refresh must land at the same fraction of every chart's
// width, or a series that starts mid-history reads as though it spanned the whole window.
Deno.test("fsm open band: every chart shares one time domain, so a late-starting series starts late", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  // Four refreshes of the size; the ages only on the last two — the live rollout shape.
  const history = openHistory(now, [800, 801, 802, 802], [null, null, [320, 95, 1600], [340, 100, 1654.5]]);
  const box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: agesOf(340, 100, 1654.5) }, history);
  const band = bandOf(box);
  const frac = (chart, cls, w) => {
    const pts = ptsOf(chartOf(band, chart), cls);
    return pts.map(([x]) => (x - OI_PAD) / (w - 2 * OI_PAD));
  };
  const size = frac("oi-size", "q-l", OI_SIZE.w), mean = frac("oi-mean", "a-l", OI_AGE.w);
  assert(Math.abs(size[0]) < 0.02, `the size series starts at the left edge: ${size[0]}`);
  assert(Math.abs(size[size.length - 1] - 1) < 0.02, `…and ends at the right: ${size[size.length - 1]}`);
  assert(
    Math.abs(mean[0] - 2 / 3) < 0.02,
    `the ages start two thirds in, where their first refresh is — not at the left edge of their own private axis: ${mean[0]}`,
  );
  assert(Math.abs(mean[mean.length - 1] - 1) < 0.02, "…and end at the same right edge as the size");
});

Deno.test("fsm open band: the current statistics come from the snapshot, else the newest rollup line that carries them", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [320, 95, 1620], [333.8, 99.0, 1654.5]]);
  // The snapshot is the present and wins, exactly as counts do.
  let band = bandOf(fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: agesOf(400, 120, 1700) }, history));
  assert(textOf(band).includes("400d") && textOf(band).includes("120d"), `snapshot wins: "${textOf(band)}"`);
  // No snapshot block: the newest rollup sample stands in.
  band = bandOf(fsmBox({ counts: { openIssues: 802 }, lanes: {} }, history));
  assert(
    textOf(band).includes("333.8d") && textOf(band).includes("99d") && textOf(band).includes("oldest 1654.5d"),
    `newest rollup sample: "${textOf(band)}"`,
  );
  // A newer refresh whose coverage read failed carries no ages — the newest line that HAS
  // them is the current reading, not "no reading at all".
  const stale = history.concat([{ t: now + DAY, counts: { openIssues: 803 } }]);
  band = bandOf(fsmBox({ counts: { openIssues: 803 }, lanes: {} }, stale));
  assert(textOf(band).includes("333.8d"), `the newest line that carried the ages: "${textOf(band)}"`);
  // Selection is by TIMESTAMP, never by array position: this page does not assume the order
  // it is handed, the same way every series builder sorts before drawing. Reversed, the
  // NEWEST reading is now the array's first element.
  band = bandOf(fsmBox({ counts: { openIssues: 802 }, lanes: {} }, history.slice().reverse()));
  assert(
    textOf(band).includes("333.8d") && !textOf(band).includes("300d"),
    `an out-of-order rollup still reads the newest reading, not the last element: "${textOf(band)}"`,
  );
});

// The 136+ historical rows, and every snapshot predating issue-pr-cron#167.
Deno.test("fsm open band: a snapshot with neither the open count nor any ages draws NO band", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = [
    { t: now - DAY, counts: { uncoveredIssues: 610 } },
    { t: now, counts: { uncoveredIssues: 617 } },
  ];
  const box = fsmBox({ counts: { uncoveredIssues: 617 }, lanes: {} }, history);
  assert(collect(box, "fsm-open").length === 0, "no band — absence degrades to exactly the page as it was");
  assert(collect(box, "oi-k").length === 0, "no population copy about a population the snapshot never reported");
  assert(!textOf(box).includes("median age in days"), "and no legend describing a mark that is not on screen");
  const b = collect(box, "fsm-state").find((x) => x.dataset.t === "uncoveredIssues");
  assert(collect(b, "fsm-spark").length === 1, "the machine's own sparkline still draws");
});

// The retired key measured the producer backlog (622). Reading it into a band labelled
// "every open issue" would show a 622-issue number under an 802-issue heading — the one
// failure mode worse than drawing nothing.
Deno.test("fsm open band: the retired ages.uncoveredIssues is never read as this population's age", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = [
    { t: now - DAY, counts: { uncoveredIssues: 610 }, ages: { uncoveredIssues: { medianDays: 41.0, oldestDays: 812.3 } } },
    { t: now, counts: { uncoveredIssues: 617 }, ages: { uncoveredIssues: { medianDays: 43.0, oldestDays: 813.3 } } },
  ];
  const box = fsmBox(
    { counts: { uncoveredIssues: 617 }, lanes: {}, ages: { uncoveredIssues: { medianDays: 43.0, oldestDays: 813.3 } } },
    history,
  );
  assert(collect(box, "fsm-open").length === 0, "the narrower population's ages are not this one's — no band");
  assert(collect(box, "oi-stat").length === 0, "and no statistic anywhere on the page");
});

// `Number()` coercion would turn a malformed `null`/`""` into a fabricated 0 — for the ages
// a "0d" the recording side refuses to emit, and for the size a claim that every issue in
// the org closed at once, which is why a failed coverage read omits the key entirely.
Deno.test("fsm open band: null, string and negative values render nothing — never a fabricated zero", () => {
  for (const v of [null, "99", -5]) {
    const box = fsmBox(
      { counts: { openIssues: v, uncoveredIssues: 617 }, lanes: {}, ages: { openIssues: { meanDays: v, medianDays: v, oldestDays: v } } },
      [],
    );
    assert(
      collect(box, "fsm-open").length === 0,
      `openIssues=${JSON.stringify(v)} draws no band — a coerced 0 would claim an empty, ageless backlog`,
    );
  }
  // A size that did not arrive alongside statistics that did: the size reads as unreported,
  // never as zero, and the statistics still draw.
  const box = fsmBox({ counts: { openIssues: null, uncoveredIssues: 617 }, lanes: {}, ages: agesOf(333.8, 99.0, 1654.5) }, []);
  assert(collect(box, "oi-n")[0].textContent === "—", `an unreported size is a dash: "${collect(box, "oi-n")[0].textContent}"`);
  assert(collect(box, "oi-stat").length === 2, "…and both statistics still draw");
  // A malformed oldest never costs the two statistics their rows.
  const box2 = fsmBox(
    { counts: { openIssues: 802 }, lanes: {}, ages: { openIssues: { meanDays: 333.8, medianDays: 99.0, oldestDays: "-" } } },
    [],
  );
  assert(collect(box2, "oi-stat").length === 2, "mean and median survive a malformed tail");
  assert(collect(box2, "oi-tail").length === 0, "no fabricated oldest");
});

Deno.test("fsm open band: a refresh that carried no ages contributes no point — a gap is never plotted as zero", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(
    now,
    [800, 801, 802, 802],
    [null, [300, 90, 1600], null, [340, 100, 1654.5]],
  );
  const box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: agesOf(340, 100, 1654.5) }, history);
  const band = bandOf(box);
  const mean = ptsOf(chartOf(band, "oi-mean"), "a-l");
  assert(mean.length === 2, `only the two refreshes that reported a mean are plotted: ${mean.length}`);
  // A fabricated 0 for the missing refresh would be the series' minimum and would pin to the
  // bottom of the chart; with only the two real readings, the LOWER of them is the minimum.
  assert(
    Math.max(...mean.map((pt) => pt[1])) === OI_AGE.h - OI_PAD &&
      mean[0][1] === OI_AGE.h - OI_PAD,
    `the earlier real reading is the series minimum — a gap plotted as 0d would take that place: ${mean.map((pt) => pt[1])}`,
  );
  const size = ptsOf(chartOf(band, "oi-size"), "q-l");
  assert(size.length === 4, `the size was carried on every line, so all four points draw: ${size.length}`);
});

Deno.test("fsm open band: the legend explains the two age series only when they are drawn", () => {
  // A size with no ages at all: the band reports the size and explains no age mark.
  let box = fsmBox({ counts: { openIssues: 802 }, lanes: {} }, []);
  assert(collect(box, "fsm-open").length === 1, "a reported size still draws, ages or not");
  assert(collect(box, "oi-stat").length === 0, "no statistics to draw");
  assert(collect(box, "oi-chart").length === 0, "and no chart: an empty series is no chart, never an empty frame");
  assert(!textOf(box).includes("median age in days"), "…so nothing about them in the legend");
  // An empty block draws nothing either, and explains nothing.
  box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: {} }, []);
  assert(!textOf(box).includes("median age in days"), "an empty ages block gets no legend");
  // Drawable statistics: exactly one sentence, and it names what the reader is looking at.
  box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: agesOf(333.8, 99.0, 1654.5) }, []);
  const txt = textOf(box);
  assert(txt.includes("median age in days") && txt.includes("mean"), `the drawn marks are explained: "${txt}"`);
  assert(
    txt.includes("own trend on its own scale"),
    "…including the one property a reader must know before comparing two charts by eye",
  );
  assert(txt.includes("is that tail"), "…and it names the tail number, which is on screen");
  // A malformed oldest costs the two statistics nothing, draws no tail number — and must
  // therefore cost the legend its tail clause too.
  box = fsmBox(
    { counts: { openIssues: 802 }, lanes: {}, ages: { openIssues: { meanDays: 333.8, medianDays: 99.0, oldestDays: "-" } } },
    [],
  );
  const noTail = textOf(box);
  assert(collect(box, "oi-tail").length === 0, "no tail number is drawn");
  assert(noTail.includes("median age in days"), "the two statistics are still explained");
  assert(!noTail.includes("is that tail"), `…and the legend names no tail that is not there: "${noTail}"`);
});

// The bottleneck flag reads the COUNT trend of the state it is on, and only that: a
// population ageing fast while the producer's backlog holds flat is stagnation, not
// accumulation, and must not trip the rising border.
Deno.test("fsm open band: a fast-ageing population never flags a bottleneck in the machine", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = [10, 20, 30, 40, 50, 60, 70, 80].map((m, i) => ({
    t: now - (7 - i) * DAY,
    counts: { uncoveredIssues: 600, openIssues: 802 },
    ages: agesOf(m * 3, m, 1654.5),
  }));
  const box = fsmBox({ counts: { uncoveredIssues: 600, openIssues: 802 }, lanes: {}, ages: agesOf(240, 80, 1654.5) }, history);
  const b = collect(box, "fsm-state").find((x) => x.dataset.t === "uncoveredIssues");
  assert(!b.classList.contains("rising"), "a flat count is not a bottleneck, however fast the population ages");
  assert(bandOf(box), "the band still draws the ageing it is measuring");
});

// Follow-up to #69: the producer's untouched backlog (open issues with no covering open PR,
// from counts.uncoveredIssues + the top-level uncoveredIssues list) is the biggest bucket of
// its inbox and must surface under Producer action.
Deno.test("pipeline FSM: producer untouched-backlog count + items render under Producer action", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0, uncoveredIssues: 2 },
    lanes: { "vetter-verdicts": {} },
    uncoveredIssues: [
      { repo: "o/x", number: 20, title: "untouched twenty" },
      { repo: "o/y", number: 21, title: "untouched twenty-one" },
    ],
  });
  const producer = ownerGroups(box).find((g) => g.title.includes("Producer action"));
  assert(producer, "a Producer action group exists");
  assert(
    producer.states.includes("untouched (no PR)"),
    `producer owns the untouched-backlog box: ${JSON.stringify(producer.states)}`,
  );
  // Clicking it opens the panel with count == the item list length (rendered as issues).
  const boxByT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  const detail = box.querySelectorAll("#fsmdetail")[0];
  boxByT("uncoveredIssues").click();
  assert(detail.classList.contains("open"), "detail opens");
  assert(
    collect(detail, "dhc")[0].textContent === "2 items",
    "header count == the uncovered-issue list length",
  );
  assert(
    collect(detail, "li").length === 2,
    "renders exactly the 2 untouched issues",
  );
});

// Backward-compat: an older human-queue.json predating the field renders the box at 0, never NaN.
Deno.test("pipeline FSM: missing uncoveredIssues renders the backlog box at 0", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: { "vetter-verdicts": {} },
  });
  const b = box
    .querySelectorAll("[data-t]")
    .find((x) => x.dataset.t === "uncoveredIssues");
  assert(b, "the backlog box still renders when the field is absent");
  assert(
    !textOf(b).includes("NaN"),
    `no NaN when uncoveredIssues is absent: ${textOf(b)}`,
  );
});

// ---- #141: a fromCounts state's click-to-expand ----------------------------
//
// A state is either a LANE state (count and items both from `lanes.<lane>.<state>`) or a
// `fromCounts` state (count from `counts.<key>`, items from a top-level array). Only the lane
// branch ever registered items; three fromCounts states were rescued by a per-key if-chain in
// `itemsFor`, and the two the close-candidate split added were never added to it — so they
// rendered a real count above an empty panel. `closeCandidateUpheld` showed it live: 1 on the
// box, nothing behind it.
//
// The whole existing suite (145 tests) passed with that bug in place, because a state's count
// and its list were only ever asserted apart. These assert them TOGETHER, for every state
// rather than the ones someone remembered to wire.

// Fixture items for a state's top-level array, tagged so one state's list can never be mistaken
// for another's.
function fcItems(tag, n) {
  return Array.from({ length: n }, (_, i) => ({
    repo: "o/" + tag,
    number: i + 1,
    title: tag + " " + (i + 1),
  }));
}

Deno.test("pipeline FSM: every fromCounts state expands to its own items, not an empty panel", () => {
  const box = fsmBox({
    counts: {
      ready: 0,
      leaks: 3,
      uncoveredIssues: 4,
      closeCandidateUnvetted: 2,
      closeCandidateUpheld: 5,
    },
    lanes: { "vetter-verdicts": {} },
    leaks: fcItems("leak", 3),
    uncoveredIssues: fcItems("uncovered", 4),
    closeCandidateUnvetted: fcItems("unvetted", 2),
    closeCandidateUpheld: fcItems("upheld", 5),
  });
  const detail = box.querySelectorAll("#fsmdetail")[0];
  const byKey = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  // state key -> [count it must show, a title only ITS list carries]
  const expected = {
    leak: [3, "leak 1"],
    uncoveredIssues: [4, "uncovered 1"],
    closeCandidateUnvetted: [2, "unvetted 1"],
    closeCandidateUpheld: [5, "upheld 1"],
  };
  for (const [key, [n, marker]] of Object.entries(expected)) {
    const b = byKey(key);
    assert(b, `${key} box rendered`);
    assert(
      collect(b, "sc")[0].textContent === String(n),
      `${key} box reads ${n}: ${collect(b, "sc")[0].textContent}`,
    );
    b.click();
    const rows = collect(detail, "li").length;
    // The bug in one assertion: the box says n, the panel says 0.
    assert(rows === n, `${key} shows ${n} but expands to ${rows} rows`);
    assert(
      collect(detail, "dhc")[0].textContent === n + (n === 1 ? " item" : " items"),
      `${key} header count == its own list length`,
    );
    assert(
      textOf(detail).includes(marker),
      `${key} lists ITS OWN items (looking for ${marker})`,
    );
    b.click(); // collapse before moving on, so nothing reads a stale panel
  }
});

// `leak` is the ONE state whose key and item array disagree: the key is `leak` (kept so the
// pre-existing click-through survives), the items live under `leaks`. Deriving the array name
// from the state key misses silently, and a silent miss renders exactly the empty panel #141 is
// about — so the mapping is pinned, with a decoy array named after the key that must never win.
Deno.test("pipeline FSM: the leak state lists from the top-level `leaks`, not from its state key", () => {
  const box = fsmBox({
    counts: { ready: 0, leaks: 2 },
    lanes: { "vetter-verdicts": {} },
    leaks: [
      { repo: "o/real", number: 1, title: "real leaked PR" },
      { repo: "o/real", number: 2, title: "second leaked PR" },
    ],
    // Named after the STATE, not the array. Nothing may read it.
    leak: [{ repo: "o/decoy", number: 99, title: "decoy from the state key" }],
  });
  const detail = box.querySelectorAll("#fsmdetail")[0];
  box.querySelectorAll("[data-t]").find((b) => b.dataset.t === "leak").click();
  const text = textOf(detail);
  assert(
    collect(detail, "li").length === 2,
    `leak expands to its 2 leaks, got ${collect(detail, "li").length}`,
  );
  assert(text.includes("real leaked PR"), `reads the \`leaks\` array: ${text}`);
  assert(
    !text.includes("decoy"),
    `never reads an array named after the state key: ${text}`,
  );
});

// The reason #141 survived: an empty panel said nothing about WHY it was empty. A genuinely
// empty state and a state whose list was never wired rendered identical silence, so the bug was
// only findable on a state that happened to be non-zero. Three causes, three readings — and the
// two that are defects say so in words, not by colour alone.
Deno.test("pipeline FSM: a zero state reads as deliberately empty; a count with no list reads as a defect", () => {
  const openDetail = (hq, key) => {
    const box = fsmBox(hq);
    const detail = box.querySelectorAll("#fsmdetail")[0];
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === key).click();
    return detail;
  };
  const lanes = { "vetter-verdicts": {} };

  // (a) genuinely empty — counted zero, nothing to list.
  const empty = openDetail(
    { counts: { ready: 0, leaks: 0, closeCandidateUpheld: 0 }, lanes },
    "closeCandidateUpheld",
  );
  const etxt = textOf(empty);
  assert(etxt.includes("Empty"), `a zero state reads as deliberately empty: ${etxt}`);
  assert(
    collect(empty, "miswired").length === 0,
    "a genuinely empty state is never flagged as a defect",
  );

  // (b) #141's exact signature — a real count with no list behind it. Must not read as empty.
  const broken = openDetail(
    { counts: { ready: 0, leaks: 0, closeCandidateUpheld: 7 }, lanes },
    "closeCandidateUpheld",
  );
  const btxt = textOf(broken);
  assert(
    collect(broken, "miswired").length === 1,
    `a count with no list is flagged as a defect: ${btxt}`,
  );
  assert(btxt.includes("7"), `it names the count that has no list: ${btxt}`);
  assert(
    btxt.includes("missing, not empty"),
    `it says which of the two this is: ${btxt}`,
  );
  assert(
    !btxt.includes("Empty — nothing"),
    `it must not also read as deliberately empty: ${btxt}`,
  );
});

// The invariant #141's own Check clause states, asserted over EVERY box the machine draws
// rather than the handful a test happens to name: the number on a box is the number of rows it
// expands to. Lane states and fromCounts states are held to it identically, which is the point
// — they were not, and only the lane half was ever wired.
Deno.test("pipeline FSM: every box's count equals the number of rows it expands to", () => {
  const laneCell = (tag, n) => ({ count: n, prs: fcItems(tag, n) });
  const box = fsmBox({
    counts: {
      // fromCounts states, each with its top-level array below.
      leaks: 3,
      uncoveredIssues: 4,
      closeCandidateUnvetted: 2,
      closeCandidateUpheld: 5,
      // lane states are counted off their own cells, not these.
      ready: 0,
    },
    lanes: {
      "vet-lifecycle": { "un-vetted": laneCell("unvetted-pr", 2) },
      "vetter-verdicts": {
        "ai:ready": laneCell("ready", 6),
        "ai:reject": laneCell("reject", 1),
        // ai:design / ai:relink / ai:close-candidate deliberately absent: a sparse lane must
        // render a zero box that expands to zero rows, which is the same invariant at n = 0.
      },
      "producer-blocked": { "ai:blocked-deploy": laneCell("deploy", 3) },
      // Retired state in the data (old snapshot): must render NO box and NO rows, so it
      // cannot create a count/rows mismatch (issue-pr-cron#133/#138).
      "human-decisions": { "human:reject": laneCell("hreject", 4) },
    },
    leaks: fcItems("leak", 3),
    uncoveredIssues: fcItems("uncovered", 4),
    closeCandidateUnvetted: fcItems("unvetted", 2),
    closeCandidateUpheld: fcItems("upheld", 5),
  });
  const detail = box.querySelectorAll("#fsmdetail")[0];
  const mismatches = [];
  let checked = 0, nonZero = 0;
  for (const b of box.querySelectorAll("[data-t]")) {
    const n = Number(collect(b, "sc")[0].textContent);
    b.click();
    const rows = collect(detail, "li").length;
    checked++;
    if (n > 0) nonZero++;
    if (rows !== n) mismatches.push(`${b.dataset.t}: box ${n}, panel ${rows}`);
    b.click();
  }
  assert(checked === 14, `the whole machine was walked, got ${checked} boxes`);
  // Guard the guard: a fixture that zeroed everything would satisfy the invariant vacuously.
  assert(nonZero >= 8, `the fixture must exercise non-zero states, got ${nonZero}`);
  assert(
    mismatches.length === 0,
    `a count that opens onto a different number of rows:\n  ${mismatches.join("\n  ")}`,
  );
});

// Top-level items are {repo, number, title} with no `url`, so the link is built from repo +
// number. github.com redirects /issues/<n> ↔ /pull/<n>, so the built form resolves for either
// population — which is what makes it safe on `closeCandidateUnvetted`, whose items are a MIX
// of issues and PRs with nothing in the shape saying which. A `url` the tool DOES supply (lane
// items today, these arrays once issue-pr-cron#114 lands) wins outright, so nothing is inferred
// where the answer is already known.
Deno.test("pipeline FSM: a top-level item's link is built from repo + number, and a supplied url wins", () => {
  const box = fsmBox({
    counts: { ready: 0, leaks: 1, uncoveredIssues: 2 },
    lanes: { "vetter-verdicts": {} },
    leaks: [{ repo: "o/p", number: 7, title: "a leaked PR" }],
    uncoveredIssues: [
      { repo: "o/i", number: 8, title: "an untouched issue" },
      { repo: "o/i", number: 9, url: "https://github.com/o/i/pull/9", title: "already resolved" },
    ],
  });
  const detail = box.querySelectorAll("#fsmdetail")[0];
  const open = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k).click();

  // Leaks are producer PRs.
  open("leak");
  assert(
    collect(detail, "li")[0].href === "https://github.com/o/p/pull/7",
    `a leaked PR links to /pull/: ${collect(detail, "li")[0].href}`,
  );

  // Untouched work is issues — except where the item already carries its own url.
  open("uncoveredIssues");
  const hrefs = collect(detail, "li").map((a) => a.href);
  assert(
    hrefs[0] === "https://github.com/o/i/issues/8",
    `an untouched issue links to /issues/: ${hrefs[0]}`,
  );
  assert(
    hrefs[1] === "https://github.com/o/i/pull/9",
    `a supplied url is used verbatim, not rebuilt: ${hrefs[1]}`,
  );
});

// The snapshot is fetched from a repo this page does not own and every field in it is
// untrusted. A malformed entry must not throw out of the row loop — that would drop the rest of
// the list on the floor — and must not be silently skipped either, because a skipped row puts
// the header count and the visible rows back into the disagreement #141 is about.
Deno.test("pipeline FSM: a malformed item renders as a malformed row, taking no other row with it", () => {
  const box = fsmBox({
    counts: { ready: 0, leaks: 0, uncoveredIssues: 4 },
    lanes: { "vetter-verdicts": {} },
    uncoveredIssues: [
      { repo: "o/i", number: 1, title: "first good" },
      null,
      "not an object",
      { repo: "o/i", number: 2, title: "last good" },
    ],
  });
  const detail = box.querySelectorAll("#fsmdetail")[0];
  box.querySelectorAll("[data-t]").find((b) => b.dataset.t === "uncoveredIssues").click();
  const text = textOf(detail);
  // The row AFTER the malformed ones still rendered — i.e. nothing threw partway through.
  assert(text.includes("first good"), `the row before the malformed ones survives: ${text}`);
  assert(text.includes("last good"), `the row AFTER the malformed ones survives: ${text}`);
  // Four entries in, four rows out: the header count and the rows still agree.
  assert(
    collect(detail, "li").length === 4,
    `every entry gets a row, malformed included: ${collect(detail, "li").length}`,
  );
  assert(
    collect(detail, "dhc")[0].textContent === "4 items",
    `header still counts them all: ${collect(detail, "dhc")[0].textContent}`,
  );
  assert(
    (text.match(/malformed entry/g) || []).length === 2,
    `each malformed entry says so: ${text}`,
  );
  // A malformed row is not a link — there is no subject to link to.
  assert(
    tags(detail, "a").length === 2,
    `only the two well-formed rows are links: ${tags(detail, "a").length}`,
  );
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
            role: "V4 authoriser clone",
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

// ---- deployments.html: authoriser role grants (#143) ----

// Two chains mid-rollout: every pinned grant is live on base, and only the
// Safe's are live on ethereum — the shape the deploy repo's own model produces
// while a chain's provisioning bundle is still pending.
const BASE_SAFE = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
const ETH_SAFE = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
const SERVICE_EOA = "0x1c66D6708914C40239D54919320b4C48cAE3D1A9";
const safeRole = (role, admin) => ({
  role,
  admin,
  chains: [
    { network: "base", address: BASE_SAFE, status: "granted" },
    { network: "ethereum", address: ETH_SAFE, status: "granted" },
  ],
});
const serviceRole = (role) => ({
  role,
  admin: false,
  chains: [
    { network: "base", address: SERVICE_EOA, status: "granted" },
    { network: "ethereum", address: SERVICE_EOA, status: "missing" },
  ],
});
function grantsData(over = {}) {
  return {
    // The owner groups are present, so these drive the SAME path the live page
    // takes. The owners-unavailable path gets its own test below.
    deploymentOwners: OWNERS.deploymentOwners,
    deploymentGrants: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      source: "src/lib/LibAuthoriserInvariants.sol",
      function: "expectedGrants(address)",
      pinnedCount: 5,
      declaredCount: 5,
      chains: [
        {
          network: "base",
          authoriser: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
          safe: BASE_SAFE,
          rpcHost: "mainnet.base.org",
          granted: 5,
          missing: 0,
          unknown: 0,
          total: 5,
          state: "live",
        },
        {
          network: "ethereum",
          authoriser: "0x66566cc91dEAf818859bD4b09B7903ac48998157",
          safe: ETH_SAFE,
          rpcHost: "ethereum-rpc.publicnode.com",
          granted: 2,
          missing: 3,
          unknown: 0,
          total: 5,
          state: "partial",
        },
      ],
      grantees: [
        {
          ident: "tokenOwnerSafe",
          kind: "safe",
          address: null,
          roles: [safeRole("DEPOSIT_ADMIN", true), safeRole("DEPOSIT", false)],
        },
        {
          ident: "GRANTEE_SERVICE_1C66",
          kind: "constant",
          address: SERVICE_EOA,
          roles: ["DEPOSIT", "WITHDRAW", "CERTIFY"].map(serviceRole),
        },
      ],
      ...over,
    },
  };
}
// The h3 section headings the grants block emits, in order.
const grantSections = (box) =>
  collect(box, "tok-h3").map((h) => h.textContent);
// Every chip label under the grants block, as text.
const chipText = (box) => collect(box, "own-chip").map((c) => c.textContent);

Deno.test("deployments: each grantee lists its own roles and per-chain live status", () => {
  const box = deploymentsBox(grantsData());
  const roles = collect(box, "own-role").map((r) => r.textContent);
  assert(
    roles.includes("GRANTEE_SERVICE_1C66"),
    "the service key is named by its constant: " + roles.join(","),
  );
  assert(roles.includes("tokenOwnerSafe"), "so is the Safe slot");
  // Its address is rendered as a link to the explorer, not just as text.
  const addr = collect(box, "own-addr").find((a) =>
    a.textContent === SERVICE_EOA
  );
  assert(
    addr && addr.href === "https://basescan.org/address/" + SERVICE_EOA,
    "the service EOA links to the explorer, got " + (addr && addr.href),
  );
  // All three action roles appear as keys, each with a chip per chain. The role
  // key is its OWN class: the shared one is a fixed 78px that does not shrink,
  // and a name like SCHEDULE_CORPORATE_ACTION_ADMIN overflows it straight over
  // the chips beside it.
  const keys = collect(box, "grt-role").map((k) => k.textContent);
  for (const r of ["DEPOSIT", "WITHDRAW", "CERTIFY"]) {
    assert(keys.includes(r), "role " + r + " listed, got " + keys.join(","));
  }
  assert(
    collect(box, "bcn-key").every((k) => !/^[A-Z_]+$/.test(k.textContent || "")),
    "no role name is rendered into the narrow fixed-width key",
  );
  const chips = chipText(box);
  assert(chips.includes("base ✓"), "granted on base");
  assert(chips.includes("ethereum ○"), "not yet granted on ethereum");
  // The per-grantee tally counts the chain cells, not the roles.
  assert(chips.includes("3/6 live"), "service key 3 of 6 cells: " + chips.join(","));
});

// The distinction the whole section turns on. A chain whose provisioning has
// not run yet is REPORTING A ROLLOUT, and a red row there would train the reader
// to ignore the one time it means a key really lost its grant.
Deno.test("deployments: a grant not yet live on a chain reads as a rollout, never as a fault", () => {
  const box = deploymentsBox(grantsData());
  const rollChips = collect(box, "own-chip-roll").map((c) => c.textContent);
  assert(
    rollChips.filter((t) => t === "ethereum ○").length === 3,
    "three not-yet-granted cells carry the rollout chip: " + rollChips.join(","),
  );
  assert(
    !chipText(box).some((t) => t === "ethereum ✗"),
    "nothing renders the not-granted state as a failure mark",
  );
  const no = collect(box, "own-chip-no");
  assert(no.length === 0, "no red chip anywhere in a healthy rollout");
  // The chain banner follows the same rule.
  assert(
    collect(box, "own-verify-roll").length === 1,
    "the partial chain gets the rollout banner",
  );
  assert(
    collect(box, "own-verify-drift").length === 0,
    "and NOT the drift banner",
  );
  const roll = collect(box, "own-verify-roll")[0];
  assert(
    textOf(roll).includes("2 of 5 pinned grants live") &&
      textOf(roll).includes("not provisioned on this chain yet"),
    "the banner says what is actually true: " + textOf(roll),
  );
  assert(
    collect(box, "own-verify-ok").map(textOf).some((t) =>
      t.includes("base — all 5 pinned grants live")
    ),
    "the finished chain reads as done",
  );
});

// With no chain resolved there is nothing to be live on. A green "0/0 live"
// would read as verified — the same unread-looks-confirmed reading the rest of
// this section refuses.
Deno.test("deployments: a grantee with no chain resolved reads as unchecked, not as fully live", () => {
  const d = grantsData();
  d.deploymentGrants.chains = [];
  for (const gr of d.deploymentGrants.grantees) {
    for (const r of gr.roles) r.chains = [];
  }
  const box = deploymentsBox(d);
  const tallies = collect(box, "own-chip").filter((c) =>
    c.textContent === "not checked" || /^\d+\/\d+ live$/.test(c.textContent)
  );
  assert(
    tallies.length > 0 &&
      tallies.every((c) => c.textContent === "not checked"),
    "every tally reads unchecked, got " +
      tallies.map((c) => c.textContent).join(","),
  );
  // Colour as well as words: a green chip reading "not checked" is the same
  // confusion in the channel a reader scans first.
  assert(
    tallies.every((c) => !c.className.split(" ").includes("own-chip-yes")),
    "and none of them is painted as confirmed: " +
      tallies.map((c) => c.className).join(","),
  );
  // …and with no chain there is no per-chain cell to claim one either.
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  assert(
    !chips.some((t) => /^(base|ethereum) /.test(t || "")),
    "no per-chain cell is rendered: " + chips.join(","),
  );
});

// A partly-live chain's remainder is two different things. Reporting only the
// provisioning half prints "0 not provisioned" over a grant that was never read
// — the conflation this whole section exists to prevent, restated as a count.
Deno.test("deployments: a partly-live chain names the unread grants apart from the unprovisioned ones", () => {
  const d = grantsData();
  d.deploymentGrants.chains[1] = {
    ...d.deploymentGrants.chains[1],
    granted: 4,
    missing: 0,
    unknown: 1,
    total: 5,
    state: "partial",
  };
  const roll = collect(deploymentsBox(d), "own-verify-roll").map(textOf)[0] ||
    "";
  assert(
    roll.includes("1 could not be read"),
    "the unread grant is counted and named: " + roll,
  );
  assert(
    !roll.includes("0 not provisioned"),
    "and a zero provisioning gap is not asserted over it: " + roll,
  );
});

// The grant map is parsed independently of the owner constants, and it is the
// half of this page that says who can move value — so it must survive the
// owners read failing rather than disappearing with it.
Deno.test("deployments: the grants section renders even when the owner constants could not be read", () => {
  const d = grantsData();
  d.deploymentOwners = null;
  const box = deploymentsBox(d);
  assert(collect(box, "empty").length === 1, "owners report themselves absent");
  assert(
    collect(box, "own-role").some((r) =>
      r.textContent === "GRANTEE_SERVICE_1C66"
    ),
    "and the grants still render",
  );
});

// admin-vs-action splits the ROLES, not the principals: the Safe holds both, so
// it appears in both tables. A top-level split of the page would have to file it
// twice or misfile it once.
Deno.test("deployments: admin and action roles split into their own tables, and the Safe is in both", () => {
  const box = deploymentsBox(grantsData());
  assert(
    grantSections(box).length === 2 &&
      grantSections(box)[0].startsWith("Action roles") &&
      grantSections(box)[1].startsWith("Admin roles"),
    "action first, admin second: " + grantSections(box).join(" | "),
  );
  const safeRows = collect(box, "own-role").filter((r) =>
    r.textContent === "tokenOwnerSafe"
  );
  assert(safeRows.length === 2, "the Safe is in both tables, got " + safeRows.length);
  const svcRows = collect(box, "own-role").filter((r) =>
    r.textContent === "GRANTEE_SERVICE_1C66"
  );
  assert(svcRows.length === 1, "the service key holds no admin role, so one row");
  // The admin role itself is only under the admin heading.
  const keys = collect(box, "grt-role").map((k) => k.textContent);
  assert(
    keys.filter((k) => k === "DEPOSIT_ADMIN").length === 1,
    "DEPOSIT_ADMIN listed once",
  );
});

// The Safe's address is a per-chain deploy artifact. One address on the row
// would be wrong on every chain but one, and the row is the audit trail for
// which address was actually asked.
Deno.test("deployments: the Safe grantee shows the address it was checked at on each chain", () => {
  const box = deploymentsBox(grantsData());
  const addrs = collect(box, "own-addr");
  const base = addrs.find((a) => a.textContent === BASE_SAFE);
  const eth = addrs.find((a) => a.textContent === ETH_SAFE);
  assert(base && eth, "both chains' Safe addresses render");
  assert(
    base.href === "https://basescan.org/address/" + BASE_SAFE,
    "base Safe → basescan, got " + base.href,
  );
  assert(
    eth.href === "https://etherscan.io/address/" + ETH_SAFE,
    "ethereum Safe → etherscan, got " + eth.href,
  );
  const nets = collect(box, "own-net").map((n) => n.textContent);
  assert(
    nets.includes("base") && nets.includes("ethereum"),
    "each Safe address is tagged with the chain it belongs to",
  );
  // A service key is ONE address covering every chain. Printing it once per
  // chain — which is what keying the address list by chain does — reads as two
  // different keys holding the same roles, i.e. exactly the miscount this
  // section exists to prevent.
  const svc = collect(box, "own-addr").filter((a) =>
    a.textContent === SERVICE_EOA
  );
  assert(
    svc.length === 1,
    "the service key's one address renders once, got " + svc.length,
  );
});

// A read that did not happen is not a grant that was revoked. The two look
// identical on a row that only knows "not granted", and only one of them is a
// reason to move a key.
Deno.test("deployments: an unread chain says why, and never reads as revoked", () => {
  const d = grantsData();
  const g = d.deploymentGrants;
  g.chains[1] = {
    network: "hyperevm",
    authoriser: null,
    safe: null,
    rpcHost: null,
    granted: 0,
    missing: 0,
    unknown: 5,
    total: 5,
    state: "unknown",
  };
  for (const gr of g.grantees) {
    for (const r of gr.roles) {
      r.chains[1] = { network: "hyperevm", address: null, status: "unknown" };
    }
  }
  const box = deploymentsBox(d);
  const down = collect(box, "own-verify-down");
  assert(down.length === 1, "the unread chain gets the dimmed banner");
  assert(
    textOf(down[0]).includes("no authoriser pinned for this chain yet") &&
      textOf(down[0]).includes("unread, not revoked"),
    "it says why and refuses the revoked reading: " + textOf(down[0]),
  );
  const chips = chipText(box);
  assert(chips.includes("hyperevm ?"), "unread cells are a question, not a cross");
  assert(
    !chips.includes("hyperevm ○") && !chips.includes("hyperevm ✗"),
    "an unread cell is neither a rollout gap nor a fault",
  );
  assert(
    !collect(box, "own-chip-roll").some((c) =>
      (c.textContent || "").includes("hyperevm")
    ),
    "no cell on the unread chain claims to be a known gap",
  );
  // The grantee tally counts only what was actually read as granted, so an
  // unread chain lowers the ratio rather than inflating or hiding it.
  assert(chips.includes("3/6 live"), "3 of 6 cells confirmed: " + chips.join(","));
});

Deno.test("deployments: a grant map that parsed short says so before any count is read", () => {
  const box = deploymentsBox(grantsData({ declaredCount: 9 }));
  const drift = collect(box, "own-verify-drift");
  assert(drift.length === 1, "a short parse is drift, not a rollout");
  const msg = drift[0].textContent || "";
  assert(
    msg.includes("9 grants declared") && msg.includes("5 parsed") &&
      msg.includes("INCOMPLETE"),
    "it names both numbers: " + msg,
  );
});

Deno.test("deployments: with no grant data the section is absent and the rest of the page survives", () => {
  const box = deploymentsBox({ ...OWNERS, deploymentGrants: null });
  assert(
    !collect(box, "tok-h3").some((h) => h.textContent.startsWith("Action roles")),
    "no grants section",
  );
  assert(collect(box, "own-group").length === 4, "the owner groups still render");
  // …and an empty grantee list is the same: nothing, rather than a table that
  // would read as "no key holds these roles".
  const empty = deploymentsBox(grantsData({ grantees: [] }));
  assert(
    collect(empty, "tok-h3").length === 0,
    "an empty grantee list renders no section at all",
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
// The boot/ttl split (rainlanguage/issue-pr-cron#84) and the null-safe
// startupPct reader that lets a PARTIAL record — one appended mid-run, before
// toolCalls exist — through the renderers without a `.toFixed` on undefined.
const startupPctOfReal = bind("metrics.html", "startupPctOf", [], []);
const bootMinReal = bind("metrics.html", "bootMin", [], []);
const ttlMinReal = bind("metrics.html", "ttlMin", [], []);
const collapseRunsReal = bind("metrics.html", "collapseRuns", [], []);
// Shared by the tooltip and the caption, so the two can never disagree about
// whether a run succeeded.
const outcomeWordReal = bind("metrics.html", "outcomeWord", [], []);
// The skip discriminant and the runs/skips partition (usage-gate pauses),
// bound from the page so the tests exercise the real predicate, not a copy.
const isSkipReal = bind("metrics.html", "isSkip", [], []);
const pmPartitionReal = bind("metrics.html", "pmPartition", ["isSkip"], [
  isSkipReal,
]);
// The whole runs.jsonl → charted-records pipeline: parse, filter, collapse,
// sort. Bound with the real collapseRuns and isSkip so the filter and the
// collapse are exercised together, which is how they run.
const pmRecordsReal = bind(
  "metrics.html",
  "pmRecords",
  ["collapseRuns", "isSkip"],
  [collapseRunsReal, isSkipReal],
);

function pmBind(name, boxId, pmMode, extraParams = [], extraValues = []) {
  const box = makeEl("div");
  const $ = (id) => (id === boxId ? box : makeEl("div"));
  const document = { createElement: (t) => makeEl(t) };
  const fn = bind(
    "metrics.html",
    name,
    [
      "$",
      "document",
      "pmMode",
      "startupMin",
      "median",
      "startupPctOf",
      "bootMin",
      "ttlMin",
      ...extraParams,
    ],
    [
      $,
      document,
      pmMode,
      startupMinReal,
      medianReal,
      startupPctOfReal,
      bootMinReal,
      ttlMinReal,
      ...extraValues,
    ],
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
    textOf(box).includes("No modernization debt"),
    "an empty summary should say so: " + textOf(box),
  );
});

// The producer runs every 4 hours, so a date-only axis label cannot identify
// which of six daily runs a point is. These render the REAL chart and assert on
// the nodes it emits — asserting on the formatter alone would pass even if the
// chart never called it, which is exactly how a renderer drifts from its helper
// unnoticed. The chart builds SVG nodes, not markup, so the assertions read the
// tree (tag, class, attribute, textContent) rather than a serialised string.
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

function pmDeps() {
  const parseRunId = bind("metrics.html", "parseRunId", [], []);
  return {
    parseRunId,
    fmtDayTime: bind("metrics.html", "fmtDayTime", ["MON", "parseRunId"], [
      MON,
      parseRunId,
    ]),
    fmtRunTime: bind("metrics.html", "fmtRunTime", ["MON", "parseRunId"], [
      MON,
      parseRunId,
    ]),
    startupMin: bind("metrics.html", "startupMin", [], []),
    plotMin: bind("metrics.html", "plotMin", [], []),
    totalMin: bind("metrics.html", "totalMin", [], []),
    niceStep: bind("metrics.html", "niceStep", [], []),
    startupPctOf: startupPctOfReal,
    bootMin: bootMinReal,
    ttlMin: ttlMinReal,
  };
}

// Bind the real pmTip and run it, returning the box the fragment landed in.
function pmTipBox(run, abs) {
  const d = pmDeps();
  const document = stubDocument();
  const fn = bind(
    "metrics.html",
    "pmTip",
    [
      "document",
      "fmtRunTime",
      "startupMin",
      "plotMin",
      "startupPctOf",
      "bootMin",
      "ttlMin",
      "outcomeWord",
    ],
    [
      document,
      d.fmtRunTime,
      d.startupMin,
      d.plotMin,
      d.startupPctOf,
      d.bootMin,
      d.ttlMin,
      outcomeWordReal,
    ],
  );
  const box = makeEl("div");
  box.replaceChildren(fn(run, abs));
  return box;
}

// Bind the real pmSkipTip and run it, returning the box the fragment landed in.
function pmSkipTipBox(run) {
  const d = pmDeps();
  const document = stubDocument();
  const fn = bind("metrics.html", "pmSkipTip", ["document", "fmtRunTime"], [
    document,
    d.fmtRunTime,
  ]);
  const box = makeEl("div");
  box.replaceChildren(fn(run));
  return box;
}

// Render the real chart; returns the wrap element it built into. `skips` is the
// partition's other half — skipped ticks, drawn as events on the timeline.
function pmChart(runs, pmMode = "pct", skips = []) {
  const wrap = makeEl("div");
  const $ = (id) => (id === "pmwrap" ? wrap : makeEl("div"));
  const d = pmDeps();
  const document = stubDocument();
  const pmTip = bind(
    "metrics.html",
    "pmTip",
    [
      "document",
      "fmtRunTime",
      "startupMin",
      "plotMin",
      "startupPctOf",
      "bootMin",
      "ttlMin",
      "outcomeWord",
    ],
    [
      document,
      d.fmtRunTime,
      d.startupMin,
      d.plotMin,
      d.startupPctOf,
      d.bootMin,
      d.ttlMin,
      outcomeWordReal,
    ],
  );
  const pmSkipTip = bind("metrics.html", "pmSkipTip", [
    "document",
    "fmtRunTime",
  ], [
    document,
    d.fmtRunTime,
  ]);
  bind(
    "metrics.html",
    "renderPmChart",
    [
      "$",
      "document",
      "pmMode",
      "pmTip",
      "pmSkipTip",
      "isSkip",
      "parseRunId",
      "fmtDayTime",
      "fmtRunTime",
      "startupMin",
      "plotMin",
      "totalMin",
      "niceStep",
      "startupPctOf",
      "bootMin",
      "ttlMin",
    ],
    [
      $,
      document,
      pmMode,
      pmTip,
      pmSkipTip,
      isSkipReal,
      d.parseRunId,
      d.fmtDayTime,
      d.fmtRunTime,
      d.startupMin,
      d.plotMin,
      d.totalMin,
      d.niceStep,
      d.startupPctOf,
      d.bootMin,
      d.ttlMin,
    ],
  )(runs, skips);
  return wrap;
}

// The chart's own <svg>, and the axis text nodes carrying a date label.
const pmSvg = (wrap) => wrap.children.find((c) => c.tagName === "svg");
const pmTexts = (wrap) => tags(wrap, "text").map((t) => t.textContent || "");

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
  const labels = pmTexts(pmChart([RUN_A, RUN_B]));
  const shown = JSON.stringify(labels);
  assert(
    labels.some((t) => t.includes("Jul 20")),
    "expected the date: " + shown,
  );
  assert(
    labels.some((t) => t.includes("01:00")),
    "expected the first run's time: " + shown,
  );
  assert(
    labels.some((t) => t.includes("17:00")),
    "expected the last run's time: " + shown,
  );
  assert(
    labels.some((t) => t.includes("UTC")),
    "expected an explicit zone: " + shown,
  );
});

Deno.test("metrics chart: two runs on the same day get distinct axis labels", () => {
  // Before absolute times both endpoints rendered as the bare date "Jul 20".
  const labels = pmTexts(pmChart([RUN_A, RUN_B])).filter((t) =>
    t.includes("Jul 20")
  );
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
  const labels = pmTexts(pmChart([RUN_A]));
  const shown = JSON.stringify(labels);
  assert(
    labels.some((t) => t.includes("01:00")),
    "single-run axis should carry its time: " + shown,
  );
  assert(
    labels.some((t) => t.includes("UTC")),
    "single-run axis should carry the zone: " + shown,
  );
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

// Startup is a PART of total run time, so the two share the minutes axis and
// total is never below startup. These pin the properties that would silently
// mislead: a total clipped by a startup-only scale, and a "total" line in
// proportion mode where it would be a flat, meaningless 100%.
const RUN_LONG = {
  runId: "20260720T010001Z",
  startupPct: 4.3,
  startupMs: 590693,
  durationMs: 1611124,
  toolCalls: 529,
  startupToolCalls: 23,
  numTurns: 66,
  outcome: "ok",
};
const RUN_LONG2 = {
  runId: "20260720T170002Z",
  startupPct: 61.5,
  startupMs: 700000,
  durationMs: 4200000,
  toolCalls: 431,
  startupToolCalls: 200,
  numTurns: 128,
  outcome: "ok",
};

Deno.test("metrics chart: absolute mode plots total run time alongside startup", () => {
  const wrap = pmChart([RUN_LONG, RUN_LONG2], "abs");
  assert(collect(wrap, "pm-total").length > 0, "expected a total-run series");
  assert(
    collect(wrap, "pm-line").length > 0,
    "startup series should still be drawn",
  );
});

Deno.test("metrics chart: proportion mode omits total, which would be a flat 100%", () => {
  const wrap = pmChart([RUN_LONG, RUN_LONG2], "pct");
  assert(
    collect(wrap, "pm-total").length === 0,
    "total as a proportion is always 100% and says nothing — it must not render",
  );
});

Deno.test("metrics chart: the y scale covers total, so it is never clipped", () => {
  // 4200000ms = 70min total vs 11.7min startup. A startup-only scale would top
  // out near 12 and push the total line off the plot.
  const wrap = pmChart([RUN_LONG, RUN_LONG2], "abs");
  const ticks = tags(wrap, "text")
    .filter((t) => t.getAttribute("text-anchor") === "end")
    .map((t) => parseFloat(t.textContent))
    .filter((v) => !isNaN(v));
  const top = Math.max(...ticks);
  assert(
    top >= 70,
    "y axis must reach the largest total (70m), got top tick " + top,
  );
});

Deno.test("metrics chart: two series carry a legend, one series does not", () => {
  const wrap = pmChart([RUN_LONG, RUN_LONG2], "abs");
  const legend = collect(wrap, "pm-legend");
  assert(legend.length === 1, "two series require a legend");
  const text = textOf(legend[0]);
  assert(
    text.includes("total run") && text.includes("startup"),
    "legend must name both series: " + text,
  );
  assert(
    collect(pmChart([RUN_LONG, RUN_LONG2], "pct"), "pm-legend").length === 0,
    "a single series needs no legend box",
  );
});

// --- boot / ttl split (rainlanguage/issue-pr-cron#84) ------------------------
// `startupMs` fused two costs with nothing in common: boot (nix resolving and
// exec'ing the flake output, the MCP handshake, the prompt load — not
// model-driven) and ttl (orientation before the first productive act — entirely
// model- and tool-surface-driven). They regress for unrelated reasons, so a
// single number that either can wreck is one you cannot act on.
//
// The emitter also appends PARTIAL records mid-run — `stage:"boot"` at the first
// tool call, `stage:"ttl"` at the first productive act — so a run that is killed
// or times out still contributes. Those partials carry no toolCalls, no
// startupPct, no durationMs and no outcome, and the "ttl" one carries no
// startupMs either. Every fixture below is shaped exactly like the emitter's
// output for that reason; a tidied-up fixture would test a record that never
// exists.

// Real figures, measured off the live traces in the issue: boot 1.125 s,
// ttl 297.0 s at tool call 17 (20260728T053610Z).
const RUN_SPLIT = {
  runId: "20260723T010001Z",
  role: "producer",
  stage: "final",
  startupPct: 61.5,
  startupMs: 166600,
  bootMs: 1125,
  ttlMs: 297016,
  durationMs: 4200000,
  toolCalls: 431,
  startupToolCalls: 200,
  numTurns: 128,
  outcome: "ok",
};
const RUN_SPLIT2 = {
  runId: "20260723T050001Z",
  role: "producer",
  stage: "final",
  startupPct: 40.0,
  startupMs: 120000,
  bootMs: 801,
  ttlMs: 180000,
  durationMs: 3000000,
  toolCalls: 300,
  startupToolCalls: 120,
  numTurns: 90,
  outcome: "ok",
};
// What a KILLED run leaves behind: the emitter got as far as the first tool call
// and the first productive act, and never reached the end-of-run record.
const PARTIAL_BOOT = {
  trace: "/runs/20260724T010001Z.jsonl",
  stage: "boot",
  bootMs: 774,
  runId: "20260724T010001Z",
  role: "producer",
  model: "claude-opus-4-8",
};
const PARTIAL_TTL = {
  trace: "/runs/20260724T010001Z.jsonl",
  stage: "ttl",
  bootMs: 774,
  ttlMs: 298141,
  startupToolCalls: 16,
  firstMutationIndex: 16,
  runId: "20260724T010001Z",
  role: "producer",
  model: "claude-opus-4-8",
};

Deno.test("metrics accessors: an absent figure reads null, never undefined", () => {
  // These five accessors are the page's ONE reading of "this run has no such
  // number", and every consumer branches on `== null` / `!= null`. Returning
  // the raw field instead would hand back `undefined` for a partial — which
  // those loose checks happen to swallow today, so nothing would break until
  // the first consumer that uses `=== null`, a strict equality, `??`, or
  // serialises a record. Pinning the contract is what keeps them interchangeable.
  const partial = { stage: "boot", bootMs: 774 };
  assert(startupPctOfReal(partial) === null, "startupPctOf must return null");
  assert(startupMinReal(partial) === null, "startupMin must return null");
  assert(ttlMinReal(partial) === null, "ttlMin must return null");
  // …and a present figure still comes back as a number, in minutes.
  assert(bootMinReal(partial) === 774 / 60000, "bootMin must convert to minutes");
  assert(
    startupPctOfReal({ startupPct: 61.5 }) === 61.5,
    "a real proportion must pass through unchanged",
  );
});

Deno.test("metrics collapse: the complete record wins over its own partials", () => {
  // All three lines share a runId. Plotting them as three runs would triple the
  // point and drag the median toward the partials' half-known values.
  const final = { ...RUN_SPLIT, runId: "20260724T010001Z" };
  const out = collapseRunsReal([PARTIAL_BOOT, PARTIAL_TTL, final]);
  assert(
    out.length === 1,
    "one run must collapse to one record, got " + out.length,
  );
  // Assert on a value ONLY the final record carries — matching on runId alone
  // would pass even if a partial had won.
  assert(
    out[0].toolCalls === 431,
    "the final record must win, got stage=" + out[0].stage,
  );
});

Deno.test("metrics collapse: with no final record the ttl partial survives", () => {
  // The killed-run case. Something must reach the chart, and it must be the
  // most complete thing the run managed to record.
  const out = collapseRunsReal([PARTIAL_BOOT, PARTIAL_TTL]);
  assert(out.length === 1, "expected one collapsed run, got " + out.length);
  assert(
    out[0].stage === "ttl" && out[0].ttlMs === 298141,
    "ttl beats boot: got " + JSON.stringify(out[0]),
  );
});

Deno.test("metrics collapse: a legacy record with no stage outranks a partial", () => {
  // Pre-#84 records carry no `stage` at all. They were written at run end, so
  // absent must rank as final — reading it as "unknown, therefore lowest" would
  // let a partial overwrite a complete historical record.
  const legacy = { ...RUN_LONG, runId: "20260724T010001Z", role: "producer" };
  const out = collapseRunsReal([legacy, PARTIAL_TTL]);
  assert(out.length === 1, "expected one collapsed run, got " + out.length);
  assert(
    out[0].toolCalls === 529,
    "the legacy end-of-run record must win: " + JSON.stringify(out[0]),
  );
});

Deno.test("metrics collapse: distinct runs are never merged", () => {
  const out = collapseRunsReal([RUN_SPLIT, RUN_SPLIT2]);
  assert(out.length === 2, "two runs must stay two records, got " + out.length);
});

Deno.test("metrics collapse: a producer and a vetter run are two runs, not one", () => {
  // The key is (runId, role), not runId. Today `pmRecords` filters to the
  // producer BEFORE collapsing, so the role half never gets to matter — which
  // is exactly why it needs pinning: the producer and the vetter cron can tick
  // in the same second, and the moment anything feeds both roles in (a vetter
  // panel, a combined view) a runId-only key silently merges two different runs
  // into one and drops the other's timings.
  const vetter = { ...PARTIAL_TTL, role: "vetter", ttlMs: 999000 };
  const out = collapseRunsReal([PARTIAL_TTL, vetter]);
  assert(out.length === 2, "one id, two roles, two runs — got " + out.length);
  assert(
    out.some((r) => r.ttlMs === 298141) && out.some((r) => r.ttlMs === 999000),
    "neither run's timings may be dropped: " + JSON.stringify(out),
  );
});

// The runs.jsonl → chart pipeline. These are the tests that pin the KILLED-RUN
// path end to end: the emitter writes partials so a run that dies still leaves
// its timings, and the dashboard has to actually admit them.
const jsonl = (...recs) => recs.map((r) => JSON.stringify(r)).join("\n") + "\n";

Deno.test("metrics records: a killed run's partials reach the chart", () => {
  // The old filter required startupPct, which no partial has — so a run that
  // died contributed nothing at all, and the dashboard reported only successes.
  const out = pmRecordsReal(jsonl(RUN_SPLIT, PARTIAL_BOOT, PARTIAL_TTL));
  assert(
    out.length === 2,
    "expected the finished run AND the killed one, got " + out.length,
  );
  const killed = out.find((r) => r.runId === "20260724T010001Z");
  assert(
    killed,
    "the killed run must be charted: " +
      JSON.stringify(out.map((r) => r.runId)),
  );
  assert(
    killed.ttlMs === 298141 && killed.bootMs === 774,
    "it must carry the timings it recorded: " + JSON.stringify(killed),
  );
});

Deno.test("metrics records: one run never plots as three points", () => {
  // All three lines of one run share a runId; without the collapse the run
  // would be plotted three times and drag the median toward its partials.
  const final = { ...RUN_SPLIT, runId: "20260724T010001Z" };
  const out = pmRecordsReal(jsonl(PARTIAL_BOOT, PARTIAL_TTL, final));
  assert(out.length === 1, "three lines are one run, got " + out.length);
  assert(
    out[0].toolCalls === 431,
    "the final record must win: " + JSON.stringify(out[0]),
  );
});

Deno.test("metrics records: the vetter's runs are not charted here", () => {
  // This panel is the PRODUCER's. A vetter partial carries bootMs too, so the
  // relaxed filter must still discriminate on role.
  const out = pmRecordsReal(
    jsonl({ ...PARTIAL_TTL, role: "vetter" }, RUN_SPLIT),
  );
  assert(
    out.length === 1 && out[0].role === "producer",
    "vetter runs must not leak in: " + JSON.stringify(out),
  );
});

Deno.test("metrics records: a half-written trailing line is skipped, not fatal", () => {
  // runs.jsonl is appended to by a live cron; the last line can be torn.
  const out = pmRecordsReal(jsonl(RUN_SPLIT) + '{"role":"producer","start');
  assert(
    out.length === 1,
    "the intact records must still load, got " + out.length,
  );
});

Deno.test("metrics records: runs come back oldest first", () => {
  const out = pmRecordsReal(jsonl(RUN_SPLIT2, RUN_SPLIT));
  assert(
    out[0].runId === "20260723T010001Z" && out[1].runId === "20260723T050001Z",
    "expected chronological order: " + JSON.stringify(out.map((r) => r.runId)),
  );
});

Deno.test("metrics chart: absolute mode plots boot and ttl as their own series", () => {
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "abs");
  assert(collect(wrap, "pm-boot").length > 0, "expected a boot series");
  assert(collect(wrap, "pm-ttl").length > 0, "expected a ttl series");
  assert(collect(wrap, "pm-total").length > 0, "total must still draw");
  assert(collect(wrap, "pm-line").length > 0, "startup must still draw");
});

Deno.test("metrics chart: proportion mode omits boot and ttl", () => {
  // As a share of the run's tool calls neither half has a meaning — they are
  // wall-clock spans, and there is no call count to divide them by.
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "pct");
  assert(
    collect(wrap, "pm-boot").length === 0,
    "boot must not draw as a proportion",
  );
  assert(
    collect(wrap, "pm-ttl").length === 0,
    "ttl must not draw as a proportion",
  );
});

Deno.test("metrics chart: a pre-split record draws no boot or ttl series", () => {
  // History was recorded under the fused metric. Inventing a zero boot for it
  // would draw a flat line along the axis that no run ever measured.
  const wrap = pmChart([RUN_LONG, RUN_LONG2], "abs");
  assert(
    collect(wrap, "pm-boot").length === 0,
    "a legacy record has no boot to plot",
  );
  assert(
    collect(wrap, "pm-ttl").length === 0,
    "a legacy record has no ttl to plot",
  );
  assert(collect(wrap, "pm-line").length > 0, "its startup series still draws");
});

Deno.test("metrics chart: a ttl longer than startupMs still fits under the y ceiling", () => {
  // boot + ttl is NOT a partition of startupMs and can EXCEED it: the legacy
  // metric starts its clock at the first tool RESULT, so it omits the first
  // call's own latency (~137 s on the MCP vetter surface) while ttl includes it.
  // Scaling the axis over startup + total alone therefore draws the ttl line off
  // the top of the plot, silently. RUN_SPLIT: ttl 297016ms = 4.95min vs
  // startupMs 166600ms = 2.78min. Both durations are shrunk to 3.3min here so
  // that ttl is genuinely the largest value on the chart — with the real 70min
  // total the axis would clear ttl by accident and the test would prove nothing.
  const wrap = pmChart([
    { ...RUN_SPLIT, durationMs: 200000 },
    { ...RUN_SPLIT2, durationMs: 200000 },
  ], "abs");
  const ticks = tags(wrap, "text")
    .filter((t) => t.getAttribute("text-anchor") === "end")
    .map((t) => parseFloat(t.textContent))
    .filter((v) => !isNaN(v));
  const top = Math.max(...ticks);
  assert(
    top >= 4.95,
    "y axis must reach the largest ttl (4.95m), got top tick " + top,
  );
});

Deno.test("metrics legend: names boot and ttl alongside startup and total", () => {
  const text = textOf(
    collect(pmChart([RUN_SPLIT, RUN_SPLIT2], "abs"), "pm-legend")[0],
  );
  for (const label of ["startup", "boot", "ttl", "total run"]) {
    assert(text.includes(label), `legend must name "${label}": ` + text);
  }
});

Deno.test("metrics tip: carries boot and ttl as absolute values", () => {
  // boot is ~1s against a 35-minute axis — a hairline the chart can show MOVING
  // but cannot show the size of. The number has to be readable somewhere.
  const t = textOf(pmTipBox(RUN_SPLIT, true));
  assert(t.includes("1.1s"), "boot 1125ms should read as 1.1s: " + t);
  assert(t.includes("5.0m"), "ttl 297016ms should read as 5.0m: " + t);
});

Deno.test("metrics tip: sub-minute values are seconds, not a rounded 0.0m", () => {
  // The whole point of putting the figure in the tip is that it is legible.
  const t = textOf(pmTipBox(RUN_SPLIT, true));
  assert(!t.includes("boot 0.0m"), "boot must not collapse to 0.0m: " + t);
});

Deno.test("metrics tip: proportion mode omits the split", () => {
  const t = textOf(pmTipBox(RUN_SPLIT, false));
  assert(!t.includes("boot"), "boot has no proportional reading: " + t);
});

Deno.test("metrics tip: a killed run's partial renders its timings, not undefined", () => {
  // THE case the issue is about: "the run you most want timings for is the one
  // that died, and it is exactly the one that leaves none". The ttl partial has
  // no startupPct, no startupMs, no toolCalls, no durationMs and no outcome —
  // every one of which used to be read unconditionally.
  const t = textOf(pmTipBox(PARTIAL_TTL, true));
  assert(!t.includes("undefined"), "no undefined may reach the tip: " + t);
  assert(!t.includes("NaN"), "no NaN may reach the tip: " + t);
  assert(t.includes("5.0m"), "the ttl it did record must show: " + t);
  assert(t.includes("0.8s"), "the boot it did record must show: " + t);
});

Deno.test("metrics tip: an unfinished run does not report itself as ok", () => {
  // A partial has no `outcome`, and the old `r.outcome || "ok"` would print a
  // success for a run that was killed. Read the outcome ELEMENT rather than
  // grepping the whole tip: "ok" is a substring of ordinary words, so a loose
  // search would be satisfied — or broken — by text that is not the outcome.
  const word = collect(pmTipBox(PARTIAL_TTL, true), "deg")[0];
  assert(word, "an unfinished outcome must render in the degraded style");
  assert(
    word.textContent === "unfinished",
    "the outcome element must read exactly unfinished: " + word.textContent,
  );
});

Deno.test("metrics tip: a record with counts but no turn count leaks no undefined", () => {
  // runs.jsonl is an artifact this page does not own, so the shape it does not
  // anticipate is the whole risk — every guard here exists because a field went
  // missing. The three fields share one line and so must share one presence
  // check: guarding two of them and interpolating the third one line below is
  // the same `undefined` leak, just narrower.
  const t = textOf(pmTipBox({
    runId: "20260724T010001Z",
    role: "producer",
    startupPct: 40,
    startupMs: 120000,
    startupToolCalls: 12,
    toolCalls: 30,
    outcome: "ok",
  }, true));
  assert(!t.includes("undefined"), "no undefined may reach the tip: " + t);
  assert(!t.includes("NaN"), "no NaN may reach the tip: " + t);
});

Deno.test("metrics caption: an unfinished latest run is not captioned ok", () => {
  // The caption derives the outcome word from the SAME helper the tooltip uses.
  // Before it was extracted the rule existed twice verbatim, so the chart could
  // say "unfinished" while the sentence under it said "ok" about the same run.
  const word = collect(pmNoteBox(PARTIAL_TTL), "deg")[0];
  assert(word, "an unfinished outcome must render in the degraded style");
  assert(
    word.textContent === "unfinished",
    "the caption must agree with the tooltip: " + word.textContent,
  );
});

Deno.test("metrics tip: a boot-only partial leads with the boot it knows", () => {
  // Killed BETWEEN the boot and ttl stages — boot is all that exists. Leading
  // with ttl regardless would bold "—", putting the one thing this run does not
  // have in the most prominent slot and burying the one thing it does.
  const t = textOf(pmTipBox(PARTIAL_BOOT, true));
  assert(!t.includes("undefined"), "no undefined may reach the tip: " + t);
  assert(!t.includes("NaN"), "no NaN may reach the tip: " + t);
  assert(t.includes("0.8s"), "boot 774ms should read as 0.8s: " + t);
  const lead = tags(pmTipBox(PARTIAL_BOOT, true), "b")[0];
  assert(
    lead && lead.textContent === "0.8s",
    "the bolded lead must be the known boot value, got: " +
      (lead && lead.textContent),
  );
});

Deno.test("metrics chart: a killed run's partial reaches the tooltip on hover", () => {
  // The integration half — the guards in pmTip are worth nothing if the chart
  // drops the partial before it gets there.
  const wrap = pmChart([RUN_SPLIT, PARTIAL_TTL], "abs");
  const svg = pmSvg(wrap);
  const rect = { left: 0, top: 0, width: 720, height: 210 };
  svg._rect = rect;
  wrap._rect = rect;
  svg.fire("mousemove", { clientX: 719, clientY: 10 });
  const t = textOf(collect(wrap, "pm-tip")[0]);
  assert(t.includes("unfinished"), "the partial must be hoverable: " + t);
  assert(!t.includes("undefined"), "no undefined on hover: " + t);
});

Deno.test("metrics controls: boot-only partials still offer absolute mode", () => {
  // A window in which every run was killed has no startupMs anywhere — and
  // those are precisely the runs whose boot and ttl the partials preserved.
  // Gating the toggle on startupMs alone locks the only view that shows them.
  const [render, box] = pmBind("renderPmControls", "pmcontrols", "abs", [
    "pmRuns",
    "renderPmTiles",
    "renderPmChart",
  ], [[], () => {}, () => {}]);
  render([PARTIAL_BOOT, PARTIAL_TTL]);
  assert(
    collect(box, "pm-toggle").length === 1,
    "boot/ttl are absolute data and must keep absolute mode available",
  );
});

Deno.test("metrics tiles: a partial contributes no startup figure", () => {
  // It has none — the run never reached a first productive act's result. A zero
  // would drag the median down and read as an improvement.
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  render([{ startupPct: 10, runId: "a" }, PARTIAL_TTL, {
    startupPct: 20,
    runId: "b",
  }]);
  const t = textOf(box);
  assert(t.includes("15.0%"), "median over [10,20] is 15: " + t);
  assert(!t.includes("undefined"), "no undefined in the tiles: " + t);
  // Tie the count to its OWN tile — a bare "3" matches any digit anywhere,
  // including the "15.0%" two tiles over, so it would pass without the count
  // ever rendering.
  const counted = collect(box, "tile").find((tile) =>
    textOf(tile).includes("runs recorded")
  );
  assert(counted, "expected a runs-recorded tile");
  assert(
    textOf(counted).startsWith("3"),
    "runs recorded still counts every run, gaps included: " + textOf(counted),
  );
});

// --- skipped ticks (usage-gate pauses) ---------------------------------------
// A tick the producer's usage gate PAUSED leaves an event row, not a run.
// `outcome: "skipped"` is the typed discriminant; `skipped` names the gate
// kind and `skipReason` is the gate's PAUSE line verbatim. SKIP_TICK carries
// the gate fields without the typed outcome (the predicate's fallback arm),
// while REAL_SKIP below is the emitter's actual full row shape
// (issue-pr-cron#160/#163), which also carries zeroed run fields. Both shapes
// must partition as events. Historical files carry no skip rows and are not
// back-filled, so the zero-skip rendering is pinned as hard as the marks.
const SKIP_TICK = {
  runId: "20260731T090001Z",
  role: "producer",
  model: "claude-opus-5",
  skipped: "usage-gate",
  skipReason: "PAUSE: weekly usage 92% >= 90% budget until 2026-08-01T00:00Z",
  exitCode: 0,
};
const SKIP_TICK2 = {
  runId: "20260731T130001Z",
  role: "producer",
  model: "claude-opus-5",
  skipped: "usage-gate",
  skipReason: "PAUSE: weekly usage 94% >= 90% budget until 2026-08-01T00:00Z",
  exitCode: 0,
};

// The emitter's real skip row: stage:"final" with every run field present but
// ZEROED, plus the skip pair. Read as a run, startupPct 0.0 drags the median,
// durationMs 0 dives the total line to the baseline, and outcome "skipped"
// paints an errored run — the partition is what stands between the real row
// and all three misreadings.
const REAL_SKIP = {
  trace: "/tmp/empty-trace-160.jsonl",
  stage: "final",
  toolCalls: 0,
  startupToolCalls: 0,
  startupPct: 0.0,
  wakeupCalls: 0,
  firstMutationIndex: null,
  bootMs: null,
  ttlMs: null,
  startupMs: null,
  durationMs: 0,
  numTurns: 0,
  tokensIn: 0,
  tokensOut: 0,
  cacheRead: 0,
  cacheCreation: 0,
  costUsd: 0.0,
  runId: "20260731T130001Z",
  role: "producer",
  model: "claude-fable-5",
  exitCode: 10,
  outcome: "skipped",
  skipped: "usage-gate",
  skipReason:
    "PAUSE: 91% of the weekly budget used (endpoint) — at/over the 90% ceiling",
};

Deno.test("metrics skip: the emitter's real row is an event, not a zero-work run", () => {
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, REAL_SKIP, RUN_SPLIT2)),
  );
  assert(
    runs.length === 2 && skips.length === 1,
    "the real row must partition as a skip: " + runs.length + "/" +
      skips.length,
  );
  // Tiles: its startupPct 0.0 must not drag the median of [61.5, 40] → 50.8.
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  render(runs);
  const t = textOf(box);
  assert(t.includes("50.8%"), "median stays over the runs alone: " + t);
  // Chart: a tick — not a dot its outcome would paint red at the baseline.
  const wrap = pmChart(runs, "abs", skips);
  assert(collect(wrap, "pm-skip").length === 1, "the real row draws its mark");
  assert(tags(wrap, "circle").length === 2, "and never a run dot");
});

Deno.test("metrics skip: typed outcome or gate field marks a skip, nothing else", () => {
  // One predicate for the whole page, keyed on the TYPED discriminant
  // (outcome "skipped") with the gate field as the fallback arm. Gating on the
  // gate kind's VALUE would silently drop the first gate the producer grows;
  // gating on anything else would misread a run. Fail-open on display, never
  // counted as a run.
  assert(isSkipReal(REAL_SKIP) === true, "the emitter's real row is a skip");
  assert(
    isSkipReal({
      runId: "20260731T090001Z",
      role: "producer",
      outcome: "skipped",
    }) === true,
    "the typed outcome alone marks a skip",
  );
  assert(
    isSkipReal(SKIP_TICK) === true,
    "the gate fields without the typed outcome still mark a skip",
  );
  assert(
    isSkipReal({ ...SKIP_TICK, skipped: "manual-hold" }) === true,
    "an unknown skip kind is still a skip",
  );
  // Ordinary rows carry NEITHER field — absent, not null — and a run's own
  // outcome ("ok", "error", "session-limit") must never read as a skip.
  for (
    const r of [
      RUN_SPLIT,
      RUN_LONG,
      PARTIAL_BOOT,
      PARTIAL_TTL,
      { ...RUN_SPLIT, outcome: "error" },
      { ...RUN_SPLIT, outcome: "session-limit" },
    ]
  ) {
    assert(
      isSkipReal(r) === false,
      "a run row is never a skip: " + JSON.stringify(r.outcome),
    );
  }
});

Deno.test("metrics records: the typed outcome alone routes a row to the skips", () => {
  // A row with outcome "skipped" and no gate fields at all: still an event —
  // it draws a generic skip mark and tooltip rather than being dropped or,
  // worse, painted as an errored run.
  const bare = {
    runId: "20260731T090001Z",
    role: "producer",
    model: "claude-opus-5",
    exitCode: 10,
    outcome: "skipped",
  };
  const { runs, skips } = pmPartitionReal(pmRecordsReal(jsonl(RUN_SPLIT, bare)));
  assert(
    runs.length === 1 && skips.length === 1,
    "the typed outcome must partition as a skip: " + JSON.stringify(skips),
  );
  const wrap = pmChart(runs, "abs", skips);
  assert(collect(wrap, "pm-skip").length === 1, "it draws the skip mark");
  const t = textOf(pmSkipTipBox(skips[0]));
  assert(
    t.includes("skipped") && !t.includes("undefined"),
    "its tooltip is the generic skip, leak-free: " + t,
  );
});

Deno.test("metrics records: a skipped tick is admitted without startup numbers", () => {
  // A skip row carries neither startupPct nor bootMs — the run filter alone
  // would drop it, and the pause would render as the unexplained dead stretch
  // it exists to explain.
  const out = pmRecordsReal(jsonl(RUN_SPLIT, SKIP_TICK));
  assert(out.length === 2, "the run AND the skip must load, got " + out.length);
  const skip = out.find((r) => r.runId === SKIP_TICK.runId);
  assert(
    skip && skip.skipReason === SKIP_TICK.skipReason,
    "the verbatim reason must survive the parse: " + JSON.stringify(skip),
  );
});

Deno.test("metrics records: a vetter skip does not leak into the producer panel", () => {
  const out = pmRecordsReal(jsonl({ ...SKIP_TICK, role: "vetter" }, RUN_SPLIT));
  assert(
    out.length === 1 && out[0].role === "producer" && !isSkipReal(out[0]),
    "this panel is the producer's, skips included: " + JSON.stringify(out),
  );
});

Deno.test("metrics partition: skips leave the runs side entirely", () => {
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, SKIP_TICK, RUN_SPLIT2)),
  );
  assert(
    runs.length === 2 && runs.every((r) => !isSkipReal(r)),
    "runs must be exactly the measurements: " + JSON.stringify(runs),
  );
  assert(
    skips.length === 1 && skips[0].skipped === "usage-gate",
    "skips must be exactly the events: " + JSON.stringify(skips),
  );
});

Deno.test("metrics aggregates: a skipped tick moves no tile", () => {
  // The exclusion is structural — pmPartition strips skips before any renderer
  // runs — so tiles fed the partitioned runs read exactly as if the skip had
  // never been in the file: same median, same run count.
  const rec = (runId, startupPct) => ({
    runId,
    role: "producer",
    startupPct,
    outcome: "ok",
  });
  const { runs } = pmPartitionReal(pmRecordsReal(
    jsonl(rec("20260731T010001Z", 10), SKIP_TICK, rec("20260731T050001Z", 20)),
  ));
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  render(runs);
  const t = textOf(box);
  assert(
    t.includes("15.0%"),
    "median over the two RUNS is 15 — a skip counted as 0 would drag it: " + t,
  );
  const counted = collect(box, "tile").find((tile) =>
    textOf(tile).includes("runs recorded")
  );
  assert(counted, "expected a runs-recorded tile");
  assert(
    textOf(counted).startsWith("2"),
    "runs recorded counts runs, not events: " + textOf(counted),
  );
});

Deno.test("metrics chart: a skipped tick draws a baseline tick, never a run mark", () => {
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "abs", [SKIP_TICK]);
  const marks = collect(wrap, "pm-skip");
  assert(marks.length === 1, "one skip, one marker, got " + marks.length);
  // Shape IS the cue: a line element, never a circle — colour does not carry
  // the distinction alone.
  assert(
    marks[0].tagName === "line",
    "the marker is a tick, got <" + marks[0].tagName + ">",
  );
  assert(
    tags(wrap, "circle").length === 2,
    "dots belong to runs only, got " + tags(wrap, "circle").length,
  );
  // At its own timestamp: later than both runs, so right of both dots.
  const cxs = tags(wrap, "circle").map((c) => parseFloat(c.attrs.cx));
  assert(
    parseFloat(marks[0].attrs.x1) > Math.max(...cxs),
    "the skip sits at its own (later) timestamp: " + marks[0].attrs.x1,
  );
  // Vertical and short at the baseline — not a bar rising to a value a reader
  // could mistake for a measured zero-work run.
  assert(marks[0].attrs.x1 === marks[0].attrs.x2, "the tick is vertical");
  const h = Math.abs(
    parseFloat(marks[0].attrs.y2) - parseFloat(marks[0].attrs.y1),
  );
  assert(h > 0 && h <= 16, "a thin tick, not a full-height bar: " + h);
});

Deno.test("metrics chart: skip ticks draw in proportion mode too", () => {
  // A skip is a timeline event, not a measurement — no unit toggle can make an
  // event meaningless, so it must not vanish when the chart shows proportions.
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "pct", [SKIP_TICK]);
  assert(
    collect(wrap, "pm-skip").length === 1,
    "the skip must mark in pct mode",
  );
});

Deno.test("metrics chart: an unknown skip kind still marks the timeline", () => {
  // Fail-open on display: the producer may grow new gates. Not "usage-gate" is
  // still a skip — generic, rendered, and (by the partition) never a run.
  const odd = { ...SKIP_TICK, skipped: "manual-hold" };
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, odd, RUN_SPLIT2)),
  );
  assert(
    runs.length === 2 && skips.length === 1,
    "the unknown kind partitions as a skip: " + JSON.stringify(skips),
  );
  const wrap = pmChart(runs, "abs", skips);
  assert(
    collect(wrap, "pm-skip").length === 1,
    "the unknown kind still draws its marker",
  );
  assert(tags(wrap, "circle").length === 2, "and never a run mark");
});

Deno.test("metrics chart: a file of nothing but skips still draws the pause", () => {
  // The renderer must be correct on files with zero, some, or ALL skip rows —
  // a long enough pause is exactly the all-skips window.
  const wrap = pmChart([], "pct", [SKIP_TICK, SKIP_TICK2]);
  assert(collect(wrap, "pm-skip").length === 2, "every skip must mark");
  assert(tags(wrap, "circle").length === 0, "no run marks exist");
  const labels = pmTexts(wrap);
  assert(
    labels.some((t) => t.includes("09:00")) &&
      labels.some((t) => t.includes("13:00")),
    "the axis is dated by the skips: " + JSON.stringify(labels),
  );
});

Deno.test("metrics chart: a skip's stray numbers never reach the y scale", () => {
  // The contract pins skipped/skipReason but not what else a skip row carries.
  // Whatever it carries, it is not a run: even a durationMs on the row must
  // not stretch the minutes axis, because the exclusion is the partition, not
  // a per-chart guard.
  const noisy = {
    ...SKIP_TICK,
    durationMs: 36000000,
    startupMs: 36000000,
    bootMs: 36000000,
    ttlMs: 36000000,
    startupPct: 100,
  };
  const { runs, skips } = pmPartitionReal([RUN_SPLIT, RUN_SPLIT2, noisy]);
  const wrap = pmChart(runs, "abs", skips);
  const ticks = tags(wrap, "text")
    .filter((t) => t.getAttribute("text-anchor") === "end")
    .map((t) => parseFloat(t.textContent))
    .filter((v) => !isNaN(v));
  const top = Math.max(...ticks);
  assert(top < 600, "a 600-minute skip row must not scale the axis: " + top);
  assert(collect(wrap, "pm-skip").length === 1, "it still marks the timeline");
});

Deno.test("metrics chart: a file with no skip rows renders exactly as before", () => {
  // The regression pin. Historical runs.jsonl files contain no skip rows, so
  // everything skip-shaped must be absent — no marker, no legend entry — and
  // the marks and labels the chart has always drawn must be untouched.
  for (const mode of ["abs", "pct"]) {
    const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], mode);
    assert(
      collect(wrap, "pm-skip").length === 0,
      "no skip marker without skip rows",
    );
    const legends = collect(wrap, "pm-legend");
    if (mode === "abs") {
      assert(
        legends.length === 1 && !textOf(legends[0]).includes("skip"),
        "the legend gains no skip entry: " + textOf(legends[0]),
      );
    } else {
      assert(legends.length === 0, "proportion mode keeps having no legend");
    }
    assert(tags(wrap, "circle").length === 2, "same dots as always");
    assert(collect(wrap, "pm-line").length > 0, "same startup line as always");
  }
  const labels = pmTexts(pmChart([RUN_SPLIT, RUN_SPLIT2], "abs"));
  assert(
    labels.some((t) => t.includes("01:00")) &&
      labels.some((t) => t.includes("05:00")),
    "axis endpoints stay the runs' own timestamps: " + JSON.stringify(labels),
  );
});

Deno.test("metrics legend: names the skip mark whenever skips are on the plot", () => {
  const absLegend = collect(
    pmChart([RUN_SPLIT, RUN_SPLIT2], "abs", [SKIP_TICK]),
    "pm-legend",
  )[0];
  assert(
    absLegend && textOf(absLegend).includes("skipped tick"),
    "absolute mode must name the mark: " + (absLegend && textOf(absLegend)),
  );
  // Proportion mode normally has no legend — a second MARK on the plot is what
  // forces one, so identity is never carried by the mark alone.
  const pctLegend = collect(
    pmChart([RUN_SPLIT, RUN_SPLIT2], "pct", [SKIP_TICK]),
    "pm-legend",
  )[0];
  assert(
    pctLegend && textOf(pctLegend).includes("skipped tick"),
    "proportion mode must name it too: " + (pctLegend && textOf(pctLegend)),
  );
});

Deno.test("metrics skip tip: carries the gate's reason verbatim", () => {
  const t = textOf(pmSkipTipBox(SKIP_TICK));
  assert(
    t.includes(SKIP_TICK.skipReason),
    "the PAUSE line must appear verbatim: " + t,
  );
  assert(t.includes("skipped"), "it names itself a skip: " + t);
  assert(t.includes("usage-gate"), "the gate kind is named: " + t);
  assert(t.includes("Jul 31 09:00 UTC"), "it is dated: " + t);
  assert(
    !t.includes("undefined") && !t.includes("NaN"),
    "no undefined may reach the tip: " + t,
  );
});

Deno.test("metrics skip tip: an unknown or unnameable kind still explains itself", () => {
  const t = textOf(
    pmSkipTipBox({ runId: "20260731T090001Z", skipped: "manual-hold" }),
  );
  assert(
    t.includes("manual-hold"),
    "the kind the row declares must show: " + t,
  );
  assert(!t.includes("undefined"), "an absent reason prints nothing: " + t);
  // A non-string kind falls back to the generic word rather than "true".
  const kind = tags(
    pmSkipTipBox({ runId: "20260731T090001Z", skipped: true }),
    "span",
  )[0];
  assert(
    kind && kind.textContent === "skip",
    "a non-string kind reads generically: " + (kind && kind.textContent),
  );
});

Deno.test("metrics chart: hovering a skip yields its reason, not run numbers", () => {
  // The integration half — pmSkipTip being right is worth nothing if the chart
  // hands a skip to the RUN tooltip, which would report it as a measurement.
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "abs", [SKIP_TICK]);
  const svg = pmSvg(wrap);
  const rect = { left: 0, top: 0, width: 720, height: 210 };
  svg._rect = rect;
  wrap._rect = rect;
  // The skip is the latest event, so the right edge is nearest to it.
  svg.fire("mousemove", { clientX: 719, clientY: 10 });
  const t = textOf(collect(wrap, "pm-tip")[0]);
  assert(
    t.includes(SKIP_TICK.skipReason),
    "the verbatim reason is the tooltip: " + t,
  );
  assert(!t.includes("startup"), "no run phrasing on a skip: " + t);
  assert(
    !t.includes("undefined") && !t.includes("NaN"),
    "no undefined on hover: " + t,
  );
});

Deno.test("metrics caption: names the pause when skips exist, and only then", () => {
  const withSkips = textOf(pmNoteBox(RUN_SPLIT, [SKIP_TICK, SKIP_TICK2]));
  assert(
    withSkips.includes("2 ticks"),
    "the count of skipped ticks: " + withSkips,
  );
  assert(withSkips.includes("usage gate"), "names the cause: " + withSkips);
  const without = textOf(pmNoteBox(RUN_SPLIT));
  assert(
    !without.includes("usage gate"),
    "no phantom sentence when the file has no skips: " + without,
  );
});

// An empty token set must EXPLAIN itself. The registry section silently
// vanished from prod for days because the governed-vault parser stopped
// matching an upstream refactor, the intersection emptied, and both the
// scanner and the page treated "nothing to show" as "show nothing" — so the
// outage looked like a feature that had never existed.
Deno.test("deployments: every chain in the beacon list gets its own section", () => {
  const set = (network, name) => ({
    org: "S01-Issuer",
    repo: "st0x.deploy",
    network,
    rpcHost: `${network}-rpc`,
    safeOwner: "0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
    targetVersion: "0.1.1",
    total: 1,
    healthy: 1,
    beacons: [{
      name,
      address: "0x4c2d2d3Bf1232bf0d3FB7123007A9B8444637bC8",
      status: "healthy",
      ownerLabel: "safe",
      implLabel: "target",
    }],
  });
  const box = deploymentsBox({
    deploymentBeacons: [
      set("base", "Base wrapped beacon"),
      set("ethereum", "Ethereum wrapped beacon"),
    ],
  });
  const t = textOf(box);
  assert(t.includes("Beacons — base"), "base section missing: " + t);
  assert(t.includes("Beacons — ethereum"), "ethereum section missing: " + t);
  assert(
    t.includes("Base wrapped beacon") && t.includes("Ethereum wrapped beacon"),
    "each chain must render its own rows: " + t,
  );
});

Deno.test("deployments: a single beacon block still renders", () => {
  const box = deploymentsBox({
    deploymentBeacons: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      network: "base",
      rpcHost: "mainnet.base.org",
      safeOwner: "0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
      targetVersion: "0.1.1",
      total: 1,
      healthy: 1,
      beacons: [{
        name: "Wrapped beacon",
        address: "0x4c2d",
        status: "healthy",
      }],
    },
  });
  const t = textOf(box);
  assert(
    t.includes("Beacons — base"),
    "a health.json written before the array shape must still render: " + t,
  );
});

Deno.test("deployments: a chain the scan could not read says so, it does not vanish", () => {
  const box = deploymentsBox({
    deploymentBeacons: [{
      network: "ethereum",
      rpcHost: "ethereum-rpc.publicnode.com",
      unavailable: true,
      reason: "the scan could not read the st0x.deploy beacon constants",
    }],
  });
  const t = textOf(box);
  assert(
    t.includes("Beacons — ethereum"),
    "the chain must still be named: " + t,
  );
  assert(
    t.includes("unavailable"),
    "must distinguish broken from absent: " + t,
  );
  assert(
    t.includes("could not read"),
    "the reason identifies the failure: " + t,
  );
});

Deno.test("deployments: an empty token set still shows the reconcile breakdown", () => {
  const box = deploymentsBox({
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      tokens: [],
      reconcile: {
        governedCount: 2,
        registryTokenCount: 23,
        source: "S01-Issuer/st0x.deploy",
        function: "LibTokenInvariants.productionReceiptVaults()",
        extraVaults: [{
          address: "0xfeed000000000000000000000000000000000001",
        }],
        missingFromMigration: [{ symbol: "MSTR" }],
      },
    },
  });
  const t = textOf(box);
  assert(
    t.includes("0xfeed000000000000000000000000000000000001"),
    "the governed vault absent from the registry is the diagnostic: " + t,
  );
  assert(
    !t.includes("could not be read"),
    "a non-zero governed count is not an unreadable list: " + t,
  );
  assert(
    t.includes("disjoint"),
    "must name the actual cause when both sides parsed: " + t,
  );
});

Deno.test("deployments: a governed set short of its declared entries says so", () => {
  const box = deploymentsBox({
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      tokens: [],
      reconcile: {
        governedCount: 20,
        governedDeclared: 22,
        registryTokenCount: 23,
        source: "S01-Issuer/st0x.deploy",
        function: "LibTokenInvariants.productionReceiptVaults()",
        extraVaults: [],
        missingFromMigration: [],
      },
    },
  });
  const t = textOf(box);
  assert(t.includes("INCOMPLETE"), "a short list must be named as short: " + t);
  assert(t.includes("22"), "the declared count must appear: " + t);
});

Deno.test("deployments: a fully resolved governed set claims no shortfall", () => {
  const box = deploymentsBox({
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      tokens: [],
      reconcile: {
        governedCount: 22,
        governedDeclared: 22,
        registryTokenCount: 23,
        source: "S01-Issuer/st0x.deploy",
        function: "LibTokenInvariants.productionReceiptVaults()",
        extraVaults: [],
        missingFromMigration: [],
      },
    },
  });
  const t = textOf(box);
  assert(
    !t.includes("INCOMPLETE"),
    "declared === resolved is not a shortfall: " + t,
  );
});

Deno.test("deployments: an empty token set reports why, it does not vanish", () => {
  const box = deploymentsBox({
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      tokens: [],
      reconcile: {
        governedCount: 0,
        registryTokenCount: 23,
        function: "LibTokenInvariants.productionReceiptVaults()",
        extraVaults: [],
        missingFromMigration: [],
      },
    },
  });
  const t = textOf(box);
  assert(
    t.includes("Tokens"),
    "the heading must still render: " + t.slice(0, 200),
  );
  assert(
    t.includes("0 governed"),
    "the governed count identifies the cause: " + t,
  );
  assert(
    t.includes("23 registry"),
    "the registry count shows the other side: " + t,
  );
  assert(
    t.includes("unavailable, not empty"),
    "must distinguish broken from genuinely empty: " + t,
  );
});

// --- hostile input ----------------------------------------------------------
// Nothing the dashboard renders is authored here. health.json carries repo
// names, git tags and PDF filenames read out of other orgs' repositories;
// deployments carry token names read off-chain, where the name is whatever the
// deployer of the contract chose; runs.jsonl carries the producer's own strings
// but from a repo this page does not own. Any of it can be markup. The pages
// therefore build DOM nodes and never assign a markup string, so a payload can
// only ever become text — these drive the REAL renderers with payloads and
// assert exactly that, because a happy-path fixture proves nothing about it.
const XSS_IMG = '<img src=x onerror="alert(1)">';
const XSS_SCRIPT = "<script>alert(1)</script>";
const XSS_SVG = '<svg onload="alert(1)">';
// The attribute-context payload: harmless as text, an event handler the moment
// it is concatenated into a quoted attribute.
const XSS_ATTR = '" onmouseover="alert(1)';

// Elements that only exist if a payload was parsed as markup rather than set as
// text. `svg`/`text`/`circle` are legitimately built by the chart, so the ban
// list is the tags a payload would introduce, not "any tag".
const MARKUP_TAGS = ["img", "script", "iframe", "object", "embed", "style"];

// Descriptions of anything in the tree that could only come from parsed markup.
function markupNodes(root, out = []) {
  for (const c of root.children || []) {
    if (c && typeof c === "object") {
      if (MARKUP_TAGS.includes(c.tagName)) out.push("<" + c.tagName + ">");
      for (const k of Object.keys(c.attrs || {})) {
        if (/^on/i.test(k)) out.push(k + "=" + c.attrs[k]);
      }
      markupNodes(c, out);
    }
  }
  return out;
}

// The payload reached the rendered TEXT (so it was set with textContent or
// appended as a string, both of which produce a text node), and the tree grew
// no element or handler from it. The first half is what fails the moment a
// renderer goes back to building a markup string: an `innerHTML` assignment
// puts the payload somewhere textContent cannot see.
function assertInert(root, payload, where) {
  const text = textOf(root);
  assert(
    text.includes(payload),
    where + ": payload must render as text, got: " + JSON.stringify(text),
  );
  const bad = markupNodes(root);
  assert(
    bad.length === 0,
    where + ": payload became markup: " + bad.join(", "),
  );
}

function repoListBind(repos, org = "testorg") {
  const box = makeEl("div");
  const search = makeEl("input");
  search.value = "";
  const $ = (id) =>
    id === "repos" ? box : id === "search" ? search : makeEl("div");
  const fn = bind(
    "repositories.html",
    "render",
    ["$", "document", "data", "activeSignal", "setSignal"],
    [$, stubDocument(), { org, repos }, null, () => {}],
  );
  return [fn, box];
}

function pmNoteBox(last, skips) {
  const box = makeEl("div");
  const $ = (id) => (id === "pmnote" ? box : makeEl("div"));
  const d = pmDeps();
  const fmtAgo = bind("metrics.html", "fmtAgo", [], []);
  bind(
    "metrics.html",
    "renderPmNote",
    ["$", "document", "fmtAgo", "parseRunId", "outcomeWord"],
    [$, stubDocument(), fmtAgo, d.parseRunId, outcomeWordReal],
  )(last, skips);
  return box;
}

Deno.test("hostile input: a repo name that is markup renders as text, and its link is a property", () => {
  const [render, box] = repoListBind([{
    name: XSS_IMG,
    signals: [XSS_SCRIPT],
  }]);
  render();
  assertInert(box, XSS_IMG, "repo row");
  assertInert(box, XSS_SCRIPT, "signal chip");
  // The href is assigned as a property, so an attribute-context payload cannot
  // break out of the quoting — it stays inside the path of the URL.
  const a = tags(box, "a")[0];
  assert(
    a.href === "https://github.com/testorg/" + XSS_IMG,
    "the repo link must carry the name verbatim in the path: " + a.href,
  );
});

Deno.test("hostile input: a quote-and-angle-bracket repo name stays inside the link", () => {
  const [render, box] = repoListBind([{ name: XSS_ATTR, signals: [] }]);
  render();
  assertInert(box, XSS_ATTR, "repo row");
  const a = tags(box, "a")[0];
  assert(
    a.href.endsWith(XSS_ATTR),
    "an attribute-context payload must remain part of the href value: " +
      a.href,
  );
});

Deno.test("hostile input: a signal name that is markup renders as text", () => {
  const [render, box] = repoSummaryBind({ [XSS_IMG]: 3 });
  render();
  assertInert(box, XSS_IMG, "signal summary");
});

Deno.test("hostile input: an audit row's repo name and git tag render as text", () => {
  const box = auditBox(auditData([
    auditRow({
      name: XSS_IMG,
      latestTag: XSS_SCRIPT,
      compareUrl: "https://h/x/compare/a...b",
      commitsSinceAudit: 3,
    }),
  ]));
  assertInert(box, XSS_IMG, "audit row name");
  assertInert(box, XSS_SCRIPT, "audit row tag");
});

// The grant rows are built from a constant NAME and a network NAME read out of
// another repo's Solidity — both attacker-influenceable by anyone who can land a
// commit there, and both rendered next to an address a reader is meant to trust.
Deno.test("hostile input: a grantee constant, its network and its address render as text", () => {
  const d = grantsData();
  d.deploymentGrants.grantees[1].ident = XSS_IMG;
  d.deploymentGrants.grantees[1].address = XSS_SCRIPT;
  d.deploymentGrants.grantees[1].roles[0].role = XSS_SVG;
  d.deploymentGrants.grantees[1].roles[0].chains[0].address = XSS_SCRIPT;
  d.deploymentGrants.chains[0].network = XSS_ATTR;
  const box = deploymentsBox(d);
  assertInert(box, XSS_IMG, "grantee constant name");
  assertInert(box, XSS_SCRIPT, "grantee address");
  assertInert(box, XSS_SVG, "role name");
  assertInert(box, XSS_ATTR, "chain name");
  // The address goes into an href as a PROPERTY, so an attribute-context
  // payload stays inside the URL rather than escaping the quoting.
  const a = tags(box, "a").find((x) => x.textContent === XSS_SCRIPT);
  assert(
    a && a.href === "https://basescan.org/address/" + XSS_SCRIPT,
    "the hostile address stays inside the href path, got " + (a && a.href),
  );
});

Deno.test("hostile input: a run's outcome, id and counts render as tooltip text", () => {
  // A DISTINCT payload per field: sharing one would let a field that stopped
  // rendering as text pass on another field's copy of the same string.
  const box = pmTipBox({
    runId: XSS_ATTR,
    startupPct: 4.3,
    startupMs: 590693,
    durationMs: 1611124,
    toolCalls: XSS_SCRIPT,
    startupToolCalls: 23,
    numTurns: XSS_SVG,
    outcome: XSS_IMG,
  }, true);
  // An unparseable runId falls back to the raw string, so it reaches the tip
  // verbatim — which is only safe because the tip is built from nodes.
  assertInert(box, XSS_ATTR, "tooltip run id");
  assertInert(box, XSS_SCRIPT, "tooltip tool calls");
  assertInert(box, XSS_SVG, "tooltip turn count");
  assertInert(box, XSS_IMG, "tooltip outcome");
});

Deno.test("hostile input: the latest run's outcome renders as note text", () => {
  const box = pmNoteBox({ runId: "20260720T010001Z", outcome: XSS_IMG });
  assertInert(box, XSS_IMG, "chart note");
});

Deno.test("hostile input: hovering the chart lands the payload in the tip as text", () => {
  // The integration half: pmTip being safe is worth nothing if the chart stops
  // calling it and interpolates the run itself.
  const run = (runId) => ({
    runId,
    startupPct: 4.3,
    startupMs: 590693,
    durationMs: 1611124,
    toolCalls: 529,
    startupToolCalls: 23,
    numTurns: 66,
    outcome: XSS_IMG,
  });
  const wrap = pmChart(
    [run("20260720T010001Z"), run("20260720T170002Z")],
    "abs",
  );
  const svg = pmSvg(wrap);
  const rect = { left: 0, top: 0, width: 720, height: 210 };
  svg._rect = rect;
  wrap._rect = rect;
  svg.fire("mousemove", { clientX: 10, clientY: 10 });
  const tip = collect(wrap, "pm-tip")[0];
  assert(tip, "the chart must build a tip element");
  assertInert(tip, XSS_IMG, "chart tooltip on hover");
  assert(
    markupNodes(wrap).length === 0,
    "no payload may become markup anywhere on the chart: " +
      markupNodes(wrap).join(", "),
  );
});

Deno.test("hostile input: an unparseable run id yields no axis label, not raw markup", () => {
  const run = (runId) => ({ runId, startupPct: 4.3, outcome: "ok" });
  const wrap = pmChart([run(XSS_IMG), run(XSS_SCRIPT)], "pct");
  assert(
    markupNodes(wrap).length === 0,
    "axis labels must never become markup: " + markupNodes(wrap).join(", "),
  );
  assert(
    !pmTexts(wrap).some((t) => t.includes("<")),
    "an unparseable id formats to an empty label: " +
      JSON.stringify(pmTexts(wrap)),
  );
});

Deno.test("hostile input: a skip's reason, kind and id render as tooltip text", () => {
  // skipReason is carried VERBATIM by contract, from a repo this page does not
  // own — the verbatim guarantee is exactly what makes it a markup carrier. A
  // distinct payload per field, so a field that stopped rendering as text
  // cannot pass on another field's copy.
  const box = pmSkipTipBox({
    runId: XSS_ATTR,
    skipped: XSS_IMG,
    skipReason: XSS_SCRIPT,
  });
  assertInert(box, XSS_IMG, "skip kind");
  assertInert(box, XSS_SCRIPT, "skip reason");
  assertInert(box, XSS_ATTR, "skip run id");
});

Deno.test("hostile input: hovering a hostile skip lands it in the tip as text", () => {
  // The integration half: pmSkipTip being safe is worth nothing if the chart
  // stops calling it and interpolates the skip row itself.
  const skip = { ...SKIP_TICK, skipReason: XSS_SCRIPT };
  const wrap = pmChart([RUN_SPLIT, RUN_SPLIT2], "abs", [skip]);
  const svg = pmSvg(wrap);
  const rect = { left: 0, top: 0, width: 720, height: 210 };
  svg._rect = rect;
  wrap._rect = rect;
  svg.fire("mousemove", { clientX: 719, clientY: 10 });
  assertInert(collect(wrap, "pm-tip")[0], XSS_SCRIPT, "skip tooltip on hover");
  assert(
    markupNodes(wrap).length === 0,
    "no payload may become markup anywhere on the chart: " +
      markupNodes(wrap).join(", "),
  );
});

// A page may not so much as NAME a markup sink in its code. Matching the
// assignment (`.innerHTML =`) instead would have to enumerate the ways one can
// be written — `+=`, `||=`, `??=`, `el["innerHTML"] = x`, `Object.assign(el, {
// innerHTML: x })` — and a guard that enumerates is the same escape-by-
// remembering the renderers just stopped doing. The identifier itself is the
// thing that must not appear: none of these pages has any business reading a
// sink either, so there is no legitimate mention to carve out.
const MARKUP_SINK =
  /\binnerHTML\b|\bouterHTML\b|\binsertAdjacentHTML\b|\bsrcdoc\b|\bdocument\.write\b|\bcreateContextualFragment\b/;

// The guard is only worth what it catches, so pin that directly rather than
// trusting the pattern by eye. Every line here is a real sink somebody could
// plausibly write; the negatives are the DOM calls that replaced them.
Deno.test("the markup-sink guard catches every assignment form, not just `=`", () => {
  for (
    const sink of [
      'el.innerHTML = "<b>x</b>";',
      'el.innerHTML += "<b>x</b>";',
      'el.innerHTML ||= "<b>x</b>";',
      'el.innerHTML ??= "<b>x</b>";',
      "el.innerHTML=x;",
      'el["innerHTML"] = x;',
      "Object.assign(el, { innerHTML: x });",
      'el.outerHTML = "<b>x</b>";',
      'el.insertAdjacentHTML("beforeend", x);',
      'document.write("<b>x</b>");',
      "range.createContextualFragment(x);",
      'frame.srcdoc = "<b>x</b>";',
    ]
  ) {
    assert(MARKUP_SINK.test(sink), "guard must flag: " + sink);
  }
  for (
    const ok of [
      "el.replaceChildren();",
      "el.textContent = x;",
      "el.append(a, b);",
      'a.href = "https://example.com/" + name;',
      'svgEl("text", { x: 1 }, label);',
    ]
  ) {
    assert(!MARKUP_SINK.test(ok), "guard must not flag: " + ok);
  }
});

// The per-renderer tests above prove the paths they drive. This one is what
// makes the class unrepresentable rather than merely absent: a page with no
// markup sink at all cannot grow a forgotten escape. Adding a section is
// therefore not a new chance to get escaping wrong — there is nothing to escape.
Deno.test("dashboard pages contain no markup sink at all", () => {
  const dir = new URL("../site/", import.meta.url);
  const sinks = MARKUP_SINK;
  const offenders = [];
  for (const entry of Deno.readDirSync(dir)) {
    if (!entry.isFile || !entry.name.endsWith(".html")) continue;
    const src = Deno.readTextFileSync(new URL(entry.name, dir));
    src.split("\n").forEach((line, i) => {
      // A comment naming the rule is not a use of it.
      if (/^\s*(\/\/|\*|<!--)/.test(line)) return;
      if (sinks.test(line)) {
        offenders.push(`${entry.name}:${i + 1}:${line.trim()}`);
      }
    });
  }
  assert(
    offenders.length === 0,
    "a page must build DOM nodes, never assign markup:\n" +
      offenders.join("\n"),
  );
});

// A stray control byte in a page is invisible in every view that matters. One
// reached this file as a NUL inside the metrics chart's collapse key — where a
// SPACE was intended — and nothing caught it: NUL is a legal character in a JS
// string, so the key still worked and the whole suite stayed green; prettier
// reformatted around it without complaint; and printing the line shows nothing
// where the byte is. The only thing that finds it is looking for it. Tab and
// newline are the two that legitimately appear in source.
Deno.test("no page or test carries a stray control byte", () => {
  // The class is every byte that is invisible yet legal: C0 (U+0000-U+001F),
  // DEL (U+007F) and C1 (U+0080-U+009F). Stopping at C0 would catch the byte
  // that actually shipped and leave its neighbours — the point of the guard is
  // the category, not the one instance. The walk recurses because `site/` has
  // subdirectories; the vendored ELK bundle is scanned too and is clean, so
  // nothing needs carving out.
  const want = (n) => /\.(html|md|js)$/.test(n);
  const walk = function* (base) {
    for (const entry of Deno.readDirSync(base)) {
      const url = new URL(
        encodeURIComponent(entry.name) + (entry.isDirectory ? "/" : ""),
        base,
      );
      if (entry.isDirectory) yield* walk(url);
      else if (entry.isFile && want(entry.name)) yield [entry.name, url];
    }
  };
  const offenders = [];
  for (const rel of ["../site/", "./"]) {
    for (const [name, url] of walk(new URL(rel, import.meta.url))) {
      const src = Deno.readTextFileSync(url);
      for (let i = 0; i < src.length; i++) {
        const c = src.charCodeAt(i);
        if ((c < 32 && c !== 10 && c !== 9) || (c >= 0x7f && c <= 0x9f)) {
          const line = src.slice(0, i).split("\n").length;
          offenders.push(
            `${name}:${line}: U+${c.toString(16).padStart(4, "0")}`,
          );
        }
      }
    }
  }
  assert(
    offenders.length === 0,
    "source must carry no invisible control bytes but tab and newline:\n" +
      offenders.join("\n"),
  );
});

// These stylesheets are heavily commented — often several prose paragraphs per rule — and a
// mis-terminated comment is SILENT. There is no parse error, no console warning, nothing in
// the DOM: CSS error recovery just consumes forward to the next `{…}` and throws that whole
// block away. A stray `*/` therefore deletes the rule that FOLLOWS it, which is the rule the
// comment was explaining. That is exactly how `.fsm-state.rising` lost its red border here —
// a paragraph appended after a comment's terminator instead of before it swallowed the rule,
// and every "is it highlighted?" check still passed because the reference box it was being
// compared against had been broken by the same stray delimiter.
const styleBlocks = (src) =>
  [...src.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)].map((m) => m[1]);

// A hand-rolled scan rather than a regex: the property is about the SEQUENCE of delimiters,
// which is what a regex over the whole file cannot see. Returns comment-stripped CSS plus
// every structural fault found, so one pass serves both assertions below.
const scanCss = (css) => {
  const faults = [];
  let out = "", i = 0, depth = 0, openedAt = -1;
  while (i < css.length) {
    if (css.startsWith("/*", i)) {
      if (depth > 0) faults.push(`nested /* at offset ${i} (CSS comments do not nest)`);
      else openedAt = i;
      depth++, i += 2;
    } else if (css.startsWith("*/", i)) {
      if (depth === 0) faults.push(`stray */ at offset ${i} with no comment open`);
      else depth--;
      i += 2;
    } else {
      if (depth === 0) out += css[i];
      i++;
    }
  }
  if (depth > 0) faults.push(`comment opened at offset ${openedAt} is never closed`);
  return { out, faults };
};

Deno.test("stylesheet comments are balanced, so no rule is silently swallowed", () => {
  const dir = new URL("../site/", import.meta.url);
  const faults = [];
  for (const entry of Deno.readDirSync(dir)) {
    if (!entry.isFile || !entry.name.endsWith(".html")) continue;
    const src = Deno.readTextFileSync(new URL(entry.name, dir));
    styleBlocks(src).forEach((css, n) => {
      for (const f of scanCss(css).faults) faults.push(`${entry.name} <style> #${n + 1}: ${f}`);
    });
  }
  assert(faults.length === 0, "malformed stylesheet comment:\n" + faults.join("\n"));
});

// Prose landing in selector position is the SYMPTOM a browser acts on, whatever produced it.
// Two independent tells, because either alone has a blind spot: punctuation a selector cannot
// contain, and a run of bare words no selector would ever string together. A word list was the
// obvious first cut and was wrong — `a` is an element selector, so `.nav a` read as prose.
const isProse = (s) =>
  /[`;—’“”]/.test(s) ||
  s.split(/\s+/).filter((t) => /^[A-Za-z]+$/.test(t)).length >= 5;

// Rules as a BROWSER would end up with them, error recovery included: a block whose prelude
// is not a selector is not merely ugly, it is discarded along with its declarations. Modelling
// the discard is the whole point — a check that reads the rule text out of the raw source
// would happily find `.fsm-state.rising` sitting behind garbage that deletes it, which is
// precisely the false pass that let this ship.
const cssRules = (css) => {
  const out = [];
  let depth = 0, prelude = "", body = "";
  for (const ch of scanCss(css).out) {
    if (ch === "{") {
      depth++;
      if (depth === 1) continue;
    } else if (ch === "}") {
      depth = Math.max(0, depth - 1);
      if (depth === 0) {
        const sel = prelude.trim().replace(/\s+/g, " ");
        // An at-rule (@media, @supports) nests real rules; recurse rather than treat its
        // prelude as a selector. Anything else with prose in the prelude is dropped.
        if (sel.startsWith("@")) out.push(...cssRules(body));
        else if (!isProse(sel)) out.push({ sel, body: body.trim() });
        prelude = "", body = "";
        continue;
      }
    }
    if (depth === 0) prelude += ch;
    else body += ch;
  }
  return out;
};

// The delimiter count above catches the cause; this catches the EFFECT, and would still fire
// for any other way prose lands in stylesheet position. Between one rule's `}` and the next
// rule's `{` there may only ever be a selector — so anything in that gap carrying prose
// punctuation means a block boundary is not where the author thought it was.
Deno.test("nothing but selectors sits between stylesheet rules", () => {
  const dir = new URL("../site/", import.meta.url);
  const offenders = [];
  for (const entry of Deno.readDirSync(dir)) {
    if (!entry.isFile || !entry.name.endsWith(".html")) continue;
    const src = Deno.readTextFileSync(new URL(entry.name, dir));
    styleBlocks(src).forEach((css, n) => {
      let depth = 0, gap = "";
      const flush = () => {
        const s = gap.trim();
        if (s && isProse(s)) {
          offenders.push(`${entry.name} <style> #${n + 1}: not a selector: ${JSON.stringify(s.slice(0, 90))}`);
        }
        gap = "";
      };
      for (const ch of scanCss(css).out) {
        if (ch === "{") depth++, depth === 1 && flush();
        else if (ch === "}") depth = Math.max(0, depth - 1);
        else if (depth === 0) gap += ch;
      }
      flush();
    });
  }
  assert(offenders.length === 0, "prose in stylesheet position swallows the next rule:\n" + offenders.join("\n"));
});

// The two guards above are structural. This one pins the specific rule the bug destroyed, in
// the terms that matter to a reader: the bottleneck highlight and its fallback must actually
// declare the red border, and must declare it TOGETHER so they can never drift apart.
Deno.test("the bottleneck highlight and its fallback survive parsing with the red border intact", () => {
  const src = Deno.readTextFileSync(new URL("../site/pipeline.html", import.meta.url));
  const rules = styleBlocks(src).flatMap(cssRules);
  const hit = rules.find((r) => /\.fsm-state\.rising\b/.test(r.sel));
  assert(hit, "the .fsm-state.rising rule survives parsing — it is not swallowed by recovery");
  assert(
    /\.fsm-state\.lead\b/.test(hit.sel),
    `the fallback shares the rule rather than restating it: ${hit.sel}`,
  );
  assert(
    /border-color:\s*var\(--crit\)/.test(hit.body),
    `the highlight declares the red border: ${hit.body}`,
  );
  assert(
    /box-shadow:\s*inset[^;]*var\(--crit\)/.test(hit.body),
    `the highlight declares the red inset ring: ${hit.body}`,
  );
});
