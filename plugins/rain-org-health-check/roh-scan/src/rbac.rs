//! Reads st0x.deploy's RBAC subgraph so the Deployments page can show, per
//! network and per contract, every role grant and revoke and who holds each
//! role now. Pure parsing + assembly live here and are unit-tested; the network
//! fetches (`gh_file`, the GraphQL POST, the chain-head read) are in main.rs and
//! are injected.
//!
//! Everything that locates the subgraph is read from the deploy repo, not typed
//! here: the networks are `subgraph/networks.json`'s keys, and the deployment
//! name and query base are the `SUBGRAPH_NAME` / `ORMI_QUERY_BASE` its deploy
//! workflow passes to rainix's `subgraph-deploy`. That task publishes each
//! network as `<SUBGRAPH_NAME>-<network>` at version label
//! `<address>-<commit>`, which changes on every redeploy, and keeps the name
//! shape so a tag can move between versions. Consumers address the tag —
//! raindex's own settings read `…/raindex-base/prod/gn` — so this does too.
//!
//! The state of each network's index is reported as read, never assumed: no
//! version at the tag, an unreachable endpoint, a query error (including "has
//! not started syncing"), indexing errors, and an index still behind the
//! chain head are each a state of their own. A partial index is never rendered
//! as a complete history.

use regex::Regex;
use serde_json::{json, Value};

/// The Ormi tag consumers read. rainix's `subgraph-deploy` keeps the
/// `<name>/<version>` shape "so an Ormi tag can move between versions", and
/// `prod` is the tag the org's consumers address (raindex's settings).
pub const PROD_TAG: &str = "prod";

/// The exact body Ormi returns for a name/version (or tag) it does not host —
/// the same string rainix's `ormi-probe` treats as "missing".
const MISSING_ERROR: &str = "subgraph name/version error";

/// Rows per GraphQL page. graph-node's cap on `first`.
const PAGE: usize = 1000;

/// Pages read per entity before the read is abandoned as incomplete rather
/// than rendered short.
const MAX_PAGES: usize = 100;

/// How far behind the chain head an index may be and still be reported as
/// caught up. The scan runs hours apart and a live index trails the head by
/// seconds; anything past this is an index that has not reached the present,
/// and its history is only complete up to its own block.
pub const SYNC_TOLERANCE_SECS: u64 = 600;

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
    // The base goes into a URL the scanner POSTs to. Anything but https is
    // refused for the reason `ormi-probe` refuses it: a response rewritten in
    // transit would be rendered as the chain's role history.
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

/// The scanner's chain name for a subgraph network name.
///
/// graph-cli names chains its own way (`mainnet`, `hyper-evm`); the rest of the
/// scan and the page use the deploy repo's constant suffixes (`ethereum`,
/// `hyperevm`). `None` for a network neither vocabulary covers, which leaves
/// that network's head unread rather than guessed.
pub fn chain_name(network: &str) -> Option<&'static str> {
    match network {
        "base" => Some("base"),
        "mainnet" => Some("ethereum"),
        "hyper-evm" => Some("hyperevm"),
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

/// One GraphQL response, classified.
#[derive(Debug, PartialEq, Clone)]
pub enum Answer {
    /// `data` with no `errors`.
    Data(Value),
    /// Ormi hosts nothing at this name and tag.
    Missing,
    /// The endpoint answered with an error: a GraphQL `errors` array (which
    /// includes "has not started syncing yet"), or an `error` other than the
    /// missing one.
    QueryError(String),
    /// No answer, or one that is not JSON.
    Unreachable(String),
}

/// Classify a response body. `None` is a transport failure.
pub fn classify(body: Option<&[u8]>) -> Answer {
    let Some(body) = body else {
        return Answer::Unreachable("no response from the endpoint".into());
    };
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return Answer::Unreachable("the endpoint answered with something that is not JSON".into());
    };
    if let Some(errs) = v.get("errors") {
        let msgs: Vec<String> = errs
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|e| {
                        e.get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| e.to_string())
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![errs.to_string()]);
        return Answer::QueryError(msgs.join("; "));
    }
    if let Some(e) = v.get("error") {
        return match e.as_str() {
            Some(MISSING_ERROR) => Answer::Missing,
            Some(s) => Answer::QueryError(s.to_string()),
            None => Answer::QueryError(e.to_string()),
        };
    }
    match v.get("data") {
        Some(d) if d.is_object() => Answer::Data(d.clone()),
        _ => Answer::QueryError("the response carried no data".into()),
    }
}

/// A chain's head, read from an RPC.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Head {
    pub number: u64,
    pub timestamp: u64,
}

/// Parse an `eth_getBlockByNumber("latest", false)` response.
pub fn parse_head(body: &[u8]) -> Option<Head> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let r = v.get("result")?;
    let hex = |k: &str| {
        r.get(k)
            .and_then(Value::as_str)
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
    };
    Some(Head {
        number: hex("number")?,
        timestamp: hex("timestamp")?,
    })
}

/// A graph-node `BigInt` / `Int`, which arrives as a string or a number.
fn num(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// The GraphQL request body for `query`.
fn body(query: &str) -> String {
    json!({ "query": query }).to_string()
}

/// POSTs a GraphQL request body to a URL; `None` on transport failure.
pub type Query<'a> = &'a dyn Fn(&str, &str) -> Option<Vec<u8>>;

/// Every row of one entity at block `block`, paged by `id`.
///
/// Pinned to one block so the pages are one snapshot: an index that moves
/// between pages would otherwise join history from two different heights.
/// A failed or malformed page fails the whole read — a short list rendered as
/// the list is the silent empty this section refuses.
fn read_all(
    query: Query,
    url: &str,
    entity: &str,
    filter: &str,
    fields: &str,
    block: u64,
) -> Result<Vec<Value>, String> {
    let mut rows: Vec<Value> = Vec::new();
    let mut cursor = "0x".to_string();
    for _ in 0..MAX_PAGES {
        let q = format!(
            r#"{{ page: {entity}(first: {PAGE}, orderBy: id, orderDirection: asc, block: {{number: {block}}}, where: {{id_gt: "{cursor}"{filter}}}) {{ id {fields} }} }}"#
        );
        let page = match classify(query(url, &body(&q)).as_deref()) {
            Answer::Data(d) => d,
            Answer::Missing => return Err(format!("{entity}: the subgraph vanished mid-read")),
            Answer::QueryError(e) | Answer::Unreachable(e) => return Err(format!("{entity}: {e}")),
        };
        let Some(items) = page.get("page").and_then(Value::as_array) else {
            return Err(format!("{entity}: the response carried no rows"));
        };
        let n = items.len();
        let Some(last) = items
            .last()
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
        else {
            return Ok(rows);
        };
        cursor = last.to_string();
        rows.extend(items.iter().cloned());
        if n < PAGE {
            return Ok(rows);
        }
    }
    Err(format!(
        "{entity}: more than {} rows; not rendered short",
        PAGE * MAX_PAGES
    ))
}

/// The index's own position, from `_meta`.
#[derive(Debug, PartialEq, Clone)]
struct Meta {
    block: u64,
    timestamp: Option<u64>,
    has_indexing_errors: bool,
    deployment: Option<String>,
}

fn parse_meta(d: &Value) -> Option<Meta> {
    let m = d.get("_meta")?;
    Some(Meta {
        block: num(m.pointer("/block/number")?)?,
        timestamp: m.pointer("/block/timestamp").and_then(num),
        has_indexing_errors: m.get("hasIndexingErrors")?.as_bool()?,
        deployment: m.get("deployment").and_then(s),
    })
}

/// How far the index has got.
///
/// Indexing errors outrank lag: an index that has hit an error stops, so its
/// lag only grows, and "syncing" would promise a catch-up that will not come.
/// Without a head (or a block timestamp) the lag is unknown, and an unknown lag
/// is not a synced one.
fn sync_state(meta: &Meta, head: Option<Head>) -> &'static str {
    if meta.has_indexing_errors {
        return "indexing-errors";
    }
    match (head, meta.timestamp) {
        (Some(h), Some(ts)) if h.timestamp.saturating_sub(ts) <= SYNC_TOLERANCE_SECS => "synced",
        (Some(_), Some(_)) => "syncing",
        _ => "head-unknown",
    }
}

/// Assemble one network's section from the three entity reads.
///
/// Grouped by contract. A contract appears when it has at least one role row,
/// which `AccessControl` creates on the first event naming the role — so every
/// role contract with any history is here, and `Ownable`-only contracts (vaults,
/// beacons) are not, having no roles to report.
fn assemble_contracts(
    roles: &[Value],
    holders: &[Value],
    grants: &[Value],
    names: &[(String, String)],
) -> Vec<Value> {
    let name_of = |id: &str| -> Value {
        names
            .iter()
            .find(|(i, _)| i.eq_ignore_ascii_case(id))
            .map(|(_, n)| Value::from(n.clone()))
            .unwrap_or(Value::Null)
    };
    // (address, kind, firstIndexedBlock) in first-seen order, then sorted.
    let mut contracts: Vec<(String, String, Option<u64>)> = Vec::new();
    for r in roles {
        let Some(c) = r.get("contract") else { continue };
        let Some(addr) = c.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !contracts.iter().any(|(a, _, _)| a == addr) {
            contracts.push((
                addr.to_string(),
                c.get("kind").and_then(s).unwrap_or_default(),
                c.get("firstIndexedBlock").and_then(num),
            ));
        }
    }
    contracts.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

    let role_ref = |v: &Value| {
        v.pointer("/role/id")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    contracts
        .into_iter()
        .map(|(addr, kind, first)| {
            let mut role_rows: Vec<(String, Value)> = roles
                .iter()
                .filter(|r| r.pointer("/contract/id").and_then(Value::as_str) == Some(&addr))
                .map(|r| {
                    let rid = r.get("id").and_then(s).unwrap_or_default();
                    let role = r.get("role").and_then(s).unwrap_or_default();
                    let admin = r.get("admin").and_then(s);
                    let mut hs: Vec<Value> = holders
                        .iter()
                        .filter(|h| role_ref(h).as_deref() == Some(rid.as_str()))
                        .map(|h| {
                            json!({
                                "account": h.get("account").and_then(s),
                                "grantedAtBlock": h.get("grantedAtBlock").and_then(num),
                                "grantedAtTimestamp": h.get("grantedAtTimestamp").and_then(num),
                            })
                        })
                        .collect();
                    hs.sort_by(|a, b| a["account"].as_str().cmp(&b["account"].as_str()));
                    let named = name_of(&role);
                    let sort_key = named.as_str().map(str::to_string).unwrap_or_else(|| {
                        // Unnamed roles after every named one, by id.
                        format!("~{role}")
                    });
                    (
                        sort_key,
                        json!({
                            "roleId": role,
                            "name": named,
                            "adminId": admin,
                            "adminName": admin.as_deref().map(name_of).unwrap_or(Value::Null),
                            "holders": hs,
                        }),
                    )
                })
                .collect();
            role_rows.sort_by(|a, b| a.0.cmp(&b.0));

            let role_ids: Vec<(String, String)> = roles
                .iter()
                .filter(|r| r.pointer("/contract/id").and_then(Value::as_str) == Some(&addr))
                .filter_map(|r| Some((r.get("id").and_then(s)?, r.get("role").and_then(s)?)))
                .collect();
            let mut changes: Vec<(u64, u64, Value)> = grants
                .iter()
                .filter_map(|g| {
                    let rid = role_ref(g)?;
                    let (_, role) = role_ids.iter().find(|(i, _)| *i == rid)?;
                    let block = g.get("blockNumber").and_then(num);
                    let log = g.get("logIndex").and_then(num);
                    Some((
                        block.unwrap_or(0),
                        log.unwrap_or(0),
                        json!({
                            "roleId": role,
                            "name": name_of(role),
                            "account": g.get("account").and_then(s),
                            "granted": g.get("granted").and_then(Value::as_bool),
                            "sender": g.get("sender").and_then(s),
                            "transactionFrom": g.get("transactionFrom").and_then(s),
                            "blockNumber": block,
                            "blockTimestamp": g.get("blockTimestamp").and_then(num),
                            "transactionHash": g.get("transactionHash").and_then(s),
                        }),
                    ))
                })
                .collect();
            // Newest first: the question a reader brings is "what changed lately".
            changes.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
            json!({
                "address": addr,
                "kind": kind,
                "firstIndexedBlock": first,
                "roles": role_rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
                "changes": changes.into_iter().map(|(_, _, v)| v).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Read one network's index: its position, then every role, current holder
/// and grant/revoke at that position.
///
/// `head` is called only once the index has answered, so an unreachable index
/// costs no RPC call.
pub fn build_network(
    src: &SubgraphSource,
    network: &str,
    names: &[(String, String)],
    query: Query,
    head: &dyn Fn(&str) -> Option<Head>,
) -> Value {
    let url = endpoint(src, network);
    let chain = chain_name(network);
    let base = |state: &str, detail: Option<String>| {
        json!({
            "network": network,
            "chain": chain,
            "endpoint": url,
            "state": state,
            "detail": detail,
        })
    };
    let meta_q = "{ _meta { block { number timestamp } hasIndexingErrors deployment } }";
    let meta = match classify(query(&url, &body(meta_q)).as_deref()) {
        Answer::Missing => {
            return base(
                "missing",
                Some(format!(
                    "nothing is published at {}-{network}/{PROD_TAG}",
                    src.name
                )),
            )
        }
        Answer::Unreachable(e) => return base("unreachable", Some(e)),
        Answer::QueryError(e) => return base("error", Some(e)),
        Answer::Data(d) => match parse_meta(&d) {
            Some(m) => m,
            None => {
                return base(
                    "error",
                    Some("the index answered without a block position".into()),
                )
            }
        },
    };
    let h = chain.and_then(head);
    let mut out = base(sync_state(&meta, h), None);
    let position = json!({
        "indexedBlock": meta.block,
        "indexedTimestamp": meta.timestamp,
        "headBlock": h.map(|h| h.number),
        "headTimestamp": h.map(|h| h.timestamp),
        "lagBlocks": h.map(|h| h.number.saturating_sub(meta.block)),
        "lagSeconds": h.zip(meta.timestamp).map(|(h, t)| h.timestamp.saturating_sub(t)),
        "hasIndexingErrors": meta.has_indexing_errors,
        "deployment": meta.deployment,
    });
    if let (Some(o), Some(p)) = (out.as_object_mut(), position.as_object()) {
        o.extend(p.clone());
    }
    let reads = (|| -> Result<Vec<Value>, String> {
        let roles = read_all(
            query,
            &url,
            "roles",
            "",
            "role admin contract { id kind firstIndexedBlock }",
            meta.block,
        )?;
        let holders = read_all(
            query,
            &url,
            "roleHolders",
            ", held: true",
            "account grantedAtBlock grantedAtTimestamp role { id }",
            meta.block,
        )?;
        let grants = read_all(
            query,
            &url,
            "roleGrants",
            "",
            "account granted sender transactionFrom blockNumber blockTimestamp transactionHash logIndex role { id }",
            meta.block,
        )?;
        Ok(assemble_contracts(&roles, &holders, &grants, names))
    })();
    match reads {
        Ok(contracts) => out["contracts"] = Value::from(contracts),
        // The position is real but the history is not whole, so none of it is
        // shown: a list missing a page reads as the list.
        Err(e) => {
            out["state"] = Value::from("error");
            out["detail"] = Value::from(e);
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
    query: Query,
    head: &dyn Fn(&str) -> Option<Head>,
) -> Value {
    let mut doc = json!({
        "org": org,
        "repo": repo,
        "tag": PROD_TAG,
        "syncToleranceSecs": SYNC_TOLERANCE_SECS,
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
            .map(|n| build_network(src, n, names, query, head))
            .collect::<Vec<_>>(),
    );
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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
            endpoint(&src(), "hyper-evm"),
            "https://subgraph.api.ormilabs.com/api/public/9f4f/subgraphs/st0x-rbac-hyper-evm/prod/gn"
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
    fn chain_names_translate_graph_cli_names() {
        assert_eq!(chain_name("mainnet"), Some("ethereum"));
        assert_eq!(chain_name("hyper-evm"), Some("hyperevm"));
        assert_eq!(chain_name("base"), Some("base"));
        assert_eq!(chain_name("robinhood"), Some("robinhood"));
        assert_eq!(chain_name("bsc"), Some("bsc"));
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
    fn a_zero_role_is_unnamed_unless_a_source_pins_it() {
        assert!(role_names(&["bytes32 constant X = bytes32(0);"]).is_empty());
    }

    #[test]
    fn classify_tells_the_four_answers_apart() {
        assert_eq!(
            classify(Some(br#"{"data":{"a":1}}"#)),
            Answer::Data(json!({"a":1}))
        );
        assert_eq!(
            classify(Some(br#"{"error":"subgraph name/version error"}"#)),
            Answer::Missing
        );
        assert_eq!(
            classify(Some(br#"{"error":"path project error"}"#)),
            Answer::QueryError("path project error".into())
        );
        assert_eq!(
            classify(Some(
                br#"{"errors":[{"message":"has not started syncing yet"},{"message":"b"}]}"#
            )),
            Answer::QueryError("has not started syncing yet; b".into())
        );
        // `errors` beside `data` is a failure, not a partial answer.
        assert!(matches!(
            classify(Some(br#"{"data":{"a":1},"errors":[{"message":"x"}]}"#)),
            Answer::QueryError(_)
        ));
        assert!(matches!(classify(Some(b"<html>")), Answer::Unreachable(_)));
        assert!(matches!(classify(None), Answer::Unreachable(_)));
        assert!(matches!(
            classify(Some(br#"{"data":null}"#)),
            Answer::QueryError(_)
        ));
        assert!(matches!(
            classify(Some(br#"{"error":{"code":1}}"#)),
            Answer::QueryError(_)
        ));
        assert!(
            matches!(classify(Some(br#"{"errors":"flat"}"#)), Answer::QueryError(m) if m.contains("flat"))
        );
    }

    #[test]
    fn head_parses_hex_number_and_timestamp() {
        assert_eq!(
            parse_head(br#"{"result":{"number":"0x10","timestamp":"0x20"}}"#),
            Some(Head {
                number: 16,
                timestamp: 32
            })
        );
        assert_eq!(parse_head(br#"{"result":{"number":"0x10"}}"#), None);
        assert_eq!(parse_head(br#"{"error":{}}"#), None);
    }

    fn meta(ts: Option<u64>, errs: bool) -> Meta {
        Meta {
            block: 100,
            timestamp: ts,
            has_indexing_errors: errs,
            deployment: None,
        }
    }

    #[test]
    fn sync_state_boundaries() {
        let h = |t| {
            Some(Head {
                number: 200,
                timestamp: t,
            })
        };
        assert_eq!(
            sync_state(&meta(Some(1000), false), h(1000 + SYNC_TOLERANCE_SECS)),
            "synced"
        );
        assert_eq!(
            sync_state(&meta(Some(1000), false), h(1001 + SYNC_TOLERANCE_SECS)),
            "syncing"
        );
        // A head behind the index (a lagging RPC) is not negative lag.
        assert_eq!(sync_state(&meta(Some(1000), false), h(900)), "synced");
        assert_eq!(sync_state(&meta(Some(1000), false), None), "head-unknown");
        assert_eq!(sync_state(&meta(None, false), h(1000)), "head-unknown");
        assert_eq!(
            sync_state(&meta(Some(1000), true), h(1000)),
            "indexing-errors"
        );
    }

    const AUTH: &str = "0x66566cc91deaf818859bd4b09b7903ac48998157";
    const TL: &str = "0x831e4e1bb2b9a67c00b7d17f252a18a22cd0bd2b";

    fn rid(c: &str, r: &str) -> String {
        format!("{c}{}", r.trim_start_matches("0x"))
    }

    /// A fake index answering `_meta` and the three entity reads, recording
    /// every request body it was sent.
    struct Fake {
        meta: String,
        roles: Vec<Value>,
        holders: Vec<Value>,
        grants: Vec<Value>,
        fail: Option<&'static str>,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Fake {
            let dep = crate::rpc::role_id_hex("DEPOSIT");
            let adm = crate::rpc::role_id_hex("DEPOSIT_ADMIN");
            let prop = crate::rpc::role_id_hex("PROPOSER_ROLE");
            let unk = format!("0x{}", "ab".repeat(32));
            let role = |c: &str, kind: &str, r: &str, admin: &str| {
                json!({"id": rid(c, r), "role": r, "admin": admin,
                       "contract": {"id": c, "kind": kind, "firstIndexedBlock": "5"}})
            };
            Fake {
                meta: r#"{"data":{"_meta":{"block":{"number":900,"timestamp":5000},"hasIndexingErrors":false,"deployment":"Qm1"}}}"#.into(),
                roles: vec![
                    role(TL, "GOVERNANCE_TIMELOCK", &prop, &format!("0x{}", "0".repeat(64))),
                    role(AUTH, "AUTHORIZER", &dep, &adm),
                    role(AUTH, "AUTHORIZER", &unk, &adm),
                ],
                holders: vec![
                    json!({"id": "h1", "account": "0xbb", "grantedAtBlock": "10", "grantedAtTimestamp": "100", "role": {"id": rid(AUTH, &dep)}}),
                    json!({"id": "h2", "account": "0xaa", "grantedAtBlock": "12", "grantedAtTimestamp": "120", "role": {"id": rid(AUTH, &dep)}}),
                ],
                grants: vec![
                    json!({"id": "g1", "account": "0xaa", "granted": true, "sender": "0x5a", "transactionFrom": "0xe0",
                           "blockNumber": "12", "blockTimestamp": "120", "transactionHash": "0xt1", "logIndex": "3", "role": {"id": rid(AUTH, &dep)}}),
                    json!({"id": "g2", "account": "0xcc", "granted": false, "sender": "0xcc", "transactionFrom": "0xcc",
                           "blockNumber": "12", "blockTimestamp": "120", "transactionHash": "0xt1", "logIndex": "7", "role": {"id": rid(AUTH, &unk)}}),
                    json!({"id": "g3", "account": "0xbb", "granted": true, "sender": "0x5a", "transactionFrom": "0xe0",
                           "blockNumber": "10", "blockTimestamp": "100", "transactionHash": "0xt0", "logIndex": "1", "role": {"id": rid(AUTH, &dep)}}),
                    // A grant on a role this snapshot holds no row for is not
                    // attributed to any contract.
                    json!({"id": "g4", "account": "0xdd", "granted": true, "sender": "0x5a", "transactionFrom": "0xe0",
                           "blockNumber": "11", "blockTimestamp": "110", "transactionHash": "0xt2", "logIndex": "1", "role": {"id": "0xnope"}}),
                ],
                fail: None,
                seen: RefCell::new(Vec::new()),
            }
        }

        fn answer(&self, req: &str) -> Option<Vec<u8>> {
            self.seen.borrow_mut().push(req.to_string());
            let q: Value = serde_json::from_str(req).unwrap();
            let q = q["query"].as_str().unwrap().to_string();
            if q.contains("_meta") {
                return Some(self.meta.clone().into_bytes());
            }
            for (entity, rows) in [
                ("roleHolders(", &self.holders),
                ("roleGrants(", &self.grants),
                ("roles(", &self.roles),
            ] {
                if q.contains(entity) {
                    if self.fail == Some(entity) {
                        return None;
                    }
                    // Honour the cursor, so the pager's termination is real.
                    let cursor = q
                        .split("id_gt: \"")
                        .nth(1)
                        .unwrap()
                        .split('"')
                        .next()
                        .unwrap()
                        .to_string();
                    let page: Vec<&Value> = rows
                        .iter()
                        .filter(|r| r["id"].as_str().unwrap() > cursor.as_str())
                        .collect();
                    return Some(json!({"data": {"page": page}}).to_string().into_bytes());
                }
            }
            panic!("unexpected query {q}");
        }
    }

    fn names() -> Vec<(String, String)> {
        role_names(&[
            r#"keccak256("DEPOSIT") keccak256("DEPOSIT_ADMIN") keccak256("PROPOSER_ROLE")
                       DEFAULT_ADMIN_ROLE = bytes32(0)"#,
        ])
    }

    fn run(f: &Fake, head: Option<Head>) -> Value {
        build_network(&src(), "mainnet", &names(), &|_, b| f.answer(b), &|_| head)
    }

    const HEAD: Option<Head> = Some(Head {
        number: 950,
        timestamp: 5100,
    });

    #[test]
    fn network_groups_roles_holders_and_changes_by_contract() {
        let f = Fake::new();
        let n = run(&f, HEAD);
        assert_eq!(n["state"], "synced");
        assert_eq!(n["chain"], "ethereum");
        assert_eq!(n["indexedBlock"], 900);
        assert_eq!(n["headBlock"], 950);
        assert_eq!(n["lagBlocks"], 50);
        assert_eq!(n["lagSeconds"], 100);
        assert_eq!(n["deployment"], "Qm1");
        let cs = n["contracts"].as_array().unwrap();
        // Sorted by kind: AUTHORIZER before GOVERNANCE_TIMELOCK.
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0]["address"], AUTH);
        assert_eq!(cs[0]["kind"], "AUTHORIZER");
        assert_eq!(cs[0]["firstIndexedBlock"], 5);
        assert_eq!(cs[1]["address"], TL);

        let roles = cs[0]["roles"].as_array().unwrap();
        // Named first, unnamed after.
        assert_eq!(roles[0]["name"], "DEPOSIT");
        assert_eq!(roles[0]["adminName"], "DEPOSIT_ADMIN");
        assert_eq!(roles[1]["name"], Value::Null);
        assert_eq!(roles[1]["roleId"], format!("0x{}", "ab".repeat(32)));
        assert_eq!(roles[1]["holders"], json!([]));
        let hs: Vec<&str> = roles[0]["holders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["account"].as_str().unwrap())
            .collect();
        assert_eq!(hs, vec!["0xaa", "0xbb"]);
        assert_eq!(roles[0]["holders"][0]["grantedAtBlock"], 12);
        assert_eq!(roles[0]["holders"][0]["grantedAtTimestamp"], 120);

        // Newest first by (block, logIndex); the orphan grant is dropped.
        let ch = cs[0]["changes"].as_array().unwrap();
        let order: Vec<&str> = ch.iter().map(|c| c["account"].as_str().unwrap()).collect();
        assert_eq!(order, vec!["0xcc", "0xaa", "0xbb"]);
        assert_eq!(ch[0]["granted"], false);
        assert_eq!(ch[0]["name"], Value::Null);
        assert_eq!(ch[1]["name"], "DEPOSIT");
        assert_eq!(ch[1]["sender"], "0x5a");
        assert_eq!(ch[1]["transactionFrom"], "0xe0");
        assert_eq!(ch[1]["blockNumber"], 12);
        assert_eq!(ch[1]["blockTimestamp"], 120);
        assert_eq!(ch[1]["transactionHash"], "0xt1");

        let tl = cs[1]["roles"].as_array().unwrap();
        assert_eq!(tl[0]["name"], "PROPOSER_ROLE");
        assert_eq!(tl[0]["adminName"], "DEFAULT_ADMIN_ROLE");
        assert_eq!(cs[1]["changes"], json!([]));
    }

    #[test]
    fn every_entity_read_is_pinned_to_the_meta_block_and_holders_to_held() {
        let f = Fake::new();
        run(&f, HEAD);
        let seen = f.seen.borrow();
        let entity_reads: Vec<&String> = seen.iter().filter(|q| !q.contains("_meta")).collect();
        assert!(entity_reads.len() >= 3);
        for q in &entity_reads {
            assert!(q.contains("block: {number: 900}"), "unpinned read: {q}");
        }
        let holders = entity_reads
            .iter()
            .find(|q| q.contains("roleHolders("))
            .unwrap();
        assert!(holders.contains("held: true"));
    }

    #[test]
    fn missing_tag_is_its_own_state_and_reads_nothing_else() {
        let mut f = Fake::new();
        f.meta = r#"{"error":"subgraph name/version error"}"#.into();
        let n = build_network(&src(), "hyper-evm", &names(), &|_, b| f.answer(b), &|_| {
            panic!("head read for a missing index")
        });
        assert_eq!(n["state"], "missing");
        assert_eq!(
            n["detail"],
            "nothing is published at st0x-rbac-hyper-evm/prod"
        );
        assert_eq!(n["chain"], "hyperevm");
        assert!(n.get("contracts").is_none());
        assert_eq!(f.seen.borrow().len(), 1);
    }

    #[test]
    fn unreachable_and_error_indexes_say_so() {
        let n = build_network(&src(), "base", &names(), &|_, _| None, &|_| HEAD);
        assert_eq!(n["state"], "unreachable");
        let mut f = Fake::new();
        f.meta = r#"{"errors":[{"message":"has not started syncing yet"}]}"#.into();
        let n = run(&f, HEAD);
        assert_eq!(n["state"], "error");
        assert_eq!(n["detail"], "has not started syncing yet");
        f.meta = r#"{"data":{"_meta":null}}"#.into();
        assert_eq!(run(&f, HEAD)["state"], "error");
    }

    #[test]
    fn a_failed_page_discards_the_history_but_keeps_the_position() {
        for entity in ["roles(", "roleHolders(", "roleGrants("] {
            let mut f = Fake::new();
            f.fail = Some(entity);
            let n = run(&f, HEAD);
            assert_eq!(n["state"], "error", "{entity}");
            assert!(n.get("contracts").is_none(), "{entity}");
            assert_eq!(n["indexedBlock"], 900);
            assert!(n["detail"]
                .as_str()
                .unwrap()
                .starts_with(entity.trim_end_matches('(')));
        }
    }

    #[test]
    fn syncing_head_unknown_and_indexing_errors_are_reported_with_data() {
        let f = Fake::new();
        let behind = Some(Head {
            number: 99_999,
            timestamp: 5000 + SYNC_TOLERANCE_SECS + 1,
        });
        let n = run(&f, behind);
        assert_eq!(n["state"], "syncing");
        assert!(n["contracts"].is_array());
        let n = run(&f, None);
        assert_eq!(n["state"], "head-unknown");
        assert_eq!(n["headBlock"], Value::Null);
        assert_eq!(n["lagSeconds"], Value::Null);
        let mut f = Fake::new();
        f.meta = r#"{"data":{"_meta":{"block":{"number":"900","timestamp":"5000"},"hasIndexingErrors":true}}}"#.into();
        let n = run(&f, HEAD);
        assert_eq!(n["state"], "indexing-errors");
        assert_eq!(n["indexedBlock"], 900);
        assert_eq!(n["hasIndexingErrors"], true);
        assert!(n["contracts"].is_array());
    }

    #[test]
    fn a_network_with_no_chain_mapping_reads_no_head() {
        let f = Fake::new();
        let n = build_network(
            &src(),
            "arbitrum-one",
            &names(),
            &|_, b| f.answer(b),
            &|_| panic!("head asked"),
        );
        assert_eq!(n["state"], "head-unknown");
        assert_eq!(n["chain"], Value::Null);
    }

    #[test]
    fn pager_follows_the_cursor_across_full_pages() {
        let mut f = Fake::new();
        let dep = crate::rpc::role_id_hex("DEPOSIT");
        // Two full pages and a short third: every row arrives once.
        f.grants = (0..(2 * PAGE + 5))
            .map(|i| json!({"id": format!("g{i:06}"), "account": "0xaa", "granted": true, "sender": "0x5a",
                            "transactionFrom": "0xe0", "blockNumber": i.to_string(), "blockTimestamp": "1",
                            "transactionHash": "0xt", "logIndex": "0", "role": {"id": rid(AUTH, &dep)}}))
            .collect();
        // The fake returns everything past the cursor; cap it at PAGE.
        let n = build_network(
            &src(),
            "mainnet",
            &names(),
            &|_, b| {
                let full = f.answer(b)?;
                let mut v: Value = serde_json::from_slice(&full).unwrap();
                if let Some(a) = v.pointer_mut("/data/page").and_then(Value::as_array_mut) {
                    a.truncate(PAGE);
                }
                Some(v.to_string().into_bytes())
            },
            &|_| HEAD,
        );
        let ch = n["contracts"][0]["changes"].as_array().unwrap();
        assert_eq!(ch.len(), 2 * PAGE + 5);
        let grant_reads = f
            .seen
            .borrow()
            .iter()
            .filter(|q| q.contains("roleGrants("))
            .count();
        assert_eq!(grant_reads, 3);
    }

    #[test]
    fn pager_refuses_an_endless_read() {
        let f = |_: &str, _: &str| {
            let rows: Vec<Value> = (0..PAGE).map(|i| json!({"id": format!("x{i}")})).collect();
            Some(json!({"data": {"page": rows}}).to_string().into_bytes())
        };
        let r = read_all(&f, "u", "roles", "", "role", 1);
        assert!(r.unwrap_err().contains("not rendered short"));
    }

    #[test]
    fn pager_rejects_a_page_without_rows() {
        let f = |_: &str, _: &str| Some(br#"{"data":{}}"#.to_vec());
        assert!(read_all(&f, "u", "roles", "", "role", 1).is_err());
        let f = |_: &str, _: &str| Some(br#"{"error":"subgraph name/version error"}"#.to_vec());
        assert!(read_all(&f, "u", "roles", "", "role", 1)
            .unwrap_err()
            .contains("vanished"));
    }

    #[test]
    fn rbac_doc_says_which_source_was_unread() {
        let q = |_: &str, _: &str| None;
        let h = |_: &str| None;
        let d = build_rbac("o", "r", None, None, &[], &q, &h);
        assert!(d["unavailable"]
            .as_str()
            .unwrap()
            .contains("deploy-subgraph.yaml"));
        assert!(d.get("networks").is_none());
        let s = src();
        let d = build_rbac("o", "r", Some(&s), None, &[], &q, &h);
        assert!(d["unavailable"].as_str().unwrap().contains("networks.json"));
        assert_eq!(d["subgraph"], "st0x-rbac");
        let nets = vec!["base".to_string(), "hyper-evm".to_string()];
        let d = build_rbac("o", "r", Some(&s), Some(&nets), &[], &q, &h);
        assert_eq!(d["tag"], "prod");
        assert_eq!(d["syncToleranceSecs"], SYNC_TOLERANCE_SECS);
        let states: Vec<&str> = d["networks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["state"].as_str().unwrap())
            .collect();
        assert_eq!(states, vec!["unreachable", "unreachable"]);
        assert!(d.get("unavailable").is_none());
    }
}
