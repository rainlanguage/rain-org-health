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
//! Everything that locates the subgraph is read from the deploy repo, not typed
//! here: the networks are `subgraph/networks.json`'s keys, and the deployment
//! name and query base are the `SUBGRAPH_NAME` / `ORMI_QUERY_BASE` its deploy
//! workflow passes to rainix's `subgraph-deploy`. That task publishes each
//! network as `<SUBGRAPH_NAME>-<network>` at version label
//! `<address>-<commit>`, which changes on every redeploy, and keeps the name
//! shape so a tag can move between versions. Consumers address the tag —
//! raindex's own settings read `…/raindex-base/prod/gn` — so this does too.

use regex::Regex;
use serde_json::{json, Map, Value};

/// The Ormi tag consumers read. rainix's `subgraph-deploy` keeps the
/// `<name>/<version>` shape "so an Ormi tag can move between versions", and
/// `prod` is the tag the org's consumers address (raindex's settings).
pub const PROD_TAG: &str = "prod";

/// Where the subgraph is published, as the deploy workflow names it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SubgraphSource {
    /// `SUBGRAPH_NAME` — each network is published as `<name>-<network>`.
    pub name: String,
    /// `ORMI_QUERY_BASE` — the public query root for the Ormi account.
    pub query_base: String,
}

/// Read `SUBGRAPH_NAME` and `ORMI_QUERY_BASE` out of the deploy workflow.
/// `None` unless both are present: half a location is not a location.
pub fn parse_deploy_workflow(yaml: &str) -> Option<SubgraphSource> {
    let value = |key: &str| -> Option<String> {
        let re = Regex::new(&format!(r#"(?m)^\s*{key}:\s*["']?([^"'\s#]+)"#)).ok()?;
        re.captures(yaml)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    let name = value("SUBGRAPH_NAME")?;
    let query_base = value("ORMI_QUERY_BASE")?.trim_end_matches('/').to_string();
    // The page POSTs to this base and renders the answer as the chain's role
    // history. Anything but https is refused for the reason rainix's
    // `ormi-probe` refuses it: a response rewritten in transit would be shown
    // as that history.
    if !query_base.starts_with("https://") {
        return None;
    }
    Some(SubgraphSource { name, query_base })
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
        _ => None,
    }
}

/// The tagged query URL for one network.
pub fn endpoint(src: &SubgraphSource, network: &str) -> String {
    format!(
        "{}/subgraphs/{}-{}/{}/gn",
        src.query_base, src.name, network, PROD_TAG
    )
}

/// Every role name the deploy repo spells out, keyed by its on-chain id.
///
/// The subgraph stores role ids, never names — the preimage is not in the log.
/// So a name is shown only where a source this scan reads writes
/// `keccak256("NAME")` for it, plus `DEFAULT_ADMIN_ROLE` where a source pins it
/// as `bytes32(0)`. Upper-case identifiers only: the same sources hash EIP-712
/// type strings and storage-slot labels, which are not roles.
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
        Regex::new(r"\bDEFAULT_ADMIN_ROLE\s*=\s*bytes32\(\s*0\s*\)"),
    ) else {
        return out;
    };
    for src in sources {
        if zero.is_match(src) {
            push(
                format!("0x{}", "0".repeat(64)),
                "DEFAULT_ADMIN_ROLE".to_string(),
            );
        }
        for c in hashed.captures_iter(src) {
            push(crate::rpc::role_id_hex(&c[1]), c[1].to_string());
        }
    }
    out
}

/// Every registry label the deploy repo spells out, keyed by its hash.
///
/// The registries emit `bytes32` names and namespaces, never their preimages.
/// A label is shown where a source writes `keccak256("label")` for a dotted
/// lower-case label (`st0x.deploy`, `st0x.vault-beacon.v1`): the convention the
/// deploy repo's binding names and migration namespaces follow. Upper-case
/// preimages are roles, which `role_names` covers.
/// @return `(0x-hash, label)` pairs, deduplicated, first appearance wins.
pub fn registry_names(sources: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Ok(hashed) =
        Regex::new(r#"keccak256\(\s*"([a-z][a-z0-9]*(?:[.-][a-z0-9]+)*\.[a-z0-9.-]+)"\s*\)"#)
    else {
        return out;
    };
    for src in sources {
        for c in hashed.captures_iter(src) {
            let id = crate::rpc::role_id_hex(&c[1]);
            if !out.iter().any(|(i, _)| *i == id) {
                out.push((id, c[1].to_string()));
            }
        }
    }
    out
}

/// Names from the deploy repo's `ideal-roles.json`: each `(role, roleName)` pair
/// whose name hashes to its role, or is `DEFAULT_ADMIN_ROLE` for `bytes32(0)`. A
/// pair whose name is not its role's is dropped, never shown.
pub fn ideal_role_names(json: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(json) else {
        return out;
    };
    let zero = format!("0x{}", "0".repeat(64));
    let chains = doc.get("chains").and_then(|c| c.as_object());
    for chain in chains.into_iter().flat_map(|c| c.values()) {
        let contracts = chain.get("contracts").and_then(|c| c.as_array());
        for contract in contracts.into_iter().flatten() {
            let pairs = contract.get("expected").and_then(|e| e.as_array());
            for pair in pairs.into_iter().flatten() {
                let (Some(role), Some(name)) = (
                    pair.get("role").and_then(|r| r.as_str()),
                    pair.get("roleName").and_then(|n| n.as_str()),
                ) else {
                    continue;
                };
                let role = role.to_lowercase();
                let hashes = if name == "DEFAULT_ADMIN_ROLE" {
                    role == zero
                } else {
                    crate::rpc::role_id_hex(name) == role
                };
                if hashes && !out.iter().any(|(i, _)| *i == role) {
                    out.push((role, name.to_string()));
                }
            }
        }
    }
    out
}

/// The `deploymentRbac` document.
///
/// `source` / `networks` are `None` when the deploy repo's workflow or
/// `networks.json` could not be read; the document then says which, rather than
/// listing no networks.
pub fn build_rbac(
    org: &str,
    repo: &str,
    source: Option<&SubgraphSource>,
    networks: Option<&[String]>,
    names: &[(String, String)],
) -> Value {
    let mut doc = json!({
        "org": org,
        "repo": repo,
        "tag": PROD_TAG,
    });
    let Some(src) = source else {
        doc["unavailable"] = Value::from(
            "could not read SUBGRAPH_NAME / ORMI_QUERY_BASE from .github/workflows/deploy-subgraph.yaml",
        );
        return doc;
    };
    doc["subgraph"] = Value::from(src.name.clone());
    let Some(nets) = networks else {
        doc["unavailable"] = Value::from("could not read subgraph/networks.json");
        return doc;
    };
    doc["networks"] = Value::from(
        nets.iter()
            .map(|n| {
                json!({
                    "network": n,
                    "chain": chain_name(n),
                    "endpoint": endpoint(src, n),
                })
            })
            .collect::<Vec<_>>(),
    );
    doc["roleNames"] = Value::Object(
        names
            .iter()
            .map(|(id, name)| (id.clone(), Value::from(name.clone())))
            .collect::<Map<_, _>>(),
    );
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r#"
name: Deploy subgraph
jobs:
  deploy:
    env:
      SUBGRAPH_NAME: st0x-rbac
      # Public query base for this Ormi account.
      ORMI_QUERY_BASE: https://subgraph.api.ormilabs.com/api/public/9f4f/
"#;

    fn src() -> SubgraphSource {
        parse_deploy_workflow(WORKFLOW).unwrap()
    }

    #[test]
    fn workflow_names_the_subgraph_and_its_query_base() {
        assert_eq!(
            src(),
            SubgraphSource {
                name: "st0x-rbac".into(),
                query_base: "https://subgraph.api.ormilabs.com/api/public/9f4f".into(),
            }
        );
    }

    #[test]
    fn workflow_missing_either_value_is_unread() {
        assert_eq!(parse_deploy_workflow("SUBGRAPH_NAME: x\n"), None);
        assert_eq!(parse_deploy_workflow("ORMI_QUERY_BASE: https://a\n"), None);
    }

    #[test]
    fn workflow_with_a_plain_http_base_is_refused() {
        assert_eq!(
            parse_deploy_workflow("SUBGRAPH_NAME: x\nORMI_QUERY_BASE: http://a\n"),
            None
        );
    }

    #[test]
    fn workflow_values_may_be_quoted() {
        let s =
            parse_deploy_workflow("SUBGRAPH_NAME: \"x\"\nORMI_QUERY_BASE: 'https://a'\n").unwrap();
        assert_eq!((s.name.as_str(), s.query_base.as_str()), ("x", "https://a"));
    }

    #[test]
    fn endpoint_addresses_the_prod_tag_not_a_version() {
        assert_eq!(
            endpoint(&src(), "hyperevm"),
            "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-hyperevm/prod/gn"
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
        assert_eq!(chain_name("arbitrum-one"), None);
    }

    #[test]
    fn ideal_role_names_keep_only_names_that_are_their_roles() {
        let zero = format!("0x{}", "0".repeat(64));
        let emergency = crate::rpc::role_id_hex("EMERGENCY");
        let mint = crate::rpc::role_id_hex("MINT");
        let json = json!({"version": 1, "chains": {"1": {"network": "ethereum", "contracts": [
            {"address": "0x01", "expected": [
                {"role": zero, "roleName": "DEFAULT_ADMIN_ROLE", "grantee": "0x02"},
                {"role": emergency.to_uppercase().replace("0X", "0x"), "roleName": "EMERGENCY", "grantee": "0x02"},
                {"role": mint, "roleName": "BURN", "grantee": "0x02"},
                {"role": crate::rpc::role_id_hex("CERTIFY"), "grantee": "0x02"}
            ]},
            {"address": "0x03", "expected": [
                {"role": zero, "roleName": "DEFAULT_ADMIN_ROLE", "grantee": "0x04"}
            ]}
        ]}}})
        .to_string();
        assert_eq!(
            ideal_role_names(&json),
            vec![
                (zero, "DEFAULT_ADMIN_ROLE".to_string()),
                (emergency, "EMERGENCY".to_string()),
            ]
        );
        assert!(ideal_role_names("not json").is_empty());
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
    fn a_zero_role_is_unnamed_unless_a_source_pins_it() {
        assert!(role_names(&["bytes32 constant X = bytes32(0);"]).is_empty());
    }

    #[test]
    fn rbac_doc_says_which_source_was_unread() {
        let d = build_rbac("o", "r", None, None, &[]);
        assert!(d["unavailable"]
            .as_str()
            .unwrap()
            .contains("deploy-subgraph.yaml"));
        assert!(d.get("networks").is_none());
        let s = src();
        let d = build_rbac("o", "r", Some(&s), None, &[]);
        assert!(d["unavailable"].as_str().unwrap().contains("networks.json"));
        assert_eq!(d["subgraph"], "st0x-rbac");
        assert!(d.get("networks").is_none());
    }

    #[test]
    fn registry_names_come_from_dotted_lower_case_hash_preimages() {
        let src = r#"
            bytes32 internal constant NAMESPACE = keccak256("st0x.deploy");
            bytes32 constant B = keccak256( "st0x.vault-beacon.v1" );
            bytes32 constant R = keccak256("MINT");
            bytes32 constant T = keccak256("EIP712Domain(string name)");
            bytes32 constant U = keccak256("undotted");
            bytes32 constant D = keccak256("st0x.deploy");
        "#;
        assert_eq!(
            registry_names(&[src]),
            vec![
                (
                    crate::rpc::role_id_hex("st0x.deploy"),
                    "st0x.deploy".to_string()
                ),
                (
                    crate::rpc::role_id_hex("st0x.vault-beacon.v1"),
                    "st0x.vault-beacon.v1".to_string()
                ),
            ]
        );
    }

    #[test]
    fn registry_name_hash_is_the_keccak_of_the_label() {
        // `cast keccak "st0x.deploy"`: LibStoxMigrations.NAMESPACE.
        assert_eq!(
            registry_names(&[r#"keccak256("st0x.deploy")"#])[0].0,
            "0xd9cec8396b568ba46ea521970e5edeac9163854e75287e989c213c3e9e84bda4"
        );
    }

    #[test]
    fn rbac_doc_lists_each_network_with_its_chain_endpoint_and_the_names() {
        let nets = vec!["base".to_string(), "arbitrum-one".to_string()];
        let names = vec![
            ("0xaa".to_string(), "MINT".to_string()),
            ("0xbb".to_string(), "BURN".to_string()),
        ];
        let d = build_rbac("o", "r", Some(&src()), Some(&nets), &names);
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
}
