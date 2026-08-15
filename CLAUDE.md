# rain-org-health

Two things: a **scanner** (`roh-scan`) that sweeps a GitHub org for
rainix/soldeer modernization debt, and a static **dashboard** (`site/`) that
reports it — plus the issue→PR pipeline's FSM-conformance state. Also packaged
as a Claude plugin (`.claude-plugin/`, `plugins/rain-org-health-check/`).

## Layout

- `plugins/rain-org-health-check/roh-scan/` — the scanner (Rust). Signal
  detection is `signals.rs` (pure, unit + mutation tested, run in-build via
  `doCheck`); `main.rs` is the `gh`/network orchestration + output;
  audit-recency parsing is `audit.rs`; the untested-external-surface check
  (concrete contracts' external/public functions no test names) is
  `untested.rs`, fed from a per-repo shallow clone. The crate is a workspace
  member but `Cargo.toml`/`Cargo.lock` live at the repo root, so builds root
  there.
- `site/` — the dashboard: one self-contained page per view (inline CSS/JS) plus
  `health.json`. `pipeline.html` is the landing page; `index.html` is a redirect
  to it, kept because Pages serves it as the site root. Deployed by `pages.yml`.
  Nothing is fetched from a CDN or any third-party host at runtime: a library
  the pages genuinely need (the ELK layout engine) is vendored into
  `site/vendor/` (byte-identical to its published release, prettier-ignored) and
  loaded from the same origin. Pan and zoom are not such a need — the browser
  scrolls and pinch-zooms natively, and its zoom focuses correctly because it is
  the thing reading the gesture. Binding those in JS requires
  `touch-action: none`, which suppresses the real pinch-zoom to reimplement it
  worse. BUTTONS are a different thing and are wanted: the audit graph's zoom
  in/out/fit buttons re-apply the same static fit scale the page already
  computes, bind no gesture, and set no `touch-action` — so the browser's own
  pinch and scroll keep working untouched. The rejection is of gesture binding,
  not of a visible control.
- `flake.nix` — `packages.roh-scan`, `packages.screenshot`,
  `packages.default = roh-scan`; `devShells.default` composes rainix's devshell
  (so `pre-commit run --all-files` reproduces CI's static suite).

## Build / test / run

```
nix run .#roh-scan -- --help                 # every mode, flag, env var, exit status
nix run .#roh-scan                          # scan the whole org (default ORG=rainlanguage) → writes site/health.json
nix run .#roh-scan -- rain.dia rain.flare   # scan specific repos
nix run .#roh-scan -- --json /tmp/h.json     # write elsewhere
nix run .#roh-scan -- consumers rain-solmem --symbol unsafeList   # reverse dependency + symbol lookup
nix run .#screenshot -- site out.png         # headless render of the dashboard (pinned chromium + fonts)
nix develop -c cargo test                    # roh-scan unit + mutation tests
nix develop -c pre-commit run --all-files    # reproduces CI's rs-static suite (rustfmt/clippy/prettier/nixfmt/…)
```

`roh-scan` env: `ORG` (default `rainlanguage`), `PAR` (parallelism, default 12),
`JSON_OUT` (default `site/health.json` — a bare run POPULATES it, never
print-and-discards). Read-only on GitHub; the caller supplies an org-read-authed
`gh`. It also reads each repo's `.audit/last-run.json` whole-repo stamp for the
audit-recency column.

**`--help` is the reference material**, not a reminder — the CLI is parsed in
`cli.rs` as a pure `argv -> Command` function, and a test asserts the help text
names every mode, flag, env var and manifest shape. Anything unrecognised (an
unknown flag, a repo that does not exist) exits 2. Never document a mode only in
a prompt or a skill file: if `--help` does not have it, the tool is
undiscoverable and someone hand-rolls a shell loop instead.

## Never answer a cross-repo question from ONE manifest shape

`consumers.rs` exists because a repo's Solidity dependencies live in at least
five places and no repo uses all of them: `foundry.toml` `[dependencies]` (and
`remappings` inside a `[profile.*]`), `soldeer.lock`, `foundry.lock`,
`remappings.txt`, `.gitmodules`. A sweep of one shape under-returns silently —
grepping `foundry.lock` misses `rainlanguage/rainlang`, which has none at all.
Every shape is parsed and unioned, names are normalized (`rain.solmem` ==
`rain-solmem` == `lib/rain.solmem`), and a manifest that will not parse is an
ERROR carried into the report, never an empty dependency list.

Two more rules that hold for anything added here:

- **GitHub code search is not a source of truth.** It indexes default branches
  only and under-returns without saying so. Measured: a search for the package
  name across the org returned 11 of the 17 rainlanguage repos that actually
  declare it. Five of the six it dropped (`rain.dia`, `rain.erc4626.words`,
  `rain.intorastring`, `rain.merkle`, `rain.verify`) carry the literal string in
  their default-branch `foundry.toml` — so it is not a spelling problem and not
  a branch problem, the index simply does not have them, and it answers 200
  anyway. Treat the counts as a snapshot; the durable property is that a miss is
  silent. Read manifests and clone sources instead.
- **`gh api …/contents/<path>` prints its 404 body to STDOUT.** An existence
  check that tests output emptiness reports every file as present. Check the
  exit status — `GhApi::api_jq` already does, which is why it returns a typed
  `Found`/`NotFound`/`Failed` rather than an `Option<String>`.

## CI

`rust.yml` builds + tests `roh-scan` (and runs the rainix static suite).
`pages.yml` deploys `site/` on pushes that touch `site/**`. There is no separate
site-lint gate — verify the page by rendering it (`nix run .#screenshot`).

## The dashboard is a CONSUMER of data, never a PRODUCER

`site/` is a pure presentation layer: it **fetches JSON artifacts at runtime and
renders them** — it never generates data, shells out to tools, or reaches into
another repo's tooling. Every data source is owned + emitted by its
**producer**:

| data                             | produced by                                                                  | how the dashboard gets it    |
| -------------------------------- | ---------------------------------------------------------------------------- | ---------------------------- |
| repo modernization signals       | `roh-scan` (this repo) → `health.json`                                       | same-origin fetch            |
| producer-run metrics             | `issue-pr-cron` → `metrics/runs.jsonl`                                       | runtime fetch of its raw URL |
| pipeline / FSM-conformance state | `issue-pr-cron`'s `pr-review-report human-queue --json` → `human-queue.json` | runtime fetch of its raw URL |

Do not regress this:

- **The dashboard must not compute pipeline state.** It does NOT call
  `pr-review-report`, and `roh-scan` does NOT call it either — the pipeline repo
  emits `human-queue.json` on its own cron and the dashboard `fetch()`es it. A
  stale FSM panel is fixed in **issue-pr-cron's refresh**, not here.
- **Data changes must never require a Pages redeploy.** Keep frequently-changing
  data out of the `pages.yml` deploy path (`site/**`) — fetch it at runtime
  (`raw.githubusercontent.com` serves `access-control-allow-origin: *`). The
  site redeploys only when the **presentation** changes.
- **New data source ⇒ new fetch, not new baking.** Have the producing repo
  commit an artifact; fetch it here. Never embed/generate the data in this repo.

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

`health.json` is `.prettierignore`d (serde-generated); `test/` is formatted by
`deno fmt`, not prettier. Never run `deno fmt` over `site/*.html` — it reindents
the inline script and breaks the column-0 function extraction the tests use.
