# rain-org-health

A Claude Code **plugin marketplace** providing the `rain-org-health-check`
skill: an org-wide health audit for the
[`rainlanguage`](https://github.com/rainlanguage) GitHub org.

**📊 Live dashboard: <https://rainlanguage.github.io/rain-org-health/>** —
per-repo modernization-debt signals, updated from each scan. Source in
[`site/`](site/).

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
nix run github:rainlanguage/rain-org-health#roh-scan -- --help         # every mode + flag
nix run github:rainlanguage/rain-org-health#roh-scan                    # whole org
nix run github:rainlanguage/rain-org-health#roh-scan -- rain.dia rain.flare  # specific repos
nix run .#roh-scan -- --json site/health.json                          # refresh dashboard data
```

`--help` is the reference material for the tool — modes, flags, env vars, exit
statuses. An unknown flag or a repo name that does not exist is an error, never
a clean empty result.

### Who consumes a package, and who uses a symbol from it

```sh
nix run .#roh-scan -- consumers rain-solmem                       # the consumer list, per org
nix run .#roh-scan -- consumers rain-solmem --symbol unsafeList   # …and who actually references it
```

Two layers, because a consumer list alone does not answer "does anything call
`unsafeList`": the **manifest** layer (which repos declare the package, at which
version) and the **source** layer (which of them name the symbol in their own
Solidity, with vendored copies of the library — which contain the symbol's
definition — counted separately rather than as callers).

Dependencies are read from **every** manifest shape and unioned: `foundry.toml`
`[dependencies]` and `[profile.*] remappings`, `soldeer.lock`, `foundry.lock`,
`remappings.txt`, `.gitmodules`. Handling one shape is the defect this mode
exists to prevent — `rainlanguage/rainlang` has no `foundry.lock` at all, so a
`foundry.lock`-only sweep drops it with no error. GitHub code search is not used
(default branches only, and it under-returns silently). A repo that could not be
read is reported and the exit status is 1, so an incomplete answer cannot pass
as a complete one.

## Layout

```
.claude-plugin/marketplace.json          # marketplace catalog
flake.nix                                # exposes packages.roh-scan
Cargo.toml                               # workspace root
plugins/rain-org-health-check/
├── .claude-plugin/plugin.json           # plugin manifest
├── roh-scan/                            # the org scanner (Rust; signal detection + gh/curl)
└── skills/rain-org-health-check/SKILL.md  # skill instructions + remediation playbook
site/                                    # dashboard (html pages + health.json)
```

## License

DecentraLicense 1.0 (LicenseRef-DCL-1.0), consistent with the rest of the org.
