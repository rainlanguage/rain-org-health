# rain-org-health

A Claude Code **plugin marketplace** providing the `rain-org-health-check`
skill: an org-wide health audit for the [`rainlanguage`](https://github.com/rainlanguage)
GitHub org.

**📊 Live dashboard: <https://rainlanguage.github.io/rain-org-health/>** — per-repo
modernization-debt signals, updated from each scan. Source in [`site/`](site/).

It scans every active repo for rainix/soldeer modernization debt and emits a
prioritized report — git submodules, the dead `magic-nix-cache` action, bespoke
(non-reusable) CI workflows, removed rainix tasks, `PRIVATE_KEY_DEV` deploy
keys, per-chain etherscan-key drift, telegram secret-name drift, deprecated
`publish-soldeer` references, old action versions, and soldeer publish gaps —
with the remediation playbook for each.

## Install

```sh
# add this repo as a marketplace
/plugin marketplace add rainlanguage/rain-org-health
# install the plugin
/plugin install rain-org-health-check@rain-org-health
```

(or from the CLI: `claude plugin marketplace add rainlanguage/rain-org-health`
then `claude plugin install rain-org-health-check@rain-org-health`.)

## Use

Ask Claude to "run a rain org health check" (or invoke
`/rain-org-health-check:rain-org-health-check`). Requires an authenticated `gh`
with org read access, plus `curl` and `python3`. The scan is **read-only**.

You can also run the scanner directly (a Rust binary, no wrapper script):

```sh
nix run github:rainlanguage/rain-org-health#roh-scan                    # whole org
nix run github:rainlanguage/rain-org-health#roh-scan -- rain.dia rain.flare  # specific repos
nix run .#roh-scan -- --json site/health.json                          # refresh dashboard data
```

## Layout

```
.claude-plugin/marketplace.json          # marketplace catalog
flake.nix                                # exposes packages.roh-scan
Cargo.toml                               # workspace root
plugins/rain-org-health-check/
├── .claude-plugin/plugin.json           # plugin manifest
├── roh-scan/                            # the org scanner (Rust; signal detection + gh/curl)
└── skills/rain-org-health-check/SKILL.md  # skill instructions + remediation playbook
site/                                    # dashboard (index.html + health.json)
```

## License

DecentraLicense 1.0 (LicenseRef-DCL-1.0), consistent with the rest of the org.
