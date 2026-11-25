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
        .ok_or_else(|| {
            format!("the deploy repo's foundry.toml has no [etherscan] entry for {network}")
        })?;
    Ok(Explorer {
        url: entry
            .get("url")
            .and_then(|u| u.as_str())
            .map(str::to_string),
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
pub fn api_url(
    e: &Explorer,
    live_chain_id: impl FnOnce() -> Option<u64>,
) -> Result<String, String> {
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
        return (
            "unknown",
            None,
            Some("the explorer did not answer".to_string()),
        );
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (
            "unknown",
            None,
            Some("the explorer answer is not JSON".to_string()),
        );
    };
    match v["result"]
        .get(0)
        .and_then(|r| r["SourceCode"].as_str().map(|s| (r, s)))
    {
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
            (
                "unknown",
                None,
                Some(format!("the explorer answered: {why}")),
            )
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
                if component.is_empty() {
                    vec![]
                } else {
                    vec![component]
                },
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
    let (verified, unverified, unknown) =
        (count("verified"), count("unverified"), count("unknown"));
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

#[cfg(test)]
mod tests {
    use super::*;

    const FOUNDRY: &str = r#"
[profile.default]
src = "src"

[etherscan]
base = { key = "${CI_DEPLOY_BASE_ETHERSCAN_API_KEY}" }
bsc = { key = "${CI_DEPLOY_BSC_ETHERSCAN_API_KEY}", chain = 56 }
robinhood = { key = "${CI_DEPLOY_ROBINHOOD_ETHERSCAN_API_KEY}", chain = 4663, url = "https://robinhoodchain.blockscout.com/api" }
zerog = { key = "${CI_DEPLOY_ZEROG_ETHERSCAN_API_KEY}", chain = 16661, url = "https://chainscan.0g.ai/open/api" }
"#;

    const CHAINSCAN_UNVERIFIED: &str = r#"{"status":"1","message":"OK","result":[{"SourceCode":"","ABI":"Contract source code not verified","ContractName":"","CompilerVersion":"","CompilerType":"","OptimizationUsed":"","Runs":"","ConstructorArguments":"","EVMVersion":"Default","Library":"","ContractFileName":"","LicenseType":"Unknown","Proxy":"0","Implementation":"","SwarmSource":"","SimilarMatch":""}]}"#;

    fn only(var: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |v| (v == var).then(|| value.to_string())
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn resolve_key_reads_a_set_variable() {
        assert_eq!(
            resolve_key("${KEY}", only("KEY", "abc")),
            Some("abc".to_string())
        );
    }

    #[test]
    fn resolve_key_is_none_for_an_unset_or_empty_variable() {
        assert_eq!(resolve_key("${KEY}", only("OTHER", "abc")), None);
        assert_eq!(resolve_key("${KEY}", only("KEY", "")), None);
    }

    #[test]
    fn resolve_key_returns_a_literal_as_itself() {
        assert_eq!(
            resolve_key("literal", only("literal", "wrong")),
            Some("literal".to_string())
        );
    }

    #[test]
    fn explorer_reads_a_keyless_url_entry() {
        let base = explorer(
            FOUNDRY,
            "base",
            only("CI_DEPLOY_BASE_ETHERSCAN_API_KEY", "basekey"),
        )
        .unwrap();
        assert_eq!(
            base,
            Explorer {
                url: None,
                chain: None,
                key: Some("basekey".to_string()),
            }
        );
        assert_eq!(explorer(FOUNDRY, "base", no_env).unwrap().key, None);
    }

    #[test]
    fn explorer_reads_a_pinned_chain() {
        assert_eq!(
            explorer(FOUNDRY, "bsc", no_env).unwrap(),
            Explorer {
                url: None,
                chain: Some(56),
                key: None,
            }
        );
    }

    #[test]
    fn explorer_reads_a_url_entry() {
        assert_eq!(
            explorer(
                FOUNDRY,
                "robinhood",
                only("CI_DEPLOY_ROBINHOOD_ETHERSCAN_API_KEY", "rk"),
            )
            .unwrap(),
            Explorer {
                url: Some("https://robinhoodchain.blockscout.com/api".to_string()),
                chain: Some(4663),
                key: Some("rk".to_string()),
            }
        );
        assert_eq!(
            explorer(FOUNDRY, "zerog", no_env).unwrap(),
            Explorer {
                url: Some("https://chainscan.0g.ai/open/api".to_string()),
                chain: Some(16661),
                key: None,
            }
        );
    }

    #[test]
    fn explorer_errs_without_an_etherscan_table() {
        let e = explorer("[profile.default]\nsrc = \"src\"\n", "base", no_env).unwrap_err();
        assert!(e.contains("no [etherscan] table"), "{e}");
    }

    #[test]
    fn explorer_errs_without_the_networks_entry() {
        let e = explorer(FOUNDRY, "ethereum", no_env).unwrap_err();
        assert!(e.contains("no [etherscan] entry for ethereum"), "{e}");
    }

    #[test]
    fn explorer_errs_on_unparseable_toml() {
        let e = explorer("[etherscan\nbase = {", "base", no_env).unwrap_err();
        assert!(e.contains("does not parse"), "{e}");
    }

    #[test]
    fn api_url_prefers_the_entry_url() {
        let e = Explorer {
            url: Some("https://chainscan.0g.ai/open/api".to_string()),
            chain: Some(16661),
            key: None,
        };
        assert_eq!(
            api_url(&e, || Some(1)).unwrap(),
            "https://chainscan.0g.ai/open/api"
        );
    }

    #[test]
    fn api_url_uses_the_pinned_chain_before_the_live_one() {
        let e = Explorer {
            url: None,
            chain: Some(56),
            key: None,
        };
        assert_eq!(
            api_url(&e, || Some(1)).unwrap(),
            "https://api.etherscan.io/v2/api?chainid=56"
        );
    }

    #[test]
    fn api_url_falls_back_to_the_live_chain_id() {
        let e = Explorer {
            url: None,
            chain: None,
            key: None,
        };
        assert_eq!(
            api_url(&e, || Some(8453)).unwrap(),
            "https://api.etherscan.io/v2/api?chainid=8453"
        );
    }

    #[test]
    fn api_url_errs_with_no_url_chain_or_live_id() {
        let e = Explorer {
            url: None,
            chain: None,
            key: None,
        };
        assert!(api_url(&e, || None).is_err());
    }

    #[test]
    fn explorer_host_strips_scheme_and_query() {
        assert_eq!(
            explorer_host("https://api.etherscan.io/v2/api?chainid=56"),
            "api.etherscan.io/v2/api"
        );
        assert_eq!(
            explorer_host("https://chainscan.0g.ai/open/api"),
            "chainscan.0g.ai/open/api"
        );
        assert_eq!(explorer_host("example.com/api#x"), "example.com/api");
    }

    #[test]
    fn source_url_starts_the_query_when_the_api_has_none() {
        assert_eq!(
            source_url("https://chainscan.0g.ai/open/api", "0xabc", None),
            "https://chainscan.0g.ai/open/api?module=contract&action=getsourcecode&address=0xabc"
        );
    }

    #[test]
    fn source_url_extends_an_existing_query_and_appends_the_key() {
        assert_eq!(
            source_url(
                "https://api.etherscan.io/v2/api?chainid=56",
                "0xabc",
                Some("K")
            ),
            "https://api.etherscan.io/v2/api?chainid=56&module=contract&action=getsourcecode&address=0xabc&apikey=K"
        );
    }

    #[test]
    fn source_status_verified_with_contract_name() {
        let body = br#"{"status":"1","message":"OK","result":[{"SourceCode":"pragma solidity =0.8.25;","ContractName":"OffchainAssetReceiptVault"}]}"#;
        assert_eq!(
            source_status(Some(body)),
            (
                "verified",
                Some("OffchainAssetReceiptVault".to_string()),
                None
            )
        );
    }

    #[test]
    fn source_status_unverified_on_empty_source() {
        assert_eq!(
            source_status(Some(CHAINSCAN_UNVERIFIED.as_bytes())),
            ("unverified", None, None)
        );
    }

    #[test]
    fn source_status_unknown_without_a_result() {
        let (s, n, r) = source_status(None);
        assert_eq!((s, n), ("unknown", None));
        assert!(r.is_some());

        let (s, n, r) = source_status(Some(b"<html>502 Bad Gateway</html>"));
        assert_eq!((s, n), ("unknown", None));
        assert!(r.is_some());

        assert_eq!(
            source_status(Some(
                br#"{"status":"0","message":"NOTOK","result":"Missing/Invalid API Key"}"#
            )),
            (
                "unknown",
                None,
                Some("the explorer answered: Missing/Invalid API Key".to_string())
            )
        );
    }

    #[test]
    fn contracts_dedup_case_insensitively_and_merge_components() {
        let infra = json!({
            "network": "base",
            "rows": [
                {"component": "timelock", "address": "0xAbC"},
                {"component": "timelock", "address": "0xabc"},
                {"component": "orchestrator", "address": "0xABC"},
                {"component": "euAuthoriser", "address": "0xdef"},
                {"component": "blank", "address": ""},
                {"component": "unread", "address": null},
            ],
        });
        let tokens = json!({
            "network": "base",
            "rows": [
                {"component": "token tMSTR", "address": "0xabc"},
                {"address": "0x123"},
            ],
        });
        let other_chain = json!({
            "network": "ethereum",
            "rows": [{"component": "timelock", "address": "0x999"}],
        });
        let unavailable = json!({
            "network": "base",
            "rpcHost": null,
            "unavailable": true,
            "reason": "no rpc",
        });
        assert_eq!(
            contracts(&[&infra, &other_chain, &unavailable, &tokens], "base"),
            vec![
                (
                    "0xAbC".to_string(),
                    vec![
                        "timelock".to_string(),
                        "orchestrator".to_string(),
                        "token tMSTR".to_string()
                    ]
                ),
                ("0xdef".to_string(), vec!["euAuthoriser".to_string()]),
                ("0x123".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn build_counts_each_status() {
        let verified = br#"{"result":[{"SourceCode":"x","ContractName":"A"}]}"#;
        let rows = vec![
            row("0x1", &["a".to_string()], Some(verified)),
            row("0x2", &[], Some(verified)),
            row("0x3", &[], Some(CHAINSCAN_UNVERIFIED.as_bytes())),
            row("0x4", &[], None),
        ];
        let d = build(
            "S01-Issuer",
            "st0x.deploy",
            "base",
            "api.etherscan.io/v2/api",
            rows,
        );
        assert_eq!(d["network"], "base");
        assert_eq!(d["explorer"], "api.etherscan.io/v2/api");
        assert_eq!(d["total"], 4);
        assert_eq!(d["verified"], 2);
        assert_eq!(d["unverified"], 1);
        assert_eq!(d["unknown"], 1);
        assert_eq!(d["rows"][0]["address"], "0x1");
        assert_eq!(d["rows"][0]["components"], json!(["a"]));
        assert_eq!(d["rows"][0]["contractName"], "A");
        assert_eq!(d["rows"][3]["status"], "unknown");
    }
}
