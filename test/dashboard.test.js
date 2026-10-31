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

const zoomBtnDisabled = bind("audit.html", "zoomBtnDisabled", [
  "GRAPH_ZOOM_MAX",
], [3]);

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
  assert(
    graphFit(1032, 1032, 1000, 1000) === 1,
    "exactly-fitting graph sits at 1:1",
  );
  // A graph twice the box width fits at half scale, and the TIGHTER axis wins.
  assert(
    graphFit(532, 1032, 1000, 1000) === 0.5,
    "width-constrained: " + graphFit(532, 1032, 1000, 1000),
  );
  assert(graphFit(1032, 532, 1000, 1000) === 0.5, "height-constrained");
  // Small graph in a big box is NOT blown up — 1 is the ceiling.
  assert(graphFit(2032, 2032, 100, 100) === 1, "never magnifies past 1:1");
});

Deno.test("graph zoom: steps in and out, clamped by the fit below and the max above", () => {
  const fit = 0.5;
  assert(
    zoomScale(fit, fit, "in") === 0.625,
    "one step in: " + zoomScale(fit, fit, "in"),
  );
  assert(
    zoomScale(1, fit, "out") === 0.8,
    "one step out: " + zoomScale(1, fit, "out"),
  );
  // Nothing smaller than "the whole graph" is useful, so the fit is the floor.
  assert(zoomScale(fit, fit, "out") === fit, "cannot zoom out past the fit");
  assert(
    zoomScale(0.55, fit, "out") === fit,
    "a step that would undershoot clamps to the fit",
  );
  // And a step cannot launch the graph out of its own scrollport.
  assert(zoomScale(3, fit, "in") === 3, "cannot zoom in past the max");
  assert(
    zoomScale(2.9, fit, "in") === 3,
    "a step that would overshoot clamps to the max",
  );
});

Deno.test("graph zoom: fit action returns the CURRENT fit, not a remembered one", () => {
  // Reset recomputes, so after a resize it lands on the new fit rather than the
  // scale the graph first rendered at.
  assert(zoomScale(2.4, 0.5, "fit") === 0.5, "resets to the fit passed in");
  assert(
    zoomScale(2.4, 0.31, "fit") === 0.31,
    "a resized box resets to its NEW fit",
  );
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

Deno.test("graph node: an unresolvable package renders as UNKNOWN, not as no-package", () => {
  // rainix#335 moves the soldeer package name out of foundry.toml into the
  // release workflow's `soldeer-package:` input. A workflow that exists but
  // yields no name means edges INTO this node may be missing — its consumers
  // can read as standing on clear ground — and the node must say so, the same
  // way depsKnown keeps unknown deps apart from zero deps.
  const box = graphNode(
    {
      repo: "migrated",
      org: "o",
      audit: "never",
      depsKnown: true,
      packageKnown: false,
    },
    null,
  );
  const t = textOf(box);
  assert(
    t.includes("package unknown"),
    "must flag the unreadable package: " + t,
  );
  // a resolved (or genuinely absent) package renders no flag…
  const known = graphNode(
    {
      repo: "plain",
      org: "o",
      audit: "never",
      depsKnown: true,
      packageKnown: true,
    },
    null,
  );
  assert(
    !textOf(known).includes("package unknown"),
    "a known package renders no flag: " + textOf(known),
  );
  // …and neither does old health.json data without the field.
  const oldData = graphNode(
    { repo: "old", org: "o", audit: "never", depsKnown: true },
    null,
  );
  assert(
    !textOf(oldData).includes("package unknown"),
    "pre-field data renders no flag: " + textOf(oldData),
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
  assert(
    count,
    "3 open findings takes the critical class: " + textOf(carrying),
  );
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
  const css = Deno.readTextFileSync(
    new URL("../site/audit.html", import.meta.url),
  );
  for (const sel of [".gn-open", ".au-open"]) {
    const rule = new RegExp("^\\s*\\" + sel + "\\s*\\{([^}]*)\\}", "m").exec(
      css,
    );
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
      {
        name: "carrying",
        org: "testorg",
        lastAudit: null,
        openAuditIssues: 14,
      },
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
  assert(
    text.includes("rain.flare#22"),
    "second leaked PR listed by repo#number",
  );
});

Deno.test("pipeline FSM: unwired queue renders the not-wired-yet empty state", () => {
  assert(
    textOf(fsmBox(null)).includes("not wired yet"),
    "null human-queue → not-wired-yet copy",
  );
});

// #66: no two boxes may share a label (they read as one contradictory control
// otherwise), and the bare, ambiguous "ai:close-candidate" never renders.
Deno.test("pipeline FSM: the close-candidate states carry distinct labels", () => {
  const box = fsmBox({
    counts: {
      leaks: 0,
      ready: 0,
      closeCandidateUnvetted: 18,
      closeCandidateUpheld: 4,
    },
    lanes: {},
  });
  const labels = collect(box, "sk").map((s) => s.textContent);
  // One flag, two boxes, split by vetting stage (issue-pr-cron#73/#212/#213) — the ONLY
  // close-candidate states the machine has. Exact set equality bans a third variant (a
  // subject-type split, a bare label, a decided-close state) as hard as a missing one.
  const cc = labels.filter((l) => l.includes("close-candidate")).sort();
  assert(
    JSON.stringify(cc) ===
      JSON.stringify([
        "ai:close-candidate (unvetted)",
        "ai:close-candidate (upheld)",
      ]),
    `exactly the two close-candidate labels render: ${JSON.stringify(cc)}`,
  );
  assert(
    new Set(labels).size === labels.length,
    `no two boxes share a label: ${JSON.stringify(labels)}`,
  );
});

// #66 (primary): the shared detail panel stays at the bottom, but clicking a state
// opens it with a HEADER naming THAT state and a count == its own list length — so
// which state the list belongs to is unambiguous (not attached to the last lane).
Deno.test("pipeline FSM: clicking a state opens the bottom panel with a header naming it + matching count", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateUpheld: 3 },
    lanes: { "vetter-verdicts": {} },
    // A mixed upheld list — an issue, a PR (resolved url), another issue — one inbox.
    closeCandidateUpheld: [
      { repo: "o/a", number: 1, title: "flagged issue one" },
      {
        repo: "o/b",
        number: 2,
        title: "flagged pr two",
        url: "https://github.com/o/b/pull/2",
      },
      { repo: "o/c", number: 3, title: "flagged issue three" },
    ],
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

  // Click the upheld box (3 mixed subjects).
  const ccBox = boxByT("closeCandidateUpheld");
  ccBox.click();
  assert(detail.classList.contains("open"), "detail is open");
  assert(
    detail.parent === detailHost,
    "the panel stays put (not relocated per click)",
  );
  assert(
    collect(detail, "dhl")[0].textContent === "ai:close-candidate (upheld)",
    "header names the clicked state",
  );
  assert(
    collect(detail, "dhc")[0].textContent === "3 items",
    "header count == the upheld list length",
  );
  assert(collect(detail, "li").length === 3, "renders exactly the 3 subjects");

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
        "ai:blocked-on": { count: 1, prs: [] },
      },
      "vetter-verdicts": {
        "ai:ready": { count: 1, prs: [] },
        "ai:reject": { count: 1, prs: [] },
        "ai:relink": { count: 1, prs: [] },
        "ai:design": { count: 1, prs: [] },
        "ai:close-candidate": { count: 1, prs: [] },
      },
      // Drained residue of a state the machine no longer has (issue-pr-cron#225): at zero
      // it draws no box at all. Nonzero it would draw a straggler — its own tests below.
      "producer-blocked": {
        "ai:blocked-deploy": { count: 0, prs: [] },
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
    `headings are the three actors in order: ${
      JSON.stringify(groups.map((g) => g.title))
    }`,
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
    `the retired awaiting-re-vet state renders nowhere: ${
      JSON.stringify(vetter.states)
    }`,
  );
  // ai:blocked-on is the vetter's too (issue-pr-cron#161): deps are typed `--blocked-by`
  // refs and the vetter's state-load clears the flag the run after every dep merges or
  // closes, so no human action sits on the exit path.
  assert(
    vetter.states.includes("ai:blocked-on"),
    `ai:blocked-on files under the vetter: ${JSON.stringify(vetter.states)}`,
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
    `retired ai:relink renders nowhere: ${
      JSON.stringify(groups.map((g) => g.states))
    }`,
  );
  // Human owns the merge/ruling/close states + the upheld close-candidate inbox.
  assert(
    [
      "ai:ready",
      "ai:design",
      "ai:close-candidate (upheld)",
      "human:design",
    ].every((s) => human.states.includes(s)),
    `human states: ${JSON.stringify(human.states)}`,
  );
  assert(
    !human.states.includes("ai:blocked-on"),
    `ai:blocked-on is not in the human's inbox: ${
      JSON.stringify(human.states)
    }`,
  );
  // ai:blocked-deploy is no longer a modeled state (issue-pr-cron#225), so the drained cell
  // in the fixture files under NO actor — it is not a human inbox with nothing in it.
  assert(
    groups.every((g) => !g.states.includes("ai:blocked-deploy")),
    `the drained ai:blocked-deploy residue renders nowhere: ${
      JSON.stringify(groups.map((g) => g.states))
    }`,
  );
  // Every state box sits under exactly one heading (no leaks into a fourth group).
  // 10 = every STATES entry filed once: producer 2 (ai:reject, the untouched backlog),
  // vetter 3 (un-vetted, the unvetted flags, ai:blocked-on), human 5 (leak, ai:ready,
  // ai:design, human:design, the upheld flags).
  const total = groups.reduce((n, g) => n + g.states.length, 0);
  assert(total === 10, `all 10 states filed once: ${total}`);
  // Both directions, same as awaiting-re-vet: a reintroduction fails here. Each retired
  // lane key in the fixture above is a DECOY — a stale snapshot still carrying it must
  // render it in NO group.
  for (
    const retired of [
      "human:reject",
      "ai:close-candidate",
      "ai:close-candidate (PRs)",
      "human:close-candidate",
    ]
  ) {
    assert(
      groups.every((g) => !g.states.includes(retired)),
      `retired ${retired} renders nowhere: ${
        JSON.stringify(groups.map((g) => g.states))
      }`,
    );
  }
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
    counts: {
      leaks: 0,
      ready: 0,
      closeCandidateIssues: 0,
      unvetted: 2,
      awaitingReVet: 7,
    },
    lanes: {
      "vet-lifecycle": {
        "un-vetted": { count: 2, prs: [] },
        "awaiting-re-vet": {
          count: 7,
          prs: [{ repo: "o/a", number: 1, title: "moved head" }],
        },
      },
    },
  });
  const boxes = collect(box, "fsm-state");
  assert(
    !boxes.some((b) => b.dataset.t === "awaiting-re-vet"),
    `no box is keyed to the retired state: ${
      JSON.stringify(boxes.map((b) => b.dataset.t))
    }`,
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
    `un-vetted reads its own count, not the sum: ${
      collect(unvetted, "sc")[0].textContent
    }`,
  );
  // Its ACTION describes the collapsed state, not the half that survived the name. "first
  // vet" would say this box holds only never-judged PRs, which is the deleted machine.
  assert(
    collect(unvetted, "sa")[0].textContent === "vet at current head",
    `the action covers a first vet and a re-vet alike: ${
      collect(unvetted, "sa")[0].textContent
    }`,
  );
  // The vetter heading's total moves with it: 2, never 9.
  const vetter = ownerGroups(box).find((g) =>
    g.title.includes("Vetter action")
  );
  assert(
    vetter.title.endsWith("2"),
    `vetter total is the un-vetted count: ${vetter.title}`,
  );
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
  const unvetted = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "un-vetted"
  );
  assert(unvetted, "un-vetted box rendered");
  const line = tags(unvetted, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  // A flat series normalises to one y for every point. Drop the fold and the first three
  // sit at the floor while the last two sit at the ceiling.
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the collapse: ${
      JSON.stringify(ys)
    }`,
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
  const reject = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "ai:reject"
  );
  assert(reject, "ai:reject box rendered");
  const line = tags(reject, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the consolidation: ${
      JSON.stringify(ys)
    }`,
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
  const reject = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "ai:reject"
  );
  assert(reject, "ai:reject box rendered");
  const line = tags(reject, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the relink retirement: ${
      JSON.stringify(ys)
    }`,
  );
  assert(
    !reject.classList.contains("rising"),
    "folding a flat inventory raises no bottleneck flag",
  );
});

// Same shape for the upheld merge (issue-pr-cron#212): a snapshot written before the
// upheld inbox absorbed its PR half carries `counts.closeCandidatePrs` beside
// `counts.closeCandidateUpheld`, counting disjoint subjects of the one inbox — so the
// quantity the surviving box measures is their SUM at those samples, and the merge draws
// no cliff.
Deno.test("fsm history: pre-merge closeCandidatePrs samples fold into the upheld series, so the merge draws no cliff", () => {
  const now = Date.parse("2026-08-05T12:00:00Z");
  const at = (d) => now - d * DAY;
  // Real inventory flat at 10 across the merge: split 6 + 4 before, one key after.
  const history = [
    { t: at(4), counts: { closeCandidateUpheld: 6, closeCandidatePrs: 4 } },
    { t: at(3), counts: { closeCandidateUpheld: 6, closeCandidatePrs: 4 } },
    { t: at(2), counts: { closeCandidateUpheld: 6, closeCandidatePrs: 4 } },
    { t: at(1), counts: { closeCandidateUpheld: 10 } },
    { t: at(0), counts: { closeCandidateUpheld: 10 } },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateUpheld: 10 },
    lanes: {},
  }, history);
  const upheld = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "closeCandidateUpheld"
  );
  assert(upheld, "upheld box rendered");
  const line = tags(upheld, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(ys.length === 5, `all five samples are plotted: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the line is continuous, with no cliff at the merge: ${JSON.stringify(ys)}`,
  );
  assert(
    !upheld.classList.contains("rising"),
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
  const unvetted = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "un-vetted"
  );
  const pts = tags(unvetted, "polyline")[0].getAttribute("points").split(" ");
  assert(
    pts.length === 2,
    `only the two carrying refreshes are plotted: ${pts.length}`,
  );
  // Both plotted points are the SUM, so the older one (4+2=6) matches the newer (6) and the
  // line is flat — the fold reads the retired key wherever it appears, not just on a key
  // the newest sample happens to carry.
  const ys = pts.map((p) => Number(p.split(",")[1]));
  assert(
    ys[0] === ys[1],
    `4+2 == 6, so the two samples sit level: ${JSON.stringify(ys)}`,
  );
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
  const unvetted = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "un-vetted"
  );
  const points = tags(unvetted, "polyline")[0].getAttribute("points");
  assert(
    !points.includes("NaN"),
    `no NaN reaches the chart geometry: ${points}`,
  );
  const pts = points.split(" ");
  assert(
    pts.length === 2,
    `the junk-only refresh contributes no point: ${points}`,
  );
  const ys = pts.map((p) => Number(p.split(",")[1]));
  assert(
    ys[0] === ys[1],
    `both samples read 6, so the line is flat: ${JSON.stringify(ys)}`,
  );
});

// #69: a historically dual-owner state resolves to ONE actor.
Deno.test("pipeline FSM: ambiguous states resolve to a single owner", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {
      "producer-blocked": {
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
  // ai:blocked-on → vetter (issue-pr-cron#161): the vetter's state-load clears the flag
  // the run after every typed dep merges/closes. This fixture keys the cell under the
  // pre-re-key `producer-blocked` lane, so the box surfacing at all is the stale-lane
  // read working — an old snapshot files under the vetter too, never a fourth bucket.
  assert(
    (owner("ai:blocked-on") || "").includes("Vetter action"),
    `ai:blocked-on is vetter-owned: ${owner("ai:blocked-on")}`,
  );
});

// issue-pr-cron#161 re-keyed ai:blocked-on's lane cell (`producer-blocked` →
// `vet-lifecycle`) when the state moved to the vetter. The box must read the CURRENT
// key: a strict read of the dead key renders 0 (dimmed) under the wrong actor while the
// real inventory renders nowhere — the exact defect this test exists to keep out.
Deno.test("pipeline FSM: a vet-lifecycle blocked-on cell renders its count under Vetter action", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {
      "vet-lifecycle": {
        "un-vetted": { count: 2, prs: [] },
        "ai:blocked-on": {
          count: 17,
          prs: [{
            repo: "o/r",
            number: 1,
            url: "https://github.com/o/r/pull/1",
            title: "blocked",
          }],
        },
      },
    },
  });
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const blocked = byKey("ai:blocked-on");
  assert(blocked, "the blocked-on box renders");
  assert(
    collect(blocked, "sc")[0].textContent === "17",
    `the box reads the vet-lifecycle cell's count: ${
      collect(blocked, "sc")[0].textContent
    }`,
  );
  assert(
    !blocked.className.includes("zero"),
    "17 blocked PRs never read as a dimmed zero",
  );
  // The act is the #161 exit — the vetter's state-load clearance, not a human merge.
  assert(
    collect(blocked, "sa")[0].textContent === "clears when deps merge/close",
    `the act is the automated clearance: ${
      collect(blocked, "sa")[0].textContent
    }`,
  );
  const groups = ownerGroups(box);
  const vetter = groups.find((g) => g.title.includes("Vetter action"));
  const human = groups.find((g) => g.title.includes("Human action"));
  assert(
    vetter.states.includes("ai:blocked-on"),
    `the box sits under Vetter action: ${JSON.stringify(vetter.states)}`,
  );
  // The count lands in the vetter's total (2 un-vetted + 17 blocked-on), not the human's.
  assert(
    vetter.title.endsWith("19"),
    `vetter total carries the 17: ${vetter.title}`,
  );
  assert(human.title.endsWith("0"), `human total does not: ${human.title}`);
  // The click-to-expand lists the cell's own prs — the moved read carries the list too.
  const detail = box.querySelectorAll("#fsmdetail")[0];
  blocked.click();
  assert(
    collect(detail, "li").length === 1,
    `the box expands to its cell's rows: ${collect(detail, "li").length}`,
  );
});

// An old snapshot from before the #161 re-key still carries the cell under
// `producer-blocked`. That inventory must SURFACE — silently reading 0 while the
// snapshot carries real blocked PRs is the failure mode the stale-lane read exists to
// prevent — and when both keys are present the CURRENT lane's cell wins outright.
Deno.test("pipeline FSM: a stale producer-blocked blocked-on cell still surfaces, and the current key wins", () => {
  const stale = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {
      "producer-blocked": {
        "ai:blocked-on": {
          count: 5,
          prs: [{
            repo: "o/r",
            number: 2,
            url: "https://github.com/o/r/pull/2",
            title: "old snapshot",
          }],
        },
      },
    },
  });
  const byKey = (b, k) =>
    collect(b, "fsm-state").find((x) => x.dataset.t === k);
  const staleBox = byKey(stale, "ai:blocked-on");
  assert(
    collect(staleBox, "sc")[0].textContent === "5",
    `the stale cell's count surfaces, never a silent 0: ${
      collect(staleBox, "sc")[0].textContent
    }`,
  );
  assert(
    !staleBox.className.includes("zero"),
    "5 blocked PRs on an old snapshot never dim to zero",
  );
  // It surfaces under the state's owner — the vetter — not under the lane's old actor.
  const vetter = ownerGroups(stale).find((g) =>
    g.title.includes("Vetter action")
  );
  assert(
    vetter.states.includes("ai:blocked-on"),
    `the stale cell files under Vetter action: ${
      JSON.stringify(vetter.states)
    }`,
  );
  // The stale read carries the list too, so the count opens onto its own rows (#141).
  const detail = stale.querySelectorAll("#fsmdetail")[0];
  staleBox.click();
  assert(
    collect(detail, "li").length === 1,
    `the stale cell expands to its rows: ${collect(detail, "li").length}`,
  );
  // Both keys present → ONE box reading the CURRENT lane's cell; the stale cell neither
  // wins nor double-counts.
  const both = fsmBox({
    counts: { leaks: 0, ready: 0, closeCandidateIssues: 0 },
    lanes: {
      "vet-lifecycle": { "ai:blocked-on": { count: 3, prs: [] } },
      "producer-blocked": { "ai:blocked-on": { count: 5, prs: [] } },
    },
  });
  const boxes = collect(both, "fsm-state").filter((x) =>
    x.dataset.t === "ai:blocked-on"
  );
  assert(
    boxes.length === 1,
    `one box, whichever keys the snapshot carries: ${boxes.length}`,
  );
  assert(
    collect(boxes[0], "sc")[0].textContent === "3",
    `the current lane's cell wins, no sum with the stale one: ${
      collect(boxes[0], "sc")[0].textContent
    }`,
  );
});

// The retired states NO surviving state absorbed. `ai:blocked-deploy`
// (issue-pr-cron#225) is DELETED from the machine — no label, no classifier arm, no counts
// emission, no parking transition — and `ai:reject` is not its successor (a reject is a
// verdict on the PR's code, not a fact about a deploy). `ai:blocked-infra`
// (issue-pr-cron#108) exits by the run ending: infra being down is a property of the
// moment, not of a PR, so the producer records `infra-down` and stops. Unlike the ABSORBED
// retirees (awaiting-re-vet, ai:relink, human:reject), whose tokens reappear in a surviving
// state's own cell, a PR still carrying either label reappears NOWHERE — so a snapshot that
// still counts one (an old snapshot, or the label reapplied by hand) surfaces as a
// straggler box in the human group instead of vanishing from the board. Both are held to
// the same behaviour here, because it is one mechanism: the entries in
// `RETIRED_NO_SUCCESSOR`, not per-state special-casing.
const NO_SUCCESSOR = [
  { state: "ai:blocked-deploy", legacy: "blockedDeploy" },
  { state: "ai:blocked-infra", legacy: "blockedInfra" },
];

Deno.test("pipeline FSM: a nonzero no-successor retiree count surfaces as a human-owned straggler box", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  for (const { state, legacy } of NO_SUCCESSOR) {
    // History still carrying the retired key draws NOTHING for it: the series has no
    // surviving state to fold into and the straggler box carries no chart.
    const history = [
      { t: now - 2 * DAY, counts: { ready: 1, [legacy]: 5 } },
      { t: now - DAY, counts: { ready: 1, [legacy]: 4 } },
      { t: now, counts: { ready: 1, [legacy]: 2 } },
    ];
    const box = fsmBox({
      counts: { leaks: 0, ready: 1, [legacy]: 2 },
      lanes: {
        "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } },
        "producer-blocked": {
          [state]: {
            count: 2,
            prs: [
              {
                repo: "rainlanguage/rain.flow",
                number: 12,
                url: "https://github.com/rainlanguage/rain.flow/pull/12",
                title: `stale ${state} straggler`,
              },
              {
                repo: "rainlanguage/rain.dia",
                number: 34,
                url: "https://github.com/rainlanguage/rain.dia/pull/34",
                title: "hand relabelled",
              },
            ],
          },
        },
      },
    }, history);
    const b = collect(box, "fsm-state").find((x) => x.dataset.t === state);
    assert(b, `${state}: the straggler box renders while the count is nonzero`);
    assert(
      collect(box, "fsm-state").filter((x) => x.dataset.t === state).length ===
        1,
      `${state}: exactly one straggler box on the whole board`,
    );
    assert(
      collect(b, "sc")[0].textContent === "2",
      `${state}: it carries the snapshot's count: ${
        collect(b, "sc")[0].textContent
      }`,
    );
    assert(
      collect(b, "sa")[0].textContent === "strip the label",
      `${state}: the act clears the residue: ${
        collect(b, "sa")[0].textContent
      }`,
    );
    assert(
      b.className.includes("blk"),
      `${state}: a stuck pile, kind blk: ${b.className}`,
    );
    assert(
      collect(b, "fsm-spark").length === 0,
      `${state}: no sparkline — the retired series is drawn nowhere`,
    );
    // Filed under the human — the actor who strips the label — and counted in that total
    // (1 ready + 2 stragglers).
    const human = ownerGroups(box).find((g) =>
      g.title.includes("Human action")
    );
    assert(
      human.states.includes(state),
      `${state}: the straggler files under the human: ${
        JSON.stringify(human.states)
      }`,
    );
    assert(
      human.title.endsWith("3"),
      `${state}: the human total counts it: ${human.title}`,
    );
    // Click-through lists the stragglers like any state box.
    b.click();
    const text = textOf(box);
    assert(
      text.includes(`stale ${state} straggler`),
      `${state}: first straggler PR listed`,
    );
    assert(
      text.includes("rain.dia#34"),
      `${state}: second straggler PR listed by repo#number`,
    );
  }
});

// Both retirees at once, on ONE snapshot: two distinct boxes, each opening onto its OWN
// list. They share a lane (`producer-blocked`) and an act, so a mechanism that keyed the
// residue by lane, or registered one list for the pile, would render one box or cross the
// click-throughs — and the total would silently lose a straggler either way.
Deno.test("pipeline FSM: two no-successor retirees on one snapshot render as two independent stragglers", () => {
  const box = fsmBox({
    counts: { leaks: 0, ready: 1, blockedDeploy: 1, blockedInfra: 2 },
    lanes: {
      "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } },
      "producer-blocked": {
        "ai:blocked-deploy": {
          count: 1,
          prs: [{
            repo: "rainlanguage/rain.flow",
            number: 12,
            url: "https://github.com/rainlanguage/rain.flow/pull/12",
            title: "deploy residue",
          }],
        },
        "ai:blocked-infra": {
          count: 2,
          prs: [
            {
              repo: "rainlanguage/rain.dia",
              number: 34,
              url: "https://github.com/rainlanguage/rain.dia/pull/34",
              title: "infra residue A",
            },
            {
              repo: "rainlanguage/rain.flare",
              number: 56,
              url: "https://github.com/rainlanguage/rain.flare/pull/56",
              title: "infra residue B",
            },
          ],
        },
      },
    },
  });
  const at = (k) => collect(box, "fsm-state").filter((x) => x.dataset.t === k);
  assert(at("ai:blocked-deploy").length === 1, "one deploy straggler");
  assert(at("ai:blocked-infra").length === 1, "one infra straggler");
  assert(
    collect(at("ai:blocked-deploy")[0], "sc")[0].textContent === "1" &&
      collect(at("ai:blocked-infra")[0], "sc")[0].textContent === "2",
    "each carries its own count, never the lane's sum",
  );
  // 1 ready + 1 deploy + 2 infra: both stragglers land in the human total.
  const human = ownerGroups(box).find((g) => g.title.includes("Human action"));
  assert(
    human.title.endsWith("4"),
    `both stragglers count toward the human total: ${human.title}`,
  );
  // Each click-through opens onto its own rows, never the other's.
  at("ai:blocked-deploy")[0].click();
  const deployText = textOf(box);
  assert(
    deployText.includes("deploy residue") &&
      !deployText.includes("infra residue"),
    `the deploy box lists only its own PRs: ${deployText}`,
  );
  at("ai:blocked-deploy")[0].click();
  at("ai:blocked-infra")[0].click();
  const infraText = textOf(box);
  assert(
    infraText.includes("infra residue A") &&
      infraText.includes("infra residue B") &&
      !infraText.includes("deploy residue"),
    `the infra box lists only its own PRs: ${infraText}`,
  );
});

// The straggler renders ONLY while a snapshot still counts the state. Absent — the live
// shape, the tool no longer emits it — draws nothing, and so does an emitter that still
// writes the cell at zero: a retired state earns no permanent dimmed box describing a
// machine that no longer exists (rain-org-health#145).
Deno.test("pipeline FSM: a zero or absent no-successor retiree count draws no straggler box", () => {
  const none = (box, state, what) => {
    assert(
      !collect(box, "fsm-state").some((x) => x.dataset.t === state),
      `no box is keyed to ${state} (${what})`,
    );
    assert(
      !collect(box, "sk").some((s) => s.textContent === state),
      `no box is labelled ${state} (${what})`,
    );
  };
  // The live shape: neither key emitted at all.
  const absent = fsmBox({
    counts: { leaks: 0, ready: 1 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } } },
  });
  for (const { state } of NO_SUCCESSOR) none(absent, state, "absent");
  // An emitter that still writes the cell and the counts key, drained to zero.
  for (const { state, legacy } of NO_SUCCESSOR) {
    none(
      fsmBox({
        counts: { leaks: 0, ready: 0, [legacy]: 0 },
        lanes: { "producer-blocked": { [state]: { count: 0, prs: [] } } },
      }),
      state,
      "zero",
    );
  }
});

// A `lanes` snapshot can carry the COUNT without the cell (the counts key set, no `lanes`
// cell — the lanes object is sparse). The count still surfaces, and the click reads as
// detailBody's missing-list defect rather than an empty queue: a count with no list is
// #141's exact signature.
Deno.test("pipeline FSM: a counts-only no-successor straggler surfaces and reports its missing list", () => {
  for (const { state, legacy } of NO_SUCCESSOR) {
    const box = fsmBox({
      counts: { leaks: 0, ready: 0, [legacy]: 4 },
      lanes: { "vetter-verdicts": { "ai:ready": { count: 0, prs: [] } } },
    });
    const b = collect(box, "fsm-state").find((x) => x.dataset.t === state);
    assert(b, `${state}: the counts-only straggler still renders`);
    assert(
      collect(b, "sc")[0].textContent === "4",
      `${state}: it carries the counts key: ${collect(b, "sc")[0].textContent}`,
    );
    b.click();
    const text = textOf(box);
    assert(
      text.includes("the list is missing, not empty"),
      `${state}: a count with no list reads as the missing-list defect: ${text}`,
    );
  }
});

// A pre-`lanes` snapshot carries the count under the flat legacy key and its items under
// the flat `states` map — the straggler surfaces from those, through the same legacy
// branch that draws the surviving states.
Deno.test("pipeline FSM: a pre-lanes snapshot with a nonzero no-successor count surfaces the straggler", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  for (const { state, legacy } of NO_SUCCESSOR) {
    const history = [
      { t: now - DAY, counts: { ready: 1, [legacy]: 6 } },
      { t: now, counts: { ready: 1, [legacy]: 3 } },
    ];
    const box = fsmBox({
      counts: { leaks: 0, ready: 1, [legacy]: 3 },
      states: {
        [state]: [
          {
            repo: "rainlanguage/rain.flare",
            number: 5,
            url: "https://github.com/rainlanguage/rain.flare/pull/5",
            title: "legacy straggler",
          },
        ],
      },
    }, history);
    const b = collect(box, "fsm-state").find((x) => x.dataset.t === state);
    assert(b, `${state}: the straggler renders from the legacy counts key`);
    assert(
      collect(b, "sc")[0].textContent === "3",
      `${state}: it carries the legacy count: ${
        collect(b, "sc")[0].textContent
      }`,
    );
    assert(
      collect(b, "fsm-spark").length === 0,
      `${state}: no sparkline on the legacy straggler either`,
    );
    b.click();
    assert(
      textOf(box).includes("rain.flare#5"),
      `${state}: click lists the legacy stragglers`,
    );
  }
  // A legacy snapshot without either key draws nothing.
  const clean = fsmBox({ counts: { leaks: 0, ready: 1 } });
  for (const { state } of NO_SUCCESSOR) {
    assert(
      !collect(clean, "fsm-state").some((x) => x.dataset.t === state),
      `an absent legacy key draws no ${state} straggler`,
    );
  }
});

// Neither `blockedDeploy` nor `blockedInfra` has a HIST_FOLD entry: no surviving state
// absorbed either inventory, so a surviving series must read its OWN samples only —
// folding a retired key in would fabricate a step in a series that never held that work.
// `ai:reject` is the discriminating target: it is where the PRs that carried
// `ai:blocked-deploy` now sit, which is exactly the resemblance that tempts a fold, and a
// relabel is not an absorption — a reject measures a verdict on the PR's code, never a
// deploy.
Deno.test("fsm history: retired no-successor samples fold into NO surviving series", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  const at = (d) => now - d * DAY;
  // reject holds flat at 2 while both retired keys swing to 0. Unfolded, reject's line is
  // FLAT; folding blockedDeploy would step it 11 → 2, blockedInfra 9 → 2, both 18 → 2.
  const history = [
    {
      t: at(1),
      counts: { reject: 2, ready: 2, blockedDeploy: 9, blockedInfra: 7 },
    },
    {
      t: at(0),
      counts: { reject: 2, ready: 2, blockedDeploy: 0, blockedInfra: 0 },
    },
  ];
  const box = fsmBox({
    counts: { leaks: 0, ready: 2 },
    lanes: {
      "vetter-verdicts": {
        "ai:reject": { count: 2, prs: [] },
        "ai:ready": { count: 2, prs: [] },
      },
    },
  }, history);
  // Every surviving series that draws a line reads flat — no state anywhere absorbs them.
  for (const key of ["ai:reject", "ai:ready"]) {
    const b = collect(box, "fsm-state").find((x) => x.dataset.t === key);
    const points = tags(b, "polyline")[0].getAttribute("points");
    const ys = points.split(" ").map((p) => Number(p.split(",")[1]));
    assert(
      ys[0] === ys[1],
      `${key} reads its own flat 2s, untouched by the retired keys: ${points}`,
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
  assert(
    sevenDaySlope(daily([1, 2, 3, 4, 5, 6, 7, 8]), now) > 0.9,
    "steady rise ≈ +1/day",
  );
  assert(
    Math.abs(sevenDaySlope(daily([5, 5, 5, 5, 5, 5, 5, 5]), now)) < 1e-9,
    "flat ≈ 0",
  );
  assert(
    sevenDaySlope(daily([8, 7, 6, 5, 4, 3, 2, 1]), now) < -0.9,
    "steady fall ≈ -1/day",
  );
  assert(
    sevenDaySlope([{ t: now, v: 5 }], now) === null,
    "single point → null (no trend)",
  );
  assert(sevenDaySlope([], now) === null, "empty series → null");
  // Points whose window is entirely older than 7 days no longer count as a trend.
  const stale = [{ t: now - 20 * DAY, v: 1 }, { t: now - 15 * DAY, v: 9 }];
  assert(
    sevenDaySlope(stale, now) === null,
    "all points outside the 7d window → null",
  );
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
  const ready = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "ai:ready"
  );
  assert(ready, "ai:ready state box rendered");
  assert(
    collect(ready, "fsm-spark").length === 1,
    "one inline sparkline in the state",
  );
  assert(tags(ready, "polyline").length === 1, "sparkline draws a polyline");
  assert(
    tags(ready, "circle").length === 1,
    "sparkline has an emphasized endpoint dot",
  );
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
    counts: {
      leaks: 0,
      ready: 9,
      closeCandidateIssues: 0,
      uncoveredIssues: 616,
    },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 9, prs: [] } } },
  };
  const box = fsmBox(hq, history);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  const untouched = byKey("uncoveredIssues");
  assert(untouched, "untouched (no PR) state box rendered");
  assert(
    collect(untouched, "fsm-spark").length === 1,
    "a single-sample state still gets a sparkline",
  );
  assert(
    tags(untouched, "circle").length === 1,
    "the single sample renders as an endpoint dot",
  );
  assert(tags(untouched, "polyline").length === 0, "one point draws no line");
  // A single point can't establish a trend, so it is still never flagged as the bottleneck.
  assert(
    !untouched.classList.contains("rising"),
    "one sample never flags a bottleneck",
  );
  // Siblings with a full series are untouched by this: line AND dot, exactly as before.
  const ready = byKey("ai:ready");
  assert(
    tags(ready, "polyline").length === 1,
    "a multi-sample sibling still draws its line",
  );
  assert(
    tags(ready, "circle").length === 1,
    "a multi-sample sibling still draws its dot",
  );
});

// issue-pr-cron#73: the close-candidate lifecycle gains a vetter verdict, so the single
// "all flagged" figure splits into two inboxes with DIFFERENT owners — unvetted (the
// vetter judges the flag) and upheld (a human closes the subject).
Deno.test("pipeline FSM: close-candidate issues split into a vetter inbox and a human inbox", () => {
  const box = fsmBox({
    counts: {
      leaks: 0,
      ready: 0,
      closeCandidateIssues: 29,
      closeCandidateUnvetted: 20,
      closeCandidateUpheld: 9,
    },
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
  // The upheld act is the bare close: the box holds issues and PRs alike, so an act
  // naming one subject type would mislabel the other half of its own list.
  assert(
    collect(upheld, "sa")[0].textContent === "close",
    `the upheld act is subject-type-neutral: ${
      collect(upheld, "sa")[0].textContent
    }`,
  );
});

// One flag now produces TWO inboxes with DIFFERENT owners, so their sum is not a state
// anyone acts on. No combined box exists in the lanes view, and each split state counts
// toward its own actor's total — the invariant a combined box could not satisfy.
Deno.test("pipeline FSM: no combined close-candidate box; each split state counts toward its own actor total", () => {
  const box = fsmBox({
    counts: {
      leaks: 0,
      ready: 0,
      closeCandidateIssues: 29,
      closeCandidateUnvetted: 20,
      closeCandidateUpheld: 9,
    },
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
  assert(
    vetter.title.endsWith("20"),
    `vetter total counts the 20 unvetted: ${vetter.title}`,
  );
  assert(
    human.title.endsWith("9"),
    `human total counts the 9 upheld: ${human.title}`,
  );
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
    {
      t: Date.parse("2026-07-25T00:00:00Z"),
      counts: { closeCandidateIssues: 29 },
    },
    {
      t: Date.parse("2026-07-26T00:00:00Z"),
      counts: { closeCandidateIssues: 29 },
    },
  ]);
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  for (const k of ["closeCandidateUnvetted", "closeCandidateUpheld"]) {
    const b = byKey(k);
    assert(b, `${k} box still rendered with the key absent`);
    // `zero` is baked into the className at construction (only `rising` is added via
    // classList), so read the string the box was built with.
    assert(b.className.includes("zero"), `${k} reads as zero, dimmed`);
    assert(
      collect(b, "fsm-spark").length === 0,
      `${k} has no spark with no samples`,
    );
    assert(
      !b.classList.contains("rising"),
      `${k} cannot flag a bottleneck with no data`,
    );
  }
});

// The first refresh after the tool starts emitting a split key gives it a single sample.
// Per #129 that must draw its lone endpoint dot rather than vanishing.
Deno.test("pipeline FSM: a freshly-emitted close-candidate split key draws its lone endpoint dot", () => {
  const now = Date.parse("2026-07-26T06:22:06Z");
  const at = (d) => now - d * DAY;
  const history = [0, 1, 2, 3, 4].map((i) => ({
    t: at(4 - i),
    counts: i === 4
      ? { closeCandidateIssues: 29, closeCandidateUnvetted: 20 }
      : { closeCandidateIssues: 29 },
  }));
  const box = fsmBox({
    counts: {
      leaks: 0,
      ready: 0,
      closeCandidateIssues: 29,
      closeCandidateUnvetted: 20,
    },
    lanes: {},
  }, history);
  const unvetted = collect(box, "fsm-state").find((b) =>
    b.dataset.t === "closeCandidateUnvetted"
  );
  assert(
    collect(unvetted, "fsm-spark").length === 1,
    "one sample still gets a sparkline",
  );
  assert(
    tags(unvetted, "circle").length === 1,
    "the sample renders as an endpoint dot",
  );
  assert(tags(unvetted, "polyline").length === 0, "one point draws no line");
  assert(
    !unvetted.classList.contains("rising"),
    "one sample never flags a bottleneck",
  );
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
    counts: {
      leaks: 0,
      ready: 54,
      reject: 10,
      relink: 4,
      closeCandidateIssues: 0,
    },
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
  const readyBox = byKey("ai:ready"),
    rejectBox = byKey("ai:reject"),
    relinkBox = byKey("ai:relink");
  assert(
    readyBox.classList.contains("rising"),
    "up-trending ai:ready gets the rising border",
  );
  assert(
    !rejectBox.classList.contains("rising"),
    "the folded blip stays under epsilon, not flagged",
  );
  // (the under-epsilon blip rides reject's FOLDED series above — retired relink renders no box)
  // Never color-alone: a visible ▲ badge + an aria-label spell the flag out.
  assert(
    collect(readyBox, "fsm-rise").length === 1,
    "rising state carries a visible ▲ cue",
  );
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
    `a non-rising state never claims a trend: ${
      rejectBox.getAttribute("aria-label")
    }`,
  );
  assert(
    !(collect(rejectBox, "fsm-rise")[0]?.getAttribute("title") || "").includes(
      "rising",
    ),
    "the ▲ on a fallback pick does not claim a trend either",
  );
  // ai:relink is fully retired (issue-pr-cron#135/#139): the fixture still carries its
  // lane data and history — an old snapshot — and it must render NO box at all; its
  // history folds into reject's series via HIST_FOLD instead.
  assert(
    relinkBox === undefined,
    "retired ai:relink draws no box, even from a snapshot that carries it",
  );
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
  assert(
    ready.classList.contains("lead"),
    "human's largest queue (44 ai:ready) is marked",
  );
  assert(
    !ready.classList.contains("rising"),
    "identical on screen, still distinct in the DOM",
  );
  // The fallback wears the EXISTING highlight, not a second visual language: the same ▲ in
  // the same row, and a `.lead` class whose stylesheet rule is shared verbatim with
  // `.rising`. A reader sees one kind of "look here", never two competing ones.
  const up = collect(ready, "fsm-rise");
  assert(
    up.length === 1,
    "the fallback carries the same visible ▲ cue a bottleneck does",
  );
  assert(
    up[0]._text === "▲",
    `the same glyph, not a substitute: ${up[0]._text}`,
  );
  assert(
    ready.querySelector(".fsm-scrow").children.includes(up[0]),
    "the ▲ sits in the same row it does on a bottleneck",
  );
  assert(
    collect(ready, "fsm-lead").length === 0,
    "no separate chip — nothing new is invented",
  );
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
  assert(
    !byKey("ai:design").classList.contains("lead"),
    "a smaller queue in the group is not marked",
  );
  assert(
    byKey("ai:reject").classList.contains("lead"),
    "producer's largest queue is marked too",
  );
  assert(
    byKey("un-vetted").classList.contains("lead"),
    "vetter's largest queue is marked too",
  );
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
  assert(
    byKey("ai:ready").classList.contains("rising"),
    "the rising flag still fires",
  );
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
    counts: { leaks: 0, ready: 0, design: 0, closeCandidateUpheld: 7 },
    lanes: {
      "vetter-verdicts": {
        "ai:reject": { count: 7, prs: [] },
        "ai:relink": { count: 7, prs: [] },
      },
      "human-decisions": {
        "human:design": { count: 7, prs: [] },
      },
    },
  });
  const byKey = (k) => collect(box, "fsm-state").find((b) => b.dataset.t === k);
  assert(
    !byKey("un-vetted").classList.contains("lead"),
    "an all-zero group is never marked",
  );
  assert(
    !byKey("ai:design").classList.contains("lead"),
    "no work really is nothing to do",
  );
  // ai:relink is fully retired (issue-pr-cron#135/#139): its equal count renders no box
  // and leads nothing — reject leads its group with relink data present but inert.
  assert(
    byKey("ai:reject").classList.contains("lead"),
    "reject leads the producer group",
  );
  assert(
    byKey("ai:relink") === undefined,
    "retired ai:relink draws no box and leads nothing",
  );
  // Same-group ties still break toward the earlier STATES entry: the human group carries
  // two equal states, and human:design precedes the upheld close-candidate box.
  assert(
    byKey("human:design").classList.contains("lead"),
    "ties break toward the earlier STATES entry",
  );
  assert(
    !byKey("closeCandidateUpheld").classList.contains("lead"),
    "the later of two equal counts is not marked",
  );
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
  assert(
    collect(box, "fsm-spark").length === 0,
    "no sparklines without a history rollup",
  );
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
  assert(
    pts.length === 2,
    `only the two well-formed lines are kept: ${pts.length}`,
  );
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
    const p = {
      t: now - (sizes.length - 1 - i) * DAY,
      counts: { openIssues: n },
    };
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
  const l = tags(node, "polyline").find((x) =>
    x.className.split(" ").includes(cls)
  );
  return l
    ? l.getAttribute("points").split(" ").map((p) => p.split(",").map(Number))
    : null;
};

Deno.test("fsm ages: parseHistory carries a well-formed ages block and drops a malformed one — without costing the line its counts", () => {
  const text = [
    '{"ts":"2026-07-28T00:00:00Z","counts":{"openIssues":800}}',
    '{"ts":"2026-07-29T00:00:00Z","counts":{"openIssues":801},"ages":{"openIssues":{"meanDays":333.8,"medianDays":99.0,"oldestDays":1654.5}}}',
    '{"ts":"2026-07-30T00:00:00Z","counts":{"openIssues":802},"ages":"not an object"}',
  ].join("\n");
  const pts = parseHistory(text);
  assert(pts.length === 3, `all three lines keep their counts: ${pts.length}`);
  assert(
    !("ages" in pts[0]),
    "a line without ages yields a point without the key — absence, not null",
  );
  assert(
    pts[1].ages.openIssues.meanDays === 333.8 &&
      pts[1].ages.openIssues.medianDays === 99.0 &&
      pts[1].ages.openIssues.oldestDays === 1654.5,
    "all three statistics ride through verbatim, as siblings",
  );
  assert(
    !("ages" in pts[2]),
    "a malformed ages block is dropped, not forwarded",
  );
  assert(
    pts[2].counts.openIssues === 802,
    "…and never invalidates the line's counts",
  );
});

Deno.test("fsm open band: the open-issue population draws its size and BOTH statistics, as equals", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [
    320,
    95,
    1620,
  ], [333.8, 99.0, 1654.5]]);
  const box = fsmBox(
    {
      counts: { openIssues: 802, uncoveredIssues: 617 },
      lanes: {},
      ages: agesOf(333.8, 99.0, 1654.5),
    },
    history,
  );
  const band = bandOf(box);
  assert(band, "a snapshot carrying the population draws the band");
  // Above the machine: this is the population the machine has to move, and the one figure on
  // the panel that belongs to no actor's inbox.
  assert(
    box.children[0] === band,
    "the band leads the panel, ahead of the actor lanes",
  );
  assert(
    collect(band, "oi-n")[0].textContent === "802",
    "the population's own size, from counts.openIssues",
  );
  assert(
    textOf(band).includes("every open issue in the pipeline's scope"),
    `the copy says which population this is: "${textOf(band)}"`,
  );
  const stats = collect(band, "oi-stat");
  assert(stats.length === 2, `mean and median, both drawn: ${stats.length}`);
  assert(
    textOf(stats[0]).includes("mean") && textOf(stats[0]).includes("333.8d"),
    `mean, unit attached: "${textOf(stats[0])}"`,
  );
  assert(
    textOf(stats[1]).includes("median") && textOf(stats[1]).includes("99d"),
    `median, unit attached: "${textOf(stats[1])}"`,
  );
  // EQUALS: same row class, same value class, one trend chart each — neither is a headline
  // and neither is a footnote hidden in a tooltip.
  assert(
    collect(band, "oi-sv").length === 2,
    "both values carry the same value class",
  );
  assert(
    collect(band, "oi-mean").length === 1 &&
      collect(band, "oi-med").length === 1,
    "each statistic gets its own trend chart — one is never drawn without the other",
  );
  // The tail number is detail, and VISIBLE detail — a tooltip is unreadable on a touch
  // screen and invisible in a screenshot.
  assert(
    collect(band, "oi-tail")[0].textContent === "oldest 1654.5d",
    `oldest renders as visible text: "${
      collect(band, "oi-tail").map((n) => n.textContent)
    }"`,
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
  const box = fsmBox({
    counts: { openIssues: 802 },
    lanes: {},
    ages: agesOf(333.8, 99.0, 1654.5),
  }, history);
  const band = bandOf(box);
  for (const [chart, name] of [["oi-mean", "mean"], ["oi-med", "median"]]) {
    const ys = ptsOf(chartOf(band, chart), "a-l").map((pt) => pt[1]);
    assert(
      Math.min(...ys) === OI_PAD && Math.max(...ys) === OI_AGE.h - OI_PAD,
      `the ${name} trend spans its own full height [${OI_PAD},${
        OI_AGE.h - OI_PAD
      }], got [${Math.min(...ys)},${
        Math.max(...ys)
      }] — on a ruler set by the OTHER statistic's level it would be a flat smear`,
    );
  }
  // …and the two are drawn identically: same stroke class, same geometry. Nothing in the
  // rendering ranks one above the other.
  const geom = (chart) => {
    const svg = chartOf(band, chart);
    return svg.getAttribute("width") + "x" + svg.getAttribute("height");
  };
  assert(
    geom("oi-mean") === geom("oi-med"),
    `same chart geometry for both: ${geom("oi-mean")} vs ${geom("oi-med")}`,
  );
  assert(
    tags(band, "polyline").filter((l) => l.className.split(" ").includes("a-l"))
      .length === 2,
    "both age trends draw in the same stroke — the names tell them apart, not a visual hierarchy",
  );
});

Deno.test("fsm open band: one quantity per chart — no chart mixes days with items", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [
    320,
    95,
    1620,
  ], [333.8, 99.0, 1654.5]]);
  const box = fsmBox(
    {
      counts: { openIssues: 802, uncoveredIssues: 617 },
      lanes: {},
      ages: agesOf(333.8, 99.0, 1654.5),
    },
    history,
  );
  const band = bandOf(box);
  assert(
    collect(band, "oi-chart").length === 3,
    "three quantities, three charts",
  );
  const cls = (n) => tags(n, "polyline").map((l) => l.className);
  for (
    const [chart, want] of [["oi-size", "q-l"], ["oi-mean", "a-l"], [
      "oi-med",
      "a-l",
    ]]
  ) {
    const c = chartOf(band, chart);
    assert(c, `${chart} draws`);
    assert(
      cls(c).length === 1 && cls(c)[0].split(" ").includes(want),
      `${chart} holds exactly one series, its own: ${cls(c)}`,
    );
  }
  assert(
    cls(chartOf(band, "oi-size")).every((k) => !k.includes("a-l")),
    "no age line inside the queue-size chart",
  );
  // …and the machine's own sparklines are untouched: no age mark ever enters a state box.
  const b = collect(box, "fsm-state").find((x) =>
    x.dataset.t === "uncoveredIssues"
  );
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
  const history = openHistory(now, [800, 801, 802, 802], [null, null, [
    320,
    95,
    1600,
  ], [340, 100, 1654.5]]);
  const box = fsmBox({
    counts: { openIssues: 802 },
    lanes: {},
    ages: agesOf(340, 100, 1654.5),
  }, history);
  const band = bandOf(box);
  const frac = (chart, cls, w) => {
    const pts = ptsOf(chartOf(band, chart), cls);
    return pts.map(([x]) => (x - OI_PAD) / (w - 2 * OI_PAD));
  };
  const size = frac("oi-size", "q-l", OI_SIZE.w),
    mean = frac("oi-mean", "a-l", OI_AGE.w);
  assert(
    Math.abs(size[0]) < 0.02,
    `the size series starts at the left edge: ${size[0]}`,
  );
  assert(
    Math.abs(size[size.length - 1] - 1) < 0.02,
    `…and ends at the right: ${size[size.length - 1]}`,
  );
  assert(
    Math.abs(mean[0] - 2 / 3) < 0.02,
    `the ages start two thirds in, where their first refresh is — not at the left edge of their own private axis: ${
      mean[0]
    }`,
  );
  assert(
    Math.abs(mean[mean.length - 1] - 1) < 0.02,
    "…and end at the same right edge as the size",
  );
});

Deno.test("fsm open band: the current statistics come from the snapshot, else the newest rollup line that carries them", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(now, [800, 801, 802], [[300, 90, 1600], [
    320,
    95,
    1620,
  ], [333.8, 99.0, 1654.5]]);
  // The snapshot is the present and wins, exactly as counts do.
  let band = bandOf(
    fsmBox({
      counts: { openIssues: 802 },
      lanes: {},
      ages: agesOf(400, 120, 1700),
    }, history),
  );
  assert(
    textOf(band).includes("400d") && textOf(band).includes("120d"),
    `snapshot wins: "${textOf(band)}"`,
  );
  // No snapshot block: the newest rollup sample stands in.
  band = bandOf(fsmBox({ counts: { openIssues: 802 }, lanes: {} }, history));
  assert(
    textOf(band).includes("333.8d") && textOf(band).includes("99d") &&
      textOf(band).includes("oldest 1654.5d"),
    `newest rollup sample: "${textOf(band)}"`,
  );
  // A newer refresh whose coverage read failed carries no ages — the newest line that HAS
  // them is the current reading, not "no reading at all".
  const stale = history.concat([{ t: now + DAY, counts: { openIssues: 803 } }]);
  band = bandOf(fsmBox({ counts: { openIssues: 803 }, lanes: {} }, stale));
  assert(
    textOf(band).includes("333.8d"),
    `the newest line that carried the ages: "${textOf(band)}"`,
  );
  // Selection is by TIMESTAMP, never by array position: this page does not assume the order
  // it is handed, the same way every series builder sorts before drawing. Reversed, the
  // NEWEST reading is now the array's first element.
  band = bandOf(
    fsmBox(
      { counts: { openIssues: 802 }, lanes: {} },
      history.slice().reverse(),
    ),
  );
  assert(
    textOf(band).includes("333.8d") && !textOf(band).includes("300d"),
    `an out-of-order rollup still reads the newest reading, not the last element: "${
      textOf(band)
    }"`,
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
  assert(
    collect(box, "fsm-open").length === 0,
    "no band — absence degrades to exactly the page as it was",
  );
  assert(
    collect(box, "oi-k").length === 0,
    "no population copy about a population the snapshot never reported",
  );
  assert(
    !textOf(box).includes("median age in days"),
    "and no legend describing a mark that is not on screen",
  );
  const b = collect(box, "fsm-state").find((x) =>
    x.dataset.t === "uncoveredIssues"
  );
  assert(
    collect(b, "fsm-spark").length === 1,
    "the machine's own sparkline still draws",
  );
});

// The retired key measured the producer backlog (622). Reading it into a band labelled
// "every open issue" would show a 622-issue number under an 802-issue heading — the one
// failure mode worse than drawing nothing.
Deno.test("fsm open band: the retired ages.uncoveredIssues is never read as this population's age", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = [
    {
      t: now - DAY,
      counts: { uncoveredIssues: 610 },
      ages: { uncoveredIssues: { medianDays: 41.0, oldestDays: 812.3 } },
    },
    {
      t: now,
      counts: { uncoveredIssues: 617 },
      ages: { uncoveredIssues: { medianDays: 43.0, oldestDays: 813.3 } },
    },
  ];
  const box = fsmBox(
    {
      counts: { uncoveredIssues: 617 },
      lanes: {},
      ages: { uncoveredIssues: { medianDays: 43.0, oldestDays: 813.3 } },
    },
    history,
  );
  assert(
    collect(box, "fsm-open").length === 0,
    "the narrower population's ages are not this one's — no band",
  );
  assert(
    collect(box, "oi-stat").length === 0,
    "and no statistic anywhere on the page",
  );
});

// `Number()` coercion would turn a malformed `null`/`""` into a fabricated 0 — for the ages
// a "0d" the recording side refuses to emit, and for the size a claim that every issue in
// the org closed at once, which is why a failed coverage read omits the key entirely.
Deno.test("fsm open band: null, string and negative values render nothing — never a fabricated zero", () => {
  for (const v of [null, "99", -5]) {
    const box = fsmBox(
      {
        counts: { openIssues: v, uncoveredIssues: 617 },
        lanes: {},
        ages: { openIssues: { meanDays: v, medianDays: v, oldestDays: v } },
      },
      [],
    );
    assert(
      collect(box, "fsm-open").length === 0,
      `openIssues=${
        JSON.stringify(v)
      } draws no band — a coerced 0 would claim an empty, ageless backlog`,
    );
  }
  // A size that did not arrive alongside statistics that did: the size reads as unreported,
  // never as zero, and the statistics still draw.
  const box = fsmBox({
    counts: { openIssues: null, uncoveredIssues: 617 },
    lanes: {},
    ages: agesOf(333.8, 99.0, 1654.5),
  }, []);
  assert(
    collect(box, "oi-n")[0].textContent === "—",
    `an unreported size is a dash: "${collect(box, "oi-n")[0].textContent}"`,
  );
  assert(
    collect(box, "oi-stat").length === 2,
    "…and both statistics still draw",
  );
  // A malformed oldest never costs the two statistics their rows.
  const box2 = fsmBox(
    {
      counts: { openIssues: 802 },
      lanes: {},
      ages: {
        openIssues: { meanDays: 333.8, medianDays: 99.0, oldestDays: "-" },
      },
    },
    [],
  );
  assert(
    collect(box2, "oi-stat").length === 2,
    "mean and median survive a malformed tail",
  );
  assert(collect(box2, "oi-tail").length === 0, "no fabricated oldest");
});

Deno.test("fsm open band: a refresh that carried no ages contributes no point — a gap is never plotted as zero", () => {
  const now = Date.parse("2026-08-05T00:00:00Z");
  const history = openHistory(
    now,
    [800, 801, 802, 802],
    [null, [300, 90, 1600], null, [340, 100, 1654.5]],
  );
  const box = fsmBox({
    counts: { openIssues: 802 },
    lanes: {},
    ages: agesOf(340, 100, 1654.5),
  }, history);
  const band = bandOf(box);
  const mean = ptsOf(chartOf(band, "oi-mean"), "a-l");
  assert(
    mean.length === 2,
    `only the two refreshes that reported a mean are plotted: ${mean.length}`,
  );
  // A fabricated 0 for the missing refresh would be the series' minimum and would pin to the
  // bottom of the chart; with only the two real readings, the LOWER of them is the minimum.
  assert(
    Math.max(...mean.map((pt) => pt[1])) === OI_AGE.h - OI_PAD &&
      mean[0][1] === OI_AGE.h - OI_PAD,
    `the earlier real reading is the series minimum — a gap plotted as 0d would take that place: ${
      mean.map((pt) => pt[1])
    }`,
  );
  const size = ptsOf(chartOf(band, "oi-size"), "q-l");
  assert(
    size.length === 4,
    `the size was carried on every line, so all four points draw: ${size.length}`,
  );
});

Deno.test("fsm open band: the legend explains the two age series only when they are drawn", () => {
  // A size with no ages at all: the band reports the size and explains no age mark.
  let box = fsmBox({ counts: { openIssues: 802 }, lanes: {} }, []);
  assert(
    collect(box, "fsm-open").length === 1,
    "a reported size still draws, ages or not",
  );
  assert(collect(box, "oi-stat").length === 0, "no statistics to draw");
  assert(
    collect(box, "oi-chart").length === 0,
    "and no chart: an empty series is no chart, never an empty frame",
  );
  assert(
    !textOf(box).includes("median age in days"),
    "…so nothing about them in the legend",
  );
  // An empty block draws nothing either, and explains nothing.
  box = fsmBox({ counts: { openIssues: 802 }, lanes: {}, ages: {} }, []);
  assert(
    !textOf(box).includes("median age in days"),
    "an empty ages block gets no legend",
  );
  // Drawable statistics: exactly one sentence, and it names what the reader is looking at.
  box = fsmBox({
    counts: { openIssues: 802 },
    lanes: {},
    ages: agesOf(333.8, 99.0, 1654.5),
  }, []);
  const txt = textOf(box);
  assert(
    txt.includes("median age in days") && txt.includes("mean"),
    `the drawn marks are explained: "${txt}"`,
  );
  assert(
    txt.includes("own trend on its own scale"),
    "…including the one property a reader must know before comparing two charts by eye",
  );
  assert(
    txt.includes("is that tail"),
    "…and it names the tail number, which is on screen",
  );
  // A malformed oldest costs the two statistics nothing, draws no tail number — and must
  // therefore cost the legend its tail clause too.
  box = fsmBox(
    {
      counts: { openIssues: 802 },
      lanes: {},
      ages: {
        openIssues: { meanDays: 333.8, medianDays: 99.0, oldestDays: "-" },
      },
    },
    [],
  );
  const noTail = textOf(box);
  assert(collect(box, "oi-tail").length === 0, "no tail number is drawn");
  assert(
    noTail.includes("median age in days"),
    "the two statistics are still explained",
  );
  assert(
    !noTail.includes("is that tail"),
    `…and the legend names no tail that is not there: "${noTail}"`,
  );
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
  const box = fsmBox({
    counts: { uncoveredIssues: 600, openIssues: 802 },
    lanes: {},
    ages: agesOf(240, 80, 1654.5),
  }, history);
  const b = collect(box, "fsm-state").find((x) =>
    x.dataset.t === "uncoveredIssues"
  );
  assert(
    !b.classList.contains("rising"),
    "a flat count is not a bottleneck, however fast the population ages",
  );
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
  const producer = ownerGroups(box).find((g) =>
    g.title.includes("Producer action")
  );
  assert(producer, "a Producer action group exists");
  assert(
    producer.states.includes("untouched (no PR)"),
    `producer owns the untouched-backlog box: ${
      JSON.stringify(producer.states)
    }`,
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
      collect(detail, "dhc")[0].textContent ===
        n + (n === 1 ? " item" : " items"),
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
  assert(
    etxt.includes("Empty"),
    `a zero state reads as deliberately empty: ${etxt}`,
  );
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
        // ai:design deliberately absent: a sparse lane must render a zero box that
        // expands to zero rows, which is the same invariant at n = 0.
      },
      // A no-successor retiree still counted by an old snapshot: it draws a STRAGGLER box,
      // which is held to this invariant like any other — a residue count that opened onto
      // someone else's rows would be #141 all over again, in the one place added last.
      "producer-blocked": { "ai:blocked-deploy": laneCell("deploy", 3) },
      // Absorbed retiree in the data (old snapshot): must render NO box and NO rows, so it
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
  // 11 = the 10 STATES entries + the ai:blocked-deploy straggler the stale cell surfaces.
  assert(checked === 11, `the whole machine was walked, got ${checked} boxes`);
  assert(
    box.querySelectorAll("[data-t]").some((b) =>
      b.dataset.t === "ai:blocked-deploy"
    ),
    "the straggler is one of the boxes walked, not skipped by the sweep",
  );
  // Guard the guard: a fixture that zeroed everything would satisfy the invariant vacuously.
  assert(
    nonZero >= 8,
    `the fixture must exercise non-zero states, got ${nonZero}`,
  );
  assert(
    mismatches.length === 0,
    `a count that opens onto a different number of rows:\n  ${
      mismatches.join("\n  ")
    }`,
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
      {
        repo: "o/i",
        number: 9,
        url: "https://github.com/o/i/pull/9",
        title: "already resolved",
      },
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
  box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "uncoveredIssues"
  ).click();
  const text = textOf(detail);
  // The row AFTER the malformed ones still rendered — i.e. nothing threw partway through.
  assert(
    text.includes("first good"),
    `the row before the malformed ones survives: ${text}`,
  );
  assert(
    text.includes("last good"),
    `the row AFTER the malformed ones survives: ${text}`,
  );
  // Four entries in, four rows out: the header count and the rows still agree.
  assert(
    collect(detail, "li").length === 4,
    `every entry gets a row, malformed included: ${
      collect(detail, "li").length
    }`,
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

// ---- rain-org-health#164: the machine rendered from emitted stateDescriptors ----------
//
// The consumer half of issue-pr-cron#130. A snapshot carrying `stateDescriptors` IS the
// machine: owner grouping, box (key/label/act/kind), occupancy from the descriptor's
// declared source, history folds from `histFold`, descriptor order as render order. The
// hand-written STATES table is the FROZEN fallback for descriptor-less snapshots only.
// Fixtures carry DECOY values on the source a state must NOT read (a counts mirror beside
// a lane cell), so a wrong-source read changes a rendered number instead of passing.

// A canonical descriptor snapshot: both occupancy kinds, a re-laned state (ai:blocked-on
// under the VETTER reading `vet-lifecycle` — the exact drift the frozen table could not
// follow), and a sparse-absent lane state.
function descHq() {
  return {
    counts: {
      // The lane states' `hist` mirrors, AGREEING with their cells — a self-consistent
      // snapshot. A mirror that contradicts its cell is itself a reported defect now
      // (the cross-check), so the wrong-source read is discriminated there rather than
      // by a decoy that would make every fixture using descHq render a defect band.
      ready: 5,
      blockedOn: 3,
      uncoveredIssues: 4,
      leaks: 2,
    },
    lanes: {
      "vetter-verdicts": { "ai:ready": { count: 5, prs: fcItems("ready", 5) } },
      "vet-lifecycle": {
        "ai:blocked-on": { count: 3, prs: fcItems("blocked", 3) },
      },
    },
    uncoveredIssues: fcItems("uncovered", 4),
    leaks: fcItems("leak", 2),
    stateDescriptors: [
      {
        key: "uncoveredIssues",
        owner: "producer",
        act: "open a PR",
        kind: "flow",
        hist: "uncoveredIssues",
        histFold: [],
        occupancy: {
          counts: "uncoveredIssues",
          items: "uncoveredIssues",
          itemsAreIssues: true,
        },
        label: "untouched (no PR)",
      },
      {
        key: "ai:blocked-on",
        owner: "vetter",
        act: "clears when every typed dep merges/closes",
        kind: "blk",
        hist: "blockedOn",
        histFold: [],
        occupancy: { lane: "vet-lifecycle" },
      },
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
      // Sparse-absent lane cell (renders 0, dimmed) deliberately BETWEEN two non-zero
      // states: descriptor order is NOT count order, so an ordering mutant cannot pass by
      // sorting.
      {
        key: "ai:design",
        owner: "human",
        act: "rule on design Q",
        kind: "rule",
        hist: "design",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
      {
        key: "leak",
        owner: "human",
        act: "model it",
        kind: "blk",
        hist: "leaks",
        histFold: [],
        occupancy: { counts: "leaks", items: "leaks" },
      },
    ],
  };
}

Deno.test("pipeline FSM: stateDescriptors drive the machine — owner grouping, act, count, and descriptor order", () => {
  const box = fsmBox(descHq());
  // Exactly the descriptor states render, in owner-group order then descriptor order —
  // and NONE of the frozen table's other boxes (un-vetted, closeCandidateUpheld, …).
  const keys = box.querySelectorAll("[data-t]").map((b) => b.dataset.t);
  assert(
    JSON.stringify(keys) ===
      JSON.stringify([
        "uncoveredIssues",
        "ai:blocked-on",
        "ai:ready",
        "ai:design",
        "leak",
      ]),
    `descriptor order is render order, and only descriptor states render: ${
      JSON.stringify(keys)
    }`,
  );
  const groups = ownerGroups(box);
  assert(groups.length === 3, `three actor groups: ${groups.length}`);
  const [producer, vetter, human] = groups;
  assert(
    JSON.stringify(producer.states) === JSON.stringify(["untouched (no PR)"]),
    `producer files the descriptor's own states: ${
      JSON.stringify(producer.states)
    }`,
  );
  // The re-laned state files under the VETTER — the descriptor's owner, not the frozen
  // table's human resolution.
  assert(
    JSON.stringify(vetter.states) === JSON.stringify(["ai:blocked-on"]),
    `blocked-on is vetter-owned by descriptor: ${
      JSON.stringify(vetter.states)
    }`,
  );
  assert(
    JSON.stringify(human.states) ===
      JSON.stringify(["ai:ready", "ai:design", "leak"]),
    `human states in descriptor order: ${JSON.stringify(human.states)}`,
  );
  // Counts come from each descriptor's DECLARED source: the vet-lifecycle cell (3), never
  // the counts mirror (99) and never the frozen table's producer-blocked lane (absent).
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  assert(
    collect(byT("ai:blocked-on"), "sc")[0].textContent === "3",
    `blocked-on reads its declared lane cell: ${
      collect(byT("ai:blocked-on"), "sc")[0].textContent
    }`,
  );
  assert(
    vetter.title.endsWith("3"),
    `the vetter total is the cell's count: ${vetter.title}`,
  );
  assert(
    collect(byT("ai:ready"), "sc")[0].textContent === "5",
    `ready reads its lane cell: ${
      collect(byT("ai:ready"), "sc")[0].textContent
    }`,
  );
  assert(
    collect(byT("uncoveredIssues"), "sc")[0].textContent === "4",
    `a counts-source state reads counts.<key>: ${
      collect(byT("uncoveredIssues"), "sc")[0].textContent
    }`,
  );
  // act + kind ride the descriptor.
  assert(
    collect(byT("ai:blocked-on"), "sa")[0].textContent ===
      "clears when every typed dep merges/closes",
    `the act is the descriptor's: ${
      collect(byT("ai:blocked-on"), "sa")[0].textContent
    }`,
  );
  assert(
    byT("ai:blocked-on").className.split(" ").includes("blk"),
    `the kind is the descriptor's: ${byT("ai:blocked-on").className}`,
  );
  // Sparse-absent lane cell ⇒ 0, dimmed — the existing empty-state language.
  assert(
    collect(byT("ai:design"), "sc")[0].textContent === "0",
    "sparse-absent reads 0",
  );
  assert(
    byT("ai:design").className.split(" ").includes("zero"),
    `a zero descriptor state dims like any other: ${
      byT("ai:design").className
    }`,
  );
  // Everything claimed ⇒ no conservation band.
  assert(
    collect(box, "fsm-defect").length === 0,
    "a conserved snapshot draws no defect band",
  );
});

Deno.test("pipeline FSM: descriptor occupancy reads its declared source — lane prs and counts+items alike, itemsAreIssues picking the path", () => {
  const box = fsmBox(descHq());
  const detail = box.querySelectorAll("#fsmdetail")[0];
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);

  // Lane source: the cell's own prs, linked as PRs.
  byT("ai:blocked-on").click();
  assert(
    collect(detail, "li").length === 3,
    `the lane cell's 3 prs list: ${collect(detail, "li").length}`,
  );
  assert(
    textOf(detail).includes("blocked 1"),
    `the cell's OWN items: ${textOf(detail)}`,
  );
  assert(
    collect(detail, "li")[0].href === "https://github.com/o/blocked/pull/1",
    `lane items link as PRs: ${collect(detail, "li")[0].href}`,
  );
  byT("ai:blocked-on").click();

  // Counts source, itemsAreIssues: true — the /issues/ path is tried first.
  byT("uncoveredIssues").click();
  assert(
    collect(detail, "li").length === 4,
    `the named top-level array lists: ${collect(detail, "li").length}`,
  );
  assert(
    collect(detail, "li")[0].href === "https://github.com/o/uncovered/issues/1",
    `itemsAreIssues links as issues: ${collect(detail, "li")[0].href}`,
  );
  byT("uncoveredIssues").click();

  // Counts source, itemsAreIssues absent — items link as PRs.
  byT("leak").click();
  assert(
    collect(detail, "li")[0].href === "https://github.com/o/leak/pull/1",
    `absent itemsAreIssues links as PRs: ${collect(detail, "li")[0].href}`,
  );
  byT("leak").click();

  // A counts-source descriptor whose named array never arrived: the count still renders
  // and the click reads as detailBody's missing-list defect — never as an empty queue.
  const noList = descHq();
  delete noList.leaks;
  const box2 = fsmBox(noList);
  const detail2 = box2.querySelectorAll("#fsmdetail")[0];
  box2.querySelectorAll("[data-t]").find((b) => b.dataset.t === "leak").click();
  assert(
    textOf(detail2).includes("missing, not empty"),
    `a count with no list reads as the missing-list defect: ${textOf(detail2)}`,
  );
});

// The part a naive consumer gets wrong (the schema contract calls it out): retired history
// keys fold into the descriptor's series per ITS histFold — generic keys with no HIST_FOLD
// entry prove the fold is the descriptor's, not the hand Map's.
Deno.test("pipeline FSM: a descriptor's histFold folds retired-key samples into its series", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  const at = (d) => now - d * DAY;
  // Real inventory flat at 10 across a collapse: split 1 + 9 before, one key after.
  // Folded ⇒ flat line; unfolded ⇒ a 1 → 10 cliff and a false bottleneck.
  const history = [
    { t: at(4), counts: { current: 1, retiredKey: 9 } },
    { t: at(3), counts: { current: 1, retiredKey: 9 } },
    { t: at(2), counts: { current: 1, retiredKey: 9 } },
    { t: at(1), counts: { current: 10 } },
    { t: at(0), counts: { current: 10 } },
  ];
  const box = fsmBox({
    counts: {},
    lanes: { "vet-lifecycle": { st: { count: 10, prs: [] } } },
    stateDescriptors: [
      {
        key: "st",
        owner: "vetter",
        act: "vet",
        kind: "flow",
        hist: "current",
        histFold: ["retiredKey"],
        occupancy: { lane: "vet-lifecycle" },
      },
    ],
  }, history);
  const b = collect(box, "fsm-state").find((x) => x.dataset.t === "st");
  assert(b, "the descriptor box renders");
  const line = tags(b, "polyline")[0];
  assert(line, "the folded series draws a line");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(ys.length === 5, `all five samples plot: ${ys.length}`);
  assert(
    ys.every((y) => y === ys[0]),
    `the folded line is continuous, no cliff at the collapse: ${
      JSON.stringify(ys)
    }`,
  );
  assert(
    !b.classList.contains("rising"),
    "a folded flat inventory raises no bottleneck flag",
  );
});

Deno.test("pipeline FSM: a descriptor's empty histFold folds nothing — the hand-maintained fallback Map never applies", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  const at = (d) => now - d * DAY;
  // `reject` HAS a HIST_FOLD entry (humanReject folds in on the frozen fallback). A
  // descriptor naming hist "reject" with histFold [] must read ONLY reject: flat 28.
  // Falling back to the Map would draw 70, 70, 28 — a cliff the tool never declared.
  const history = [
    { t: at(2), counts: { reject: 28, humanReject: 42 } },
    { t: at(1), counts: { reject: 28, humanReject: 42 } },
    { t: at(0), counts: { reject: 28 } },
  ];
  const box = fsmBox({
    counts: {},
    lanes: { "vetter-verdicts": { "ai:reject": { count: 28, prs: [] } } },
    stateDescriptors: [
      {
        key: "ai:reject",
        owner: "producer",
        act: "rework per note",
        kind: "blk",
        hist: "reject",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  }, history);
  const b = collect(box, "fsm-state").find((x) => x.dataset.t === "ai:reject");
  const line = tags(b, "polyline")[0];
  assert(line, "the series draws");
  const ys = line.getAttribute("points").split(" ").map((p) =>
    Number(p.split(",")[1])
  );
  assert(
    ys.length === 3 && ys.every((y) => y === ys[0]),
    `an empty histFold reads the bare key — flat, no Map fold: ${
      JSON.stringify(ys)
    }`,
  );
});

// CONSERVATION, the invariant that would have made the blocked-on drift scream instead of
// hiding 17 PRs: every lanes cell and every counted top-level array is claimed by exactly
// one descriptor, or the panel says so loudly.
Deno.test("pipeline FSM: a lane cell no descriptor claims surfaces as a loud defect, never silence", () => {
  const hq = descHq();
  // The tool re-laned a state but its descriptor list missed the old cell: 17 real PRs
  // sit in occupancy nothing renders.
  hq.lanes["producer-blocked"] = {
    "ai:blocked-on": { count: 17, prs: fcItems("stranded", 17) },
  };
  const box = fsmBox(hq);
  const bands = collect(box, "fsm-defect");
  assert(bands.length === 1, `one defect band renders: ${bands.length}`);
  const text = textOf(bands[0]);
  assert(
    text.includes('lanes["producer-blocked"]["ai:blocked-on"]'),
    `it names the unclaimed cell: ${text}`,
  );
  assert(text.includes("17"), `it names the held inventory: ${text}`);
  assert(
    text.includes("renders nowhere"),
    `it says the failure in words: ${text}`,
  );
  assert(
    collect(bands[0], "fd-li").length === 1,
    `exactly the one unclaimed key is reported: ${
      collect(bands[0], "fd-li").length
    }`,
  );
  // The band ADDS to the machine — every claimed state still renders beside it.
  assert(
    collect(box, "fsm-state").length === 5,
    `the claimed machine still renders: ${collect(box, "fsm-state").length}`,
  );
});

Deno.test("pipeline FSM: a counts key no descriptor claims is a defect; a bare top-level array is not occupancy", () => {
  const hq = descHq();
  // Drop the leak descriptor: counts.leaks (and with it the leaks array) goes unclaimed.
  hq.stateDescriptors = hq.stateDescriptors.filter((d) => d.key !== "leak");
  // A top-level array with no counts entry is not occupancy the page knows how to count
  // — it must not be reported. Neither may the descriptor array itself ever be.
  hq.sideChannel = [{ repo: "o/x", number: 1 }];
  const box = fsmBox(hq);
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the defect band renders");
  const rows = collect(band, "fd-li").map((r) => r.textContent);
  assert(
    rows.length === 1,
    `exactly the leaks row is reported: ${JSON.stringify(rows)}`,
  );
  assert(
    rows[0].includes("leaks") && rows[0].includes("2 held") &&
      rows[0].includes("renders nowhere"),
    `it names the array and its held inventory: ${rows[0]}`,
  );
});

// Two DISTINCT descriptors reading one counts key: the same inventory wired into two
// series, so it renders twice. Distinct keys are what makes this the double-claim defect
// rather than the duplicate-key one — a lane claim id embeds the claiming state's own
// key, so two descriptors can only collide on a lane id by REPEATING a key, which the
// duplicate guard catches first and more specifically. The counts side has no such
// structural protection: the id is the counts key alone.
Deno.test("pipeline FSM: occupancy claimed by TWO descriptors is a defect too — exactly one claims each key", () => {
  const hq = descHq();
  hq.stateDescriptors.push({
    key: "ai:ready-mirror",
    owner: "producer",
    act: "merge again",
    kind: "flow",
    // The SAME series `ai:ready` already draws.
    hist: "ready",
    histFold: [],
    occupancy: { lane: "vetter-verdicts" },
  });
  const box = fsmBox(hq);
  const band = collect(box, "fsm-defect")[0];
  assert(band, "a double claim surfaces");
  const text = textOf(band);
  assert(
    text.includes('counts["ready"]') && text.includes("2 descriptors") &&
      text.includes("renders 2 times"),
    `it names the key and says the inventory double-renders: ${text}`,
  );
  assert(
    !text.includes("repeats the key"),
    `distinct keys read as a double CLAIM, not a duplicate key: ${text}`,
  );
});

Deno.test("pipeline FSM: a malformed descriptor renders no state but is reported — and takes no good descriptor with it", () => {
  const box = fsmBox({
    counts: { uncoveredIssues: 4 },
    lanes: {},
    uncoveredIssues: fcItems("uncovered", 4),
    stateDescriptors: [
      "junk",
      {
        key: "no-occupancy",
        owner: "human",
        act: "x",
        kind: "blk",
        occupancy: {},
      },
      {
        key: "bad-owner",
        owner: "misc",
        act: "x",
        kind: "blk",
        occupancy: { lane: "vet-lifecycle" },
      },
      {
        key: "uncoveredIssues",
        owner: "producer",
        act: "open a PR",
        kind: "flow",
        hist: "uncoveredIssues",
        histFold: [],
        occupancy: {
          counts: "uncoveredIssues",
          items: "uncoveredIssues",
          itemsAreIssues: true,
        },
        label: "untouched (no PR)",
      },
    ],
  });
  const keys = box.querySelectorAll("[data-t]").map((b) => b.dataset.t);
  assert(
    JSON.stringify(keys) === JSON.stringify(["uncoveredIssues"]),
    `only the well-formed descriptor renders a state: ${JSON.stringify(keys)}`,
  );
  assert(
    collect(box, "sc")[0].textContent === "4",
    `and it renders its real count: ${collect(box, "sc")[0].textContent}`,
  );
  const band = collect(box, "fsm-defect")[0];
  assert(band, "malformed descriptors surface in the defect band");
  const text = textOf(band);
  for (const i of [0, 1, 2]) {
    assert(
      text.includes("stateDescriptors[" + i + "]"),
      `descriptor ${i} is reported by index: ${text}`,
    );
  }
});

Deno.test("pipeline FSM: a descriptor's kind is clamped to the display kinds — snapshot content cannot smuggle a class onto a box", () => {
  const box = fsmBox({
    counts: { uncoveredIssues: 4 },
    lanes: {},
    uncoveredIssues: fcItems("uncovered", 4),
    stateDescriptors: [
      {
        key: "uncoveredIssues",
        owner: "producer",
        act: "open a PR",
        // Not a display kind: must not land in className, where it would dim a box
        // holding 4 real items (or fake a bottleneck / a selection).
        kind: "zero",
        hist: "",
        histFold: [],
        occupancy: {
          counts: "uncoveredIssues",
          items: "uncoveredIssues",
          itemsAreIssues: true,
        },
      },
    ],
  });
  const b = collect(box, "fsm-state")[0];
  assert(b, "the box renders, with the neutral default border");
  const classes = b.className.split(" ");
  for (const cls of ["zero", "rising", "sel", "lead"]) {
    assert(
      !classes.includes(cls),
      `unknown kind never lands as .${cls}: ${b.className}`,
    );
  }
});

// The snapshot is untrusted field by field: a lane cell that EXISTS but carries a junk or
// missing `count` renders 0 (dimmed), never NaN — the same coercion the sparse-absent cell
// gets, asserted separately because Number(null) is 0 while Number(undefined) is NaN, so
// only this shape catches a dropped `|| 0`.
Deno.test("pipeline FSM: a descriptor lane cell with a junk count renders 0, never NaN", () => {
  const box = fsmBox({
    counts: {},
    lanes: { "vetter-verdicts": { "ai:ready": { count: "junk", prs: [] } } },
    stateDescriptors: [
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  });
  const b = collect(box, "fsm-state")[0];
  assert(b, "the box renders");
  assert(
    collect(b, "sc")[0].textContent === "0",
    `a junk count coerces to 0: ${collect(b, "sc")[0].textContent}`,
  );
  assert(
    b.className.split(" ").includes("zero"),
    `and the box dims like any zero state: ${b.className}`,
  );
});

// The occupancy pair exists because the two names CAN disagree (the leak state's key and
// array already do): a descriptor claims both its counts key and its items array, so a
// conserved snapshot with a differently-named items array raises no spurious defect.
Deno.test("pipeline FSM: a descriptor claims BOTH its counts key and a differently-named items array", () => {
  const box = fsmBox({
    counts: { leakCount: 2 },
    lanes: {},
    leakItems: fcItems("leak", 2),
    stateDescriptors: [
      {
        key: "leak",
        owner: "human",
        act: "model it",
        kind: "blk",
        hist: "",
        histFold: [],
        occupancy: { counts: "leakCount", items: "leakItems" },
      },
    ],
  });
  assert(
    collect(box, "fsm-defect").length === 0,
    "the items array is claimed too — no spurious defect",
  );
  assert(
    collect(box, "sc")[0].textContent === "2",
    "the count reads from counts.<counts>",
  );
  const detail = box.querySelectorAll("#fsmdetail")[0];
  box.querySelectorAll("[data-t]")[0].click();
  assert(
    textOf(detail).includes("leak 1"),
    "the click lists from the named items array",
  );
});

// The retired-residue contract (issue-pr-cron#130, 2026-08-06 clarification): a retired
// state's descriptor is EMITTED ONLY WHILE its occupancy is nonzero, so the consumer
// renders every emitted descriptor and carries NO retired-detection of its own — the
// residue is a state like any other while it lasts, and it leaves the shape when the
// tool stops emitting it (together with its cell and counts key).
Deno.test("pipeline FSM: a residue descriptor renders while emitted; drained, its absence renders nothing and nothing screams", () => {
  const withResidue = {
    counts: { blockedInfra: 2, ready: 1 },
    lanes: {
      "producer-blocked": {
        "ai:blocked-infra": { count: 2, prs: fcItems("residue", 2) },
      },
      "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } },
    },
    stateDescriptors: [
      {
        key: "ai:blocked-infra",
        owner: "producer",
        act: "strip the label",
        kind: "blk",
        hist: "blockedInfra",
        histFold: [],
        occupancy: { lane: "producer-blocked" },
        label: "ai:blocked-infra (retired #108)",
      },
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  };
  const box = fsmBox(withResidue);
  const b = collect(box, "fsm-state").find((x) =>
    x.dataset.t === "ai:blocked-infra"
  );
  assert(b, "the residue renders while the tool emits its descriptor");
  assert(
    collect(b, "sc")[0].textContent === "2",
    `it carries its cell's count: ${collect(b, "sc")[0].textContent}`,
  );
  assert(
    collect(b, "sk")[0].textContent === "ai:blocked-infra (retired #108)",
    `the retired suffix rides the emitted label: ${
      collect(b, "sk")[0].textContent
    }`,
  );
  assert(
    collect(box, "fsm-defect").length === 0,
    "a claimed residue is conserved",
  );

  // Drained: the tool drops the descriptor, the cell and the counts key TOGETHER.
  const drained = {
    counts: { ready: 1 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } } },
    stateDescriptors: [
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  };
  const box2 = fsmBox(drained);
  assert(
    !collect(box2, "fsm-state").some((x) => x.dataset.t === "ai:blocked-infra"),
    "no box for the drained retired state — no permanent dimmed residue",
  );
  assert(
    collect(box2, "fsm-state").some((x) => x.dataset.t === "ai:ready"),
    "the live machine still renders",
  );
  assert(
    collect(box2, "fsm-defect").length === 0,
    "and nothing screams: nothing unclaimed remains",
  );
});

// The counts-side scoping the ratified contract froze: exactly five named keys may go
// unclaimed — the non-state metrics (totalProducerPrs, openIssues, archivedRepoPrs), the
// legacy duplicate of claimed inventory (closeCandidateIssues), and `designNoQuestion`
// (issue-pr-cron#240), the size of the ai:design half /ndd withholds, whose PRs the
// ai:design cell already claims and which no box here draws. Anything else is
// state-shaped occupancy rendering nowhere.
Deno.test("pipeline FSM: the frozen-legacy counts keys never defect; any other unclaimed counts key does", () => {
  const conserved = () => ({
    counts: {
      ready: 2,
      totalProducerPrs: 145,
      openIssues: 767,
      archivedRepoPrs: 4,
      closeCandidateIssues: 0,
      designNoQuestion: 6,
    },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 2, prs: [] } } },
    closeCandidateIssues: [],
    archivedRepoPrs: fcItems("archived", 4),
    stateDescriptors: [
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  });
  assert(
    collect(fsmBox(conserved()), "fsm-defect").length === 0,
    "the five frozen-legacy keys are the whole exception — no defect band",
  );
  const drifted = conserved();
  drifted.counts.blockedOn = 17;
  const band = collect(fsmBox(drifted), "fsm-defect")[0];
  assert(band, "an unclaimed state-shaped counts key surfaces");
  const text = textOf(band);
  assert(
    text.includes("blockedOn") && text.includes("17 held") &&
      text.includes("renders nowhere"),
    `it names the key and its held inventory: ${text}`,
  );
});

// The same class one level down: a lane cell carries its own two numbers — the `count` it
// STATES and the `prs` it HOLDS. A claimed cell leaves the sweep early (n === 1), so a
// broken or absent count over a populated list would render 0 while the click expanded
// onto real PRs, in silence.
Deno.test("pipeline FSM: a lane cell whose count disagrees with its prs is reported, and the box renders the larger", () => {
  for (
    const [what, count] of [["junk count", "many"], ["absent count", undefined]]
  ) {
    const cell = { prs: fcItems("stranded", 17) };
    if (count !== undefined) cell.count = count;
    const box = fsmBox({
      counts: {},
      lanes: { "vet-lifecycle": { "ai:blocked-on": cell } },
      stateDescriptors: [
        {
          key: "ai:blocked-on",
          owner: "vetter",
          act: "clears when deps merge/close",
          kind: "blk",
          hist: "",
          histFold: [],
          occupancy: { lane: "vet-lifecycle" },
        },
      ],
    });
    const b = collect(box, "fsm-state").find((x) =>
      x.dataset.t === "ai:blocked-on"
    );
    assert(b, `the box renders (${what})`);
    // The box shows the larger, so the list cannot hide inventory behind a broken count.
    assert(
      collect(b, "sc")[0].textContent === "17",
      `the box renders the PRs it holds, not 0 (${what}): ${
        collect(b, "sc")[0].textContent
      }`,
    );
    // …and the count-vs-rows invariant holds: the number on the box is the number of rows.
    const detail = box.querySelectorAll("#fsmdetail")[0];
    b.click();
    assert(
      collect(detail, "li").length === 17,
      `the box's number equals the rows it expands to (${what}): ${
        collect(detail, "li").length
      }`,
    );
    const band = collect(box, "fsm-defect")[0];
    assert(band, `the disagreement is reported (${what})`);
    const text = textOf(band);
    assert(
      text.includes("states count 0") && text.includes("carries 17 PRs") &&
        text.includes("renders 17"),
      `it names both numbers and which was rendered (${what}): ${text}`,
    );
  }
});

Deno.test("pipeline FSM: a lane cell whose count agrees with its prs reports nothing", () => {
  const box = fsmBox({
    counts: {},
    lanes: {
      "vet-lifecycle": {
        "ai:blocked-on": { count: 3, prs: fcItems("blocked", 3) },
      },
    },
    stateDescriptors: [
      {
        key: "ai:blocked-on",
        owner: "vetter",
        act: "clears when deps merge/close",
        kind: "blk",
        hist: "",
        histFold: [],
        occupancy: { lane: "vet-lifecycle" },
      },
    ],
  });
  assert(
    collect(box, "fsm-defect").length === 0,
    "an agreeing cell draws no band",
  );
  const b = collect(box, "fsm-state")[0];
  assert(
    collect(b, "sc")[0].textContent === "3",
    "and renders its stated count",
  );
});

// The page holds TWO numbers for a lane state — the lane cell it RENDERS and the `counts`
// key its SERIES is drawn from — and the tool computes them two ways
// (issue-pr-cron#228). Every claim check passes when they disagree (each key is claimed
// exactly once), so only a comparison catches it: the box would draw one number under a
// sparkline sitting at the other, silently. The live witness the moment the producer side
// lands: `counts.ready` is 23 off the label bucket while the `vetter-verdicts` cell is
// ABSENT (all 23 are un-vetted at head), so the box renders 0, dimmed, under a line at 23.
Deno.test("pipeline FSM: a lane state whose counts mirror contradicts its rendered cell is a loud defect naming both numbers", () => {
  const box = fsmBox({
    // The label bucket says 23; classify_lane routed all of them to un-vetted, so the
    // vetter-verdicts cell for ai:ready is absent entirely.
    counts: { ready: 23, unvetted: 24 },
    lanes: { "vet-lifecycle": { "un-vetted": { count: 24, prs: [] } } },
    stateDescriptors: [
      {
        key: "un-vetted",
        owner: "vetter",
        act: "vet at current head",
        kind: "flow",
        hist: "unvetted",
        histFold: [],
        occupancy: { lane: "vet-lifecycle" },
      },
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  });
  // The box renders its DECLARED occupancy — the cell, not the mirror.
  const ready = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:ready"
  );
  assert(
    collect(ready, "sc")[0].textContent === "0",
    `the box renders the lane cell, absent ⇒ 0: ${
      collect(ready, "sc")[0].textContent
    }`,
  );
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the disagreement surfaces");
  const text = textOf(band);
  assert(
    text.includes('counts["ready"]') && text.includes("23"),
    `it names the mirror and its number: ${text}`,
  );
  assert(
    text.includes('lanes["vetter-verdicts"]["ai:ready"]') &&
      text.includes("holds 0"),
    `it names the rendered occupancy and its number: ${text}`,
  );
  assert(
    text.includes("renders 0"),
    `it says which number is on screen: ${text}`,
  );
  assert(
    text.includes("disagrees with itself"),
    `it reports a snapshot contradiction, not a guess at the right number: ${text}`,
  );
  // The agreeing state says nothing.
  assert(
    !text.includes("un-vetted"),
    `a state whose two numbers agree stays silent: ${text}`,
  );
  // And the claim is KEPT: no "unclaimed counts key" defect rides along.
  assert(
    !text.includes("renders nowhere"),
    `the mirror is still claimed: ${text}`,
  );
});

Deno.test("pipeline FSM: lane states whose counts mirror agrees draw no divergence defect", () => {
  // descHq's mirrors agree with their cells (ready 5, blockedOn 3), and ai:design has a
  // sparse-absent cell with no mirror at all — neither shape may report.
  assert(
    collect(fsmBox(descHq()), "fsm-defect").length === 0,
    "a self-consistent snapshot stays silent",
  );
});

// The frozen-legacy exception licenses a key going UNCLAIMED; it never licenses the same
// inventory being wired twice. Two descriptors both drawing `openIssues` would render the
// open-issue population as two state series.
Deno.test("pipeline FSM: a frozen-legacy counts key is exempt from unclaimed, never from a DOUBLE claim", () => {
  const box = fsmBox({
    counts: { openIssues: 767, ready: 0 },
    lanes: {},
    stateDescriptors: [
      {
        key: "a",
        owner: "producer",
        act: "x",
        kind: "flow",
        hist: "openIssues",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
      {
        key: "b",
        owner: "vetter",
        act: "y",
        kind: "flow",
        hist: "openIssues",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  });
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the double claim on a frozen-legacy key surfaces");
  const text = textOf(band);
  assert(
    text.includes('counts["openIssues"]') && text.includes("2 descriptors"),
    `it names the key and the double claim: ${text}`,
  );
});

// Two descriptors naming the same top-level array: distinct keys, possibly distinct
// counts keys, and yet both boxes open onto ONE list — #141 by another route, invisible
// to the lane and counts checks.
Deno.test("pipeline FSM: two descriptors naming the same items array is a defect", () => {
  const box = fsmBox({
    counts: { leaks: 2, alsoLeaks: 2 },
    lanes: {},
    leaks: fcItems("leak", 2),
    stateDescriptors: [
      {
        key: "leak",
        owner: "human",
        act: "model it",
        kind: "blk",
        hist: "leaks",
        histFold: [],
        occupancy: { counts: "leaks", items: "leaks" },
      },
      {
        key: "leak-copy",
        owner: "producer",
        act: "model it",
        kind: "blk",
        hist: "alsoLeaks",
        histFold: [],
        occupancy: { counts: "alsoLeaks", items: "leaks" },
      },
    ],
  });
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the shared list surfaces");
  const text = textOf(band);
  assert(
    text.includes('"leaks" is named by 2 descriptors') &&
      text.includes("open onto the same list"),
    `it names the array and the collision: ${text}`,
  );
  // One array named once is not a defect.
  assert(
    collect(fsmBox(descHq()), "fsm-defect").length === 0,
    "distinct arrays stay silent",
  );
});

// The zero-held carve-out must not swallow a cell that HOLDS PRs behind a dropped or
// non-numeric count — the one place the sweep has the list in hand to check.
Deno.test("pipeline FSM: an unclaimed cell holding PRs with no usable count still screams", () => {
  for (
    const [what, cell] of [
      ["absent count", { prs: fcItems("stranded", 17) }],
      ["junk count", { count: "many", prs: fcItems("stranded", 17) }],
    ]
  ) {
    const box = fsmBox({
      counts: { ready: 1 },
      lanes: {
        "vetter-verdicts": { "ai:ready": { count: 1, prs: [] } },
        "producer-blocked": { "ai:blocked-on": cell },
      },
      stateDescriptors: [
        {
          key: "ai:ready",
          owner: "human",
          act: "merge",
          kind: "flow",
          hist: "ready",
          histFold: [],
          occupancy: { lane: "vetter-verdicts" },
        },
      ],
    });
    const band = collect(box, "fsm-defect")[0];
    assert(band, `the stranded cell surfaces (${what})`);
    assert(
      textOf(band).includes("17 held"),
      `the list it carries is what it holds (${what}): ${textOf(band)}`,
    );
  }
});

// Prototype-shaped strings off the untrusted snapshot must be ordinary data everywhere
// they are used as a lookup or a set member. The code is written to be immune (Map/Set,
// hasOwnProperty.call, Array.isArray); this pins it so a refactor to plain objects —
// where `__proto__` and `constructor` are not ordinary keys — fails loudly instead of
// silently mis-claiming, mis-rendering or throwing.
Deno.test("pipeline FSM: prototype-shaped keys, counts, items and lane names are ordinary data", () => {
  // Built through JSON.parse — the only way to get a genuine OWN `__proto__` key (an
  // object literal's `__proto__:` sets the prototype instead), and exactly how the page
  // receives the snapshot: `fetch(...).then((r) => r.json())`.
  const box = fsmBox(JSON.parse(
    JSON.stringify({
      counts: { PROTO: 3, constructor: 2, toString: 0 },
      lanes: { constructor: { PROTO: { count: 4, prs: fcItems("proto", 4) } } },
      PROTO: fcItems("protoitems", 3),
      stateDescriptors: [
        // A lane-sourced state whose key AND lane are prototype-shaped.
        {
          key: "PROTO",
          owner: "vetter",
          act: "vet",
          kind: "flow",
          hist: "",
          histFold: [],
          occupancy: { lane: "constructor" },
        },
        // A counts-sourced state whose counts key and items array are prototype-shaped.
        {
          key: "protoCounts",
          owner: "producer",
          act: "open a PR",
          kind: "flow",
          hist: "constructor",
          histFold: [],
          occupancy: {
            counts: "constructor",
            items: "PROTO",
            itemsAreIssues: true,
          },
        },
      ],
    }).replace(/PROTO/g, "__proto__"),
  ));
  const keys = box.querySelectorAll("[data-t]").map((b) => b.dataset.t);
  assert(
    JSON.stringify(keys) === JSON.stringify(["protoCounts", "__proto__"]),
    `both states render, keyed by their own strings: ${JSON.stringify(keys)}`,
  );
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  assert(
    collect(byT("__proto__"), "sc")[0].textContent === "4",
    `a prototype-shaped lane+key resolves its own cell: ${
      collect(byT("__proto__"), "sc")[0].textContent
    }`,
  );
  assert(
    collect(byT("protoCounts"), "sc")[0].textContent === "2",
    `a prototype-shaped counts key resolves its own number: ${
      collect(byT("protoCounts"), "sc")[0].textContent
    }`,
  );
  // Each opens onto its OWN list — registration is keyed by these strings too.
  const detail = box.querySelectorAll("#fsmdetail")[0];
  byT("__proto__").click();
  assert(
    textOf(detail).includes("proto 1"),
    `the lane cell's own list: ${textOf(detail)}`,
  );
  byT("__proto__").click();
  byT("protoCounts").click();
  assert(
    textOf(detail).includes("protoitems 1"),
    `the named array's own list: ${textOf(detail)}`,
  );
  // The sweep treats them as data: `counts.__proto__` (3 held) is claimed by nobody and
  // reports; the claimed `constructor` does not; `toString` holds 0 and is skipped.
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the unclaimed prototype-shaped counts key reports");
  const rows = collect(band, "fd-li").map((r) => r.textContent);
  assert(
    rows.some((r) => r.includes('counts["__proto__"]') && r.includes("3 held")),
    `it names the unclaimed prototype-shaped key: ${JSON.stringify(rows)}`,
  );
  assert(
    !rows.some((r) => r.includes('counts["constructor"]')),
    `a claimed prototype-shaped key is not reported: ${JSON.stringify(rows)}`,
  );
  assert(
    !rows.some((r) => r.includes('counts["toString"]')),
    `a zero-held prototype-shaped key is skipped like any other: ${
      JSON.stringify(rows)
    }`,
  );
});

// A state key is the panel's IDENTITY for a state — the box's `data-t` and the
// register/itemsFor map key the click-to-expand reads — so two descriptors sharing one
// key let the second's registration OVERWRITE the first's, and the first box opens onto
// the second's list. #141's exact failure, and neither claim check sees it: with
// different occupancy sources the two claims are different ids, so both read as singly
// claimed. The repeat therefore renders no state and is reported, first occurrence wins.
Deno.test("pipeline FSM: a repeated descriptor key renders one box and is reported, with the loser's occupancy named unclaimed", () => {
  const box = fsmBox({
    counts: { ready: 5, readyDupe: 9 },
    lanes: {
      "vetter-verdicts": { "ai:ready": { count: 5, prs: fcItems("first", 5) } },
    },
    readyDupe: fcItems("second", 9),
    stateDescriptors: [
      // FIRST wins: array order is render order.
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
      // Same key, a DIFFERENT occupancy source — so the two claims are different ids and
      // the sweep alone would call both singly claimed.
      {
        key: "ai:ready",
        owner: "producer",
        act: "open a PR",
        kind: "flow",
        hist: "readyDupe",
        histFold: [],
        occupancy: {
          counts: "readyDupe",
          items: "readyDupe",
          itemsAreIssues: true,
        },
      },
    ],
  });
  const boxes = box.querySelectorAll("[data-t]").filter((b) =>
    b.dataset.t === "ai:ready"
  );
  assert(
    boxes.length === 1,
    `exactly one box carries the key: ${boxes.length}`,
  );
  assert(
    collect(boxes[0], "sc")[0].textContent === "5",
    `the FIRST descriptor's occupancy renders: ${
      collect(boxes[0], "sc")[0].textContent
    }`,
  );
  // Its click-through is its OWN list, not the repeat's — the overwrite this guard exists
  // to prevent.
  const detail = box.querySelectorAll("#fsmdetail")[0];
  boxes[0].click();
  assert(
    collect(detail, "li").length === 5,
    `it opens onto its own 5 rows: ${collect(detail, "li").length}`,
  );
  assert(
    textOf(detail).includes("first 1"),
    `and onto its own items: ${textOf(detail)}`,
  );
  assert(!textOf(detail).includes("second 1"), "never the repeat's list");

  const band = collect(box, "fsm-defect")[0];
  assert(band, "the repeat is reported");
  const rows = collect(band, "fd-li").map((r) => r.textContent);
  assert(
    rows.some((r) =>
      r.includes("stateDescriptors[1]") &&
      r.includes('repeats the key "ai:ready"')
    ),
    `named by index and key: ${JSON.stringify(rows)}`,
  );
  // Its claim was never registered, so the occupancy it meant to claim reads unclaimed.
  assert(
    rows.some((r) =>
      r.includes("readyDupe") && r.includes("9 held") &&
      r.includes("renders nowhere")
    ),
    `the loser's occupancy is named unclaimed: ${JSON.stringify(rows)}`,
  );
});

Deno.test("pipeline FSM: all-distinct descriptor keys draw no duplicate-key defect", () => {
  assert(
    collect(fsmBox(descHq()), "fsm-defect").length === 0,
    "a conserved, distinctly-keyed snapshot still renders no band",
  );
});

// A descriptor also READS its `histFold` keys — that is where its series draws the
// absorbed retirees' samples from — so those counts keys are claimed by it. The live
// shape behind this: `ai:reject` folds `humanReject`/`relink` while the residue
// rows carry NO `hist` of their own (the successor draws the past, so the same
// history is not rendered twice). Without the fold claim, a nonzero absorbed-retiree
// count paints a FALSE defect on a healthy snapshot.
Deno.test("pipeline FSM: a folded retired counts key is claimed by the folding descriptor, nonzero and all", () => {
  const hq = {
    counts: { reject: 66, humanReject: 4, relink: 0 },
    lanes: {
      "vetter-verdicts": { "ai:reject": { count: 66, prs: [] } },
      "human-decisions": {
        "human:reject": { count: 4, prs: fcItems("hreject", 4) },
      },
    },
    stateDescriptors: [
      // The live row folds both retired keys and owns the continuous series.
      {
        key: "ai:reject",
        owner: "producer",
        act: "rework per note",
        kind: "blk",
        hist: "reject",
        histFold: ["humanReject", "relink"],
        occupancy: { lane: "vetter-verdicts" },
      },
      // The residue row: its LANE cell is its occupancy; it carries no hist, because the
      // successor above already draws its past.
      {
        key: "human:reject",
        owner: "human",
        act: "strip the label",
        kind: "blk",
        hist: "",
        histFold: [],
        occupancy: { lane: "human-decisions" },
        label: "human:reject (retired #133)",
      },
    ],
  };
  const box = fsmBox(hq);
  assert(
    collect(box, "fsm-defect").length === 0,
    `a folded nonzero retired key is claimed, not a defect: ${
      textOf(collect(box, "fsm-defect")[0] || makeEl("div"))
    }`,
  );
  // Both rows render, each off its own lane cell.
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  assert(
    collect(byT("ai:reject"), "sc")[0].textContent === "66",
    "the live row reads its cell",
  );
  assert(
    collect(byT("human:reject"), "sc")[0].textContent === "4",
    "the residue row reads its cell",
  );

  // And the fold is not a blanket amnesty: a retired counts key NO descriptor folds or
  // reads still screams when it holds inventory.
  const stranded = JSON.parse(JSON.stringify(hq));
  stranded.counts.awaitingReVet = 7;
  const band = collect(fsmBox(stranded), "fsm-defect")[0];
  assert(band, "an unfolded, unclaimed retired key still surfaces");
  assert(
    textOf(band).includes("awaitingReVet") && textOf(band).includes("7 held"),
    `it names the stranded key: ${textOf(band)}`,
  );
});

// The fold claim must not weaken the double-claim direction: a key claimed BOTH as one
// descriptor's fold and as another's live hist is the same inventory wired twice — the
// producer side is held to fold keys being retired (never a live row's hist), so this
// reads as the structural defect it is.
Deno.test("pipeline FSM: a counts key claimed as both a fold and another descriptor's hist is still a double claim", () => {
  const box = fsmBox({
    counts: { reject: 66, humanReject: 4 },
    lanes: {
      "vetter-verdicts": { "ai:reject": { count: 66, prs: [] } },
      "human-decisions": { "human:reject": { count: 4, prs: [] } },
    },
    stateDescriptors: [
      {
        key: "ai:reject",
        owner: "producer",
        act: "rework per note",
        kind: "blk",
        hist: "reject",
        histFold: ["humanReject"],
        occupancy: { lane: "vetter-verdicts" },
      },
      // Illegal per the contract: the residue row draws the SAME history the fold above
      // already absorbed, so humanReject's inventory would render twice.
      {
        key: "human:reject",
        owner: "human",
        act: "strip the label",
        kind: "blk",
        hist: "humanReject",
        histFold: [],
        occupancy: { lane: "human-decisions" },
      },
    ],
  });
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the double claim surfaces");
  const text = textOf(band);
  assert(
    text.includes("humanReject") && text.includes("2 descriptors") &&
      text.includes("renders 2 times"),
    `it names the key and says it renders twice: ${text}`,
  );
});

// Zero inventory renders nowhere VACUOUSLY: the live tool still emits drained retired
// counts keys at 0 (kept-while-nonzero governs the descriptor list, not the keys), so an
// unclaimed key holding nothing is not hidden inventory and must not paint a permanent
// false defect on a healthy snapshot. The same keys NONZERO are the real hazard and
// scream; a DOUBLE claim is structural — the same inventory wired twice — and defects at
// any count, zero included.
Deno.test("pipeline FSM: zero-held unclaimed occupancy is vacuously conserved; nonzero screams; a zero-held double claim still defects", () => {
  const base = () => ({
    counts: { ready: 2, blockedInfra: 0 },
    lanes: {
      "vetter-verdicts": { "ai:ready": { count: 2, prs: [] } },
      "producer-blocked": { "ai:blocked-deploy": { count: 0, prs: [] } },
    },
    stateDescriptors: [
      {
        key: "ai:ready",
        owner: "human",
        act: "merge",
        kind: "flow",
        hist: "ready",
        histFold: [],
        occupancy: { lane: "vetter-verdicts" },
      },
    ],
  });
  assert(
    collect(fsmBox(base()), "fsm-defect").length === 0,
    "a drained retired counts key and an empty unclaimed cell hide nothing — no band",
  );

  const held = base();
  held.counts.blockedInfra = 2;
  held.lanes["producer-blocked"]["ai:blocked-deploy"].count = 3;
  const band = collect(fsmBox(held), "fsm-defect")[0];
  assert(band, "the same keys nonzero surface");
  const text = textOf(band);
  assert(
    text.includes("blockedInfra") && text.includes("2 held"),
    `the nonzero drained key screams: ${text}`,
  );
  assert(
    text.includes("ai:blocked-deploy") && text.includes("3 held"),
    `the nonzero unclaimed cell screams: ${text}`,
  );

  // Two DISTINCT descriptors reading the SAME zero-held counts key — a double claim, not
  // a duplicate key (which is its own, earlier defect).
  const doubled = base();
  doubled.stateDescriptors.push(
    {
      key: "ai:blocked-infra",
      owner: "human",
      act: "strip the label",
      kind: "blk",
      hist: "blockedInfra",
      histFold: [],
      occupancy: { lane: "producer-blocked" },
    },
    {
      key: "ai:blocked-infra-mirror",
      owner: "producer",
      act: "strip the label",
      kind: "blk",
      hist: "blockedInfra",
      histFold: [],
      occupancy: { lane: "producer-blocked" },
    },
  );
  const band2 = collect(fsmBox(doubled), "fsm-defect")[0];
  assert(band2, "a double claim on a zero-held cell still surfaces");
  assert(
    textOf(band2).includes("2 descriptors"),
    `it reads as the structural defect it is: ${textOf(band2)}`,
  );
});

// A counts-source descriptor's declared keys must RESOLVE in the snapshot: a tool that
// emits a descriptor pointing at nothing has broken its own contract, and the count/list
// it strands must not degrade into a quiet zero.
Deno.test("pipeline FSM: a descriptor whose counts or items source does not resolve is a loud defect", () => {
  const box = fsmBox({
    counts: {},
    lanes: {},
    stateDescriptors: [
      {
        key: "uncoveredIssues",
        owner: "producer",
        act: "open a PR",
        kind: "flow",
        hist: "",
        histFold: [],
        occupancy: {
          counts: "uncoveredIssues",
          items: "uncoveredIssues",
          itemsAreIssues: true,
        },
      },
    ],
  });
  const band = collect(box, "fsm-defect")[0];
  assert(band, "unresolved sources surface");
  const rows = collect(band, "fd-li").map((r) => r.textContent);
  assert(
    rows.length === 2,
    `both unresolved sources report: ${JSON.stringify(rows)}`,
  );
  assert(
    rows.some((r) =>
      r.includes('counts source "uncoveredIssues"') &&
      r.includes("does not resolve")
    ),
    `the counts source is named: ${JSON.stringify(rows)}`,
  );
  assert(
    rows.some((r) =>
      r.includes('items source "uncoveredIssues"') &&
      r.includes("does not resolve")
    ),
    `the items source is named: ${JSON.stringify(rows)}`,
  );
});

// The frozen fallback: a descriptor-less snapshot renders the hand table exactly as
// before this change — same boxes, same order — with no conservation band. Junk (a
// non-array) is not a descriptor list and falls back the same way.
Deno.test("pipeline FSM: a descriptor-less snapshot renders the frozen hand table, defect-free — junk stateDescriptors falls back too", () => {
  const lanesHq = () => ({
    counts: {
      leaks: 1,
      ready: 2,
      uncoveredIssues: 3,
      closeCandidateUnvetted: 0,
      closeCandidateUpheld: 0,
    },
    lanes: {
      "vetter-verdicts": {
        "ai:ready": { count: 2, prs: [] },
        "ai:reject": { count: 1, prs: [] },
      },
    },
    leaks: fcItems("leak", 1),
    uncoveredIssues: fcItems("uncovered", 3),
  });
  const frozen = (box, what) => {
    const keys = box.querySelectorAll("[data-t]").map((b) => b.dataset.t);
    assert(
      JSON.stringify(keys) === JSON.stringify([
        "ai:reject",
        "uncoveredIssues",
        "un-vetted",
        "closeCandidateUnvetted",
        "ai:blocked-on",
        "leak",
        "ai:ready",
        "ai:design",
        "human:design",
        "closeCandidateUpheld",
      ]),
      `the frozen table renders whole and in order (${what}): ${
        JSON.stringify(keys)
      }`,
    );
    assert(
      collect(box, "fsm-defect").length === 0,
      `no conservation band on the fallback (${what})`,
    );
  };
  frozen(fsmBox(lanesHq()), "absent");
  frozen(
    fsmBox({ ...lanesHq(), stateDescriptors: "junk" }),
    "junk descriptors",
  );
});

// An EMPTY stateDescriptors array is a present — broken — declaration: the machine claims
// zero states while the snapshot holds occupancy. The hand table must not resurrect
// (vocabulary lives in the tool), and the conservation band must scream instead.
Deno.test("pipeline FSM: an empty stateDescriptors array screams via conservation rather than resurrecting the hand table", () => {
  const box = fsmBox({
    counts: { ready: 2 },
    lanes: { "vetter-verdicts": { "ai:ready": { count: 2, prs: [] } } },
    stateDescriptors: [],
  });
  assert(
    collect(box, "fsm-state").length === 0,
    `no hand-table box resurrects: ${collect(box, "fsm-state").length}`,
  );
  const band = collect(box, "fsm-defect")[0];
  assert(band, "the unclaimed occupancy surfaces");
  assert(
    textOf(band).includes("ai:ready"),
    `it names the stranded cell: ${textOf(band)}`,
  );
});

// The panel's existing behaviors survive descriptor mode: sparklines draw from the
// descriptor's hist key, and the per-actor lead fallback marks the largest flat queue.
Deno.test("pipeline FSM: descriptor boxes keep sparklines and the lead fallback", () => {
  const now = Date.parse("2026-08-06T00:00:00Z");
  const history = [
    { t: now - 2 * DAY, counts: { ready: 5 } },
    { t: now, counts: { ready: 5 } },
  ];
  const box = fsmBox(descHq(), history);
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  const ready = byT("ai:ready");
  assert(
    collect(ready, "fsm-spark").length === 1,
    "the descriptor's hist key draws a sparkline",
  );
  // Human group: ready 5, design 0, leak 2 — nothing rising, so the largest queue leads.
  assert(
    ready.classList.contains("lead"),
    "the lead fallback marks the actor's largest queue",
  );
  assert(
    !byT("leak").classList.contains("lead"),
    "and only that one",
  );
});

// descHq() with the design cell carrying the /ndd split (issue-pr-cron#240): six
// labelled PRs, zero presentable, six withheld with no trusted question — the measured
// 2026-08-09 shape that motivated the change — plus the counts keys the tool emits
// beside it. The whole breakdown is present because the tool emits the whole breakdown;
// the page reads the one bucket it draws. Callers mutate the cell/counts for their own
// variants.
function splitHq() {
  const hq = descHq();
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 6,
    prs: fcItems("design", 6).map((e) => ({ ...e, bucket: "noQuestion" })),
    breakdown: {
      presentable: 0,
      noQuestion: 6,
      draft: 0,
      unaddressable: 0,
      fetchErrors: 0,
    },
  };
  hq.counts.design = 6;
  hq.counts.designPresentable = 0;
  hq.counts.designNoQuestion = 6;
  return hq;
}

Deno.test("pipeline FSM: the design box is the /ndd inbox — presentable, never the raw label total", () => {
  const box = fsmBox(splitHq());
  const byT = (k) =>
    box.querySelectorAll("[data-t]").find((b) => b.dataset.t === k);
  const design = byT("ai:design");
  // Six PRs wear the label; /ndd will present zero. The box says zero — the number that
  // goes to zero when a human drains the queue is the number a human inbox shows — and
  // dims like any other empty inbox.
  assert(
    collect(design, "sc")[0].textContent === "0",
    "the inbox is presentable, not 6",
  );
  assert(design.className.split(" ").includes("zero"), "an empty inbox dims");
  // The human owner total counts the inbox: ready 5 + design 0 + leak 2.
  const groups = ownerGroups(box);
  const human = groups.find((g) => g.title.startsWith("Human action"));
  assert(
    human.title.endsWith("7"),
    `the human total is the inbox: ${human.title}`,
  );
  // The withheld rows are drawn NOWHERE: no extra group beside the three actors, no box
  // of their own, and no listing under the design box, which opens onto the presentable
  // rows and holds none.
  assert(
    groups.length === 3,
    `the three actor groups are the whole machine: ${groups.length}`,
  );
  assert(
    !byT("designNoQuestion") && !byT("designDraft") &&
      !byT("designFetchErrors") &&
      !byT("designUnaddressable"),
    "no box is built out of the rows /ndd withholds",
  );
  design.click();
  const text = textOf(box);
  assert(
    !text.includes("o/design#"),
    `no withheld row is listed: ${text.slice(0, 400)}`,
  );
  // A conserved split snapshot draws no defect band: the key the box draws is claimed by
  // it, and the withheld half's key is the ratified unclaimed one below.
  assert(
    collect(box, "fsm-defect").length === 0,
    "no conservation defect on a clean split",
  );
});

Deno.test("pipeline FSM: a snapshot without the split renders the design cell exactly as before", () => {
  const hq = descHq();
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 2,
    prs: fcItems("design", 2),
  };
  hq.counts.design = 2;
  const box = fsmBox(hq);
  const design = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:design"
  );
  assert(
    collect(design, "sc")[0].textContent === "2",
    "without a breakdown the raw cell count renders, as it always did",
  );
  design.click();
  assert(
    textOf(box).includes("o/design#1"),
    "and opens onto the whole cell's PRs",
  );
  assert(collect(box, "fsm-defect").length === 0, "and nothing screams");
});

Deno.test("pipeline FSM: presentable rows list under the design box", () => {
  const hq = splitHq();
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 2,
    prs: fcItems("design", 2).map((e) => ({ ...e, bucket: "presentable" })),
    breakdown: {
      presentable: 2,
      noQuestion: 0,
      draft: 0,
      unaddressable: 0,
      fetchErrors: 0,
    },
  };
  hq.counts.design = 2;
  hq.counts.designPresentable = 2;
  hq.counts.designNoQuestion = 0;
  const box = fsmBox(hq);
  const design = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:design"
  );
  assert(
    collect(design, "sc")[0].textContent === "2",
    "two presentable rows are the inbox",
  );
  design.click();
  assert(
    textOf(box).includes("o/design#1"),
    "the inbox lists the presentable rows",
  );
  assert(
    collect(box, "fsm-defect").length === 0,
    "a clean split draws no defect band",
  );
});

Deno.test("pipeline FSM: a malformed presentable falls back to the raw cell, never a fabricated zero inbox", () => {
  const hq = splitHq();
  // A number-shaped STRING is the emit fault that matters: `Number()` coercion would turn
  // it into a real-looking 0 inbox over six labelled PRs, which is the reading the raw
  // count at least does not fake. `presentable` is the whole gate now, so an unreadable
  // one degrades the cell to exactly the pre-split rendering.
  hq.lanes["vetter-verdicts"]["ai:design"].breakdown.presentable = "0";
  const box = fsmBox(hq);
  const design = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:design"
  );
  assert(
    collect(design, "sc")[0].textContent === "6",
    `an unreadable split renders the whole cell: ${
      collect(design, "sc")[0].textContent
    }`,
  );
  design.click();
  assert(
    textOf(box).includes("o/design#6"),
    "and opens onto the whole cell's PRs",
  );
});

Deno.test("pipeline FSM: a breakdown carrying only presentable still renders the inbox", () => {
  const hq = splitHq();
  // `presentable` is the only bucket this page reads, so it is the only one a snapshot has
  // to carry for the inbox to draw: the withheld vocabulary is /ndd's, and a tool that
  // stopped emitting it — or emitted a bucket named something else — must not cost the
  // human inbox its number.
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 6,
    prs: [
      ...fcItems("design", 4).map((e) => ({ ...e, bucket: "noQuestion" })),
      ...fcItems("present", 2).map((e) => ({ ...e, bucket: "presentable" })),
    ],
    breakdown: { presentable: 2 },
  };
  hq.counts.designPresentable = 2;
  delete hq.counts.designNoQuestion;
  const box = fsmBox(hq);
  const design = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:design"
  );
  assert(
    collect(design, "sc")[0].textContent === "2",
    "the inbox draws off the one bucket",
  );
  design.click();
  assert(textOf(box).includes("o/present#1"), "and opens onto its rows");
  assert(
    collect(box, "fsm-defect").length === 0,
    "and nothing screams over the absent buckets",
  );
});

Deno.test("pipeline FSM: a presentable count broken low cannot hide the rows annotated into it", () => {
  const hq = splitHq();
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 6,
    prs: [
      ...fcItems("design", 4).map((e) => ({ ...e, bucket: "noQuestion" })),
      {
        repo: "o/present",
        number: 5,
        title: "present 5",
        bucket: "presentable",
      },
      {
        repo: "o/present",
        number: 6,
        title: "present 6",
        bucket: "presentable",
      },
    ],
    breakdown: {
      presentable: 0,
      noQuestion: 4,
      draft: 0,
      unaddressable: 0,
      fetchErrors: 0,
    },
  };
  hq.counts.designPresentable = 2;
  hq.counts.designNoQuestion = 4;
  const box = fsmBox(hq);
  const design = box.querySelectorAll("[data-t]").find((b) =>
    b.dataset.t === "ai:design"
  );
  // The bucket's stated count says nobody is servable; two rows are annotated into it. The
  // box renders the larger, so a broken count cannot hide an inbox the list proves is
  // there — and it opens onto exactly those rows.
  assert(
    collect(design, "sc")[0].textContent === "2",
    "the box renders the larger",
  );
  design.click();
  assert(
    textOf(box).includes("o/present#5"),
    "and opens onto the annotated rows",
  );
  const defects = textOf(collect(box, "fsm-defect")[0] || makeEl("div"));
  assert(
    defects.includes("breakdown.presentable states 0 but 2"),
    `the disagreement is named rather than rendered silently: ${defects}`,
  );
});

Deno.test("pipeline FSM: the design box's trend is drawn from designPresentable, not the raw label series", () => {
  const now = Date.parse("2026-08-09T00:00:00Z");
  const at = (d) => now - d * DAY;
  const rise = [1, 2, 3, 4, 5, 6, 7, 8];
  const cell = (presentable, noQuestion) => {
    const hq = splitHq();
    hq.lanes["vetter-verdicts"]["ai:design"] = {
      count: presentable + noQuestion,
      prs: [
        ...fcItems("present", presentable).map((e) => ({
          ...e,
          bucket: "presentable",
        })),
        ...fcItems("design", noQuestion).map((e) => ({
          ...e,
          bucket: "noQuestion",
        })),
      ],
      breakdown: {
        presentable,
        noQuestion,
        draft: 0,
        unaddressable: 0,
        fetchErrors: 0,
      },
    };
    hq.counts.design = presentable + noQuestion;
    hq.counts.designPresentable = presentable;
    hq.counts.designNoQuestion = noQuestion;
    return hq;
  };
  const designOf = (box) =>
    collect(box, "fsm-state").find((b) => b.dataset.t === "ai:design");
  // The raw label total climbs 9→16 while the half the box draws sits at 0: every new PR
  // is withheld. A rising border here would announce a bottleneck in a queue no human
  // command serves — the exact claim issue-pr-cron#240 removed from this box.
  const withheldClimb = rise.map((n, i) => ({
    t: at(7 - i),
    counts: {
      design: n + 8,
      designPresentable: 0,
      designNoQuestion: n + 8,
      ready: 5,
      leaks: 2,
    },
  }));
  assert(
    !designOf(fsmBox(cell(0, 16), withheldClimb)).classList.contains("rising"),
    "a climbing withheld pile is not this box's trend",
  );
  // The same shape the other way round: the presentable half climbs 1→8 under a label
  // total that never moves, and the box flags the bottleneck it does own.
  const presentableClimb = rise.map((n, i) => ({
    t: at(7 - i),
    counts: {
      design: 16,
      designPresentable: n,
      designNoQuestion: 16 - n,
      ready: 5,
      leaks: 2,
    },
  }));
  const flagged = designOf(fsmBox(cell(8, 8), presentableClimb));
  assert(flagged.classList.contains("rising"), "a filling /ndd inbox is");
  assert(
    (flagged.getAttribute("aria-label") || "").includes("rising"),
    `and says so in words: ${flagged.getAttribute("aria-label")}`,
  );
});

Deno.test("pipeline FSM: a split whose counts mirror disagrees is a loud defect naming both numbers", () => {
  const hq = splitHq();
  // The series key says 3; the breakdown the box renders says 0. The box draws the
  // breakdown; the sparkline would draw the mirror — the two-numbers divergence the
  // page refuses to render silently.
  hq.counts.designPresentable = 3;
  const box = fsmBox(hq);
  const defects = textOf(collect(box, "fsm-defect")[0] || makeEl("div"));
  assert(
    defects.includes("designPresentable") &&
      defects.includes("counted two ways"),
    `the divergence is named: ${defects}`,
  );
});

Deno.test("pipeline FSM: the drawn split key must be claimed; the withheld half's key is ratified unclaimed", () => {
  // `designPresentable` is the key the inbox box's series is DRAWN from. Emitted with no
  // breakdown behind it, no box draws it and the sweep says so rather than let the count
  // vanish.
  const orphan = descHq();
  orphan.counts.designPresentable = 3;
  const named = textOf(
    collect(fsmBox(orphan), "fsm-defect")[0] || makeEl("div"),
  );
  assert(
    named.includes("designPresentable") &&
      named.includes("claimed by no descriptor"),
    `an orphaned inbox key screams: ${named}`,
  );
  // `designNoQuestion` counts the rows /ndd withholds. Nothing here draws it: those PRs
  // are claimed as inventory by the ai:design cell, and /ndd reports the withholding
  // itself, so the key is one of the named frozen-legacy set and a nonzero one is silent
  // — with a breakdown behind it and without.
  const withheld = descHq();
  withheld.counts.designNoQuestion = 6;
  assert(
    collect(fsmBox(withheld), "fsm-defect").length === 0,
    "the withheld half's size is ratified unclaimed, not a defect",
  );
});

// ---- deployments.html: known owners ----

// The default `io` has no network: a test that renders a role-change section
// without standing in a subgraph sees every index as unreachable, never a
// real request.
const NO_NETWORK = {
  fetch: () => Promise.reject(new TypeError("no network in tests")),
  now: () => 0,
};
function deploymentsBox(data, io = NO_NETWORK) {
  return deploymentsRender(data, io).box;
}
// renderDeployments takes (document, $, data, io) as its own params, so bind
// with no injected free vars and call the returned function with the stubs.
// Returns the rendered box and the promise the live role-change read settles
// with.
function deploymentsRender(data, io = NO_NETWORK) {
  const box = makeEl("div");
  const document = {
    createElement: (t) => makeEl(t),
    createTextNode: (t) => t,
  };
  const $ = (id) => (id === "deployments" ? box : makeEl("div"));
  const done = bind("deployments.html", "renderDeployments", [], [])(
    document,
    $,
    data,
    io,
  );
  return { box, done };
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
// The real keccak256 of each role name, as the scan publishes it — the value a
// `grantRole` calldata word carries. Kept as literals so the page's rendering is
// tested against the ids a reviewer actually reads off a transaction, not
// against a placeholder the test invented.
const ROLE_IDS = {
  DEPOSIT: "0x87a7811f4bfedea3d341ad165680ae306b01aaeacc205d227629cf157dd9f821",
  DEPOSIT_ADMIN:
    "0x1ae915b310cb86de75afe5db1721d474dd0a8617151f7524866025476454bc02",
  WITHDRAW:
    "0x7a8dc26796a1e50e6e190b70259f58f6a4edd5b22280ceecc82b687b8e982869",
  CERTIFY: "0x50a07cb25d0d864370863300b20987dfdae089abad71b607faf639d09d053391",
};
const safeRole = (role, admin) => ({
  role,
  roleId: ROLE_IDS[role],
  admin,
  chains: [
    { network: "base", address: BASE_SAFE, status: "granted" },
    { network: "ethereum", address: ETH_SAFE, status: "granted" },
  ],
});
const serviceRole = (role) => ({
  role,
  roleId: ROLE_IDS[role],
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
      contract: "authoriser",
      source: "src/lib/LibAuthoriserInvariants.sol",
      function: "expectedGrants(address)",
      pinnedCount: 5,
      declaredCount: 5,
      chains: [
        {
          network: "base",
          target: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
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
          target: "0x66566cc91dEAf818859bD4b09B7903ac48998157",
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
const grantSections = (box) => collect(box, "tok-h3").map((h) => h.textContent);
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
  // Scoped to the KEY LIST rows. The same address may also appear in a
  // violated-expectation line above, which is a reference to this key, not a
  // second key — the thing this assertion exists to catch.
  const keyRows = collect(box, "own-row");
  const addr = keyRows.flatMap((r) => collect(r, "own-addr")).find((a) =>
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
    collect(box, "bcn-key").every((k) =>
      !/^[A-Z_]+$/.test(k.textContent || "")
    ),
    "no role name is rendered into the narrow fixed-width key",
  );
  const chips = chipText(box);
  assert(chips.includes("base ✓"), "granted on base");
  assert(chips.includes("ethereum ○"), "not yet granted on ethereum");
  // The per-grantee tally counts the chain cells, not the roles.
  assert(
    chips.includes("3/6 live"),
    "service key 3 of 6 cells: " + chips.join(","),
  );
});

// Reviewing a governance bundle means reading 32-byte role words out of
// `grantRole` / `renounceRole` calldata. The page names every role it checks, so
// it is the natural place to carry the id beside the name — otherwise a reviewer
// hashes each name by hand to find out which role a word refers to.
Deno.test("deployments: every role is listed with the full id the chain stores", () => {
  const box = deploymentsBox(grantsData());
  const ids = collect(box, "grt-roleid").map((n) => n.textContent);
  for (const [role, id] of Object.entries(ROLE_IDS)) {
    assert(ids.includes(id), "id for " + role + " shown, got " + ids.join(","));
  }
  // In FULL. A prefix cannot be matched against a calldata word, which is the
  // only thing this column is for, so a truncated or ellipsised id is a bug and
  // not a styling choice.
  for (const id of ids) {
    assert(
      /^0x[0-9a-f]{64}$/.test(id),
      "a role id renders as all 32 bytes, got " + id,
    );
  }
  // Beside its own name, in the same row — an id in a different row than its
  // name is worse than no id at all.
  const paired = collect(box, "bcn-line")
    .map((l) => [
      collect(l, "grt-role")[0],
      collect(l, "grt-roleid")[0],
    ])
    .filter(([n]) => n && ROLE_IDS[n.textContent])
    .map(([n, h]) => [n.textContent, h && h.textContent]);
  assert(paired.length >= 4, "found the role rows: " + JSON.stringify(paired));
  for (const [name, id] of paired) {
    assert(
      id === ROLE_IDS[name],
      name + " is paired with its own id, got " + id,
    );
  }
});

// Scan output predating the id carries no `roleId`. The column is dropped rather
// than rendering an empty slot or the string "undefined" next to a role name a
// reviewer is about to trust.
Deno.test("deployments: a role with no published id renders no id column", () => {
  const d = grantsData();
  for (const gr of d.deploymentGrants.grantees) {
    for (const r of gr.roles) delete r.roleId;
  }
  const box = deploymentsBox(d);
  assert(
    collect(box, "grt-roleid").length === 0,
    "no id column without an id",
  );
  assert(
    !textOf(box).includes("undefined"),
    "and nothing leaks the missing value as text",
  );
  // The roles themselves still list, with their chips.
  const keys = collect(box, "grt-role").map((k) => k.textContent);
  for (const r of ["DEPOSIT", "WITHDRAW", "CERTIFY"]) {
    assert(keys.includes(r), "role " + r + " still listed: " + keys.join(","));
  }
});

// The distinction the whole section turns on. A chain whose provisioning has
// not run yet is REPORTING A ROLLOUT, and a red row there would train the reader
// to ignore the one time it means a key really lost its grant.
Deno.test("deployments: a grant not yet live on a chain reads as a rollout, never as a fault", () => {
  const box = deploymentsBox(grantsData());
  const rollChips = collect(box, "own-chip-roll").map((c) => c.textContent);
  assert(
    rollChips.filter((t) => t === "ethereum ○").length === 3,
    "three not-yet-granted cells carry the rollout chip: " +
      rollChips.join(","),
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
    textOf(roll).includes("2 of 5 expectations met") &&
      textOf(roll).includes("3 expectations not met"),
    "the banner says what is actually true: " + textOf(roll),
  );
  assert(
    collect(box, "own-verify-ok").map(textOf).some((t) =>
      t.includes("base — all 5 expectations met")
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
  assert(
    safeRows.length === 2,
    "the Safe is in both tables, got " + safeRows.length,
  );
  const svcRows = collect(box, "own-role").filter((r) =>
    r.textContent === "GRANTEE_SERVICE_1C66"
  );
  assert(
    svcRows.length === 1,
    "the service key holds no admin role, so one row",
  );
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
  // Scoped to the KEY LIST rows, for the same reason as above.
  const svc = collect(box, "own-row").flatMap((r) => collect(r, "own-addr"))
    .filter((a) => a.textContent === SERVICE_EOA);
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
    target: null,
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
  assert(
    chips.includes("hyperevm ?"),
    "unread cells are a question, not a cross",
  );
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
  assert(
    chips.includes("3/6 live"),
    "3 of 6 cells confirmed: " + chips.join(","),
  );
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
    !collect(box, "tok-h3").some((h) =>
      h.textContent.startsWith("Action roles")
    ),
    "no grants section",
  );
  assert(
    collect(box, "own-group").length === 4,
    "the owner groups still render",
  );
  // …and an empty grantee list is the same: nothing, rather than a table that
  // would read as "no key holds these roles".
  const empty = deploymentsBox(grantsData({ grantees: [] }));
  assert(
    collect(empty, "tok-h3").length === 0,
    "an empty grantee list renders no section at all",
  );
});

const ORCH_INSTANCE = "0x3A7387a484d87Aa8bBA45E98AAB401Ce4FBF03E2";
function orchestratorGrants() {
  const chain = (network, safe, timelock, emergency) => ({
    network,
    target: ORCH_INSTANCE,
    safe,
    rpcHost: `${network}.rpc`,
    granted: emergency === "granted" ? 4 : 3,
    missing: emergency === "granted" ? 0 : 1,
    unknown: 0,
    total: 4,
    state: emergency === "granted" ? "live" : "partial",
    safeKept: [],
  });
  return {
    org: "S01-Issuer",
    repo: "st0x.deploy",
    contract: "orchestrator",
    source: "src/lib/LibOrchestratorInvariants.sol",
    function: "expectedGrants(address,address)",
    pinnedCount: 4,
    declaredCount: 4,
    chains: [
      chain(
        "base",
        BASE_SAFE,
        "0x48ba1371A78E6cC54157c63721756ab444510DB3",
        "granted",
      ),
      chain(
        "ethereum",
        ETH_SAFE,
        "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
        "missing",
      ),
    ],
    grantees: [
      {
        ident: "timelock",
        kind: "admin-holder",
        address: null,
        roles: [{
          role: "DEFAULT_ADMIN_ROLE",
          roleId: "0x" + "0".repeat(64),
          admin: true,
          chains: [
            {
              network: "base",
              address: "0x48ba1371A78E6cC54157c63721756ab444510DB3",
              status: "granted",
              safeHolds: false,
            },
            {
              network: "ethereum",
              address: "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
              status: "granted",
              safeHolds: false,
            },
          ],
        }],
      },
      {
        ident: "chainSafe",
        kind: "safe",
        address: null,
        roles: [{
          role: "EMERGENCY",
          roleId: "0x" + "e".repeat(64),
          admin: false,
          chains: [
            {
              network: "base",
              address: BASE_SAFE,
              status: "granted",
              safeHolds: null,
            },
            {
              network: "ethereum",
              address: ETH_SAFE,
              status: "missing",
              safeHolds: null,
            },
          ],
        }],
      },
    ],
  };
}

Deno.test("deployments: the orchestrator's grants render beside the authoriser's, read on the orchestrator instance", () => {
  const d = grantsData();
  d.deploymentOrchestratorGrants = orchestratorGrants();
  const box = deploymentsBox(d);
  const heads = tags(box, "h2").map((h) => h.textContent);
  const auth = heads.findIndex((h) => h.startsWith("Authoriser role grants"));
  const orch = heads.findIndex((h) => h.startsWith("Orchestrator role grants"));
  assert(
    auth >= 0 && orch === auth + 1,
    "orchestrator section follows the authoriser's: " + heads.join(" | "),
  );
  const banners = collect(box, "own-verify").map(textOf);
  const orchBanners = banners.filter((t) =>
    t.includes(`orchestrator ${ORCH_INSTANCE}`)
  );
  assert(
    orchBanners.length === 2,
    "one banner per chain names the instance: " + banners.join(" | "),
  );
  assert(
    orchBanners.some((t) =>
      t.startsWith("ethereum — 3 of 4 expectations met; 1 expectation not met")
    ),
    "ethereum's missing EMERGENCY holder is counted: " +
      orchBanners.join(" | "),
  );
  const viol = collect(box, "viol").map(textOf);
  assert(
    viol.some((t) =>
      t.includes("chainSafe") && t.includes(ETH_SAFE) && t.includes("EMERGENCY")
    ),
    "the violated expectation names the holder, its address and the role: " +
      viol.join(" | "),
  );
  const links = tags(box, "a").map((a) => a.textContent);
  assert(
    links.includes(
      "LibOrchestratorInvariants.expectedGrants(address,address)",
    ) &&
      links.includes("LibAuthoriserInvariants.expectedGrants(address)"),
    "each map is linked by its own library: " +
      links.filter((l) => l.includes("expectedGrants")).join(" | "),
  );
  const sections = grantSections(box);
  assert(
    sections.includes("Admin role — who can grant and revoke") &&
      sections.includes("Action roles — who can mint, burn and recover"),
    "the orchestrator's sections carry its own headings: " +
      sections.join(" | "),
  );
});

Deno.test("deployments: an orchestrator chain with no instance pinned says so, not 'authoriser'", () => {
  const d = grantsData();
  const o = orchestratorGrants();
  o.chains[1] = {
    ...o.chains[1],
    target: null,
    granted: 0,
    missing: 0,
    unknown: 2,
    total: 2,
    state: "unknown",
  };
  d.deploymentOrchestratorGrants = o;
  const box = deploymentsBox(d);
  const down = collect(box, "own-verify-down").map(textOf);
  assert(
    down.length === 1 &&
      down[0].includes("no orchestrator pinned for this chain yet"),
    "the unread orchestrator chain names the missing pin: " + down.join(" | "),
  );
});

Deno.test("deployments: with no orchestrator grant data its section is absent", () => {
  const box = deploymentsBox({
    ...grantsData(),
    deploymentOrchestratorGrants: null,
  });
  assert(
    !tags(box, "h2").some((h) =>
      h.textContent.startsWith("Orchestrator role grants")
    ),
    "no orchestrator section",
  );
  assert(
    tags(box, "h2").some((h) =>
      h.textContent.startsWith("Authoriser role grants")
    ),
    "the authoriser section still renders",
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

Deno.test("deployments: a timelock-owned beacon reads as governed, not drift", () => {
  const TARGET = "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286";
  const TIMELOCK = "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B";
  const data = {
    deploymentOwners: null,
    deploymentHealth: null,
    deploymentBeacons: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      network: "ethereum",
      rpcHost: "ethereum-rpc.publicnode.com",
      safeOwner: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
      targetVersion: "0.1.1",
      total: 1,
      healthy: 1,
      beacons: [
        {
          name: "Receipt beacon",
          address: "0xace121ae30d754536863a546f41b147be11202db",
          owner: TIMELOCK,
          ownerLabel: "timelock",
          implementation: TARGET,
          implVersion: "0.1.1",
          targetImpl: TARGET,
          targetVersion: "0.1.1",
          atTarget: true,
          status: "healthy",
        },
      ],
    },
  };
  const box = deploymentsBox(data);
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  // The owner is NAMED as the timelock, not rendered as an unknown dash — the
  // reader has to be able to see which governing holder it is.
  assert(
    chips.includes("timelock"),
    "owner labelled timelock: " + chips.join(","),
  );
  assert(!chips.includes("—"), "a timelock owner is not an unknown dash");
  assert(!chips.includes("foreign"), "a timelock owner is not foreign");
  // Before the timelock arm this beacon rendered drift, i.e. the completed
  // migration shown as a fault.
  assert(!chips.includes("drift"), "a timelock-owned beacon is not drift");
  assert(chips.includes("healthy"), "status healthy: " + chips.join(","));
  assert(
    collect(box, "own-verify-drift").length === 0,
    "no drift banner when every beacon is governed and at target",
  );
  const addrs = collect(box, "own-addr").map((a) => a.textContent);
  assert(
    addrs.some((a) => a.toLowerCase() === TIMELOCK.toLowerCase()),
    "the timelock address is shown so the reader can check which one it is",
  );
});

Deno.test("deployments: sub-tabs split the page and start on Safe & ownership", () => {
  const box = deploymentsBox(OWNERS);
  const btns = collect(box, "subnav-btn");
  assert(
    btns.length === 6,
    "six sub-tabs: " + btns.map((b) => b.textContent).join(","),
  );
  assert(
    btns.map((b) => b.dataset.tab).join(",") ===
      "safe,contracts,rbac,held,registry,migrations",
    "tabs in order safe,contracts,rbac,held,registry,migrations: " +
      btns.map((b) => b.dataset.tab).join(","),
  );
  // Exactly one active, and it is the first — a page that opens with none
  // active looks broken, and one that opens with both looks like no tabs.
  const active = btns.filter((b) => b.classList.contains("active"));
  assert(
    active.length === 1 && active[0].dataset.tab === "safe",
    "safe tab active on load",
  );

  const panels = collect(box, "tabpanel");
  assert(panels.length === 6, "six panels");
  const byTab = Object.fromEntries(panels.map((p) => [p.dataset.tab, p]));
  assert(
    !byTab.safe.classList.contains("tab-hidden"),
    "safe panel visible on load",
  );
  assert(
    byTab.contracts.classList.contains("tab-hidden"),
    "contracts panel hidden on load",
  );
  assert(
    byTab.rbac.classList.contains("tab-hidden"),
    "rbac panel hidden on load",
  );
  assert(
    byTab.held.classList.contains("tab-hidden"),
    "held panel hidden on load",
  );
  assert(
    byTab.migrations.classList.contains("tab-hidden"),
    "migrations panel hidden on load",
  );
});

// ---- deployments.html: migrations ----

const MIG_GENESIS =
  "0xd85a7f14bb19a07649810ac09029cc9c29667e4059cb83e258bc52e213365d3f";
const MIG_NS = "0x" + "aa".repeat(32);
const MIG_ID_1 = "0x" + "01".repeat(32);
const MIG_ID_2 = "0x" + "02".repeat(32);
const MIG_REGISTRY = "0xc89F95eC7e626CFBef37a1F502A69BaF2022F8A8";

// The shape `migrations::build` emits.
function migRow(over) {
  return {
    script: "script/one.s.sol",
    namespace: MIG_NS,
    id: MIG_ID_1,
    writer: ETH_SAFE,
    state: "notApplied",
    appliedAt: null,
    executedAt: null,
    waitsOn: [],
    ...over,
  };
}
function migNetwork(over) {
  return {
    chainId: 1,
    network: "ethereum",
    registry: MIG_REGISTRY,
    rpcHost: "ethereum-rpc.publicnode.com",
    unread: 0,
    migrations: [],
    ...over,
  };
}
function migData(networks) {
  return {
    ...OWNERS,
    deploymentMigrations: {
      org: "acme",
      repo: "deploy",
      path: "migrations.json",
      genesis: MIG_GENESIS,
      networks,
    },
  };
}
const migPanel = (box) =>
  collect(box, "tabpanel").find((p) => p.dataset.tab === "migrations");
const migRows = (panel) => collect(panel, "own-row");
const migChips = (node) => collect(node, "own-chip").map((c) => c.textContent);

Deno.test("deployments: the Migrations tab lists every migration per network with its state", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      migrations: [
        migRow({
          script: "script/history.s.sol",
          state: "notRecorded",
          executedAt: 1791056759,
        }),
        migRow({
          script: "script/no-record.s.sol",
          namespace: null,
          id: null,
          writer: null,
          state: "unregistered",
        }),
        migRow({ script: "script/pending.s.sol", id: MIG_ID_2 }),
      ],
    }),
    migNetwork({
      chainId: 8453,
      network: "base",
      registry: MIG_REGISTRY,
      rpcHost: "mainnet.base.org",
      migrations: [
        migRow({
          script: "script/done.s.sol",
          writer: BASE_SAFE,
          state: "applied",
          appliedAt: 1791457413,
        }),
      ],
    }),
  ]));
  const panel = migPanel(box);
  assert(
    textOf(panel).includes("Migrations — run or not, per network"),
    "migrations heading on the migrations tab",
  );
  const file = tags(collect(panel, "own-sub")[0], "a");
  assert(
    file.length === 1 &&
      file[0].textContent === "acme/deploy's migrations.json" &&
      file[0].href ===
        "https://github.com/acme/deploy/blob/HEAD/migrations.json",
    "the source file is named and linked",
  );
  const heads = collect(panel, "tok-h3").map((h) => h.textContent);
  assert(
    heads.join("|") ===
      "ethereum — 0 applied, 1 pending, 1 not recorded, 0 unknown, 1 with no registry record|" +
        "base — 1 applied, 0 pending, 0 not recorded, 0 unknown, 0 with no registry record",
    "per-network heads count states off the rows: " + heads.join("|"),
  );
  const rows = migRows(panel);
  assert(
    rows.length === 4,
    "one row per migration per network: " + rows.length,
  );
  assert(
    rows.map((r) => collect(r, "own-role")[0].textContent).join(",") ===
      "history.s.sol,no-record.s.sol,pending.s.sol,done.s.sol",
    "rows in file order, named by script file",
  );
  assert(
    collect(rows[0], "own-role")[0].href ===
      "https://github.com/acme/deploy/blob/HEAD/script/history.s.sol",
    "script links to its source: " + collect(rows[0], "own-role")[0].href,
  );
  assert(
    migChips(rows[0]).join() === "not recorded" &&
      textOf(rows[0]).includes("ran 2026-10-03 19:45 UTC"),
    "executed but unrecorded says when it ran: " + textOf(rows[0]),
  );
  assert(
    migChips(rows[1]).join() === "no registry record" &&
      collect(rows[1], "own-chip-no").length === 1 &&
      rows[1].className.split(" ").includes("mig-unregistered") &&
      textOf(rows[1]).includes(
        "error — writes no registry record, so it is in no line",
      ) &&
      collect(rows[1], "mig-hash").length === 0,
    "a migration with no registry record is an error, not a dim not-recorded",
  );
  assert(
    migChips(rows[2]).join() === "pending" &&
      rows[2].className.split(" ").includes("mig-notApplied") &&
      collect(rows[2], "own-chip-roll").length === 1,
    "not applied is the rollout amber",
  );
  assert(
    collect(rows[2], "mig-hash").map((h) => h.textContent).join("|") ===
      `id ${MIG_ID_2}|namespace ${MIG_NS}`,
    "full id and namespace shown",
  );
  assert(
    migChips(rows[3]).join() === "applied" &&
      collect(rows[3], "own-chip-yes").length === 1 &&
      textOf(rows[3]).includes("2026-10-08 11:03 UTC"),
    "applied carries the registry's time: " + textOf(rows[3]),
  );
  const writer = collect(rows[3], "own-addr")[0];
  assert(
    writer.textContent === BASE_SAFE &&
      writer.href === "https://basescan.org/address/" + BASE_SAFE,
    "writer linked on its own chain's explorer",
  );
  const where = collect(panel, "own-groupnote").map(textOf);
  assert(
    where[1].includes(MIG_REGISTRY) &&
      where[1].includes("via mainnet.base.org"),
    "registry and endpoint named per network: " + where[1],
  );
});

const migBanners = (panel) =>
  collect(panel, "mig-banner").map((b) => ({
    cls: b.className.split(" ").find((c) => c.startsWith("own-verify-")),
    text: textOf(b),
  }));

Deno.test("deployments: every declared migration not in its registry is a pending warning, per network and in total", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      migrations: [
        migRow({ script: "script/a.s.sol" }),
        migRow({
          script: "script/b.s.sol",
          id: MIG_ID_2,
          state: "notRecorded",
          executedAt: 1791056759,
        }),
        migRow({
          script: "script/c.s.sol",
          state: "applied",
          appliedAt: 1791457413,
        }),
      ],
    }),
    migNetwork({
      chainId: 8453,
      network: "base",
      migrations: [
        migRow({
          script: "script/a.s.sol",
          writer: BASE_SAFE,
          state: "applied",
          appliedAt: 1791457413,
        }),
      ],
    }),
    migNetwork({
      chainId: 56,
      network: "bsc",
      migrations: [migRow({ script: "script/a.s.sol", state: "unknown" })],
    }),
  ]));
  const panel = migPanel(box);
  const b = migBanners(panel);
  assert(b.length === 4, "one total banner and one per network: " + b.length);
  assert(
    b[0].cls === "own-verify-roll" &&
      b[0].text.startsWith(
        "2 required migrations pending across every network",
      ) &&
      b[0].text.includes("1 more could not be read") &&
      b[0].text.includes("ethereum 2") && !b[0].text.includes("base"),
    "total counts pending and not-recorded, names the networks owing them: " +
      b[0].text,
  );
  assert(
    b[1].cls === "own-verify-roll" &&
      b[1].text === "2 required migrations pending on ethereum",
    "ethereum warns: " + b[1].text,
  );
  assert(
    b[2].cls === "own-verify-ok" &&
      b[2].text === "Every declared migration is applied on base",
    "base is clean: " + b[2].text,
  );
  assert(
    b[3].cls === "own-verify-down" &&
      b[3].text === "Nothing known pending on bsc, but 1 could not be read",
    "an unread network is never reported clean: " + b[3].text,
  );
  const rows = migRows(panel);
  assert(
    ["mig-notApplied", "mig-notRecorded"].every((c, i) =>
      rows[i].className.split(" ").includes(c) &&
      collect(rows[i], "own-chip-roll").length === 1
    ) && migChips(rows[0]).join() === "pending",
    "pending and not-recorded rows are warning rows",
  );
  assert(
    collect(rows[2], "own-chip-roll").length === 0,
    "an applied row is not a warning",
  );
});

Deno.test("deployments: a network with every migration applied reads clean, and one pending reads singular", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      migrations: [migRow({ state: "applied", appliedAt: 1791457413 })],
    }),
    migNetwork({
      chainId: 8453,
      network: "base",
      migrations: [migRow({ writer: BASE_SAFE })],
    }),
  ]));
  const b = migBanners(migPanel(box));
  assert(
    b[0].text === "1 required migration pending across every networkbase 1" &&
      b[1].cls === "own-verify-ok" &&
      b[2].text === "1 required migration pending on base",
    "singular wording, clean network ok: " + b.map((x) => x.text).join("|"),
  );
  const clean = migBanners(migPanel(deploymentsBox(migData([
    migNetwork({
      migrations: [migRow({ state: "applied", appliedAt: 1791457413 })],
    }),
  ]))));
  assert(
    clean[0].cls === "own-verify-ok" &&
      clean[0].text ===
        "Every declared migration is applied across every network",
    "nothing pending anywhere is the clean total: " + clean[0].text,
  );
});

Deno.test("deployments: a migration names what it waits on and that prerequisite's own state", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      migrations: [
        migRow({
          waitsOn: [{
            writer: ETH_SAFE,
            namespace: MIG_NS,
            id: MIG_GENESIS,
            script: null,
            state: "genesis",
            appliedAt: null,
            executedAt: null,
          }],
        }),
        migRow({
          script: "script/two.s.sol",
          id: MIG_ID_2,
          waitsOn: [
            {
              writer: ETH_SAFE,
              namespace: MIG_NS,
              id: MIG_ID_1,
              script: "script/one.s.sol",
              state: "notApplied",
              appliedAt: null,
              executedAt: null,
            },
            {
              writer: ETH_SAFE,
              namespace: MIG_NS,
              id: MIG_ID_2,
              script: null,
              state: "applied",
              appliedAt: 1791457413,
              executedAt: null,
            },
            {
              writer: ETH_SAFE,
              namespace: MIG_NS,
              id: MIG_ID_1,
              script: "script/history.s.sol",
              state: "notRecorded",
              appliedAt: null,
              executedAt: 1791056759,
            },
          ],
        }),
      ],
    }),
  ]));
  const rows = migRows(migPanel(box));
  assert(
    textOf(rows[0]).includes("waits on") &&
      textOf(rows[0]).includes(
        "nothing — it opens its namespace's line in the registry",
      ) &&
      migChips(rows[0]).join() === "pending",
    "a genesis prerequisite opens its namespace's line, with no chip of its own",
  );
  const wait = collect(rows[1], "mig-wait");
  assert(
    wait.length === 2 && wait[0].textContent === "one.s.sol" &&
      wait[0].href ===
        "https://github.com/acme/deploy/blob/HEAD/script/one.s.sol",
    "a listed prerequisite is named by its script and linked",
  );
  assert(
    migChips(rows[1]).join() === "pending,applied,not recorded,pending",
    "each prerequisite carries its own chip, the row's last: " +
      migChips(rows[1]).join(),
  );
  assert(
    textOf(rows[1]).includes(MIG_ID_2) &&
      textOf(rows[1]).includes("2026-10-08 11:03 UTC"),
    "an unlisted prerequisite is named by its id, with its applied time",
  );
  assert(
    textOf(rows[1]).includes("ran 2026-10-03 19:45 UTC"),
    "an unrecorded prerequisite says when it ran: " + textOf(rows[1]),
  );
});

Deno.test("deployments: a migration entry with no script is skipped and the rest render", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      migrations: [
        null,
        migRow({ script: null }),
        migRow({ script: "" }),
        migRow(),
      ],
    }),
  ]));
  const panel = migPanel(box);
  const rows = migRows(panel);
  assert(
    rows.length === 1 &&
      collect(rows[0], "own-role")[0].textContent === "one.s.sol",
    "only the entry with a script is a row: " + rows.length,
  );
  assert(
    collect(panel, "tok-h3")[0].textContent ===
      "ethereum — 0 applied, 1 pending, 0 not recorded, 0 unknown, 0 with no registry record",
    "heading counts only the rendered rows",
  );
});

Deno.test("deployments: an unread registry reads unknown and says why, never not applied", () => {
  const box = deploymentsBox(migData([
    migNetwork({
      network: "newchain",
      rpcHost: null,
      unread: 2,
      migrations: [migRow({ state: "unknown", writer: null })],
    }),
  ]));
  const panel = migPanel(box);
  const where = textOf(collect(panel, "own-groupnote")[0]);
  assert(
    where.includes("no RPC endpoint for this network") &&
      where.includes("2 registry reads unanswered"),
    "no endpoint and the unanswered count are stated: " + where,
  );
  const row = migRows(panel)[0];
  assert(migChips(row).join() === "unknown", "unknown chip");
  assert(
    textOf(row).includes("names no writer on this network"),
    "a recorded migration with no writer here says so",
  );
  assert(
    collect(panel, "tok-h3")[0].textContent ===
      "newchain — 0 applied, 0 pending, 0 not recorded, 1 unknown, 0 with no registry record",
    "heading counts the unknown",
  );
  assert(
    collect(row, "own-addr").length === 0,
    "no address rendered for a missing writer",
  );
});

Deno.test("deployments: unreadable migration data is an honest unavailable and the page survives", () => {
  const cases = [
    [
      {
        org: "acme",
        repo: "deploy",
        path: "migrations.json",
        unavailable: "migrations.json: not schema version 1",
      },
      "Migrations unavailable — migrations.json: not schema version 1.",
    ],
    [null, "Migrations unavailable — the scan produced no migration data."],
    [
      undefined,
      "Migrations unavailable — the scan produced no migration data.",
    ],
    [
      { networks: [] },
      "Migrations unavailable — the scan produced no migration data.",
    ],
  ];
  for (const [v, want] of cases) {
    const box = deploymentsBox({ ...OWNERS, deploymentMigrations: v });
    const panel = migPanel(box);
    const empty = collect(panel, "mig-empty");
    assert(
      empty.length === 1 && empty[0].textContent === want,
      "unavailable states why: " + (empty[0] && empty[0].textContent),
    );
    assert(migRows(panel).length === 0, "no rows when unavailable");
    assert(
      collect(box, "tabpanel").find((p) => p.dataset.tab === "rbac") &&
        textOf(box).includes("Known owners"),
      "the rest of the page still renders",
    );
  }
  const unread = deploymentsBox({ deploymentMigrations: null });
  assert(
    collect(migPanel(unread), "mig-empty").length === 1,
    "unavailable also renders when the owner constants are unread",
  );
});

Deno.test("deployments: an unrecognised migration state is named, not coloured as a known one", () => {
  const box = deploymentsBox(migData([
    migNetwork({ migrations: [migRow({ state: "pending" })] }),
  ]));
  const row = migRows(migPanel(box))[0];
  assert(
    migChips(row).join() === "unrecognised state pending" &&
      collect(row, "own-chip-dim").length === 1,
    "unknown token shown as unrecognised: " + migChips(row).join(),
  );
});

Deno.test("deployments: migration fields render as text, never markup", () => {
  const payload = '<img src=x onerror="alert(1)">';
  const box = deploymentsBox(migData([
    migNetwork({
      network: payload,
      migrations: [migRow({
        script: `script/${payload}`,
        waitsOn: [{
          writer: ETH_SAFE,
          namespace: MIG_NS,
          id: payload,
          script: null,
          state: "notApplied",
        }],
      })],
    }),
  ]));
  const panel = migPanel(box);
  assert(
    collect(panel, "own-role")[0].textContent === payload,
    "script lands as text",
  );
  assert(
    collect(panel, "tok-h3")[0].textContent.startsWith(payload + " — "),
    "network lands as text",
  );
  assert(
    textOf(panel).includes(payload + "pending"),
    "unlisted id lands as text",
  );
  assert(tags(panel, "img").length === 0, "no element built from the payload");
});

Deno.test("deployments: clicking a sub-tab moves both the panel and the active mark", () => {
  const box = deploymentsBox(OWNERS);
  const btns = collect(box, "subnav-btn");
  const panels = Object.fromEntries(
    collect(box, "tabpanel").map((p) => [p.dataset.tab, p]),
  );
  const contractsBtn = btns.find((b) => b.dataset.tab === "contracts");

  contractsBtn.click();
  assert(
    panels.contracts && !panels.contracts.classList.contains("tab-hidden"),
    "contracts shown",
  );
  assert(panels.safe.classList.contains("tab-hidden"), "safe hidden");
  assert(
    contractsBtn.classList.contains("active"),
    "clicked tab marked active",
  );
  assert(
    !btns.find((b) => b.dataset.tab === "safe").classList.contains("active"),
    "the other tab is no longer active",
  );

  // And back, so the toggle is not one-way.
  btns.find((b) => b.dataset.tab === "safe").click();
  assert(!panels.safe.classList.contains("tab-hidden"), "safe shown again");
  assert(
    panels.contracts.classList.contains("tab-hidden"),
    "contracts hidden again",
  );
});

Deno.test("deployments: a closed tab's content is still in the document", () => {
  // The panels are hidden, never dropped. If a closed tab were unrendered,
  // find-in-page would miss it and every "is X reported?" assertion on this
  // page would silently depend on which tab happened to be open.
  const box = deploymentsBox({
    ...OWNERS,
    deploymentGrants: null,
    deploymentVaultOwners: null,
    deploymentHealth: null,
    deploymentBeacons: {
      org: "o",
      repo: "r",
      network: "ethereum",
      rpcHost: "h",
      safeOwner: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
      targetVersion: "0.1.1",
      total: 1,
      healthy: 1,
      beacons: [{
        name: "Receipt beacon",
        address: "0xace121ae30d754536863a546f41b147be11202db",
        owner: "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
        ownerLabel: "timelock",
        implementation: "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286",
        implVersion: "0.1.1",
        targetImpl: "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286",
        targetVersion: "0.1.1",
        atTarget: true,
        status: "healthy",
      }],
    },
  });
  const panels = Object.fromEntries(
    collect(box, "tabpanel").map((p) => [p.dataset.tab, p]),
  );
  // Beacons live on the contracts tab, which is closed on load.
  assert(
    panels.contracts.classList.contains("tab-hidden"),
    "contracts tab is the closed one",
  );
  const beaconChips = collect(panels.contracts, "own-chip").map((c) =>
    c.textContent
  );
  assert(
    beaconChips.includes("timelock"),
    "closed tab still holds its rows: " + beaconChips.join(","),
  );
  // And the whole-box queries the rest of this suite relies on still find it.
  assert(
    collect(box, "own-chip").map((c) => c.textContent).includes("timelock"),
    "a whole-box query reaches content on a closed tab",
  );
});

Deno.test("deployments: the Safe tab holds the ownership sections, not the contract ones", () => {
  const box = deploymentsBox({
    ...OWNERS,
    deploymentVaultOwners: {
      org: "o",
      repo: "r",
      source: "s",
      chains: [{
        network: "ethereum",
        safe: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
        timelock: "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
        vaults: [{
          vault: "0x8500189061e2206bc33bf04dc10ffb1fe7ded637",
          owner: "0x831e4e1bb2b9a67c00b7d17f252a18a22cd0bd2b",
          label: "timelock",
        }],
      }],
    },
  });
  const panels = Object.fromEntries(
    collect(box, "tabpanel").map((p) => [p.dataset.tab, p]),
  );
  const safeText = textOf(panels.safe);
  assert(safeText.includes("Known owners"), "Known owners is on the Safe tab");
  assert(
    safeText.includes("Receipt vaults — owner and certification"),
    "vault ownership is on the Safe tab: " + safeText.slice(0, 200),
  );
  assert(
    !textOf(panels.contracts).includes("Known owners"),
    "Known owners is not duplicated onto the contracts tab",
  );
});

Deno.test("deployments: the health and tokens sections link their own chain too", () => {
  // Both of these render functions take the chain from their own payload, so a
  // fixture can exercise them even though production only emits `base` today.
  // Without this, reverting either call site to a hardcoded Basescan URL left
  // every test green, because `explorer(addr, "base")` is byte-identical to the
  // string it replaced.
  const H = "0x2dF5cFE6d688EF9fF1B7c59A499D254b1527b286";
  const health = deploymentsBox({
    deploymentOwners: null,
    deploymentBeacons: null,
    deploymentHealth: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      version: "0.1.1",
      network: "ethereum",
      rpcHost: "ethereum-rpc.publicnode.com",
      total: 1,
      healthy: 1,
      contracts: [{
        name: "StoxReceipt",
        address: H,
        status: "healthy",
        codeMatch: true,
        hashMatch: true,
        erc165: "conformant",
      }],
    },
  });
  const hh = collect(health, "own-addr").map((a) => a.href).filter(Boolean);
  assert(
    hh.some((h) => h.startsWith("https://etherscan.io/address/")),
    "health section links etherscan on ethereum: " + hh.join(","),
  );
  assert(
    hh.every((h) => !h.includes("basescan.org")),
    "health section sends no ethereum address to Basescan: " + hh.join(","),
  );

  const T = "0xFb5B41acdbA20a3230F84BE995173CFb98b8D6E7";
  const tokens = deploymentsBox({
    deploymentOwners: null,
    deploymentBeacons: null,
    deploymentHealth: null,
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "bsc",
      rpcHost: "bsc-dataseed.bnbchain.org",
      total: 1,
      ok: 1,
      wrappedCount: 1,
      atAuthoriserTarget: 1,
      authoriser: { current: "0xc1", target: "0xt1", targetDeployed: true },
      tokens: [{
        symbol: "wtNVDA",
        name: "Wrapped NVIDIA Corporation ST0x",
        address: T,
        status: "ok",
        wrapped: true,
        nameOk: true,
        symbolOk: true,
        decimalsOk: true,
        assetOk: true,
      }],
    },
  });
  const th = collect(tokens, "own-addr").map((a) => a.href).filter(Boolean);
  assert(
    th.some((h) => h.startsWith("https://bscscan.com/address/")),
    "tokens section links bscscan on bsc: " + th.join(","),
  );
  assert(
    th.every((h) => !h.includes("basescan.org")),
    "tokens section sends no bsc address to Basescan: " + th.join(","),
  );
});

Deno.test("deployments: an address on an unknown chain is plain text, not a Base link", () => {
  // The old fallback sent an unrecognised chain's addresses to Basescan, where
  // the address is a different contract or nothing — a link that looks like
  // verification and is not.
  const A = "0xace121ae30d754536863a546f41b147be11202db";
  const box = deploymentsBox({
    deploymentOwners: null,
    deploymentHealth: null,
    deploymentBeacons: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      network: "someNewChain",
      rpcHost: "rpc.example.invalid",
      safeOwner: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
      targetVersion: "0.1.1",
      total: 1,
      healthy: 1,
      beacons: [{
        name: "Receipt beacon",
        address: A,
        owner: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
        ownerLabel: "safe",
        implementation: "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286",
        implVersion: "0.1.1",
        targetImpl: "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286",
        targetVersion: "0.1.1",
        atTarget: true,
        status: "healthy",
      }],
    },
  });
  const nodes = collect(box, "own-addr");
  assert(nodes.length > 0, "the address is still shown");
  assert(
    nodes.every((n) => !n.href),
    "an unknown chain's address carries no href at all",
  );
  assert(
    nodes.some((n) => n.textContent === A),
    "and is still readable as text: " +
      nodes.map((n) => n.textContent).join(","),
  );
});

Deno.test("deployments: per-chain sections link their OWN chain's explorer", () => {
  const ADDR = "0xace121ae30d754536863a546f41b147be11202db";
  const TARGET = "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286";
  const box = deploymentsBox({
    deploymentOwners: null,
    deploymentHealth: null,
    deploymentBeacons: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      network: "ethereum",
      rpcHost: "ethereum-rpc.publicnode.com",
      safeOwner: "0x3840aedaec8e82f79d8f6a8f6adca271e13e0329",
      targetVersion: "0.1.1",
      total: 1,
      healthy: 1,
      beacons: [{
        name: "Receipt beacon",
        address: ADDR,
        owner: "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
        ownerLabel: "timelock",
        implementation: TARGET,
        implVersion: "0.1.1",
        targetImpl: TARGET,
        targetVersion: "0.1.1",
        atTarget: true,
        status: "healthy",
      }],
    },
  });
  const hrefs = collect(box, "own-addr").map((a) => a.href).filter(Boolean);
  assert(hrefs.length > 0, "the beacon address is linked at all");
  // An Ethereum address on Basescan shows nothing, or a different contract at
  // the same address — a link that looks right and goes to the wrong chain.
  assert(
    hrefs.every((h) => !h.includes("basescan.org")),
    "no Ethereum link points at Basescan: " + hrefs.join(","),
  );
  assert(
    hrefs.some((h) => h.startsWith("https://etherscan.io/address/")),
    "the Ethereum beacon links etherscan.io: " + hrefs.join(","),
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
// The forced discriminant (a human ran the producer past the usage gate).
// Bound from the page for the same reason isSkip is: the tests must exercise
// the real predicate, never a copy that could drift from it.
const isForcedReal = bind("metrics.html", "isForced", [], []);
// The two shape-coercing readers behind the tooltip. Bound from the page for
// the same reason the predicate is: a copy here could drift from what ships.
const forcedKindsReal = bind("metrics.html", "forcedKinds", [], []);
const forcedReasonsReal = bind("metrics.html", "forcedReasons", [], []);
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
      "isForced",
      "forcedKinds",
      "forcedReasons",
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
      isForcedReal,
      forcedKindsReal,
      forcedReasonsReal,
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
      "isForced",
      "forcedKinds",
      "forcedReasons",
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
      isForcedReal,
      forcedKindsReal,
      forcedReasonsReal,
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
      "isForced",
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
      isForcedReal,
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
  assert(
    bootMinReal(partial) === 774 / 60000,
    "bootMin must convert to minutes",
  );
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
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, bare)),
  );
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

// --- forced runs (a human ran the producer past the usage gate) ---------------
// `--force` (issue-pr-cron#245) runs one tick past the gate's PAUSE on purpose,
// so a human can watch it. The row carries `forced` (the gate kind it was run
// past) and `forceReason` (that gate's own line, VERBATIM). It is the OPPOSITE
// of a skip: a skip is a tick the pipeline declined to run, a force is one it
// was told to run anyway — so a forced row is a RUN, with real measurements,
// and must keep its dot and its weight in every statistic. What it must not do
// is look like a tick the schedule produced.
const FORCED_RUN = {
  runId: "20260806T114500Z",
  role: "producer",
  stage: "final",
  startupPct: 22.5,
  startupMs: 90000,
  bootMs: 900,
  ttlMs: 60000,
  durationMs: 1800000,
  toolCalls: 200,
  startupToolCalls: 45,
  numTurns: 60,
  outcome: "ok",
  // PARALLEL LISTS: index i of `forced` is a stop's kind and index i of
  // `forceReason` is that stop's own line. A single forced run can walk past
  // both the kill switch and a gate pause (issue-pr-cron#245 as ruled), which
  // is why neither is a single value any more.
  forced: ["disabled", "usage-gate"],
  forceReason: [
    "DISABLED flag present",
    "PAUSE: 91% of the weekly budget used (endpoint) — at/over the 90% ceiling",
  ],
};

Deno.test("metrics forced: the gate field marks a forced run and nothing else", () => {
  // Keyed on the field's PRESENCE, never its value — gating on "usage-gate"
  // would silently stop marking the first other gate a force is ever aimed at.
  // Ordinary rows carry it not-at-all (absent, not null), which is what keeps
  // every row written before the field existed rendering exactly as it did.
  assert(
    isForcedReal(FORCED_RUN) === true,
    "the emitter's forced row is forced",
  );
  assert(
    isForcedReal({ ...FORCED_RUN, forced: ["some-future-stop"] }) === true,
    "an unknown stop kind is still a force",
  );
  for (const r of [RUN_SPLIT, RUN_LONG, PARTIAL_BOOT, SKIP_TICK, REAL_SKIP]) {
    assert(
      isForcedReal(r) === false,
      "an unforced row is never forced: " + JSON.stringify(r.runId),
    );
  }
  // And the two are never confused for each other in either direction.
  assert(isSkipReal(FORCED_RUN) === false, "a forced run is not a skip");
});

Deno.test("metrics forced: a forced run is a run — it charts, and it counts", () => {
  // The whole distinction from a skip. A skip is stripped before any renderer
  // sees it; a forced run really happened and really spent budget, so it stays
  // on the runs side and moves the tiles like any other.
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, FORCED_RUN)),
  );
  assert(
    runs.length === 2 && skips.length === 0,
    "a forced row belongs to the runs: " + JSON.stringify({ runs, skips }),
  );
  const [render, box] = pmBind("renderPmTiles", "pmtiles", "pct");
  render(runs);
  assert(
    textOf(box).includes("2"),
    "and is counted among the runs recorded: " + textOf(box),
  );
});

Deno.test("metrics forced: the chart rings the forced run, and only it", () => {
  const { runs } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, FORCED_RUN, RUN_SPLIT2)),
  );
  const wrap = pmChart(runs, "abs");
  assert(
    collect(wrap, "pm-forced").length === 1,
    "exactly one ring, for the one forced run",
  );
  assert(
    collect(wrap, "pm-dot").length === 3,
    "every run still draws its own dot, forced included",
  );
  // A second mark on the plot means the legend must name it, or identity rests
  // on the mark alone.
  assert(
    textOf(wrap).includes("forced run"),
    "the legend names the ring: " + textOf(wrap),
  );
});

Deno.test("metrics forced: the ring is ADDED to an errored run, never instead of it", () => {
  // Hue on this chart means "this run errored". A forced run can also error, so
  // the two cues have to compose: recolouring the dot for forcedness would make
  // the chart choose between two facts it must show at once.
  const { runs } = pmPartitionReal(pmRecordsReal(
    jsonl({ ...FORCED_RUN, outcome: "error" }),
  ));
  const wrap = pmChart(runs, "abs");
  assert(collect(wrap, "pm-forced").length === 1, "the ring is drawn");
  assert(
    collect(wrap, "deg").length >= 1,
    "and the errored styling survives beside it",
  );
});

Deno.test("metrics forced: a forced run with no plotted value gets no ring", () => {
  // A ring hovering at a y the run never had would invent a measurement. The
  // run is still admitted (its bootMs is real) — it is the MARK that is absent,
  // exactly as the dot is for any gap run.
  const gap = {
    runId: "20260806T154500Z",
    role: "producer",
    stage: "boot",
    bootMs: 1200,
    forced: ["usage-gate"],
    forceReason: ["PAUSE: 91% of the weekly budget used (endpoint)"],
  };
  const { runs } = pmPartitionReal(pmRecordsReal(jsonl(RUN_SPLIT, gap)));
  assert(runs.length === 2, "the partial forced run still loads");
  const wrap = pmChart(runs, "pct");
  assert(
    collect(wrap, "pm-forced").length === 0,
    "no ring without a point to ring",
  );
});

Deno.test("metrics forced: the end label clears the ring on a forced last run", () => {
  // Caught by READING the render, not by reasoning about it: at the unforced
  // offset the label sits inside the 7.5px ring and the two read as one glyph.
  const { runs } = pmPartitionReal(pmRecordsReal(jsonl(RUN_SPLIT, FORCED_RUN)));
  const forcedWrap = pmChart(runs, "abs");
  const plainWrap = pmChart(
    pmPartitionReal(pmRecordsReal(jsonl(RUN_SPLIT, RUN_SPLIT2))).runs,
    "abs",
  );
  const labelX = (wrap) => Number(collect(wrap, "pm-endlabel")[0].attrs.x);
  const dotX = (wrap) => {
    const dots = collect(wrap, "pm-dot");
    return Number(dots[dots.length - 1].attrs.cx);
  };
  const forcedGap = labelX(forcedWrap) - dotX(forcedWrap);
  const plainGap = labelX(plainWrap) - dotX(plainWrap);
  assert(
    forcedGap > plainGap,
    `a ringed last point pushes its label further out: ${forcedGap} vs ${plainGap}`,
  );
  assert(
    forcedGap > 7.5,
    "and past the ring's outer edge, not merely further: " + forcedGap,
  );
});

Deno.test("metrics forced: the tooltip names EVERY stop, each with its own line", () => {
  const t = textOf(pmTipBox(FORCED_RUN, true));
  for (const why of FORCED_RUN.forceReason) {
    assert(
      t.includes(why),
      "every stop's own line, verbatim — dropping one hides an override: " + t,
    );
  }
  assert(t.includes("forced past"), "and say what it was forced past: " + t);
  for (const kind of FORCED_RUN.forced) {
    assert(t.includes(kind), `naming the stop kind ${kind}: ` + t);
  }
  // Forcing describes how the run STARTED. The outcome is a separate reading
  // and must still be there, unchanged — otherwise "forced" reads as a verdict.
  assert(t.includes("ok"), "the outcome still renders: " + t);
  assert(
    !t.includes("undefined") && !t.includes("NaN"),
    "no leaks on hover: " + t,
  );
  // An unforced run says nothing about forcing at all.
  const plain = textOf(pmTipBox(RUN_SPLIT, true));
  assert(
    !plain.includes("forced"),
    "no phantom clause on an ordinary run: " + plain,
  );
});

Deno.test("metrics forced: a stop kind that is not a string still names itself", () => {
  // Fail-open on display, like the skip tip's kind line: the row is forced
  // whatever the field's shape, and printing "undefined" (or dropping the line)
  // would hide that.
  const t = textOf(pmTipBox({ ...FORCED_RUN, forced: [true, null] }, true));
  assert(t.includes("forced past"), "the line is still drawn: " + t);
  assert(!t.includes("undefined"), "and carries no leak: " + t);
  assert(!t.includes("null"), "and no null element leaks either: " + t);
});

Deno.test("metrics forced: a run that overrode NOTHING still says a human started it", () => {
  // The ordinary forced run once the crons are back: `--force` typed to watch a
  // run, with nothing in the way. The emitter writes `"forced": []`, and the
  // row's first job is still to say the schedule did not start it. Rendering
  // this as an ordinary tick is the corruption the whole field exists to
  // prevent, so the empty case gets its own words rather than an empty clause.
  const bare = { ...FORCED_RUN, forced: [], forceReason: [] };
  assert(isForcedReal(bare) === true, "empty is forced, not unforced");
  const t = textOf(pmTipBox(bare, true));
  assert(
    t.includes("forced"),
    "the tip still says the run was forced: " + t,
  );
  assert(
    !t.includes("forced past"),
    "…without claiming it walked past anything: " + t,
  );
  assert(!t.includes("undefined"), "no leak: " + t);
  // And it is still ringed and still counted.
  const { runs } = pmPartitionReal(pmRecordsReal(jsonl(RUN_SPLIT, bare)));
  assert(runs.length === 2, "an empty-override forced row is still a run");
  assert(
    collect(pmChart(runs, "abs"), "pm-forced").length === 1,
    "and still carries the ring",
  );
});

Deno.test("metrics forced: a legacy single-value row still renders", () => {
  // The emitter shipped `forced` as a bare string before the kill switch became
  // overridable (issue-pr-cron#246). No such row reached the live file, but the
  // page must not be the thing that decides that — a shape it cannot read would
  // render `undefined` beside a real run.
  const legacy = {
    ...FORCED_RUN,
    forced: "usage-gate",
    forceReason: "PAUSE: 91% of the weekly budget used (endpoint)",
  };
  const t = textOf(pmTipBox(legacy, true));
  assert(t.includes("forced past usage-gate"), "the kind still reads: " + t);
  assert(
    t.includes("PAUSE: 91% of the weekly budget used (endpoint)"),
    "and its line: " + t,
  );
  assert(!t.includes("undefined"), "no leak: " + t);
});

Deno.test("metrics forced: a file with no forced runs renders exactly as before", () => {
  // Backward compatibility is the whole reason the fields are ABSENT rather
  // than null on an ordinary row: every historical file has none, and none of
  // them is back-filled.
  const { runs, skips } = pmPartitionReal(
    pmRecordsReal(jsonl(RUN_SPLIT, SKIP_TICK, RUN_SPLIT2)),
  );
  const wrap = pmChart(runs, "abs", skips);
  assert(collect(wrap, "pm-forced").length === 0, "no rings");
  assert(
    !textOf(wrap).includes("forced run"),
    "and no legend key for a mark that is not on the plot: " + textOf(wrap),
  );
  assert(
    !textOf(pmNoteBox(RUN_SPLIT2, skips, runs)).includes("forced"),
    "and no caption sentence",
  );
});

Deno.test("metrics forced caption: counts them, and says they still count", () => {
  const runs = [RUN_SPLIT, FORCED_RUN];
  const t = textOf(pmNoteBox(FORCED_RUN, [], runs));
  assert(t.includes("1 run was started by hand"), "the count: " + t);
  assert(
    t.includes("counts in every figure"),
    "and the reading that separates it from a skip: " + t,
  );
  // The caption no longer names the usage gate, because the gate is no longer
  // the only stop a force can walk past — the kill switch is one too. Naming
  // one would mislabel a run that overrode the other.
  assert(
    !t.includes("forced past the usage gate"),
    "the caption must not claim one particular stop: " + t,
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
  assert(
    collect(box, "own-verify-error").length >= 1,
    "an empty token set is an error banner",
  );
});

Deno.test("deployments: an unreadable governed list names its error in the token section", () => {
  const box = deploymentsBox({
    deploymentTokens: {
      org: "ST0x-Technology",
      repo: "st0x.registry",
      network: "base",
      tokens: [],
      reconcile: {
        governedCount: 0,
        registryTokenCount: 23,
        function: "privileged-addresses.json (base receiptVaults)",
        error: "privileged-addresses.json could not be fetched",
        extraVaults: [],
        missingFromMigration: [],
      },
    },
  });
  const t = textOf(box);
  assert(
    t.includes(
      "The governed list could not be read: privileged-addresses.json could not be fetched.",
    ),
    "the producer's error is the stated cause: " + t,
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

function pmNoteBox(last, skips, runs) {
  const box = makeEl("div");
  const $ = (id) => (id === "pmnote" ? box : makeEl("div"));
  const d = pmDeps();
  const fmtAgo = bind("metrics.html", "fmtAgo", [], []);
  bind(
    "metrics.html",
    "renderPmNote",
    ["$", "document", "fmtAgo", "parseRunId", "outcomeWord", "isForced"],
    [$, stubDocument(), fmtAgo, d.parseRunId, outcomeWordReal, isForcedReal],
  )(last, skips, runs);
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
  // The id is published by the scan, not validated as hex by this page — so it
  // is as untrusted as the name beside it.
  const XSS_ID = '<iframe src="javascript:alert(1)">';
  d.deploymentGrants.grantees[1].roles[1].roleId = XSS_ID;
  d.deploymentGrants.grantees[1].roles[0].chains[0].address = XSS_SCRIPT;
  d.deploymentGrants.chains[0].network = XSS_ATTR;
  const box = deploymentsBox(d);
  assertInert(box, XSS_IMG, "grantee constant name");
  assertInert(box, XSS_SCRIPT, "grantee address");
  assertInert(box, XSS_SVG, "role name");
  assertInert(box, XSS_ID, "role id");
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

Deno.test("hostile input: a forced run's gate and reason render as tooltip text", () => {
  // forceReason is carried VERBATIM by contract, from a repo this page does not
  // own — the same verbatim guarantee that makes skipReason a markup carrier.
  // A distinct payload per field, so a field that stopped rendering as text
  // cannot pass on another field's copy.
  const box = pmTipBox({
    ...FORCED_RUN,
    forced: XSS_IMG,
    forceReason: XSS_SCRIPT,
  }, true);
  assertInert(box, XSS_IMG, "forced gate kind");
  assertInert(box, XSS_SCRIPT, "force reason");
});

Deno.test("hostile input: hovering a hostile forced run lands it in the tip as text", () => {
  // The integration half: pmTip being safe is worth nothing if the chart stops
  // calling it and interpolates the row itself.
  const run = { ...FORCED_RUN, forceReason: XSS_SCRIPT };
  const wrap = pmChart([RUN_SPLIT, run], "abs");
  const svg = pmSvg(wrap);
  const rect = { left: 0, top: 0, width: 720, height: 210 };
  svg._rect = rect;
  wrap._rect = rect;
  svg.fire("mousemove", { clientX: 719, clientY: 10 });
  assertInert(
    collect(wrap, "pm-tip")[0],
    XSS_SCRIPT,
    "forced tooltip on hover",
  );
  assert(
    markupNodes(wrap).length === 0,
    "no payload may become markup anywhere on the chart: " +
      markupNodes(wrap).join(", "),
  );
});

// The design split (issue-pr-cron#240) put PR titles and repo names on a new render path
// — the design box now opens onto the cell's `presentable` rows rather than the whole
// cell — built from the same attacker-influenceable snapshot fields every other list here
// is built from. The payload is planted in a presentable row, and DIFFERENT payloads ride
// the withheld rows the page draws nowhere: a filter that leaked a withheld row onto the
// panel would carry its payload with it, and one shared string could not tell the two
// apart.
Deno.test("hostile input: a design row's title and repo render as text on the presentable path", () => {
  const hq = descHq();
  hq.lanes["vetter-verdicts"]["ai:design"] = {
    count: 3,
    prs: [
      { repo: "o/design", number: 1, title: XSS_IMG, bucket: "presentable" },
      {
        repo: XSS_ATTR,
        number: 2,
        title: "withheld one",
        bucket: "noQuestion",
      },
      { repo: "o/design", number: 3, title: XSS_SCRIPT, bucket: "draft" },
    ],
    breakdown: {
      presentable: 1,
      noQuestion: 1,
      draft: 1,
      unaddressable: 0,
      fetchErrors: 0,
    },
  };
  hq.counts.design = 3;
  hq.counts.designPresentable = 1;
  hq.counts.designNoQuestion = 1;
  const box = fsmBox(hq);
  box.querySelectorAll("[data-t]").find((b) => b.dataset.t === "ai:design")
    .click();
  assertInert(box, XSS_IMG, "presentable row title");
  const text = textOf(box);
  assert(
    !text.includes(XSS_ATTR) && !text.includes(XSS_SCRIPT),
    "a withheld row reaches the panel by no path at all, payload included",
  );
  assert(
    markupNodes(box).length === 0,
    "no payload may become markup anywhere on the panel: " +
      markupNodes(box).join(", "),
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
      if (depth > 0) {
        faults.push(`nested /* at offset ${i} (CSS comments do not nest)`);
      } else openedAt = i;
      depth++, i += 2;
    } else if (css.startsWith("*/", i)) {
      if (depth === 0) {
        faults.push(`stray */ at offset ${i} with no comment open`);
      } else depth--;
      i += 2;
    } else {
      if (depth === 0) out += css[i];
      i++;
    }
  }
  if (depth > 0) {
    faults.push(`comment opened at offset ${openedAt} is never closed`);
  }
  return { out, faults };
};

Deno.test("stylesheet comments are balanced, so no rule is silently swallowed", () => {
  const dir = new URL("../site/", import.meta.url);
  const faults = [];
  for (const entry of Deno.readDirSync(dir)) {
    if (!entry.isFile || !entry.name.endsWith(".html")) continue;
    const src = Deno.readTextFileSync(new URL(entry.name, dir));
    styleBlocks(src).forEach((css, n) => {
      for (const f of scanCss(css).faults) {
        faults.push(`${entry.name} <style> #${n + 1}: ${f}`);
      }
    });
  }
  assert(
    faults.length === 0,
    "malformed stylesheet comment:\n" + faults.join("\n"),
  );
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
          offenders.push(
            `${entry.name} <style> #${n + 1}: not a selector: ${
              JSON.stringify(s.slice(0, 90))
            }`,
          );
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
  assert(
    offenders.length === 0,
    "prose in stylesheet position swallows the next rule:\n" +
      offenders.join("\n"),
  );
});

// The two guards above are structural. This one pins the specific rule the bug destroyed, in
// the terms that matter to a reader: the bottleneck highlight and its fallback must actually
// declare the red border, and must declare it TOGETHER so they can never drift apart.
Deno.test("the bottleneck highlight and its fallback survive parsing with the red border intact", () => {
  const src = Deno.readTextFileSync(
    new URL("../site/pipeline.html", import.meta.url),
  );
  const rules = styleBlocks(src).flatMap(cssRules);
  const hit = rules.find((r) => /\.fsm-state\.rising\b/.test(r.sel));
  assert(
    hit,
    "the .fsm-state.rising rule survives parsing — it is not swallowed by recovery",
  );
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

// The tab row is hand-declared on each page (the duplication rain-org-health#150
// catalogues), so nothing structural keeps one page's nav from drifting away from
// the others'. One expectation, checked against every page that renders the nav:
// the same five tabs in the same order, and the page's own tab — exactly one tab —
// carrying the active styling. index.html is not in the set: it is a bare redirect
// to deployments.html and renders no nav.
const NAV_TABS = [
  ["deployments.html", "Deployments"],
  ["pipeline.html", "Pipeline"],
  ["audit.html", "Audit"],
  ["metrics.html", "Metrics"],
  ["repositories.html", "Repositories"],
];

Deno.test("every page renders the same five nav tabs in the same order, its own tab active", () => {
  const want = NAV_TABS.map(([href, label]) => `${href}|${label}`).join(" ");
  for (const [page] of NAV_TABS) {
    const src = Deno.readTextFileSync(
      new URL(`../site/${page}`, import.meta.url),
    );
    const nav = src.match(/<nav class="nav">([\s\S]*?)<\/nav>/);
    assert(nav, `${page}: has a <nav class="nav"> block`);
    const tabs = [...nav[1].matchAll(/<a href="([^"]+)"([^>]*)>([^<]+)<\/a>/g)]
      .map((m) => ({
        href: m[1],
        active: /\bclass="active"/.test(m[2]),
        label: m[3],
      }));
    const got = tabs.map((t) => `${t.href}|${t.label}`).join(" ");
    assert(
      got === want,
      `${page}: nav tabs are ${
        NAV_TABS.map(([, l]) => l).join(" · ")
      } in that order, got ` +
        tabs.map((t) => t.label).join(" · "),
    );
    const active = tabs.filter((t) => t.active).map((t) => t.href);
    assert(
      active.length === 1 && active[0] === page,
      `${page}: exactly its own tab carries class="active", got [${
        active.join(", ")
      }]`,
    );
  }
});

Deno.test("Deployments is the first nav tab on every page", () => {
  const dir = new URL("../site/", import.meta.url);
  const checked = [];
  for (const entry of Deno.readDirSync(dir)) {
    if (
      !entry.isFile || !entry.name.endsWith(".html") ||
      entry.name === "index.html"
    ) continue;
    const src = Deno.readTextFileSync(new URL(entry.name, dir));
    const nav = src.match(/<nav class="nav">([\s\S]*?)<\/nav>/);
    assert(nav, `${entry.name}: has a <nav class="nav"> block`);
    const first = nav[1].match(/<a href="([^"]+)"[^>]*>([^<]+)<\/a>/);
    assert(
      first && first[1] === "deployments.html" && first[2] === "Deployments",
      `${entry.name}: first nav tab is Deployments, got ${first && first[2]}`,
    );
    checked.push(entry.name);
  }
  for (const [page] of NAV_TABS) {
    assert(checked.includes(page), `${page} was checked`);
  }
});

Deno.test("the site root redirects to Deployments", () => {
  const src = Deno.readTextFileSync(
    new URL("../site/index.html", import.meta.url),
  );
  assert(
    /<meta http-equiv="refresh" content="0; url=deployments\.html" \/>/.test(
      src,
    ),
    "the root refresh target is deployments.html",
  );
  assert(
    /<link rel="canonical" href="deployments\.html" \/>/.test(src),
    "the root canonical link is deployments.html",
  );
  assert(
    /<a href="deployments\.html">Deployments<\/a>/.test(src),
    "the root fallback link targets Deployments",
  );
});

// ---- deployments.html: receipt vault owner and certification ----

const BASE_TL = "0x48ba1371A78E6cC54157c63721756ab444510DB3";
const ETH_TL = "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B";

const vaultsDoc = (chains) => ({
  deploymentVaultOwners: { source: "privileged-addresses.json", chains },
});
const vaultChainRow = (network, safe, timelock, vaults) => ({
  network,
  safe,
  timelock,
  vaults,
  discoveredCount: vaults.length,
  discoveredAtBlock: 1,
});
const vrow = (vault, owner, label, certificationExpired) => ({
  vault,
  owner,
  label,
  certificationExpired,
  pinned: true,
  onChain: true,
});

Deno.test("deployments: an on-chain vault missing from the file is a warning with its pill", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("bsc", ETH_SAFE, ETH_TL, [
      vrow("0xaaa", ETH_TL, "timelock", false),
      { ...vrow("0xbbb", ETH_TL, "timelock", false), pinned: false },
    ]),
  ]));
  const t = textOf(box);
  assert(
    t.includes("bsc: 1 on-chain but not pinned in privileged-addresses.json"),
    "the warning names the chain and count, got: " + t,
  );
  assert(collect(box, "own-verify-drift").length >= 1, "warning banner");
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
  assert(collect(box, "own-verify-error").length === 0, "not an error");
  const pills = collect(box, "own-status-notpinned");
  assert(
    pills.length === 1 && pills[0].textContent === "not pinned",
    "one not-pinned pill",
  );
  const folded = collect(box, "vault-clean").map(textOf).join("|");
  assert(
    !folded.includes("0xbbb") && folded.includes("0xaaa"),
    "an unpinned vault is not folded as clean, got: " + folded,
  );
  assert(
    t.includes("1 not pinned, 0 not on-chain"),
    "counted in the chain heading, got: " + t,
  );
});

Deno.test("deployments: a pinned vault not found on-chain is an error with its pill", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("base", BASE_SAFE, BASE_TL, [
      vrow("0xaaa", BASE_TL, "timelock", false),
      { ...vrow("0xbbb", BASE_TL, "timelock", false), onChain: false },
    ]),
  ]));
  const t = textOf(box);
  assert(
    t.includes(
      "base: 1 pinned in privileged-addresses.json but not found on-chain",
    ),
    "the error names the chain and count, got: " + t,
  );
  assert(collect(box, "own-verify-error").length >= 1, "error banner");
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
  const pills = collect(box, "own-status-notonchain");
  assert(
    pills.length === 1 && pills[0].textContent === "not on-chain",
    "one not-on-chain pill",
  );
  const folded = collect(box, "vault-clean").map(textOf).join("|");
  assert(
    !folded.includes("0xbbb"),
    "a vault not on-chain is not folded as clean",
  );
});

Deno.test("deployments: a failed or absent discovery is an error, never clean", () => {
  const rows = [vrow("0xaaa", BASE_TL, "timelock", false)];
  const failed = {
    ...vaultChainRow("base", BASE_SAFE, BASE_TL, rows),
    discoveryError: "the subgraph did not answer",
  };
  delete failed.discoveredCount;
  const absent = { ...vaultChainRow("ethereum", ETH_SAFE, ETH_TL, rows) };
  delete absent.discoveredCount;
  const box = deploymentsBox(vaultsDoc([failed, absent]));
  const t = textOf(box);
  assert(
    t.includes(
      "base: on-chain vault discovery failed: the subgraph did not answer",
    ),
    "the producer's discovery error is shown, got: " + t,
  );
  assert(
    t.includes("ethereum: on-chain vault discovery did not run"),
    "a chain with no discovery result is an error, got: " + t,
  );
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
});

Deno.test("deployments: the chain note says how many vaults were found on-chain and when", () => {
  const box = deploymentsBox(vaultsDoc([
    {
      ...vaultChainRow("bsc", ETH_SAFE, ETH_TL, [
        vrow("0xaaa", ETH_TL, "timelock", false),
      ]),
      discoveredCount: 103,
      discoveredAtBlock: 129276993,
    },
  ]));
  const t = textOf(box);
  assert(t.includes("103 found on-chain at block 129276993"), "got: " + t);
});

const VAULTS_TWO_CHAINS = vaultsDoc([
  vaultChainRow("base", BASE_SAFE, BASE_TL, [
    vrow("0xaaa", BASE_TL, "timelock", false),
    vrow("0xbbb", BASE_SAFE, "safe", false),
    vrow("0xccc", null, "unknown", null),
  ]),
  vaultChainRow("ethereum", ETH_SAFE, ETH_TL, [
    vrow("0xeee", "0x00000000000000000000000000000000deadbeef", "other", true),
    vrow("0xfff", ETH_TL, "timelock", false),
  ]),
]);

const ALL_CLEAN = vaultsDoc([
  vaultChainRow("base", BASE_SAFE, BASE_TL, [
    vrow("0xaaa", BASE_TL, "timelock", false),
    vrow("0xbbb", BASE_TL, "timelock", false),
  ]),
]);

Deno.test("deployments: no vault-owner data is an error banner, never silence", () => {
  for (
    const d of [
      { deploymentVaultOwners: null },
      {},
      vaultsDoc([]),
      vaultsDoc([null]),
    ]
  ) {
    const box = deploymentsBox(d);
    const t = textOf(box);
    assert(
      t.includes("Receipt vaults unavailable"),
      "says unavailable, got: " + t,
    );
    assert(
      collect(box, "own-verify-error").length >= 1,
      "rendered as an error banner for " + JSON.stringify(d),
    );
  }
});

Deno.test("deployments: an unreadable vault source names its error", () => {
  const box = deploymentsBox({
    deploymentVaultOwners: {
      source: "privileged-addresses.json",
      error: "privileged-addresses.json is not valid JSON",
      chains: [],
    },
  });
  const t = textOf(box);
  assert(
    t.includes(
      "Receipt vaults unavailable — privileged-addresses.json is not valid JSON",
    ),
    "the producer's error is shown, got: " + t,
  );
});

Deno.test("deployments: a chain listing no vaults is an error, not a clean chain", () => {
  const box = deploymentsBox(vaultsDoc([
    {
      ...vaultChainRow("bsc", ETH_SAFE, ETH_TL, []),
      error: "privileged-addresses.json lists no receipt vaults for bsc",
    },
    vaultChainRow("hyperevm", ETH_SAFE, ETH_TL, []),
  ]));
  const t = textOf(box);
  assert(
    t.includes(
      "bsc: privileged-addresses.json lists no receipt vaults for bsc",
    ),
    "the chain's own error is shown, got: " + t,
  );
  assert(
    t.includes("hyperevm: no receipt vaults listed"),
    "an empty list without an error is still an error, got: " + t,
  );
  assert(
    !t.includes("Every receipt vault is owned"),
    "an empty chain is never the ok banner",
  );
});

Deno.test("deployments: a fully clean set gets the ok banner and folds its rows", () => {
  const box = deploymentsBox(ALL_CLEAN);
  const t = textOf(box);
  assert(
    collect(box, "own-verify-ok").length === 1 &&
      t.includes("Every receipt vault is owned by its chain's timelock"),
    "ok banner, got: " + t,
  );
  assert(collect(box, "own-verify-error").length === 0, "no error banner");
  assert(collect(box, "own-verify-drift").length === 0, "no warning banner");
  const folded = collect(box, "vault-clean");
  assert(folded.length === 1, "clean rows are folded into one details");
  assert(
    textOf(folded[0]).includes("2 on the timelock with current certification"),
    "the fold counts its rows, got: " + textOf(folded[0]),
  );
});

Deno.test("deployments: a vault off the timelock is a warning banner", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("base", BASE_SAFE, BASE_TL, [
      vrow("0xaaa", BASE_SAFE, "safe", false),
      vrow("0xbbb", BASE_TL, "timelock", false),
    ]),
  ]));
  const t = textOf(box);
  assert(
    t.includes("base: 1 of 2 not owned by the timelock"),
    "the warning names the chain and count, got: " + t,
  );
  assert(collect(box, "own-verify-drift").length >= 1, "warning banner");
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
  assert(collect(box, "own-verify-error").length === 0, "not an error");
});

Deno.test("deployments: expired certification is a warning with its own pill", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("hyperevm", ETH_SAFE, ETH_TL, [
      vrow("0xaaa", ETH_TL, "timelock", true),
      vrow("0xbbb", ETH_TL, "timelock", false),
    ]),
  ]));
  const t = textOf(box);
  assert(
    t.includes("hyperevm: 1 with expired certification"),
    "expiry is a warning, got: " + t,
  );
  assert(collect(box, "own-verify-drift").length >= 1, "warning banner");
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
  const expired = collect(box, "own-cert-expired");
  assert(expired.length === 1, "one expired pill");
  assert(expired[0].textContent === "cert expired", "expired pill text");
  assert(collect(box, "own-cert-current").length === 1, "one current pill");
  const folded = collect(box, "vault-clean").map(textOf).join("|");
  assert(
    !folded.includes("0xaaa") && folded.includes("0xbbb"),
    "an expired vault on the timelock is not folded as clean, got: " + folded,
  );
});

Deno.test("deployments: a failed read is an error banner naming which read", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("robinhood", ETH_SAFE, ETH_TL, [
      vrow("0xaaa", null, "unknown", null),
      vrow("0xbbb", ETH_TL, "timelock", null),
    ]),
  ]));
  const t = textOf(box);
  assert(
    t.includes(
      "robinhood: 1 owner() read(s) failed, 2 isCertificationExpired() read(s) failed",
    ),
    "both failed reads are counted, got: " + t,
  );
  assert(collect(box, "own-verify-error").length >= 1, "error banner");
  assert(collect(box, "own-verify-ok").length === 0, "never the ok banner");
  assert(collect(box, "own-cert-unknown").length === 2, "unknown cert pills");
});

Deno.test("deployments: every chain is rendered, named, with its own counts", () => {
  const text = textOf(deploymentsBox(VAULTS_TWO_CHAINS));
  assert(
    text.includes(
      "base — 1 timelock, 1 safe, 0 other, 0 unpinned, 1 unknown of 3",
    ),
    "base's tally, got: " + text,
  );
  assert(
    text.includes(
      "ethereum — 1 timelock, 0 safe, 1 other, 0 unpinned, 0 unknown of 2",
    ),
    "ethereum's tally, got: " + text,
  );
});

Deno.test("deployments: an unread vault is listed, it is this chain's", () => {
  const box = deploymentsBox(VAULTS_TWO_CHAINS);
  const listed = collect(box, "own-role")
    .map((r) => String(r.textContent || ""))
    .filter((s) => s.startsWith("0x"));
  for (const v of ["0xaaa", "0xbbb", "0xccc", "0xeee", "0xfff"]) {
    assert(listed.includes(v), v + " is listed, got: " + listed.join(","));
  }
  assert(
    collect(box, "own-status-unknown").length === 1,
    "the unread vault carries the unknown pill",
  );
});

Deno.test("deployments: each row gets the pill its own label names", () => {
  const box = deploymentsBox(VAULTS_TWO_CHAINS);
  assert(
    collect(box, "own-status-timelock").length === 2,
    "two timelock pills",
  );
  assert(collect(box, "own-status-safe").length === 1, "one safe pill");
  assert(collect(box, "own-status-other").length === 1, "one other pill");
  assert(collect(box, "own-status-unknown").length === 1, "one unknown pill");
  const owner = ["timelock", "safe", "other", "unknown", "unpinned"];
  const pills = collect(box, "own-status")
    .map((p) => p.textContent)
    .filter((t) => owner.includes(t))
    .sort();
  assert(
    pills.join(",") === "other,safe,timelock,timelock,unknown",
    "pill text matches the labels, got: " + pills.join(","),
  );
});

Deno.test("deployments: problem rows are shown and clean rows folded", () => {
  const box = deploymentsBox(VAULTS_TWO_CHAINS);
  const folded = collect(box, "vault-clean");
  const foldedText = folded.map(textOf).join("|");
  assert(folded.length === 2, "one fold per chain with clean rows");
  assert(
    foldedText.includes("0xaaa") && foldedText.includes("0xfff"),
    "clean rows are in the folds, got: " + foldedText,
  );
  for (const v of ["0xbbb", "0xccc", "0xeee"]) {
    assert(!foldedText.includes(v), v + " is a problem row, not folded");
  }
});

Deno.test("deployments: a red other row carries the pins to compare it against", () => {
  const text = textOf(deploymentsBox(VAULTS_TWO_CHAINS));
  assert(text.includes(ETH_TL), "ethereum's timelock pin is shown");
  assert(text.includes(ETH_SAFE), "ethereum's safe pin is shown");
});

Deno.test("deployments: one malformed chain does not abandon the page", () => {
  for (
    const chains of [
      [null],
      [{ network: "base", vaults: "oops" }],
      [{ network: "base", vaults: [null] }],
      [{ network: "base", vaults: [{ label: "safe" }] }],
    ]
  ) {
    const box = deploymentsBox(vaultsDoc(chains));
    assert(
      textOf(box).includes("Receipt vaults — owner and certification"),
      "the section still renders for " + JSON.stringify(chains),
    );
  }
});

Deno.test("deployments: a label this page cannot colour is held out and counted", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("base", BASE_SAFE, BASE_TL, [
      vrow(
        "0xaaa",
        "0x00000000000000000000000000000000deadbeef",
        "renounced",
        false,
      ),
    ]),
  ]));
  const text = textOf(box);
  assert(
    collect(box, "own-status-renounced").length === 0,
    "an unknown label is not rendered as a pill",
  );
  assert(
    text.includes("1 with a label this page does not know"),
    "it is counted and named instead, got: " + text,
  );
  assert(collect(box, "own-verify-error").length >= 1, "as an error");
});

Deno.test("deployments: an unpinned vault is listed with its own pill, not hidden", () => {
  const box = deploymentsBox(vaultsDoc([
    vaultChainRow("bsc", ETH_SAFE, null, [
      vrow(
        "0xddd",
        "0x00000000000000000000000000000000deadbeef",
        "unpinned",
        false,
      ),
    ]),
  ]));
  assert(
    collect(box, "own-status-unpinned").length === 1,
    "the unpinned pill is rendered",
  );
  const text = textOf(box);
  assert(text.includes("1 unpinned, 0 unknown of 1"), "counted, got: " + text);
  assert(
    text.includes('"unpinned" means the chain has no timelock pinned'),
    "the section explains unpinned, got: " + text,
  );
});

Deno.test("deployments: the admin holder is a timelock, never a hot single signer", () => {
  // The timelock is the slowest, coldest principal in the system. Falling into
  // the service-key branch described it as a hot single signer on the rows
  // saying who can grant and revoke — the inversion of what the migration is
  // for, made credible by the per-chain addresses beside it.
  const d = grantsData();
  d.deploymentGrants.grantees = [
    {
      ident: "adminHolder",
      kind: "admin-holder",
      address: null,
      roles: [
        {
          role: "DEPOSIT_ADMIN",
          admin: true,
          chains: [
            {
              network: "base",
              address: "0x48ba1371A78E6cC54157c63721756ab444510DB3",
              status: "missing",
            },
          ],
        },
      ],
    },
  ];
  const text = textOf(deploymentsBox(d));
  assert(
    !text.includes("hot single signer"),
    "the timelock is not called a hot single signer, got: " + text,
  );
  assert(
    text.includes("governance timelock"),
    "it is named as the timelock, got: " + text,
  );
});

Deno.test("deployments: a chain lists the expectations it does not meet", () => {
  // A count says how many without saying which, and a sentence explaining WHY
  // is a sentence about whatever rollout is in flight — it goes stale when that
  // rollout lands. The durable statement is the expectation itself: this holder
  // should hold these roles, and does not.
  const d = grantsData();
  d.deploymentGrants.chains = [{
    network: "base",
    target: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
    safe: BASE_SAFE,
    rpcHost: "mainnet.base.org",
    granted: 3,
    missing: 2,
    unknown: 0,
    total: 5,
    state: "partial",
  }];
  d.deploymentGrants.grantees = [{
    ident: "adminHolder",
    kind: "admin-holder",
    address: null,
    roles: [
      {
        role: "DEPOSIT_ADMIN",
        admin: true,
        chains: [{
          network: "base",
          address: "0x48ba",
          status: "missing",
          safeHolds: true,
        }],
      },
      {
        role: "WITHDRAW_ADMIN",
        admin: true,
        chains: [{
          network: "base",
          address: "0x48ba",
          status: "missing",
          safeHolds: true,
        }],
      },
    ],
  }];
  const box = deploymentsBox(d);
  const rows = collect(box, "viol");
  // ONE row, not one per role: the same holder repeated down two rows is noise,
  // and it would print that holder twice. The Safe's own retention is a
  // separate row driven by the chain's `safeKept`, which this chain does not
  // carry — so nothing else is listed here.
  assert(rows.length === 1, "one row per holder, got " + rows.length);
  const t = textOf(rows[0]);
  assert(t.includes("adminHolder"), "names the holder: " + t);
  assert(t.includes("0x48ba"), "and the address it should be: " + t);
  assert(t.includes("should hold"), "and what it should hold: " + t);
  assert(
    t.includes("DEPOSIT_ADMIN") && t.includes("WITHDRAW_ADMIN"),
    "lists both roles: " + t,
  );
  assert(t.includes("it does not"), "states the violation: " + t);
  // No narrative about any particular rollout.
  const page = textOf(box);
  assert(
    !page.includes("migration has not run"),
    "no migration-specific narrative",
  );
  assert(!page.includes("not provisioned"), "no provisioning vocabulary");
});

Deno.test("deployments: an unprobed holder is not reported as nobody holding it", () => {
  // `safeHolds: null` means no other holder was asked about. Reporting that as
  // "nobody has it" asserts more than was read — the direction that turns an
  // unread field into a finding.
  const d = grantsData();
  d.deploymentGrants.chains = [{
    network: "base",
    target: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
    safe: BASE_SAFE,
    rpcHost: "mainnet.base.org",
    granted: 4,
    missing: 1,
    unknown: 0,
    total: 5,
    state: "partial",
  }];
  const mk = (safeHolds) => ({
    ident: "adminHolder",
    kind: "admin-holder",
    address: null,
    roles: [{
      role: "DEPOSIT_ADMIN",
      admin: true,
      chains: [{
        network: "base",
        address: "0x48ba",
        status: "missing",
        safeHolds,
      }],
    }],
  });

  d.deploymentGrants.grantees = [mk(null)];
  const unread = textOf(collect(deploymentsBox(d), "viol")[0]);
  assert(
    unread.includes("no other holder was read"),
    "unprobed says so: " + unread,
  );
  assert(
    !unread.includes("nor does"),
    "and does not claim nobody holds it: " + unread,
  );

  d.deploymentGrants.grantees = [mk(false)];
  const none = textOf(collect(deploymentsBox(d), "viol")[0]);
  assert(
    none.includes("nor does the chain's Safe"),
    "a probed absence says so: " + none,
  );

  d.deploymentGrants.grantees = [mk(true)];
  const safe = textOf(collect(deploymentsBox(d), "viol")[0]);
  assert(
    safe.includes("the chain's Safe does"),
    "a Safe-held role says so: " + safe,
  );
});

Deno.test("deployments: an admin role the Safe still holds says so", () => {
  // The declared map names the timelock, so pre-migration every `_ADMIN` row
  // is missing. Without naming the Safe, the page's answer to "who can grant
  // and revoke" is nobody, anywhere — while the Safe holds all seven.
  const d = grantsData();
  d.deploymentGrants.grantees = [
    {
      ident: "adminHolder",
      kind: "admin-holder",
      address: null,
      roles: [
        {
          role: "DEPOSIT_ADMIN",
          admin: true,
          chains: [
            {
              network: "base",
              address: "0x48ba1371A78E6cC54157c63721756ab444510DB3",
              status: "missing",
              safeHolds: true,
            },
            {
              network: "ethereum",
              address: "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B",
              status: "missing",
              safeHolds: false,
            },
          ],
        },
      ],
    },
  ];
  const text = textOf(deploymentsBox(d));
  assert(
    text.includes("base ○ safe"),
    "the Safe holding it is named, got: " + text,
  );
  assert(
    text.includes("ethereum ○ unheld"),
    "and nobody holding it is distinct from that, got: " + text,
  );
});

// The half of a migration the positive check structurally cannot see. Granting
// the timelock the role and having the Safe renounce it are two steps; if the
// grant lands and the renounce does not, every declared holder holds what it
// should and the Safe can still grant and revoke at will. Before this was
// reported the page called that chain fully met.
Deno.test("deployments: a Safe that kept an admin role is a violated expectation, even where the timelock has it too", () => {
  const d = grantsData();
  d.deploymentGrants.chains = [{
    network: "base",
    target: "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
    safe: BASE_SAFE,
    rpcHost: "mainnet.base.org",
    // The scan counts the renounce as its own expectation, so a chain whose
    // grant landed and whose renounce did not is partial, not live.
    // Every declared grant is in place — the renounce is NOT folded into this
    // tally, so the chain is `live` on its declared grants and still carries a
    // violated expectation.
    granted: 1,
    missing: 0,
    unknown: 0,
    total: 1,
    state: "live",
    safeKept: ["DEPOSIT_ADMIN"],
  }];
  d.deploymentGrants.grantees = [{
    ident: "adminHolder",
    kind: "admin-holder",
    address: null,
    roles: [{
      role: "DEPOSIT_ADMIN",
      roleId: ROLE_IDS.DEPOSIT_ADMIN,
      admin: true,
      // GRANTED to the timelock — and the Safe never gave it up.
      chains: [{
        network: "base",
        address: "0x48ba1371A78E6cC54157c63721756ab444510DB3",
        status: "granted",
        safeHolds: true,
      }],
    }],
  }];
  const box = deploymentsBox(d);
  const text = textOf(box);
  // A chain whose declared grants are all in place must NOT read as clean
  // while the Safe keeps a role: that green banner was the whole defect.
  // Scoped to THIS chain's banner — the page carries other verify banners
  // (the Safe roster's, for one) that have nothing to do with the grants.
  const banner = collect(box, "own-verify").find((b) =>
    textOf(b).startsWith("base —")
  );
  assert(banner, "found the chain's banner");
  assert(
    !banner.className.includes("own-verify-ok"),
    "it is not the clean green one, got " + banner.className,
  );
  assert(
    textOf(banner).includes("the Safe still holds 1 it should not"),
    "and it says why, got: " + textOf(banner),
  );
  assert(
    text.includes("should NOT hold"),
    "the retention is stated as a violated expectation, got: " + text,
  );
  assert(
    text.includes("it still does"),
    "and says the Safe still holds it, got: " + text,
  );
  // Against the Safe's OWN address, not the timelock's — naming the wrong
  // address here points the reader at the holder that is behaving correctly.
  const viol = collect(box, "viol").find((v) =>
    textOf(v).includes("should NOT hold")
  );
  assert(viol, "found the retention row");
  const addrs = collect(viol, "own-addr").map((a) => a.textContent);
  assert(
    addrs.includes(BASE_SAFE),
    "the Safe's address is named, got " + addrs.join(","),
  );
  assert(
    !addrs.includes("0x48ba1371A78E6cC54157c63721756ab444510DB3"),
    "and not the timelock's, got " + addrs.join(","),
  );
  assert(
    textOf(viol).includes("DEPOSIT_ADMIN"),
    "the role is named, got: " + textOf(viol),
  );
});

// `safeHolds: false` is the migrated state and must stay silent, or every
// healthy chain grows a violation row that never clears.
Deno.test("deployments: a Safe that gave the role up is not reported", () => {
  const d = grantsData();
  d.deploymentGrants.grantees = [{
    ident: "adminHolder",
    kind: "admin-holder",
    address: null,
    roles: [{
      role: "DEPOSIT_ADMIN",
      roleId: ROLE_IDS.DEPOSIT_ADMIN,
      admin: true,
      chains: [
        {
          network: "base",
          address: BASE_SAFE,
          status: "granted",
          safeHolds: false,
        },
        // Unprobed is not evidence of absence, but it is not evidence of
        // retention either — it must not manufacture a violation.
        {
          network: "ethereum",
          address: ETH_SAFE,
          status: "granted",
          safeHolds: null,
        },
      ],
    }],
  }];
  const box = deploymentsBox(d);
  const text = textOf(box);
  assert(!text.includes("should NOT hold"), "no retention row, got: " + text);
  assert(
    !text.includes("it still does"),
    "and no retention clause, got: " + text,
  );
  // The MIGRATED end state. `safeHolds: false` on a granted row means the
  // declared holder has it and the Safe gave it up — the chip said "✓ unheld"
  // there, a green tick calling a correctly-held role unheld, because the
  // suffix read `safeHolds` without reading `status`.
  const chips = collect(box, "own-chip").map((c) => c.textContent);
  assert(
    !chips.some((t) => t.includes("unheld")),
    "a granted role is never labelled unheld, got " + chips.join(","),
  );
  assert(
    chips.includes("base ✓"),
    "it reads as plainly granted, got " + chips.join(","),
  );
});

// The other half of the same chip rule: on a granted row the Safe holding it
// too is the one thing still worth saying, or the chip reads clean while a
// violated expectation sits above it.
Deno.test("deployments: a granted role the Safe also kept is marked on the chip", () => {
  const d = grantsData();
  d.deploymentGrants.grantees = [{
    ident: "adminHolder",
    kind: "admin-holder",
    address: null,
    roles: [{
      role: "DEPOSIT_ADMIN",
      roleId: ROLE_IDS.DEPOSIT_ADMIN,
      admin: true,
      chains: [{
        network: "base",
        address: BASE_SAFE,
        status: "granted",
        safeHolds: true,
      }],
    }],
  }];
  const chips = collect(deploymentsBox(d), "own-chip").map((c) =>
    c.textContent
  );
  assert(
    chips.includes("base ✓ +safe"),
    "granted, and the Safe has it too, got " + chips.join(","),
  );
});

// ---- deployments.html: role changes (live RBAC subgraph read) ----

const RBAC_AUTH = "0x66566cc91deaf818859bd4b09b7903ac48998157";
const RBAC_TL = "0x831e4e1bb2b9a67c00b7d17f252a18a22cd0bd2b";
const RBAC_HASH = "0x" + "ab".repeat(32);
const RBAC_ZERO = "0x" + "0".repeat(64);
const RBAC_URL = "https://q.example/subgraphs/st0x-rbac-mainnet/prod/gn";
// The indexed block's time; `now` defaults to 12s later — a current index.
const RBAC_TS = 1791883787;

function rbacData(networks, extra = {}) {
  return {
    ...grantsData(),
    deploymentRbac: {
      org: "S01-Issuer",
      repo: "st0x.deploy",
      tag: "prod",
      subgraph: "st0x-rbac",
      networks,
      roleNames: {
        [ROLE_IDS.DEPOSIT]: "DEPOSIT",
        [ROLE_IDS.DEPOSIT_ADMIN]: "DEPOSIT_ADMIN",
        [RBAC_ZERO]: "DEFAULT_ADMIN_ROLE",
      },
      ...extra,
    },
  };
}
const rbacNet = (over = {}) => ({
  network: "mainnet",
  chain: "ethereum",
  endpoint: RBAC_URL,
  ...over,
});

// A stand-in subgraph: answers `_meta` and the three paged entity reads the
// way graph-node does (rows past the `id_gt` cursor, `first` and `held: true`
// honoured),
// recording every request it was sent.
function fakeSubgraph(over = {}) {
  const rid = (c, r) => c + r.slice(2);
  const role = (c, kind, r, admin) => ({
    id: rid(c, r),
    role: r,
    admin,
    contract: { id: c, kind, firstIndexedBlock: "5" },
  });
  const grant = (id, r, account, granted, block, log, tx) => ({
    id,
    account,
    granted,
    sender: "0x5a",
    blockNumber: String(block),
    blockTimestamp: String(1791883000 + block),
    transactionHash: tx,
    logIndex: String(log),
    role: { id: r },
  });
  const f = {
    meta: {
      data: {
        _meta: {
          block: { number: 900, timestamp: RBAC_TS },
          hasIndexingErrors: false,
        },
      },
    },
    rows: {
      roles: [
        role(RBAC_TL, "GOVERNANCE_TIMELOCK", ROLE_IDS.DEPOSIT, RBAC_ZERO),
        role(RBAC_AUTH, "AUTHORIZER", ROLE_IDS.DEPOSIT, ROLE_IDS.DEPOSIT_ADMIN),
        role(RBAC_AUTH, "AUTHORIZER", RBAC_HASH, ROLE_IDS.DEPOSIT_ADMIN),
      ],
      roleHolders: [
        {
          id: "h1",
          account: "0xbb",
          grantedAtBlock: "10",
          grantedAtTimestamp: "1791883010",
          role: { id: rid(RBAC_AUTH, ROLE_IDS.DEPOSIT) },
        },
        {
          id: "h2",
          account: "0xaa",
          grantedAtBlock: "12",
          grantedAtTimestamp: "1791883012",
          role: { id: rid(RBAC_AUTH, ROLE_IDS.DEPOSIT) },
        },
      ],
      roleGrants: [
        grant(
          "g1",
          rid(RBAC_AUTH, ROLE_IDS.DEPOSIT),
          "0xaa",
          true,
          12,
          3,
          "0xt1",
        ),
        grant("g2", rid(RBAC_AUTH, RBAC_HASH), "0xcc", false, 12, 7, "0xt1"),
        grant(
          "g3",
          rid(RBAC_AUTH, ROLE_IDS.DEPOSIT),
          "0xbb",
          true,
          10,
          1,
          "0xt0",
        ),
        // A grant on a role this snapshot holds no row for is not attributed.
        grant("g4", "0xnope", "0xdd", true, 11, 1, "0xt2"),
      ],
      // Read only when the schema probe finds the timelock entities.
      contracts: [],
      timelockOperations: [],
      timelockCalls: [],
    },
    // The schema probe's answer: `t: null` is a subgraph version without
    // `TimelockOperation`, which is what the prod tag serves until it moves.
    // `null` here is a failed fetch.
    schema: { data: { t: null } },
    // The registry probe's answer, as `schema` is the timelock's: `t: null` is a
    // version that does not index the registries.
    registrySchema: {
      data: {
        t0: null,
        t1: null,
        t2: null,
        t3: null,
        t4: null,
        t5: null,
        t6: null,
        t7: null,
        t8: null,
      },
    },
    // entity name -> a raw response body to answer with instead of rows.
    raw: {},
    status: 200,
    now: (RBAC_TS + 12) * 1000,
    seen: [],
    // ideal-roles.json: an object is served as JSON, a string as a body that
    // is not JSON, an Error as a failed fetch, null as a 404.
    ideal: null,
    idealSeen: [],
    ...over,
  };
  f.io = {
    now: () => f.now,
    fetch: (url, init) => {
      if (url.endsWith("/ideal-roles.json")) {
        f.idealSeen.push({ url, init });
        if (f.ideal instanceof Error) return Promise.reject(f.ideal);
        const ok = f.ideal !== null;
        return Promise.resolve({
          ok,
          status: ok ? 200 : 404,
          json: () =>
            ok && typeof f.ideal === "object"
              ? Promise.resolve(f.ideal)
              : Promise.reject(new SyntaxError("not JSON")),
        });
      }
      f.seen.push({ url, init });
      const q = JSON.parse(init.body).query;
      // `status` is the HTTP status of every answer (fetch resolves on any).
      const reply = (body) =>
        Promise.resolve({
          ok: f.status >= 200 && f.status < 300,
          status: f.status,
          json: () =>
            typeof body === "string"
              ? Promise.reject(new SyntaxError("not JSON"))
              : Promise.resolve(body),
        });
      if (q.includes("_meta")) {
        return f.meta === null
          ? Promise.reject(new TypeError("Failed to fetch"))
          : reply(f.meta);
      }
      if (q.includes("__type") && q.includes("ConfigBinding")) {
        return f.registrySchema === null
          ? Promise.reject(new TypeError("Failed to fetch"))
          : reply(f.registrySchema);
      }
      if (q.includes("__type")) {
        return f.schema === null
          ? Promise.reject(new TypeError("Failed to fetch"))
          : reply(f.schema);
      }
      const entity = /page: (\w+)\(/.exec(q)[1];
      if (entity in f.raw) return reply(f.raw[entity]);
      const cursor = /id_gt: "([^"]*)"/.exec(q)[1];
      const first = Number(/first: (\d+)/.exec(q)[1]);
      if (!(entity in f.rows)) {
        throw new Error("fake subgraph: unknown entity " + entity);
      }
      const page = f.rows[entity]
        .filter((r) => r.id > cursor)
        .filter((r) => !q.includes("held: true") || r.held !== false)
        .sort((a, b) => (a.id < b.id ? -1 : 1))
        .slice(0, first);
      return reply({ data: { page } });
    },
  };
  return f;
}

async function rbacPanelLive(f, data = rbacData([rbacNet()])) {
  const { box, done } = deploymentsRender(data, f.io);
  await done;
  return collect(box, "tabpanel").find((p) => p.dataset.tab === "rbac");
}
// The network banners: the timelock section's own banners are not one.
const rbacBanners = (p) =>
  collect(p, "own-verify").filter((b) => !/\btl-/.test(b.className));

Deno.test("deployments: role changes get their own tab beside the untouched grants view", async () => {
  const f = fakeSubgraph();
  const { box, done } = deploymentsRender(rbacData([rbacNet()]), f.io);
  await done;
  const tabs = collect(box, "subnav-btn").map((b) => b.textContent);
  assert(tabs.includes("Role changes"), "tab listed, got " + tabs.join(","));
  const byTab = Object.fromEntries(
    collect(box, "tabpanel").map((p) => [p.dataset.tab, p]),
  );
  assert(
    textOf(byTab.contracts).includes("Authoriser role grants"),
    "grants view kept on the contracts tab",
  );
  assert(
    !textOf(byTab.rbac).includes("Authoriser role grants"),
    "and not moved",
  );
});

Deno.test("deployments: each network reads as loading until its live answer lands", async () => {
  const f = fakeSubgraph();
  const { box, done } = deploymentsRender(rbacData([rbacNet()]), f.io);
  const p = collect(box, "tabpanel").find((x) => x.dataset.tab === "rbac");
  assert(
    textOf(p).includes("Reading the index"),
    "loading said before the read settles: " + textOf(p),
  );
  assert(collect(p, "rbac-chg").length === 0, "no rows while loading");
  await done;
  assert(!textOf(p).includes("Reading the index"), "loading replaced");
  assert(rbacBanners(p).length === 1, "one banner per network, not two");
});

Deno.test("deployments: a current index lists holders and changes per contract, read live", async () => {
  const f = fakeSubgraph();
  const p = await rbacPanelLive(f);
  const text = textOf(p);
  assert(text.includes("Read live from"), "says the read is live: " + text);
  assert(
    text.includes("ethereum (mainnet)"),
    "chain and subgraph network named",
  );
  assert(collect(p, "own-verify-ok").length === 1, "green banner");
  assert(
    text.includes("block 900") && text.includes("12s old"),
    "position and age stated",
  );
  // Every request is a JSON POST to the scan's endpoint for the network.
  for (const s of f.seen) {
    assert(s.url === RBAC_URL, "posted to the endpoint, got " + s.url);
    assert(s.init.method === "POST", "POST");
    assert(
      s.init.headers["content-type"] === "application/json",
      "json content type",
    );
  }
  // Contracts sorted by kind: the authoriser before the timelock.
  const heads = collect(p, "rbac-contract").map((h) => textOf(h));
  assert(
    heads.length === 2 && heads[0].includes("Authoriser") &&
      heads[0].includes(RBAC_AUTH) && heads[1].includes("Governance timelock"),
    "contracts grouped and ordered: " + heads.join(" | "),
  );
  const roles = collect(p, "own-role").map((r) => r.textContent);
  assert(roles[0] === "DEPOSIT", "named role first: " + roles.join(","));
  assert(roles[1] === RBAC_HASH, "unnamed role after it, by its hash");
  assert(text.includes("admin: DEPOSIT_ADMIN"), "admin role named");
  assert(text.includes("admin: DEFAULT_ADMIN_ROLE"), "zero admin named");
  assert(text.includes("no current holder"), "an unheld role says so");
  const held = collect(p, "own-row").filter((r) =>
    !r.className.includes("rbac-chg")
  );
  const holders = collect(held[0], "own-addr").map((a) => a.textContent);
  assert(holders.join(",") === "0xaa,0xbb", "holders sorted: " + holders);
  const chips = collect(p, "own-chip").map((c) => c.textContent);
  assert(
    chips.join(",") === "2 held,0 held,revoked,granted,granted,0 held",
    "chips in order, got " + chips.join(","),
  );
  // Newest first by (block, logIndex); the unattributed grant is dropped.
  const rows = collect(p, "rbac-chg").map((r) => textOf(r));
  assert(rows.length === 3, "three attributed changes, got " + rows.length);
  assert(
    rows[0].includes("block 12") && rows[0].includes("0xcc") &&
      rows[1].includes("0xaa") && rows[2].includes("block 10"),
    "newest first: " + rows.join(" | "),
  );
  assert(!textOf(p).includes("0xdd"), "orphan grant not shown");
  assert(
    rows[0].includes("from ") && rows[1].includes("to "),
    "revoke is from, grant is to",
  );
  assert(text.includes("3 grant/revoke events"), "count stated");
  const tx = collect(p, "own-verify-src").find((a) => a.textContent === "tx");
  assert(
    tx && tx.href === "https://etherscan.io/tx/0xt1",
    "tx links to the chain's explorer, got " + (tx && tx.href),
  );
});

Deno.test("deployments: contracts order by kind then address, changes by block then log", async () => {
  const f = fakeSubgraph();
  const low = "0x1111111111111111111111111111111111111111";
  const high = "0xffffffffffffffffffffffffffffffffffffffff";
  const role = (id, c, kind) => ({
    id,
    role: ROLE_IDS.DEPOSIT,
    admin: RBAC_ZERO,
    contract: { id: c, kind, firstIndexedBlock: "1" },
  });
  // The pager reads in row-id order, chosen here so that neither the read
  // order nor the address order is the kind order, and the two authorisers
  // arrive in the reverse of their address order.
  f.rows.roles = [
    role("r1", high, "AUTHORIZER"),
    role("r2", low, "GOVERNANCE_TIMELOCK"),
    role("r3", RBAC_AUTH, "AUTHORIZER"),
  ];
  f.rows.roleHolders = [];
  const g = (id, block, log) => ({
    id,
    account: "0x" + id,
    granted: true,
    sender: "0x5a",
    blockNumber: String(block),
    blockTimestamp: "1",
    transactionHash: "0xt",
    logIndex: String(log),
    role: { id: "r3" },
  });
  // A later block wins over a higher log index.
  f.rows.roleGrants = [g("a1", 10, 9), g("a2", 12, 3), g("a3", 12, 7)];
  const p = await rbacPanelLive(f);
  const heads = collect(p, "rbac-contract").map((h) => textOf(h));
  assert(
    heads.length === 3 && heads[0].includes(RBAC_AUTH) &&
      heads[1].includes(high) && heads[2].includes(low),
    "authorisers by address, then the timelock: " + heads.join(" | "),
  );
  const rows = collect(p, "rbac-chg").map((r) => textOf(r));
  assert(
    rows.length === 3 && rows[0].includes("0xa3") && rows[1].includes("0xa2") &&
      rows[2].includes("0xa1"),
    "newest block first, then newest log: " + rows.join(" | "),
  );
});

Deno.test("deployments: every entity read is pinned to the indexed block, holders to held", async () => {
  const f = fakeSubgraph();
  await rbacPanelLive(f);
  const qs = f.seen.map((s) => JSON.parse(s.init.body).query);
  // The schema probe is a question about the version, not the history, so
  // it is the one read with no block.
  const reads = qs.filter((q) => !q.includes("_meta") && !q.includes("__type"));
  assert(reads.length === 3, "three entity reads, got " + reads.length);
  for (const q of reads) {
    assert(q.includes("block: {number: 900}"), "unpinned read: " + q);
  }
  // A version with the timelock entities reads three more, at the same block.
  const t = fakeSubgraph({ schema: TL_SCHEMA });
  await rbacPanelLive(t);
  const tq = t.seen
    .map((s) => JSON.parse(s.init.body).query)
    .filter((q) => !q.includes("_meta") && !q.includes("__type"));
  assert(tq.length === 6, "six entity reads, got " + tq.length);
  for (const e of ["contracts(", "timelockOperations(", "timelockCalls("]) {
    const q = tq.find((x) => x.includes(e));
    assert(
      q && q.includes("block: {number: 900}"),
      "unpinned or missing read: " + e,
    );
  }
  assert(
    reads.find((q) => q.includes("roleHolders(")).includes("held: true"),
    "only current holders",
  );
});

Deno.test("deployments: the role pager follows the cursor and refuses an endless read", async () => {
  const f = fakeSubgraph();
  const base = f.rows.roleGrants[0];
  f.rows.roleGrants = Array.from({ length: 2005 }, (_, i) => ({
    ...base,
    id: "g" + String(i).padStart(6, "0"),
    blockNumber: String(i),
  }));
  const p = await rbacPanelLive(f);
  assert(collect(p, "rbac-chg").length === 2005, "every row, once");
  const grantReads = f.seen.filter((s) => s.init.body.includes("roleGrants("));
  assert(grantReads.length === 3, "two full pages and a short one");

  // An index that never ends a page is abandoned, not shown short.
  const g = fakeSubgraph();
  g.rows.roleGrants = Array.from({ length: 1000 }, (_, i) => ({
    ...base,
    id: "g" + String(i).padStart(6, "0"),
  }));
  g.raw.roleGrants = { data: { page: g.rows.roleGrants } };
  const q = await rbacPanelLive(g);
  assert(collect(q, "rbac-chg").length === 0, "no rows from an endless read");
  assert(
    textOf(q).includes("more than 100000 rows"),
    "says why: " + textOf(q),
  );
});

Deno.test("deployments: every index state is stated, and no data never reads as no changes", async () => {
  const meta = (block, hasIndexingErrors) => ({
    data: { _meta: { block, hasIndexingErrors } },
  });
  const cases = [
    [
      { meta: { error: "subgraph name/version error" } },
      "No subgraph — nothing is published at st0x-rbac-mainnet/prod",
      "down",
      false,
    ],
    [{ meta: null }, "No readable answer", "down", false],
    [{ meta: "<html>" }, "No readable answer", "down", false],
    // A non-2xx answer was received: its status is the error, never
    // "unreachable", unless the body has a GraphQL shape of its own.
    [{ meta: "<html>", status: 502 }, "answered HTTP 502", "drift", false],
    [
      { meta: { message: "rate limited" }, status: 429 },
      "answered HTTP 429",
      "drift",
      false,
    ],
    [{ meta: 5, status: 500 }, "answered HTTP 500", "drift", false],
    [
      { meta: { errors: [{ message: "bad query" }] }, status: 400 },
      "bad query",
      "drift",
      false,
    ],
    [
      { meta: { error: "subgraph name/version error" }, status: 404 },
      "No subgraph — nothing is published at st0x-rbac-mainnet/prod",
      "down",
      false,
    ],
    [
      { meta: { errors: [{ message: "has not started syncing yet" }] } },
      "has not started syncing yet",
      "drift",
      false,
    ],
    [
      { meta: { error: "path project error" } },
      "path project error",
      "drift",
      false,
    ],
    [{ meta: { data: null } }, "carried no data", "drift", false],
    [{ meta: 5 }, "not a JSON object", "drift", false],
    [{ meta: { errors: "flat" } }, '"flat"', "drift", false],
    [{ meta: { errors: [{ code: 1 }] } }, '{"code":1}', "drift", false],
    [{ meta: { error: { code: 2 } } }, '{"code":2}', "drift", false],
    [
      { meta: meta({ number: 900, timestamp: RBAC_TS }, "no") },
      "without a block position",
      "drift",
      false,
    ],
    [
      { meta: meta(null, false) },
      "without a block position",
      "drift",
      false,
    ],
    [
      { meta: meta({ number: 900, timestamp: RBAC_TS }, true) },
      "Indexing errors",
      "drift",
      true,
    ],
    [
      { now: (RBAC_TS + 601) * 1000 },
      "10m old. History below is complete only up to block 900",
      "roll",
      true,
    ],
    [{ now: (RBAC_TS + 7200) * 1000 }, "2h old", "roll", true],
    [{ now: (RBAC_TS + 119) * 1000 }, "119s old", "ok", true],
    [{ now: RBAC_TS * 1000 }, "0s old", "ok", true],
    [{ now: (RBAC_TS + 120) * 1000 }, "2m old", "ok", true],
    [{ now: (RBAC_TS + 7199) * 1000 }, "120m old", "roll", true],
    [
      { meta: meta({ number: "900", timestamp: null }, false) },
      "of unknown age. History below is complete only up to block 900",
      "roll",
      true,
    ],
    [{ now: (RBAC_TS + 600) * 1000 }, "current", "ok", true],
    // A browser clock behind the chain is not an index behind it.
    [
      { now: (RBAC_TS - 60) * 1000 },
      "ahead of this browser's clock",
      "ok",
      true,
    ],
    [
      { raw: { roleHolders: { errors: [{ message: "boom" }] } } },
      "roleHolders: boom",
      "drift",
      false,
    ],
    [
      { raw: { roles: { error: "subgraph name/version error" } } },
      "roles: the subgraph vanished mid-read",
      "drift",
      false,
    ],
    [
      { raw: { roleGrants: { data: {} } } },
      "roleGrants: the response carried no rows",
      "drift",
      false,
    ],
    [
      { raw: { roles: "<html>" } },
      "roles: no readable answer mid-read",
      "drift",
      false,
    ],
  ];
  for (const [over, phrase, cls, rows] of cases) {
    const label = JSON.stringify(over);
    const p = await rbacPanelLive(fakeSubgraph(over));
    const banners = rbacBanners(p);
    assert(banners.length === 1, label + ": one banner");
    assert(
      banners[0].className.includes("own-verify-" + cls),
      label + ": class " + banners[0].className,
    );
    assert(
      textOf(banners[0]).includes(phrase),
      label + ": says " + textOf(banners[0]),
    );
    assert(
      textOf(banners[0]).includes(RBAC_URL),
      label + ": names the endpoint",
    );
    if (rows) {
      assert(collect(p, "rbac-chg").length === 3, label + ": keeps its rows");
    } else {
      assert(
        textOf(banners[0]).includes("not the same as no changes"),
        label + ": no data is not no changes",
      );
      assert(collect(p, "rbac-chg").length === 0, label + ": no rows");
      assert(
        collect(p, "rbac-contract").length === 0,
        label + ": no contracts",
      );
    }
  }
});

Deno.test("deployments: an absent or unreadable role-history location says so", async () => {
  const f = fakeSubgraph();
  let p = await rbacPanelLive(f, grantsData());
  assert(
    textOf(p).includes("Role history unavailable"),
    "absent key: " + textOf(p),
  );
  p = await rbacPanelLive(
    f,
    rbacData(undefined, {
      unavailable: "could not read subgraph/networks.json",
    }),
  );
  assert(
    textOf(p).includes("could not read subgraph/networks.json"),
    "reason named: " + textOf(p),
  );
  p = await rbacPanelLive(f, rbacData([]));
  assert(textOf(p).includes("declares no networks"), "empty list stated");
  assert(f.seen.length === 0, "nothing fetched for no location");
  // A network whose endpoint is not https is never fetched.
  p = await rbacPanelLive(f, rbacData([rbacNet({ endpoint: "http://x" })]));
  assert(textOf(p).includes("no https endpoint"), "refused: " + textOf(p));
  assert(f.seen.length === 0, "and not fetched");
  // An index with no role rows at all says so rather than listing nothing.
  const empty = fakeSubgraph();
  empty.rows = { roles: [], roleHolders: [], roleGrants: [] };
  p = await rbacPanelLive(empty);
  assert(textOf(p).includes("no role events"), "empty index stated");
  // The owners-unavailable path renders this tab too, and waits on it.
  p = await rbacPanelLive(fakeSubgraph(), {
    ...rbacData([rbacNet()]),
    deploymentOwners: null,
  });
  assert(
    collect(p, "rbac-chg").length === 3,
    "rendered on the early-return path",
  );
});

Deno.test("deployments: networks read independently, unnamed chains stay unlinked", async () => {
  // One network down does not take the others with it, and a network with no
  // chain mapping shows its addresses as text rather than a wrong-chain link.
  const f = fakeSubgraph();
  const io = {
    now: f.io.now,
    fetch: (url, init) =>
      url.includes("down")
        ? Promise.reject(new TypeError("Failed to fetch"))
        : f.io.fetch(url, init),
  };
  const { box, done } = deploymentsRender(
    rbacData([
      rbacNet({ network: "base", chain: "base", endpoint: "https://down/gn" }),
      rbacNet({ network: "arbitrum-one", chain: null }),
    ]),
    io,
  );
  await done;
  const p = collect(box, "tabpanel").find((x) => x.dataset.tab === "rbac");
  const banners = rbacBanners(p);
  assert(banners.length === 2, "one banner each");
  assert(textOf(banners[0]).includes("No readable answer"), "first down");
  assert(collect(banners[1].parent, "rbac-chg").length === 3, "second read");
  assert(textOf(p).includes("arbitrum-one"), "unmapped network titled by name");
  const auth = collect(p, "own-addr").find((a) => a.textContent === RBAC_AUTH);
  assert(auth && auth.tagName === "span", "unlinked on an unknown chain");
});

Deno.test("deployments: role-change strings land as text", async () => {
  const f = fakeSubgraph();
  f.rows.roleGrants[0].account = XSS_IMG;
  f.rows.roles[1].contract.kind = XSS_IMG;
  const bad = fakeSubgraph({ meta: { error: XSS_IMG } });
  const io = {
    now: f.io.now,
    fetch: (url, init) =>
      url.includes("bad") ? bad.io.fetch(url, init) : f.io.fetch(url, init),
  };
  const { box, done } = deploymentsRender(
    rbacData([rbacNet(), rbacNet({ endpoint: "https://bad/gn" })], {
      subgraph: XSS_IMG,
    }),
    io,
  );
  await done;
  const p = collect(box, "tabpanel").find((x) => x.dataset.tab === "rbac");
  assert(textOf(p).includes(XSS_IMG), "payload rendered as text");
  assert(tags(p, "img").length === 0, "no element created from it");
});

// ---- deployments.html: role changes name every address the deploy repo pins ----

// The scan's per-chain pin table: on Ethereum the authoriser, holder 0xaa and
// the timelock are pinned; holder 0xbb, revoked 0xcc and sender 0x5a are not.
// Base pins the authoriser's address as something else entirely.
const pinned = (name, contract) => ({ name, contract, path: "src/lib/L.sol" });
const RBAC_PINS = {
  ethereum: {
    [RBAC_AUTH]: [
      pinned("STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM", "LibProdDeployV4"),
      pinned("ALIAS_OF_IT", "LibAuthoriserInvariants"),
    ],
    [RBAC_TL]: [
      pinned("STOX_GOVERNANCE_TIMELOCK_ETHEREUM", "LibTimelockInvariants"),
    ],
    "0xaa": [pinned("GRANTEE_SERVICE_AA", "LibAuthoriserInvariants")],
  },
  base: { [RBAC_AUTH]: [pinned("SOMETHING_ON_BASE", "LibX")] },
};
const whoOf = (node) =>
  collect(node, "rbac-who").map((w) => {
    const n = collect(w, "rbac-name")[0];
    const a = collect(w, "own-addr")[0];
    return {
      name: n ? n.textContent : null,
      title: n ? n.title : null,
      unknown: collect(w, "rbac-unknown").length === 1,
      hex: a ? a.textContent : null,
      href: a ? a.href : null,
    };
  });

Deno.test("deployments: role changes name pinned addresses and keep the hex link", async () => {
  const p = await rbacPanelLive(
    fakeSubgraph(),
    rbacData([rbacNet()], { addressNames: RBAC_PINS, addressNamesUnread: [] }),
  );
  const heads = collect(p, "rbac-contract").map(whoOf);
  assert(heads.length === 2, "two contracts");
  const [auth, tl] = heads.map((h) => h[0]);
  assert(
    auth.name === "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM" && !auth.unknown,
    "authoriser header named by its chain's pin: " + JSON.stringify(auth),
  );
  assert(
    auth.hex === RBAC_AUTH &&
      auth.href === "https://etherscan.io/address/" + RBAC_AUTH,
    "hex kept as the explorer link",
  );
  assert(
    auth.title ===
      "LibProdDeployV4.STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM\nLibAuthoriserInvariants.ALIAS_OF_IT",
    "every pinning constant in the tooltip, qualified: " + auth.title,
  );
  assert(
    tl.name === "STOX_GOVERNANCE_TIMELOCK_ETHEREUM",
    "timelock header named",
  );
  // Holder rows: 0xaa is pinned, 0xbb is not.
  const held = collect(p, "own-row").filter((r) =>
    !r.className.includes("rbac-chg")
  );
  const holders = whoOf(held[0]);
  assert(
    holders.length === 2 && holders[0].hex === "0xaa" &&
      holders[0].name === "GRANTEE_SERVICE_AA" && !holders[0].unknown,
    "pinned holder named: " + JSON.stringify(holders),
  );
  assert(
    holders[1].hex === "0xbb" && holders[1].name === null && holders[1].unknown,
    "unpinned holder marked unrecognised: " + JSON.stringify(holders[1]),
  );
  // History rows: the account and the sender are both looked up.
  const rows = collect(p, "rbac-chg").map(whoOf);
  assert(
    rows[0].length === 2 && rows[0][0].hex === "0xcc" && rows[0][0].unknown &&
      rows[0][1].hex === "0x5a" && rows[0][1].unknown,
    "revoked account and sender both unrecognised: " + JSON.stringify(rows[0]),
  );
  assert(
    rows[1][0].name === "GRANTEE_SERVICE_AA" && rows[1][1].unknown,
    "granted account named, sender unrecognised: " + JSON.stringify(rows[1]),
  );
  assert(
    textOf(p).includes("An address no constant pins is marked unrecognised"),
    "the marking is explained",
  );
  assert(
    collect(p, "rbac-names-gap").length === 0,
    "no gap when all were read",
  );
});

Deno.test("deployments: an address is named by its own chain's pins only", async () => {
  const f = fakeSubgraph();
  const p = await rbacPanelLive(
    f,
    rbacData(
      [
        rbacNet({ network: "base", chain: "base" }),
        rbacNet({ network: "bsc", chain: "bsc" }),
      ],
      { addressNames: RBAC_PINS },
    ),
  );
  const nets = collect(p, "rbac-net");
  const baseAuth = whoOf(collect(nets[0], "rbac-contract")[0])[0];
  assert(
    baseAuth.name === "SOMETHING_ON_BASE",
    "base's own name, not ethereum's",
  );
  const baseTl = whoOf(collect(nets[0], "rbac-contract")[1])[0];
  assert(baseTl.unknown, "ethereum's timelock pin does not name it on base");
  // A chain the table has no entry for pins nothing: everything stands out.
  const bsc = collect(nets[1], "rbac-who").map(whoOf).flat();
  const all = collect(nets[1], "rbac-who").length;
  assert(
    all > 0 && collect(nets[1], "rbac-unknown").length === all,
    "every bsc address unrecognised: " + JSON.stringify(bsc),
  );
});

Deno.test("deployments: an upper-case address still finds its lower-case pin", async () => {
  const f = fakeSubgraph();
  f.rows.roleGrants[0].account = "0xAA";
  const p = await rbacPanelLive(
    f,
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const row = collect(p, "rbac-chg").map(whoOf)[1];
  assert(
    row[0].hex === "0xAA" && row[0].name === "GRANTEE_SERVICE_AA",
    JSON.stringify(row),
  );
});

Deno.test("deployments: no pin table marks nothing unrecognised and says names are missing", async () => {
  const p = await rbacPanelLive(fakeSubgraph());
  assert(
    collect(p, "rbac-unknown").length === 0,
    "nothing marked without a table",
  );
  assert(collect(p, "rbac-name").length === 0, "nothing named");
  assert(collect(p, "rbac-who").length > 0, "addresses still rendered");
  assert(
    textOf(p).includes("no address-name table"),
    "the missing names are stated: " + textOf(p),
  );
});

Deno.test("deployments: a network with no chain is not looked up in the pin table", async () => {
  const p = await rbacPanelLive(
    fakeSubgraph(),
    rbacData([rbacNet({ network: "arbitrum-one", chain: null })], {
      addressNames: RBAC_PINS,
    }),
  );
  assert(collect(p, "rbac-who").length > 0, "addresses rendered");
  assert(
    collect(p, "rbac-unknown").length === 0,
    "nothing marked on an unknown chain",
  );
  assert(
    collect(p, "rbac-name").length === 0,
    "nothing named on an unknown chain",
  );
});

Deno.test("deployments: unread pin sources are named, and inherited keys are not pins", async () => {
  const f = fakeSubgraph();
  // An address spelled like an Object.prototype member must not resolve.
  f.rows.roleGrants[0].account = "constructor";
  const p = await rbacPanelLive(
    f,
    rbacData([rbacNet()], {
      addressNames: RBAC_PINS,
      addressNamesUnread: ["src/lib/LibGone.sol", 7],
    }),
  );
  const gap = collect(p, "rbac-names-gap");
  assert(
    gap.length === 1 && gap[0].textContent.includes("src/lib/LibGone.sol") &&
      !gap[0].textContent.includes("7"),
    "the unread path is named: " + (gap[0] && gap[0].textContent),
  );
  const row = collect(p, "rbac-chg").map(whoOf)[1];
  assert(row[0].hex === "constructor" && row[0].unknown, JSON.stringify(row));
  // An inherited table or entry is not the scan's, even when it has the
  // right shape: a polluted prototype must not name anything.
  Object.prototype["0xbb"] = [pinned("POLLUTED_HOLDER", "LibX")];
  try {
    const q = await rbacPanelLive(
      fakeSubgraph(),
      rbacData([rbacNet()], { addressNames: RBAC_PINS }),
    );
    const held = collect(q, "own-row").filter((r) =>
      !r.className.includes("rbac-chg")
    );
    const bb = whoOf(held[0])[1];
    assert(
      bb.hex === "0xbb" && bb.unknown && bb.name === null,
      "inherited entry named: " + JSON.stringify(bb),
    );
  } finally {
    delete Object.prototype["0xbb"];
  }
  Object.prototype.bsc = { [RBAC_AUTH]: [pinned("POLLUTED_CHAIN", "LibX")] };
  try {
    const q = await rbacPanelLive(
      fakeSubgraph(),
      rbacData([rbacNet({ network: "bsc", chain: "bsc" })], {
        addressNames: RBAC_PINS,
      }),
    );
    const auth = whoOf(collect(q, "rbac-contract")[0])[0];
    assert(
      auth.unknown && auth.name === null,
      "inherited chain table named: " + JSON.stringify(auth),
    );
  } finally {
    delete Object.prototype.bsc;
  }
});

Deno.test("deployments: malformed pin entries are skipped, never shown as a name", async () => {
  const p = await rbacPanelLive(
    fakeSubgraph(),
    rbacData([rbacNet()], {
      addressNames: {
        ethereum: {
          "0xaa": [
            null,
            { name: "" },
            { name: 7 },
            { contract: "LibX" },
            pinned("REAL_AA", "LibA"),
          ],
          "0xbb": [null, {}, { name: "" }],
        },
      },
    }),
  );
  const held = collect(p, "own-row").filter((r) =>
    !r.className.includes("rbac-chg")
  );
  const [aa, bb] = whoOf(held[0]);
  assert(
    aa.name === "REAL_AA" && aa.title === "LibA.REAL_AA" && !aa.unknown,
    "first well-formed entry names it, alone in the tooltip: " +
      JSON.stringify(aa),
  );
  assert(
    bb.unknown && bb.name === null,
    "no well-formed entry is unrecognised: " + JSON.stringify(bb),
  );
});

Deno.test("deployments: pinned names land as text", async () => {
  const p = await rbacPanelLive(
    fakeSubgraph(),
    rbacData([rbacNet()], {
      addressNames: { ethereum: { "0xaa": [pinned(XSS_IMG, XSS_IMG)] } },
    }),
  );
  assert(textOf(p).includes(XSS_IMG), "payload rendered as text");
  assert(tags(p, "img").length === 0, "no element created from it");
});

// ---- deployments.html: roles held now, per address ----

async function heldPanelLive(io, data) {
  const { box, done } = deploymentsRender(data, io);
  await done;
  return collect(box, "tabpanel").find((p) => p.dataset.tab === "held");
}
const heldGroups = (p) =>
  collect(p, "rbac-held").map((g) => ({
    ...whoOf(collect(g, "rbac-contract")[0])[0],
    rows: collect(g, "own-row").map((r) => ({
      net: collect(r, "own-role")[0].textContent,
      who: whoOf(r),
      roles: collect(r, "viol-role").map((x) => x.textContent),
      text: textOf(r),
    })),
    none: collect(g, "rbac-held-none").map((n) => n.textContent)[0] ?? null,
  }));
const heldNets = (p) =>
  collect(collect(p, "rbac-held-nets")[0], "own-row").map((r) => ({
    net: collect(r, "own-role")[0].textContent,
    chip: collect(r, "own-chip")[0].textContent,
    detail: collect(r, "own-note")[0].textContent,
  }));

Deno.test("deployments: roles held groups current holders by address, then network and contract", async () => {
  const f = fakeSubgraph();
  const p = await heldPanelLive(
    f.io,
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const groups = heldGroups(p);
  assert(
    groups.map((g) => g.hex).join(",") ===
      ["0xbb", "0xaa", RBAC_TL, RBAC_AUTH].join(","),
    "unrecognised holder, pinned holder, then pinned addresses holding nothing: " +
      groups.map((g) => g.hex).join(","),
  );
  const [bb, aa, tl, auth] = groups;
  assert(
    bb.unknown && bb.name === null && bb.rows.length === 1,
    "unpinned holder listed, marked: " + JSON.stringify(bb),
  );
  assert(
    aa.name === "GRANTEE_SERVICE_AA" && !aa.unknown && aa.none === null,
    "pinned holder named by its pin: " + JSON.stringify(aa),
  );
  assert(aa.rows.length === 1, "one network row");
  const row = aa.rows[0];
  assert(row.net === "ethereum (mainnet)", "network named: " + row.net);
  assert(
    row.who[0].hex === "0xaa" && row.who[0].name === "GRANTEE_SERVICE_AA" &&
      row.who[1].hex === RBAC_AUTH &&
      row.who[1].name === "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM" &&
      row.who[1].href === "https://etherscan.io/address/" + RBAC_AUTH,
    "holder and contract labelled for the chain: " + JSON.stringify(row.who),
  );
  assert(row.text.includes("Authoriser"), "contract kind: " + row.text);
  assert(row.roles.join(",") === "DEPOSIT", "role named: " + row.roles);
  assert(
    tl.name === "STOX_GOVERNANCE_TIMELOCK_ETHEREUM" &&
      tl.none === "Holds no role on any network." && tl.rows.length === 0,
    "pinned address holding nothing says so: " + JSON.stringify(tl),
  );
  assert(
    auth.title ===
      "ethereum: LibProdDeployV4.STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM\n" +
        "ethereum: LibAuthoriserInvariants.ALIAS_OF_IT\nbase: LibX.SOMETHING_ON_BASE",
    "every chain's pins in the tooltip: " + auth.title,
  );
  assert(
    f.seen.filter((s) => s.init.body.includes("roleHolders(")).length === 1,
    "both tabs render from one holder read",
  );
});

Deno.test("deployments: address(0) as a holder is named, not marked unrecognised", async () => {
  const f = fakeSubgraph();
  const zero = "0x" + "0".repeat(40);
  f.rows.roleHolders.push({
    id: "h0",
    account: zero,
    grantedAtBlock: "11",
    grantedAtTimestamp: "1791883011",
    role: { id: RBAC_TL + ROLE_IDS.DEPOSIT.slice(2) },
  });
  const p = await heldPanelLive(
    f.io,
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const groups = heldGroups(p);
  assert(
    groups.map((g) => g.hex).join(",") ===
      ["0xbb", zero, "0xaa", RBAC_TL, RBAC_AUTH].join(","),
    "address(0) ranks with named holders, not unrecognised ones: " +
      groups.map((g) => g.hex).join(","),
  );
  const z = groups[1];
  assert(
    z.name === "address(0)" && !z.unknown && z.rows.length === 1,
    "group header named: " + JSON.stringify(z),
  );
  assert(
    z.rows[0].who[0].name === "address(0)" && !z.rows[0].who[0].unknown,
    "network row named: " + JSON.stringify(z.rows[0].who),
  );
});
Deno.test("deployments: roles held is current state — a revoke ends a holding, a re-grant restores it", async () => {
  const f = fakeSubgraph();
  const deposit = { id: RBAC_AUTH + ROLE_IDS.DEPOSIT.slice(2) };
  const ev = (id, account, granted, block) => ({
    id,
    account,
    granted,
    sender: "0x5a",
    blockNumber: String(block),
    blockTimestamp: String(1791883000 + block),
    transactionHash: "0xt" + id,
    logIndex: "0",
    role: deposit,
  });
  f.rows.roleGrants.push(
    ev("r1", "0xcc", true, 20),
    ev("r2", "0xcc", false, 21),
    ev("r3", "0xdd", true, 20),
    ev("r4", "0xdd", false, 21),
    ev("r5", "0xdd", true, 22),
  );
  f.rows.roleHolders.push(
    {
      id: "hc",
      account: "0xcc",
      held: false,
      grantedAtBlock: "20",
      grantedAtTimestamp: "1791883020",
      role: deposit,
    },
    {
      id: "hd",
      account: "0xdd",
      held: true,
      grantedAtBlock: "22",
      grantedAtTimestamp: "1791883022",
      role: deposit,
    },
  );
  const p = await heldPanelLive(
    f.io,
    rbacData([rbacNet()], {
      addressNames: {
        ethereum: {
          "0xcc": [pinned("REVOKED_CC", "LibA")],
          "0xdd": [pinned("REGRANTED_DD", "LibA")],
        },
      },
    }),
  );
  const groups = heldGroups(p);
  const cc = groups.find((g) => g.hex === "0xcc");
  const dd = groups.find((g) => g.hex === "0xdd");
  assert(
    cc && cc.rows.length === 0 && cc.none === "Holds no role on any network.",
    "granted then revoked is not held: " + JSON.stringify(cc),
  );
  assert(
    dd && dd.rows.length === 1 && dd.rows[0].roles.join(",") === "DEPOSIT",
    "revoked then granted again is held: " + JSON.stringify(dd),
  );
});

Deno.test("deployments: roles held names an address per chain, one group across networks", async () => {
  const eth = fakeSubgraph();
  const base = fakeSubgraph();
  base.rows.roleHolders[1].account = "0xAA";
  const io = {
    now: eth.io.now,
    fetch: (url, init) =>
      url.includes("base") ? base.io.fetch(url, init) : eth.io.fetch(url, init),
  };
  const p = await heldPanelLive(
    io,
    rbacData(
      [
        rbacNet({
          network: "base",
          chain: "base",
          endpoint: "https://base/gn",
        }),
        rbacNet(),
      ],
      { addressNames: RBAC_PINS },
    ),
  );
  const aa = heldGroups(p).filter((g) => g.hex === "0xaa");
  assert(aa.length === 1, "one group whatever the case: " + aa.length);
  const [onBase, onEth] = aa[0].rows;
  assert(
    onBase.net === "base" && onEth.net === "ethereum (mainnet)",
    "a row per network, in network order",
  );
  assert(
    onBase.who[0].unknown && onBase.who[0].hex === "0xaa" &&
      onBase.who[0].href === "https://basescan.org/address/0xaa",
    "unpinned on base, so unrecognised there: " + JSON.stringify(onBase.who),
  );
  assert(
    onBase.who[1].name === "SOMETHING_ON_BASE" &&
      onEth.who[1].name === "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM",
    "the contract named by each chain's own pin",
  );
  assert(
    onEth.who[0].name === "GRANTEE_SERVICE_AA" && !onEth.who[0].unknown,
    "named where it is pinned",
  );
});

Deno.test("deployments: roles held never claims nothing held on a network it could not read", async () => {
  const f = fakeSubgraph();
  const io = {
    now: f.io.now,
    fetch: (url, init) =>
      url.includes("down")
        ? Promise.reject(new TypeError("Failed to fetch"))
        : f.io.fetch(url, init),
  };
  const data = rbacData(
    [
      rbacNet({ network: "base", chain: "base", endpoint: "https://down/gn" }),
      rbacNet(),
    ],
    { addressNames: RBAC_PINS },
  );
  const { box, done } = deploymentsRender(data, io);
  const live = collect(box, "tabpanel").find((x) => x.dataset.tab === "held");
  assert(
    textOf(live).includes("Reading the indexes") &&
      collect(live, "rbac-held").length === 0,
    "nothing claimed before a read lands: " + textOf(live),
  );
  assert(
    heldNets(live).every((n) => n.chip === "reading"),
    "every network reading",
  );
  await done;
  assert(
    JSON.stringify(heldNets(live).map((n) => [n.net, n.chip])) ===
      JSON.stringify([["base", "not read"], ["ethereum (mainnet)", "current"]]),
    "each network's state: " + JSON.stringify(heldNets(live)),
  );
  const tl = heldGroups(live).find((g) => g.hex === RBAC_TL);
  assert(
    tl.none === "Holds no role on the networks read; base not read.",
    "the unread network is named: " + tl.none,
  );
  const dead = await heldPanelLive(
    NO_NETWORK,
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  assert(
    collect(dead, "rbac-held").length === 0 &&
      textOf(dead).includes("No index could be read"),
    "no group at all when no index was read: " + textOf(dead),
  );
});

Deno.test("deployments: roles held states each index's position", async () => {
  const cases = [
    [
      { now: (RBAC_TS + 7200) * 1000 },
      "syncing",
      "holdings as of block 900, 2h old",
    ],
    [
      {
        meta: {
          data: {
            _meta: {
              block: { number: 900, timestamp: RBAC_TS },
              hasIndexingErrors: true,
            },
          },
        },
      },
      "indexing errors",
      "holdings as of block 900, where the index stopped",
    ],
    [{}, "current", "indexed to block 900"],
  ];
  for (const [over, chip, detail] of cases) {
    const p = await heldPanelLive(
      fakeSubgraph(over).io,
      rbacData([rbacNet()], { addressNames: RBAC_PINS }),
    );
    const [n] = heldNets(p);
    assert(n.chip === chip, chip + ": " + JSON.stringify(n));
    assert(n.detail.startsWith(detail), chip + ": " + n.detail);
    assert(heldGroups(p).length === 4, chip + ": holdings still shown");
  }
});

Deno.test("deployments: roles held without a pin table lists holders unmarked", async () => {
  const p = await heldPanelLive(fakeSubgraph().io, rbacData([rbacNet()]));
  const groups = heldGroups(p);
  assert(
    groups.map((g) => g.hex).join(",") === "0xaa,0xbb",
    "holders only: " + groups.map((g) => g.hex).join(","),
  );
  assert(collect(p, "rbac-unknown").length === 0, "nothing marked");
  assert(
    textOf(p).includes("no address-name table"),
    "the missing names are stated",
  );
});

Deno.test("deployments: roles held says when the scan carries no subgraph", async () => {
  const f = fakeSubgraph();
  let p = await heldPanelLive(f.io, grantsData());
  assert(
    textOf(p).includes(
      "Roles held unavailable — this scan carries no RBAC subgraph location.",
    ),
    "absent: " + textOf(p),
  );
  p = await heldPanelLive(
    f.io,
    rbacData(undefined, {
      unavailable: "could not read subgraph/networks.json",
    }),
  );
  assert(
    textOf(p).includes("the scan could not read subgraph/networks.json"),
    "reason named: " + textOf(p),
  );
  p = await heldPanelLive(f.io, rbacData([]));
  assert(textOf(p).includes("declares no networks"), "no networks stated");
});

Deno.test("deployments: roles-held strings land as text", async () => {
  const f = fakeSubgraph();
  f.rows.roleHolders[0].account = XSS_IMG;
  f.rows.roles[1].contract.kind = XSS_SCRIPT;
  const data = rbacData([rbacNet()], {
    addressNames: { ethereum: { "0xaa": [pinned(XSS_SVG, XSS_ATTR)] } },
  });
  data.deploymentRbac.roleNames[ROLE_IDS.DEPOSIT] = XSS_ATTR;
  const p = await heldPanelLive(f.io, data);
  const text = textOf(p);
  for (const x of [XSS_IMG, XSS_SCRIPT, XSS_SVG, XSS_ATTR]) {
    assert(text.includes(x), "payload rendered as text: " + x);
  }
  for (const t of ["img", "script", "svg"]) {
    assert(tags(p, t).length === 0, "no " + t + " element created");
  }
});

// ---- deployments.html: holdings against st0x.deploy's ideal-roles.json ----

const IDEAL_KEEP = "0x" + "11".repeat(20);
const IDEAL_DROP = "0x" + "22".repeat(20);
const IDEAL_NEW = "0x" + "3c".repeat(20);
const IDEAL_OTHER = "0x" + "0a".repeat(20);
const IDEAL_EU = "0x" + "44".repeat(20);
const IDEAL_URL =
  "https://raw.githubusercontent.com/S01-Issuer/st0x.deploy/main/ideal-roles.json";
const idealPair = (role, grantee, roleName = null) => ({
  role,
  roleName,
  grantee,
  granteeName: "GRANTEE",
});
// DROP's DEPOSIT is expected on the timelock, not on the authoriser.
function idealFixture() {
  return {
    version: 1,
    chains: {
      "1": {
        network: "ethereum",
        contracts: [
          {
            address: RBAC_AUTH,
            name: "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM",
            kind: "AUTHORIZER",
            expected: [
              idealPair(ROLE_IDS.DEPOSIT, IDEAL_KEEP, "DEPOSIT"),
              idealPair(ROLE_IDS.WITHDRAW, IDEAL_NEW, "WITHDRAW"),
              idealPair(RBAC_HASH, IDEAL_KEEP),
            ],
          },
          {
            address: RBAC_TL,
            name: "STOX_GOVERNANCE_TIMELOCK_ETHEREUM",
            kind: "GOVERNANCE_TIMELOCK",
            expected: [
              idealPair(ROLE_IDS.DEPOSIT, IDEAL_DROP, "DEPOSIT"),
              idealPair(ROLE_IDS.DEPOSIT, IDEAL_OTHER, "DEPOSIT"),
            ],
          },
        ],
      },
    },
  };
}
// The authoriser's DEPOSIT is held by KEEP and DROP; the timelock's by OTHER and KEEP.
function idealSubgraph(over = {}) {
  const f = fakeSubgraph({ ideal: idealFixture(), ...over });
  const holder = (id, account, c) => ({
    id,
    account,
    grantedAtBlock: "10",
    grantedAtTimestamp: "1791883010",
    role: { id: c + ROLE_IDS.DEPOSIT.slice(2) },
  });
  f.rows.roleHolders = [
    holder("h1", IDEAL_KEEP, RBAC_AUTH),
    holder("h2", IDEAL_DROP, RBAC_AUTH),
    holder("h3", IDEAL_OTHER, RBAC_TL),
    holder("h4", IDEAL_KEEP, RBAC_TL),
  ];
  return f;
}
const offTags = (node) => collect(node, "rbac-tag").map((t) => t.textContent);
const roleRows = (p) =>
  collect(p, "own-row")
    .filter((r) => !r.className.includes("rbac-chg"))
    .map((r) => ({
      role: collect(r, "own-role")[0].textContent,
      off: r.className.split(" ").includes("rbac-off"),
      lines: collect(r, "grt-addr").map((l) =>
        [whoOf(l)[0].hex, ...offTags(l)].join(" ")
      ),
      chip: collect(r, "own-chip")[0].textContent,
    }));
const idealLines = (p) => collect(p, "rbac-ideal").map((l) => l.textContent);

Deno.test("deployments: role changes mark an expected holding not held to grant, a held one not expected as a warning", async () => {
  const p = await rbacPanelLive(idealSubgraph());
  assert(
    JSON.stringify(idealLines(p)) ===
      JSON.stringify(["Ideal roles: 3 to grant, 2 not expected."]),
    "the network's count: " + idealLines(p),
  );
  assert(
    collect(p, "rbac-ideal")[0].className.includes("rbac-ideal-off"),
    "a discrepancy reads as one",
  );
  const rows = roleRows(p);
  const want = [
    {
      role: "DEPOSIT",
      off: true,
      lines: [`${IDEAL_KEEP}`, `${IDEAL_DROP} not expected`],
      chip: "2 held",
    },
    {
      role: RBAC_HASH,
      off: true,
      lines: [`${IDEAL_KEEP} to grant`],
      chip: "0 held",
    },
    {
      role: "WITHDRAW",
      off: true,
      lines: [`${IDEAL_NEW} to grant`],
      chip: "0 held",
    },
    {
      role: "DEPOSIT",
      off: true,
      lines: [
        `${IDEAL_OTHER}`,
        `${IDEAL_KEEP} not expected`,
        `${IDEAL_DROP} to grant`,
      ],
      chip: "2 held",
    },
  ];
  assert(
    JSON.stringify(rows) === JSON.stringify(want),
    "authoriser rows then the timelock's, keyed by contract: " +
      JSON.stringify(rows),
  );
  const titles = Object.fromEntries(
    collect(p, "rbac-tag").map((t) => [t.textContent, t.title]),
  );
  assert(
    JSON.stringify(titles) ===
      JSON.stringify({
        "not expected":
          "ideal-roles.json does not expect this holding; it is held.",
        "to grant": "ideal-roles.json expects this holding; it is not held.",
      }),
    JSON.stringify(titles),
  );
});

Deno.test("deployments: an ideal role the file leaves unnamed takes the scan's name for it", async () => {
  const ideal = idealFixture();
  ideal.chains["1"].contracts[0].expected[1].roleName = null;
  const named = rbacData([rbacNet()]);
  named.deploymentRbac.roleNames[ROLE_IDS.WITHDRAW] = "WITHDRAW";
  let rows = roleRows(await rbacPanelLive(idealSubgraph({ ideal }), named));
  assert(
    rows.some((r) =>
      r.role === "WITHDRAW" && r.lines.join() === `${IDEAL_NEW} to grant`
    ),
    "the scan's name labels the to-grant role: " + JSON.stringify(rows),
  );
  rows = roleRows(await rbacPanelLive(idealSubgraph({ ideal })));
  assert(
    rows.some((r) =>
      r.role === ROLE_IDS.WITHDRAW && r.lines.join() === `${IDEAL_NEW} to grant`
    ),
    "with no name anywhere the role shows its id: " + JSON.stringify(rows),
  );
});

Deno.test("deployments: roles held lists every discrepancy first, a grantee to grant even where it holds nothing", async () => {
  const p = await heldPanelLive(idealSubgraph().io, rbacData([rbacNet()]));
  const groups = heldGroups(p);
  assert(
    groups.map((g) => g.hex).join(",") ===
      [IDEAL_KEEP, IDEAL_DROP, IDEAL_NEW, IDEAL_OTHER].join(","),
    "addresses off the ideal first: " + groups.map((g) => g.hex).join(","),
  );
  const row = (g) => {
    const r = collect(collect(p, "rbac-held")[g], "own-row")[0];
    return {
      off: r.className.split(" ").includes("rbac-off"),
      pairs: collect(r, "rbac-pair").map((x) =>
        collect(x, "viol-role")[0].textContent + ":" + offTags(x).join()
      ),
      roles: collect(r, "viol-role").map((x) => x.textContent),
      chips: collect(r, "own-chip").map((c) =>
        c.className.replace("own-chip own-chip-", "") + " " + c.textContent
      ),
    };
  };
  const want = [
    {
      off: true,
      pairs: [RBAC_HASH + ":to grant", "DEPOSIT:not expected"],
      roles: ["DEPOSIT", RBAC_HASH, "DEPOSIT"],
      chips: ["yes 2 held", "roll 1 to grant", "no 1 not expected"],
    },
    {
      off: true,
      pairs: ["DEPOSIT:not expected", "DEPOSIT:to grant"],
      roles: ["DEPOSIT", "DEPOSIT"],
      chips: ["yes 1 held", "roll 1 to grant", "no 1 not expected"],
    },
    {
      off: true,
      pairs: ["WITHDRAW:to grant"],
      roles: ["WITHDRAW"],
      chips: ["dim 0 held", "roll 1 to grant"],
    },
    { off: false, pairs: [], roles: ["DEPOSIT"], chips: ["yes 1 held"] },
  ];
  for (const [i, w] of want.entries()) {
    assert(
      JSON.stringify(row(i)) === JSON.stringify(w),
      groups[i].hex + ": " + JSON.stringify(row(i)),
    );
  }
  assert(
    collect(collect(p, "rbac-held-nets")[0], "rbac-ideal")[0].textContent ===
      "Ideal roles: 3 to grant, 2 not expected.",
    "each index's row carries its count",
  );
});

Deno.test("deployments: a contract ideal-roles.json does not declare expects nothing, and a forbidden list in it changes nothing", async () => {
  const f = idealSubgraph();
  f.ideal.chains["1"].contracts.pop();
  const p = await rbacPanelLive(f);
  assert(
    idealLines(p).join() === "Ideal roles: 2 to grant, 3 not expected.",
    idealLines(p),
  );
  const tl = roleRows(p)[3];
  assert(
    JSON.stringify(tl) ===
      JSON.stringify({
        role: "DEPOSIT",
        off: true,
        lines: [`${IDEAL_OTHER} not expected`, `${IDEAL_KEEP} not expected`],
        chip: "2 held",
      }),
    "every holder on the undeclared timelock is not expected: " +
      JSON.stringify(tl),
  );
  const held = heldGroups(
    await heldPanelLive(f.io, rbacData([rbacNet()])),
  ).map((x) => x.hex);
  assert(
    held.join() === [IDEAL_OTHER, IDEAL_KEEP, IDEAL_DROP, IDEAL_NEW].join(),
    "a holder whose only discrepancy is a warning is listed first: " + held,
  );
  const g = idealSubgraph();
  const auth = g.ideal.chains["1"].contracts[0];
  auth.forbidden = [idealPair(ROLE_IDS.DEPOSIT, IDEAL_KEEP, "DEPOSIT"), 7];
  const q = await rbacPanelLive(g);
  assert(
    idealLines(q).join() === "Ideal roles: 3 to grant, 2 not expected." &&
      roleRows(q)[0].lines.join() ===
        `${IDEAL_KEEP},${IDEAL_DROP} not expected`,
    "forbidden is not read: " + idealLines(q),
  );
});

Deno.test("deployments: ideal-roles.json is read once, from the deploy repo's main", async () => {
  const f = idealSubgraph();
  const { box, done } = deploymentsRender(
    rbacData([rbacNet(), rbacNet({ network: "base", chain: "base" })]),
    f.io,
  );
  await done;
  assert(
    f.idealSeen.length === 1 && f.idealSeen[0].url === IDEAL_URL,
    "one read: " + JSON.stringify(f.idealSeen),
  );
  for (const tab of ["rbac", "held"]) {
    const p = collect(box, "tabpanel").find((x) => x.dataset.tab === tab);
    const a = tags(p, "a").find((x) => x.textContent.includes("ideal-roles"));
    assert(
      a &&
        a.href ===
          "https://github.com/S01-Issuer/st0x.deploy/blob/main/ideal-roles.json",
      tab + ": the source is linked",
    );
    assert(
      textOf(p).includes(
        ": a role it expects that is not held is marked to grant, and a held role it does not expect, not expected.",
      ),
      tab + ": the marks are explained",
    );
  }
});

Deno.test("deployments: an unreadable ideal-roles.json marks nothing and says no discrepancy is not none", async () => {
  const bad = (edit) => {
    const v = idealFixture();
    edit(v, v.chains["1"].contracts[0]);
    return v;
  };
  const cases = [
    [null, "HTTP 404"],
    [new TypeError("Failed to fetch"), "no answer"],
    ["404: Not Found", "it is not JSON"],
    [[], "it is not a JSON object"],
    [bad((v) => (v.version = 2)), "it is version 2; this page reads version 1"],
    [bad((v) => (v.chains = [])), "it has no chains object"],
    [bad((v) => (v.chains["1"] = 7)), 'chains["1"] is not an object'],
    [bad((v) => delete v.chains["1"].network), 'chains["1"] names no network'],
    [
      bad((v) => (v.chains["2"] = { ...v.chains["1"] })),
      'chains["2"] repeats network ethereum',
    ],
    [
      bad((v) => (v.chains["1"].contracts = {})),
      'chains["1"] has no contracts list',
    ],
    [
      bad((v) => v.chains["1"].contracts.push(null)),
      'chains["1"].contracts[2] is not an object',
    ],
    [
      bad((_, c) => (c.address = "0x1234")),
      'chains["1"].contracts[0] has no address',
    ],
    [
      bad((_, c) => delete c.expected),
      'chains["1"].contracts[0] has no expected list',
    ],
    [
      bad((_, c) => (c.expected[1] = "x")),
      'chains["1"].contracts[0].expected[1] is not an object',
    ],
    [
      bad((_, c) => (c.expected[0].role = ROLE_IDS.DEPOSIT.slice(0, 65))),
      'chains["1"].contracts[0].expected[0] has no bytes32 role',
    ],
    [
      bad((_, c) => (c.expected[0].role = "0x" + "zz".repeat(32))),
      'chains["1"].contracts[0].expected[0] has no bytes32 role',
    ],
    [
      bad((_, c) => (c.expected[0].grantee = IDEAL_KEEP + "00")),
      'chains["1"].contracts[0].expected[0] has no grantee address',
    ],
  ];
  for (const [ideal, why] of cases) {
    const f = idealSubgraph({ ideal });
    const { box, done } = deploymentsRender(rbacData([rbacNet()]), f.io);
    await done;
    for (const tab of ["rbac", "held"]) {
      const p = collect(box, "tabpanel").find((x) => x.dataset.tab === tab);
      const note = collect(p, "rbac-ideal-gap").map((n) => n.textContent);
      assert(
        note.length === 1 &&
          note[0] ===
            `Discrepancies unknown — S01-Issuer/st0x.deploy's ideal-roles.json could not be read: ${why}. ` +
              "None is marked, which is not the same as none.",
        tab + " " + why + ": " + note,
      );
      assert(
        idealLines(p).join() ===
          "Ideal roles: not checked — ideal-roles.json could not be read.",
        tab + " " + why + ": " + idealLines(p),
      );
      assert(
        offTags(p).length === 0 && collect(p, "rbac-off").length === 0,
        tab + " " + why + ": nothing marked",
      );
    }
  }
});

Deno.test("deployments: ideal roles are matched per chain name, and hex in any case is read lower-case", async () => {
  const f = idealSubgraph();
  const up = (h) => "0x" + h.slice(2).toUpperCase();
  const auth = f.ideal.chains["1"].contracts[0];
  auth.address = up(RBAC_AUTH);
  auth.expected[0].role = up(ROLE_IDS.DEPOSIT);
  auth.expected[1].grantee = up(IDEAL_NEW);
  auth.expected[2].role = up(RBAC_HASH);
  f.ideal.chains["1"].contracts[1].expected[1].grantee = up(IDEAL_OTHER);
  const { box, done } = deploymentsRender(rbacData([rbacNet()]), f.io);
  await done;
  const tab = (t) => collect(box, "tabpanel").find((x) => x.dataset.tab === t);
  assert(
    idealLines(tab("rbac")).join() ===
      "Ideal roles: 3 to grant, 2 not expected.",
    "upper-case hex still matches: " + idealLines(tab("rbac")),
  );
  const withdraw = roleRows(tab("rbac")).find((r) => r.role === "WITHDRAW");
  assert(
    withdraw && withdraw.lines.join() === `${IDEAL_NEW} to grant`,
    "the grantee is shown lower-case: " + JSON.stringify(withdraw),
  );
  const keep = collect(tab("held"), "rbac-held")[0];
  assert(
    collect(keep, "rbac-pair").map((x) =>
      collect(x, "viol-role")[0].textContent
    )
      .join() === RBAC_HASH + ",DEPOSIT",
    "an unnamed role is shown lower-case",
  );
  const g = idealSubgraph();
  g.ideal.chains["1"].network = "mainnet";
  const p = await rbacPanelLive(g);
  assert(
    idealLines(p).join() ===
      "Ideal roles: not checked — ideal-roles.json declares nothing for this network.",
    "the subgraph's network name is not the chain: " + idealLines(p),
  );
  assert(
    collect(p, "rbac-ideal")[0].className.includes("rbac-ideal-gap"),
    "an undeclared network reads as a gap",
  );
  assert(
    collect(p, "rbac-ideal-gap").some((n) =>
      n.textContent ===
        "Not checked: ideal-roles.json declares mainnet (chain 1), which the subgraph does not index."
    ),
    "the stray declaration is named",
  );
  assert(offTags(p).length === 0, "nothing marked");
});

Deno.test("deployments: a declared contract the index holds no role for is named unchecked, not marked to grant", async () => {
  const f = idealSubgraph();
  const [auth, tl] = f.ideal.chains["1"].contracts;
  auth.expected = [
    auth.expected[0],
    idealPair(ROLE_IDS.DEPOSIT, IDEAL_DROP, "DEPOSIT"),
  ];
  tl.expected = [
    tl.expected[1],
    idealPair(ROLE_IDS.DEPOSIT, IDEAL_KEEP, "DEPOSIT"),
  ];
  f.ideal.chains["1"].contracts.push({
    address: IDEAL_EU,
    name: "STOX_EU_AUTHORISER",
    kind: "AUTHORIZER",
    expected: [idealPair(ROLE_IDS.DEPOSIT, IDEAL_NEW, "DEPOSIT")],
  });
  const p = await rbacPanelLive(f);
  assert(
    idealLines(p).join() ===
      "Ideal roles: held exactly as expected, on the contracts the index holds. " +
        "Not checked: STOX_EU_AUTHORISER — the index holds no role events for it.",
    idealLines(p),
  );
  assert(
    collect(p, "rbac-ideal")[0].className.includes("rbac-ideal-gap"),
    "a partial check reads as a gap",
  );
  assert(offTags(p).length === 0, "the unindexed grantee is not marked");
  f.ideal.chains["1"].contracts.pop();
  const q = await rbacPanelLive(f);
  assert(
    idealLines(q).join() === "Ideal roles: held exactly as expected.",
    idealLines(q),
  );
  assert(
    collect(q, "rbac-ideal")[0].className === "own-note rbac-ideal",
    "a whole match is not flagged",
  );
});

Deno.test("deployments: ideal roles wait for both reads, and never check an index that was not read", async () => {
  const f = idealSubgraph();
  let open;
  const gate = new Promise((r) => (open = r));
  const io = {
    now: f.io.now,
    fetch: (url, init) =>
      url === IDEAL_URL
        ? f.io.fetch(url, init)
        : gate.then(() => f.io.fetch(url, init)),
  };
  const { box, done } = deploymentsRender(rbacData([rbacNet()]), io);
  const panel = () =>
    collect(box, "tabpanel").find((x) => x.dataset.tab === "rbac");
  assert(
    idealLines(panel()).join() === "Ideal roles: reading ideal-roles.json…",
    idealLines(panel()),
  );
  const reading = "Reading S01-Issuer/st0x.deploy's ideal-roles.json…";
  assert(textOf(panel()).includes(reading), "the read is said to be under way");
  await new Promise((r) => setTimeout(r, 0));
  assert(
    idealLines(panel()).join() === "Ideal roles: waiting for the index…",
    idealLines(panel()),
  );
  open();
  await done;
  assert(
    idealLines(panel()).join() === "Ideal roles: 3 to grant, 2 not expected.",
    idealLines(panel()),
  );
  assert(!textOf(panel()).includes(reading), "and no longer once it lands");
  let release;
  const late = new Promise((r) => (release = r));
  const lateIo = {
    now: f.io.now,
    fetch: (url, init) =>
      url === IDEAL_URL
        ? late.then(() => f.io.fetch(url, init))
        : f.io.fetch(url, init),
  };
  const second = deploymentsRender(rbacData([rbacNet()]), lateIo);
  const tab = (t) =>
    collect(second.box, "tabpanel").find((x) => x.dataset.tab === t);
  for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 0));
  assert(
    roleRows(tab("rbac")).length === 3 &&
      idealLines(tab("rbac")).join() ===
        "Ideal roles: reading ideal-roles.json…",
    "the index lands first: " + idealLines(tab("rbac")),
  );
  assert(offTags(tab("held")).length === 0, "nothing marked before the ideal");
  release();
  await second.done;
  for (const t of ["rbac", "held"]) {
    assert(offTags(tab(t)).length === 5, t + ": marked once the ideal lands");
  }
  const dead = await rbacPanelLive(idealSubgraph({ meta: null }));
  assert(
    idealLines(dead).join() ===
      "Ideal roles: not checked — the index was not read.",
    idealLines(dead),
  );
  assert(offTags(dead).length === 0, "an unread index marks nothing");
});

Deno.test("deployments: ideal-roles.json strings land as text", async () => {
  const f = idealSubgraph();
  const auth = f.ideal.chains["1"].contracts[0];
  auth.expected[1].roleName = XSS_IMG;
  f.ideal.chains["1"].contracts.push({
    address: IDEAL_EU,
    name: XSS_SCRIPT,
    expected: [],
  });
  f.ideal.chains["8453"] = {
    network: XSS_SVG,
    contracts: [],
  };
  for (const tab of ["rbac", "held"]) {
    const { box, done } = deploymentsRender(rbacData([rbacNet()]), f.io);
    await done;
    const p = collect(box, "tabpanel").find((x) => x.dataset.tab === tab);
    const text = textOf(p);
    for (const x of [XSS_IMG, XSS_SCRIPT, XSS_SVG]) {
      assert(text.includes(x), tab + ": payload rendered as text: " + x);
    }
    for (const t of ["img", "script", "svg"]) {
      assert(tags(p, t).length === 0, tab + ": no " + t + " element created");
    }
  }
});

// ---- deployments.html: governance timelock operations ----

// A subgraph version that indexes `TimelockOperation`.
const TL_SCHEMA = { data: { t: { name: "TimelockOperation" } } };
const TL_NOW = RBAC_TS + 12; // the fake's default clock, in seconds
const TL_BEACON = "0x" + "be".repeat(20);
const TL_STRANGER = "0x" + "5e".repeat(20);
const TL_IMPL = "0x" + "11".repeat(20);
// keccak256("EXECUTOR_ROLE"), as `cast keccak EXECUTOR_ROLE` prints it.
const TL_EXECUTOR =
  "0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63";
const TL_ADDRESS_ZERO = "0x" + "0".repeat(40);
const tlWord = (hex) => hex.replace(/^0x/, "").padStart(64, "0");
const tlCallData = (sel, ...words) => sel + words.map(tlWord).join("");
const SEL = {
  grantRole: "0x2f2ff15d",
  upgradeTo: "0x3659cfe6",
  upgradeToAndCall: "0x4f1ef286",
  updateDelay: "0x64d62353",
  transferOwnership: "0xf2fde38b",
  renounceOwnership: "0x715018a6",
};
const tlOpRow = (id, status, readyAt, over = {}) => ({
  id,
  timelock: { id: RBAC_TL },
  operationId: "0x" + id.slice(2).padEnd(64, "0"),
  schedulingIndex: 0,
  status,
  callCount: 1,
  predecessor: RBAC_ZERO,
  salt: RBAC_ZERO,
  delay: "172800",
  readyAt: String(readyAt),
  scheduledAtBlock: "800",
  scheduledAtTimestamp: String(readyAt - 172800),
  scheduledTransactionHash: "0xs" + id.slice(2),
  executedAtBlock: null,
  executedAtTimestamp: null,
  executedTransactionHash: null,
  cancelledAtBlock: null,
  cancelledAtTimestamp: null,
  cancelledTransactionHash: null,
  ...over,
});
const tlCallRow = (id, op, index, target, data, value = "0") => ({
  id,
  operation: { id: op },
  index,
  target,
  value,
  data,
});

// The timelock holds DEPOSIT on the authoriser and owns the beacon. Three
// pending operations — one on each of those, one on a stranger — and two
// finished ones.
function tlFake(over = {}) {
  const f = fakeSubgraph({ schema: TL_SCHEMA, ...over });
  f.rows.roleHolders.push({
    id: "h3",
    account: RBAC_TL,
    grantedAtBlock: "11",
    grantedAtTimestamp: "1791883011",
    role: { id: RBAC_AUTH + ROLE_IDS.DEPOSIT.slice(2) },
  });
  // EXECUTOR_ROLE on the timelock, held by address(0): open execution.
  f.rows.roles.push({
    id: RBAC_TL + TL_EXECUTOR.slice(2),
    role: TL_EXECUTOR,
    admin: RBAC_ZERO,
    contract: {
      id: RBAC_TL,
      kind: "GOVERNANCE_TIMELOCK",
      firstIndexedBlock: "5",
    },
  });
  f.rows.roleHolders.push({
    id: "hx",
    account: TL_ADDRESS_ZERO,
    grantedAtBlock: "5",
    grantedAtTimestamp: "1791883005",
    role: { id: RBAC_TL + TL_EXECUTOR.slice(2) },
  });
  f.rows.contracts = [
    { id: RBAC_TL, kind: "GOVERNANCE_TIMELOCK", owner: null },
    { id: RBAC_AUTH, kind: "AUTHORIZER", owner: null },
    { id: TL_BEACON, kind: "BEACON", owner: RBAC_TL },
  ];
  f.rows.timelockOperations = [
    tlOpRow("0xp1", "PENDING", TL_NOW + 36 * 3600),
    tlOpRow("0xp2", "PENDING", TL_NOW),
    tlOpRow("0xp3", "PENDING", TL_NOW + 1, {
      callCount: 2,
      predecessor: "0x" + "77".repeat(32),
    }),
    tlOpRow("0xh1", "EXECUTED", TL_NOW - 9000, {
      scheduledAtBlock: "700",
      executedAtBlock: "750",
      executedAtTimestamp: String(TL_NOW - 8000),
      executedTransactionHash: "0xe1",
    }),
    tlOpRow("0xh2", "CANCELLED", TL_NOW - 5000, {
      scheduledAtBlock: "720",
      cancelledAtBlock: "760",
      cancelledAtTimestamp: String(TL_NOW - 4000),
      cancelledTransactionHash: "0xc2",
    }),
  ];
  f.rows.timelockCalls = [
    tlCallRow(
      "0xc1",
      "0xp1",
      0,
      RBAC_AUTH,
      tlCallData(SEL.grantRole, ROLE_IDS.DEPOSIT, TL_STRANGER),
    ),
    tlCallRow("0xc2", "0xp2", 0, TL_BEACON, tlCallData(SEL.upgradeTo, TL_IMPL)),
    tlCallRow("0xc3", "0xp3", 0, TL_STRANGER, "0xdeadbeef00", "5"),
    tlCallRow(
      "0xc4",
      "0xh1",
      0,
      RBAC_TL,
      tlCallData(SEL.updateDelay, (172800).toString(16)),
    ),
    tlCallRow(
      "0xc5",
      "0xh2",
      0,
      TL_BEACON,
      tlCallData(SEL.transferOwnership, TL_STRANGER),
    ),
  ];
  return f;
}
const tlSec = (p) => collect(p, "tl-sec");
const pendingCards = (p) => collect(p, "tl-pending");
const historyRows = (p) => collect(p, "tl-done");

Deno.test("deployments: a subgraph version without timelock operations says so and keeps the roles", async () => {
  const p = await rbacPanelLive(fakeSubgraph());
  const absent = collect(p, "tl-absent");
  assert(absent.length === 1, "one not-indexed banner for the network");
  assert(
    textOf(absent[0]).includes("not indexed by this subgraph version") &&
      textOf(absent[0]).includes("not the same as none pending"),
    "states the version gap honestly: " + textOf(absent[0]),
  );
  assert(collect(p, "tl-error").length === 0, "not reported as an error");
  assert(pendingCards(p).length === 0, "no operations invented");
  // The rest of the tab is untouched.
  const banners = rbacBanners(p);
  assert(
    banners.length === 1 && banners[0].className.includes("own-verify-ok"),
    "network banner still current",
  );
  assert(collect(p, "rbac-chg").length === 3, "role history still rendered");
});

Deno.test("deployments: an unreadable timelock read is an error, distinct from an old version", async () => {
  const cases = [
    [{ schema: null }, "no readable answer to the schema probe"],
    [
      { schema: { errors: [{ message: "introspection off" }] } },
      "introspection off",
    ],
    [
      { schema: { error: "subgraph name/version error" } },
      "the subgraph vanished mid-read",
    ],
    [{ schema: { data: { t: { name: "Other" } } } }, "not a type"],
    [{ schema: { data: { t: 7 } } }, "not a type"],
    [
      { raw: { timelockCalls: { errors: [{ message: "calls boom" }] } } },
      "timelockCalls: calls boom",
    ],
    [
      { raw: { contracts: { data: {} } } },
      "contracts: the response carried no rows",
    ],
  ];
  for (const [over, phrase] of cases) {
    const label = JSON.stringify(over);
    const p = await rbacPanelLive(tlFake(over));
    const err = collect(p, "tl-error");
    assert(err.length === 1, label + ": one error banner");
    assert(textOf(err[0]).includes(phrase), label + ": says " + textOf(err[0]));
    assert(
      textOf(err[0]).includes("not the same as none pending"),
      label + ": no data is not none",
    );
    assert(
      collect(p, "tl-absent").length === 0,
      label + ": not called an old version",
    );
    assert(
      pendingCards(p).length === 0 && historyRows(p).length === 0,
      label + ": nothing shown short",
    );
    assert(
      collect(p, "rbac-chg").length === 3,
      label + ": role history unaffected",
    );
  }
});

Deno.test("deployments: pending timelock operations lead, soonest first, readiness from the clock", async () => {
  const p = await rbacPanelLive(
    tlFake(),
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const sec = tlSec(p);
  assert(sec.length === 1, "one timelock section");
  // The section sits above the role contracts.
  const net = collect(p, "rbac-net")[0];
  const order = net.children.map((c) => c.className);
  assert(
    order.indexOf("tl-sec") <
      order.findIndex((c) => c.includes("rbac-contract")),
    "timelock first: " + order,
  );
  const head = collect(sec[0], "rbac-contract")[0];
  assert(textOf(head).includes("3 pending"), "pending count: " + textOf(head));
  assert(
    whoOf(head)[0].name === "STOX_GOVERNANCE_TIMELOCK_ETHEREUM",
    "timelock named",
  );
  const cards = pendingCards(p);
  assert(cards.length === 3, "three pending cards");
  const when = cards.map((c) => textOf(collect(c, "tl-when")[0]));
  // readyAt == now is ready (OpenZeppelin's `<=`); one second later is not.
  assert(
    when[0].startsWith("ready now") && when[0].includes("anyone can execute"),
    "boundary is ready: " + when[0],
  );
  assert(when[1].startsWith("ready in 1s"), "one second out: " + when[1]);
  assert(when[2].startsWith("ready in 1d 12h"), "countdown: " + when[2]);
  // Pending cards come before the history table in the section.
  const kids = sec[0].children.map((c) => c.className);
  assert(
    kids.lastIndexOf("tl-op tl-pending tl-governs") <
      kids.indexOf("own-table tl-hist"),
    "pending before history",
  );
});

Deno.test("deployments: a pending call on a contract the timelock governs stands out, and why", async () => {
  const p = await rbacPanelLive(
    tlFake(),
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const [beacon, stranger, auth] = pendingCards(p);
  assert(beacon.className.includes("tl-governs"), "owned beacon: governed");
  assert(
    collect(beacon, "tl-governed")[0].textContent ===
      "governed by this timelock",
    "chip text",
  );
  assert(
    collect(beacon, "tl-governed")[0].title === "this timelock owns it",
    "owner reason",
  );
  assert(
    auth.className.includes("tl-governs"),
    "role-held authoriser: governed",
  );
  assert(
    collect(auth, "tl-governed")[0].title ===
      "this timelock holds DEPOSIT on it",
    "role reason: " + collect(auth, "tl-governed")[0].title,
  );
  assert(!stranger.className.includes("tl-governs"), "stranger: not governed");
  assert(
    collect(stranger, "tl-governed").length === 0,
    "no chip on a stranger",
  );
  // The target and the role arguments are named the way the role tab names them.
  const authWho = whoOf(auth);
  assert(
    authWho[0].name === "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM",
    "target named",
  );
  assert(
    authWho[1].unknown && authWho[1].hex === TL_STRANGER,
    "unpinned grantee marked unrecognised",
  );
  const fn = textOf(collect(auth, "tl-fn")[0]);
  assert(
    fn.startsWith("grantRole(DEPOSIT, ") && fn.endsWith(")"),
    "decoded with the role named: " + fn,
  );
  assert(
    textOf(collect(beacon, "tl-fn")[0]).includes("upgradeTo(") &&
      textOf(beacon).includes(TL_IMPL),
    "upgradeTo decoded",
  );
  // Unknown selector, value, predecessor and a short call list are all said.
  const s = textOf(stranger);
  assert(
    s.includes("0xdeadbeef — not a function this page decodes"),
    "unknown selector: " + s,
  );
  assert(s.includes("0xdeadbeef00"), "raw calldata shown");
  assert(s.includes("value 5 wei"), "value shown");
  assert(
    s.includes("runs only after 0x" + "77".repeat(32)),
    "predecessor shown",
  );
  assert(s.includes("1 of 2 calls indexed"), "missing call flagged");
  assert(
    !textOf(beacon).includes("calls indexed"),
    "a whole operation is not flagged",
  );
});

Deno.test("deployments: executed and cancelled timelock operations are history, newest first", async () => {
  const p = await rbacPanelLive(tlFake());
  const rows = historyRows(p);
  assert(rows.length === 2, "two history rows");
  // h2 was scheduled at block 720, h1 at 700.
  assert(
    textOf(rows[0]).includes("cancelled") &&
      textOf(rows[0]).includes("transferOwnership("),
    "cancelled first",
  );
  assert(
    textOf(rows[1]).includes("executed") &&
      textOf(rows[1]).includes("updateDelay(172800s (2d 0h))"),
    "executed: " + textOf(rows[1]),
  );
  assert(
    collect(rows[1], "tl-governed")[0].title === "it is this timelock",
    "a call on itself is governed",
  );
  const links = (r) => tags(r, "a").map((a) => a.href);
  assert(
    links(rows[0]).includes("https://etherscan.io/tx/0xc2"),
    "cancel tx linked",
  );
  assert(
    links(rows[1]).includes("https://etherscan.io/tx/0xe1"),
    "execute tx linked",
  );
  assert(
    links(rows[1]).includes("https://etherscan.io/tx/0xsh1"),
    "schedule tx linked",
  );
  assert(
    textOf(tlSec(p)[0]).includes("2 executed or cancelled operations"),
    "history counted",
  );
  assert(
    pendingCards(p).every((c) => !c.className.includes("tl-done")),
    "pending is not history",
  );
});

Deno.test("deployments: timelock calldata is decoded only where it fits its ABI", async () => {
  const f = tlFake();
  const A = tlWord(TL_STRANGER);
  const calls = [
    ["0x", "no calldata"],
    ["0x2f2f", "calldata shorter than a function selector"],
    [
      SEL.grantRole + tlWord(ROLE_IDS.DEPOSIT),
      "calldata too short for its arguments",
    ],
    [SEL.transferOwnership + "ff" + A.slice(2), "argument 0 is not an address"],
    [SEL.transferOwnership + A + "00", "1 bytes past the arguments"],
    [SEL.renounceOwnership, "renounceOwnership()"],
    [
      SEL.upgradeToAndCall + tlWord(TL_IMPL) + tlWord("40") + tlWord("3") +
      "abcdef".padEnd(64, "0"),
      "3 bytes)",
    ],
    [
      SEL.upgradeToAndCall + tlWord(TL_IMPL) + tlWord("40") + tlWord("21") +
      "ab".padEnd(64, "0"),
      "argument 1 runs past the calldata",
    ],
    [
      SEL.upgradeToAndCall + tlWord(TL_IMPL) + tlWord("ffff"),
      "argument 1 runs past the calldata",
    ],
    [XSS_IMG, "calldata is not hex"],
    ["0xabc", "calldata is not hex"],
  ];
  f.rows.timelockOperations = [
    tlOpRow("0xd1", "PENDING", TL_NOW + 60, { callCount: calls.length }),
  ];
  f.rows.timelockCalls = calls.map(([data], i) =>
    tlCallRow("0xd" + String(i).padStart(2, "0"), "0xd1", i, TL_STRANGER, data)
  );
  const p = await rbacPanelLive(f);
  const rows = collect(p, "tl-call");
  assert(rows.length === calls.length, "every call rendered, in index order");
  calls.forEach(([, want], i) => {
    assert(
      textOf(rows[i]).includes(want),
      `call ${i}: wants "${want}", got ${textOf(rows[i])}`,
    );
  });
  assert(
    !textOf(rows[5]).includes("past the arguments"),
    "an exact fit has no trailing bytes",
  );
  assert(
    textOf(rows[6]).includes("upgradeToAndCall(") &&
      !textOf(rows[6]).includes("runs past") &&
      // The padded tail is part of the arguments, not past them.
      !textOf(rows[6]).includes("past the arguments"),
    "valid bytes arg decoded",
  );
  assert(tags(p, "img").length === 0, "calldata payload lands as text");
});

Deno.test("deployments: a network whose index holds no timelock says so", async () => {
  const f = tlFake();
  f.rows.contracts = [];
  f.rows.timelockOperations = [];
  f.rows.timelockCalls = [];
  const p = await rbacPanelLive(f);
  assert(
    textOf(tlSec(p)[0]).includes("holds no governance timelock"),
    textOf(tlSec(p)[0]),
  );
  const g = tlFake();
  g.rows.timelockOperations = [];
  g.rows.timelockCalls = [];
  const q = await rbacPanelLive(g);
  const head = collect(tlSec(q)[0], "rbac-contract")[0];
  assert(
    textOf(head).includes("0 pending"),
    "a timelock with nothing scheduled: " + textOf(head),
  );
  assert(
    textOf(tlSec(q)[0]).includes("Nothing is scheduled on this timelock."),
    "said in words",
  );
});

Deno.test("deployments: no timelock read when the network's index could not be read at all", async () => {
  const f = tlFake({ meta: null });
  const p = await rbacPanelLive(f);
  assert(
    tlSec(p).length === 0,
    "no timelock section under an unreachable banner",
  );
  assert(!f.seen.some((s) => s.init.body.includes("__type")), "not probed");
});

// One ready pending operation (p2, readyAt == now), on its own, with the
// timelock's EXECUTOR_ROLE holders and the operation's predecessor varied.
function tlReadyFake(over = {}, opOver = {}) {
  const f = tlFake(over);
  f.rows.timelockOperations = [
    tlOpRow("0xp2", "PENDING", TL_NOW, opOver),
    ...f.rows.timelockOperations.filter((o) => o.id !== "0xp2"),
  ].filter((o) => o.status !== "PENDING" || o.id === "0xp2");
  return f;
}
const readyCard = (p) =>
  pendingCards(p).find((c) => textOf(c).includes("upgradeTo("));
const execLine = (card) => collect(card, "tl-exec");

Deno.test("deployments: a ready operation is open to anyone only while address(0) holds EXECUTOR_ROLE", async () => {
  const p = await rbacPanelLive(tlReadyFake());
  const w = textOf(collect(readyCard(p), "tl-when")[0]);
  assert(w.startsWith("ready now"), "ready: " + w);
  assert(
    w.includes("anyone can execute it") && w.includes("only a cancel stops it"),
    "open executor: " + w,
  );
});

Deno.test("deployments: a restricted EXECUTOR_ROLE names its holders, never 'anyone'", async () => {
  const f = tlReadyFake();
  const ex = f.rows.roleHolders.find((h) => h.id === "hx");
  ex.account = RBAC_AUTH;
  f.rows.roleHolders.push({ ...ex, id: "hy", account: TL_STRANGER });
  const p = await rbacPanelLive(
    f,
    rbacData([rbacNet()], { addressNames: RBAC_PINS }),
  );
  const card = readyCard(p);
  const w = textOf(collect(card, "tl-when")[0]);
  assert(w.startsWith("ready now"), "still ready: " + w);
  assert(!w.includes("anyone"), "not open: " + w);
  const line = execLine(card);
  assert(line.length === 1, "one executor line");
  assert(
    textOf(line[0]).startsWith("only an EXECUTOR_ROLE holder can execute it"),
    textOf(line[0]),
  );
  const who = whoOf(line[0]);
  assert(
    who.length === 2 &&
      who[1].name === "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM" &&
      who[0].unknown && who[0].hex === TL_STRANGER,
    "holders named, unpinned one unrecognised: " + JSON.stringify(who),
  );
  // address(0) among other holders still opens it.
  f.rows.roleHolders.push({ ...ex, id: "hz", account: TL_ADDRESS_ZERO });
  const q = await rbacPanelLive(f);
  assert(
    textOf(execLine(readyCard(q))[0]).includes("anyone can execute it"),
    "address(0) beside others is open",
  );
});

Deno.test("deployments: no EXECUTOR_ROLE holder means nobody can execute", async () => {
  const f = tlReadyFake();
  f.rows.roleHolders = f.rows.roleHolders.filter((h) => h.id !== "hx");
  const p = await rbacPanelLive(f);
  const t = textOf(execLine(readyCard(p))[0]);
  assert(t.includes("nobody can execute it"), t);
  assert(!t.includes("anyone"), t);
  // EXECUTOR_ROLE held on ANOTHER contract does not count for this timelock.
  const g = tlReadyFake();
  g.rows.roles.find((r) => r.role === TL_EXECUTOR).contract = {
    id: RBAC_AUTH,
    kind: "AUTHORIZER",
    firstIndexedBlock: "5",
  };
  g.rows.roles.find((r) => r.role === TL_EXECUTOR).id = RBAC_AUTH +
    TL_EXECUTOR.slice(2);
  g.rows.roleHolders.find((h) => h.id === "hx").role.id = RBAC_AUTH +
    TL_EXECUTOR.slice(2);
  const q = await rbacPanelLive(g);
  assert(
    textOf(execLine(readyCard(q))[0]).includes("nobody can execute it"),
    "another contract's executor: " + textOf(execLine(readyCard(q))[0]),
  );
  // address(0) holding some OTHER role on the timelock is not open execution.
  const h = tlReadyFake();
  h.rows.roles.find((r) => r.role === TL_EXECUTOR).role = ROLE_IDS.WITHDRAW;
  const r = await rbacPanelLive(h);
  assert(
    textOf(execLine(readyCard(r))[0]).includes("nobody can execute it"),
    "another role's holder: " + textOf(execLine(readyCard(r))[0]),
  );
});

Deno.test("deployments: unreadable roles leave the executor unknown, never asserted", async () => {
  const p = await rbacPanelLive(
    tlReadyFake({ raw: { roleHolders: { errors: [{ message: "boom" }] } } }),
  );
  const card = readyCard(p);
  const w = textOf(collect(card, "tl-when")[0]);
  assert(!w.includes("anyone"), "no open claim: " + w);
  assert(
    textOf(execLine(card)[0]).startsWith("executor unknown"),
    textOf(execLine(card)[0]),
  );
});

Deno.test("deployments: the executor is said only for an operation that can run now", async () => {
  const p = await rbacPanelLive(tlFake());
  const notReady = pendingCards(p).filter((c) =>
    !textOf(c).includes("upgradeTo(")
  );
  assert(notReady.length === 2, "two not-yet-ready cards");
  for (const c of notReady) {
    assert(execLine(c).length === 0, "no executor line: " + textOf(c));
  }
});

Deno.test("deployments: a predecessor must have executed before an operation is ready", async () => {
  const pred = (id) => "0x" + id.slice(2).padEnd(64, "0");
  const when = (p) => textOf(collect(readyCard(p), "tl-when")[0]);
  // Executed (h1): no extra condition.
  const done = await rbacPanelLive(
    tlReadyFake({}, { predecessor: pred("0xh1") }),
  );
  assert(when(done).startsWith("ready now"), "executed: " + when(done));
  assert(
    when(done).includes(`predecessor ${pred("0xh1")} executed`) &&
      when(done).includes("anyone can execute it"),
    when(done),
  );
  // Pending: not ready, whatever the clock says.
  const f = tlReadyFake({}, { predecessor: pred("0xq1") });
  f.rows.timelockOperations.push(tlOpRow("0xq1", "PENDING", TL_NOW + 60));
  const pend = await rbacPanelLive(f);
  const w = when(pend);
  assert(w.startsWith("waits on predecessor"), "pending: " + w);
  assert(!w.includes("ready now") && !w.includes("anyone"), w);
  assert(
    w.includes(
      `waits on predecessor ${pred("0xq1")}, not yet executed (it is PENDING)`,
    ),
    w,
  );
  assert(execLine(readyCard(pend)).length === 0, "no executor claim");
  // Cancelled (h2): still not executed.
  const canc = await rbacPanelLive(
    tlReadyFake({}, { predecessor: pred("0xh2") }),
  );
  assert(
    when(canc).startsWith("waits on predecessor") &&
      when(canc).includes("not yet executed (it is CANCELLED)"),
    when(canc),
  );
  // Absent from the read: said, and not ready.
  const abs = await rbacPanelLive(
    tlReadyFake({}, { predecessor: "0x" + "77".repeat(32) }),
  );
  const a = when(abs);
  assert(a.startsWith("predecessor not indexed"), "absent: " + a);
  assert(
    a.includes(
      `predecessor 0x${
        "77".repeat(32)
      } is not in the index — whether it executed is unknown`,
    ) && !a.includes("ready now") && !a.includes("anyone"),
    a,
  );
  // Executed on ANOTHER timelock does not satisfy this one.
  const g = tlReadyFake({}, { predecessor: pred("0xh1") });
  g.rows.timelockOperations.find((o) => o.id === "0xh1").timelock = {
    id: TL_BEACON,
  };
  const other = await rbacPanelLive(g);
  assert(
    when(other).startsWith("predecessor not indexed"),
    "other timelock: " + when(other),
  );
});

Deno.test("deployments: a predecessor's executed scheduling outranks a cancelled one", async () => {
  const id = "0x" + "q1".padEnd(64, "0");
  const f = tlReadyFake({}, { predecessor: id });
  f.rows.timelockOperations.push(
    tlOpRow("0xqa", "CANCELLED", TL_NOW - 50, {
      operationId: id,
      scheduledAtBlock: "600",
    }),
    tlOpRow("0xqb", "EXECUTED", TL_NOW - 40, {
      operationId: id,
      scheduledAtBlock: "500",
    }),
  );
  const p = await rbacPanelLive(f);
  const w = textOf(collect(readyCard(p), "tl-when")[0]);
  assert(w.startsWith("ready now"), w);
  // Cancelled then rescheduled and still pending: the newest scheduling speaks.
  const g = tlReadyFake({}, { predecessor: id });
  g.rows.timelockOperations.push(
    tlOpRow("0xqa", "CANCELLED", TL_NOW - 50, {
      operationId: id,
      scheduledAtBlock: "600",
    }),
    tlOpRow("0xqb", "PENDING", TL_NOW + 50, {
      operationId: id,
      scheduledAtBlock: "650",
    }),
  );
  const q = await rbacPanelLive(g);
  const v = textOf(collect(readyCard(q), "tl-when")[0]);
  assert(v.includes("not yet executed (it is PENDING)"), v);
});

Deno.test("deployments: a predecessor still pending before the delay ends keeps the countdown", async () => {
  const f = tlFake();
  f.rows.timelockOperations.find((o) => o.id === "0xp1").predecessor = "0x" +
    "77".repeat(32);
  const p = await rbacPanelLive(f);
  const card = pendingCards(p).find((c) => textOf(c).includes("grantRole("));
  const w = textOf(collect(card, "tl-when")[0]);
  assert(w.startsWith("ready in 1d 12h"), w);
  assert(w.includes("delay ends") && !w.includes("executable at"), w);
  assert(w.includes("is not in the index"), w);
});

// ---- deployments.html: root registry ----

const REG_TYPES = [
  "ConfigBinding",
  "ConfigRegister",
  "MigrationLine",
  "MigrationRecord",
  "RegistryDelegate",
  "RegistryDelegateChange",
  "RegistryUnscopedDelegate",
  "RegistryUnscopedDelegateChange",
  "RegistryMeta",
];
const regProbe = (names) => ({
  data: Object.fromEntries(
    REG_TYPES.map((t, i) => [`t${i}`, names.includes(t) ? { name: t } : null]),
  ),
});
// A subgraph version that indexes the registries.
const REG_SCHEMA = regProbe(REG_TYPES);
// st0x.deploy's V5 registries (subgraph/networks.json, migrations.json).
const REG_CONFIG = {
  id: "0x82b7e6fdfd25ef64b013f98e8f2c1fc1d1872ec9",
  kind: "CONFIG_REGISTRY_V5",
};
const REG_MIGRATION = {
  id: "0xd708186f68d527b64c001398550627508205ee31",
  kind: "MIGRATION_REGISTRY_V5",
};
const REG_MIGRATION_DECLARED = "0xd708186f68D527B64c001398550627508205EE31";
const REG_CONFIG_OTHER = {
  id: "0x" + "c5".repeat(20),
  kind: "CONFIG_REGISTRY_V5",
};
const REG_MIGRATION_OTHER = {
  id: "0x" + "d5".repeat(20),
  kind: "MIGRATION_REGISTRY_V5",
};
// `cast keccak "<type>"`.
const REG_TYPE_HASH = {
  address: "0x421683f821a0574472445355be6d2b769119e8515f8376a1d7878523dfdecf7b",
  uint256: "0xec13d6d12b88433319b64e1065a96ea19cd330ef6603f5f6fb685dde3959a320",
  bytes4: "0x55eba6289855380d9b683f0861f19ccda2c51f4eb4a532fbf1ce830b150f32ee",
};
const REG_ROOT = "0x0b300013cd54a8f1ac40981f80faaa18b8cc1e4c";
const REG_ROOT_CHECKSUM = "0x0b300013CD54a8F1aC40981f80FaaA18b8Cc1E4c";
const REG_SAFE = "0x" + "5a".repeat(20);
const REG_BEACON = "0x" + "be".repeat(20);
const REG_STRANGER = "0x" + "5f".repeat(20);
// `cast keccak "st0x.deploy"`.
const REG_NS =
  "0xd9cec8396b568ba46ea521970e5edeac9163854e75287e989c213c3e9e84bda4";
// `cast keccak "rain.migration-registry.head.genesis"`.
const REG_GENESIS =
  "0xd85a7f14bb19a07649810ac09029cc9c29667e4059cb83e258bc52e213365d3f";
const REG_NAME = "0x" + "c1".repeat(32);
const regWord = (n) => "0x" + n.repeat(64 / n.length);
const regAddressWord = (a) => "0x" + a.slice(2).padStart(64, "0");
const REG_M = [regWord("e1"), regWord("e2"), regWord("e3")];
const REG_LINE = "0x" + "11".repeat(72);
const REG_LINE_B = "0x" + "00".repeat(72);
const REG_NS_B = regWord("ee");
const regRecord = (id, line, index, migration, appliedOnto, over = {}) => ({
  id,
  line: { id: line },
  index,
  migration,
  appliedOnto,
  appliedAt: String(RBAC_TS - 5000 + index),
  sender: REG_SAFE,
  blockNumber: String(40 + index),
  blockTimestamp: String(RBAC_TS - 40 + index),
  transactionHash: "0xtx" + id,
  logIndex: "0",
  metas: [],
  ...over,
});
const regMeta = (verified, strings) => ({
  verified,
  nameString: null,
  namespaceString: null,
  migrationString: null,
  ...strings,
});
const REG_M1_LABEL = "st0x.underlying-naming.rename.1";

function regFake(over = {}) {
  const f = fakeSubgraph({ registrySchema: REG_SCHEMA, ...over });
  f.rows.contracts = [
    { id: REG_SAFE, kind: "TOKEN_OWNER_SAFE" },
    { id: RBAC_TL, kind: "GOVERNANCE_TIMELOCK" },
    { id: RBAC_AUTH, kind: "AUTHORIZER" },
  ];
  const delegate = (registry) => ({
    registry,
    account: REG_ROOT,
    scope: REG_NS,
    delegate: REG_SAFE,
  });
  f.rows.registryDelegates = [
    {
      id: "d1",
      ...delegate(REG_MIGRATION),
      approved: true,
      approvedAtBlock: "20",
      approvedAtTimestamp: String(RBAC_TS - 100),
      revokedAtBlock: null,
      revokedAtTimestamp: null,
    },
    {
      id: "d2",
      ...delegate(REG_CONFIG),
      approved: false,
      approvedAtBlock: "10",
      approvedAtTimestamp: String(RBAC_TS - 300),
      revokedAtBlock: "15",
      revokedAtTimestamp: String(RBAC_TS - 200),
    },
  ];
  const change = (id, registry, approved, block) => ({
    id,
    registryDelegate: delegate(registry),
    approved,
    blockNumber: String(block),
    blockTimestamp: String(RBAC_TS - 1000 + block),
    transactionHash: "0xtx" + id,
    transactionFrom: REG_ROOT,
    logIndex: "0",
  });
  f.rows.registryDelegateChanges = [
    change("c1", REG_CONFIG, true, 10),
    change("c2", REG_CONFIG, false, 15),
    change("c3", REG_MIGRATION, true, 20),
  ];
  f.rows.registryUnscopedDelegates = [];
  f.rows.registryUnscopedDelegateChanges = [];
  f.rows.configBindings = [
    {
      id: "b1",
      registry: REG_CONFIG,
      authority: REG_ROOT,
      name: REG_NAME,
      value: regAddressWord(REG_BEACON),
      valueType: "address",
      valueTypeHash: REG_TYPE_HASH.address,
      sender: REG_SAFE,
      boundAtBlock: "30",
      boundAtTimestamp: String(TL_NOW - (3 * 86400 + 2 * 3600)),
      registerCount: 2,
      metas: [regMeta(true, { nameString: "st0x.vault-beacon.v1" })],
    },
  ];
  const write = (id, value, valueType, block) => ({
    id,
    binding: {
      registry: { id: REG_CONFIG.id },
      authority: REG_ROOT,
      name: REG_NAME,
    },
    sender: REG_SAFE,
    value,
    valueType,
    valueTypeHash: REG_TYPE_HASH[valueType],
    blockNumber: String(block),
    blockTimestamp: String(RBAC_TS - 1000 + block),
    transactionHash: "0xtx" + id,
    transactionFrom: REG_SAFE,
    logIndex: "0",
  });
  f.rows.configRegisters = [
    write("r1", "0xdeadbeef", "bytes4", 25),
    write("r2", regAddressWord(REG_BEACON), "address", 30),
  ];
  // Line B sorts after line A by namespace, though its id sorts first.
  f.rows.migrationLines = [
    {
      id: REG_LINE_B,
      registry: REG_MIGRATION,
      writer: REG_ROOT,
      namespace: REG_NS_B,
      head: regWord("98"),
      recordCount: 1,
      headRecord: { id: "0xr8" },
    },
    {
      id: REG_LINE,
      registry: REG_MIGRATION,
      writer: REG_ROOT,
      namespace: REG_NS,
      head: REG_M[2],
      recordCount: 3,
      headRecord: { id: "0xra" },
    },
  ];
  // Ids in reverse index order (the fake pages by id): line order is `index`.
  // Only record #1's write carried a meta the index verified.
  f.rows.migrationRecords = [
    regRecord("0xra", REG_LINE, 2, REG_M[2], REG_M[1]),
    regRecord("0xrb", REG_LINE, 1, REG_M[1], REG_M[0], {
      sender: RBAC_TL,
      metas: [
        regMeta(true, {
          namespaceString: "st0x.deploy",
          migrationString: REG_M1_LABEL,
        }),
      ],
    }),
    regRecord("0xrc", REG_LINE, 0, REG_M[0], REG_GENESIS),
    regRecord("0xr8", REG_LINE_B, 0, regWord("98"), REG_GENESIS),
  ];
  for (const r of f.rows.migrationRecords) {
    r.line.namespace = f.rows.migrationLines.find((l) =>
      l.id === r.line.id
    ).namespace;
  }
  return f;
}

// A network both vocabularies name alike: graph-cli's and the deploy repo's.
const REG_NET = rbacNet({ network: "base", chain: "base" });
// The block the scan read every headV5 at: after every record regFake indexes.
const REG_HEAD_BLOCK = 50;
// roh-scan's deploymentMigrations: the declared registry and each line's headV5.
const regScan = (over = {}) => ({
  org: "S01-Issuer",
  repo: "st0x.deploy",
  path: "migrations.json",
  genesis: REG_GENESIS,
  networks: [
    {
      chainId: 8453,
      network: "base",
      registry: REG_MIGRATION_DECLARED,
      rpcHost: null,
      unread: 0,
      migrations: [],
      lines: [
        {
          writer: REG_ROOT_CHECKSUM,
          namespace: REG_NS,
          headBlock: REG_HEAD_BLOCK,
          head: REG_M[2],
        },
        {
          writer: REG_ROOT_CHECKSUM,
          namespace: REG_NS_B,
          headBlock: REG_HEAD_BLOCK,
          head: regWord("98"),
        },
      ],
    },
  ],
  ...over,
});

async function regPanel(
  f,
  extra = {},
  migrations = regScan(),
  net = REG_NET,
) {
  const data = {
    ...rbacData([net], {
      configRegistries: { [net.network]: REG_CONFIG.id },
      ...extra,
    }),
    deploymentMigrations: migrations,
  };
  const { box, done } = deploymentsRender(data, f.io);
  await done;
  return collect(box, "tabpanel").find((p) => p.dataset.tab === "registry");
}
const regGroup = (p, head) => {
  const kids = collect(p, "rbac-net")[0].children;
  const at = kids.findIndex((k) =>
    k.className === "own-grouphead" && k.textContent.startsWith(head)
  );
  assert(at >= 0, "section " + head + " present: " + textOf(p));
  return kids[at + 1];
};
const regQueries = (f) => f.seen.map((s) => JSON.parse(s.init.body).query);
const regLines = (p) => collect(p, "reg-line");
const regRecs = (p) => collect(p, "reg-record");
const regIndex = (r) => collect(r, "reg-index")[0].textContent;
const regErr = (p) =>
  collect(collect(p, "rbac-net")[0], "own-verify").map(textOf).join(" | ");

Deno.test("root registry: an index that does not read the registries says so and reads nothing", async () => {
  const f = fakeSubgraph();
  const p = await regPanel(f);
  assert(textOf(p).includes("does not read the registries"), textOf(p));
  const probe = regQueries(f).find((q) => q.includes("ConfigBinding"));
  for (const t of REG_TYPES) {
    assert(probe.includes(`__type(name: "${t}")`), "probed " + t);
  }
  const reads = regQueries(f).filter((q) =>
    /page: (registry|config|migration)/.test(q)
  );
  assert(reads.length === 0, "no registry entity read: " + reads.join(" | "));
});

Deno.test("root registry: an index lacking some V5 registry types says it predates them, names them, and reads nothing", async () => {
  const lacking = async (names) => {
    const f = regFake({ registrySchema: regProbe(names) });
    const p = await regPanel(f);
    const t = textOf(p);
    assert(!t.includes("Index error"), "not an error: " + t);
    assert(!t.includes("does not read the registries"), "not absent: " + t);
    assert(
      collect(collect(p, "rbac-net")[0], "own-verify-ok").length === 1,
      "the normal index banner: " + regErr(p),
    );
    assert(!t.includes("Delegations in force"), "nothing shown: " + t);
    const reads = regQueries(f).filter((q) =>
      /page: (registry|config|migration)/.test(q)
    );
    assert(
      reads.length === 0,
      "no registry entity read: " + reads.join(" | "),
    );
    return t;
  };
  const some = await lacking(["ConfigBinding", "MigrationRecord"]);
  assert(
    some.includes(
      "This version of the index predates the V5 registries (lacks ConfigRegister, MigrationLine, RegistryDelegate, RegistryDelegateChange, RegistryUnscopedDelegate, RegistryUnscopedDelegateChange, RegistryMeta); nothing is shown for them.",
    ),
    some,
  );
  const last = await lacking(REG_TYPES.slice(0, -1));
  assert(last.includes("(lacks RegistryMeta);"), last);
  // The prod st0x-rbac schema before the V5 subgraph is deployed.
  const live = await lacking(
    REG_TYPES.filter((t) =>
      ![
        "RegistryUnscopedDelegate",
        "RegistryUnscopedDelegateChange",
        "RegistryMeta",
      ].includes(t)
    ),
  );
  assert(
    live.includes(
      "(lacks RegistryUnscopedDelegate, RegistryUnscopedDelegateChange, RegistryMeta);",
    ),
    live,
  );
});

Deno.test("root registry: a schema probe answered with a non-type, an error, or not at all is an index error", async () => {
  const bad = regProbe([]);
  bad.data.t0 = { name: "Nope" };
  const gap = regProbe(REG_TYPES);
  delete gap.data.t3;
  for (const probe of [bad, gap]) {
    const r = await regPanel(regFake({ registrySchema: probe }));
    assert(regErr(r).includes("is not a type"), regErr(r));
    assert(!textOf(r).includes("predates the V5 registries"), textOf(r));
  }
  for (
    const [probe, want] of [
      [null, "no readable answer to the schema probe"],
      [
        { error: "subgraph name/version error" },
        "the subgraph vanished mid-read",
      ],
      [{ errors: [{ message: "boom" }] }, "boom"],
    ]
  ) {
    const s = await regPanel(regFake({ registrySchema: probe }));
    assert(regErr(s).includes(want), want + ": " + regErr(s));
    assert(
      !textOf(s).includes("predates the V5 registries") &&
        !textOf(s).includes("does not read the registries"),
      want + ": " + textOf(s),
    );
  }
});

Deno.test("root registry: only approvals in force are listed as delegations, a scope labelled by its verified preimage", async () => {
  const p = await regPanel(regFake());
  const head = collect(p, "own-grouphead").map((h) => h.textContent);
  assert(head.includes("Delegations in force — 1"), head.join(" | "));
  const live = regGroup(p, "Delegations in force");
  const rows = collect(live, "own-row");
  assert(rows.length === 1, "one live approval");
  const t = textOf(rows[0]);
  assert(
    t.includes("st0x.deploy") && t.includes(REG_NS),
    "scope labelled with its hash: " + t,
  );
  assert(
    t.includes("MigrationRegistry") && !t.includes("ConfigRegistry"),
    "the revoked ConfigRegistry approval is not in force: " + t,
  );
  assert(t.includes(REG_ROOT) && t.includes(REG_SAFE), t);
});

Deno.test("root registry: a delegate that is neither a Safe nor a timelock in the index is flagged", async () => {
  const f = regFake();
  const live = (id, delegate) => ({
    ...f.rows.registryDelegates[0],
    id,
    delegate,
  });
  f.rows.registryDelegates.push(
    live("d3", RBAC_TL),
    live("d4", REG_STRANGER),
    live("d5", RBAC_AUTH),
  );
  const p = await regPanel(f);
  const rows = collect(regGroup(p, "Delegations in force"), "reg-delegate");
  assert(rows.length === 4, "every live approval: " + rows.length);
  const by = (a) => rows.find((r) => textOf(r).includes(a));
  for (
    const [a, as] of [[REG_SAFE, "token-owner Safe"], [
      RBAC_TL,
      "governance timelock",
    ]]
  ) {
    const r = by(a);
    assert(
      collect(r, "reg-stray").length === 0 && !r.className.includes("rbac-off"),
      "not flagged: " + a,
    );
    assert(
      collect(r, "own-chip").map((c) => c.textContent).includes(as),
      textOf(r),
    );
  }
  for (const a of [REG_STRANGER, RBAC_AUTH]) {
    const r = by(a);
    const tag = collect(r, "reg-stray");
    assert(
      tag.length === 1 && tag[0].textContent === "not Safe or timelock",
      "flagged: " + a,
    );
    assert(r.className.includes("rbac-off"), "row marked: " + a);
  }
  assert(
    !textOf(p).includes("holds no token-owner Safe"),
    "no gap note when the index holds them",
  );
  const g = regFake();
  g.rows.contracts = [];
  const q = await regPanel(g);
  const only = collect(regGroup(q, "Delegations in force"), "reg-delegate")[0];
  assert(
    collect(only, "reg-stray").length === 1,
    "nothing recognised without the index's Safe and timelock",
  );
  assert(
    textOf(q).includes("holds no token-owner Safe or governance timelock"),
    textOf(q),
  );
});

Deno.test("root registry: delegation changes are newest first with approve and revoke told apart", async () => {
  const p = await regPanel(regFake());
  const rows = collect(regGroup(p, "Delegation changes"), "own-row");
  const chips = rows.map((r) => collect(r, "own-chip")[0].textContent);
  assert(chips.join(",") === "approved,revoked,approved", chips.join(","));
  const kinds = rows.map((r) => collect(r, "own-chip")[0].className);
  assert(
    kinds.join() ===
      [
        "own-chip own-chip-yes",
        "own-chip own-chip-no",
        "own-chip own-chip-yes",
      ].join(),
    kinds.join(),
  );
  assert(textOf(rows[0]).includes("MigrationRegistry"), "block 20 first");
  assert(textOf(rows[2]).includes("ConfigRegistry"), "block 10 last");
});

Deno.test("root registry: every delegation and config change shows when it happened and links its transaction", async () => {
  const f = regFake();
  f.rows.registryDelegateChanges[1].blockTimestamp = String(RBAC_TS - 86400);
  f.rows.configRegisters[0].blockTimestamp = String(RBAC_TS - 2 * 86400);
  const p = await regPanel(f);
  for (
    const [group, want] of [
      ["Delegation changes", [
        ["0xtxc3", "2026-10-13 09:13 UTC"],
        ["0xtxc2", "2026-10-12 09:29 UTC"],
        ["0xtxc1", "2026-10-13 09:13 UTC"],
      ]],
      ["Config changes", [
        ["0xtxr2", "2026-10-13 09:13 UTC"],
        ["0xtxr1", "2026-10-11 09:29 UTC"],
      ]],
    ]
  ) {
    const rows = collect(regGroup(p, group), "own-row");
    assert(rows.length === want.length, group + ": " + rows.length);
    rows.forEach((r, i) => {
      const [tx, at] = want[i];
      assert(textOf(r).includes(at), group + " " + i + ": " + textOf(r));
      const hrefs = tags(r, "a").map((a) => a.href);
      assert(
        hrefs.includes("https://basescan.org/tx/" + tx),
        group + " " + i + ": " + hrefs.join(),
      );
    });
  }
});

Deno.test("root registry: a delegation change names who sent it only when that is not the approver, whatever the case", async () => {
  const f = regFake();
  f.rows.registryDelegateChanges[0].transactionFrom = REG_ROOT_CHECKSUM;
  f.rows.registryDelegateChanges[1].transactionFrom = REG_STRANGER;
  const p = await regPanel(f);
  const rows = collect(regGroup(p, "Delegation changes"), "own-row");
  const sentBy = rows.map((r) =>
    collect(r, "bcn-line").filter((x) =>
      collect(x, "bcn-key")[0].textContent === "sent by"
    ).map(textOf).join()
  );
  assert(sentBy.length === 3, sentBy.join(" | "));
  assert(sentBy[0] === "" && sentBy[2] === "", sentBy.join(" | "));
  assert(sentBy[1].includes(REG_STRANGER), sentBy.join(" | "));
});

Deno.test("root registry: a delegation in force shows when it was approved, if the index says", async () => {
  const at = async (ts) => {
    const f = regFake();
    f.rows.registryDelegates[0].approvedAtTimestamp = ts;
    const p = await regPanel(f);
    return textOf(
      collect(regGroup(p, "Delegations in force"), "reg-delegate")[0],
    );
  };
  const known = await at(String(RBAC_TS - 100));
  assert(known.includes("2026-10-13 09:28 UTC"), known);
  const unknown = await at(null);
  assert(
    !unknown.includes("UTC") && !unknown.includes("time unknown"),
    unknown,
  );
});

Deno.test("root registry: a delegate is recognised as the index's Safe whatever the hex case of either address", async () => {
  const upper = (a) => "0x" + a.slice(2).toUpperCase();
  for (
    const [why, contract, delegate] of [
      ["the index's Safe in upper case", upper(REG_SAFE), REG_SAFE],
      ["the delegate in upper case", REG_SAFE, upper(REG_SAFE)],
    ]
  ) {
    const f = regFake();
    f.rows.contracts[0] = { ...f.rows.contracts[0], id: contract };
    f.rows.registryDelegates[0].delegate = delegate;
    const p = await regPanel(f);
    const row = collect(regGroup(p, "Delegations in force"), "reg-delegate")[0];
    assert(collect(row, "reg-stray").length === 0, why + ": " + textOf(row));
    assert(
      collect(row, "own-chip").map((c) => c.textContent).includes(
        "token-owner Safe",
      ),
      why + ": " + textOf(row),
    );
  }
});

Deno.test("root registry: changes in one block are newest first by log index", async () => {
  const f = regFake();
  const c3 = f.rows.registryDelegateChanges[2];
  c3.logIndex = "9";
  f.rows.registryDelegateChanges.push({
    ...c3,
    id: "c4",
    approved: false,
    logIndex: "10",
  });
  const r2 = f.rows.configRegisters[1];
  r2.logIndex = "9";
  f.rows.configRegisters.push({
    ...r2,
    id: "r3",
    value: "0xcafe",
    valueType: "bytes4",
    logIndex: "10",
  });
  const p = await regPanel(f);
  const changes = collect(regGroup(p, "Delegation changes"), "own-row");
  assert(
    changes.map((r) => collect(r, "own-chip")[0].textContent).join() ===
      "revoked,approved,revoked,approved",
    changes.map(textOf).join(" | "),
  );
  const writes = collect(regGroup(p, "Config changes"), "own-row").map(textOf);
  assert(writes.length === 3, writes.join(" | "));
  assert(writes[0].includes("0xcafe"), "log 10 first: " + writes[0]);
  assert(writes[1].includes(REG_BEACON), "log 9 next: " + writes[1]);
  assert(writes[2].includes("0xdeadbeef"), "the older block last");
});

Deno.test("root registry: config values that hold an address read as it, others as their bytes", async () => {
  const p = await regPanel(regFake());
  const binding = textOf(regGroup(p, "Config bindings"));
  assert(
    binding.includes("st0x.vault-beacon.v1") && binding.includes(REG_BEACON),
    binding,
  );
  assert(binding.includes("2 writes"), binding);
  assert(
    binding.includes("bound by"),
    "a delegate's write names who wrote it: " + binding,
  );
  const writes = collect(regGroup(p, "Config changes"), "own-row").map(textOf);
  assert(writes.length === 2, "every write");
  assert(writes[0].includes(REG_BEACON), "newest write first: " + writes[0]);
  assert(
    writes[1].includes("0xdeadbeef"),
    "a non-address value as its bytes: " + writes[1],
  );
});

Deno.test("root registry: bindings show their age and block, labelled names first", async () => {
  const f = regFake();
  const bind = (id, name, ts) => ({
    ...f.rows.configBindings[0],
    id,
    name,
    boundAtBlock: "31",
    boundAtTimestamp: String(ts),
    metas: [],
  });
  // An unnamed hash that sorts below the labelled one, bound ahead of the clock.
  f.rows.configBindings.push(
    bind("b0", regWord("01"), TL_NOW + 30),
    bind("b2", regWord("cc"), TL_NOW - 90),
    bind("b3", regWord("dd"), TL_NOW),
    {
      ...bind("b4", regWord("ef"), 0),
      boundAtBlock: null,
      boundAtTimestamp: null,
    },
  );
  const p = await regPanel(f);
  const rows = collect(regGroup(p, "Config bindings"), "reg-binding");
  const names = rows.map((r) => collect(r, "own-role")[0].textContent);
  assert(
    names.join() ===
      [
        "st0x.vault-beacon.v1",
        regWord("01"),
        regWord("cc"),
        regWord("dd"),
        regWord("ef"),
      ].join(),
    names.join(),
  );
  const ages = rows.map((r) => collect(r, "reg-age")[0].textContent);
  assert(
    ages.join() ===
      "3d 2h ago,ahead of this browser's clock,1m 30s ago,0s ago,age unknown",
    ages.join(),
  );
  assert(textOf(rows[0]).includes("block 30 · "), textOf(rows[0]));
  assert(textOf(rows[1]).includes("block 31 · "), textOf(rows[1]));
  assert(
    textOf(rows[4]).includes("block ? · time unknown"),
    textOf(rows[4]),
  );
});

Deno.test("root registry: a binding names its write count only past one, whether the index gives it as a number or a string", async () => {
  for (
    const [count, want] of [
      [2, "2 writes"],
      ["2", "2 writes"],
      ["17", "17 writes"],
      [1, null],
      ["1", null],
    ]
  ) {
    const f = regFake();
    f.rows.configBindings[0].registerCount = count;
    const p = await regPanel(f);
    const t = textOf(
      collect(regGroup(p, "Config bindings"), "reg-binding")[0],
    );
    const shown = /\d+ writes/.exec(t);
    assert(
      (shown ? shown[0] : null) === want,
      JSON.stringify(count) + ": " + t,
    );
  }
});

Deno.test("root registry: a binding or config write names who wrote it only when that is not the authority, whatever the case", async () => {
  const f = regFake();
  f.rows.configBindings[0].sender = REG_ROOT_CHECKSUM;
  f.rows.configRegisters[0].sender = REG_ROOT_CHECKSUM;
  const p = await regPanel(f);
  const boundBy = (r) =>
    collect(r, "bcn-line").filter((x) =>
      collect(x, "bcn-key")[0].textContent === "bound by"
    ).map(textOf).join();
  const binding = collect(regGroup(p, "Config bindings"), "reg-binding")[0];
  assert(boundBy(binding) === "", textOf(binding));
  const writes = collect(regGroup(p, "Config changes"), "own-row");
  assert(writes.length === 2, writes.map(textOf).join(" | "));
  assert(boundBy(writes[0]).includes(REG_SAFE), textOf(writes[0]));
  assert(boundBy(writes[1]) === "", textOf(writes[1]));
});

Deno.test("root registry: each migration line reads in index order with its head, the chain checked", async () => {
  const p = await regPanel(regFake());
  const lines = regLines(p);
  assert(lines.length === 2, "two lines");
  assert(
    textOf(lines[0]).includes("st0x.deploy") &&
      textOf(lines[1]).includes(REG_NS_B),
    "ordered by namespace",
  );
  const head = textOf(lines[0]);
  assert(
    head.includes(REG_M[2]) && head.includes("3 records, chain intact"),
    head,
  );
  assert(head.includes(REG_ROOT), "writer named");
  assert(textOf(lines[1]).includes("1 record, chain intact"), textOf(lines[1]));
  const recs = regRecs(p);
  assert(
    recs.map(regIndex).join() === "#0,#1,#2,#0",
    recs.map(regIndex).join(),
  );
  assert(
    recs.slice(0, 3).every((r, i) => textOf(r).includes(REG_M[i])),
    "each record's migration",
  );
  assert(
    textOf(recs[0]).includes("applied onto") &&
      textOf(recs[0]).includes("genesis"),
    "genesis labelled: " + textOf(recs[0]),
  );
  assert(
    textOf(recs[1]).includes("recorded by") &&
      textOf(recs[1]).includes(RBAC_TL),
    "a sender other than the writer",
  );
  assert(
    collect(p, "reg-problem").length === 0,
    "no problem on an intact line",
  );
  assert(recs.every((r) => !r.className.includes("rbac-off")), "none marked");
});

Deno.test("root registry: lines in one namespace are ordered by writer", async () => {
  const f = regFake();
  const writer = "0x" + "0a".repeat(20);
  // Its id sorts after line A's, so the index reads it after.
  f.rows.migrationLines.push({
    id: "0x" + "22".repeat(72),
    registry: REG_MIGRATION,
    writer,
    namespace: REG_NS,
    head: null,
    recordCount: 0,
    headRecord: null,
  });
  const p = await regPanel(f);
  const lines = regLines(p);
  const writerOf = (l) =>
    textOf(
      collect(l, "bcn-line").find((x) =>
        collect(x, "bcn-key")[0].textContent === "writer"
      ),
    );
  assert(lines.length === 3, regHeads(p));
  assert(
    writerOf(lines[0]).includes(writer) &&
      writerOf(lines[1]).includes(REG_ROOT) &&
      textOf(lines[1]).includes("3 records") &&
      textOf(lines[2]).includes(REG_NS_B),
    lines.map(textOf).join(" | "),
  );
});

Deno.test("root registry: a broken migration chain is named record by record", async () => {
  const leafs = (n) => collect(n, "reg-problem").map((x) => x.textContent);
  const f = regFake();
  f.rows.migrationRecords.find((r) => r.id === "0xrb").appliedOnto = regWord(
    "77",
  );
  const p = await regPanel(f);
  const recs = regRecs(p);
  assert(
    leafs(recs[1]).join() ===
      `applied onto ${regWord("77")}, not the previous record's migration ${
        REG_M[0]
      }`,
    leafs(recs[1]).join(),
  );
  assert(recs[1].className.includes("rbac-off"), "row marked");
  assert(
    leafs(recs[0]).length === 0 && leafs(recs[2]).length === 0,
    "others clean",
  );
  assert(
    textOf(regLines(p)[0]).includes("3 records, chain broken"),
    textOf(regLines(p)[0]),
  );
  assert(
    textOf(regLines(p)[1]).includes("chain intact"),
    "the other line is untouched",
  );

  const g = regFake();
  g.rows.migrationRecords.find((r) => r.id === "0xrc").appliedOnto = RBAC_ZERO;
  const q = await regPanel(g);
  assert(
    leafs(regRecs(q)[0]).join() === `applied onto ${RBAC_ZERO}, not genesis`,
    textOf(regRecs(q)[0]),
  );
  assert(textOf(regLines(q)[0]).includes("chain broken"), "broken");

  const h = regFake();
  h.rows.migrationRecords = h.rows.migrationRecords.filter((r) =>
    r.id !== "0xrb"
  );
  const r = await regPanel(h);
  assert(
    leafs(regRecs(r)[1]).includes("at index 2, where 1 was next"),
    leafs(regRecs(r)[1]).join(" | "),
  );
  const lineProbs = collect(regLines(r)[0], "reg-problem").map((x) =>
    x.textContent
  );
  assert(
    lineProbs.join() === "the line counts 3 records; the read holds 2",
    lineProbs.join(" | "),
  );

  const u = regFake();
  u.rows.migrationLines[1].recordCount = "three";
  const uProbs = collect(regLines(await regPanel(u))[0], "reg-problem").map((
    x,
  ) => x.textContent);
  assert(
    uProbs.join(" | ") === "the line counts ? records; the read holds 3",
    "an unreadable count is a problem, shown as ?: " + uProbs.join(" | "),
  );

  const k = regFake();
  k.rows.migrationLines[1].headRecord = { id: "0xrb" };
  const behind = regScan();
  behind.networks[0].lines[0].head = REG_M[1];
  const s = await regPanel(k, undefined, behind);
  const hp = collect(regLines(s)[0], "reg-problem").map((x) => x.textContent);
  assert(
    hp.join(" | ") ===
      `the head ${
        REG_M[1]
      } is not the migration of the last record at or before block ${REG_HEAD_BLOCK} | the head record is not the last record`,
    hp.join(" | "),
  );
  assert(textOf(regLines(s)[0]).includes("chain broken"), "broken");
});

Deno.test("root registry: a record the index gives no position sorts last and is named at index ?", async () => {
  const f = regFake();
  f.rows.migrationRecords.push(
    regRecord("0xr0", REG_LINE, null, regWord("e4"), REG_M[2], {
      line: { id: REG_LINE, namespace: REG_NS },
      blockNumber: "43",
    }),
  );
  f.rows.migrationLines[1] = {
    ...f.rows.migrationLines[1],
    recordCount: 4,
    headRecord: { id: "0xr0" },
  };
  const scan = regScan();
  scan.networks[0].lines[0].head = regWord("e4");
  const p = await regPanel(f, undefined, scan);
  const recs = regRecs(p).slice(0, 4);
  assert(
    recs.map(regIndex).join() === "#0,#1,#2,#?",
    recs.map(regIndex).join(),
  );
  const leafs = recs.map((r) =>
    collect(r, "reg-problem").map((x) => x.textContent).join(" | ")
  );
  assert(
    leafs.join(",") === ",,,at index ?, where 3 was next",
    leafs.join(","),
  );
  assert(
    collect(regLines(p)[0], "reg-problem").length === 0,
    textOf(regLines(p)[0]),
  );
});

Deno.test("root registry: without the scan's genesis the first record is unchecked, the rest still checked", async () => {
  const p = await regPanel(
    regFake(),
    undefined,
    regScan({ genesis: undefined }),
  );
  const lines = regLines(p);
  assert(
    textOf(lines[0]).includes("3 records, chain unchecked"),
    textOf(lines[0]),
  );
  assert(textOf(lines[0]).includes("no migration genesis"), textOf(lines[0]));
  assert(collect(p, "reg-problem").length === 0, "nothing called broken");
  assert(
    !textOf(regRecs(p)[0]).includes("genesis"),
    "genesis not labelled without the scan's",
  );
  const malformed = await regPanel(
    regFake(),
    undefined,
    regScan({ genesis: "0x1234" }),
  );
  assert(
    textOf(regLines(malformed)[0]).includes("chain unchecked"),
    "a genesis that is not a word is no genesis: " +
      textOf(regLines(malformed)[0]),
  );
  const f = regFake();
  f.rows.migrationRecords.find((r) => r.id === "0xra").appliedOnto = regWord(
    "77",
  );
  const q = await regPanel(f, undefined, regScan({ genesis: "0x1234" }));
  assert(
    textOf(regLines(q)[0]).includes("chain broken"),
    "a wrong link is still found",
  );
  const early = regScan({ genesis: undefined });
  early.networks[0].lines[0] = {
    ...early.networks[0].lines[0],
    headBlock: 30,
    head: REG_M[0],
  };
  const e = regLines(await regPanel(regFake(), undefined, early))[0];
  assert(
    collect(e, "reg-problem").length === 0 &&
      textOf(e).includes("3 records, chain unchecked"),
    "a head read before the first record has nothing to check against: " +
      textOf(e),
  );
  const empty = regFake();
  empty.rows.migrationLines[1] = {
    ...empty.rows.migrationLines[1],
    recordCount: 0,
    headRecord: null,
  };
  empty.rows.migrationRecords = empty.rows.migrationRecords.filter((r) =>
    r.line.id === REG_LINE_B
  );
  const z = regLines(
    await regPanel(empty, undefined, regScan({ genesis: undefined })),
  )[0];
  assert(
    textOf(z).includes("0 records, chain intact") &&
      !textOf(z).includes("no migration genesis"),
    "a line with no records has no first record to leave unchecked: " +
      textOf(z),
  );
});

Deno.test("root registry: a registry with no migration line says none has been recorded", async () => {
  const f = regFake();
  f.rows.migrationLines = [];
  f.rows.migrationRecords = [];
  const p = await regPanel(f, undefined, regScan({ networks: [] }));
  assert(regHeads(p).includes("Migration lines — 0"), regHeads(p));
  assert(regLines(p).length === 0, textOf(p));
  assert(
    textOf(p).includes(
      "No migration has been recorded in the MigrationRegistry.",
    ),
    textOf(p),
  );
  const q = await regPanel(regFake());
  assert(
    !textOf(q).includes("No migration has been recorded"),
    "not beside lines: " + textOf(q),
  );
});

Deno.test("root registry: a record on a line the index does not hold fails the read", async () => {
  for (
    const [why, line] of [
      ["another line", { id: "0x" + "33".repeat(72) }],
      ["no line", null],
    ]
  ) {
    const f = regFake();
    f.rows.migrationRecords.push(
      regRecord("0xr0", REG_LINE, 0, regWord("97"), REG_GENESIS, { line }),
    );
    const p = await regPanel(f);
    assert(
      regErr(p).includes(
        "migrationRecords: a record on a line the index does not hold",
      ),
      why + ": " + regErr(p),
    );
    assert(
      regLines(p).length === 0 && !textOf(p).includes("Delegations in force"),
      why + ": nothing shown short",
    );
  }
});

Deno.test("root registry: a row that is not an object, or has no string id, fails its read, wherever it sits", async () => {
  for (
    const [entity, at] of [
      ["configBindings", 0],
      ["registryDelegates", 1],
      ["migrationRecords", 0],
      ["contracts", 2],
      ["registryUnscopedDelegates", 0],
      ["registryUnscopedDelegateChanges", 0],
    ]
  ) {
    const like = regFake().rows[entity][0] ?? {};
    for (
      const [why, bad] of [
        ["null", null],
        ["no id", {}],
        ["a number id", { ...like, id: 5 }],
      ]
    ) {
      const f = regFake();
      const page = f.rows[entity].slice();
      page.splice(at, 0, bad);
      f.raw[entity] = { data: { page } };
      const p = await regPanel(f);
      assert(
        regErr(p).includes(`${entity}: a row without an id`),
        entity + " (" + why + "): " + regErr(p),
      );
      assert(
        !textOf(p).includes("Delegations in force"),
        entity + " (" + why + "): nothing shown short",
      );
    }
  }
  for (const bad of [null, {}, { id: 5 }]) {
    const f = regFake();
    f.raw.roles = { data: { page: [bad, ...f.rows.roles] } };
    const p = await rbacPanelLive(f);
    assert(
      textOf(p).includes("roles: a row without an id"),
      "the same holds for every paged read: " + textOf(p),
    );
  }
});

const regShown = (p) => {
  const value = (n, key) =>
    collect(
      collect(n, "bcn-line").find((x) =>
        collect(x, "bcn-key")[0].textContent === key
      ),
      "own-note",
    )[0].textContent;
  const role = (n) => collect(n, "own-role")[0].textContent;
  return [
    role(regGroup(p, "Delegations in force")),
    role(regGroup(p, "Config bindings")),
    value(regLines(p)[0], "namespace"),
    role(regRecs(p)[1]),
    value(regRecs(p)[2], "applied onto"),
  ];
};
const REG_RAW = [REG_NS, REG_NAME, REG_NS, REG_M[1], REG_M[1]];

Deno.test("root registry: a hash is labelled by its preimage only where the index verified it", async () => {
  const f = regFake();
  f.rows.configBindings[0].metas.push(
    regMeta(false, { nameString: "forged.name" }),
  );
  f.rows.migrationRecords.find((r) => r.id === "0xrb").metas.push(
    regMeta(false, {
      namespaceString: "forged.namespace",
      migrationString: "forged.migration",
    }),
  );
  const p = await regPanel(f);
  const shown = regShown(p);
  assert(
    shown.join() ===
      [
        "st0x.deploy",
        "st0x.vault-beacon.v1",
        "st0x.deploy",
        REG_M1_LABEL,
        REG_M1_LABEL,
      ].join(),
    shown.join(),
  );
  assert(
    collect(regRecs(p)[1], "grt-roleid")[0].textContent === REG_M[1],
    "a labelled migration keeps its hash beside it",
  );
  assert(
    !textOf(collect(p, "rbac-net")[0]).includes("forged"),
    "an unverified meta labels nothing",
  );
  const raws = regRecs(p).map((r) => collect(r, "own-role")[0].textContent);
  assert(
    raws.join() === [REG_M[0], REG_M1_LABEL, REG_M[2], regWord("98")].join(),
    "a migration no verified meta names is its hash: " + raws.join(),
  );

  const g = regFake();
  for (const m of [...g.rows.configBindings, ...g.rows.migrationRecords]) {
    for (const x of m.metas) x.verified = false;
  }
  const q = await regPanel(g);
  assert(regShown(q).join() === REG_RAW.join(), regShown(q).join());
  for (const s of ["st0x.deploy", "st0x.vault-beacon.v1", REG_M1_LABEL]) {
    assert(!textOf(collect(q, "rbac-net")[0]).includes(s), "unverified: " + s);
  }

  const h = regFake();
  for (const m of [...h.rows.configBindings, ...h.rows.migrationRecords]) {
    m.metas = [];
  }
  const r = await regPanel(h, {
    roleNames: {
      [REG_NS]: "NS_ROLE",
      [REG_NAME]: "NAME_ROLE",
      [REG_M[1]]: "M_ROLE",
    },
  });
  assert(regShown(r).join() === REG_RAW.join(), regShown(r).join());
  assert(
    !/NS_ROLE|NAME_ROLE|M_ROLE/.test(textOf(collect(r, "rbac-net")[0])),
    "a role name is no label",
  );
});

Deno.test("root registry: a verified preimage labels its hash whatever the hex case the index wrote it in", async () => {
  const f = regFake();
  f.rows.configBindings[0].name = "0x" + REG_NAME.slice(2).toUpperCase();
  const p = await regPanel(f);
  const binding = collect(regGroup(p, "Config bindings"), "reg-binding")[0];
  assert(
    collect(binding, "own-role")[0].textContent === "st0x.vault-beacon.v1",
    textOf(binding),
  );
});

Deno.test("root registry: a verified meta that carries no preimage leaves the label another gave", async () => {
  const f = regFake();
  f.rows.configBindings[0].metas.push(regMeta(true, {}));
  f.rows.migrationRecords.find((r) => r.id === "0xrc").metas = [
    regMeta(true, { migrationString: "st0x.first" }),
  ];
  const p = await regPanel(f);
  const shown = regShown(p);
  assert(
    shown.join() ===
      [
        "st0x.deploy",
        "st0x.vault-beacon.v1",
        "st0x.deploy",
        REG_M1_LABEL,
        REG_M1_LABEL,
      ].join(),
    shown.join(),
  );
  assert(
    collect(regRecs(p)[0], "own-role")[0].textContent === "st0x.first",
    textOf(regRecs(p)[0]),
  );
});

Deno.test("root registry: a preimage verified for a name that is no hash labels no other name", async () => {
  const f = regFake();
  f.rows.configBindings[0].name = null;
  f.rows.configRegisters[0].binding.name = "";
  const p = await regPanel(f);
  const writes = collect(regGroup(p, "Config changes"), "own-row");
  assert(
    writes.map((r) => collect(r, "own-role")[0].textContent).join() ===
      [REG_NAME, ""].join(),
    writes.map(textOf).join(" | "),
  );
  assert(
    !textOf(collect(p, "rbac-net")[0]).includes("st0x.vault-beacon.v1"),
    "the preimage of no hash labels nothing",
  );
});

Deno.test("root registry: metas that are not a list label nothing and the read stands", async () => {
  const f = regFake();
  f.rows.configBindings[0].metas = regMeta(true, {
    nameString: "st0x.vault-beacon.v1",
  });
  f.rows.migrationRecords.find((r) => r.id === "0xrb").metas = regMeta(true, {
    namespaceString: "st0x.deploy",
    migrationString: REG_M1_LABEL,
  });
  const p = await regPanel(f);
  assert(regShown(p).join() === REG_RAW.join(), regShown(p).join());
  assert(
    regHeads(p).includes("Config bindings — 1") &&
      regHeads(p).includes("Migration lines — 2"),
    regErr(p),
  );
});

Deno.test("root registry: a scope or name the index gives as no hash reads as (unknown)", async () => {
  const f = regFake();
  f.rows.registryDelegates[0].scope = null;
  f.rows.configBindings[0].name = null;
  f.rows.configRegisters[1].binding.name = 7;
  const p = await regPanel(f);
  const role = (n) => collect(n, "own-role")[0].textContent;
  const live = collect(regGroup(p, "Delegations in force"), "reg-delegate");
  assert(live.length === 1 && role(live[0]) === "(unknown)", textOf(p));
  const binding = collect(regGroup(p, "Config bindings"), "reg-binding")[0];
  assert(role(binding) === "(unknown)", textOf(binding));
  assert(
    !textOf(binding).includes("st0x.vault-beacon.v1"),
    "no label without a hash: " + textOf(binding),
  );
  const writes = collect(regGroup(p, "Config changes"), "own-row");
  assert(
    writes.map(role).join() === ["(unknown)", REG_NAME].join(),
    writes.map(role).join(),
  );
});

Deno.test("root registry: a failed page shows no registry history at all", async () => {
  const f = regFake({
    raw: { configRegisters: { errors: [{ message: "boom" }] } },
  });
  const p = await regPanel(f);
  const t = textOf(p);
  assert(t.includes("boom"), t);
  assert(!t.includes("Delegations in force"), "nothing shown short: " + t);
});

Deno.test("root registry: labels and registry strings render as text, never markup", async () => {
  const f = regFake();
  f.rows.configBindings[0].value = XSS_IMG;
  f.rows.configBindings[0].name = XSS_SCRIPT;
  f.rows.configBindings[0].valueType = XSS_SVG;
  f.rows.configBindings[0].metas = [regMeta(true, { nameString: XSS_IMG })];
  f.rows.registryDelegates[0].scope = XSS_ATTR;
  f.rows.migrationRecords.find((r) => r.id === "0xrb").metas = [
    regMeta(true, { namespaceString: XSS_IMG, migrationString: XSS_IMG }),
  ];
  const scan = regScan();
  scan.networks[0].lines[0].head = XSS_SVG;
  const p = await regPanel(f, undefined, scan);
  const t = textOf(p);
  for (const x of [XSS_SCRIPT, XSS_SVG, XSS_ATTR]) {
    assert(t.includes(x), "as text: " + x);
  }
  assert(
    textOf(regGroup(p, "Config bindings")).includes(XSS_SVG),
    "a value type as text",
  );
  assert(textOf(regLines(p)[0]).includes(XSS_SVG), "a scan head as text");
  assert(
    t.includes("unreadable value"),
    "a non-hex value is not echoed as bytes",
  );
  assert(textOf(regLines(p)[0]).includes(XSS_IMG), "a namespace as text");
  assert(
    textOf(regGroup(p, "Config bindings")).includes(XSS_IMG),
    "a name as text",
  );
  assert(textOf(regRecs(p)[1]).includes(XSS_IMG), "a migration as text");
  for (const tag of MARKUP_TAGS) {
    assert(tags(p, tag).length === 0, "no " + tag + " element created");
  }
});

// Rows on a MigrationRegistry and a ConfigRegistry the deploy repo does not
// declare: a line with its record, delegations scoped and unscoped, a binding
// and its write.
function regElsewhere(f) {
  const other = "0x" + "44".repeat(72);
  f.rows.migrationLines.push({
    id: other,
    registry: REG_MIGRATION_OTHER,
    writer: REG_ROOT,
    namespace: REG_NS,
    head: regWord("97"),
    recordCount: 1,
    headRecord: { id: "0xr4" },
  });
  f.rows.migrationRecords.push(
    regRecord("0xr4", other, 0, regWord("97"), REG_GENESIS, {
      line: { id: other, namespace: REG_NS },
    }),
  );
  for (
    const [id, registry] of [["d8", REG_MIGRATION_OTHER], [
      "d9",
      REG_CONFIG_OTHER,
    ]]
  ) {
    f.rows.registryDelegates.push({
      ...f.rows.registryDelegates[0],
      id,
      registry,
    });
    f.rows.registryDelegateChanges.push({
      ...f.rows.registryDelegateChanges[2],
      id: "c" + id,
      registryDelegate: {
        ...f.rows.registryDelegateChanges[2].registryDelegate,
        registry,
      },
    });
  }
  f.rows.registryUnscopedDelegates.push({
    id: "u8",
    registry: REG_MIGRATION_OTHER,
    account: REG_ROOT,
    delegate: REG_SAFE,
    approved: true,
    approvedAtBlock: "21",
    approvedAtTimestamp: String(RBAC_TS - 90),
    revokedAtBlock: null,
    revokedAtTimestamp: null,
  });
  f.rows.registryUnscopedDelegateChanges.push({
    id: "uc8",
    registryUnscopedDelegate: {
      registry: REG_MIGRATION_OTHER,
      account: REG_ROOT,
      delegate: REG_SAFE,
    },
    approved: true,
    blockNumber: "21",
    blockTimestamp: String(RBAC_TS - 979),
    transactionHash: "0xtxuc8",
    transactionFrom: REG_ROOT,
    logIndex: "0",
  });
  f.rows.configBindings.push({
    ...f.rows.configBindings[0],
    id: "b9",
    registry: REG_CONFIG_OTHER,
    name: regWord("b9"),
    metas: [],
  });
  f.rows.configRegisters.push({
    ...f.rows.configRegisters[1],
    id: "r9",
    binding: {
      registry: { id: REG_CONFIG_OTHER.id },
      authority: REG_ROOT,
      name: regWord("b9"),
    },
  });
  return f;
}
const regHeads = (p) =>
  collect(p, "own-grouphead").map((h) => h.textContent).join(" | ");

Deno.test("root registry: only the registries the deploy repo declares are read into the tab", async () => {
  const p = await regPanel(regElsewhere(regFake()));
  const heads = regHeads(p);
  for (
    const h of [
      "Delegations in force — 1",
      "Delegation changes — 3, newest first",
      "Config bindings — 1",
      "Config changes — 2, newest first",
      "Migration lines — 2",
    ]
  ) {
    assert(heads.includes(h), h + ": " + heads);
  }
  assert(regRecs(p).length === 4, "the other registry's record is not shown");
  assert(!textOf(p).includes(regWord("97")), "nor its line's head");
  assert(!textOf(p).includes(regWord("b9")), "nor its binding");

  const q = await regPanel(
    regElsewhere(regFake()),
    { configRegistries: {} },
    regScan({ networks: [] }),
  );
  const all = regHeads(q);
  for (
    const h of [
      "Delegations in force — 4",
      "Delegation changes — 6, newest first",
      "Config bindings — 2",
      "Config changes — 3, newest first",
      "Migration lines — 3",
    ]
  ) {
    assert(all.includes(h), "nothing declared, nothing dropped: " + all);
  }
});

Deno.test("root registry: on Ethereum the declared registry and heads apply though the subgraph names it mainnet", async () => {
  const scan = regScan();
  scan.networks[0] = { ...scan.networks[0], chainId: 1, network: "ethereum" };
  const p = await regPanel(
    regElsewhere(regFake()),
    undefined,
    scan,
    rbacNet(),
  );
  assert(regHeads(p).includes("Migration lines — 2"), regHeads(p));
  assert(
    regLines(p).every((l) => textOf(l).includes("chain intact")),
    regLines(p).map(textOf).join(" | "),
  );
  assert(collect(p, "reg-problem").length === 0, "no line is missing its head");
  assert(
    regLines(p).every((l) => !textOf(regHeadLine(l)).includes("not read")),
    "every head read: " + regLines(p).map(textOf).join(" | "),
  );
});

Deno.test("root registry: on Ethereum the declared ConfigRegistry is found under the subgraph's own network name", async () => {
  const scan = regScan();
  scan.networks[0] = { ...scan.networks[0], chainId: 1, network: "ethereum" };
  // roh-scan keys configRegistries by subgraph/networks.json's keys, as it
  // keys the networks the page reads.
  const p = await regPanel(
    regElsewhere(regFake()),
    { configRegistries: { mainnet: REG_CONFIG.id } },
    scan,
    rbacNet(),
  );
  const heads = regHeads(p);
  for (
    const h of [
      "Delegations in force — 1",
      "Delegation changes — 3, newest first",
      "Config bindings — 1",
      "Config changes — 2, newest first",
    ]
  ) {
    assert(heads.includes(h), h + ": " + heads);
  }
  assert(!textOf(p).includes(regWord("b9")), "the other registry's binding");
});

Deno.test("root registry: a value reads as an address only when its type says address", async () => {
  const f = regFake();
  const bind = (id, value, valueType) => ({
    ...f.rows.configBindings[0],
    id,
    name: regWord(id.slice(1).padStart(2, "0")),
    value,
    valueType,
    valueTypeHash: REG_TYPE_HASH[valueType] ?? regWord("ff"),
    metas: [],
  });
  f.rows.configBindings.push(
    bind("b2", regAddressWord(REG_STRANGER), "uint256"),
    bind("b3", "0xdeadbeef", "address"),
    // graph-node decoded the type lossily: no type, so the word is judged alone.
    bind("b4", regAddressWord(REG_STRANGER), null),
  );
  const p = await regPanel(f);
  const rows = collect(regGroup(p, "Config bindings"), "reg-binding");
  const by = (name) =>
    rows.find((r) => collect(r, "own-role")[0].textContent === name);
  const value = (r) =>
    collect(r, "bcn-line").find((l) =>
      collect(l, "bcn-key")[0].textContent === "value"
    );
  const addrs = (r) => collect(value(r), "own-addr").map((a) => a.textContent);
  const raw = (r) => collect(value(r), "mig-hash").map((a) => a.textContent);
  const type = (r) => collect(value(r), "own-note").map((a) => a.textContent);

  const a = by("st0x.vault-beacon.v1");
  assert(addrs(a).join() === REG_BEACON && raw(a).length === 0, textOf(a));
  assert(type(a).join() === "address", type(a).join());

  const n = by(regWord("02"));
  assert(
    addrs(n).length === 0,
    "a uint256 word is not an address: " + textOf(n),
  );
  assert(raw(n).join() === regAddressWord(REG_STRANGER), raw(n).join());
  assert(type(n).join() === "uint256", type(n).join());

  const s = by(regWord("03"));
  assert(
    addrs(s).length === 0 && raw(s).join() === "0xdeadbeef",
    "an address type over a short value is its bytes: " + textOf(s),
  );

  const u = by(regWord("04"));
  assert(addrs(u).join() === REG_STRANGER, textOf(u));
  assert(type(u).length === 0, "no type shown: " + type(u).join());

  const writes = collect(regGroup(p, "Config changes"), "own-row");
  const types = writes.map((w) =>
    collect(value(w), "own-note").map((x) => x.textContent).join()
  );
  assert(types.join() === "address,bytes4", types.join());
});

Deno.test("root registry: only a 32-byte word with twelve zero bytes before it reads as an address, in either case", async () => {
  const f = regFake();
  const tail = REG_STRANGER.slice(2);
  const values = {
    "longer than a word": "0x" + "0".repeat(24) + tail + "00",
    "eleven and a half zero bytes": "0x" + "0".repeat(23) + "1" + tail,
    "no zero bytes": regWord("c1"),
    "an odd count of hex digits": "0xabc",
    "upper-case hex": "0x" + regAddressWord(REG_STRANGER).slice(2)
      .toUpperCase(),
  };
  Object.values(values).forEach((value, i) =>
    f.rows.configBindings.push({
      ...f.rows.configBindings[0],
      id: "b" + (i + 2),
      name: regWord("0" + (i + 2)),
      value,
      valueType: "address",
      metas: [],
    })
  );
  const p = await regPanel(f);
  const rows = collect(regGroup(p, "Config bindings"), "reg-binding");
  const shown = Object.keys(values).map((why, i) => {
    const r = rows.find((x) =>
      collect(x, "own-role")[0].textContent === regWord("0" + (i + 2))
    );
    const v = collect(r, "bcn-line").find((l) =>
      collect(l, "bcn-key")[0].textContent === "value"
    );
    return [
      why,
      collect(v, "own-addr").map((a) => a.textContent).join(),
      collect(v, "mig-hash").map((a) => a.textContent).join(),
    ];
  });
  const want = [
    ["longer than a word", "", values["longer than a word"]],
    [
      "eleven and a half zero bytes",
      "",
      values["eleven and a half zero bytes"],
    ],
    ["no zero bytes", "", regWord("c1")],
    ["an odd count of hex digits", "", "unreadable value"],
    ["upper-case hex", REG_STRANGER, ""],
  ];
  assert(
    JSON.stringify(shown) === JSON.stringify(want),
    JSON.stringify(shown),
  );
});

Deno.test("root registry: only the V5 registries are named; any other kind reads as itself", async () => {
  const f = regFake();
  f.rows.registryDelegates.push(
    {
      ...f.rows.registryDelegates[0],
      id: "d5",
      registry: { id: "0x" + "c3".repeat(20), kind: "CONFIG_REGISTRY" },
    },
    {
      ...f.rows.registryDelegates[0],
      id: "d6",
      registry: { id: "0x" + "d3".repeat(20), kind: "MIGRATION_REGISTRY" },
    },
    {
      ...f.rows.registryDelegates[0],
      id: "d7",
      registry: REG_CONFIG,
    },
  );
  const p = await regPanel(
    f,
    { configRegistries: {} },
    regScan({ networks: [] }),
  );
  const kinds = collect(regGroup(p, "Delegations in force"), "bcn-line")
    .filter((l) => collect(l, "bcn-key")[0].textContent === "in")
    .map((l) => collect(l, "own-note")[0].textContent);
  assert(
    kinds.join() ===
      [
        "MigrationRegistry V5",
        "registry (CONFIG_REGISTRY)",
        "registry (MIGRATION_REGISTRY)",
        "ConfigRegistry V5",
      ].join(),
    kinds.join(),
  );
});

Deno.test("root registry: a delegate in a registry of neither V5 kind is listed whichever registries are declared", async () => {
  const f = regFake();
  f.rows.registryDelegates.push({
    ...f.rows.registryDelegates[0],
    id: "d8",
    registry: { id: "0x" + "c4".repeat(20), kind: "VAULT_REGISTRY" },
  });
  const p = await regPanel(f);
  const kinds = collect(regGroup(p, "Delegations in force"), "bcn-line")
    .filter((l) => collect(l, "bcn-key")[0].textContent === "in")
    .map((l) => collect(l, "own-note")[0].textContent);
  assert(
    kinds.join() ===
      ["MigrationRegistry V5", "registry (VAULT_REGISTRY)"].join(),
    kinds.join(),
  );
});

Deno.test("root registry: an unscoped delegate holds every scope, in force and in the changes", async () => {
  const f = regFake();
  const unscoped = (id, registry, delegate, approved, block) => ({
    id,
    registry,
    account: REG_ROOT,
    delegate,
    approved,
    approvedAtBlock: String(block),
    approvedAtTimestamp: String(RBAC_TS - 100 + block),
    revokedAtBlock: approved ? null : String(block + 2),
    revokedAtTimestamp: approved ? null : String(RBAC_TS - 98 + block),
  });
  f.rows.registryUnscopedDelegates = [
    unscoped("u1", REG_MIGRATION, RBAC_TL, true, 22),
    unscoped("u2", REG_CONFIG, REG_STRANGER, false, 12),
  ];
  const change = (id, u, approved, block) => ({
    id,
    registryUnscopedDelegate: {
      registry: u.registry,
      account: u.account,
      delegate: u.delegate,
    },
    approved,
    blockNumber: String(block),
    blockTimestamp: String(RBAC_TS - 1000 + block),
    transactionHash: "0xtx" + id,
    transactionFrom: REG_ROOT,
    logIndex: "0",
  });
  const [u1, u2] = f.rows.registryUnscopedDelegates;
  f.rows.registryUnscopedDelegateChanges = [
    change("uc1", u1, true, 22),
    change("uc2", u2, true, 12),
    change("uc3", u2, false, 14),
  ];
  const p = await regPanel(f);
  const scope = (r) => collect(r, "own-role")[0].textContent;

  const live = collect(regGroup(p, "Delegations in force"), "reg-delegate");
  assert(live.length === 2, "the revoked unscoped approval is not in force");
  const u = live.find((r) => scope(r) === "every scope (unscoped)");
  assert(u, live.map(textOf).join(" | "));
  assert(
    textOf(u).includes(RBAC_TL) && textOf(u).includes("MigrationRegistry V5"),
    textOf(u),
  );
  assert(
    collect(u, "own-chip").map((c) => c.textContent).includes(
      "governance timelock",
    ),
    textOf(u),
  );
  assert(
    !textOf(u).includes(REG_NS),
    "an unscoped approval names no scope: " + textOf(u),
  );

  const rows = collect(regGroup(p, "Delegation changes"), "own-row");
  const chips = rows.map((r) => collect(r, "own-chip")[0].textContent);
  assert(
    chips.join() === "approved,approved,revoked,revoked,approved,approved",
    chips.join(),
  );
  const scopes = rows.map(scope);
  const U = "every scope (unscoped)";
  assert(
    scopes.join() ===
      [U, "st0x.deploy", "st0x.deploy", U, U, "st0x.deploy"].join(),
    scopes.join(),
  );
});

Deno.test("root registry: each line's head is the scan's headV5, keyed by writer and namespace", async () => {
  const f = regFake();
  f.rows.migrationLines[1].head = regWord("77");
  const p = await regPanel(f);
  const head = textOf(regLines(p)[0]);
  assert(head.includes(REG_M[2]) && head.includes("chain intact"), head);
  assert(!head.includes(regWord("77")), "the index's own head is not shown");
});

const regHeadLine = (l) =>
  collect(l, "bcn-line").find((x) =>
    collect(x, "bcn-key")[0].textContent === "head"
  );

Deno.test("root registry: a hash matches its line, its link and its label whatever its hex case", async () => {
  const upper = (h) => "0x" + h.slice(2).toUpperCase();
  const f = regFake();
  f.rows.migrationRecords.find((r) => r.id === "0xrc").appliedOnto = upper(
    REG_GENESIS,
  );
  f.rows.migrationRecords.find((r) => r.id === "0xrb").appliedOnto = upper(
    REG_M[0],
  );
  const scan = regScan();
  scan.networks[0].lines[0] = {
    ...scan.networks[0].lines[0],
    namespace: upper(REG_NS),
    headBlock: 41,
    head: upper(REG_M[1]),
  };
  const p = await regPanel(f, undefined, scan);
  assert(regHeads(p).includes("Migration lines — 2"), regHeads(p));
  const a = regLines(p)[0];
  assert(collect(p, "reg-problem").length === 0, textOf(p));
  assert(textOf(a).includes("3 records, chain intact"), textOf(a));
  assert(
    textOf(regHeadLine(a)).includes(REG_M1_LABEL) &&
      !textOf(regHeadLine(a)).includes(upper(REG_M[1])),
    "the head takes its label: " + textOf(regHeadLine(a)),
  );
  const onto = collect(regRecs(p)[0], "bcn-line").find((x) =>
    collect(x, "bcn-key")[0].textContent === "applied onto"
  );
  assert(textOf(onto).includes("genesis"), textOf(onto));
});

Deno.test("root registry: a head the scan did not read shows as not read, never as a problem", async () => {
  const absent = regScan();
  absent.networks[0].lines = absent.networks[0].lines.slice(1);
  const unread = regScan();
  unread.networks[0].lines[0] = {
    ...unread.networks[0].lines[0],
    head: null,
    headBlock: null,
  };
  const noBlock = regScan();
  delete noBlock.networks[0].lines[0].headBlock;
  const noHash = regScan();
  noHash.networks[0].lines[0] = { ...noHash.networks[0].lines[0], head: 5 };
  const elsewhere = regScan();
  elsewhere.networks[0].lines = absent.networks[0].lines;
  elsewhere.networks.unshift({
    ...regScan().networks[0],
    chainId: 56,
    network: "bsc",
  });
  for (
    const [why, scan] of [
      ["not in the scan", absent],
      ["unread", unread],
      ["read at no block", noBlock],
      ["given as no hash", noHash],
      ["read on another network", elsewhere],
    ]
  ) {
    const q = await regPanel(regFake(), undefined, scan);
    const [a, b] = regLines(q);
    assert(
      collect(a, "reg-problem").length === 0,
      why + ": " +
        collect(a, "reg-problem").map((x) => x.textContent).join(" | "),
    );
    const h = textOf(regHeadLine(a));
    assert(h.includes("not read") && !h.includes(REG_M[2]), why + ": " + h);
    assert(
      textOf(a).includes("3 records, chain intact"),
      why + ": " + textOf(a),
    );
    assert(
      collect(b, "reg-problem").length === 0 &&
        textOf(regHeadLine(b)).includes(regWord("98")),
      why + ": the other line keeps its head",
    );
  }
  const noLines = regScan();
  delete noLines.networks[0].lines;
  const n = await regPanel(regFake(), undefined, noLines);
  assert(regLines(n).length === 2, regHeads(n));
  assert(
    regLines(n).every((l) => textOf(regHeadLine(l)).includes("not read")) &&
      collect(n, "reg-problem").length === 0,
    "a network the scan gives no lines reads no head: " + textOf(n),
  );
});

Deno.test("root registry: a head is checked against the records up to the block the scan read it at", async () => {
  // Line A gains a fourth record at block 60, after the scan's block 50.
  const later = () => {
    const f = regFake();
    f.rows.migrationRecords.push(
      regRecord("0xr9", REG_LINE, 3, regWord("e4"), REG_M[2], {
        line: { id: REG_LINE, namespace: REG_NS },
        blockNumber: "60",
      }),
    );
    f.rows.migrationLines[1] = {
      ...f.rows.migrationLines[1],
      head: regWord("e4"),
      recordCount: 4,
      headRecord: { id: "0xr9" },
    };
    return f;
  };
  const at = (headBlock, head) => {
    const scan = regScan();
    scan.networks[0].lines[0] = {
      ...scan.networks[0].lines[0],
      headBlock,
      head,
    };
    return scan;
  };
  for (
    const [why, headBlock, head] of [
      ["a record after the scan's block", REG_HEAD_BLOCK, REG_M[2]],
      ["the block just before that record", 59, REG_M[2]],
      ["a record at the scan's block", 60, regWord("e4")],
      ["read before the line's first record", 30, REG_GENESIS],
    ]
  ) {
    const p = await regPanel(later(), undefined, at(headBlock, head));
    const a = regLines(p)[0];
    assert(
      collect(a, "reg-problem").length === 0,
      why + ": " +
        collect(a, "reg-problem").map((x) => x.textContent).join(" | "),
    );
    assert(
      textOf(a).includes("4 records, chain intact"),
      why + ": " + textOf(a),
    );
    assert(
      textOf(regHeadLine(a)).includes(head === REG_GENESIS ? "genesis" : head),
      why,
    );
  }
  for (
    const [why, headBlock, head, want] of [
      ["the record at the scan's block", 60, REG_M[2], regWord("e4")],
      ["before the line's first record", 30, REG_M[0], "genesis"],
      ["genesis after the line's records", 60, REG_GENESIS, regWord("e4")],
    ]
  ) {
    const p = await regPanel(later(), undefined, at(headBlock, head));
    const a = regLines(p)[0];
    const probs = collect(a, "reg-problem").map((x) => x.textContent);
    assert(
      probs.join(" | ") ===
        `the head ${head} is not the migration of the last record at or before block ${headBlock}`,
      why + " (" + want + "): " + probs.join(" | "),
    );
    assert(
      textOf(a).includes("4 records, chain broken"),
      why + ": " + textOf(a),
    );
  }
});

Deno.test("root registry: a record the index gives no block or no migration never confirms the head", async () => {
  const problems = async (field, head) => {
    const f = regFake();
    f.rows.migrationRecords.find((r) => r.id === "0xra")[field] = null;
    const scan = regScan();
    scan.networks[0].lines[0].head = head;
    const a = regLines(await regPanel(f, undefined, scan))[0];
    return collect(a, "reg-problem").map((x) => x.textContent).join(" | ");
  };
  const wrong = (head) =>
    `the head ${head} is not the migration of the last record at or before block ${REG_HEAD_BLOCK}`;
  const noBlockPrior = await problems("blockNumber", REG_M[1]);
  assert(
    noBlockPrior === "",
    "the record before it is the last read: " + noBlockPrior,
  );
  const noBlockOwn = await problems("blockNumber", REG_M[2]);
  assert(noBlockOwn === wrong(REG_M[2]), noBlockOwn);
  const noMigration = await problems("migration", REG_M[2]);
  assert(noMigration === wrong(REG_M[2]), noMigration);
});

Deno.test("root registry: a head read past the index's block is behind, never broken; at the index's block it is compared", async () => {
  // On chain line A gains a fourth record at block 60; the index holds only
  // its three records at blocks 40-42 and line B's at block 40.
  const read = (indexed) => {
    const f = regFake();
    f.meta.data._meta.block.number = indexed;
    return f;
  };
  const at = (headBlock, head) => {
    const scan = regScan();
    scan.networks[0].lines[0] = {
      ...scan.networks[0].lines[0],
      headBlock,
      head,
    };
    return scan;
  };
  const note = (indexed, headBlock) =>
    `subgraph behind the scan (indexed to block ${indexed}, head read at block ${headBlock})`;
  for (
    const [why, indexed, headBlock, head] of [
      ["a record the index has not reached", 55, 60, regWord("e4")],
      ["one block past the index", 59, 60, regWord("e4")],
      ["a head both reads agree on", 55, 60, REG_M[2]],
    ]
  ) {
    const p = await regPanel(read(indexed), undefined, at(headBlock, head));
    const [a, b] = regLines(p);
    assert(
      collect(p, "reg-problem").length === 0,
      why + ": " +
        collect(p, "reg-problem").map((x) => x.textContent).join(" | "),
    );
    assert(
      textOf(a).includes("3 records, chain intact"),
      why + ": " + textOf(a),
    );
    assert(textOf(regHeadLine(a)).includes(head), why + ": " + textOf(a));
    assert(
      textOf(a).includes(note(indexed, headBlock)),
      why + ": " + textOf(a),
    );
    assert(
      !textOf(b).includes("subgraph behind the scan") &&
        textOf(b).includes("1 record, chain intact"),
      why + ": a head read within the index is not behind: " + textOf(b),
    );
  }
  const p = await regPanel(read(60), undefined, at(60, regWord("e4")));
  const a = regLines(p)[0];
  const probs = collect(a, "reg-problem").map((x) => x.textContent);
  assert(
    probs.join(" | ") ===
      `the head ${
        regWord("e4")
      } is not the migration of the last record at or before block 60`,
    probs.join(" | "),
  );
  assert(textOf(a).includes("3 records, chain broken"), textOf(a));
  assert(!textOf(a).includes("subgraph behind the scan"), textOf(a));
});

Deno.test("root registry: a declared line the scan reads at genesis, with nothing indexed, reads as not yet backfilled", async () => {
  const f = regFake();
  f.rows.migrationLines = [];
  f.rows.migrationRecords = [];
  const scan = regScan();
  scan.networks[0].lines = [
    {
      writer: REG_ROOT_CHECKSUM,
      namespace: REG_NS,
      headBlock: REG_HEAD_BLOCK,
      head: REG_GENESIS,
    },
  ];
  const p = await regPanel(f, undefined, scan);
  const lines = regLines(p);
  assert(lines.length === 1, "the scan's line is listed: " + textOf(p));
  const t = textOf(lines[0]);
  assert(
    t.includes(REG_NS) && t.toLowerCase().includes(REG_ROOT),
    t,
  );
  assert(t.includes("its history is not yet backfilled"), t);
  assert(
    collect(p, "reg-problem").length === 0 && !t.includes("chain broken"),
    "nothing called broken: " + t,
  );

  const g = regFake();
  g.rows.migrationLines = g.rows.migrationLines.filter((l) =>
    l.id === REG_LINE_B
  );
  g.rows.migrationRecords = g.rows.migrationRecords.filter((r) =>
    r.line.id === REG_LINE_B
  );
  const both = regScan();
  both.networks[0].lines[0].head = REG_GENESIS;
  const q = await regPanel(g, undefined, both);
  assert(regHeads(q).includes("Migration lines — 2"), regHeads(q));
  const [a, b] = regLines(q);
  assert(
    textOf(a).includes(REG_NS) &&
      textOf(a).includes("its history is not yet backfilled") &&
      textOf(a).includes("0 records, chain intact"),
    "beside an indexed line: " + textOf(a),
  );
  assert(
    textOf(b).includes(REG_NS_B) &&
      textOf(b).includes("1 record, chain intact") &&
      !textOf(b).includes("not yet backfilled"),
    textOf(b),
  );
  assert(collect(q, "reg-problem").length === 0, textOf(q));
});

Deno.test("root registry: an indexed line the scan reads at genesis still names a head record the read does not hold", async () => {
  for (
    const [count, want] of [
      [0, "the head record is not the last record"],
      [
        3,
        "the line counts 3 records; the read holds 0 | the head record is not the last record",
      ],
    ]
  ) {
    const f = regFake();
    f.rows.migrationLines[1] = {
      ...f.rows.migrationLines[1],
      recordCount: count,
      headRecord: { id: "0xra" },
    };
    f.rows.migrationRecords = f.rows.migrationRecords.filter((r) =>
      r.line.id === REG_LINE_B
    );
    const scan = regScan();
    scan.networks[0].lines[0].head = REG_GENESIS;
    const a = regLines(await regPanel(f, undefined, scan))[0];
    assert(textOf(a).includes(REG_NS), textOf(a));
    const probs = collect(a, "reg-problem").map((x) => x.textContent);
    assert(probs.join(" | ") === want, count + ": " + probs.join(" | "));
    assert(
      textOf(a).includes("0 records, chain broken"),
      count + ": " + textOf(a),
    );
  }
});

Deno.test("root registry: a line only the scan holds, its head unread, is not read rather than broken", async () => {
  const f = regFake();
  f.rows.migrationLines = f.rows.migrationLines.filter((l) =>
    l.id === REG_LINE_B
  );
  f.rows.migrationRecords = f.rows.migrationRecords.filter((r) =>
    r.line.id === REG_LINE_B
  );
  const scan = regScan();
  scan.networks[0].lines[0] = {
    ...scan.networks[0].lines[0],
    head: null,
    headBlock: null,
  };
  const p = await regPanel(f, undefined, scan);
  assert(regHeads(p).includes("Migration lines — 2"), regHeads(p));
  const a = regLines(p)[0];
  assert(textOf(a).includes(REG_NS), textOf(a));
  assert(textOf(regHeadLine(a)).includes("not read"), textOf(a));
  assert(
    collect(a, "reg-problem").length === 0 &&
      !textOf(a).includes("chain broken"),
    "an unread head breaks nothing: " + textOf(a),
  );
  assert(
    !textOf(a).includes("not yet backfilled"),
    "an unread head is not genesis: " + textOf(a),
  );
});

Deno.test("root registry: a line's chip is green only for an intact chain", async () => {
  const chipOf = (l) => collect(regHeadLine(l), "own-chip")[0].className;
  const intact = regLines(await regPanel(regFake()))[0];
  assert(textOf(intact).includes("chain intact"), textOf(intact));
  assert(chipOf(intact) === "own-chip own-chip-yes", chipOf(intact));
  const f = regFake();
  f.rows.migrationRecords.find((r) => r.id === "0xrb").appliedOnto = regWord(
    "77",
  );
  const broken = regLines(await regPanel(f))[0];
  assert(textOf(broken).includes("chain broken"), textOf(broken));
  assert(chipOf(broken) === "own-chip own-chip-no", chipOf(broken));
  const unchecked = regLines(
    await regPanel(regFake(), undefined, regScan({ genesis: undefined })),
  )[0];
  assert(textOf(unchecked).includes("chain unchecked"), textOf(unchecked));
  assert(chipOf(unchecked) === "own-chip own-chip-no", chipOf(unchecked));
});

Deno.test("root registry: a line the scan read at genesis before its first record is not called unbackfilled", async () => {
  const scan = regScan();
  scan.networks[0].lines[0] = {
    ...scan.networks[0].lines[0],
    headBlock: 39,
    head: REG_GENESIS,
  };
  const a = regLines(await regPanel(regFake(), undefined, scan))[0];
  assert(textOf(a).includes("3 records, chain intact"), textOf(a));
  assert(collect(a, "reg-problem").length === 0, textOf(a));
  assert(!textOf(a).includes("not yet backfilled"), textOf(a));
});

Deno.test("root registry: a line with no records shows no record table", async () => {
  const f = regFake();
  f.rows.migrationLines = f.rows.migrationLines.filter((l) =>
    l.id === REG_LINE_B
  );
  f.rows.migrationRecords = f.rows.migrationRecords.filter((r) =>
    r.line.id === REG_LINE_B
  );
  const scan = regScan();
  scan.networks[0].lines[0].head = REG_GENESIS;
  const p = await regPanel(f, undefined, scan);
  const [a, b] = regLines(p);
  assert(textOf(a).includes("0 records"), textOf(a));
  assert(textOf(b).includes("1 record,"), textOf(b));
  const kids = collect(p, "rbac-net")[0].children;
  const next = (n) => kids[kids.indexOf(n) + 1];
  assert(next(a) === b, "no table between: " + textOf(next(a)));
  assert(
    next(b).className === "own-table" && regRecs(next(b)).length === 1,
    textOf(next(b)),
  );
  assert(kids.indexOf(next(b)) === kids.length - 1, "nothing after");
});

Deno.test("root registry: a record names who recorded it only when that is not the line's writer, whatever the case", async () => {
  const f = regFake();
  const rec = (id) => f.rows.migrationRecords.find((r) => r.id === id);
  rec("0xrc").sender = REG_ROOT_CHECKSUM;
  rec("0xra").sender = REG_ROOT;
  const p = await regPanel(f);
  const by = regRecs(p).slice(0, 3).map((r) =>
    collect(r, "bcn-line").filter((x) =>
      collect(x, "bcn-key")[0].textContent === "recorded by"
    ).map(textOf).join()
  );
  assert(by[0] === "" && by[2] === "", by.join(" | "));
  assert(by[1].includes(RBAC_TL), by.join(" | "));
});

Deno.test("root registry: each record shows when it was applied, if the index says", async () => {
  const f = regFake();
  const rec = (id) => f.rows.migrationRecords.find((r) => r.id === id);
  rec("0xrc").appliedAt = String(RBAC_TS - 3 * 86400);
  rec("0xra").appliedAt = null;
  const p = await regPanel(f);
  const at = regRecs(p).slice(0, 3).map((r) =>
    collect(collect(r, "own-prov")[0], "own-note").map((n) => n.textContent)
      .filter((t) => t.startsWith("applied")).join()
  );
  assert(
    at.join(" | ") ===
      "applied 2026-10-10 09:29 UTC | applied 2026-10-13 08:06 UTC | applied time unknown",
    at.join(" | "),
  );
});

Deno.test("root registry: an absent, unreadable or empty subgraph location says so and reads nothing", async () => {
  const off = async (rbac) => {
    const f = regFake();
    const data = { ...grantsData(), deploymentMigrations: regScan() };
    if (rbac !== undefined) data.deploymentRbac = rbac;
    const { box, done } = deploymentsRender(data, f.io);
    await done;
    assert(f.seen.length === 0, "nothing read: " + f.seen.length);
    const t = textOf(
      collect(box, "tabpanel").find((x) => x.dataset.tab === "registry"),
    );
    assert(!t.includes("Read live from"), t);
    return t;
  };
  for (const r of [undefined, null, "st0x-rbac", 5, true]) {
    const t = await off(r);
    assert(
      t.includes(
        "Registry history unavailable — this scan carries no subgraph location.",
      ),
      String(r) + ": " + t,
    );
  }
  const why = await off(
    rbacData(undefined, {
      unavailable: "could not read subgraph/networks.json",
    }).deploymentRbac,
  );
  assert(
    why.includes(
      "Registry history unavailable — the scan could not read subgraph/networks.json.",
    ),
    why,
  );
  const f = regFake();
  const { box, done } = deploymentsRender(
    { ...rbacData([]), deploymentMigrations: regScan() },
    f.io,
  );
  await done;
  const empty = textOf(
    collect(box, "tabpanel").find((x) => x.dataset.tab === "registry"),
  );
  assert(
    empty.includes(
      "Registry history unavailable — the subgraph declares no networks.",
    ),
    empty,
  );
  assert(f.seen.length === 0, "nothing read for no networks");
});

Deno.test("root registry: a network the scan gives no https endpoint is an error and is never read", async () => {
  for (const endpoint of ["http://q.example/gn", undefined]) {
    const f = regFake();
    const p = await regPanel(
      f,
      undefined,
      regScan(),
      rbacNet({ network: "base", chain: "base", endpoint }),
    );
    assert(
      regErr(p).includes("the scan gave no https endpoint for this network"),
      String(endpoint) + ": " + regErr(p),
    );
    assert(!textOf(p).includes("Reading the index"), textOf(p));
    assert(f.seen.length === 0, "never read: " + f.seen.length);
  }
});

Deno.test("root registry: each network is titled by its chain, the subgraph's name beside it where they differ", async () => {
  const titles = async (net) =>
    collect(await regPanel(regFake(), undefined, regScan(), net), "tok-h3")
      .map((h) => h.textContent).join(" | ");
  assert((await titles(REG_NET)) === "base", await titles(REG_NET));
  assert(
    (await titles(rbacNet())) === "ethereum (mainnet)",
    await titles(rbacNet()),
  );
  assert(
    (await titles(rbacNet({ chain: null }))) === "mainnet",
    await titles(rbacNet({ chain: null })),
  );
});

Deno.test("root registry: the scan's genesis checks the first record and labels it whatever its hex case", async () => {
  const upper = "0x" + REG_GENESIS.slice(2).toUpperCase();
  const p = await regPanel(regFake(), undefined, regScan({ genesis: upper }));
  const [a, b] = regLines(p);
  assert(textOf(a).includes("3 records, chain intact"), textOf(a));
  assert(textOf(b).includes("1 record, chain intact"), textOf(b));
  assert(collect(p, "reg-problem").length === 0, textOf(p));
  const onto = collect(regRecs(p)[0], "bcn-line").find((x) =>
    collect(x, "bcn-key")[0].textContent === "applied onto"
  );
  assert(textOf(onto) === "applied ontogenesis", textOf(onto));
});

Deno.test("root registry: the scan's ConfigRegistry is matched whatever its hex case; one given as no address declares none", async () => {
  const has = (p, want) => {
    const heads = regHeads(p);
    for (const h of want) assert(heads.includes(h), h + ": " + heads);
  };
  const upper = "0x" + REG_CONFIG.id.slice(2).toUpperCase();
  const p = await regPanel(regElsewhere(regFake()), {
    configRegistries: { base: upper },
  });
  has(p, ["Config bindings — 1", "Config changes — 2, newest first"]);
  assert(!textOf(p).includes(regWord("b9")), "the other registry's binding");
  for (const a of [null, 5]) {
    const q = await regPanel(regElsewhere(regFake()), {
      configRegistries: { base: a },
    });
    has(q, ["Config bindings — 2", "Config changes — 3, newest first"]);
  }
});

Deno.test("root registry: a network the scan declares no MigrationRegistry for reads every migration line, its heads still applied", async () => {
  for (const registry of [null, undefined, 5]) {
    const scan = regScan();
    scan.networks[0].registry = registry;
    if (registry === undefined) delete scan.networks[0].registry;
    const p = await regPanel(regElsewhere(regFake()), undefined, scan);
    assert(
      regHeads(p).includes("Migration lines — 3"),
      String(registry) + ": " + regHeads(p),
    );
    assert(regRecs(p).length === 5, "the other registry's record too");
    const b = regLines(p).find((l) => textOf(l).includes(REG_NS_B));
    assert(
      textOf(regHeadLine(b)).includes(regWord("98")),
      String(registry) + ": " + textOf(b),
    );
  }
});

Deno.test("root registry: a scan line without a string writer or namespace is no line", async () => {
  const scan = regScan();
  scan.networks[0].lines.push(
    {
      writer: REG_ROOT_CHECKSUM,
      namespace: null,
      headBlock: REG_HEAD_BLOCK,
      head: regWord("55"),
    },
    {
      writer: null,
      namespace: regWord("56"),
      headBlock: REG_HEAD_BLOCK,
      head: regWord("57"),
    },
  );
  const p = await regPanel(regFake(), undefined, scan);
  assert(regHeads(p).includes("Migration lines — 2"), regHeads(p));
  assert(
    !textOf(p).includes(regWord("55")) && !textOf(p).includes(regWord("57")),
    textOf(p),
  );
});

Deno.test("root registry: the page settles only once every network's registry read has rendered, with or without the owners", async () => {
  for (const owners of [OWNERS.deploymentOwners, null]) {
    const f = regFake();
    // The registry's schema probe answers after the role reads have settled.
    const io = {
      now: f.io.now,
      fetch: (url, init) =>
        JSON.parse(init.body).query.includes("ConfigBinding")
          ? new Promise((r) => setTimeout(r, 5)).then(() =>
            f.io.fetch(url, init)
          )
          : f.io.fetch(url, init),
    };
    const { box, done } = deploymentsRender({
      ...rbacData([REG_NET], { configRegistries: { base: REG_CONFIG.id } }),
      deploymentMigrations: regScan(),
      deploymentOwners: owners,
    }, io);
    await done;
    const p = collect(box, "tabpanel").find((x) =>
      x.dataset.tab === "registry"
    );
    const why = owners ? "owners: " : "no owners: ";
    assert(!textOf(p).includes("Reading the index"), why + textOf(p));
    assert(
      regHeads(p).includes("Delegations in force — 1"),
      why + regHeads(p),
    );
  }
});
