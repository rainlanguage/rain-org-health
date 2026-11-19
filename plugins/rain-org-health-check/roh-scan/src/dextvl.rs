//! Dex TVL: what every recorded raindex holds, per network, per raindex, per
//! token, with USD totals (rain-org-health#248).
//!
//! - **Which raindexes:** raindex.deploy's `tokens-subgraph/manifests/<network>.yaml`,
//!   one data source per raindex, generated from that repo's canonical
//!   `deployments/raindex.json`. Nothing is listed here.
//! - **Which tokens:** that subgraph's `RaindexToken` (raindex, token) pairs, one
//!   per token ever deposited into each raindex.
//! - **How much:** `token.balanceOf(raindex)` at one stated block per network,
//!   on chain, the same for every raindex version.
//! - **USD:** one price source. A token it does not price is shown unpriced and
//!   counted in `excludedTokens`; it is never valued at zero.
//!
//! Anything unreadable carries its reason instead of a number.

use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// graph-node's maximum `first`.
pub const PAGE: usize = 1000;

/// One price source, named on the page beside every USD figure.
pub const PRICE_SOURCE: &str = "DefiLlama coins API, current prices";

/// One network's raindexes, as one tokens-subgraph manifest declares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// The graph network name, which is also the manifest's file stem.
    pub name: String,
    /// Lowercased, in manifest order.
    pub raindexes: Vec<String>,
}

fn is_address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// The network and raindex addresses of one generated manifest. `None` unless
/// it names exactly one network and every `address:` is an address.
pub fn parse_manifest(yaml: &str) -> Option<Network> {
    let network_re = Regex::new(r#"(?m)^\s*network:\s*["']?([A-Za-z0-9_-]+)"#).ok()?;
    let networks: BTreeSet<&str> = network_re
        .captures_iter(yaml)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if networks.len() != 1 {
        return None;
    }
    let address_re = Regex::new(r#"(?m)^\s*address:\s*["']?([^"'\s]+)"#).ok()?;
    let mut raindexes = Vec::new();
    for c in address_re.captures_iter(yaml) {
        let a = c.get(1)?.as_str();
        if !is_address(a) {
            return None;
        }
        raindexes.push(a.to_lowercase());
    }
    if raindexes.is_empty() {
        return None;
    }
    Some(Network {
        name: networks.into_iter().next()?.to_string(),
        raindexes,
    })
}

/// Where the tokens subgraph is published: the `Deploy subgraph` workflow's
/// `ORMI_QUERY_BASE`, and the matrix `name` whose `dir` is `tokens-subgraph`.
/// Each network is `<base>/subgraphs/<name>-<network>/prod/gn`.
pub fn parse_tokens_source(workflow: &str) -> Option<crate::rbac::SubgraphSource> {
    let base = Regex::new(r#"(?m)^\s*ORMI_QUERY_BASE:\s*["']?([^"'\s#]+)"#)
        .ok()?
        .captures(workflow)?
        .get(1)?
        .as_str()
        .trim_end_matches('/')
        .to_string();
    if !base.starts_with("https://") {
        return None;
    }
    let name = Regex::new(r#"(?m)^\s*-\s*name:\s*["']?([A-Za-z0-9_-]+)["']?\s*\n\s*dir:\s*["']?tokens-subgraph["']?\s*$"#)
        .ok()?
        .captures(workflow)?
        .get(1)?
        .as_str()
        .to_string();
    Some(crate::rbac::SubgraphSource {
        name,
        query_base: base,
    })
}

/// One page of pairs after `after`, at `block` once the first page has fixed it.
pub fn page_query(after: &str, block: Option<u64>) -> String {
    let at = block.map_or(String::new(), |b| format!("block:{{number:{b}}},"));
    let meta = block.map_or("_meta".to_string(), |b| {
        format!("_meta(block:{{number:{b}}})")
    });
    let query = format!(
        "{{raindexTokens(first:{PAGE},orderBy:id,orderDirection:asc,{at}where:{{id_gt:\"{after}\"}}){{id raindex token}} {meta}{{block{{number}} hasIndexingErrors}}}}"
    );
    json!({ "query": query }).to_string()
}

/// One discovered pair, lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub id: String,
    pub raindex: String,
    pub token: String,
}

/// One page's pairs and the block it was answered at.
pub fn parse_page(body: &[u8]) -> Result<(Vec<Pair>, u64), String> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| "the tokens subgraph answered with something that is not JSON".to_string())?;
    if let Some(errors) = v.get("errors").and_then(Value::as_array) {
        let msg = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("the tokens subgraph refused the query: {msg}"));
    }
    let malformed = || "the tokens subgraph's answer is not a pair list".to_string();
    let data = v.get("data").ok_or_else(malformed)?;
    let meta = &data["_meta"];
    match meta["hasIndexingErrors"].as_bool() {
        Some(false) => {}
        Some(true) => {
            return Err(
                "the tokens subgraph has indexing errors, so its token list is incomplete"
                    .to_string(),
            )
        }
        None => return Err(malformed()),
    }
    let block = meta["block"]["number"].as_u64().ok_or_else(malformed)?;
    let pairs = data["raindexTokens"]
        .as_array()
        .ok_or_else(malformed)?
        .iter()
        .map(|p| {
            let field = |k: &str| p[k].as_str().map(str::to_lowercase);
            match (field("id"), field("raindex"), field("token")) {
                (Some(id), Some(raindex), Some(token))
                    if is_address(&raindex) && is_address(&token) =>
                {
                    Ok(Pair { id, raindex, token })
                }
                _ => Err(malformed()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((pairs, block))
}

/// Every pair, paging by id at one block. `post` sends a query body.
pub fn discover(post: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<(Vec<Pair>, u64), String> {
    let mut pairs: Vec<Pair> = Vec::new();
    let mut block: Option<u64> = None;
    loop {
        let after = pairs.last().map_or("0x", |p| p.id.as_str()).to_string();
        let body = post(&page_query(&after, block))
            .ok_or_else(|| "the tokens subgraph did not answer".to_string())?;
        let (page, at) = parse_page(&body)?;
        if block.is_some_and(|b| b != at) {
            return Err("the tokens subgraph answered pages at different blocks".to_string());
        }
        let ascending = page.windows(2).all(|w| w[0].id < w[1].id)
            && page
                .first()
                .is_none_or(|f| pairs.last().is_none_or(|l| f.id > l.id));
        if !ascending {
            return Err("the tokens subgraph's pages are not in id order".to_string());
        }
        let full = page.len() == PAGE;
        pairs.extend(page);
        block = Some(at);
        if !full {
            return Ok((pairs, at));
        }
    }
}

/// `raw` (base units) as a decimal string with `decimals` places, trailing
/// zeros trimmed.
pub fn format_units(raw: &str, decimals: u8) -> Option<String> {
    if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let d = decimals as usize;
    let padded = format!("{raw:0>width$}", width = d + 1);
    let (int, frac) = padded.split_at(padded.len() - d);
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    Some(if frac.is_empty() {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    })
}

/// A network's pairs and the subgraph block they were read at.
pub type PairsFn<'a> = dyn Fn(&str) -> Result<(Vec<Pair>, u64), String> + 'a;
/// `eth_call`s on a network at a block (`None` = latest), one slot each.
pub type CallsFn<'a> = dyn Fn(&str, &[(String, String)], Option<u64>) -> Vec<Option<String>> + 'a;
/// USD price per lowercased token on a network.
pub type PricesFn<'a> = dyn Fn(&str, &[String]) -> Result<BTreeMap<String, f64>, String> + 'a;

/// The reads one scan makes, injected so the assembly is testable offline.
pub struct Reads<'a> {
    /// A network's pairs and the subgraph block they were read at.
    pub pairs: &'a PairsFn<'a>,
    /// A network's chain head; `Err` when the scanner has no RPC for it or it
    /// did not answer.
    pub head: &'a dyn Fn(&str) -> Result<u64, String>,
    /// `eth_call`s on a network at a block (`None` = latest), one slot each.
    pub calls: &'a CallsFn<'a>,
    /// USD price per lowercased token on a network; `Err` with the reason when
    /// the source cannot price that network at all.
    pub prices: &'a PricesFn<'a>,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// One network's section.
fn network_section(n: &Network, reads: &Reads) -> Value {
    let unreadable = |reason: String| {
        json!({
            "network": n.name,
            "error": reason,
            "raindexes": n.raindexes.iter().map(|r| json!({ "address": r })).collect::<Vec<_>>(),
        })
    };
    let (pairs, subgraph_block) = match (reads.pairs)(&n.name) {
        Ok(p) => p,
        Err(e) => return unreadable(e),
    };
    let block = match (reads.head)(&n.name) {
        Ok(b) => b,
        Err(e) => return unreadable(e),
    };
    // Only the raindexes this network records; a pair for any other address
    // is not this dashboard's to value.
    let recorded: BTreeSet<&str> = n.raindexes.iter().map(String::as_str).collect();
    let pairs: Vec<&Pair> = pairs
        .iter()
        .filter(|p| recorded.contains(p.raindex.as_str()))
        .collect();
    let tokens: Vec<String> = pairs
        .iter()
        .map(|p| p.token.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let balance_calls: Vec<(String, String)> = pairs
        .iter()
        .map(|p| {
            (
                p.token.clone(),
                crate::rpc::balance_of_calldata(&p.raindex).expect("raindex is an address"),
            )
        })
        .collect();
    let balances = (reads.calls)(&n.name, &balance_calls, Some(block));
    let meta_calls: Vec<(String, String)> = tokens
        .iter()
        .flat_map(|t| {
            [
                (t.clone(), crate::rpc::decimals_calldata()),
                (t.clone(), crate::rpc::symbol_calldata()),
            ]
        })
        .collect();
    let metas = (reads.calls)(&n.name, &meta_calls, None);
    let meta: BTreeMap<&str, (Option<u8>, Option<String>)> = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let decimals = metas
                .get(2 * i)
                .and_then(|h| h.as_deref())
                .and_then(crate::rpc::decode_u8);
            let symbol = metas
                .get(2 * i + 1)
                .and_then(|h| h.as_deref())
                .and_then(crate::rpc::decode_string);
            (t.as_str(), (decimals, symbol))
        })
        .collect();
    let prices = (reads.prices)(&n.name, &tokens);

    let mut by_raindex: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut usd_by_raindex: BTreeMap<&str, (f64, u64)> = BTreeMap::new();
    for (i, p) in pairs.iter().enumerate() {
        let (decimals, symbol) = meta.get(p.token.as_str()).cloned().unwrap_or((None, None));
        let mut row = json!({ "token": p.token });
        if let Some(s) = &symbol {
            row["symbol"] = json!(s);
        }
        let balance = balances
            .get(i)
            .and_then(|h| h.as_deref())
            .and_then(crate::rpc::decode_u256_decimal);
        let entry = usd_by_raindex.entry(p.raindex.as_str()).or_insert((0.0, 0));
        match (balance, decimals) {
            (None, _) => {
                row["error"] = json!("balanceOf did not answer");
                entry.1 += 1;
            }
            (Some(raw), None) => {
                row["balance"] = json!(raw);
                if raw != "0" {
                    row["error"] = json!("decimals() did not answer, so the amount is unknown");
                    entry.1 += 1;
                }
            }
            (Some(raw), Some(d)) => {
                let amount = format_units(&raw, d).expect("decoded uint256 is digits");
                row["balance"] = json!(raw);
                row["decimals"] = json!(d);
                row["amount"] = json!(amount);
                let held = raw != "0";
                match &prices {
                    Ok(ps) => match ps.get(&p.token) {
                        Some(price) => {
                            let usd = amount.parse::<f64>().unwrap_or(0.0) * price;
                            row["priceUsd"] = json!(price);
                            row["usd"] = json!(round2(usd));
                            entry.0 += usd;
                        }
                        None => {
                            row["unpriced"] = json!("the price source has no price for this token");
                            if held {
                                entry.1 += 1;
                            }
                        }
                    },
                    Err(e) => {
                        row["unpriced"] = json!(e);
                        if held {
                            entry.1 += 1;
                        }
                    }
                }
            }
        }
        by_raindex.entry(p.raindex.as_str()).or_default().push(row);
    }

    let mut usd = 0.0;
    let mut excluded = 0;
    let raindexes: Vec<Value> = n
        .raindexes
        .iter()
        .map(|r| {
            let (u, x) = usd_by_raindex.get(r.as_str()).copied().unwrap_or((0.0, 0));
            usd += u;
            excluded += x;
            json!({
                "address": r,
                "usd": round2(u),
                "excludedTokens": x,
                "tokens": by_raindex.remove(r.as_str()).unwrap_or_default(),
            })
        })
        .collect();
    json!({
        "network": n.name,
        "block": block,
        "subgraphBlock": subgraph_block,
        "usd": round2(usd),
        "excludedTokens": excluded,
        "raindexes": raindexes,
    })
}

/// The `dexTvl` block of `health.json`.
pub fn build(networks: Result<Vec<Network>, String>, reads: &Reads) -> Value {
    let networks = match networks {
        Ok(n) => n,
        Err(e) => return json!({ "priceSource": PRICE_SOURCE, "error": e }),
    };
    let sections: Vec<Value> = networks.iter().map(|n| network_section(n, reads)).collect();
    let usd: f64 = sections.iter().filter_map(|s| s["usd"].as_f64()).sum();
    let excluded: u64 = sections
        .iter()
        .filter_map(|s| s["excludedTokens"].as_u64())
        .sum();
    let unreadable = sections.iter().filter(|s| s.get("error").is_some()).count();
    json!({
        "priceSource": PRICE_SOURCE,
        "usd": round2(usd),
        "excludedTokens": excluded,
        "unreadableNetworks": unreadable,
        "networks": sections,
    })
}

/// The DefiLlama chain slug for a graph network; `None` for one it cannot
/// price (a testnet, or a chain it does not list).
pub fn llama_chain(network: &str) -> Option<&'static str> {
    match network {
        "arbitrum-one" => Some("arbitrum"),
        "base" => Some("base"),
        "bsc" => Some("bsc"),
        "flare" => Some("flare"),
        "hyperevm" => Some("hyperliquid"),
        "linea" => Some("linea"),
        "mainnet" => Some("ethereum"),
        "matic" => Some("polygon"),
        _ => None,
    }
}

/// The DefiLlama `prices/current` URL for `tokens` on `chain`.
pub fn llama_url(chain: &str, tokens: &[String]) -> String {
    let coins = tokens
        .iter()
        .map(|t| format!("{chain}:{t}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("https://coins.llama.fi/prices/current/{coins}")
}

/// Prices from one DefiLlama answer, keyed by lowercased token.
pub fn parse_llama(chain: &str, body: &[u8]) -> Result<BTreeMap<String, f64>, String> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| "the price source answered with something that is not JSON".to_string())?;
    let coins = v["coins"]
        .as_object()
        .ok_or_else(|| "the price source's answer has no coins".to_string())?;
    let prefix = format!("{chain}:");
    Ok(coins
        .iter()
        .filter_map(|(k, c)| {
            let token = k.strip_prefix(&prefix)?.to_lowercase();
            let price = c["price"].as_f64().filter(|p| p.is_finite() && *p >= 0.0)?;
            Some((token, price))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const R1: &str = "0x00000000000000000000000000000000000000a1";
    const R2: &str = "0x00000000000000000000000000000000000000a2";
    const T1: &str = "0x0000000000000000000000000000000000000011";
    const T2: &str = "0x0000000000000000000000000000000000000022";

    fn word(n: u128) -> String {
        format!("0x{n:064x}")
    }

    fn string_return(s: &str) -> String {
        let mut hex = format!("0x{:064x}{:064x}", 32, s.len());
        let mut bytes = s.as_bytes().to_vec();
        bytes.resize(s.len().div_ceil(32) * 32, 0);
        for b in bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        hex
    }

    #[test]
    fn a_manifest_yields_its_one_network_and_every_raindex() {
        let yaml = format!(
            "dataSources:\n  - kind: ethereum/contract\n    network: linea\n    source:\n      address: \"{}\"\n  - network: linea\n    source:\n      address: \"{}\"\n",
            R1.to_uppercase().replace("0X", "0x"),
            R2
        );
        assert_eq!(
            parse_manifest(&yaml),
            Some(Network {
                name: "linea".into(),
                raindexes: vec![R1.into(), R2.into()]
            })
        );
    }

    #[test]
    fn a_manifest_with_two_networks_or_a_bad_address_or_none_is_refused() {
        assert_eq!(
            parse_manifest(&format!("network: a\naddress: \"{R1}\"\nnetwork: b\n")),
            None
        );
        assert_eq!(parse_manifest("network: a\naddress: \"0x12\"\n"), None);
        assert_eq!(parse_manifest("network: a\n"), None);
    }

    #[test]
    fn the_tokens_source_is_the_matrix_entry_for_tokens_subgraph() {
        let wf = "    strategy:\n      matrix:\n        include:\n          - name: raindex\n            dir: subgraph\n          - name: raindex-tokens\n            dir: tokens-subgraph\n    env:\n      ORMI_QUERY_BASE: https://q.example/api/public/x/\n";
        let src = parse_tokens_source(wf).unwrap();
        assert_eq!(src.name, "raindex-tokens");
        assert_eq!(src.query_base, "https://q.example/api/public/x");
        assert_eq!(
            parse_tokens_source(&wf.replace("https://", "http://")),
            None
        );
        assert_eq!(
            parse_tokens_source(&wf.replace("tokens-subgraph", "other")),
            None
        );
    }

    #[test]
    fn format_units_places_the_point_and_trims() {
        assert_eq!(format_units("1500000", 6).as_deref(), Some("1.5"));
        assert_eq!(format_units("5", 6).as_deref(), Some("0.000005"));
        assert_eq!(format_units("0", 18).as_deref(), Some("0"));
        assert_eq!(format_units("1000", 0).as_deref(), Some("1000"));
        assert_eq!(format_units("12x", 2), None);
    }

    fn page_body(pairs: &[(&str, &str)], block: u64) -> Vec<u8> {
        let rows: Vec<_> = pairs
            .iter()
            .map(|(r, t)| json!({ "id": format!("{r}{}", &t[2..]), "raindex": r, "token": t }))
            .collect();
        json!({ "data": { "raindexTokens": rows, "_meta": { "block": { "number": block }, "hasIndexingErrors": false } } })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn discover_reads_every_pair_at_one_block() {
        let calls = RefCell::new(Vec::new());
        let post = |q: &str| {
            calls.borrow_mut().push(q.to_string());
            Some(page_body(&[(R1, T1), (R1, T2)], 77))
        };
        let (pairs, block) = discover(&post).unwrap();
        assert_eq!(block, 77);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1].token, T2);
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn an_unusable_subgraph_answer_is_a_reason_never_an_empty_list() {
        assert!(parse_page(b"<html>").unwrap_err().contains("not JSON"));
        assert!(parse_page(br#"{"errors":[{"message":"nope"}]}"#)
            .unwrap_err()
            .contains("nope"));
        assert!(parse_page(
            br#"{"data":{"raindexTokens":[],"_meta":{"block":{"number":1},"hasIndexingErrors":true}}}"#
        )
        .unwrap_err()
        .contains("indexing errors"));
        assert!(discover(&|_| None).unwrap_err().contains("did not answer"));
    }

    fn reads_fixture<'a>(
        pairs: &'a PairsFn<'a>,
        prices: &'a PricesFn<'a>,
        calls: &'a CallsFn<'a>,
    ) -> Reads<'a> {
        Reads {
            pairs,
            head: &|_| Ok(1000),
            calls,
            prices,
        }
    }

    fn pair(r: &str, t: &str) -> Pair {
        Pair {
            id: format!("{r}{}", &t[2..]),
            raindex: r.into(),
            token: t.into(),
        }
    }

    /// Balances: R1 holds 1.5 T1 (6 dp) and 2 T2 (18 dp); R2 holds 0 T1.
    fn calls_fixture(_: &str, calls: &[(String, String)], at: Option<u64>) -> Vec<Option<String>> {
        calls
            .iter()
            .map(|(to, data)| {
                let r1 = crate::rpc::balance_of_calldata(R1).unwrap();
                let r2 = crate::rpc::balance_of_calldata(R2).unwrap();
                if *data == r1 || *data == r2 {
                    assert_eq!(at, Some(1000), "balances are read at the stated block");
                }
                Some(match (to.as_str(), data.as_str()) {
                    (T1, d) if d == r1 => word(1_500_000),
                    (T2, d) if d == r1 => word(2_000_000_000_000_000_000),
                    (T1, d) if d == r2 => word(0),
                    (T1, d) if d == crate::rpc::decimals_calldata() => word(6),
                    (T2, d) if d == crate::rpc::decimals_calldata() => word(18),
                    (T1, _) => string_return("USDC"),
                    (T2, _) => string_return("WETH"),
                    _ => return None,
                })
            })
            .collect()
    }

    fn net() -> Network {
        Network {
            name: "base".into(),
            raindexes: vec![R1.into(), R2.into()],
        }
    }

    #[test]
    fn totals_sum_priced_holdings_by_raindex_network_and_org() {
        let pairs = |_: &str| Ok((vec![pair(R1, T1), pair(R1, T2), pair(R2, T1)], 990));
        let prices = |_: &str, _: &[String]| {
            Ok(BTreeMap::from([
                (T1.to_string(), 1.0),
                (T2.to_string(), 3000.0),
            ]))
        };
        let doc = build(
            Ok(vec![net()]),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(doc["usd"], json!(6001.5));
        assert_eq!(doc["excludedTokens"], json!(0));
        let n = &doc["networks"][0];
        assert_eq!(n["block"], json!(1000));
        assert_eq!(n["subgraphBlock"], json!(990));
        assert_eq!(n["raindexes"][0]["usd"], json!(6001.5));
        assert_eq!(n["raindexes"][0]["tokens"][0]["amount"], json!("1.5"));
        assert_eq!(n["raindexes"][0]["tokens"][0]["symbol"], json!("USDC"));
        assert_eq!(n["raindexes"][0]["tokens"][1]["amount"], json!("2"));
        assert_eq!(n["raindexes"][1]["usd"], json!(0.0));
    }

    #[test]
    fn an_unpriced_held_token_is_excluded_and_counted_never_zero() {
        let pairs = |_: &str| Ok((vec![pair(R1, T1), pair(R1, T2), pair(R2, T1)], 990));
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::from([(T1.to_string(), 1.0)]));
        let doc = build(
            Ok(vec![net()]),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(doc["usd"], json!(1.5));
        assert_eq!(doc["excludedTokens"], json!(1));
        let weth = &doc["networks"][0]["raindexes"][0]["tokens"][1];
        assert!(weth.get("usd").is_none());
        assert!(weth["unpriced"].as_str().unwrap().contains("no price"));
    }

    #[test]
    fn an_unpriceable_network_counts_only_held_tokens_as_excluded() {
        let pairs = |_: &str| Ok((vec![pair(R1, T1), pair(R2, T1)], 990));
        let prices = |_: &str, _: &[String]| Err("testnet".to_string());
        let doc = build(
            Ok(vec![net()]),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(
            doc["excludedTokens"],
            json!(1),
            "R2's zero balance is not excluded value"
        );
        assert_eq!(
            doc["networks"][0]["raindexes"][0]["tokens"][0]["unpriced"],
            json!("testnet")
        );
    }

    #[test]
    fn an_unread_balance_is_a_reason_and_excluded() {
        let pairs = |_: &str| Ok((vec![pair(R1, T1)], 990));
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::from([(T1.to_string(), 1.0)]));
        let calls = |n: &str, c: &[(String, String)], at: Option<u64>| {
            let mut out = calls_fixture(n, c, at);
            if at.is_some() {
                out.iter_mut().for_each(|s| *s = None);
            }
            out
        };
        let doc = build(Ok(vec![net()]), &reads_fixture(&pairs, &prices, &calls));
        assert_eq!(doc["usd"], json!(0.0));
        assert_eq!(doc["excludedTokens"], json!(1));
        assert_eq!(
            doc["networks"][0]["raindexes"][0]["tokens"][0]["error"],
            json!("balanceOf did not answer")
        );
    }

    #[test]
    fn an_unreadable_network_keeps_its_raindexes_and_its_reason() {
        let pairs = |_: &str| Err("the tokens subgraph did not answer".to_string());
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::new());
        let doc = build(
            Ok(vec![net()]),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(doc["unreadableNetworks"], json!(1));
        let n = &doc["networks"][0];
        assert_eq!(n["error"], json!("the tokens subgraph did not answer"));
        assert_eq!(n["raindexes"].as_array().unwrap().len(), 2);
        assert!(n.get("usd").is_none());
    }

    #[test]
    fn a_pair_for_an_unrecorded_raindex_is_ignored() {
        let other = "0x00000000000000000000000000000000000000ff";
        let pairs = move |_: &str| Ok((vec![pair(other, T1), pair(R1, T1)], 990));
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::from([(T1.to_string(), 1.0)]));
        let doc = build(
            Ok(vec![net()]),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(doc["usd"], json!(1.5));
        // Not read, not counted, not listed: the pair is not this section's.
        assert_eq!(doc["excludedTokens"], json!(0));
        let listed: Vec<&str> = doc["networks"][0]["raindexes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["address"].as_str().unwrap())
            .collect();
        assert_eq!(listed, vec![R1, R2]);
        // ...and never read: no RPC is spent on a raindex the record does not name.
        let asked = RefCell::new(Vec::<String>::new());
        let calls = |n: &str, c: &[(String, String)], at: Option<u64>| {
            asked
                .borrow_mut()
                .extend(c.iter().map(|(_, data)| data.clone()));
            calls_fixture(n, c, at)
        };
        build(Ok(vec![net()]), &reads_fixture(&pairs, &prices, &calls));
        let other_balance = crate::rpc::balance_of_calldata(other).unwrap();
        assert!(!asked.borrow().contains(&other_balance));
    }

    #[test]
    fn a_held_token_with_unread_decimals_is_a_reason_and_excluded() {
        let pairs = |_: &str| Ok((vec![pair(R1, T1)], 990));
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::from([(T1.to_string(), 1.0)]));
        let calls = |n: &str, c: &[(String, String)], at: Option<u64>| {
            let mut out = calls_fixture(n, c, at);
            if at.is_none() {
                // decimals() is the first of each token's two metadata calls.
                out[0] = None;
            }
            out
        };
        let doc = build(Ok(vec![net()]), &reads_fixture(&pairs, &prices, &calls));
        assert_eq!(doc["usd"], json!(0.0));
        assert_eq!(doc["excludedTokens"], json!(1));
        let row = &doc["networks"][0]["raindexes"][0]["tokens"][0];
        assert_eq!(row["balance"], json!("1500000"));
        assert!(row["error"].as_str().unwrap().contains("decimals()"));
    }

    #[test]
    fn discover_pages_past_a_full_page_at_the_first_page_block() {
        let first: Vec<(String, String)> = (0..PAGE)
            .map(|i| (R1.to_string(), format!("0x{:040x}", i + 1)))
            .collect();
        let first_refs: Vec<(&str, &str)> = first
            .iter()
            .map(|(r, t)| (r.as_str(), t.as_str()))
            .collect();
        let last_token = format!("0x{:040x}", PAGE + 1);
        let queries = RefCell::new(Vec::<String>::new());
        let post = |q: &str| {
            queries.borrow_mut().push(q.to_string());
            Some(if queries.borrow().len() == 1 {
                page_body(&first_refs, 50)
            } else {
                page_body(&[(R1, last_token.as_str())], 50)
            })
        };
        let (pairs, block) = discover(&post).unwrap();
        assert_eq!(pairs.len(), PAGE + 1);
        assert_eq!(block, 50);
        assert_eq!(queries.borrow().len(), 2);
        assert!(queries.borrow()[1].contains("block:{number:50}"));
    }

    #[test]
    fn discover_refuses_pages_at_different_blocks_or_out_of_order() {
        let full: Vec<(String, String)> = (0..PAGE)
            .map(|i| (R1.to_string(), format!("0x{:040x}", i + 1)))
            .collect();
        let full_refs: Vec<(&str, &str)> =
            full.iter().map(|(r, t)| (r.as_str(), t.as_str())).collect();
        let n = RefCell::new(0);
        let drift = |_: &str| {
            *n.borrow_mut() += 1;
            Some(if *n.borrow() == 1 {
                page_body(&full_refs, 50)
            } else {
                page_body(&[(R1, "0x00000000000000000000000000000000000fffff")], 51)
            })
        };
        assert!(discover(&drift).unwrap_err().contains("different blocks"));
        let unordered = |_: &str| Some(page_body(&[(R1, T2), (R1, T1)], 50));
        assert!(discover(&unordered).unwrap_err().contains("id order"));
    }

    #[test]
    fn a_negative_or_non_finite_price_is_no_price() {
        let body = br#"{"coins":{"base:0x0000000000000000000000000000000000000001":{"price":-1.0},"base:0x0000000000000000000000000000000000000002":{"price":1.0}}}"#;
        let p = parse_llama("base", body).unwrap();
        assert_eq!(p.len(), 1);
        assert!(p.contains_key("0x0000000000000000000000000000000000000002"));
    }

    #[test]
    fn no_manifests_is_a_reason_for_the_whole_block() {
        let pairs = |_: &str| Ok((vec![], 0));
        let prices = |_: &str, _: &[String]| Ok(BTreeMap::new());
        let doc = build(
            Err("raindex.deploy has no tokens-subgraph manifests".into()),
            &reads_fixture(&pairs, &prices, &calls_fixture),
        );
        assert_eq!(
            doc["error"],
            json!("raindex.deploy has no tokens-subgraph manifests")
        );
        assert!(doc.get("usd").is_none());
    }

    #[test]
    fn llama_prices_are_keyed_by_lowercased_token_and_drop_bad_prices() {
        let body = br#"{"coins":{"base:0xAbC0000000000000000000000000000000000001":{"price":2.5},"base:0x0000000000000000000000000000000000000002":{"price":"x"},"arbitrum:0x0000000000000000000000000000000000000003":{"price":1}}}"#;
        let p = parse_llama("base", body).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p["0xabc0000000000000000000000000000000000001"], 2.5);
        assert!(parse_llama("base", b"nope").is_err());
        assert_eq!(
            llama_url("base", &[T1.into(), T2.into()]),
            format!("https://coins.llama.fi/prices/current/base:{T1},base:{T2}")
        );
        assert_eq!(llama_chain("base-sepolia"), None);
        assert_eq!(llama_chain("matic"), Some("polygon"));
    }
}
