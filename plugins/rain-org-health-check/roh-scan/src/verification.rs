//! Explorer source verification of every contract the deployments page reports,
//! per chain. The explorer for a chain is the deploy repo's `foundry.toml`
//! `[etherscan]` entry named for that chain; each answers
//! `module=contract&action=getsourcecode`. Parsing and classification live here;
//! the HTTP and RPC reads are in main.

use serde_json::json;

/// Etherscan V2, which foundry uses for an `[etherscan]` entry without a `url`.
pub const ETHERSCAN_V2: &str = "https://api.etherscan.io/v2/api";

/// One `[etherscan]` entry, its `key` already resolved from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explorer {
    pub url: Option<String>,
    pub chain: Option<u64>,
    pub key: Option<String>,
}

/// `${VAR}` resolved through `env`; a literal key is itself. `None` when the
/// variable is unset or empty.
pub fn resolve_key(raw: &str, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let key = match raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        Some(var) => env(var)?,
        None => raw.to_string(),
    };
    (!key.is_empty()).then_some(key)
}

/// The `[etherscan]` entry named `network` in `foundry_toml`.
pub fn explorer(
    foundry_toml: &str,
    network: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Explorer, String> {
    let doc: toml::Table = foundry_toml
        .parse()
        .map_err(|e| format!("the deploy repo's foundry.toml does not parse: {e}"))?;
    let entry = doc
        .get("etherscan")
        .and_then(|t| t.as_table())
        .ok_or("the deploy repo's foundry.toml has no [etherscan] table")?
        .get(network)
        .and_then(|e| e.as_table())
        .ok_or_else(|| format!("the deploy repo's foundry.toml has no [etherscan] entry for {network}"))?;
    Ok(Explorer {
        url: entry.get("url").and_then(|u| u.as_str()).map(str::to_string),
        chain: entry
            .get("chain")
            .and_then(|c| c.as_integer())
            .and_then(|c| u64::try_from(c).ok()),
        key: entry
            .get("key")
            .and_then(|k| k.as_str())
            .and_then(|k| resolve_key(k, env)),
    })
}

/// The API base the entry answers at: its `url`, else Etherscan V2 for its
/// `chain`, else Etherscan V2 for `live_chain_id`.
pub fn api_url(e: &Explorer, live_chain_id: impl FnOnce() -> Option<u64>) -> Result<String, String> {
    if let Some(u) = &e.url {
        return Ok(u.clone());
    }
    let id = e
        .chain
        .or_else(live_chain_id)
        .ok_or("the [etherscan] entry has no url or chain and eth_chainId did not answer")?;
    Ok(format!("{ETHERSCAN_V2}?chainid={id}"))
}

/// The explorer host shown beside a verdict: `api` without scheme or query.
pub fn explorer_host(api: &str) -> String {
    let rest = api.split_once("://").map_or(api, |(_, r)| r);
    rest.split(['?', '#']).next().unwrap_or(rest).to_string()
}

/// The `getsourcecode` request for `address` against `api`.
pub fn source_url(api: &str, address: &str, key: Option<&str>) -> String {
    let sep = if api.contains('?') { '&' } else { '?' };
    let mut url = format!("{api}{sep}module=contract&action=getsourcecode&address={address}");
    if let Some(k) = key {
        url.push_str("&apikey=");
        url.push_str(k);
    }
    url
}

/// A `getsourcecode` answer → (status, contract name, reason). `verified` when
/// the first result carries non-empty `SourceCode`, `unverified` when it is
/// empty, `unknown` when there was no answer or it carries no result object.
pub fn source_status(body: Option<&[u8]>) -> (&'static str, Option<String>, Option<String>) {
    let Some(body) = body else {
        return ("unknown", None, Some("the explorer did not answer".to_string()));
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return ("unknown", None, Some("the explorer answer is not JSON".to_string()));
    };
    match v["result"].get(0).and_then(|r| r["SourceCode"].as_str().map(|s| (r, s))) {
        Some((r, src)) => {
            let name = r["ContractName"]
                .as_str()
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            if src.is_empty() {
                ("unverified", name, None)
            } else {
                ("verified", name, None)
            }
        }
        None => {
            let why = v["result"]
                .as_str()
                .or_else(|| v["message"].as_str())
                .unwrap_or("no result");
            ("unknown", None, Some(format!("the explorer answered: {why}")))
        }
    }
}

/// Every distinct contract address the per-chain `docs` report for `network`,
/// case-insensitively, each with the components that name it, in first-seen order.
pub fn contracts(docs: &[&serde_json::Value], network: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for r in docs
        .iter()
        .filter(|d| d["network"] == network)
        .filter_map(|d| d["rows"].as_array())
        .flatten()
    {
        let Some(a) = r["address"].as_str().filter(|a| !a.is_empty()) else {
            continue;
        };
        let component = r["component"].as_str().unwrap_or("").to_string();
        match out.iter_mut().find(|(x, _)| x.eq_ignore_ascii_case(a)) {
            Some((_, cs)) => {
                if !component.is_empty() && !cs.contains(&component) {
                    cs.push(component);
                }
            }
            None => out.push((
                a.to_string(),
                if component.is_empty() { vec![] } else { vec![component] },
            )),
        }
    }
    out
}

/// One contract's verification row from its `getsourcecode` answer.
pub fn row(address: &str, components: &[String], body: Option<&[u8]>) -> serde_json::Value {
    let (status, name, reason) = source_status(body);
    json!({
        "address": address,
        "components": components,
        "contractName": name,
        "status": status,
        "reason": reason,
    })
}

/// Assemble one chain's `deploymentVerification` document.
pub fn build(
    org: &str,
    repo: &str,
    network: &str,
    explorer: &str,
    rows: Vec<serde_json::Value>,
) -> serde_json::Value {
    let count = |s: &str| rows.iter().filter(|r| r["status"] == s).count();
    let (verified, unverified, unknown) = (count("verified"), count("unverified"), count("unknown"));
    json!({
        "org": org,
        "repo": repo,
        "network": network,
        "explorer": explorer,
        "source": "foundry.toml",
        "total": rows.len(),
        "verified": verified,
        "unverified": unverified,
        "unknown": unknown,
        "rows": rows,
    })
}

/// A chain whose verification could not be read, with the reason.
pub fn unavailable(network: &str, reason: &str) -> serde_json::Value {
    json!({
        "network": network,
        "unavailable": true,
        "reason": reason,
    })
}
