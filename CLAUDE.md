# rain-org-health

Two things: a **scanner** (`roh-scan`) that sweeps a GitHub org for
rainix/soldeer modernization debt, and a static **dashboard** (`site/`) that
reports it — plus the issue→PR pipeline's FSM-conformance state. Also packaged
as a Claude plugin (`.claude-plugin/`, `plugins/rain-org-health-check/`).

## Layout

- `plugins/rain-org-health-check/roh-scan/` — the scanner (Rust). Signal
  detection is `signals.rs` (pure, unit + mutation tested, run in-build via
  `doCheck`); `main.rs` is the `gh`/network orchestration + output;
  audit-recency parsing is `audit.rs`. The crate is a workspace member but
  `Cargo.toml`/`Cargo.lock` live at the repo root, so builds root there.
- `site/` — the dashboard: `index.html` (self-contained: inline CSS/JS, no
  external scripts/fonts) + `health.json`. Deployed by `pages.yml`.
- `flake.nix` — `packages.roh-scan`, `packages.screenshot`,
  `packages.default = roh-scan`; `devShells.default` composes rainix's devshell
  (so `pre-commit run --all-files` reproduces CI's static suite).

## Build / test / run

```
nix run .#roh-scan                          # scan the whole org (default ORG=rainlanguage) → writes site/health.json
nix run .#roh-scan -- rain.dia rain.flare   # scan specific repos
nix run .#roh-scan -- --json /tmp/h.json     # write elsewhere
nix run .#screenshot -- site out.png         # headless render of the dashboard (pinned chromium + fonts)
nix develop -c cargo test                    # roh-scan unit + mutation tests
nix develop -c pre-commit run --all-files    # reproduces CI's rs-static suite (rustfmt/clippy/prettier/nixfmt/…)
```

`roh-scan` env: `ORG` (default `rainlanguage`), `PAR` (parallelism, default 12),
`JSON_OUT` (default `site/health.json` — a bare run POPULATES it, never
print-and-discards). Read-only on GitHub; the caller supplies an org-read-authed
`gh`. It also reads each repo's `.audit/last-run.json` whole-repo stamp for the
audit-recency column.

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

The dashboard renders cross-repo, attacker-influenceable strings (PR/issue
titles, producer reasons). Build the DOM directly — `createElement` /
`textContent` / `.append` / the `.href` property — **never** `innerHTML`
string-building with a hand-rolled escaper (escaping is context-dependent and
fragile; see rainlanguage/claude-audit-skills#44). DOM construction escapes by
construction. `health.json` is `.prettierignore`d (serde-generated).
