---
name: rain-org-health-check
description: >-
  Audit the health of all rainlanguage GitHub org repos and produce a
  prioritized modernization report. Detects git submodules, the dead
  DeterminateSystems/magic-nix-cache action, bespoke (non-reusable) CI
  workflows, removed rainix tasks (rainix-rs-prelude / *-artifacts),
  PRIVATE_KEY_DEV deploy keys, per-chain etherscan-key drift, telegram
  secret-name drift, deprecated publish-soldeer references, old action
  versions, and soldeer publish gaps. Use when asked to check rain org repo
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
```bash
bash ${CLAUDE_PLUGIN_ROOT}/scripts/scan.sh            # whole org
bash ${CLAUDE_PLUGIN_ROOT}/scripts/scan.sh rain.dia rain.flare   # specific repos
```
It prints per-repo findings + an org-wide summary, and writes raw findings to
`/tmp/roh_findings.txt`. For a different org: `ORG=<org> bash .../scan.sh`.

After running, summarize the report for the user: lead with the org-wide
counts, then group repos by the highest-priority finding. Don't dump the raw
table unless asked.

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
- Put a hidden marker in the body per finding it covers: `<!-- rain-health:<flag> -->`.
- Before filing, list open markers to avoid duplicates:
  `gh issue list --repo <org>/<repo> --label rain-health --state open --json number,body`,
  and skip any finding whose marker is already present.
- On a later scan, close any open `rain-health` issue whose finding no longer
  appears (with a short comment), so the tracker self-heals.

## Audit existing issues for staleness
Issues outlive the problems they describe — a bug gets fixed or a subsystem
reworked, but the issue stays open. Thoroughly audit open issues and retire the
ones the codebase has already resolved. **Judge against the CURRENT code, not the
issue's filing date.**
1. List open issues, widest first: `gh issue list --repo <org>/<repo> --state open
   --limit 200 --json number,title,body,labels,createdAt`. Cover every repo, not
   just `rain-health` ones.
2. For each, decide if the described problem still exists:
   - **`rain-health` issues** — re-run the matching scan; if the finding's
     `<!-- rain-health:<flag> -->` marker no longer appears, it's resolved.
   - **other issues** — read the files/symbols/workflows the issue names and check
     `git log`/PRs since `createdAt`. Stale signals: the named code path was
     deleted/renamed, the API it complains about was reworked, the workflow it
     references no longer exists, or a merged PR explicitly closes it.
3. For each clearly-resolved issue, comment with the **concrete evidence** (the
   commit/PR that fixed it, or e.g. "magic-nix-cache no longer in any workflow")
   and close it. When the signal is weak, label `stale?` and leave it for a human
   rather than closing.
Be conservative — close only on positive evidence the problem is gone; a quiet or
old issue is not automatically a resolved one. This is Claude's judgment call,
issue by issue, not a bulk auto-close.

## What each finding means + how to fix it

| finding | meaning | remediation |
|---|---|---|
| `submodules` | repo still uses git submodules (`.gitmodules`) | de-submodule to soldeer: add `[dependencies]` (flattened tree) + `[soldeer] recursive_deps = false`, rewrite imports to versioned soldeer paths (`<pkg>-<ver>/src/...`), add the OZ bridge remapping only if it pulls `@openzeppelin-contracts-upgradeable`, drop `lib/` + `.gitmodules`. If a submodule's repo was renamed, check the redirect (e.g. `ethgild` → `rain.vats` = soldeer `rain-vats`). |
| `dead-magic-nix-cache` | uses `DeterminateSystems/magic-nix-cache-action` (service sunset → HTTP 418, builds fail) | replace the nix setup with `nixbuild/nix-quick-install-action@v30` + `cachix/cachix-action@v15` (name `rainlanguage`, `continue-on-error`) + `nix-community/cache-nix-action@v6`. Better: switch the whole job to a rainix reusable. |
| `removed-rainix-task` | runs `rainix-rs-prelude` / `rainix-rs-artifacts` / `rainix-sol-artifacts` (removed from latest rainix, or deploy-in-push-CI) | convert CI to the reusable workflows; move deploy out of push CI into `manual-sol-artifacts`. |
| `bespoke-ci` | runs rainix sol/rs tasks inline instead of calling a reusable | replace with `rainlanguage/rainix/.github/workflows/rainix-sol.yaml` / `rainix-rs.yaml` (or the individual `-static`/`-test`/`-legal`/`-wasm` ones). `secrets: inherit`. |
| `private-key-dev` | deploy/CI falls back to `secrets.PRIVATE_KEY_DEV` | always sign with `secrets.PRIVATE_KEY` (drop the `github.ref == 'refs/heads/main' && ... || PRIVATE_KEY_DEV` ternary). |
| `deprecated-publish-soldeer` | references the removed `publish-soldeer.yaml` reusable | migrate to `rainix-autopublish` (`package-release.yaml`, `soldeer-package: <name>`, `on: push: branches: [main]`) + add `[package].version` to foundry.toml. |
| `per-chain-etherscan-key` | foundry.toml/workflow uses `CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY` | Etherscan V2 is one multichain key — consolidate to `EXPLORER_VERIFICATION_KEY`. Keep flare/songbird separate (Routescan/Blockscout, not Etherscan). |
| `telegram-secret-drift` | uses `TG_TOKEN`/`TG_CHAT_ID` | standardize on `TELEGRAM_BOT_TOKEN` / `TELEGRAM_CHAT_ID` (the org convention). |
| `old-actions-checkout` / `old-nix-installer` | pinned to deprecated action versions | bump `actions/checkout` to v4+, prefer `nixbuild/nix-quick-install-action`. |
| `soldeer-unpublished` | foundry.toml has a `[package]` but no revision on the soldeer registry | a publishable package never got pushed — wire `rainix-autopublish` (+ `[package].version`), add a `.soldeerignore` (publish only `src/` + license/readme; soldeer's sensitive-file prompt otherwise hangs CI), and have an org admin create the project on soldeer.xyz before the first push. |
| `deprecated-interface` | Solidity imports a deprecated rain interpreter interface (V2/V3-era) — `IInterpreterV2`, `IInterpreterCallerV2`, `IInterpreterStoreV2`, `IExpressionDeployerV3`, `EvaluableConfigV3`/`EvaluableV2`, `LibEncodedDispatch`, `.eval2(`, `deployExpression2`, or any `rain.interpreter.interface/.../deprecated/` path | migrate to the current V4 API: `IInterpreterV4.eval4(EvalV4{...})` with `EvaluableV4{interpreter,store,bytecode}` (no expression deployment / encoded dispatch), `StackItem`/`bytes32[]`, eval-time validation. Follow the upstream `RaindexV6`/`LibRaindex` caller pattern. Worked example: flow#474. |

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
secret **names**. Names are low-sensitivity: in-use ones already appear in public
workflow YAML (that's how the scan finds them) and unused ones are headed for
deletion, so enumerating names exposes nothing new. Keep the audit generic and
re-runnable — do not commit any org's actual name list into a shared/public repo
(that's data, not tooling; a reusable skill stays org-agnostic).

1. **Referenced set** — names referenced anywhere:
   ```bash
   bash ${CLAUDE_PLUGIN_ROOT}/scripts/secret-inventory.sh        # whole org
   ```
   Lists each referenced name + repos, and flags repos that index
   `secrets[<expr>]` dynamically (names not statically resolvable — check by hand).
2. **Set list** — names that actually exist (admin or fine-grained `Secrets:read`):
   ```bash
   gh api orgs/<org>/actions/secrets --paginate --jq '.secrets[].name' | sort
   ```
3. **Dead = set − referenced.** Before deleting a candidate: re-run step 1 (the
   referenced set drifts), treat dynamically-built names
   (`CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY` / `_RPC_URL`) as live even if absent, and
   ignore `GITHUB_TOKEN` (auto-injected, not an org secret).
4. **Consolidate naming drift:** `CI_DEPLOY_<CHAIN>_RPC_URL` vs `RPC_URL_<CHAIN>_FORK`
   vs generic `CI_DEPLOY_RPC_URL`; per-chain `CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY`
   → `EXPLORER_VERIFICATION_KEY` (keep flare/songbird — Routescan/Blockscout, not
   Etherscan); `TG_*` → `TELEGRAM_*`.

**Optional re-runnable automation:** wrap steps 2–3 in a `workflow_dispatch`-only
workflow in a repo you control, authed with a fine-grained PAT scoped to *only*
that org + `Secrets: read` (worst-case leak: reading non-sensitive names). Keep it
dispatch-only and free of third-party actions so untrusted code can't run in the
token's context, and never have it emit a value. Generate the referenced set at
run time rather than committing a name snapshot.

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
