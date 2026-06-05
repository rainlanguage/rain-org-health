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

## Scope control
Scanning the whole org is dozens of `gh api` calls; for a quick check pass
specific repo names. The scan is the discovery step — fixing is a separate,
per-repo task (often a branch + PR each). Don't start mutating repos unless the
user asks.
