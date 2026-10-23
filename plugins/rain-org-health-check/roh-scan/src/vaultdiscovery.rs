//! The receipt vaults each chain actually has: every `Deployment` the vault
//! deployer emitted, as st0x.deploy's subgraph indexes them (`Contract` entities
//! of kind `RECEIPT_VAULT`). Located through `rbac`'s `SubgraphSource`.

use crate::rbac;
use serde_json::Value;

/// graph-node's maximum `first`.
pub const PAGE: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// Lowercased, ascending.
    pub vaults: Vec<String>,
    /// The block every page was read at.
    pub block: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    NoSource,
    NoNetworks,
    NotIndexed(String),
    Unreachable,
    NotJson,
    Graphql(String),
    IndexingErrors,
    Malformed,
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::NoSource => write!(
                f,
                "the subgraph location could not be read from st0x.deploy's deploy-subgraph workflow"
            ),
            DiscoveryError::NoNetworks => {
                write!(f, "st0x.deploy's subgraph/networks.json could not be read")
            }
            DiscoveryError::NotIndexed(n) => {
                write!(f, "st0x.deploy's subgraph is not deployed for {n}")
            }
            DiscoveryError::Unreachable => write!(f, "the subgraph did not answer"),
            DiscoveryError::NotJson => write!(f, "the subgraph answered with something that is not JSON"),
            DiscoveryError::Graphql(m) => write!(f, "the subgraph refused the query: {m}"),
            DiscoveryError::IndexingErrors => {
                write!(f, "the subgraph has indexing errors, so its vault list is incomplete")
            }
            DiscoveryError::Malformed => write!(f, "the subgraph's answer is not a vault list"),
        }
    }
}

/// The subgraph network whose page chain name is `chain`.
pub fn subgraph_network<'a>(networks: &'a [String], chain: &str) -> Option<&'a str> {
    networks
        .iter()
        .map(String::as_str)
        .find(|n| rbac::chain_name(n) == Some(chain))
}

/// One page of vaults after `after`, at `block` once the first page has fixed it.
pub fn page_query(after: &str, block: Option<u64>) -> String {
    let at = block.map_or(String::new(), |b| format!("block:{{number:{b}}},"));
    let query = format!(
        "{{contracts(first:{PAGE},orderBy:id,orderDirection:asc,{at}where:{{kind:RECEIPT_VAULT,id_gt:\"{after}\"}}){{id}} _meta({at}){{block{{number}} hasIndexingErrors}}}}"
    )
    .replace("_meta()", "_meta");
    serde_json::json!({ "query": query }).to_string()
}

fn is_address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// One page's ids and the block it was answered at.
pub fn parse_page(body: &[u8]) -> Result<(Vec<String>, u64), DiscoveryError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| DiscoveryError::NotJson)?;
    if let Some(errors) = v.get("errors").and_then(Value::as_array) {
        let msg = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(DiscoveryError::Graphql(msg));
    }
    let data = v.get("data").ok_or(DiscoveryError::Malformed)?;
    let meta = &data["_meta"];
    if meta["hasIndexingErrors"].as_bool() != Some(false) {
        return Err(if meta["hasIndexingErrors"].as_bool() == Some(true) {
            DiscoveryError::IndexingErrors
        } else {
            DiscoveryError::Malformed
        });
    }
    let block = meta["block"]["number"]
        .as_u64()
        .ok_or(DiscoveryError::Malformed)?;
    let ids = data["contracts"]
        .as_array()
        .ok_or(DiscoveryError::Malformed)?
        .iter()
        .map(|c| {
            c["id"]
                .as_str()
                .filter(|a| is_address(a))
                .map(str::to_lowercase)
                .ok_or(DiscoveryError::Malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ids, block))
}

/// Every vault, paging by id at one block. `post` sends a query body.
pub fn discover(post: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Discovered, DiscoveryError> {
    let mut vaults: Vec<String> = Vec::new();
    let mut block: Option<u64> = None;
    loop {
        let after = vaults.last().map_or("0x", String::as_str);
        let body = post(&page_query(after, block)).ok_or(DiscoveryError::Unreachable)?;
        let (ids, at) = parse_page(&body)?;
        if block.is_some_and(|b| b != at) {
            return Err(DiscoveryError::Malformed);
        }
        // Ascending and strictly after the cursor, or paging could loop or skip.
        let ascending = ids.windows(2).all(|w| w[0] < w[1])
            && ids
                .first()
                .is_none_or(|f| vaults.last().is_none_or(|l| f > l));
        if !ascending {
            return Err(DiscoveryError::Malformed);
        }
        let full = ids.len() == PAGE;
        vaults.extend(ids);
        block = Some(at);
        if !full {
            return Ok(Discovered { vaults, block: at });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn addr(i: usize) -> String {
        format!("0x{i:040x}")
    }

    fn page(ids: &[String], block: u64) -> Vec<u8> {
        let contracts: Vec<_> = ids.iter().map(|i| serde_json::json!({ "id": i })).collect();
        serde_json::json!({
            "data": {
                "contracts": contracts,
                "_meta": { "block": { "number": block }, "hasIndexingErrors": false },
            }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_page_reads_lowercased_ids_and_its_block() {
        let body = br#"{"data":{"contracts":[{"id":"0x00472AA0d0611f933c22b8148f02b0cdd1ae5fbc"}],"_meta":{"block":{"number":129276993},"hasIndexingErrors":false}}}"#;
        assert_eq!(
            parse_page(body),
            Ok((
                vec!["0x00472aa0d0611f933c22b8148f02b0cdd1ae5fbc".to_string()],
                129276993
            ))
        );
    }

    #[test]
    fn an_unusable_answer_is_an_error_never_an_empty_list() {
        assert_eq!(parse_page(b"<html>"), Err(DiscoveryError::NotJson));
        assert_eq!(
            parse_page(br#"{"errors":[{"message":"no field `nope`"}]}"#),
            Err(DiscoveryError::Graphql("no field `nope`".into()))
        );
        assert_eq!(
            parse_page(br#"{"data":{"contracts":[],"_meta":{"block":{"number":1},"hasIndexingErrors":true}}}"#),
            Err(DiscoveryError::IndexingErrors)
        );
        assert_eq!(
            parse_page(br#"{"data":{"contracts":[]}}"#),
            Err(DiscoveryError::Malformed)
        );
        assert_eq!(
            parse_page(br#"{"data":{"_meta":{"block":{"number":1},"hasIndexingErrors":false}}}"#),
            Err(DiscoveryError::Malformed)
        );
        assert_eq!(
            parse_page(br#"{"data":{"contracts":[{"id":"0x01"}],"_meta":{"block":{"number":1},"hasIndexingErrors":false}}}"#),
            Err(DiscoveryError::Malformed)
        );
    }

    #[test]
    fn a_full_page_is_followed_at_the_same_block_from_the_last_id() {
        let all: Vec<String> = (1..=PAGE + 3).map(addr).collect();
        let asked = RefCell::new(Vec::new());
        let post = |q: &str| {
            asked.borrow_mut().push(q.to_string());
            Some(if asked.borrow().len() == 1 {
                page(&all[..PAGE], 77)
            } else {
                page(&all[PAGE..], 77)
            })
        };
        let got = discover(&post).expect("discovers");
        assert_eq!(
            got,
            Discovered {
                vaults: all.clone(),
                block: 77
            }
        );
        let asked = asked.into_inner();
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains(r#"id_gt:\"0x\""#), "{}", asked[0]);
        assert!(!asked[0].contains("block:"), "{}", asked[0]);
        assert!(
            asked[1].contains(&format!(r#"id_gt:\"{}\""#, all[PAGE - 1])),
            "{}",
            asked[1]
        );
        assert!(asked[1].contains("block:{number:77}"), "{}", asked[1]);
    }

    #[test]
    fn a_failed_or_inconsistent_page_fails_the_whole_discovery() {
        let full: Vec<String> = (1..=PAGE).map(addr).collect();
        let calls = RefCell::new(0);
        let unreachable_second = |_: &str| {
            *calls.borrow_mut() += 1;
            (*calls.borrow() == 1).then(|| page(&full, 5))
        };
        assert_eq!(
            discover(&unreachable_second),
            Err(DiscoveryError::Unreachable)
        );

        let calls = RefCell::new(0);
        let moved_block = |_: &str| {
            *calls.borrow_mut() += 1;
            Some(page(
                if *calls.borrow() == 1 {
                    &full
                } else {
                    &full[..0]
                },
                *calls.borrow() as u64,
            ))
        };
        assert_eq!(discover(&moved_block), Err(DiscoveryError::Malformed));

        let calls = RefCell::new(0);
        let repeats = |_: &str| {
            *calls.borrow_mut() += 1;
            (*calls.borrow() <= 2).then(|| page(&full, 5))
        };
        assert_eq!(discover(&repeats), Err(DiscoveryError::Malformed));

        let unordered = |_: &str| Some(page(&[addr(2), addr(1)], 5));
        assert_eq!(discover(&unordered), Err(DiscoveryError::Malformed));
    }

    #[test]
    fn a_chain_is_found_under_the_subgraphs_own_network_name() {
        let nets: Vec<String> = ["base", "mainnet", "hyper-evm", "bsc"]
            .map(String::from)
            .to_vec();
        assert_eq!(subgraph_network(&nets, "ethereum"), Some("mainnet"));
        assert_eq!(subgraph_network(&nets, "hyperevm"), Some("hyper-evm"));
        assert_eq!(subgraph_network(&nets, "bsc"), Some("bsc"));
        assert_eq!(subgraph_network(&nets, "robinhood"), None);
    }
}
