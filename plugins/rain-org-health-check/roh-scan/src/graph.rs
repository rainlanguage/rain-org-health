//! The first-party dependency graph beneath the org's Solidity repos (#71).
//!
//! Auditing a repo whose dependencies are unaudited is auditing on sand: a
//! finding in a leaf propagates to every consumer above it, and a green consumer
//! audit reads as assurance it has not earned. The graph makes that visible, and
//! answers what a sorted list cannot: given a finding in X, who inherits it?
//!
//! Pure: the caller does the fetching, this relates what it fetched.

use crate::protofire;
use std::collections::{BTreeMap, BTreeSet};

/// Whether a repo's audit clears the consumers above it.
///
/// Takes the verdict `protofire::classify_external_audit` already emits rather
/// than restating the taxonomy: `current`/`stale`/`never`/`na`/`unknown` is one
/// definitionally-locked set, and a second copy of it would drift the day a
/// verdict is added or renamed.
pub fn is_cleared(audit: protofire::ExternalAudit) -> bool {
    match audit {
        protofire::ExternalAudit::Current => true,
        // Stale reviewed code that has since changed. Never has no audit. Na has
        // a PDF but no tag to date it against, so nothing pins WHAT was audited.
        // Unknown is a FAILED fetch: indeterminate, and must be read as neither
        // cleared nor a confirmed gap. Matched exhaustively so a new verdict
        // cannot default to "does not clear" without someone deciding it should.
        protofire::ExternalAudit::Stale
        | protofire::ExternalAudit::Never
        | protofire::ExternalAudit::Na
        | protofire::ExternalAudit::Unknown => false,
    }
}

/// One repo in the org, as the graph needs it.
#[derive(Clone, Debug)]
pub struct Node {
    /// The repo name WITHIN the scanned org (`rain.solmem`), as `gh repo list`
    /// returns it — not `owner/name`. The scan is org-scoped, so the org is
    /// implicit and the bare name is the identity everything else keys on.
    pub repo: String,
    /// The soldeer `[package].name` this repo publishes, if any. This is what
    /// consumers name it by, so it is the graph's join key.
    pub package: Option<String>,
    /// Soldeer package names from this repo's `[dependencies]`. Empty with
    /// `deps_known == false` means the manifest would not parse, NOT that the
    /// repo has no dependencies.
    pub deps: Vec<String>,
    /// False when `foundry.toml` would not parse. Its dependencies are unknown,
    /// so nothing may claim its ground is clear.
    pub deps_known: bool,
    /// The verdict from `protofire::classify_external_audit`.
    pub audit: protofire::ExternalAudit,
}

/// Two repos publishing the same soldeer package name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicatePackage {
    pub package: String,
    pub repos: Vec<String>,
}

/// package name -> the repo that publishes it.
///
/// Built once and validated: a `collect()` over duplicate keys keeps an
/// arbitrary winner, and every edge and blocker for that package would then
/// point at whichever repo happened to land last. Two repos publishing one
/// package is an org-level error, so it is reported rather than resolved.
pub fn package_index(nodes: &[Node]) -> Result<BTreeMap<&str, &Node>, DuplicatePackage> {
    let mut idx: BTreeMap<&str, &Node> = BTreeMap::new();
    for n in nodes {
        let Some(pkg) = n.package.as_deref() else {
            continue;
        };
        if let Some(prev) = idx.insert(pkg, n) {
            let mut repos = vec![prev.repo.clone(), n.repo.clone()];
            repos.sort();
            return Err(DuplicatePackage {
                package: pkg.to_string(),
                repos,
            });
        }
    }
    Ok(idx)
}

/// One first-party dependency edge: `from` consumes `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// Extract the first-party dependency package names from a `foundry.toml`.
///
/// Parsed as TOML rather than scanned line-wise: real manifests quote some keys
/// and not others (`"rain-solmem" = "0.1.3"` beside `rainlang = "0.1.5"`), carry
/// comments inside the section, and soldeer also permits the inline-table form
/// (`dep = { version = "1", url = "..." }`). A missed dependency is not a
/// cosmetic bug here — it silently drops an edge, so the campaign would order an
/// audit before something it depends on, which is the exact failure this module
/// exists to prevent.
///
/// Returns every declared dependency name; mapping them to repos (and dropping
/// third-party ones) is `build_campaign`'s job, since only it knows the org.
pub fn foundry_dependencies(foundry: &str) -> Result<Vec<String>, MalformedManifest> {
    let value = foundry
        .parse::<toml::Value>()
        .map_err(|e| MalformedManifest(e.to_string()))?;
    // No `[dependencies]` is a real, readable answer: the repo declares none.
    // A manifest that will not parse is NOT that answer — it is "unknown", and
    // collapsing the two would let a broken manifest read as clear ground and
    // make the repo look actionable on false grounds.
    let Some(table) = value.get("dependencies").and_then(|d| d.as_table()) else {
        return Ok(Vec::new());
    };
    Ok(table.keys().cloned().collect())
}

/// A `foundry.toml` that will not parse. Its dependencies are unknown, which is
/// distinct from having none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MalformedManifest(pub String);

/// Every first-party edge in the scan, for the whole org rather than one
/// entrypoint's slice.
///
/// The graph is the primary artifact, not a by-product of ordering. A
/// topological order is only ONE of the many valid linearisations of it, so a
/// numbered list invents precedence between independent leaves that a reader
/// then believes; the graph states exactly what depends on what and no more. It
/// also answers the question the order cannot: given a finding in X, who
/// inherits it (#71)?
///
/// Third-party deps name no repo here and are dropped, so an edge always joins
/// two scanned repos.
pub fn graph_edges(nodes: &[Node]) -> Result<Vec<Edge>, DuplicatePackage> {
    let by_package = package_index(nodes)?;
    let mut edges: Vec<Edge> = Vec::new();
    for node in nodes {
        for dep_pkg in &node.deps {
            if let Some(dep) = by_package.get(dep_pkg.as_str()) {
                edges.push(Edge {
                    from: node.repo.clone(),
                    to: dep.repo.clone(),
                });
            }
        }
    }
    edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    edges.dedup();
    Ok(edges)
}

/// Every first-party repo beneath `repo`, transitively.
fn deps_beneath<'a>(
    repo: &'a str,
    by_repo: &BTreeMap<&'a str, &'a Node>,
    by_package: &BTreeMap<&'a str, &'a Node>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<&str> = vec![repo];
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        let Some(node) = by_repo.get(cur) else {
            continue;
        };
        for dep_pkg in &node.deps {
            if let Some(dep) = by_package.get(dep_pkg.as_str()) {
                if dep.repo != repo {
                    out.insert(dep.repo.clone());
                }
                stack.push(&dep.repo);
            }
        }
    }
    out
}

/// The repos beneath each node that are NOT cleared, keyed by repo.
///
/// Empty means the ground beneath is solid, so a finding there is genuinely that
/// repo's. Walked transitively, not over direct deps: a cleared dependency
/// standing on an unaudited one of its own is still sand.
pub fn blockers(nodes: &[Node]) -> Result<BTreeMap<String, Vec<String>>, DuplicatePackage> {
    let by_repo: BTreeMap<&str, &Node> = nodes.iter().map(|n| (n.repo.as_str(), n)).collect();
    let by_package = package_index(nodes)?;
    Ok(nodes
        .iter()
        .map(|n| {
            let mut b: Vec<String> = deps_beneath(&n.repo, &by_repo, &by_package)
                .into_iter()
                .filter(|d| !is_cleared(by_repo[d.as_str()].audit))
                .collect();
            b.sort();
            (n.repo.clone(), b)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        repo: &str,
        package: Option<&str>,
        deps: &[&str],
        audit: protofire::ExternalAudit,
    ) -> Node {
        Node {
            repo: repo.to_string(),
            package: package.map(str::to_string),
            deps: deps.iter().map(|d| d.to_string()).collect(),
            deps_known: true,
            audit,
        }
    }

    /// Verbatim shapes from real manifests: quoted and unquoted keys side by
    /// side, a comment inside the section (raindex carries one explaining a
    /// rename), an inline table, and third-party entries mixed with first-party.
    #[test]
    fn parses_the_shapes_real_manifests_actually_use() {
        let foundry = r#"
[profile.default]
solc = "0.8.25"

[dependencies]
forge-std = "1.16.1"
"@openzeppelin-contracts" = "5.6.1"
# rainlanguage/rain.interpreter was renamed to rainlanguage/rainlang;
# its Soldeer registry name follows.
rainlang = "0.1.5"
"rain-solmem" = "0.1.3"
inline-form = { version = "0.1.0", url = "https://example.invalid/x.zip" }

[soldeer]
recursive_deps = false
"#;
        let mut deps = foundry_dependencies(foundry).expect("valid manifest");
        deps.sort();
        assert_eq!(
            deps,
            vec![
                "@openzeppelin-contracts",
                "forge-std",
                "inline-form",
                "rain-solmem",
                "rainlang"
            ]
        );
    }

    /// "declares none" is a real answer; "will not parse" is not. Collapsing
    /// them lets a broken manifest read as clear ground and makes the repo look
    /// actionable on false grounds.
    #[test]
    fn no_dependencies_is_ok_empty_but_malformed_is_an_error() {
        assert_eq!(
            foundry_dependencies("[profile.default]\nsolc = \"0.8.25\"\n"),
            Ok(Vec::new()),
            "a manifest with no [dependencies] declares none"
        );
        assert!(
            foundry_dependencies("this is not toml {{{").is_err(),
            "a malformed manifest must not read as no dependencies"
        );
    }

    /// Edges join scanned repos only: a third-party dep names no repo, and an
    /// edge to a node the graph does not contain renders as a phantom.
    #[test]
    fn edges_join_scanned_repos_and_drop_third_party() {
        let nodes = vec![
            node(
                "app",
                Some("app"),
                &["lib", "forge-std"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "lib",
                Some("lib"),
                &["@openzeppelin-contracts"],
                protofire::ExternalAudit::Stale,
            ),
        ];
        assert_eq!(
            graph_edges(&nodes).unwrap(),
            vec![Edge {
                from: "app".into(),
                to: "lib".into()
            }]
        );
    }

    /// Two repos on the same leaf yield two edges, not a merged one: the fan-in
    /// IS the blast radius the graph exists to show.
    #[test]
    fn edges_keep_every_consumer_of_a_shared_leaf() {
        let nodes = vec![
            node("a", Some("a"), &["core"], protofire::ExternalAudit::Never),
            node("b", Some("b"), &["core"], protofire::ExternalAudit::Never),
            node("core", Some("core"), &[], protofire::ExternalAudit::Stale),
        ];
        let edges = graph_edges(&nodes).unwrap();
        assert_eq!(edges.len(), 2, "{edges:?}");
        assert!(edges.iter().all(|e| e.to == "core"));
    }

    /// Two repos publishing one package: a plain collect() keeps an arbitrary
    /// winner and every edge for that package then points at whichever landed
    /// last. Reported, not resolved.
    #[test]
    fn duplicate_package_names_are_an_error_not_an_arbitrary_winner() {
        let nodes = vec![
            node(
                "first",
                Some("shared"),
                &[],
                protofire::ExternalAudit::Never,
            ),
            node(
                "second",
                Some("shared"),
                &[],
                protofire::ExternalAudit::Never,
            ),
            node(
                "app",
                Some("app"),
                &["shared"],
                protofire::ExternalAudit::Never,
            ),
        ];
        let err = package_index(&nodes).expect_err("duplicate package accepted");
        assert_eq!(err.package, "shared");
        assert_eq!(err.repos, vec!["first".to_string(), "second".to_string()]);
        assert!(
            graph_edges(&nodes).is_err(),
            "edges built over a duplicate package"
        );
        assert!(
            blockers(&nodes).is_err(),
            "blockers built over a duplicate package"
        );
    }

    /// Blockers are TRANSITIVE: a cleared direct dependency standing on an
    /// unaudited one of its own is still sand.
    #[test]
    fn blockers_walk_the_whole_tree_not_just_direct_deps() {
        let nodes = vec![
            node(
                "app",
                Some("app"),
                &["lib"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "lib",
                Some("lib"),
                &["core"],
                protofire::ExternalAudit::Current,
            ),
            node("core", Some("core"), &[], protofire::ExternalAudit::Never),
        ];
        let b = blockers(&nodes).unwrap();
        assert_eq!(b["app"], vec!["core".to_string()]);
        assert!(b["core"].is_empty(), "a leaf has solid ground");
    }

    /// Only CURRENT clears. Each of the other four leaves the consumer above
    /// standing on something unpinned, so each must block.
    #[test]
    fn every_non_current_verdict_blocks() {
        for audit in [
            protofire::ExternalAudit::Stale,
            protofire::ExternalAudit::Never,
            protofire::ExternalAudit::Na,
            protofire::ExternalAudit::Unknown,
        ] {
            assert!(!is_cleared(audit), "{audit:?} cleared");
            let nodes = vec![
                node(
                    "app",
                    Some("app"),
                    &["lib"],
                    protofire::ExternalAudit::Never,
                ),
                node("lib", Some("lib"), &[], audit),
            ];
            assert_eq!(
                blockers(&nodes).unwrap()["app"],
                vec!["lib".to_string()],
                "{audit:?} did not block"
            );
        }
        assert!(is_cleared(protofire::ExternalAudit::Current));
    }

    /// Third-party deps must not block: nobody here can audit forge-std, so
    /// treating it as a blocker would block every repo forever.
    #[test]
    fn third_party_deps_do_not_block() {
        let nodes = vec![node(
            "app",
            Some("app"),
            &["forge-std"],
            protofire::ExternalAudit::Never,
        )];
        assert!(blockers(&nodes).unwrap()["app"].is_empty());
    }
}
