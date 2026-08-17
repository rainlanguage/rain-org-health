---
paths:
  - "site/**"
  - "test/**"
---

# Dashboard pages

## Rendering untrusted data

The dashboard renders cross-repo, attacker-influenceable strings (repo names,
git tags, PR/issue titles, producer reasons, token names read off-chain). Build
the DOM directly — `createElement` / `createElementNS` / `textContent` /
`.append` / the `.href` property — **never** `innerHTML` string-building with a
hand-rolled escaper (escaping is context-dependent and fragile; see
rainlanguage/claude-audit-skills#44). DOM construction escapes by construction.

No page assigns a markup string anywhere, and `test/dashboard.test.js` enforces
it: one test greps every `site/*.html` for
`innerHTML`/`outerHTML`/`insertAdjacentHTML`/`document.write`, and others drive
the real renderers with payloads (`<img src=x onerror=…>`, `<script>…`,
quote-and-angle-bracket strings) and assert the payload lands as a text node.
That is the point — a page with no markup sink has nothing to forget to escape,
so adding a section cannot reintroduce the hazard. The SVG chart in
`metrics.html` is built the same way (`createElementNS` + `setAttribute`), not
as an `innerHTML` string.

## Nothing is fetched from a third-party host at runtime

A library the pages genuinely need (the ELK layout engine) is vendored into
`site/vendor/` (byte-identical to its published release, prettier-ignored) and
loaded from the same origin — never a CDN.

## Pan and zoom are not bound in JS

The browser scrolls and pinch-zooms natively, and its zoom focuses correctly
because it is the thing reading the gesture. Binding those in JS requires
`touch-action: none`, which suppresses the real pinch-zoom to reimplement it
worse. BUTTONS are a different thing and are wanted: the audit graph's zoom
in/out/fit buttons re-apply the same static fit scale the page already computes,
bind no gesture, and set no `touch-action` — so the browser's own pinch and
scroll keep working untouched. The rejection is of gesture binding, not of a
visible control.

## Never run `deno fmt` over `site/*.html`

It reindents the inline script and breaks the column-0 function extraction the
tests use. `deno fmt` owns `test/` only; prettier owns the pages.

## Verifying a change

No gate renders a page: `site-test.yml` (`nix run .#dashboard-test`) covers
render logic under a DOM stub, never appearance. See a visual change before
claiming it — `nix run .#screenshot -- site out.png [page]`.
