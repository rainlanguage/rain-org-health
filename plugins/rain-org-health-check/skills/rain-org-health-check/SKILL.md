---
name: rain-org-health-check
description: >-
  Audit the health of all rainlanguage GitHub org repos and produce a
  prioritized modernization report. Detects the dead
  DeterminateSystems/magic-nix-cache action, bespoke (non-reusable) CI
  workflows, removed rainix tasks (rainix-rs-prelude / *-artifacts),
  PRIVATE_KEY_DEV deploy keys, per-chain etherscan-key drift, telegram
  secret-name drift, deprecated publish-soldeer references, old action versions,
  soldeer publish gaps, dead foundry.lock submodule pins, and untested
  external/public functions on concrete Solidity contracts. Also reports when
  each repo was last fully (whole-repo)
  audited by the audit skill. Use when asked to check rain org repo
  health, audit rainix/soldeer CI modernization, find which repos still need
  updating, or before/after an org-wide rainix bump.
allowed-tools: Bash Read Grep WebFetch
---

# Rain org health check

Audits the `rainlanguage` GitHub org for repo-modernization debt and emits a
prioritized report. The signals encode the rainix-reusable / soldeer migration
playbook.

## Prerequisites

- `gh` authenticated with org read access; `curl`; `python3`.
- Read-only — the scan never writes or pushes.

## Run the scan

The scanner is a Rust binary (`roh-scan`) run directly from nix — no wrapper
script:

```bash
nix run github:rainlanguage/rain-org-health#roh-scan                     # whole org
nix run github:rainlanguage/rain-org-health#roh-scan -- rain.dia rain.flare   # specific repos
nix run github:rainlanguage/rain-org-health#roh-scan -- --json site/health.json   # write the dashboard data
```

It prints per-repo findings + an org-wide summary. For a different org:
`ORG=<org> nix run …#roh-scan`. Requires an authenticated `gh` (plus `curl`).

After running, summarize the report for the user: lead with the org-wide counts,
then group repos by the highest-priority finding. Don't dump the raw table
unless asked.

## Audit recency (last whole-repo audit)

Alongside the modernization signals, the scan reports **when each repo was last
audited by the audit skill's whole-repo pass** — it reads `.audit/last-run.json`
(the stamp the audit skill commits per run) from each repo. Accuracy hinges on
the `scope` field: the audit skill is _also_ run PR-scoped (the vetter/producer
audit only a PR's changed files), so a stamp counts as a full audit **only**
when `scope == "whole-repo"`; a `pr:<n>` / `paths:<…>` stamp (or none) reads as
"never fully audited". For an audited repo the scan flags staleness by comparing
`auditedCommit` to the current branch HEAD.

Output: an "audit recency" section (never-audited + stalest first) in the text
report, and per-repo entries in the JSON `audits` array plus the
`reposWholeRepoAudited` / `reposNeverAudited` counts. Use it to see which repos
are overdue for a full audit.

## Untested external surface (external/public functions no test names)

External/public functions are a contract's API and attack surface; one with zero
test coverage is a latent risk (rain.math.float#156: three external `format()`
overloads sat uncovered for months and were only tested in #169 after the gap
was noticed by hand). The scan generalizes that one-off into a standing check:
for every Foundry repo it shallow-clones the default branch, **enumerates**
(never samples) the external/public functions declared on every concrete
`contract` — not `abstract contract`, `interface`, or `library` — via a real
Solidity parse, then greps the repo's own test sources for each function's name
as a whole identifier. Only a function whose name appears **nowhere** in any
test file is flagged.

Restraint, so the flag stays a strong claim:

- A test can exercise a function indirectly, so ANY whole-identifier mention in
  a test source (a helper call, a selector table, even a comment) counts as
  referenced and suppresses the flag. When triaging a flagged function, that
  grep has already been done — the finding means no test so much as names it.
- Overloads collapse into one contract/function entry (a name grep cannot tell
  them apart); vendored code (`lib/`, `dependencies/`, `node_modules/`) counts
  neither as surface nor as coverage; Foundry scripts and test helpers are not
  surface; `constructor`/`receive`/`fallback` and auto-generated public-getter
  functions are excluded.
- Functions inherited from an abstract base are enumerated at the base only if a
  concrete contract declares them, so the report is a floor, not a ceiling.
- A repo whose clone failed reports `unknown` — never "clean" — and a source
  file that does not parse is counted in `sourcesUnparsed` rather than silently
  contributing nothing.

Output: an "untested external surface" section in the text report listing every
flagged contract/function per repo, the per-repo `untestedExternals` array in
`health.json` (`state: analyzed|unknown`, `externalFunctions`, `testFiles`,
`sourcesUnparsed`, `untested[]`), and an `untested-externals` findings flag on
affected repos (so it rides the same triage/issue-marker flow as every other
signal). Before filing, sanity-check a sample of flagged functions against the
repo's tests — the check is static and generous toward coverage, but filing is
still judgment.

## Triage in chat, then file issues directly (don't blind-file)

Detection is mechanical; filing is judgment — so **Claude files the issues
directly**, not a script. Never pipe a raw scan into issues.

1. Run the scan and **present the findings as a table in chat** (repo × finding,
   grouped by severity), then discuss with the user: which are real vs false
   positives, what's already known or won't-fix, how to group related findings,
   and what order to tackle them.
2. File only the **agreed** issues, with `gh issue create`, grouping several
   findings on one repo into a single issue where that's the real unit of work
   (e.g. the whole nix/CI modernization), and writing each issue's body from the
   discussion + the remediation column below.

Follow these conventions so repeat scans stay clean:

- Label every filed issue `rain-health`.
- Put a hidden marker in the body per finding it covers:
  `<!-- rain-health:<flag> -->`.
- Before filing, list open markers to avoid duplicates:
  `gh issue list --repo <org>/<repo> --label rain-health --state open --json number,body`,
  and skip any finding whose marker is already present.
- On a later scan, close any open `rain-health` issue whose finding no longer
  appears (with a short comment), so the tracker self-heals.

## Audit existing issues for staleness

Issues outlive the problems they describe — a bug gets fixed or a subsystem
reworked, but the issue stays open. Thoroughly audit open issues and retire the
ones the codebase has already resolved. **Judge against the CURRENT code, not
the issue's filing date.**

1. List open issues, widest first:
   `gh issue list --repo <org>/<repo> --state open --limit 200 --json number,title,body,labels,createdAt`.
   Cover every repo, not just `rain-health` ones.
2. For each, decide if the described problem still exists:
   - **`rain-health` issues** — re-run the matching scan; if the finding's
     `<!-- rain-health:<flag> -->` marker no longer appears, it's resolved.
   - **other issues** — read the files/symbols/workflows the issue names and
     check `git log`/PRs since `createdAt`. Stale signals: the named code path
     was deleted/renamed, the API it complains about was reworked, the workflow
     it references no longer exists, or a merged PR explicitly closes it.
3. For each clearly-resolved issue, comment with the **concrete evidence** (the
   commit/PR that fixed it, or e.g. "magic-nix-cache no longer in any workflow")
   and close it. When the signal is weak, label `stale?` and leave it for a
   human rather than closing. Be conservative — close only on positive evidence
   the problem is gone; a quiet or old issue is not automatically a resolved
   one. This is Claude's judgment call, issue by issue, not a bulk auto-close.

## What each finding means + how to fix it

| finding                                      | meaning                                                                                                                                                                                                                                                                                                                           | remediation                                                                                                                                                                                                                                                                                                                                        |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dead-magic-nix-cache`                       | uses `DeterminateSystems/magic-nix-cache-action` (service sunset → HTTP 418, builds fail)                                                                                                                                                                                                                                         | replace the nix setup with `nixbuild/nix-quick-install-action@v30` + `cachix/cachix-action@v15` (name `rainlanguage`, `continue-on-error`) + `nix-community/cache-nix-action@v6`. Better: switch the whole job to a rainix reusable.                                                                                                               |
| `removed-rainix-task`                        | runs `rainix-rs-prelude` / `rainix-rs-artifacts` / `rainix-sol-artifacts` (removed from latest rainix, or deploy-in-push-CI)                                                                                                                                                                                                      | convert CI to the reusable workflows; move deploy out of push CI into `manual-sol-artifacts`.                                                                                                                                                                                                                                                      |
| `bespoke-ci`                                 | runs rainix sol/rs tasks inline instead of calling a reusable                                                                                                                                                                                                                                                                     | replace with `rainlanguage/rainix/.github/workflows/rainix-sol.yaml` / `rainix-rs.yaml` (or the individual `-static`/`-test`/`-legal`/`-wasm` ones). `secrets: inherit`.                                                                                                                                                                           |
| `private-key-dev`                            | deploy/CI falls back to `secrets.PRIVATE_KEY_DEV`                                                                                                                                                                                                                                                                                 | always sign with `secrets.PRIVATE_KEY` (drop the `github.ref == 'refs/heads/main' && ...                                                                                                                                                                                                                                                           |
| `deprecated-publish-soldeer`                 | references the removed `publish-soldeer.yaml` reusable                                                                                                                                                                                                                                                                            | migrate to `rainix-autopublish` (`package-release.yaml`, `soldeer-package: <name>`, `on: push: branches: [main]`) + add `[package].version` to foundry.toml.                                                                                                                                                                                       |
| `per-chain-etherscan-key`                    | foundry.toml/workflow uses `CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY`                                                                                                                                                                                                                                                                  | Etherscan V2 is one multichain key — consolidate to `EXPLORER_VERIFICATION_KEY`. Keep flare/songbird separate (Routescan/Blockscout, not Etherscan).                                                                                                                                                                                               |
| `telegram-secret-drift`                      | uses `TG_TOKEN`/`TG_CHAT_ID`                                                                                                                                                                                                                                                                                                      | standardize on `TELEGRAM_BOT_TOKEN` / `TELEGRAM_CHAT_ID` (the org convention).                                                                                                                                                                                                                                                                     |
| `old-actions-checkout` / `old-nix-installer` | pinned to deprecated action versions                                                                                                                                                                                                                                                                                              | bump `actions/checkout` to v4+, prefer `nixbuild/nix-quick-install-action`.                                                                                                                                                                                                                                                                        |
| `soldeer-unpublished`                        | the repo names a soldeer package (release-metadata table, or `package-release.yaml`/`.yml`'s `soldeer-package:` input after rainix#335) but no revision is on the registry                                                                                                                                                        | a publishable package never got pushed — wire `rainix-autopublish` (+ `[package].version`), add a `.soldeerignore` (publish only `src/` + license/readme; soldeer's sensitive-file prompt otherwise hangs CI), and have an org admin create the project on soldeer.xyz before the first push.                                                      |
| `deprecated-interface`                       | Solidity imports a deprecated rain interpreter interface (V2/V3-era) — `IInterpreterV2`, `IInterpreterCallerV2`, `IInterpreterStoreV2`, `IExpressionDeployerV3`, `EvaluableConfigV3`/`EvaluableV2`, `LibEncodedDispatch`, `.eval2(`, `deployExpression2`, or any `rain.interpreter.interface/.../deprecated/` path                | migrate to the current V4 API: `IInterpreterV4.eval4(EvalV4{...})` with `EvaluableV4{interpreter,store,bytecode}` (no expression deployment / encoded dispatch), `StackItem`/`bytes32[]`, eval-time validation. Follow the upstream `RaindexV6`/`LibRaindex` caller pattern. Worked example: flow#474.                                             |
| `soldeer-skip-warnings`                      | a workflow runs `forge soldeer push` with `--skip-warnings`                                                                                                                                                                                                                                                                       | **Never** skip soldeer publish warnings — they're the guard that catches accidentally publishing sensitive files (`.env`, keys, `.git`, build dirs) into the package. Remove `--skip-warnings` and scope the publish with a `.soldeerignore` (publish only `src/` + license/readme) so the push succeeds in CI _without_ suppressing the warning.  |
| `untested-externals`                         | a concrete contract declares external/public function(s) whose name appears in NO test source (see "Untested external surface" below)                                                                                                                                                                                             | write tests exercising each flagged function directly (the flagged list is per contract/function in `health.json`'s `untestedExternals` and the text report). Worked example: rain.math.float#156 → #169. Confirm each is a real gap first — the grep already suppresses any test that so much as names the function.                              |
| `stale-foundry-lock`                         | `foundry.lock` pins a `lib/<name>` path that `.gitmodules` does not declare as a submodule — a git-submodule lockfile in a repo that resolves through soldeer instead. Not silent: `forge build` emits `Dependency '<path>' not found at expected path` per entry, and the dead pin contradicts the version the build really uses | delete `foundry.lock`, plus its `REUSE.toml` annotation entry and `.soldeerignore` line if present. Submodules cannot come back — rainix CI's `no-submodules` check fails on a root `.gitmodules` or any committed gitlink. Worked example: rain.solmem#111. Judged PER PIN, so a repo that genuinely still uses submodules (flow) is not flagged. |

## Detecting deprecated interfaces (code search)

`deprecated-interface` lives in Solidity source, not workflows, so detect it
org-wide with code search rather than the workflow scan:

```bash
for q in 'IInterpreterV2' 'IInterpreterCallerV2' 'IExpressionDeployerV3' 'LibEncodedDispatch' \
         'deployExpression2' 'EvaluableConfigV3' '.eval2('; do
  gh search code --owner rainlanguage "$q" --json repository -q '.[].repository.name'
done | sort -u
```

Any repo that appears is wired to the pre-V4 interpreter API and should be
migrated to `eval4`/`EvaluableV4` (track per repo; flow#474 is the template).
Note flow itself was silently on this — its deploy looked fine but the contract
called the now-removed `LibEncodedDispatch`.

## Notes / gotchas to carry into fixes

- A soldeer CI push hangs (`error during IO operation: not connected`) when no
  `.soldeerignore` excludes repo dotfiles — mirror raindex's `.soldeerignore`.
- `rainix-copy-artifacts` regenerates committed artifacts via consumer hooks
  (`script/build-meta.sh`, `BuildPointers.sol`, `CopyArtifacts.sol`,
  `script/build.sh`); meta/fixtures needing `rain`/`node` belong in `build.sh`
  (sol-shell lacks them), not `build-meta.sh`.
- New non-`.sol` files (`.soldeerignore`, `remappings.txt`, `soldeer.lock`,
  `slither.config.json`, shell scripts) need a license header or a `REUSE.toml`
  entry or `reuse lint` (the `legal` check) fails.
- After de-submoduling a **deployed** repo, the bytecode/address changes — cut a
  legacy tag and plan a redeploy (deterministic Zoltu via
  `LibRainDeploy.deployAndBroadcast` + committed `*.pointers.sol`).

## Secret consolidation / dead-secret audit

Secret **values** are write-only and unreadable; this audit only ever handles
secret **names**. Names are low-sensitivity: in-use ones already appear in
public workflow YAML (that's how the scan finds them) and unused ones are headed
for deletion, so enumerating names exposes nothing new. Keep the audit generic
and re-runnable — do not commit any org's actual name list into a shared/public
repo (that's data, not tooling; a reusable skill stays org-agnostic).

1. **Referenced set** — names referenced anywhere:
   ```bash
   bash ${CLAUDE_PLUGIN_ROOT}/scripts/secret-inventory.sh        # whole org
   ```
   Lists each referenced name + repos, and flags repos that index
   `secrets[<expr>]` dynamically (names not statically resolvable — check by
   hand).
2. **Set list** — names that actually exist (admin or fine-grained
   `Secrets:read`):
   ```bash
   gh api orgs/<org>/actions/secrets --paginate --jq '.secrets[].name' | sort
   ```
3. **Dead = set − referenced.** Before deleting a candidate: re-run step 1 (the
   referenced set drifts), treat dynamically-built names
   (`CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY` / `_RPC_URL`) as live even if absent,
   and ignore `GITHUB_TOKEN` (auto-injected, not an org secret).
4. **Consolidate naming drift:** `CI_DEPLOY_<CHAIN>_RPC_URL` vs
   `RPC_URL_<CHAIN>_FORK` vs generic `CI_DEPLOY_RPC_URL`; per-chain
   `CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY` → `EXPLORER_VERIFICATION_KEY` (keep
   flare/songbird — Routescan/Blockscout, not Etherscan); `TG_*` → `TELEGRAM_*`.

**Optional re-runnable automation:** wrap steps 2–3 in a
`workflow_dispatch`-only workflow in a repo you control, authed with a
fine-grained PAT scoped to _only_ that org + `Secrets: read` (worst-case leak:
reading non-sensitive names). Keep it dispatch-only and free of third-party
actions so untrusted code can't run in the token's context, and never have it
emit a value. Generate the referenced set at run time rather than committing a
name snapshot.

## Deployment verification (explorer)

Deploy repos (those with `src/generated/*.pointers.sol`) land contracts at
deterministic Zoltu addresses — the SAME address on every chain. A deploy's
`--verify` step can silently fail on one chain (e.g. a bytecode-metadata
mismatch) and leave a deployed-but-unverified contract. Every published tag's
contracts should be source-verified on every network it targets; check it:

```bash
EXPLORER_VERIFICATION_KEY=<etherscan-v2-key> \
  bash ${CLAUDE_PLUGIN_ROOT}/scripts/verify-deployments.sh          # all deploy repos
bash ${CLAUDE_PLUGIN_ROOT}/scripts/verify-deployments.sh rain.verify  # specific
```

Per contract it prints per-network `verified | UNVERIFIED | ?`. Etherscan V2
chains (arbitrum/base/base_sepolia/polygon/ethereum/sepolia) share the one
multichain key; flare/songbird use Routescan (keyless). `UNVERIFIED` on a live
network = re-run that chain's verify step. It checks the current (HEAD) pointer
addresses; a tag with different bytecode has a different address, so verify
older tags by checking out the tag. It can't distinguish unverified-but-deployed
from not-deployed — cross-check the prod test if unsure.

## Scope control

Scanning the whole org is dozens of `gh api` calls; for a quick check pass
specific repo names. The scan is the discovery step — fixing is a separate,
per-repo task (often a branch + PR each). Don't start mutating repos unless the
user asks.
