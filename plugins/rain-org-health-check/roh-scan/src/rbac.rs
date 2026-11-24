//! Where st0x.deploy's RBAC subgraph is published, and the role names its
//! sources spell out, for the Deployments page's "Role changes" tab.
//!
//! The role history itself is not read here. It changes between scans, so the
//! page queries the subgraph when it loads and shows the index as it is at that
//! moment; a copy taken by this scan would be hours stale by design. What this
//! module emits is the configuration the page cannot read for itself: the
//! per-network query URLs, and the role-id → name map, whose preimages live in
//! Solidity sources rather than in any log.
//!
//! The "Root registry" tab reads rain.deploy's registry subgraph, located the
//! same way from that repo's workflow and `networks.json`; see
//! [`build_location`].
//!
//! Everything that locates the subgraph is read from the deploy repo, not typed
//! here: the networks are `subgraph/networks.json`'s keys, and the deployment
//! name and query bases are the `subgraph-name` / `ormi-query-base` /
//! `goldsky-query-base` inputs its deploy workflow passes to rainix's
//! `rainix-subgraph-deploy` reusable, and which host a network is on is
//! rainix's own `GOLDSKY_NETWORKS` split. That
//! publishes each network as `<subgraph-name>-<network>` at version label
//! `<address>-<commit>`, which changes on every redeploy, and keeps the name
//! shape so a tag can move between versions. Consumers address the tag —
//! raindex's own settings read `…/raindex-base/prod/gn` — so this does too.

use regex::Regex;
use serde_json::{json, Map, Value};

/// The Ormi tag consumers read. rainix's `subgraph-deploy` keeps the
/// `<name>/<version>` shape "so an Ormi tag can move between versions", and
/// `prod` is the tag the org's consumers address (raindex's settings).
pub const PROD_TAG: &str = "prod";

/// The rainix source that defines which networks deploy to Goldsky.
pub const RAINIX_SUBGRAPH_TARGETS: (&str, &str, &str) = (
    "rainlanguage",
    "rainix",
    "rainix-static/src/subgraph_targets.rs",
);

/// rainix's `GOLDSKY_NETWORKS`: the networks its subgraph deploy publishes to
/// Goldsky; every other network is published to Ormi. `None` when the constant
/// is not in `src`.
pub fn parse_goldsky_networks(src: &str) -> Option<Vec<String>> {
    let decl =
        Regex::new(r"GOLDSKY_NETWORKS\s*:\s*&\[\s*&str\s*\]\s*=\s*&\[([^\]]*)\]").ok()?;
    let body = decl.captures(src)?.get(1)?.as_str();
    let item = Regex::new(r#""([^"]*)""#).ok()?;
    Some(item.captures_iter(body).map(|c| c[1].to_string()).collect())
}

/// Why a location cannot be built without rainix's network→host split.
pub fn goldsky_networks_unread() -> String {
    let (org, repo, path) = RAINIX_SUBGRAPH_TARGETS;
    format!("could not read GOLDSKY_NETWORKS from {org}/{repo} {path}")
}

/// Where the subgraph is published, as the deploy workflow names it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SubgraphSource {
    /// `subgraph-name` — each network is published as `<name>-<network>`.
    pub name: String,
    /// `ormi-query-base` — the public query root for the Ormi account.
    pub ormi_query_base: Option<String>,
    /// `goldsky-query-base` — the public query root for the Goldsky project.
    pub goldsky_query_base: Option<String>,
}

/// Read the `subgraph-name`, `ormi-query-base` and `goldsky-query-base` inputs
/// out of the deploy workflow.
/// `None` without a name and at least one query base, or when a query base is
/// not https: the page POSTs to it and renders the answer as the chain's role
/// history, so a response rewritten in transit would be shown as that history.
pub fn parse_deploy_workflow(yaml: &str) -> Option<SubgraphSource> {
    let value = |key: &str| -> Option<String> {
        let re = Regex::new(&format!(r#"(?m)^\s*{key}:\s*["']?([^"'\s#]+)"#)).ok()?;
        re.captures(yaml)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim_end_matches('/').to_string())
    };
    let name = value("subgraph-name")?;
    let ormi_query_base = value("ormi-query-base");
    let goldsky_query_base = value("goldsky-query-base");
    let bases = [&ormi_query_base, &goldsky_query_base];
    if bases.iter().all(|b| b.is_none())
        || bases
            .iter()
            .any(|b| b.as_ref().is_some_and(|b| !b.starts_with("https://")))
    {
        return None;
    }
    Some(SubgraphSource {
        name,
        ormi_query_base,
        goldsky_query_base,
    })
}

/// The networks the subgraph is deployed to, in `networks.json` order. `None`
/// when the file does not parse as an object — unread, not "no networks".
pub fn parse_networks(json_src: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(json_src).ok()?;
    Some(v.as_object()?.keys().cloned().collect())
}

/// Each network's V5 `ConfigRegistry`, as `networks.json` declares it for the
/// subgraph to index, lowercased.
pub fn config_registries(json_src: &str) -> Map<String, Value> {
    let Ok(Value::Object(v)) = serde_json::from_str::<Value>(json_src) else {
        return Map::new();
    };
    v.iter()
        .filter_map(|(n, c)| {
            let a = c.get("ConfigRegistryV5")?.get("address")?.as_str()?;
            Some((n.clone(), Value::from(a.to_lowercase())))
        })
        .collect()
}

/// The page's chain name for a subgraph network name.
///
/// graph-cli names chains its own way (`mainnet`, `hyper-evm` / `hyperevm`);
/// the rest of the page uses the deploy repo's constant suffixes (`ethereum`,
/// `hyperevm`), which pick the explorer. `None` for a network neither
/// vocabulary covers, which leaves that network's addresses unlinked rather
/// than linked to the wrong chain.
pub fn chain_name(network: &str) -> Option<&'static str> {
    match network {
        "base" => Some("base"),
        "mainnet" => Some("ethereum"),
        "hyper-evm" | "hyperevm" => Some("hyperevm"),
        "robinhood" => Some("robinhood"),
        "bsc" => Some("bsc"),
        "0g" => Some("zerog"),
        _ => None,
    }
}

/// The tagged query URL for one network, on the host `goldsky_networks` routes
/// it to. `Err` names the query base the deploy workflow does not give.
pub fn endpoint(
    src: &SubgraphSource,
    goldsky_networks: &[String],
    network: &str,
) -> Result<String, String> {
    let (input, base) = if goldsky_networks.iter().any(|n| n == network) {
        ("goldsky-query-base", &src.goldsky_query_base)
    } else {
        ("ormi-query-base", &src.ormi_query_base)
    };
    let base = base
        .as_ref()
        .ok_or_else(|| format!("deploy-subgraph.yaml has no {input} for {network}"))?;
    Ok(format!(
        "{}/subgraphs/{}-{}/{}/gn",
        base, src.name, network, PROD_TAG
    ))
}

/// Every role name the deploy repo spells out, keyed by its on-chain id.
///
/// The subgraph stores role ids, never names — the preimage is not in the log.
/// So a name is shown only where a source this scan reads writes
/// `keccak256("NAME")` for it, plus each `bytes32` `*_ROLE` constant a source
/// defines as zero (`= 0x00` or `= bytes32(0)`), by its own name. Upper-case
/// identifiers only: the same sources hash EIP-712 type strings and storage-slot
/// labels, which are not roles.
/// @return `(0x-id, name)` pairs, deduplicated, first appearance wins.
pub fn role_names(sources: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |id: String, name: String| {
        if !out.iter().any(|(i, _)| *i == id) {
            out.push((id, name));
        }
    };
    let (Ok(hashed), Ok(zero)) = (
        Regex::new(r#"keccak256\(\s*"([A-Z][A-Z0-9_]*)"\s*\)"#),
        Regex::new(
            r"\bbytes32\s+(?:(?:public|internal|private)\s+)?constant\s+([A-Z][A-Z0-9_]*_ROLE)\s*=\s*(?:0x0+|bytes32\(\s*0\s*\))\s*;",
        ),
    ) else {
        return out;
    };
    for src in sources {
        for c in zero.captures_iter(src) {
            push(format!("0x{}", "0".repeat(64)), c[1].to_string());
        }
        for c in hashed.captures_iter(src) {
            push(crate::rpc::role_id_hex(&c[1]), c[1].to_string());
        }
    }
    out
}

/// `AccessControl.sol` at the OpenZeppelin tag the deploy repo's `foundry.toml` pins.
pub fn openzeppelin_access_control_path(foundry: &str) -> Option<String> {
    let version = crate::graph::foundry_dependencies(foundry)
        .ok()?
        .into_iter()
        .find(|d| d.package == "@openzeppelin-contracts")?
        .version_req;
    (!version.is_empty()).then(|| format!("contracts/access/AccessControl.sol?ref=v{version}"))
}

/// Where a repo's subgraph is published per network: the location part of the
/// `deploymentRbac` and `deploymentRegistry` documents.
///
/// `source` / `networks` are `None` when the repo's workflow or `networks.json`
/// could not be read; the document then says which, rather than listing no
/// networks.
pub fn build_location(
    org: &str,
    repo: &str,
    source: Option<&SubgraphSource>,
    networks: Option<&[String]>,
    goldsky_networks: Option<&[String]>,
) -> Value {
    let mut doc = json!({
        "org": org,
        "repo": repo,
        "tag": PROD_TAG,
    });
    let Some(src) = source else {
        doc["unavailable"] = Value::from(SOURCE_UNREAD);
        return doc;
    };
    doc["subgraph"] = Value::from(src.name.clone());
    let Some(nets) = networks else {
        doc["unavailable"] = Value::from("could not read subgraph/networks.json");
        return doc;
    };
    let Some(goldsky) = goldsky_networks else {
        doc["unavailable"] = Value::from(goldsky_networks_unread());
        return doc;
    };
    doc["networks"] = Value::from(
        nets.iter()
            .map(|n| {
                let mut net = json!({
                    "network": n,
                    "chain": chain_name(n),
                });
                match endpoint(src, goldsky, n) {
                    Ok(url) => net["endpoint"] = Value::from(url),
                    Err(why) => {
                        net["endpoint"] = Value::Null;
                        net["unavailable"] = Value::from(why);
                    }
                }
                net
            })
            .collect::<Vec<_>>(),
    );
    doc
}

/// Why a location has no source: the deploy workflow's inputs did not parse.
pub const SOURCE_UNREAD: &str = "could not read subgraph-name / ormi-query-base / goldsky-query-base from .github/workflows/deploy-subgraph.yaml";

/// The `deploymentRbac` document: [`build_location`] plus the role names.
pub fn build_rbac(
    org: &str,
    repo: &str,
    source: Option<&SubgraphSource>,
    networks: Option<&[String]>,
    goldsky_networks: Option<&[String]>,
    names: &[(String, String)],
) -> Value {
    let mut doc = build_location(org, repo, source, networks, goldsky_networks);
    if source.is_none() || networks.is_none() || goldsky_networks.is_none() {
        return doc;
    }
    doc["roleNames"] = Value::Object(
        names
            .iter()
            .map(|(id, name)| (id.clone(), Value::from(name.clone())))
            .collect::<Map<_, _>>(),
    );
    doc
}

/// The repo whose registry subgraph the "Root registry" tab reads.
pub const REGISTRY_ORG: &str = "rainlanguage";
pub const REGISTRY_REPO: &str = "rain.deploy";

/// The `deploymentRegistry` document: [`build_location`] for the registry repo,
/// its config registries, and the address names `rbac_doc` holds.
pub fn build_registry(
    rbac_doc: &Value,
    goldsky_networks: Option<&[String]>,
    fetch: impl Fn(&str, &str, &str) -> String,
) -> Value {
    let workflow_src = fetch(
        REGISTRY_ORG,
        REGISTRY_REPO,
        ".github/workflows/deploy-subgraph.yaml",
    );
    let networks_src = fetch(REGISTRY_ORG, REGISTRY_REPO, "subgraph/networks.json");
    let mut doc = build_location(
        REGISTRY_ORG,
        REGISTRY_REPO,
        parse_deploy_workflow(&workflow_src).as_ref(),
        parse_networks(&networks_src).as_deref(),
        goldsky_networks,
    );
    doc["configRegistries"] = Value::Object(config_registries(&networks_src));
    if let Some(names) = rbac_doc.get("addressNames") {
        doc["addressNames"] = names.clone();
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r#"
name: Deploy subgraph
jobs:
  deploy:
    uses: rainlanguage/rainix/.github/workflows/rainix-subgraph-deploy.yaml@main
    with:
      subgraph-name: st0x-rbac
      # Public query base for this Ormi account.
      ormi-query-base: https://subgraph.api.ormilabs.com/api/public/9f4f/
    secrets:
      ORMI_DEPLOY_KEY: ${{ secrets.ORMI_DEPLOY_KEY }}
"#;

    fn src() -> SubgraphSource {
        parse_deploy_workflow(WORKFLOW).unwrap()
    }

    /// No network on Goldsky: every network routes to Ormi.
    const ALL_ORMI: &[String] = &[];

    /// st0x.deploy's `.github/workflows/deploy-subgraph.yaml` at
    /// S01-Issuer/st0x.deploy#638 head 451a9a3f.
    const BOTH_HOSTS_WORKFLOW: &str = r#"
name: Deploy subgraph
on:
  workflow_dispatch:
jobs:
  deploy:
    permissions:
      contents: read
    uses: rainlanguage/rainix/.github/workflows/rainix-subgraph-deploy.yaml@main
    with:
      subgraph-name: st0x-rbac
      ormi-query-base: https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71
      goldsky-query-base: https://api.goldsky.com/api/public/project_cmjr2df7svg6t01tl2ic706ao
    secrets:
      CACHIX_AUTH_TOKEN: ${{ secrets.CACHIX_AUTH_TOKEN }}
      # GitHub secret ORMI_DEPLOY_KEY, from Vault secret/infra/rain/ormi (deploy_key).
      ORMI_DEPLOY_KEY: ${{ secrets.ORMI_DEPLOY_KEY }}
      CI_GOLDSKY_TOKEN: ${{ secrets.CI_GOLDSKY_TOKEN }}
"#;
    const ORMI_BASE: &str =
        "https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71";
    const GOLDSKY_BASE: &str =
        "https://api.goldsky.com/api/public/project_cmjr2df7svg6t01tl2ic706ao";

    /// rainix `rainix-static/src/subgraph_targets.rs` at 2d16db31.
    const RAINIX_TARGETS: &str = r#"
//! [`GOLDSKY_NETWORKS`] (Ormi does not index it), Ormi for every other.

/// Graph network names Ormi does not index, so their subgraphs deploy to
/// Goldsky. Every other network deploys to Ormi.
pub(crate) const GOLDSKY_NETWORKS: &[&str] = &["0g"];

/// The host a network's subgraph deploys to.
pub(crate) fn host(network: &str) -> &'static str {
    if GOLDSKY_NETWORKS.contains(&network) {
        "goldsky"
    } else {
        "ormi"
    }
}
"#;

    fn goldsky() -> Vec<String> {
        parse_goldsky_networks(RAINIX_TARGETS).unwrap()
    }

    fn reusable_caller(inputs: &[&str]) -> String {
        let mut y = "jobs:\n  deploy:\n    uses: rainlanguage/rainix/.github/workflows/rainix-subgraph-deploy.yaml@main\n    with:\n".to_string();
        for i in inputs {
            y.push_str(&format!("      {i}\n"));
        }
        y
    }

    #[test]
    fn workflow_names_the_subgraph_and_its_query_base() {
        assert_eq!(
            src(),
            SubgraphSource {
                name: "st0x-rbac".into(),
                ormi_query_base: Some("https://subgraph.api.ormilabs.com/api/public/9f4f".into()),
                goldsky_query_base: None,
            }
        );
    }

    #[test]
    fn workflow_names_both_query_bases_each_from_its_own_input() {
        assert_eq!(
            parse_deploy_workflow(BOTH_HOSTS_WORKFLOW),
            Some(SubgraphSource {
                name: "st0x-rbac".into(),
                ormi_query_base: Some(ORMI_BASE.into()),
                goldsky_query_base: Some(GOLDSKY_BASE.into()),
            })
        );
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&[
                "subgraph-name: x",
                "goldsky-query-base: https://g/",
            ])),
            Some(SubgraphSource {
                name: "x".into(),
                ormi_query_base: None,
                goldsky_query_base: Some("https://g".into()),
            })
        );
    }

    #[test]
    fn workflow_without_a_name_or_any_query_base_is_unread() {
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&["subgraph-name: x"])),
            None
        );
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&["ormi-query-base: https://a"])),
            None
        );
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&[
                "ormi-query-base: https://a",
                "goldsky-query-base: https://g",
            ])),
            None
        );
    }

    #[test]
    fn workflow_with_a_plain_http_base_is_refused() {
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&[
                "subgraph-name: x",
                "ormi-query-base: http://a",
            ])),
            None
        );
        for (ormi, goldsky) in [
            ("https://a", "http://g"),
            ("http://a", "https://g"),
            ("http://a", "http://g"),
        ] {
            let ormi_input = format!("ormi-query-base: {ormi}");
            let goldsky_input = format!("goldsky-query-base: {goldsky}");
            assert_eq!(
                parse_deploy_workflow(&reusable_caller(&[
                    "subgraph-name: x",
                    ormi_input.as_str(),
                    goldsky_input.as_str(),
                ])),
                None,
                "{ormi} / {goldsky}"
            );
        }
        assert_eq!(
            parse_deploy_workflow(&reusable_caller(&[
                "subgraph-name: x",
                "goldsky-query-base: http://g",
            ])),
            None
        );
    }

    #[test]
    fn workflow_values_may_be_quoted() {
        let s = parse_deploy_workflow(&reusable_caller(&[
            "subgraph-name: \"x\"",
            "ormi-query-base: 'https://a'",
            "goldsky-query-base: \"https://g/\"",
        ]))
        .unwrap();
        assert_eq!(
            (
                s.name.as_str(),
                s.ormi_query_base.as_deref(),
                s.goldsky_query_base.as_deref()
            ),
            ("x", Some("https://a"), Some("https://g"))
        );
    }

    #[test]
    fn goldsky_networks_are_read_from_rainixs_constant() {
        assert_eq!(goldsky(), ["0g"]);
        assert_eq!(
            parse_goldsky_networks(
                "pub(crate) const GOLDSKY_NETWORKS: &[&str] = &[\n    \"0g\",\n    \"x-y\",\n];"
            ),
            Some(vec!["0g".to_string(), "x-y".to_string()])
        );
        assert_eq!(
            parse_goldsky_networks("const GOLDSKY_NETWORKS:&[&str]=&[];"),
            Some(vec![])
        );
    }

    #[test]
    fn goldsky_networks_are_unread_without_the_constant() {
        for src in [
            "",
            "not found",
            "//! [`GOLDSKY_NETWORKS`] (Ormi does not index it)",
            "if GOLDSKY_NETWORKS.contains(&network) {}",
            "const ORMI_NETWORKS: &[&str] = &[\"0g\"];",
        ] {
            assert_eq!(parse_goldsky_networks(src), None, "{src:?}");
        }
        assert_eq!(
            goldsky_networks_unread(),
            "could not read GOLDSKY_NETWORKS from rainlanguage/rainix rainix-static/src/subgraph_targets.rs"
        );
    }

    #[test]
    fn workflow_passing_the_values_as_env_not_reusable_inputs_is_unread() {
        let env_form = r#"
name: Deploy subgraph
on:
  workflow_dispatch:
jobs:
  deploy:
    runs-on: ubuntu-latest
    env:
      SUBGRAPH_NAME: rain-deploy-registry
      ORMI_QUERY_BASE: https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71
    steps:
      - run: nix develop github:rainlanguage/rainix#subgraph-shell -c subgraph-deploy
"#;
        assert_eq!(parse_deploy_workflow(env_form), None);
        assert_eq!(
            build_location(
                "o",
                "r",
                parse_deploy_workflow(env_form).as_ref(),
                Some(&[]),
                Some(ALL_ORMI)
            )["unavailable"],
            "could not read subgraph-name / ormi-query-base / goldsky-query-base from .github/workflows/deploy-subgraph.yaml"
        );
    }

    #[test]
    fn endpoint_addresses_the_prod_tag_not_a_version() {
        assert_eq!(
            endpoint(&src(), ALL_ORMI, "hyperevm"),
            Ok("https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-hyperevm/prod/gn".into())
        );
    }

    #[test]
    fn endpoint_routes_a_goldsky_network_to_the_goldsky_base_and_every_other_to_ormi() {
        let s = parse_deploy_workflow(BOTH_HOSTS_WORKFLOW).unwrap();
        assert_eq!(
            endpoint(&s, &goldsky(), "0g"),
            Ok(format!("{GOLDSKY_BASE}/subgraphs/st0x-rbac-0g/prod/gn"))
        );
        for n in ["base", "mainnet", "hyper-evm", "bsc", "robinhood"] {
            assert_eq!(
                endpoint(&s, &goldsky(), n),
                Ok(format!("{ORMI_BASE}/subgraphs/st0x-rbac-{n}/prod/gn")),
                "{n}"
            );
        }
        assert_eq!(
            endpoint(&s, ALL_ORMI, "0g"),
            Ok(format!("{ORMI_BASE}/subgraphs/st0x-rbac-0g/prod/gn"))
        );
        let all_goldsky = ["base".to_string()];
        assert_eq!(
            endpoint(&s, &all_goldsky, "base"),
            Ok(format!("{GOLDSKY_BASE}/subgraphs/st0x-rbac-base/prod/gn"))
        );
    }

    #[test]
    fn endpoint_names_the_query_base_the_workflow_does_not_give() {
        assert_eq!(
            endpoint(&src(), &goldsky(), "0g"),
            Err("deploy-subgraph.yaml has no goldsky-query-base for 0g".into())
        );
        let goldsky_only = SubgraphSource {
            name: "st0x-rbac".into(),
            ormi_query_base: None,
            goldsky_query_base: Some(GOLDSKY_BASE.into()),
        };
        assert_eq!(
            endpoint(&goldsky_only, &goldsky(), "base"),
            Err("deploy-subgraph.yaml has no ormi-query-base for base".into())
        );
        assert_eq!(
            endpoint(&goldsky_only, &goldsky(), "0g"),
            Ok(format!("{GOLDSKY_BASE}/subgraphs/st0x-rbac-0g/prod/gn"))
        );
    }

    #[test]
    fn networks_keep_file_order() {
        assert_eq!(
            parse_networks(r#"{"base":{},"mainnet":{},"hyper-evm":{},"bsc":{}}"#),
            Some(vec![
                "base".into(),
                "mainnet".into(),
                "hyper-evm".into(),
                "bsc".into()
            ])
        );
        assert_eq!(parse_networks("[1]"), None);
        assert_eq!(parse_networks("not json"), None);
    }

    #[test]
    fn config_registries_are_each_networks_v5_config_registry_lowercased() {
        let src = r#"{
            "base": {
                "ConfigRegistry": {"address": "0x2222222222222222222222222222222222222222", "startBlock": 52910632},
                "ConfigRegistryV5": {"address": "0x82b7e6FDfD25EF64b013F98e8F2c1fC1D1872eC9", "startBlock": 53210195},
                "MigrationRegistryV5": {"address": "0xd708186f68D527B64c001398550627508205EE31", "startBlock": 53210703}
            },
            "mainnet": {"ConfigRegistry": {"address": "0x2222222222222222222222222222222222222222"}},
            "bsc": {"ConfigRegistryV5": {"startBlock": 1}},
            "hyperevm": {"ConfigRegistryV5": {"address": 7}},
            "robinhood": [1]
        }"#;
        let got = config_registries(src);
        assert_eq!(
            got,
            serde_json::from_str::<Map<String, Value>>(
                r#"{"base": "0x82b7e6fdfd25ef64b013f98e8f2c1fc1d1872ec9"}"#
            )
            .unwrap()
        );
        for bad in ["", "not json", "[1]", "null"] {
            assert!(config_registries(bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn chain_names_translate_graph_cli_names() {
        assert_eq!(chain_name("mainnet"), Some("ethereum"));
        assert_eq!(chain_name("hyper-evm"), Some("hyperevm"));
        assert_eq!(chain_name("hyperevm"), Some("hyperevm"));
        assert_eq!(chain_name("base"), Some("base"));
        assert_eq!(chain_name("robinhood"), Some("robinhood"));
        assert_eq!(chain_name("bsc"), Some("bsc"));
        assert_eq!(chain_name("0g"), Some("zerog"));
        assert_eq!(chain_name("zerog"), None);
        assert_eq!(chain_name("arbitrum-one"), None);
    }

    #[test]
    fn role_names_come_from_upper_case_hash_preimages_and_the_zero_pin() {
        let names = role_names(&[
            r#"bytes32 public constant MINT_ROLE = keccak256("MINT");
               bytes32 constant T = keccak256("MintAuth(address token)");
               bytes32 constant L = keccak256("st0x.orchestrator.main");"#,
            r#"bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);
               RoleGrant(keccak256("MINT"), x);
               RoleGrant(keccak256( "DEPOSIT_ADMIN" ), x);"#,
        ]);
        assert_eq!(
            names,
            vec![
                (crate::rpc::role_id_hex("MINT"), "MINT".to_string()),
                (
                    format!("0x{}", "0".repeat(64)),
                    "DEFAULT_ADMIN_ROLE".to_string()
                ),
                (
                    crate::rpc::role_id_hex("DEPOSIT_ADMIN"),
                    "DEPOSIT_ADMIN".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_zero_role_is_named_by_its_open_zeppelin_constant() {
        assert_eq!(
            role_names(&["bytes32 public constant DEFAULT_ADMIN_ROLE = 0x00;"]),
            vec![(
                format!("0x{}", "0".repeat(64)),
                "DEFAULT_ADMIN_ROLE".to_string()
            )]
        );
    }

    #[test]
    fn a_zero_role_is_unnamed_unless_a_source_pins_it() {
        assert!(role_names(&["bytes32 constant X = bytes32(0);"]).is_empty());
    }

    #[test]
    fn rbac_doc_says_which_source_was_unread() {
        let d = build_rbac("o", "r", None, None, Some(ALL_ORMI), &[]);
        assert!(d["unavailable"]
            .as_str()
            .unwrap()
            .contains("deploy-subgraph.yaml"));
        assert!(d.get("networks").is_none());
        let s = src();
        let d = build_rbac("o", "r", Some(&s), None, Some(ALL_ORMI), &[]);
        assert!(d["unavailable"].as_str().unwrap().contains("networks.json"));
        assert_eq!(d["subgraph"], "st0x-rbac");
        assert!(d.get("networks").is_none());
        let nets = vec!["base".to_string()];
        let d = build_rbac(
            "o",
            "r",
            Some(&s),
            Some(&nets),
            None,
            &[("0xaa".to_string(), "MINT".to_string())],
        );
        assert_eq!(d["unavailable"], goldsky_networks_unread());
        assert_eq!(d["subgraph"], "st0x-rbac");
        assert!(d.get("networks").is_none());
        assert!(d.get("roleNames").is_none());
    }

    #[test]
    fn rbac_doc_lists_each_network_with_its_chain_endpoint_and_the_names() {
        let nets = vec!["base".to_string(), "arbitrum-one".to_string()];
        let names = vec![
            ("0xaa".to_string(), "MINT".to_string()),
            ("0xbb".to_string(), "BURN".to_string()),
        ];
        let d = build_rbac("o", "r", Some(&src()), Some(&nets), Some(ALL_ORMI), &names);
        assert!(d.get("unavailable").is_none());
        assert_eq!(d["tag"], "prod");
        assert_eq!(
            d["networks"],
            json!([
                {
                    "network": "base",
                    "chain": "base",
                    "endpoint": "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-base/prod/gn",
                },
                {
                    "network": "arbitrum-one",
                    "chain": null,
                    "endpoint": "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-arbitrum-one/prod/gn",
                },
            ])
        );
        assert_eq!(d["roleNames"], json!({"0xaa": "MINT", "0xbb": "BURN"}));
    }

    #[test]
    fn location_doc_says_which_source_was_unread_and_lists_no_networks() {
        let d = build_location("o", "r", None, None, Some(ALL_ORMI));
        assert_eq!(
            d,
            json!({
                "org": "o",
                "repo": "r",
                "tag": "prod",
                "unavailable": "could not read subgraph-name / ormi-query-base / goldsky-query-base from .github/workflows/deploy-subgraph.yaml",
            })
        );
        let nets = vec!["base".to_string()];
        assert_eq!(
            build_location("o", "r", None, Some(&nets), Some(ALL_ORMI)),
            d,
            "networks without a source are no location"
        );
        assert_eq!(
            build_location("o", "r", None, Some(&nets), None),
            d,
            "an unread source is named before an unread host split"
        );
        assert_eq!(
            build_location("o", "r", Some(&src()), None, Some(ALL_ORMI)),
            json!({
                "org": "o",
                "repo": "r",
                "tag": "prod",
                "subgraph": "st0x-rbac",
                "unavailable": "could not read subgraph/networks.json",
            })
        );
        assert_eq!(
            build_location("o", "r", Some(&src()), None, None)["unavailable"],
            "could not read subgraph/networks.json"
        );
        assert_eq!(
            build_location("o", "r", Some(&src()), Some(&nets), None),
            json!({
                "org": "o",
                "repo": "r",
                "tag": "prod",
                "subgraph": "st0x-rbac",
                "unavailable": "could not read GOLDSKY_NETWORKS from rainlanguage/rainix rainix-static/src/subgraph_targets.rs",
            })
        );
    }

    #[test]
    fn location_doc_routes_each_network_to_its_host() {
        let s = parse_deploy_workflow(BOTH_HOSTS_WORKFLOW).unwrap();
        let nets = vec!["base".to_string(), "0g".to_string()];
        assert_eq!(
            build_location("o", "r", Some(&s), Some(&nets), Some(&goldsky()))["networks"],
            json!([
                {
                    "network": "base",
                    "chain": "base",
                    "endpoint": format!("{ORMI_BASE}/subgraphs/st0x-rbac-base/prod/gn"),
                },
                {
                    "network": "0g",
                    "chain": "zerog",
                    "endpoint": format!("{GOLDSKY_BASE}/subgraphs/st0x-rbac-0g/prod/gn"),
                },
            ])
        );
    }

    #[test]
    fn location_doc_lists_a_network_whose_host_has_no_query_base_as_unavailable() {
        let nets = vec!["base".to_string(), "0g".to_string()];
        let d = build_location("o", "r", Some(&src()), Some(&nets), Some(&goldsky()));
        assert!(d.get("unavailable").is_none());
        assert_eq!(
            d["networks"],
            json!([
                {
                    "network": "base",
                    "chain": "base",
                    "endpoint": "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-base/prod/gn",
                },
                {
                    "network": "0g",
                    "chain": "zerog",
                    "endpoint": null,
                    "unavailable": "deploy-subgraph.yaml has no goldsky-query-base for 0g",
                },
            ])
        );
    }

    #[test]
    fn location_doc_lists_each_network_with_its_chain_and_tagged_endpoint_and_nothing_else() {
        let nets = vec!["mainnet".to_string(), "arbitrum-one".to_string()];
        assert_eq!(
            build_location("o", "r", Some(&src()), Some(&nets), Some(ALL_ORMI)),
            json!({
                "org": "o",
                "repo": "r",
                "tag": "prod",
                "subgraph": "st0x-rbac",
                "networks": [
                    {
                        "network": "mainnet",
                        "chain": "ethereum",
                        "endpoint": "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-mainnet/prod/gn",
                    },
                    {
                        "network": "arbitrum-one",
                        "chain": null,
                        "endpoint": "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-arbitrum-one/prod/gn",
                    },
                ],
            })
        );
        assert_eq!(
            build_location("o", "r", Some(&src()), Some(&[]), Some(ALL_ORMI))["networks"],
            json!([])
        );
    }

    #[test]
    fn rbac_doc_is_its_location_plus_role_names_only_when_located() {
        let names = vec![("0xaa".to_string(), "MINT".to_string())];
        let nets = vec!["base".to_string()];
        let s = src();
        let hosts = goldsky();
        for (source, networks, goldsky_networks) in [
            (None, None, Some(&hosts)),
            (None, Some(&nets), Some(&hosts)),
            (Some(&s), None, Some(&hosts)),
            (Some(&s), Some(&nets), None),
            (None, None, None),
        ] {
            let networks = networks.map(|n| n.as_slice());
            let goldsky_networks = goldsky_networks.map(|n| n.as_slice());
            assert_eq!(
                build_rbac("o", "r", source, networks, goldsky_networks, &names),
                build_location("o", "r", source, networks, goldsky_networks),
                "an unlocated doc carries no role names"
            );
        }
        let mut want = build_location("o", "r", Some(&s), Some(&nets), Some(&hosts));
        want["roleNames"] = json!({"0xaa": "MINT"});
        assert_eq!(
            build_rbac("o", "r", Some(&s), Some(&nets), Some(&hosts), &names),
            want
        );
        assert_eq!(
            build_rbac("o", "r", Some(&s), Some(&nets), Some(&hosts), &[])["roleNames"],
            json!({}),
            "a located doc with no names says so with an empty map"
        );
    }

    /// rain.deploy's `.github/workflows/deploy-subgraph.yaml` and
    /// `subgraph/networks.json` on main.
    const REGISTRY_WORKFLOW: &str = r#"
name: Deploy subgraph
on:
  workflow_dispatch:
jobs:
  deploy:
    permissions:
      contents: read
    uses: rainlanguage/rainix/.github/workflows/rainix-subgraph-deploy.yaml@main
    with:
      subgraph-name: rain-deploy-registry
      ormi-query-base: https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71
    secrets:
      CACHIX_AUTH_TOKEN: ${{ secrets.CACHIX_AUTH_TOKEN }}
      ORMI_DEPLOY_KEY: ${{ secrets.ORMI_DEPLOY_KEY }}
"#;
    const REGISTRY_NETWORKS: [(&str, u64, u64); 9] = [
        ("arbitrum-one", 519115682, 519119422),
        ("base", 53210195, 53210703),
        ("base-sepolia", 51439343, 51444423),
        ("bsc", 130324009, 130326266),
        ("flare", 73190792, 73191741),
        ("mainnet", 26292829, 26292913),
        ("hyperevm", 49768958, 49769995),
        ("matic", 96336285, 96336965),
        ("robinhood", 100259389, 100268990),
    ];
    const REGISTRY_CONFIG_V5: &str = "0x82b7e6FDfD25EF64b013F98e8F2c1fC1D1872eC9";

    fn registry_networks_json() -> String {
        let mut doc = Map::new();
        for (n, config_block, migration_block) in REGISTRY_NETWORKS {
            doc.insert(
                n.to_string(),
                json!({
                    "ConfigRegistryV5": {"address": REGISTRY_CONFIG_V5, "startBlock": config_block},
                    "MigrationRegistryV5": {"address": "0xd708186f68D527B64c001398550627508205EE31", "startBlock": migration_block},
                }),
            );
        }
        Value::Object(doc).to_string()
    }

    #[test]
    fn registry_location_reads_rain_deploys_workflow_and_all_nine_networks() {
        let source = parse_deploy_workflow(REGISTRY_WORKFLOW).unwrap();
        let networks_src = registry_networks_json();
        let networks = parse_networks(&networks_src).unwrap();
        let d = build_location(
            "rainlanguage",
            "rain.deploy",
            Some(&source),
            Some(&networks),
            Some(&goldsky()),
        );
        let base =
            "https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71";
        let chains = [
            None,
            Some("base"),
            None,
            Some("bsc"),
            None,
            Some("ethereum"),
            Some("hyperevm"),
            None,
            Some("robinhood"),
        ];
        let want: Vec<Value> = REGISTRY_NETWORKS
            .iter()
            .zip(chains)
            .map(|((n, _, _), chain)| {
                json!({
                    "network": n,
                    "chain": chain,
                    "endpoint": format!("{base}/subgraphs/rain-deploy-registry-{n}/prod/gn"),
                })
            })
            .collect();
        assert_eq!(
            d,
            json!({
                "org": "rainlanguage",
                "repo": "rain.deploy",
                "tag": "prod",
                "subgraph": "rain-deploy-registry",
                "networks": want,
            })
        );
        let registries = config_registries(&networks_src);
        assert_eq!(
            registries.keys().map(String::as_str).collect::<Vec<_>>(),
            REGISTRY_NETWORKS
                .iter()
                .map(|(n, _, _)| *n)
                .collect::<Vec<_>>()
        );
        for (n, a) in &registries {
            assert_eq!(a, &Value::from(REGISTRY_CONFIG_V5.to_lowercase()), "{n}");
        }
    }

    fn rbac_doc_with_address_names() -> Value {
        let mut d = build_rbac(
            "o",
            "r",
            Some(&src()),
            Some(&["base".to_string()]),
            Some(ALL_ORMI),
            &[("0xaa".to_string(), "MINT".to_string())],
        );
        d["configRegistries"] = json!({"base": "0x2222222222222222222222222222222222222222"});
        d["addressNames"] = json!({
            "base": {
                "0x5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a": [
                    {"name": "TOKEN_OWNER_SAFE", "contract": "LibSafeInvariants"}
                ]
            },
            "ethereum": {},
        });
        d["addressNamesUnread"] = json!(["src/lib/LibUnread.sol"]);
        d
    }

    #[test]
    fn registry_doc_is_rain_deploys_location_its_config_registries_and_the_rbac_docs_address_names()
    {
        let networks_src = registry_networks_json();
        let rbac_doc = rbac_doc_with_address_names();
        let mut want = build_location(
            "rainlanguage",
            "rain.deploy",
            parse_deploy_workflow(REGISTRY_WORKFLOW).as_ref(),
            parse_networks(&networks_src).as_deref(),
            Some(&goldsky()),
        );
        want["configRegistries"] = Value::Object(
            REGISTRY_NETWORKS
                .iter()
                .map(|(n, _, _)| {
                    (
                        n.to_string(),
                        Value::from(REGISTRY_CONFIG_V5.to_lowercase()),
                    )
                })
                .collect(),
        );
        want["addressNames"] = rbac_doc["addressNames"].clone();
        let d = build_registry(
            &rbac_doc,
            Some(&goldsky()),
            |_: &str, _: &str, path: &str| match path {
                ".github/workflows/deploy-subgraph.yaml" => REGISTRY_WORKFLOW.to_string(),
                "subgraph/networks.json" => networks_src.clone(),
                _ => String::new(),
            },
        );
        assert_eq!(d, want);
        assert_eq!(d["subgraph"], "rain-deploy-registry");
        assert_eq!(d["networks"].as_array().map(Vec::len), Some(9));
    }

    #[test]
    fn registry_doc_fetches_rain_deploys_workflow_and_networks_each_for_its_part() {
        let workflow = reusable_caller(&[
            "subgraph-name: from-the-workflow",
            "ormi-query-base: https://q.example/api",
        ]);
        let networks = json!({
            "base": {"ConfigRegistryV5": {"address": REGISTRY_CONFIG_V5}},
            "flare": {"MigrationRegistryV5": {"address": "0xd708186f68D527B64c001398550627508205EE31"}},
        })
        .to_string();
        let asked = std::cell::RefCell::new(Vec::new());
        let rbac_doc = rbac_doc_with_address_names();
        let d = build_registry(
            &rbac_doc,
            Some(ALL_ORMI),
            |org: &str, repo: &str, path: &str| {
                asked
                    .borrow_mut()
                    .push((org.to_string(), repo.to_string(), path.to_string()));
                match (org, repo, path) {
                    ("rainlanguage", "rain.deploy", ".github/workflows/deploy-subgraph.yaml") => {
                        workflow.to_string()
                    }
                    ("rainlanguage", "rain.deploy", "subgraph/networks.json") => networks.clone(),
                    _ => String::new(),
                }
            },
        );
        let mut asked = asked.into_inner();
        asked.sort();
        assert_eq!(
            asked,
            [
                ".github/workflows/deploy-subgraph.yaml",
                "subgraph/networks.json"
            ]
            .map(|p| (
                "rainlanguage".to_string(),
                "rain.deploy".to_string(),
                p.to_string()
            ))
        );
        assert_eq!(
            d,
            json!({
                "org": "rainlanguage",
                "repo": "rain.deploy",
                "tag": "prod",
                "subgraph": "from-the-workflow",
                "networks": [
                    {
                        "network": "base",
                        "chain": "base",
                        "endpoint": "https://q.example/api/subgraphs/from-the-workflow-base/prod/gn",
                    },
                    {
                        "network": "flare",
                        "chain": null,
                        "endpoint": "https://q.example/api/subgraphs/from-the-workflow-flare/prod/gn",
                    },
                ],
                "configRegistries": {"base": REGISTRY_CONFIG_V5.to_lowercase()},
                "addressNames": rbac_doc["addressNames"],
            })
        );
    }

    #[test]
    fn registry_doc_lists_a_network_without_a_v5_config_registry_and_declares_none_for_it() {
        let networks_src = json!({
            "base": {"ConfigRegistryV5": {"address": REGISTRY_CONFIG_V5}},
            "flare": {"MigrationRegistryV5": {"address": "0xd708186f68D527B64c001398550627508205EE31"}},
        })
        .to_string();
        let d = build_registry(
            &json!({}),
            Some(ALL_ORMI),
            |_: &str, _: &str, path: &str| match path {
                ".github/workflows/deploy-subgraph.yaml" => REGISTRY_WORKFLOW.to_string(),
                "subgraph/networks.json" => networks_src.clone(),
                _ => String::new(),
            },
        );
        assert_eq!(
            d["networks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["network"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["base", "flare"]
        );
        assert_eq!(
            d["configRegistries"],
            json!({"base": REGISTRY_CONFIG_V5.to_lowercase()})
        );
    }

    #[test]
    fn registry_doc_carries_no_address_names_when_the_rbac_doc_has_none() {
        let networks_src = registry_networks_json();
        let mut located = rbac_doc_with_address_names();
        located.as_object_mut().unwrap().remove("addressNames");
        for rbac_doc in [
            json!({}),
            Value::Null,
            build_rbac("o", "r", None, None, Some(ALL_ORMI), &[]),
            located,
        ] {
            let d =
                build_registry(
                    &rbac_doc,
                    Some(ALL_ORMI),
                    |_: &str, _: &str, path: &str| match path {
                        ".github/workflows/deploy-subgraph.yaml" => REGISTRY_WORKFLOW.to_string(),
                        "subgraph/networks.json" => networks_src.clone(),
                        _ => String::new(),
                    },
                );
            assert!(d.get("addressNames").is_none(), "{rbac_doc}");
            assert_eq!(d["networks"].as_array().map(Vec::len), Some(9));
        }
    }

    #[test]
    fn registry_doc_says_which_rain_deploy_source_was_unread() {
        let networks_src = registry_networks_json();
        let workflow_unread = "could not read subgraph-name / ormi-query-base / goldsky-query-base from .github/workflows/deploy-subgraph.yaml";
        let networks_unread = "could not read subgraph/networks.json";
        let plain_http = REGISTRY_WORKFLOW.replace("https://", "http://");
        let name_only = reusable_caller(&["subgraph-name: rain-deploy-registry"]);
        for (workflow, networks, why, subgraph) in [
            ("", networks_src.as_str(), workflow_unread, None),
            (
                name_only.as_str(),
                networks_src.as_str(),
                workflow_unread,
                None,
            ),
            (
                plain_http.as_str(),
                networks_src.as_str(),
                workflow_unread,
                None,
            ),
            ("", "", workflow_unread, None),
            (
                REGISTRY_WORKFLOW,
                "",
                networks_unread,
                Some("rain-deploy-registry"),
            ),
            (
                REGISTRY_WORKFLOW,
                "[1]",
                networks_unread,
                Some("rain-deploy-registry"),
            ),
            (
                REGISTRY_WORKFLOW,
                "not json",
                networks_unread,
                Some("rain-deploy-registry"),
            ),
        ] {
            let d = build_registry(
                &rbac_doc_with_address_names(),
                Some(ALL_ORMI),
                |_: &str, _: &str, path: &str| match path {
                    ".github/workflows/deploy-subgraph.yaml" => workflow.to_string(),
                    "subgraph/networks.json" => networks.to_string(),
                    _ => String::new(),
                },
            );
            let case = format!("{workflow:?} / {networks:?}");
            assert_eq!(d["org"], "rainlanguage", "{case}");
            assert_eq!(d["repo"], "rain.deploy", "{case}");
            assert_eq!(d["tag"], "prod", "{case}");
            assert_eq!(d["unavailable"], why, "{case}");
            assert_eq!(
                d.get("subgraph").and_then(Value::as_str),
                subgraph,
                "{case}"
            );
            assert!(d.get("networks").is_none(), "{case}");
        }
    }

    #[test]
    fn registry_doc_routes_its_networks_by_the_given_host_split() {
        let networks_src = json!({
            "base": {"ConfigRegistryV5": {"address": REGISTRY_CONFIG_V5}},
            "0g": {"ConfigRegistryV5": {"address": REGISTRY_CONFIG_V5}},
        })
        .to_string();
        let fetch = |_: &str, _: &str, path: &str| match path {
            ".github/workflows/deploy-subgraph.yaml" => REGISTRY_WORKFLOW.to_string(),
            "subgraph/networks.json" => networks_src.clone(),
            _ => String::new(),
        };
        let base =
            "https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71";
        let d = build_registry(&json!({}), Some(&goldsky()), fetch);
        assert_eq!(
            d["networks"],
            json!([
                {
                    "network": "base",
                    "chain": "base",
                    "endpoint": format!("{base}/subgraphs/rain-deploy-registry-base/prod/gn"),
                },
                {
                    "network": "0g",
                    "chain": "zerog",
                    "endpoint": null,
                    "unavailable": "deploy-subgraph.yaml has no goldsky-query-base for 0g",
                },
            ])
        );
        let d = build_registry(&json!({}), Some(ALL_ORMI), fetch);
        assert_eq!(
            d["networks"][1]["endpoint"],
            format!("{base}/subgraphs/rain-deploy-registry-0g/prod/gn")
        );
        let d = build_registry(&json!({}), None, fetch);
        assert_eq!(d["unavailable"], goldsky_networks_unread());
        assert_eq!(d["subgraph"], "rain-deploy-registry");
        assert!(d.get("networks").is_none());
    }

    #[test]
    fn openzeppelin_path_follows_the_foundry_pin() {
        let foundry = r#"
[profile.default]
src = "src"

[dependencies]
forge-std = "1.9.7"
"@openzeppelin-contracts" = "5.7.0"
"#;
        assert_eq!(
            openzeppelin_access_control_path(foundry).as_deref(),
            Some("contracts/access/AccessControl.sol?ref=v5.7.0")
        );
        assert_eq!(
            openzeppelin_access_control_path(&foundry.replace("5.7.0", "5.8.1")).as_deref(),
            Some("contracts/access/AccessControl.sol?ref=v5.8.1")
        );
    }

    #[test]
    fn openzeppelin_path_is_none_without_a_readable_pin() {
        for foundry in [
            "",
            "not toml {{{",
            "[dependencies]\nforge-std = \"1.9.7\"\n",
            "[dependencies]\n\"@openzeppelin-contracts\" = { git = \"https://x\" }\n",
        ] {
            assert_eq!(
                openzeppelin_access_control_path(foundry),
                None,
                "{foundry:?}"
            );
        }
    }
}
