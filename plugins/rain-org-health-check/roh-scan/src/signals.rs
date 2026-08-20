//! Pure signal detection: (fetched repo content) → modernization-debt signal names.
//! No I/O here — every check is a function of the strings the caller fetched, so the
//! whole detection surface is unit- and mutation-testable without gh/network.

use regex::Regex;
use std::sync::OnceLock;

/// The content a scan fetches for one repo. The soldeer registry lookup is the one
/// network fact, and both `soldeer_*` fields come from that single query.
#[derive(Default)]
pub struct RepoInputs {
    /// All `.github/workflows/*.{yml,yaml}` file contents, concatenated.
    pub workflows: String,
    /// `foundry.toml` content ("" if absent).
    pub foundry: String,
    /// `.github/workflows/package-release.yaml` (or `.yml`) content, kept apart
    /// from the concatenation: its `soldeer-package:` input is the canonical
    /// home of the package name once rainix#335 drops the manifest's release
    /// metadata (`resolve_package_name`). `None` when the workflow listing has
    /// no such file — a repo with no release workflow publishes nothing.
    /// `Some("")` when the file is listed but its body could not be read, which
    /// is UNKNOWN, not absence.
    pub release_workflow: Option<String>,
    /// Registry lookup for the resolved package name (`RepoInputs::package`):
    /// Some(true) published,
    /// Some(false) unpublished, None if there is no package name or it wasn't queried.
    pub soldeer_published: Option<bool>,
    /// The newest revision the registry has for the resolved package name — the
    /// newest version a consumer can actually pin, and so the ceiling a dependant's
    /// pin is judged stale against (#79). `None` when unpublished, unqueried, or
    /// the query failed; an unknown ceiling flags nobody.
    ///
    /// NOT the manifest's own `version` from HEAD. Under the org's release
    /// lifecycle that field is the NEXT, unreleased version — bumped straight
    /// after each publish — so judging pins against it marks every consumer stale
    /// for failing to pin a version that does not exist (#86).
    pub soldeer_version: Option<String>,
    /// `foundry.lock` — the git SUBMODULE lockfile.
    pub foundry_lock: RepoFile,
    /// `.gitmodules` — the definition of which paths are submodules at all.
    pub gitmodules: RepoFile,
}

/// A repo file the scan tried to read.
///
/// `Absent` and `Unreadable` are three-way rather than an `Option<String>`
/// deliberately. [`stale_foundry_lock`] fires on the ABSENCE of a `.gitmodules`
/// entry, so an unread `.gitmodules` collapsed into "this repo has no
/// submodules" would turn a rate-limited fetch into a finding — the #52 rule
/// (an errored fetch must never masquerade as an empty resource) applied to the
/// input side.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RepoFile {
    /// Read, with this content.
    Present(String),
    /// Read as a genuine absence: the repo does not have this file.
    Absent,
    /// The fetch failed. NOTHING may be concluded from it.
    #[default]
    Unreadable,
}

impl RepoFile {
    /// The content of a file that is known to be there, `Some("")` for one known
    /// to be absent, and `None` when the read failed and neither is known.
    fn known(&self) -> Option<&str> {
        match self {
            RepoFile::Present(s) => Some(s),
            RepoFile::Absent => Some(""),
            RepoFile::Unreadable => None,
        }
    }
}

/// The `foundry.lock` pins that name a path the repo does not have as a
/// submodule — every one of them dead.
///
/// `foundry.lock` is Foundry's GIT SUBMODULE lockfile: it maps a vendored
/// `lib/<name>` path to the commit `forge install`/`forge update` should
/// restore. It says nothing about soldeer, which resolves through `soldeer.lock`
/// into `dependencies/`. So once a repo migrates off submodules the file keeps
/// pinning paths that no longer exist, and it is not silent — `forge build`
/// emits `Dependency '<path>' not found at expected path` once per entry. Worse,
/// it is an active lie about what the build uses: rain.solmem's dead pin names
/// forge-std v1.14.0 while the build actually resolves 1.16.1 through soldeer.
///
/// Judged PER PIN against `.gitmodules` rather than by the presence of the file,
/// because that is the property that makes a pin dead. A repo that still uses
/// submodules keeps a live `foundry.lock` (rainlanguage/flow pins six paths and
/// declares all six as submodules) and must not be flagged, while a repo
/// half-way through the migration has some live pins and some dead ones and the
/// dead ones are still worth naming.
///
/// An unparseable lock yields no pins: this signal claims that specific paths
/// are dead, and it cannot make that claim about a file it could not read.
pub fn dead_foundry_lock_pins(foundry_lock: &str, gitmodules: &str) -> Vec<String> {
    let submodules = submodule_paths(gitmodules);
    // TOP-LEVEL keys only. A pin's value nests its own `tag`/`rev` keys, so a
    // grep for quoted keys would report `tag` as a pinned path.
    let Ok(serde_json::Value::Object(pins)) =
        serde_json::from_str::<serde_json::Value>(foundry_lock)
    else {
        return Vec::new();
    };
    pins.keys()
        .map(|p| p.trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty() && !submodules.contains(p))
        .collect()
}

/// Every path `.gitmodules` declares as a submodule.
///
/// Both the `path =` value and the `[submodule "…"]` section name are taken:
/// git writes the two identically in practice, but a section whose `path` line
/// is missing would otherwise read as declaring no submodule at all, and this is
/// the set whose ABSENCE condemns a pin.
fn submodule_paths(gitmodules: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for line in gitmodules.lines() {
        let t = line.trim();
        let found = if let Some(rest) = t.strip_prefix("[submodule") {
            rest.trim_end_matches(']').trim().trim_matches('"').trim()
        } else if let Some(rest) = t.strip_prefix("path") {
            match rest.trim_start().strip_prefix('=') {
                Some(v) => v.trim(),
                None => continue,
            }
        } else {
            continue;
        };
        let found = found.trim_end_matches('/');
        if !found.is_empty() {
            out.insert(found.to_string());
        }
    }
    out
}

/// Whether this repo carries a `foundry.lock` with at least one dead pin.
///
/// Both files must have been READ. If either is unreadable the answer is
/// unknown, and an unknown answer flags nobody.
fn stale_foundry_lock(inputs: &RepoInputs) -> bool {
    let (Some(lock), Some(modules)) = (inputs.foundry_lock.known(), inputs.gitmodules.known())
    else {
        return false;
    };
    !dead_foundry_lock_pins(lock, modules).is_empty()
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static signal regex")
}

/// Extract the soldeer package `name` from a foundry.toml, if any.
pub fn foundry_package_name(foundry: &str) -> Option<String> {
    foundry_package_field(foundry, "name")
}

/// The value of a scalar `key = "..."` in the manifest's soldeer release-metadata
/// table, which is `[external.package]` or `[package]`.
///
/// Both spellings are live in the org and BOTH must read, because this scanner
/// reads repos it does not control and the rename is landing repo by repo. The
/// package name is the dependency graph's join key: a name lost here does not
/// degrade the node, it deletes it — the repo drops out of `package_index`, and
/// every edge INTO it disappears with it, so its consumers read as standing on
/// clear ground. A `None` from this reader alone no longer decides that:
/// `resolve_package_name` falls through to the release workflow's
/// `soldeer-package:` input, because rainix#335 removes this table from
/// migrated manifests entirely.
///
/// `[external.*]` is the tree foundry reserves for other tools' config and
/// ignores, which is why the release metadata belongs there: a bare `[package]`
/// is not reserved, so forge reads it as a profile and warns on every
/// invocation, and `forge config --fix` rewrites it to `[profile.package]` —
/// which is a profile named "package" and no longer release metadata at all.
/// `[profile.package]` therefore must NOT read as the package table.
///
/// Parsed as TOML rather than scanned line-wise. The line scanner this replaced
/// compared each section header to the literal `[package]`, so it saw a rename
/// as an absence — and would equally miss `[ external.package ]`,
/// `["external"."package"]` or the dotted `external.package.name = "..."`, all
/// of which are the same table to every tool that actually reads the file.
/// Matching the TOML structure instead of the bytes is what makes those
/// equivalent here too, and it is already how `graph::foundry_dependencies`
/// reads `[dependencies]` out of this same file — so a manifest that will not
/// parse is now unreadable to both, rather than half-read by one.
fn foundry_package_field(foundry: &str, key: &str) -> Option<String> {
    let doc: toml::Table = foundry.parse().ok()?;
    let external = doc.get("external").and_then(toml::Value::as_table);
    // `[package]` is checked first only to make the two-spelling case
    // deterministic; a manifest carrying both is mid-rename, and either answer
    // is the same package.
    let found = [doc.get("package"), external.and_then(|e| e.get("package"))]
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
        // A table that carries no usable value for `key` is not an answer, so the
        // other spelling still gets its turn.
        .find_map(|t| {
            t.get(key)
                .and_then(toml::Value::as_str)
                .filter(|v| !v.is_empty())
        })
        .map(str::to_string);
    // Bound, not returned as the tail expression: the borrow of `doc` inside the
    // iterator has to end before `doc` itself does (E0597).
    found
}

/// How a repo's soldeer package name resolved. The name is the dependency
/// graph's join key, so the two ways of not having one are DIFFERENT answers
/// and must not collapse: a repo with no release lifecycle genuinely publishes
/// nothing, while a repo whose name merely could not be read still publishes a
/// package its consumers pin — dropping it from `graph::package_index` deletes
/// every edge into it and its consumers read as standing on clear ground.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageResolution {
    /// The name, from the manifest's release-metadata table or, failing that,
    /// the release workflow's `soldeer-package:` input.
    Named(String),
    /// A release workflow exists — the repo publishes SOMETHING — but no name
    /// could be read from the manifest or the workflow. Per the same rule that
    /// keeps a missing `[dependencies]` from reading as zero deps
    /// (`deps_known`): this renders as UNKNOWN, never as "no package".
    Unknown,
    /// No release metadata and no release workflow: the repo publishes
    /// nothing, and drawing no edges into it is the truth rather than a hole.
    NoPackage,
}

impl PackageResolution {
    /// The resolved name, if any.
    pub fn name(&self) -> Option<&str> {
        match self {
            PackageResolution::Named(name) => Some(name),
            PackageResolution::Unknown | PackageResolution::NoPackage => None,
        }
    }

    /// False only for `Unknown`: a `NoPackage` repo's absence is a KNOWN fact,
    /// exactly as an empty dependency list with `deps_known == true` is.
    pub fn known(&self) -> bool {
        !matches!(self, PackageResolution::Unknown)
    }
}

impl RepoInputs {
    /// This repo's package resolution, from the content the scan fetched.
    pub fn package(&self) -> PackageResolution {
        resolve_package_name(&self.foundry, self.release_workflow.as_deref())
    }
}

/// Resolve the repo's soldeer package name: the manifest's release-metadata
/// table first (both spellings, `foundry_package_name`), then the release
/// workflow's `soldeer-package:` input.
///
/// rainix#335 drops the release-metadata table from `foundry.toml` entirely —
/// version intent moves to `next-v*` tags — so a migrated repo's manifest has
/// nothing to say about the package and the workflow input is the canonical
/// name. Both release lifecycles pass it (`rainix-autopublish` on merge to
/// main, `rainix-tag-release` on `sol-v*` tags), so the fallback covers every
/// publishing repo, migrated or not.
pub fn resolve_package_name(foundry: &str, release_workflow: Option<&str>) -> PackageResolution {
    if let Some(name) = foundry_package_name(foundry) {
        return PackageResolution::Named(name);
    }
    match release_workflow {
        None => PackageResolution::NoPackage,
        Some(workflow) => match release_workflow_package_name(workflow) {
            Some(name) => PackageResolution::Named(name),
            None => PackageResolution::Unknown,
        },
    }
}

/// Extract the `soldeer-package:` input value from a release workflow, if one
/// can be read.
///
/// Line-anchored like the rest of the workflow readers (`detect_signals`), not
/// a YAML parse: the input is a `with:` scalar both reusable lifecycles take,
/// and the shapes that occur are a bare name, a quoted name, and a trailing
/// comment. The guards carry meaning: a commented-out line is not an input,
/// and a `${{ … }}` expression is a reference the scan cannot evaluate —
/// joining on its literal text would join nothing. Both read as "no name
/// here", which the caller keeps apart from "no workflow at all".
pub fn release_workflow_package_name(workflow: &str) -> Option<String> {
    static RE_SOLDEER_PKG: OnceLock<Regex> = OnceLock::new();
    // `[ \t]` rather than `\s` around the separator: `\s` matches newlines,
    // and a key with no value on its line must not swallow the next line.
    let re_pkg = RE_SOLDEER_PKG.get_or_init(|| {
        re(r#"(?m)^[ \t]*["']?soldeer-package["']?[ \t]*:[ \t]*("[^"\n]*"|'[^'\n]*'|[^#\s]+)"#)
    });
    let raw = re_pkg.captures(workflow)?.get(1)?.as_str();
    let name = raw.trim_matches(|c| c == '"' || c == '\'').trim();
    if name.is_empty() || name.contains("${{") {
        return None;
    }
    Some(name.to_string())
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
    // soldeer-unpublished: the manifest names a package but the registry has no
    // revision of it.
    if inputs.soldeer_published == Some(false) {
        out.push("soldeer-unpublished");
    }
    if stale_foundry_lock(inputs) {
        out.push("stale-foundry-lock");
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

    // ---- stale-foundry-lock ----

    fn locked(lock: &str, gitmodules: RepoFile) -> RepoInputs {
        RepoInputs {
            foundry_lock: RepoFile::Present(lock.into()),
            gitmodules,
            ..Default::default()
        }
    }

    /// Verbatim from rainlanguage/rain.solmem: one pin, no `.gitmodules`, so the
    /// pin restores nothing and `forge build` warns about it. The rev is
    /// forge-std v1.14.0 while the build actually resolves 1.16.1 through
    /// soldeer — the file is not merely dead, it disagrees with the build.
    #[test]
    fn a_pin_with_no_submodule_to_restore_is_stale() {
        let lock = r#"{
  "lib/forge-std": {
    "rev": "1801b0541f4fda118a10798fd3486bb7051c5dd6"
  }
}"#;
        assert_eq!(
            dead_foundry_lock_pins(lock, ""),
            vec!["lib/forge-std".to_string()]
        );
        assert!(detect_signals(&locked(lock, RepoFile::Absent)).contains(&"stale-foundry-lock"));
    }

    /// Verbatim from rainlanguage/flow: six pins, and `.gitmodules` declares all
    /// six. The lockfile is doing its job and this repo is NOT in debt — judging
    /// by the mere PRESENCE of `foundry.lock` would flag it wrongly.
    #[test]
    fn a_lockfile_whose_pins_are_real_submodules_is_live() {
        let lock = r#"{
  "lib/forge-std": { "rev": "1d9650e951204a0ddce9ff89c32f1997984cef4d" },
  "lib/rain.solmem": { "rev": "6414ab88a017eacf2b263e9e08d0787fbd677192" }
}"#;
        let gitmodules = "\
[submodule \"lib/forge-std\"]
\tpath = lib/forge-std
\turl = https://github.com/foundry-rs/forge-std
[submodule \"lib/rain.solmem\"]
\tpath = lib/rain.solmem
\turl = https://github.com/rainprotocol/rain.solmem
";
        assert!(dead_foundry_lock_pins(lock, gitmodules).is_empty());
        assert!(
            !detect_signals(&locked(lock, RepoFile::Present(gitmodules.into())))
                .contains(&"stale-foundry-lock")
        );
    }

    /// Half-migrated: one submodule removed, its pin left behind. The live pin
    /// must not excuse the dead one — `forge build` still warns about it.
    #[test]
    fn only_the_pins_without_a_submodule_are_reported() {
        let lock = r#"{"lib/forge-std": {"rev": "a"}, "lib/rain.solmem": {"rev": "b"}}"#;
        let gitmodules = "[submodule \"lib/forge-std\"]\n\tpath = lib/forge-std\n";
        assert_eq!(
            dead_foundry_lock_pins(lock, gitmodules),
            vec!["lib/rain.solmem".to_string()]
        );
        assert!(
            detect_signals(&locked(lock, RepoFile::Present(gitmodules.into())))
                .contains(&"stale-foundry-lock")
        );
    }

    /// Verbatim from ST0x-Technology/st0x.issuance, which carries a `foundry.lock`
    /// holding `{}`. It pins nothing, so nothing is dead and `forge build` emits
    /// no warning — an empty lockfile is not debt, and a rule keyed on the file
    /// existing would invent a finding here.
    #[test]
    fn an_empty_lockfile_pins_nothing_and_is_not_stale() {
        assert!(dead_foundry_lock_pins("{}", "").is_empty());
        assert!(!detect_signals(&locked("{}", RepoFile::Absent)).contains(&"stale-foundry-lock"));
    }

    /// Verbatim from ST0x-Technology/st0x.liquidity, whose pins nest `tag` as an
    /// object. Only TOP-LEVEL keys are paths: a scan that walked nested keys
    /// would report a pinned path literally named `tag`.
    #[test]
    fn a_nested_tag_object_is_not_mistaken_for_a_pinned_path() {
        let lock = r#"{
  "lib/forge-std": {
    "tag": { "name": "v1.14.0", "rev": "1801b0541f4fda118a10798fd3486bb7051c5dd6" }
  }
}"#;
        assert_eq!(
            dead_foundry_lock_pins(lock, ""),
            vec!["lib/forge-std".to_string()],
            "the nested `tag` key is a pin's VALUE, never a path"
        );
    }

    /// A section whose `path` line is missing still declares a submodule, and
    /// the section name is where its path is written.
    #[test]
    fn a_submodule_is_recognised_by_its_section_name_too() {
        let lock = r#"{"lib/forge-std": {"rev": "a"}}"#;
        assert!(dead_foundry_lock_pins(lock, "[submodule \"lib/forge-std\"]\n").is_empty());
    }

    /// A trailing slash is a spelling of the path, not a different path. It is
    /// trimmed on BOTH sides before they are compared, because the comparison is
    /// what condemns a pin: leave either side untrimmed and `lib/forge-std/`
    /// fails to match `lib/forge-std`, so a pin whose submodule is right there
    /// gets reported dead. This signal's whole value is that it does not invent
    /// findings, and a string-equality detail is enough to break that.
    #[test]
    fn a_trailing_slash_is_the_same_path_on_either_side() {
        // Slash on the PIN.
        assert!(
            dead_foundry_lock_pins(
                r#"{"lib/forge-std/": {"rev": "a"}}"#,
                "[submodule \"x\"]\n\tpath = lib/forge-std\n"
            )
            .is_empty(),
            "a pin written with a trailing slash is the same path as the submodule"
        );
        // Slash on the SUBMODULE.
        assert!(
            dead_foundry_lock_pins(
                r#"{"lib/forge-std": {"rev": "a"}}"#,
                "[submodule \"x\"]\n\tpath = lib/forge-std/\n"
            )
            .is_empty(),
            "a submodule path written with a trailing slash still declares the pin"
        );
        // …and the normalisation does not make DIFFERENT paths equal.
        assert_eq!(
            dead_foundry_lock_pins(
                r#"{"lib/forge-std/": {"rev": "a"}}"#,
                "[submodule \"x\"]\n\tpath = lib/rain.solmem/\n"
            ),
            vec!["lib/forge-std".to_string()]
        );
    }

    /// An unreadable input is not evidence. A rate-limited `.gitmodules` fetch
    /// read as "this repo has no submodules" would condemn every pin in a repo
    /// whose submodules are all present — a finding manufactured from a network
    /// blip.
    #[test]
    fn an_unreadable_input_flags_nobody() {
        let lock = r#"{"lib/forge-std": {"rev": "a"}}"#;
        assert!(!detect_signals(&RepoInputs {
            foundry_lock: RepoFile::Present(lock.into()),
            gitmodules: RepoFile::Unreadable,
            ..Default::default()
        })
        .contains(&"stale-foundry-lock"));
        // …and the same when it is the lock itself that could not be read.
        assert!(!detect_signals(&RepoInputs {
            foundry_lock: RepoFile::Unreadable,
            gitmodules: RepoFile::Absent,
            ..Default::default()
        })
        .contains(&"stale-foundry-lock"));
    }

    /// A lock that will not parse leaves its pins UNKNOWN. This signal names
    /// specific dead paths, so it cannot make that claim about a file it could
    /// not read — and must not silently invent one either.
    #[test]
    fn an_unparseable_lock_claims_nothing() {
        for junk in ["not json [[[", "[1,2,3]", "\"a string\"", ""] {
            assert!(
                dead_foundry_lock_pins(junk, "").is_empty(),
                "{junk} produced pins"
            );
            assert!(
                !detect_signals(&locked(junk, RepoFile::Absent)).contains(&"stale-foundry-lock")
            );
        }
    }

    #[test]
    fn foundry_package_name_parsing() {
        assert_eq!(
            foundry_package_name("[package]\nname = \"rain.vats\"\nversion = \"1.0\""),
            Some("rain.vats".to_string())
        );
        // name outside the package table (e.g. in [profile.default]) must NOT match
        assert_eq!(
            foundry_package_name("[profile.default]\nname = \"nope\""),
            None
        );
        assert_eq!(foundry_package_name("[dependencies]\nfoo = \"1\""), None);
        assert_eq!(foundry_package_name(""), None);
    }

    /// The org moves soldeer release metadata out of a bare `[package]` (which
    /// forge warns about, and `forge config --fix` rewrites into
    /// `[profile.package]`) and into the `[external.*]` tree it reserves for
    /// other tools. Both spellings must read: the rename lands repo by repo, and
    /// a name this reader misses deletes the repo from the dependency graph
    /// along with every edge into it.
    #[test]
    fn foundry_package_name_reads_the_external_package_table() {
        assert_eq!(
            foundry_package_name(
                "[external.package]\nname = \"rain-sol-codegen\"\nversion = \"0.1.36\""
            ),
            Some("rain-sol-codegen".to_string())
        );
        // dotted key and quoted-segment header are the SAME table to every tool
        // that reads the file, so they are the same table here.
        assert_eq!(
            foundry_package_name("external.package.name = \"rain-sol-codegen\""),
            Some("rain-sol-codegen".to_string())
        );
        assert_eq!(
            foundry_package_name("[\"external\".\"package\"]\nname = \"rain-sol-codegen\""),
            Some("rain-sol-codegen".to_string())
        );
        // mid-rename, carrying both, is one package either way
        assert_eq!(
            foundry_package_name(
                "[package]\nname = \"rain-x\"\n[external.package]\nname = \"rain-x\""
            ),
            Some("rain-x".to_string())
        );
        // a package table with no usable name is not an answer — the other
        // spelling still gets its turn
        assert_eq!(
            foundry_package_name(
                "[package]\nversion = \"0.1.0\"\n[external.package]\nname = \"rain-x\""
            ),
            Some("rain-x".to_string())
        );
        // an empty name is not a package name — nothing joins on ""
        assert_eq!(
            foundry_package_name("[external.package]\nname = \"\""),
            None
        );
        // `[external]` is a whole tree of other tools' config: only `package` in it
        // is the release metadata.
        assert_eq!(
            foundry_package_name("[external.other-tool]\nname = \"nope\""),
            None
        );
        // what `forge config --fix` turns a bare `[package]` INTO: a profile named
        // "package", which is foundry config and not release metadata.
        assert_eq!(
            foundry_package_name("[profile.package]\nname = \"nope\""),
            None
        );
    }

    /// The real rain.sol.codegen manifest shape, which the previous line-wise
    /// section match read as "no package at all" — detaching the node and all 13
    /// edges into it from the audit graph. `[dependencies]` reads the same either
    /// way, so the fixture pins both halves at once.
    #[test]
    fn external_package_manifest_keeps_name_and_dependencies() {
        const CODEGEN: &str = r#"
# Release metadata, not foundry config.
[external.package]
name = "rain-sol-codegen"
version = "0.1.36"

[profile.default]
src = 'src'
solc = "0.8.25"

[dependencies]
forge-std = "1.16.2"

[soldeer]
recursive_deps = false
"#;
        assert_eq!(
            foundry_package_name(CODEGEN),
            Some("rain-sol-codegen".to_string())
        );
        assert_eq!(
            crate::graph::foundry_dependencies(CODEGEN).unwrap(),
            vec![crate::graph::Dep {
                package: "forge-std".to_string(),
                version_req: "1.16.2".to_string(),
            }]
        );
    }

    /// A manifest that will not parse yields no name — the same answer
    /// `graph::foundry_dependencies` gives it for `[dependencies]`, so one broken
    /// file cannot be half-read as authoritative by one reader and rejected by
    /// the other.
    #[test]
    fn unparseable_manifest_yields_no_package_name() {
        assert_eq!(foundry_package_name("[package]\nname = "), None);
        assert!(crate::graph::foundry_dependencies("[package]\nname = ").is_err());
    }

    #[test]
    fn clean_repo_no_signals() {
        let clean = RepoInputs {
            workflows: "uses: rainlanguage/rainix/.github/workflows/rainix-sol-test.yaml@main\nuses: actions/checkout@v4".into(),
            foundry: "[profile.default]\nsrc = \"src\"".into(),
            release_workflow: None,
            soldeer_published: Some(true),
            soldeer_version: Some("0.1.3".into()),
            foundry_lock: RepoFile::Absent,
            gitmodules: RepoFile::Absent,
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

    /// The real rain.factory files after rainlanguage/rain.factory#54
    /// (rainix#335): the manifest carries NO release-metadata table in either
    /// spelling, and the release workflow (merge lifecycle,
    /// `rainix-autopublish`) is the only thing that names the package.
    const FACTORY_FOUNDRY: &str = r#"
[profile.default]
src = 'src'
test = 'test'
out = 'out'

# See more config options https://github.com/foundry-rs/foundry/tree/master/config

solc = "0.8.25"
optimizer = true
optimizer_runs = 100000

evm_version = "cancun"

bytecode_hash = "none"
cbor_metadata = false

libs = ["dependencies"]

[fuzz]
runs = 2048

# Library repo: the ICloneable* interface surface only. The concrete CloneFactory,
# its deploy-pin snapshots and deploy scripts live in rain.factory.deploy
# (rainlanguage/rain.factory#46). Interfaces import nothing, so there are no
# Solidity dependencies beyond the test harness.
[dependencies]
forge-std = "1.16.1"

[soldeer]
recursive_deps = false
"#;

    const FACTORY_RELEASE: &str = r#"
name: Package Release
on:
  push:
    branches:
      - main
jobs:
  release:
    uses: rainlanguage/rainix/.github/workflows/rainix-autopublish.yaml@main
    with:
      # Library repo: the ICloneable* interface surface. `foundry.toml` version is
      # the NEXT (in-development) release; on a content change on merge, autopublish
      # publishes it and bumps to the next. The deploy half (concrete + deploy-pin
      # snapshots) lives in rain.factory.deploy (rainlanguage/rain.factory#46).
      soldeer-package: rain-factory
    secrets: inherit
"#;

    /// The real rain.factory.deploy release workflow: the OTHER lifecycle
    /// (`rainix-tag-release` on `sol-v*` tags), passing the same input.
    const DEPLOY_RELEASE: &str = r#"
name: Package Release
# Deploy repo: a manual `sol-v*` tag is the sole release trigger. The tag names
# the version; rainix-tag-release verifies prod exists at the regenerated pins,
# publishes rain-factory-deploy to Soldeer, and commits the frozen snapshot back
# to main. The on-chain deploy is separate and manual (rainix-manual-sol-artifacts
# dispatch), run before tagging. Nothing publishes on merge, so [package].version
# and the frozen src/generated/<tag>/ snapshot it names only ever move together.
on:
  push:
    tags:
      - sol-v*
jobs:
  release:
    uses: rainlanguage/rainix/.github/workflows/rainix-tag-release.yaml@main
    with:
      soldeer-package: rain-factory-deploy
      snapshot-generate-cmd: forge script ./script/BuildPointers.sol && forge fmt
    secrets: inherit
"#;

    /// The real rain.factory.deploy manifest: unmigrated (still carrying
    /// `[package]`), pinning `rain-factory = "0.1.5"` — the consumer side of
    /// the edge the 2026-08-19 scan lost.
    const DEPLOY_FOUNDRY: &str = r#"
[package]
name = "rain-factory-deploy"
version = "0.1.5"

[profile.default]
src = 'src'
test = 'test'
out = 'out'

# See more config options https://github.com/foundry-rs/foundry/tree/master/config

solc = "0.8.25"
optimizer = true
optimizer_runs = 100000

evm_version = "cancun"

bytecode_hash = "none"
cbor_metadata = false

# BuildPointers reads the version from foundry.toml and writes the generated
# per-tag snapshots + the current-pin lib under src/. Nothing else in this repo
# touches the filesystem.
fs_permissions = [
  { access = "read", path = "./foundry.toml" },
  { access = "read-write", path = "./src" },
]
libs = ["dependencies"]

[fuzz]
runs = 2048

[dependencies]
forge-std = "1.16.1"
"@openzeppelin-contracts" = "5.6.1"
"rain-extrospection" = "0.1.1"
"rain-deploy" = "0.1.3"
"rain-sol-codegen" = "0.1.3"
"rain-factory" = "0.1.5"

[soldeer]
recursive_deps = false

[rpc_endpoints]
arbitrum = "${ARBITRUM_RPC_URL}"
base = "${BASE_RPC_URL}"
base_sepolia = "${BASE_SEPOLIA_RPC_URL}"
flare = "${FLARE_RPC_URL}"
polygon = "${POLYGON_RPC_URL}"

[etherscan]
arbitrum = { key = "${CI_DEPLOY_ARBITRUM_ETHERSCAN_API_KEY}" }
base = { key = "${CI_DEPLOY_BASE_ETHERSCAN_API_KEY}" }
base_sepolia = { key = "${CI_DEPLOY_BASE_SEPOLIA_ETHERSCAN_API_KEY}" }
flare = { key = "${CI_DEPLOY_FLARE_ETHERSCAN_API_KEY}" }
polygon = { key = "${CI_DEPLOY_POLYGON_ETHERSCAN_API_KEY}" }
"#;

    /// Both live release lifecycles name the package the same way, so one
    /// reader covers the org: `rainix-autopublish` on merge (rain.factory) and
    /// `rainix-tag-release` on `sol-v*` tags (rain.factory.deploy), each
    /// passing `soldeer-package:` to the reusable workflow.
    #[test]
    fn release_workflow_package_name_reads_both_lifecycles() {
        assert_eq!(
            release_workflow_package_name(FACTORY_RELEASE),
            Some("rain-factory".to_string())
        );
        assert_eq!(
            release_workflow_package_name(DEPLOY_RELEASE),
            Some("rain-factory-deploy".to_string())
        );
    }

    /// What must and must not read as the input: quoting is YAML's business,
    /// a trailing comment is not part of the name, a commented-out line is not
    /// an input at all, and a `${{ … }}` expression is a reference the scan
    /// cannot evaluate — resolving it to the literal would join nothing.
    #[test]
    fn release_workflow_package_name_guards() {
        assert_eq!(
            release_workflow_package_name("      soldeer-package: \"rain-factory\""),
            Some("rain-factory".to_string())
        );
        assert_eq!(
            release_workflow_package_name("      soldeer-package: 'rain-factory'"),
            Some("rain-factory".to_string())
        );
        assert_eq!(
            release_workflow_package_name("      soldeer-package: rain-factory # the name"),
            Some("rain-factory".to_string())
        );
        assert_eq!(
            release_workflow_package_name("      # soldeer-package: rain-factory"),
            None
        );
        assert_eq!(
            release_workflow_package_name("      soldeer-package: ${{ inputs.pkg }}"),
            None
        );
        // a key with no value on its line must not swallow the NEXT line
        assert_eq!(
            release_workflow_package_name("      soldeer-package:\n      other: value"),
            None
        );
        assert_eq!(release_workflow_package_name(""), None);
        assert_eq!(
            release_workflow_package_name("      soldeer-package: \"\""),
            None
        );
    }

    /// The regression itself, on the real post-#335 rain.factory pair: the
    /// manifest alone yields no name (that is rainix#335's whole point), and
    /// the resolution must come through the workflow instead of deleting the
    /// node from the graph.
    #[test]
    fn migrated_repo_resolves_package_from_release_workflow() {
        // premise: the migrated manifest genuinely has no release metadata
        assert_eq!(foundry_package_name(FACTORY_FOUNDRY), None);
        assert_eq!(
            resolve_package_name(FACTORY_FOUNDRY, Some(FACTORY_RELEASE)),
            PackageResolution::Named("rain-factory".to_string())
        );
        let inputs = RepoInputs {
            foundry: FACTORY_FOUNDRY.into(),
            release_workflow: Some(FACTORY_RELEASE.into()),
            ..Default::default()
        };
        assert_eq!(
            inputs.package(),
            PackageResolution::Named("rain-factory".to_string())
        );
    }

    /// An unmigrated repo keeps resolving from the manifest — the workflow is
    /// the fallback, not a second authority — and an unparseable manifest does
    /// not block the workflow answer.
    #[test]
    fn manifest_name_resolves_first_when_present() {
        assert_eq!(
            resolve_package_name(
                "[package]\nname = \"rain-factory-deploy\"\nversion = \"0.1.5\"",
                Some(DEPLOY_RELEASE)
            ),
            PackageResolution::Named("rain-factory-deploy".to_string())
        );
        assert_eq!(
            resolve_package_name("[package]\nname = ", Some(FACTORY_RELEASE)),
            PackageResolution::Named("rain-factory".to_string())
        );
        // sources that DISAGREE pin the ruled order: the manifest speaks first,
        // and the workflow only when the manifest lacks the name (rainix#335
        // ruling: "resolve from there when foundry.toml lacks the name").
        assert_eq!(
            resolve_package_name("[package]\nname = \"manifest-name\"", Some(FACTORY_RELEASE)),
            PackageResolution::Named("manifest-name".to_string())
        );
    }

    /// Per the repo's own rule that a missing `[dependencies]` must not read
    /// as zero deps: a release workflow that exists but yields no name is
    /// UNKNOWN, never "publishes nothing". Only a repo with no release
    /// workflow at all is a known non-publisher.
    #[test]
    fn unreadable_release_workflow_is_unknown_never_no_package() {
        // listed but unreadable body
        assert_eq!(
            resolve_package_name("", Some("")),
            PackageResolution::Unknown
        );
        // readable but passing only an expression the scan cannot evaluate
        assert_eq!(
            resolve_package_name(
                "",
                Some("jobs:\n  release:\n    with:\n      soldeer-package: ${{ inputs.pkg }}\n")
            ),
            PackageResolution::Unknown
        );
        assert!(!resolve_package_name("", Some("")).known());
        assert_eq!(resolve_package_name("", Some("")).name(), None);
        // no release workflow, no metadata: a real non-publisher, and KNOWN
        assert_eq!(resolve_package_name("", None), PackageResolution::NoPackage);
        assert!(resolve_package_name("", None).known());
        assert_eq!(resolve_package_name("", None).name(), None);
    }

    /// End to end over the real file shapes on both sides: rain.factory's
    /// package resolves through its release workflow, so the
    /// `rain.factory.deploy -> rain.factory` edge that vanished from the
    /// 2026-08-19 scan (the manifest's release metadata was dropped by
    /// rainlanguage/rain.factory#54) is drawn again.
    #[test]
    fn workflow_resolved_package_restores_the_consumer_edge() {
        use crate::graph::{graph_edges, Node};
        use crate::protofire::ExternalAudit;

        let resolved = resolve_package_name(FACTORY_FOUNDRY, Some(FACTORY_RELEASE));
        let factory = Node {
            repo: "rain.factory".to_string(),
            package: resolved.name().map(str::to_string),
            package_known: resolved.known(),
            version: None,
            deps: crate::graph::foundry_dependencies(FACTORY_FOUNDRY).unwrap(),
            deps_known: true,
            audit: ExternalAudit::Never,
        };
        let deploy = Node {
            repo: "rain.factory.deploy".to_string(),
            package: resolve_package_name(DEPLOY_FOUNDRY, Some(DEPLOY_RELEASE))
                .name()
                .map(str::to_string),
            package_known: true,
            version: None,
            deps: crate::graph::foundry_dependencies(DEPLOY_FOUNDRY).unwrap(),
            deps_known: true,
            audit: ExternalAudit::Never,
        };
        let edges = graph_edges(&[factory, deploy]).unwrap();
        assert!(
            edges
                .iter()
                .any(|e| e.from == "rain.factory.deploy" && e.to == "rain.factory"),
            "the rain.factory.deploy -> rain.factory edge must survive the manifest \
             losing its release metadata: {edges:?}"
        );
    }
}
