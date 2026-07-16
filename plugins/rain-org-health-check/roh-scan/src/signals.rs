//! Pure signal detection: (fetched repo content) → modernization-debt signal names.
//! No I/O here — every check is a function of the strings the caller fetched, so the
//! whole detection surface is unit- and mutation-testable without gh/network.

use regex::Regex;
use std::sync::OnceLock;

/// The content a scan fetches for one repo. `soldeer_published` is the one network
/// fact (Some(true/false) once the registry was queried, None if not applicable/unknown).
#[derive(Default)]
pub struct RepoInputs {
    /// All `.github/workflows/*.{yml,yaml}` file contents, concatenated.
    pub workflows: String,
    /// `foundry.toml` content ("" if absent).
    pub foundry: String,
    /// Registry lookup for the foundry `[package] name`: Some(true) published,
    /// Some(false) unpublished, None if there is no package name or it wasn't queried.
    pub soldeer_published: Option<bool>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static signal regex")
}

/// Extract the `[package] name = "..."` value from foundry.toml (section-scoped), if any.
pub fn foundry_package_name(foundry: &str) -> Option<String> {
    foundry_package_field(foundry, "name")
}

/// Extract the `[package] version = "..."` value — the version the repo currently
/// publishes, which a dependant's pin is judged stale against (#79).
pub fn foundry_package_version(foundry: &str) -> Option<String> {
    foundry_package_field(foundry, "version")
}

/// The value of a scalar `key = "..."` under `[package]`, section-scoped.
fn foundry_package_field(foundry: &str, key: &str) -> Option<String> {
    let mut in_package = false;
    for line in foundry.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = t.strip_prefix(key) {
                let rest = rest.trim_start();
                if let Some(v) = rest.strip_prefix('=') {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Detect every signal present in `inputs`, in the canonical (scan.sh) order.
pub fn detect_signals(inputs: &RepoInputs) -> Vec<&'static str> {
    static RE_REMOVED: OnceLock<Regex> = OnceLock::new();
    static RE_BESPOKE: OnceLock<Regex> = OnceLock::new();
    static RE_CHECKOUT: OnceLock<Regex> = OnceLock::new();
    static RE_ETHERSCAN: OnceLock<Regex> = OnceLock::new();
    static RE_SKIPWARN: OnceLock<Regex> = OnceLock::new();

    let wf = &inputs.workflows;
    let foundry = &inputs.foundry;
    let mut out: Vec<&'static str> = Vec::new();

    if wf.contains("magic-nix-cache") {
        out.push("dead-magic-nix-cache");
    }
    if wf.contains("DeterminateSystems/nix-installer-action") {
        out.push("old-nix-installer");
    }
    let re_removed = RE_REMOVED.get_or_init(|| {
        re(r"(-c|command|nix run[^ ]*) +rainix-(rs|sol)-artifacts|rainix-rs-prelude")
    });
    if re_removed.is_match(wf) {
        out.push("removed-rainix-task");
    }
    let re_bespoke = RE_BESPOKE.get_or_init(|| {
        re(r"\-c +rainix-(sol|rs)-(test|static|legal)|command +rainix-(sol|rs)-(test|static|legal)")
    });
    if re_bespoke.is_match(wf) && !wf.contains("rainlanguage/rainix/.github/workflows/") {
        out.push("bespoke-ci");
    }
    if wf.contains("PRIVATE_KEY_DEV") {
        out.push("private-key-dev");
    }
    if wf.contains("publish-soldeer") {
        out.push("deprecated-publish-soldeer");
    }
    if wf.contains("TG_TOKEN") || wf.contains("TG_CHAT_ID") {
        out.push("telegram-secret-drift");
    }
    // @v1 / @v2 but NOT @v12 — the trailing boundary is the whole point.
    let re_checkout = RE_CHECKOUT.get_or_init(|| re(r"actions/checkout@v[12]([^0-9]|$)"));
    if re_checkout.is_match(wf) {
        out.push("old-actions-checkout");
    }
    let re_etherscan = RE_ETHERSCAN.get_or_init(|| re(r"CI_DEPLOY_[A-Z_]*ETHERSCAN_API_KEY"));
    if re_etherscan.is_match(wf) || re_etherscan.is_match(foundry) {
        out.push("per-chain-etherscan-key");
    }
    let re_skip = RE_SKIPWARN.get_or_init(|| re(r"skip[-_]warnings"));
    if wf.contains("soldeer push") && re_skip.is_match(wf) {
        out.push("soldeer-skip-warnings");
    }
    // soldeer-unpublished: a [package] exists but the registry has no revision.
    if inputs.soldeer_published == Some(false) {
        out.push("soldeer-unpublished");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(wf: &str) -> RepoInputs {
        RepoInputs {
            workflows: wf.into(),
            ..Default::default()
        }
    }

    #[test]
    fn magic_nix_cache() {
        assert!(
            detect_signals(&inp("uses: DeterminateSystems/magic-nix-cache-action@v2"))
                .contains(&"dead-magic-nix-cache")
        );
        assert!(!detect_signals(&inp("uses: cachix/cachix-action@v14"))
            .contains(&"dead-magic-nix-cache"));
    }

    #[test]
    fn old_nix_installer() {
        assert!(
            detect_signals(&inp("uses: DeterminateSystems/nix-installer-action@v4"))
                .contains(&"old-nix-installer")
        );
        assert!(
            !detect_signals(&inp("uses: nixbuild/nix-quick-install-action@v27"))
                .contains(&"old-nix-installer")
        );
    }

    #[test]
    fn removed_rainix_task() {
        assert!(
            detect_signals(&inp("run: nix develop -c rainix-sol-artifacts"))
                .contains(&"removed-rainix-task")
        );
        assert!(detect_signals(&inp("run: nix run .#rainix-rs-prelude"))
            .contains(&"removed-rainix-task"));
        assert!(!detect_signals(&inp("run: nix develop -c rainix-sol-test"))
            .contains(&"removed-rainix-task"));
    }

    #[test]
    fn bespoke_ci_only_without_reusable() {
        // inline rainix task + NO reusable call → bespoke.
        assert!(detect_signals(&inp("run: nix develop -c rainix-sol-test")).contains(&"bespoke-ci"));
        // same inline task but the repo calls the reusable → NOT bespoke.
        let with_reusable = "uses: rainlanguage/rainix/.github/workflows/rainix-sol-test.yaml@main\nrun: nix develop -c rainix-sol-test";
        assert!(!detect_signals(&inp(with_reusable)).contains(&"bespoke-ci"));
    }

    #[test]
    fn secrets_and_deprecated_refs() {
        assert!(detect_signals(&inp("key: ${{ secrets.PRIVATE_KEY_DEV }}"))
            .contains(&"private-key-dev"));
        assert!(
            detect_signals(&inp("uses: ./.github/workflows/publish-soldeer.yaml"))
                .contains(&"deprecated-publish-soldeer")
        );
        assert!(detect_signals(&inp("TG_TOKEN: x")).contains(&"telegram-secret-drift"));
        assert!(detect_signals(&inp("TG_CHAT_ID: y")).contains(&"telegram-secret-drift"));
    }

    #[test]
    fn checkout_v1_v2_but_not_v12() {
        assert!(detect_signals(&inp("uses: actions/checkout@v2")).contains(&"old-actions-checkout"));
        assert!(
            detect_signals(&inp("uses: actions/checkout@v1\n")).contains(&"old-actions-checkout")
        );
        // the boundary case the regex exists for:
        assert!(
            !detect_signals(&inp("uses: actions/checkout@v12")).contains(&"old-actions-checkout")
        );
        assert!(
            !detect_signals(&inp("uses: actions/checkout@v4")).contains(&"old-actions-checkout")
        );
    }

    #[test]
    fn per_chain_etherscan_from_either_source() {
        assert!(
            detect_signals(&inp("CI_DEPLOY_ARBITRUM_ETHERSCAN_API_KEY: x"))
                .contains(&"per-chain-etherscan-key")
        );
        let from_foundry = RepoInputs {
            foundry: "arbitrum_api_key = \"${CI_DEPLOY_BASE_ETHERSCAN_API_KEY}\"".into(),
            ..Default::default()
        };
        assert!(detect_signals(&from_foundry).contains(&"per-chain-etherscan-key"));
        assert!(
            !detect_signals(&inp("ETHERSCAN_API_KEY: shared")).contains(&"per-chain-etherscan-key")
        );
    }

    #[test]
    fn soldeer_skip_warnings_needs_both() {
        assert!(
            detect_signals(&inp("run: forge soldeer push --skip-warnings"))
                .contains(&"soldeer-skip-warnings")
        );
        assert!(
            detect_signals(&inp("run: forge soldeer push --skip_warnings"))
                .contains(&"soldeer-skip-warnings")
        );
        // push without skip, or skip without push → not flagged
        assert!(!detect_signals(&inp("run: forge soldeer push")).contains(&"soldeer-skip-warnings"));
        assert!(!detect_signals(&inp("run: something --skip-warnings"))
            .contains(&"soldeer-skip-warnings"));
    }

    #[test]
    fn soldeer_unpublished_from_registry_flag() {
        let unpub = RepoInputs {
            soldeer_published: Some(false),
            ..Default::default()
        };
        assert!(detect_signals(&unpub).contains(&"soldeer-unpublished"));
        let pub_ = RepoInputs {
            soldeer_published: Some(true),
            ..Default::default()
        };
        assert!(!detect_signals(&pub_).contains(&"soldeer-unpublished"));
        let unknown = RepoInputs {
            soldeer_published: None,
            ..Default::default()
        };
        assert!(!detect_signals(&unknown).contains(&"soldeer-unpublished"));
    }

    #[test]
    fn foundry_package_name_parsing() {
        assert_eq!(
            foundry_package_name("[package]\nname = \"rain.vats\"\nversion = \"1.0\""),
            Some("rain.vats".to_string())
        );
        // name outside [package] (e.g. in [profile.default]) must NOT match
        assert_eq!(
            foundry_package_name("[profile.default]\nname = \"nope\""),
            None
        );
        assert_eq!(foundry_package_name("[dependencies]\nfoo = \"1\""), None);
        assert_eq!(foundry_package_name(""), None);
    }

    #[test]
    fn clean_repo_no_signals() {
        let clean = RepoInputs {
            workflows: "uses: rainlanguage/rainix/.github/workflows/rainix-sol-test.yaml@main\nuses: actions/checkout@v4".into(),
            foundry: "[profile.default]\nsrc = \"src\"".into(),
            soldeer_published: Some(true),
        };
        assert!(detect_signals(&clean).is_empty());
    }

    #[test]
    fn canonical_order_preserved() {
        // a repo tripping several signals emits them in scan.sh order
        let many = RepoInputs {
            workflows: "magic-nix-cache\nDeterminateSystems/nix-installer-action\nPRIVATE_KEY_DEV\nactions/checkout@v2".into(),
            ..Default::default()
        };
        let got = detect_signals(&many);
        let dead = got
            .iter()
            .position(|s| *s == "dead-magic-nix-cache")
            .unwrap();
        let installer = got.iter().position(|s| *s == "old-nix-installer").unwrap();
        let checkout = got
            .iter()
            .position(|s| *s == "old-actions-checkout")
            .unwrap();
        assert!(dead < installer && installer < checkout);
    }
}
