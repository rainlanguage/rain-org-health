# Adversarial Mutation Test — roh-scan + dashboard

Scope: plugins/rain-org-health-check/roh-scan/src/signals.rs (12 signal checks,
pure) + site/index.html JS (filter/render) Harness:
rust=`nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc -c cargo test` |
dash=node harness over extracted pure fns Oracle: signals.rs must match
scan.sh's grep patterns EXACTLY (adversarial: any divergence = port bug)

## Signal-check mutation targets (kill = a named test fails)

- [TODO] contains checks (magic-nix-cache, nix-installer, PRIVATE_KEY_DEV,
  publish-soldeer, TG_*)
- [TODO] @v[12] boundary (v1/v2 yes, v12/v4 no)
- [TODO] bespoke AND-not-reusable (both conditions)
- [TODO] etherscan wf-OR-foundry (both sources)
- [TODO] soldeer-skip push AND skip (both required)
- [TODO] deploy-constants prod AND NOT versioned (negation)
- [TODO] foundry_package_name section gate ([package] only)
- [TODO] canonical order preserved

## ADVERSARIAL — port fidelity vs scan.sh

- [TODO] each regex ported char-for-char; @v[12] boundary; multiline anchors ($)
  on tree checks
