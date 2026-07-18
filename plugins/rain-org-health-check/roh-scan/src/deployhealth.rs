//! On-chain HEALTH of a pinned deployment suite: for each contract, is it
//! deployed at its pinned address, and does the live code match BOTH pins — the
//! exact `RUNTIME_CODE` bytes AND the `BYTECODE_HASH` keccak (EXTCODEHASH). Pure
//! parsing + comparison live here and are unit-tested; the RPC fetch is in main.
//!
//! Checking both cross-validates the two pins against each other and against the
//! chain: a `RUNTIME_CODE` that disagrees with its own `BYTECODE_HASH`, or either
//! disagreeing with the deployed code, is a finding.

use crate::rpc::{keccak256_hex, CallClass};
use regex::Regex;
use serde_json::json;

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

/// ERC-165 conformance from the two required probes: `supportsInterface(0x01ffc9a7)`
/// (must be `True`) and `supportsInterface(0xffffffff)` (must be `False`).
/// `absent` = both revert (the contract simply doesn't implement ERC-165);
/// `nonconformant` = it answers but breaks the spec; `unknown` = a probe failed.
pub fn erc165_status(supports_165: CallClass, rejects_invalid: CallClass) -> &'static str {
    use CallClass::*;
    match (supports_165, rejects_invalid) {
        (Unknown, _) | (_, Unknown) => "unknown",
        (True, False) => "conformant",
        (Reverted, Reverted) => "absent",
        _ => "nonconformant",
    }
}

/// Health of one contract given its pins and the live `eth_getCode` result.
/// `onchain` is `Some("0x…")` (deployed), `Some("0x")` (no code), or `None` (the
/// RPC call failed). A contract is `healthy` only when BOTH the exact runtime
/// bytes AND the keccak codehash match their pins. `erc165` is the separate
/// conformance verdict (informational — a beacon legitimately has none).
pub fn contract_health(
    name: &str,
    address: Option<String>,
    runtime_pin: Option<String>,
    hash_pin: Option<String>,
    onchain: Option<String>,
    erc165: &str,
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
        "erc165": erc165,
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

/// Health of one production beacon, resolving BOTH what its `owner()` and
/// `implementation()` actually ARE — not just whether they match a constant:
/// - owner is labelled `safe` (the current, correct token-owner Safe), `legacy`
///   (the pre-migration deploy EOA), `foreign` (anything else), or `unknown`.
/// - impl is resolved to a version: the `target_version` when it equals the
///   target impl, `V1` when it's the pre-Zoltu impl, else `unknown`.
/// - status: `healthy` only when Safe-owned AND at the target version; `behind`
///   when Safe-owned but the impl isn't the target (e.g. still V1); `drift` when
///   the owner isn't the Safe; `unknown` when a live read failed.
#[allow(clippy::too_many_arguments)]
pub fn beacon_health(
    name: &str,
    address: Option<String>,
    safe_owner: &str,
    legacy_owner: &str,
    target_impl: Option<&str>,
    v1_impl: Option<&str>,
    target_version: &str,
    live_owner: Option<String>,
    live_impl: Option<String>,
) -> serde_json::Value {
    let owner_label = match live_owner.as_deref() {
        None => "unknown",
        Some(o) if o.eq_ignore_ascii_case(safe_owner) => "safe",
        Some(o) if o.eq_ignore_ascii_case(legacy_owner) => "legacy",
        Some(_) => "foreign",
    };
    let impl_version = match live_impl.as_deref() {
        None => "unknown",
        Some(l) if target_impl.is_some_and(|t| l.eq_ignore_ascii_case(t)) => target_version,
        Some(l) if v1_impl.is_some_and(|v| l.eq_ignore_ascii_case(v)) => "V1",
        Some(_) => "unknown",
    };
    // `atTarget` is only determinable when BOTH the live impl and the target are
    // known — otherwise `null`, so a missing target can't masquerade as "behind".
    let at_target = match (live_impl.as_deref(), target_impl) {
        (Some(l), Some(t)) => Some(l.eq_ignore_ascii_case(t)),
        _ => None,
    };
    // Without a readable target we can't assert "behind"; stay `unknown`.
    let status = if live_owner.is_none() || live_impl.is_none() || target_impl.is_none() {
        "unknown"
    } else if owner_label != "safe" {
        "drift"
    } else if at_target != Some(true) {
        "behind"
    } else {
        "healthy"
    };
    json!({
        "name": name,
        "address": address,
        "owner": live_owner,
        "ownerLabel": owner_label,
        // What it points at NOW, and what it SHOULD point at (the target-version
        // impl) — so a proposed upgradeTo(...) can be checked against both.
        "implementation": live_impl,
        "implVersion": impl_version,
        "targetImpl": target_impl,
        "targetVersion": target_version,
        "atTarget": at_target,
        "status": status,
    })
}

/// Assemble the `deploymentBeacons` document (sorted by name; `None` if empty).
pub fn build_beacons(
    org: &str,
    repo: &str,
    network: &str,
    rpc_host: &str,
    safe_owner: &str,
    target_version: &str,
    mut beacons: Vec<serde_json::Value>,
) -> Option<serde_json::Value> {
    if beacons.is_empty() {
        return None;
    }
    beacons.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let total = beacons.len();
    let healthy = beacons.iter().filter(|b| b["status"] == "healthy").count();
    Some(json!({
        "org": org,
        "repo": repo,
        "network": network,
        "rpcHost": rpc_host,
        "safeOwner": safe_owner,
        "targetVersion": target_version,
        "total": total,
        "healthy": healthy,
        "beacons": beacons,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "conformant",
        );
        assert_eq!(h["status"], "healthy");
        assert_eq!(h["codeMatch"], true);
        assert_eq!(h["hashMatch"], true);
        assert_eq!(h["erc165"], "conformant");
    }

    #[test]
    fn missing_when_no_code_on_chain() {
        let h = contract_health(
            "Foo",
            Some("0xabc".into()),
            Some("6080".into()),
            Some("0xdeadbeef".into()),
            Some("0x".into()),
            "absent",
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
            "conformant",
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
            "unknown",
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
            "conformant",
        );
        let bad = contract_health(
            "Alpha",
            Some("0x1".into()),
            Some("6080".into()),
            Some("0xbad".into()),
            Some("0x".into()),
            "absent",
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

    #[test]
    fn erc165_status_maps_the_two_probes() {
        use CallClass::*;
        assert_eq!(erc165_status(True, False), "conformant");
        assert_eq!(erc165_status(Reverted, Reverted), "absent");
        // implements it but doesn't reject the invalid id — a spec violation
        assert_eq!(erc165_status(True, True), "nonconformant");
        assert_eq!(erc165_status(False, False), "nonconformant");
        // either probe undetermined -> unknown
        assert_eq!(erc165_status(Unknown, False), "unknown");
        assert_eq!(erc165_status(True, Unknown), "unknown");
    }

    const SAFE: &str = "0xe70d821f3462a074e63b42d0aac6523faae1d611";
    const LEGACY: &str = "0x8e4bdeec7ceb9570d440676345da1dce10329f5b";
    const TARGET: &str = "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286"; // 0.1.1 impl
    const V1: &str = "0xe7573879d73455dc92cb4087fa8177594387cbcd"; // pre-Zoltu impl

    fn bh(owner: Option<&str>, imp: Option<&str>) -> serde_json::Value {
        beacon_health(
            "Receipt beacon",
            Some("0x86e9".into()),
            SAFE,
            LEGACY,
            Some(TARGET),
            Some(V1),
            "0.1.1",
            owner.map(str::to_string),
            imp.map(str::to_string),
        )
    }

    #[test]
    fn beacon_healthy_only_when_safe_owned_and_at_target() {
        // live reads are checksummed; the compare is case-insensitive.
        let b = bh(
            Some("0xE70d821f3462a074e63b42d0AaC6523faAe1d611"),
            Some(TARGET),
        );
        assert_eq!(b["status"], "healthy");
        assert_eq!(b["ownerLabel"], "safe");
        assert_eq!(b["implVersion"], "0.1.1");
        assert_eq!(b["atTarget"], true);
    }

    #[test]
    fn beacon_behind_when_safe_owned_but_still_on_v1() {
        let b = bh(Some(SAFE), Some(V1));
        assert_eq!(b["status"], "behind");
        assert_eq!(b["ownerLabel"], "safe");
        assert_eq!(b["implVersion"], "V1");
        assert_eq!(b["atTarget"], false);
        // both the current (V1) and the should-be (target) impl are surfaced.
        assert_eq!(b["implementation"], V1);
        assert_eq!(b["targetImpl"], TARGET);
    }

    #[test]
    fn beacon_labels_legacy_and_foreign_owners_and_drifts() {
        let legacy = bh(Some(LEGACY), Some(TARGET));
        assert_eq!(legacy["ownerLabel"], "legacy");
        assert_eq!(legacy["status"], "drift");
        let foreign = bh(
            Some("0xdead000000000000000000000000000000000001"),
            Some(TARGET),
        );
        assert_eq!(foreign["ownerLabel"], "foreign");
        assert_eq!(foreign["status"], "drift");
    }

    #[test]
    fn beacon_unknown_when_a_live_read_fails() {
        let b = bh(None, None);
        assert_eq!(b["status"], "unknown");
        assert_eq!(b["ownerLabel"], "unknown");
        assert_eq!(b["implVersion"], "unknown");
    }

    #[test]
    fn beacon_unknown_when_target_impl_unavailable() {
        // Owner + impl read fine, but the target pointer couldn't be read — we
        // can't assert "behind" without it, so stay unknown / atTarget null.
        let b = beacon_health(
            "Receipt beacon",
            Some("0x86e9".into()),
            SAFE,
            LEGACY,
            None,
            Some(V1),
            "0.1.1",
            Some(SAFE.into()),
            Some(V1.into()),
        );
        assert_eq!(b["status"], "unknown");
        assert!(b["atTarget"].is_null());
    }

    #[test]
    fn build_beacons_counts_sorts_and_carries_target() {
        let ok = bh(Some(SAFE), Some(TARGET)); // healthy
        let behind = beacon_health(
            "Alpha",
            Some("0x1".into()),
            SAFE,
            LEGACY,
            Some(TARGET),
            Some(V1),
            "0.1.1",
            Some(SAFE.into()),
            Some(V1.into()),
        );
        let v = build_beacons("o", "r", "base", "host", SAFE, "0.1.1", vec![ok, behind]).unwrap();
        assert_eq!(v["total"], 2);
        assert_eq!(v["healthy"], 1);
        assert_eq!(v["targetVersion"], "0.1.1");
        assert_eq!(v["beacons"][0]["name"], "Alpha");
    }
}
