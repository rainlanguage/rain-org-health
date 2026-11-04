//! On-chain HEALTH of a pinned deployment suite: for each contract, is it
//! deployed at its pinned address, and does the live code match BOTH pins — the
//! exact `RUNTIME_CODE` bytes AND the `BYTECODE_HASH` keccak (EXTCODEHASH). Pure
//! parsing + comparison live here and are unit-tested; the RPC fetch is in main.
//!
//! Checking both cross-validates the two pins against each other and against the
//! chain: a `RUNTIME_CODE` that disagrees with its own `BYTECODE_HASH`, or either
//! disagreeing with the deployed code, is a finding.

use crate::expected::{BeaconAt, ExpectedToken, ExpectedZoltu, EXPECTED_STATE_PATH};
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

/// Whether a pin is an address rather than the all-zero placeholder a chain
/// carries before its deploy has landed.
///
/// Matching against a placeholder would label a contract whose `owner()` or
/// `implementation()` answers the zero address — renounced, bricked, or pointing
/// at nothing — as held by the Safe or the timelock, or as at its target, and so
/// `healthy`. That is the dangerous direction.
///
/// The prefix is stripped case-insensitively and an empty pin counts as
/// unhydrated, so this does not depend on `parse_address_constant`'s regex
/// continuing to emit a lowercase `0x` and exactly 40 hex digits. It does today,
/// which is why `address(0)` arrives as `None` rather than as zeroes — but a
/// guard that holds only because of a regex in another module is one edit from
/// not holding.
pub fn hydrated_pin(pin: &str) -> bool {
    let p = pin
        .strip_prefix("0x")
        .or_else(|| pin.strip_prefix("0X"))
        .unwrap_or(pin);
    !p.is_empty() && !p.chars().all(|c| c == '0')
}

/// The owners a beacon's live `owner()` is resolved against. `expected` is the
/// owner st0x.deploy's CI asserts (`expected-state.json`, the chain's timelock)
/// and is the only healthy one; `safe` and `legacy` only name a wrong owner.
#[derive(Clone, Copy)]
pub struct BeaconOwners<'a> {
    pub expected: &'a str,
    pub safe: Option<&'a str>,
    pub legacy: Option<&'a str>,
}

/// Health of one production beacon, resolving BOTH what its `owner()` and
/// `implementation()` actually ARE:
/// - owner is labelled `timelock` (the expected owner), `safe` (the token-owner
///   Safe), `legacy` (the pre-migration deploy EOA), `foreign`, or `unknown`.
/// - impl is labelled `expected` at the expected implementation, `V1` at the
///   pre-Zoltu impl, else `unknown`.
/// - status: `healthy` when owned by the expected owner AND at the expected
///   implementation; `drift` on any other owner; `behind` on the expected owner
///   but another implementation; `unknown` when a live read failed.
///
/// A Safe-owned beacon is drift: a beacon owner can repoint every production
/// proxy in one transaction, so a beacon on the Safe bypasses the timelock delay.
pub fn beacon_health(
    name: &str,
    address: Option<String>,
    owners: &BeaconOwners,
    target_impl: &str,
    v1_impl: Option<&str>,
    live_owner: Option<String>,
    live_impl: Option<String>,
) -> serde_json::Value {
    // An all-zero pin would match a beacon answering the zero address.
    let is = |o: &str, pin: Option<&str>| {
        pin.is_some_and(|p| hydrated_pin(p) && o.eq_ignore_ascii_case(p))
    };
    let owner_label = match live_owner.as_deref() {
        None => "unknown",
        Some(o) if is(o, Some(owners.expected)) => "timelock",
        Some(o) if is(o, owners.safe) => "safe",
        Some(o) if is(o, owners.legacy) => "legacy",
        Some(_) => "foreign",
    };
    let target_pin = Some(target_impl).filter(|t| hydrated_pin(t));
    let at_target = match (live_impl.as_deref(), target_pin) {
        (Some(l), Some(t)) => Some(l.eq_ignore_ascii_case(t)),
        _ => None,
    };
    let impl_version = match live_impl.as_deref() {
        None => "unknown",
        Some(_) if at_target == Some(true) => "expected",
        Some(l) if is(l, v1_impl) => "V1",
        Some(_) => "unknown",
    };
    let status = if live_owner.is_none() || at_target.is_none() {
        "unknown"
    } else if owner_label != "timelock" {
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
        "implementation": live_impl,
        "implVersion": impl_version,
        "targetImpl": target_pin,
        "targetOwner": Some(owners.expected).filter(|t| hydrated_pin(t)),
        "atTarget": at_target,
        "status": status,
    })
}

/// Assemble the `deploymentBeacons` document (sorted by name; `None` if empty).
/// `timelockOwner` is the expected owner when every beacon shares one.
pub fn build_beacons(
    org: &str,
    repo: &str,
    network: &str,
    rpc_host: &str,
    safe: Option<&str>,
    mut beacons: Vec<serde_json::Value>,
) -> Option<serde_json::Value> {
    if beacons.is_empty() {
        return None;
    }
    beacons.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let total = beacons.len();
    let healthy = beacons.iter().filter(|b| b["status"] == "healthy").count();
    let first = beacons[0]["targetOwner"].as_str();
    let timelock = first.filter(|f| {
        beacons.iter().all(|b| {
            b["targetOwner"]
                .as_str()
                .is_some_and(|o| o.eq_ignore_ascii_case(f))
        })
    });
    Some(json!({
        "org": org,
        "repo": repo,
        "network": network,
        "rpcHost": rpc_host,
        // An unhydrated pin is null, never printed as a `0x0000…` holder.
        "safeOwner": safe.filter(|s| hydrated_pin(s)),
        "timelockOwner": timelock,
        "source": EXPECTED_STATE_PATH,
        "total": total,
        "healthy": healthy,
        "beacons": beacons,
    }))
}

/// A chain, or with `network` `None` every chain, whose beacon set could not be
/// read. Emitted instead of dropping it: a dropped chain renders as no section,
/// which reads as "no production beacons" rather than "this broke".
pub fn beacons_unavailable(
    network: Option<&str>,
    rpc_host: Option<&str>,
    reason: &str,
) -> serde_json::Value {
    json!({
        "network": network,
        "rpcHost": rpc_host,
        "unavailable": true,
        "reason": reason,
    })
}

/// One infra fact `expected-state.json` pins, checked live. `live` is `None`
/// when the read failed; a codehash row's `Some("none")` means no code.
/// status: `healthy` on a case-insensitive match, `drift` on a mismatch,
/// `unknown` when the read failed or there is no pin.
pub fn infra_row(
    component: &str,
    field: &str,
    address: Option<&str>,
    pin: Option<&str>,
    live: Option<String>,
) -> serde_json::Value {
    let status = match (live.as_deref(), pin) {
        (Some(l), Some(e)) if l.eq_ignore_ascii_case(e) => "healthy",
        (Some(_), Some(_)) => "drift",
        _ => "unknown",
    };
    json!({
        "component": component,
        "field": field,
        "address": address,
        "expected": pin,
        "live": live,
        "status": status,
    })
}

/// One Zoltu suite's codehash row; `live` as for [`infra_row`].
pub fn zoltu_row(z: &ExpectedZoltu, live: Option<String>) -> serde_json::Value {
    infra_row(
        &format!("zoltu {}", z.key),
        "codehash",
        Some(&z.code.address),
        Some(z.code.codehash.as_str()).filter(|p| hydrated_pin(p)),
        live,
    )
}

/// The beacon-set deployer's beacons, read live, for legs the file pins as `0x0`.
#[derive(Debug, Clone, Default)]
pub struct DeployerBeacons {
    pub receipt_vault: Option<String>,
    pub receipt: Option<String>,
}

/// One token's legs read live; `None` is a failed read, a codehash
/// `Some("none")` no code.
#[derive(Debug, Clone, Default)]
pub struct TokenLive {
    pub receipt_vault_name: Option<String>,
    pub receipt_vault_symbol: Option<String>,
    pub receipt_vault_decimals: Option<u8>,
    pub receipt_vault_receipt: Option<String>,
    pub receipt_vault_owner: Option<String>,
    pub receipt_vault_authoriser: Option<String>,
    pub receipt_vault_beacon: Option<String>,
    pub receipt_vault_codehash: Option<String>,
    pub receipt_beacon: Option<String>,
    pub receipt_codehash: Option<String>,
    pub wrapped_token_vault_name: Option<String>,
    pub wrapped_token_vault_symbol: Option<String>,
    pub wrapped_token_vault_decimals: Option<u8>,
    pub wrapped_token_vault_asset: Option<String>,
    pub wrapped_token_vault_beacon: Option<String>,
    pub wrapped_token_vault_codehash: Option<String>,
}

/// One token's rows. `renamed` is whether the chain records the file's
/// `renameMigration` (`None`: unread, so the renamable rows are unknown).
pub fn token_rows(
    t: &ExpectedToken,
    renamed: Option<bool>,
    deployer: &DeployerBeacons,
    live: TokenLive,
) -> Vec<serde_json::Value> {
    let pin = |p: &str| Some(p.to_string()).filter(|p| hydrated_pin(p));
    let pick = |name: &str, renamed_name: &str| {
        renamed.map(|r| if r { renamed_name } else { name }.to_string())
    };
    let beacon = |at: &BeaconAt| match at {
        BeaconAt::Address(a) => Some(a.clone()),
        BeaconAt::DeployerReceipt => deployer.receipt.clone(),
        BeaconAt::DeployerReceiptVault => deployer.receipt_vault.clone(),
    };
    let rv = t.receipt_vault.as_str();
    let rc = t.receipt.as_str();
    let wv = t.wrapped_token_vault.as_str();
    let checks: [(&str, &str, Option<String>, Option<String>); 16] = [
        (
            "receiptVault name",
            rv,
            pick(&t.name, &t.renamed_name),
            live.receipt_vault_name,
        ),
        (
            "receiptVault symbol",
            rv,
            Some(t.symbol.clone()),
            live.receipt_vault_symbol,
        ),
        (
            "receiptVault decimals",
            rv,
            Some(t.decimals.to_string()),
            live.receipt_vault_decimals.map(|d| d.to_string()),
        ),
        (
            "receiptVault receipt",
            rv,
            pin(&t.receipt_vault_receipt),
            live.receipt_vault_receipt,
        ),
        (
            "receiptVault owner",
            rv,
            pin(&t.owner),
            live.receipt_vault_owner,
        ),
        (
            "receiptVault authoriser",
            rv,
            pin(&t.authoriser),
            live.receipt_vault_authoriser,
        ),
        (
            "receiptVault beacon",
            rv,
            beacon(&t.receipt_vault_beacon),
            live.receipt_vault_beacon,
        ),
        (
            "receiptVault codehash",
            rv,
            pin(&t.receipt_vault_codehash),
            live.receipt_vault_codehash,
        ),
        (
            "receipt beacon",
            rc,
            beacon(&t.receipt_beacon),
            live.receipt_beacon,
        ),
        (
            "receipt codehash",
            rc,
            pin(&t.receipt_codehash),
            live.receipt_codehash,
        ),
        (
            "wrappedTokenVault name",
            wv,
            pick(
                &t.wrapped_token_vault_name,
                &t.wrapped_token_vault_renamed_name,
            ),
            live.wrapped_token_vault_name,
        ),
        (
            "wrappedTokenVault symbol",
            wv,
            pick(
                &t.wrapped_token_vault_symbol,
                &t.wrapped_token_vault_renamed_symbol,
            ),
            live.wrapped_token_vault_symbol,
        ),
        (
            "wrappedTokenVault decimals",
            wv,
            Some(t.wrapped_token_vault_decimals.to_string()),
            live.wrapped_token_vault_decimals.map(|d| d.to_string()),
        ),
        (
            "wrappedTokenVault asset",
            wv,
            pin(&t.wrapped_token_vault_asset),
            live.wrapped_token_vault_asset,
        ),
        (
            "wrappedTokenVault beacon",
            wv,
            pin(&t.wrapped_token_vault_beacon),
            live.wrapped_token_vault_beacon,
        ),
        (
            "wrappedTokenVault codehash",
            wv,
            pin(&t.wrapped_token_vault_codehash),
            live.wrapped_token_vault_codehash,
        ),
    ];
    let id = format!("token {}", t.symbol);
    checks
        .into_iter()
        .map(|(field, address, expected, live)| {
            infra_row(&id, field, Some(address), expected.as_deref(), live)
        })
        .collect()
}

/// Assemble one chain's `deploymentInfra` document.
pub fn build_infra(
    org: &str,
    repo: &str,
    network: &str,
    rpc_host: &str,
    rows: Vec<serde_json::Value>,
) -> serde_json::Value {
    let total = rows.len();
    let healthy = rows.iter().filter(|r| r["status"] == "healthy").count();
    json!({
        "org": org,
        "repo": repo,
        "network": network,
        "rpcHost": rpc_host,
        "source": EXPECTED_STATE_PATH,
        "total": total,
        "healthy": healthy,
        "rows": rows,
    })
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
    const TIMELOCK: &str = "0x48ba1371a78e6cc54157c63721756ab444510db3";
    const TARGET: &str = "0xb092f127bd44f19bfa96e78db4beaf1066d2914e"; // 0.1.30 receipt impl
    const V1: &str = "0xe7573879d73455dc92cb4087fa8177594387cbcd"; // pre-Zoltu impl
    const ZERO: &str = "0x0000000000000000000000000000000000000000";

    const OWNERS: BeaconOwners = BeaconOwners {
        expected: TIMELOCK,
        safe: Some(SAFE),
        legacy: Some(LEGACY),
    };

    fn bh_with(
        owners: &BeaconOwners,
        target: &str,
        owner: Option<&str>,
        imp: Option<&str>,
    ) -> serde_json::Value {
        beacon_health(
            "Receipt beacon",
            Some("0x86e9".into()),
            owners,
            target,
            Some(V1),
            owner.map(str::to_string),
            imp.map(str::to_string),
        )
    }

    fn bh(owner: Option<&str>, imp: Option<&str>) -> serde_json::Value {
        bh_with(&OWNERS, TARGET, owner, imp)
    }

    #[test]
    fn beacon_healthy_only_when_timelock_owned_and_at_target() {
        // live reads are checksummed; the compare is case-insensitive.
        let b = bh(
            Some("0x48Ba1371A78E6cC54157c63721756ab444510DB3"),
            Some("0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E"),
        );
        assert_eq!(b["status"], "healthy");
        assert_eq!(b["ownerLabel"], "timelock");
        assert_eq!(b["implVersion"], "expected");
        assert_eq!(b["atTarget"], true);
        assert_eq!(b["targetOwner"], TIMELOCK);
        assert_eq!(b["targetImpl"], TARGET);
    }

    /// #219: the Safe can repoint every proxy in one transaction, so a beacon
    /// handed back to it bypasses the timelock delay.
    #[test]
    fn a_safe_owned_beacon_is_drift_even_at_target() {
        let b = bh(Some(SAFE), Some(TARGET));
        assert_eq!(b["ownerLabel"], "safe");
        assert_eq!(b["status"], "drift");
    }

    /// #216: an upgraded beacon at the expected impl is at target, and one still
    /// on an older impl is behind with both addresses surfaced.
    #[test]
    fn a_timelock_owned_beacon_on_another_impl_is_behind() {
        let b = bh(Some(TIMELOCK), Some(V1));
        assert_eq!(b["ownerLabel"], "timelock");
        assert_eq!(b["status"], "behind");
        assert_eq!(b["implVersion"], "V1");
        assert_eq!(b["atTarget"], false);
        assert_eq!(b["implementation"], V1);
        assert_eq!(b["targetImpl"], TARGET);
        let other = bh(
            Some(TIMELOCK),
            Some("0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286"),
        );
        assert_eq!(other["status"], "behind");
        assert_eq!(other["implVersion"], "unknown");
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
        let no_labels = BeaconOwners {
            expected: TIMELOCK,
            safe: None,
            legacy: None,
        };
        for o in [SAFE, LEGACY] {
            assert_eq!(
                bh_with(&no_labels, TARGET, Some(o), Some(TARGET))["ownerLabel"],
                "foreign"
            );
        }
    }

    /// The expected owner is what CI accepts, so it wins over the Safe label
    /// when both name the same address.
    #[test]
    fn the_expected_owner_wins_over_the_safe_label() {
        let same = BeaconOwners {
            expected: SAFE,
            safe: Some(SAFE),
            legacy: Some(LEGACY),
        };
        let b = bh_with(&same, TARGET, Some(SAFE), Some(TARGET));
        assert_eq!(b["ownerLabel"], "timelock");
        assert_eq!(b["status"], "healthy");
    }

    /// An all-zero pin is a placeholder, not an address: a beacon answering the
    /// zero address must never match one, in any spelling.
    #[test]
    fn an_unhydrated_pin_never_adopts_a_zero_owner_or_impl() {
        for spelling in [
            ZERO,
            "0X0000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000",
            "",
        ] {
            let zeros = BeaconOwners {
                expected: spelling,
                safe: Some(spelling),
                legacy: Some(spelling),
            };
            let b = bh_with(&zeros, TARGET, Some(ZERO), Some(TARGET));
            assert_eq!(
                b["ownerLabel"], "foreign",
                "zero owner matched {spelling:?}"
            );
            assert_eq!(b["status"], "drift");
            assert!(b["targetOwner"].is_null());
            let t = bh_with(&OWNERS, spelling, Some(TIMELOCK), Some(ZERO));
            assert!(t["atTarget"].is_null(), "zero impl matched {spelling:?}");
            assert!(t["targetImpl"].is_null());
            assert_eq!(t["status"], "unknown");
        }
    }

    #[test]
    fn beacon_unknown_when_a_live_read_fails() {
        let b = bh(None, None);
        assert_eq!(b["status"], "unknown");
        assert_eq!(b["ownerLabel"], "unknown");
        assert_eq!(b["implVersion"], "unknown");
        assert_eq!(bh(Some(TIMELOCK), None)["status"], "unknown");
        assert_eq!(bh(None, Some(TARGET))["status"], "unknown");
    }

    #[test]
    fn build_beacons_counts_sorts_and_carries_the_expected_owner() {
        let ok = bh(Some(TIMELOCK), Some(TARGET));
        let safe_owned = beacon_health(
            "Alpha",
            Some("0x1".into()),
            &OWNERS,
            TARGET,
            None,
            Some(SAFE.into()),
            Some(TARGET.into()),
        );
        let v = build_beacons("o", "r", "base", "host", Some(SAFE), vec![ok, safe_owned]).unwrap();
        assert_eq!(v["total"], 2);
        assert_eq!(v["healthy"], 1);
        assert_eq!(v["beacons"][0]["name"], "Alpha");
        assert_eq!(v["safeOwner"], SAFE);
        assert_eq!(v["timelockOwner"], TIMELOCK);
        assert_eq!(v["source"], "expected-state.json");
        assert!(v.get("targetVersion").is_none());
        assert!(build_beacons("o", "r", "base", "host", Some(SAFE), vec![]).is_none());
    }

    /// Beacons expecting different owners have no one chain owner to show.
    #[test]
    fn build_beacons_shows_no_timelock_when_beacons_disagree() {
        let other = BeaconOwners {
            expected: "0x831e4e1bb2b9a67c00b7d17f252a18a22cd0bd2b",
            ..OWNERS
        };
        let a = bh(Some(TIMELOCK), Some(TARGET));
        let b = bh_with(&other, TARGET, Some(TIMELOCK), Some(TARGET));
        let v = build_beacons("o", "r", "base", "host", Some(ZERO), vec![a, b]).unwrap();
        assert!(v["timelockOwner"].is_null());
        assert!(
            v["safeOwner"].is_null(),
            "zero Safe pin emitted as a holder"
        );
    }

    /// An unreadable chain is REPORTED, not dropped. A dropped chain renders
    /// as no section, which reads as "this chain has no production beacons" —
    /// the same silent vanish that hid the token registry.
    #[test]
    fn beacons_unavailable_names_the_chain_and_the_reason() {
        let v = beacons_unavailable(
            Some("ethereum"),
            Some("ethereum-rpc.publicnode.com"),
            "constants unreadable",
        );
        assert_eq!(v["network"], "ethereum");
        assert_eq!(v["unavailable"], true);
        assert_eq!(v["reason"], "constants unreadable");
        assert_eq!(v["rpcHost"], "ethereum-rpc.publicnode.com");
        assert!(
            v["beacons"].is_null(),
            "an unavailable chain must not claim a beacon list"
        );
    }
}
