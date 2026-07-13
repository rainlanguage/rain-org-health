// Dashboard render-logic test harness (deno test).
//
// The dashboard pages are self-contained inline JS (CLAUDE.md: no external
// scripts), so there is nothing to `import`. Instead this harness EXTRACTS the
// real `renderAudit` from the shipped `site/audit.html` and runs it against
// fixtures under a minimal DOM stub — so the tests track the deployed code
// without a browser or any third-party dependency.
//
// Run: `deno test --allow-read test/` (see .github/workflows/site-test.yml).

function assert(cond, msg) {
  if (!cond) throw new Error("assertion failed: " + msg);
}

// A DOM element stub covering only what renderAudit touches: className, a
// separate classList, textContent (setting it clears children, like the DOM),
// append/appendChild, and arbitrary assignable props (href/rel/target/style).
function makeEl(tag) {
  const el = {
    tagName: String(tag).toLowerCase(),
    className: "",
    _text: undefined,
    children: [],
    style: {},
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

// Pull a top-level `function NAME() { ... }` out of the page's inline script by
// column-0 brace matching (nested braces are indented, so the first `^}$` after
// the header is the function's own close).
function extractFn(script, name) {
  const re = new RegExp("^function " + name + "\\(\\)[\\s\\S]*?^\\}$", "m");
  const m = re.exec(script);
  if (!m) throw new Error("could not extract function " + name);
  return m[0];
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

function renderAuditWith(protofireAudits) {
  const html = Deno.readTextFileSync(new URL("../site/audit.html", import.meta.url));
  const script = html.replace(/^[\s\S]*?<script type="module">/, "").replace(/<\/script>[\s\S]*$/, "");
  const src = extractFn(script, "renderAudit");
  const box = makeEl("div");
  const document = { createElement: (t) => makeEl(t) };
  const $ = (id) => (id === "audit" ? box : makeEl("div"));
  const data = { org: "testorg", protofireAudits, reposNeverExternallyAudited: 0 };
  const renderAudit = new Function("document", "$", "data", src + "\nreturn renderAudit;")(document, $, data);
  renderAudit();
  return box;
}

Deno.test("audit drift cell is red ONLY when the line drift is unenumerable", () => {
  const box = renderAuditWith([
    // Truncated compare → tree-diff file count, no line +/- : UNENUMERABLE → red.
    {
      name: "unenumerable",
      hasProtofireAudit: true,
      externalAudit: "stale",
      sourceDriftTruncated: true,
      filesChangedSinceAudit: 47,
      commitsSinceAudit: 687,
      compareUrl: "https://github.com/x/y/compare/a...b",
    },
    // A large but ENUMERATED diff must stay neutral, however large.
    {
      name: "enumerated-large",
      hasProtofireAudit: true,
      externalAudit: "stale",
      sourceLocAddedSinceAudit: 9000,
      sourceLocRemovedSinceAudit: 8000,
      filesChangedSinceAudit: 200,
      commitsSinceAudit: 500,
      compareUrl: "https://github.com/x/y/compare/v1...b",
    },
    // Truncated but zero .sol changed → "no Solidity drift": not red.
    {
      name: "nodrift",
      hasProtofireAudit: true,
      externalAudit: "stale",
      sourceDriftTruncated: true,
      filesChangedSinceAudit: 0,
      commitsSinceAudit: 5,
    },
  ]);

  const drifts = collect(box, "au-drift");
  assert(drifts.length === 3, `expected 3 drift cells, got ${drifts.length}`);

  const red = drifts.filter((d) => d.classList.contains("big"));
  assert(red.length === 1, `exactly one drift cell must be red, got ${red.length}`);

  const unenum = drifts.filter((d) => textOf(d).includes("line drift too large to size"));
  assert(
    unenum.length === 1 && unenum[0].classList.contains("big"),
    "the unenumerable (too-large-to-size) cell must be red",
  );

  const enumerated = drifts.filter((d) => textOf(d).includes("src LOC"));
  assert(
    enumerated.length === 1 && !enumerated[0].classList.contains("big"),
    "a large but enumerated +/- diff must NOT be red",
  );

  const noDrift = drifts.filter((d) => textOf(d).includes("no Solidity drift"));
  assert(
    noDrift.length === 1 && !noDrift[0].classList.contains("big"),
    "a zero-drift (no Solidity change) cell must NOT be red",
  );
});
