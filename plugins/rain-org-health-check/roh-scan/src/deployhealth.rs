//! On-chain HEALTH of a pinned deployment suite: for each contract, is it
//! deployed at its pinned address, and does the live code match BOTH pins — the
//! exact `RUNTIME_CODE` bytes AND the `BYTECODE_HASH` keccak (EXTCODEHASH). Pure
//! parsing + comparison live here and are unit-tested; the RPC fetch is in main.
//!
//! Checking both cross-validates the two pins against each other and against the
//! chain: a `RUNTIME_CODE` that disagrees with its own `BYTECODE_HASH`, or either
//! disagreeing with the deployed code, is a finding.

use regex::Regex;
use serde_json::json;
use tiny_keccak::{Hasher, Keccak};

/// keccak256 of the bytes represented by `hex` (with or without `0x`), as a
/// lowercase `0x…` string. Ethereum's codehash is Keccak-256 (the pre-NIST
/// variant), which is what `Keccak::v256` computes. `None` if `hex` isn't valid
/// even-length hex.
pub fn keccak256_hex(hex: &str) -> Option<String> {
    let h = hex.strip_prefix("0x").unwrap_or(hex);
    if !h.len().is_multiple_of(2) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes: Vec<u8> = (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let mut k = Keccak::v256();
    k.update(&bytes);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    let mut s = String::with_capacity(66);
    s.push_str("0x");
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    Some(s)
}

/// Parse a `bytes constant NAME = hex"…";` payload (the declaration may wrap
/// across lines) → the lowercase hex, no `0x`. `None` if absent.
pub fn parse_hex_constant(src: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"\b{}\b\s*=\s*hex"([0-9a-fA-F]*)""#,
        regex::escape(name)
    ))
    .ok()?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_lowercase())
}

/// Parse a `bytes32 constant NAME = bytes32(0x…64…);` → lowercase `0x…`.
pub fn parse_bytes32_constant(src: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r"\b{}\b\s*=\s*bytes32\(\s*(0x[0-9a-fA-F]{{64}})\s*\)",
        regex::escape(name),
    ))
    .ok()?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_lowercase())
}

/// Health of one contract given its pins and the live `eth_getCode` result.
/// `onchain` is `Some("0x…")` (deployed), `Some("0x")` (no code), or `None` (the
/// RPC call failed). A contract is `healthy` only when BOTH the exact runtime
/// bytes AND the keccak codehash match their pins.
pub fn contract_health(
    name: &str,
    address: Option<String>,
    runtime_pin: Option<String>,
    hash_pin: Option<String>,
    onchain: Option<String>,
) -> serde_json::Value {
    let (status, code_match, hash_match): (&str, Option<bool>, Option<bool>) =
        match onchain.as_deref() {
            None => ("unknown", None, None),
            Some(code) => {
                let bare = code.strip_prefix("0x").unwrap_or(code).to_lowercase();
                if bare.is_empty() {
                    ("missing", Some(false), Some(false))
                } else {
                    let cm = runtime_pin.as_deref().map(|p| p.to_lowercase() == bare);
                    let hm = match (hash_pin.as_deref(), keccak256_hex(&bare)) {
                        (Some(p), Some(k)) => Some(p.to_lowercase() == k),
                        _ => None,
                    };
                    // A definite disagreement on either axis is a mismatch; both
                    // confirmed is healthy; otherwise a pin couldn't be read.
                    let status = if cm == Some(false) || hm == Some(false) {
                        "mismatch"
                    } else if cm == Some(true) && hm == Some(true) {
                        "healthy"
                    } else {
                        "unknown"
                    };
                    (status, cm, hm)
                }
            }
        };
    json!({
        "name": name,
        "address": address,
        "status": status,
        "codeMatch": code_match,
        "hashMatch": hash_match,
    })
}

/// Assemble the `deploymentHealth` document from the per-contract results,
/// sorted by name for stable output. `None` when there are no contracts (the
/// pinned-suite directory was absent or unreadable).
pub fn build_health(
    org: &str,
    repo: &str,
    version: &str,
    network: &str,
    rpc_host: &str,
    mut contracts: Vec<serde_json::Value>,
) -> Option<serde_json::Value> {
    if contracts.is_empty() {
        return None;
    }
    contracts.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let total = contracts.len();
    let healthy = contracts
        .iter()
        .filter(|c| c["status"] == "healthy")
        .count();
    Some(json!({
        "org": org,
        "repo": repo,
        "version": version,
        "network": network,
        "rpcHost": rpc_host,
        "total": total,
        "healthy": healthy,
        "contracts": contracts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak256_of_empty_is_the_known_vector() {
        assert_eq!(
            keccak256_hex("0x").unwrap(),
            "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert_eq!(keccak256_hex(""), keccak256_hex("0x"));
        assert_eq!(keccak256_hex("xyz"), None, "invalid hex");
        assert_eq!(keccak256_hex("0xabc"), None, "odd length");
    }

    #[test]
    fn parses_runtime_hex_and_bytecode_hash() {
        let src = r#"
            bytes32 constant BYTECODE_HASH = bytes32(0x2a67c52129df123456789012345678901234567890123456789012345678AAAA);
            bytes constant RUNTIME_CODE =
                hex"6080604052DEAD";
        "#;
        assert_eq!(
            parse_hex_constant(src, "RUNTIME_CODE"),
            Some("6080604052dead".to_string())
        );
        assert_eq!(
            parse_bytes32_constant(src, "BYTECODE_HASH").unwrap(),
            "0x2a67c52129df123456789012345678901234567890123456789012345678aaaa"
        );
        assert_eq!(parse_hex_constant(src, "NOPE"), None);
    }

    #[test]
    fn healthy_when_code_and_keccak_both_match() {
        let code = "6080604052";
        let hash = keccak256_hex(code).unwrap();
        let h = contract_health(
            "Foo",
            Some("0xabc".into()),
            Some(code.to_string()),
            Some(hash),
            Some(format!("0x{code}")),
        );
        assert_eq!(h["status"], "healthy");
        assert_eq!(h["codeMatch"], true);
        assert_eq!(h["hashMatch"], true);
    }

    #[test]
    fn missing_when_no_code_on_chain() {
        let h = contract_health(
            "Foo",
            Some("0xabc".into()),
            Some("6080".into()),
            Some("0xdeadbeef".into()),
            Some("0x".into()),
        );
        assert_eq!(h["status"], "missing");
        assert_eq!(h["codeMatch"], false);
        assert_eq!(h["hashMatch"], false);
    }

    #[test]
    fn mismatch_when_onchain_code_differs_from_pin() {
        let code = "6080";
        let hash = keccak256_hex(code).unwrap();
        // deployed, but the on-chain bytes are not the pinned bytes
        let h = contract_health(
            "Foo",
            Some("0xabc".into()),
            Some(code.to_string()),
            Some(hash),
            Some("0xdead".into()),
        );
        assert_eq!(h["status"], "mismatch");
        assert_eq!(h["codeMatch"], false);
        assert_eq!(h["hashMatch"], false, "different bytes hash differently");
    }

    #[test]
    fn unknown_when_rpc_did_not_answer() {
        let h = contract_health(
            "Foo",
            Some("0xabc".into()),
            Some("6080".into()),
            Some("0xdeadbeef".into()),
            None,
        );
        assert_eq!(h["status"], "unknown");
        assert!(h["codeMatch"].is_null());
        assert!(h["hashMatch"].is_null());
    }

    #[test]
    fn build_health_counts_and_sorts_by_name() {
        let ok = contract_health(
            "Zeta",
            Some("0x2".into()),
            Some("6080".into()),
            Some(keccak256_hex("6080").unwrap()),
            Some("0x6080".into()),
        );
        let bad = contract_health(
            "Alpha",
            Some("0x1".into()),
            Some("6080".into()),
            Some("0xbad".into()),
            Some("0x".into()),
        );
        let v = build_health("o", "r", "0.1.1", "base", "host", vec![ok, bad]).unwrap();
        assert_eq!(v["total"], 2);
        assert_eq!(v["healthy"], 1);
        assert_eq!(v["version"], "0.1.1");
        // sorted by name: Alpha before Zeta
        assert_eq!(v["contracts"][0]["name"], "Alpha");
        assert_eq!(v["contracts"][1]["name"], "Zeta");
    }

    #[test]
    fn build_health_none_when_empty() {
        assert_eq!(
            build_health("o", "r", "0.1.1", "base", "host", vec![]),
            None
        );
    }
}
