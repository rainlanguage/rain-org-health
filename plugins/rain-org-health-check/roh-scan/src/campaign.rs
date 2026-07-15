//! Audit campaigns: order the first-party dependency tree beneath an entrypoint
//! so audits clear leaves first (issue #71).
//!
//! Auditing a repo whose dependencies are unaudited is auditing on sand: a
//! finding in a leaf propagates to every consumer above it, so a consumer audit
//! done first must be redone the moment the leaf turns out to be broken — and a
//! green consumer audit reads as assurance it has not earned.
//!
//! Pure: the caller does the fetching, this orders what it fetched.

use crate::protofire;
use std::collections::{BTreeMap, BTreeSet};

/// Whether a repo's audit clears the consumers above it.
///
/// Takes the verdict string `protofire::classify_external_audit` already emits
/// and compares it against that module's own constant, rather than restating the
/// taxonomy here: `current`/`stale`/`never`/`na` is one definitionally-locked set,
/// and a second copy of it would drift the day a verdict is added or renamed.
///
/// Only CURRENT clears. A stale audit reviewed code that has since changed, so a
/// consumer standing on it is still standing on unreviewed source.
pub fn is_cleared(audit: protofire::ExternalAudit) -> bool {
    match audit {
        protofire::ExternalAudit::Current => true,
        // Stale reviewed code that has since changed; Never/Na were never
        // audited; Unknown is a FAILED fetch, so coverage is indeterminate and
        // must not be read as cleared (nor as a confirmed gap). Matched
        // exhaustively so a new verdict cannot default to "does not clear"
        // without someone deciding it should.
        protofire::ExternalAudit::Stale
        | protofire::ExternalAudit::Never
        | protofire::ExternalAudit::Na
        | protofire::ExternalAudit::Unknown => false,
    }
}

/// One repo in the org, as the graph needs it.
#[derive(Clone, Debug)]
pub struct Node {
    /// `owner/name`.
    pub repo: String,
    /// The soldeer `[package].name` this repo publishes, if any. This is what
    /// consumers name it by, so it is the graph's join key.
    pub package: Option<String>,
    /// Soldeer package names from this repo's `[dependencies]`.
    pub deps: Vec<String>,
    /// The verdict from `protofire::classify_external_audit`.
    pub audit: protofire::ExternalAudit,
}

/// One step of the ordered campaign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub repo: String,
    pub audit: protofire::ExternalAudit,
    /// True when every first-party dependency beneath this node is cleared, so
    /// auditing it now yields findings that are genuinely THIS repo's.
    pub ready: bool,
    /// The uncleared repos beneath it. Non-empty exactly when `ready` is false.
    pub blocked_by: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CampaignError {
    /// The entrypoint is not a repo in the scan.
    UnknownEntrypoint(String),
    /// A dependency cycle. Reported rather than broken arbitrarily: any order
    /// picked from a cycle is a guess, and a guessed order is what this exists
    /// to eliminate.
    Cycle(Vec<String>),
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
pub fn foundry_dependencies(foundry: &str) -> Vec<String> {
    let Ok(value) = foundry.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(table) = value.get("dependencies").and_then(|d| d.as_table()) else {
        return Vec::new();
    };
    table.keys().cloned().collect()
}

/// Whether a campaign may be planned from this scan scope.
///
/// The graph resolves a dependency to a repo only if that repo was scanned; an
/// unscanned one is indistinguishable from a third-party package and is dropped.
/// So a scan limited to an explicit repo list silently loses edges and emits a
/// plausible WRONG order — the precise failure this module exists to prevent.
/// A campaign therefore requires the full org scan.
pub fn campaign_scope_ok(explicit_repo_list: bool) -> bool {
    !explicit_repo_list
}

/// Order the first-party tree beneath `entrypoint`, leaves first.
///
/// Dependencies that name no repo in `nodes` are third-party (`forge-std`,
/// `@openzeppelin-contracts`) and are dropped: they are out of scope for an org
/// audit campaign, and treating them as nodes would block every campaign forever
/// on repos nobody here can audit.
pub fn build_campaign(entrypoint: &str, nodes: &[Node]) -> Result<Vec<Step>, CampaignError> {
    let by_repo: BTreeMap<&str, &Node> = nodes.iter().map(|n| (n.repo.as_str(), n)).collect();
    let by_package: BTreeMap<&str, &Node> = nodes
        .iter()
        .filter_map(|n| n.package.as_deref().map(|p| (p, n)))
        .collect();

    if !by_repo.contains_key(entrypoint) {
        return Err(CampaignError::UnknownEntrypoint(entrypoint.to_string()));
    }

    let mut order: Vec<&str> = Vec::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut path: Vec<&str> = Vec::new();
    visit(
        entrypoint,
        &by_repo,
        &by_package,
        &mut order,
        &mut done,
        &mut path,
    )?;

    // A node is ready once every first-party repo beneath it is cleared. Walked
    // over the whole transitive set, not just direct deps: a cleared direct
    // dependency standing on an unaudited one of its own is still sand.
    let mut steps = Vec::with_capacity(order.len());
    for repo in order {
        let node = by_repo[repo];
        let mut blocked_by: Vec<String> = transitive_deps(repo, &by_repo, &by_package)
            .into_iter()
            .filter(|d| !is_cleared(by_repo[d.as_str()].audit))
            .collect();
        blocked_by.sort();
        steps.push(Step {
            repo: repo.to_string(),
            audit: node.audit,
            ready: blocked_by.is_empty(),
            blocked_by,
        });
    }
    Ok(steps)
}

/// Depth-first post-order: a node is emitted only after everything beneath it.
fn visit<'a>(
    repo: &'a str,
    by_repo: &BTreeMap<&'a str, &'a Node>,
    by_package: &BTreeMap<&'a str, &'a Node>,
    order: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> Result<(), CampaignError> {
    if done.contains(repo) {
        return Ok(());
    }
    if let Some(at) = path.iter().position(|p| *p == repo) {
        let mut cycle: Vec<String> = path[at..].iter().map(|s| s.to_string()).collect();
        cycle.push(repo.to_string());
        return Err(CampaignError::Cycle(cycle));
    }
    path.push(repo);
    for dep_pkg in &by_repo[repo].deps {
        if let Some(dep) = by_package.get(dep_pkg.as_str()) {
            visit(&dep.repo, by_repo, by_package, order, done, path)?;
        }
    }
    path.pop();
    done.insert(repo);
    order.push(repo);
    Ok(())
}

/// Every first-party repo reachable beneath `repo`, excluding itself.
fn transitive_deps<'a>(
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
            audit,
        }
    }

    /// Verbatim shapes from real manifests: quoted and unquoted keys side by
    /// side, a comment inside the section (raindex carries one explaining a
    /// rename), and third-party entries mixed in with first-party.
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
        let mut deps = foundry_dependencies(foundry);
        deps.sort();
        assert_eq!(
            deps,
            vec![
                "@openzeppelin-contracts",
                "forge-std",
                "inline-form",
                "rain-solmem",
                "rainlang",
            ]
        );
    }

    #[test]
    fn no_dependencies_section_or_unparseable_yields_none() {
        assert!(foundry_dependencies("[profile.default]\nsolc = \"0.8.25\"\n").is_empty());
        assert!(foundry_dependencies("this is not toml {{{").is_empty());
    }

    /// A campaign off a partial scan would drop the edges to unscanned repos and
    /// emit a confident wrong order, so the scope is refused rather than guessed.
    #[test]
    fn a_campaign_requires_the_full_org_scan() {
        assert!(campaign_scope_ok(false), "full scan must be allowed");
        assert!(
            !campaign_scope_ok(true),
            "explicit repo list must be refused"
        );
    }

    /// The campaign's whole purpose: the leaf precedes the consumer.
    #[test]
    fn orders_leaves_before_consumers() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["lib"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/lib",
                Some("lib"),
                &["core"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/core",
                Some("core"),
                &[],
                protofire::ExternalAudit::Never,
            ),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        let order: Vec<&str> = steps.iter().map(|s| s.repo.as_str()).collect();
        assert_eq!(order, vec!["org/core", "org/lib", "org/app"]);
    }

    /// Third-party deps name no repo, so they must not become nodes — otherwise
    /// every campaign blocks forever on packages nobody here can audit.
    #[test]
    fn third_party_deps_are_not_nodes_and_do_not_block() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["forge-std", "@openzeppelin-contracts", "lib"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/lib",
                Some("lib"),
                &["forge-std"],
                protofire::ExternalAudit::Current,
            ),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        let order: Vec<&str> = steps.iter().map(|s| s.repo.as_str()).collect();
        assert_eq!(order, vec!["org/lib", "org/app"]);
        assert!(steps[1].ready, "third-party deps blocked the campaign");
    }

    /// Readiness is transitive: a CURRENT direct dep standing on an unaudited
    /// one of its own is still sand, so the consumer is not ready.
    #[test]
    fn readiness_walks_the_whole_tree_not_just_direct_deps() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["lib"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/lib",
                Some("lib"),
                &["core"],
                protofire::ExternalAudit::Current,
            ),
            node(
                "org/core",
                Some("core"),
                &[],
                protofire::ExternalAudit::Never,
            ),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        let app = steps.iter().find(|s| s.repo == "org/app").unwrap();
        assert!(!app.ready, "app ready despite an unaudited transitive dep");
        assert_eq!(app.blocked_by, vec!["org/core".to_string()]);
    }

    /// Only a CURRENT audit clears. A stale one audited code that has since
    /// changed, so it leaves the consumer above standing on unreviewed source.
    #[test]
    fn stale_does_not_clear() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["lib"],
                protofire::ExternalAudit::Never,
            ),
            node("org/lib", Some("lib"), &[], protofire::ExternalAudit::Stale),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        let app = steps.iter().find(|s| s.repo == "org/app").unwrap();
        assert!(!app.ready);
        assert_eq!(app.blocked_by, vec!["org/lib".to_string()]);
    }

    #[test]
    fn a_leaf_is_ready_and_a_cleared_tree_makes_its_consumer_ready() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["lib"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/lib",
                Some("lib"),
                &[],
                protofire::ExternalAudit::Current,
            ),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        assert!(steps[0].ready, "leaf not ready");
        assert!(steps[1].ready, "consumer of a cleared leaf not ready");
    }

    /// A cycle must fail loudly: any order picked from one is a guess, and a
    /// guessed order is what this module exists to eliminate.
    #[test]
    fn cycles_fail_loudly_rather_than_picking_an_order() {
        let nodes = vec![
            node("org/a", Some("a"), &["b"], protofire::ExternalAudit::Never),
            node("org/b", Some("b"), &["a"], protofire::ExternalAudit::Never),
        ];
        match build_campaign("org/a", &nodes) {
            Err(CampaignError::Cycle(c)) => {
                assert!(
                    c.contains(&"org/a".to_string()) && c.contains(&"org/b".to_string()),
                    "{c:?}"
                )
            }
            other => panic!("expected a cycle error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_entrypoint_is_an_error_not_an_empty_plan() {
        let nodes = vec![node(
            "org/app",
            Some("app"),
            &[],
            protofire::ExternalAudit::Never,
        )];
        assert_eq!(
            build_campaign("org/nope", &nodes),
            Err(CampaignError::UnknownEntrypoint("org/nope".to_string()))
        );
    }

    /// A diamond emits each node once, still after both its dependencies.
    #[test]
    fn diamond_emits_each_node_once_after_its_deps() {
        let nodes = vec![
            node(
                "org/app",
                Some("app"),
                &["l", "r"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/l",
                Some("l"),
                &["core"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/r",
                Some("r"),
                &["core"],
                protofire::ExternalAudit::Never,
            ),
            node(
                "org/core",
                Some("core"),
                &[],
                protofire::ExternalAudit::Never,
            ),
        ];
        let steps = build_campaign("org/app", &nodes).unwrap();
        let order: Vec<&str> = steps.iter().map(|s| s.repo.as_str()).collect();
        assert_eq!(order.len(), 4, "node emitted more than once: {order:?}");
        let pos = |r: &str| order.iter().position(|x| *x == r).unwrap();
        assert!(pos("org/core") < pos("org/l") && pos("org/core") < pos("org/r"));
        assert!(pos("org/l") < pos("org/app") && pos("org/r") < pos("org/app"));
    }
}
