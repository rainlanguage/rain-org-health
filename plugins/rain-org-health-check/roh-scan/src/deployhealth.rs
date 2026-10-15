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

/// The pointer files directly under `src/generated/<dir>/`, grouped per dir and
/// sorted: `(suite, [(contract, body)])`, the suite being the dir with `_` read as `.`.
pub fn pointer_suites(sources: &[(String, String)]) -> Vec<(String, Vec<(String, &str)>)> {
    let mut files: Vec<(String, String, &str)> = sources
        .iter()
        .filter_map(|(path, body)| {
            let (dir, file) = path.strip_prefix("src/generated/")?.split_once('/')?;
            let name = file.strip_suffix(".sol").filter(|n| !n.contains('/'))?;
            let contract = name.strip_suffix(".pointers").unwrap_or(name);
            Some((dir.replace('_', "."), contract.to_string(), body.as_str()))
        })
        .collect();
    files.sort();
    let mut suites: Vec<(String, Vec<(String, &str)>)> = Vec::new();
    for (suite, contract, body) in files {
        match suites.last_mut() {
            Some((s, fs)) if *s == suite => fs.push((contract, body)),
            _ => suites.push((suite, vec![(contract, body)])),
        }
    }
    suites
}

/// A pointer file's `DEPLOYED_ADDRESS` literal.
pub fn deployed_address(src: &str) -> Option<String> {
    crate::addrnames::parse_pins(src)?
        .into_iter()
        .find_map(|p| match p.value {
            crate::addrnames::PinValue::Literal(a) if p.name == "DEPLOYED_ADDRESS" => Some(a),
            _ => None,
        })
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

/// Health of one production beacon from its live `owner()` and `implementation()`.
/// Owner: `safe`, `legacy`, `timelock`, `foreign` or `unknown`. Impl: `pinned`,
/// `v1` or `unknown`. Status: `healthy` when Safe- or timelock-held and at the
/// pinned impl, `behind` when governed but elsewhere, `drift` when not governed,
/// `unknown` when a read or the pin is missing.
#[allow(clippy::too_many_arguments)]
pub fn beacon_health(
    name: &str,
    address: Option<String>,
    safe_owner: Option<&str>,
    legacy_owner: Option<&str>,
    timelock_owner: Option<&str>,
    pinned_impl: Option<&str>,
    v1_impl: Option<&str>,
    live_owner: Option<String>,
    live_impl: Option<String>,
) -> serde_json::Value {
    let is = |pin: Option<&str>, a: &str| pin.is_some_and(|p| a.eq_ignore_ascii_case(p));
    // Legacy before timelock: a timelock pin equal to the deploy EOA must read as drift.
    let owner_label = match live_owner.as_deref() {
        None => "unknown",
        Some(o) if is(safe_owner, o) => "safe",
        Some(o) if is(legacy_owner, o) => "legacy",
        Some(o) if is(timelock_owner, o) => "timelock",
        Some(_) => "foreign",
    };
    let governed = owner_label == "safe" || owner_label == "timelock";
    let impl_version = match live_impl.as_deref() {
        None => "unknown",
        Some(l) if is(pinned_impl, l) => "pinned",
        Some(l) if is(v1_impl, l) => "v1",
        Some(_) => "unknown",
    };
    let at_pinned = live_impl
        .as_deref()
        .zip(pinned_impl)
        .map(|(l, p)| l.eq_ignore_ascii_case(p));
    let status = if live_owner.is_none() || at_pinned.is_none() {
        "unknown"
    } else if !governed {
        "drift"
    } else if at_pinned != Some(true) {
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
        "pinnedImpl": pinned_impl,
        "atPinned": at_pinned,
        "status": status,
    })
}

/// Assemble one chain's `deploymentBeacons` entry, beacons in slot order.
pub fn build_beacons(
    org: &str,
    repo: &str,
    network: &str,
    rpc_host: &str,
    safe_owner: Option<&str>,
    timelock_owner: Option<&str>,
    beacons: Vec<serde_json::Value>,
) -> serde_json::Value {
    let healthy = beacons.iter().filter(|b| b["status"] == "healthy").count();
    json!({
        "org": org,
        "repo": repo,
        "network": network,
        "rpcHost": rpc_host,
        "safeOwner": safe_owner,
        "timelockOwner": timelock_owner,
        "total": beacons.len(),
        "healthy": healthy,
        "beacons": beacons,
    })
}

/// A chain whose beacon set could not be read at all.
///
/// Emitted INSTEAD of dropping the chain. A dropped chain renders as no
/// section, which is indistinguishable from "this chain has no production
/// beacons" — the same silent vanish that hid the token registry. The block
/// carries the chain's identity so the dashboard can say which chain is
/// missing and why.
pub fn beacons_unavailable(network: &str, rpc_host: &str, reason: &str) -> serde_json::Value {
    json!({
        "network": network,
        "rpcHost": rpc_host,
        "unavailable": true,
        "reason": reason,
    })
}

/// Live on-chain reads for one token; each field is `None` if that read failed.
#[derive(Default)]
pub struct TokenLive {
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
    pub asset: Option<String>,
    pub unwrapped_deployed: Option<bool>,
    pub legacy_deployed: Option<bool>,
    pub receipt_deployed: Option<bool>,
    /// `authorizer()` read from the token's receipt vault (its `unwrappedAddress`,
    /// where the setter lives); `None` for a plain token or a failed read.
    pub authoriser: Option<String>,
}

/// The registry-declared facts about one token plus the authoriser addresses it
/// is checked against — grouped so `token_health` isn't a wall of positional args.
#[derive(Clone, Copy)]
pub struct TokenSpec<'a> {
    pub symbol: &'a str,
    pub name: &'a str,
    pub decimals: u64,
    pub address: &'a str,
    pub unwrapped: Option<&'a str>,
    pub legacy: Option<&'a str>,
    pub receipt: Option<&'a str>,
    /// The chain's authoriser as privileged-addresses.json pins it.
    pub auth_pinned: Option<&'a str>,
    /// Whether this token's receipt vault is in the chain's pinned `receiptVaults`:
    /// `Some(true/false)` for a wrapped token, `None` for a plain token.
    pub in_migration: Option<bool>,
}

/// A live authoriser against the pinned one → (`label`, `atPinned`). `label`:
/// `pinned`, `none` (address(0)), `foreign`, or `unknown` (unread).
pub fn resolve_authoriser(
    live: Option<&str>,
    pinned: Option<&str>,
) -> (&'static str, Option<bool>) {
    let label = match live {
        None => "unknown",
        Some(a) if pinned.is_some_and(|p| a.eq_ignore_ascii_case(p)) => "pinned",
        Some(a) if a.trim_start_matches("0x").chars().all(|c| c == '0') => "none",
        Some(_) => "foreign",
    };
    (
        label,
        live.zip(pinned).map(|(a, p)| a.eq_ignore_ascii_case(p)),
    )
}

/// A pinned receipt vault no registry token names, with its live identity and authoriser.
pub fn extra_vault(
    address: &str,
    name: Option<String>,
    symbol: Option<String>,
    deployed: Option<bool>,
    live_authoriser: Option<&str>,
    auth_pinned: Option<&str>,
) -> serde_json::Value {
    let (label, at_pinned) = resolve_authoriser(live_authoriser, auth_pinned);
    json!({
        "address": address,
        "name": name,
        "symbol": symbol,
        "deployed": deployed,
        "authoriser": live_authoriser,
        "authoriserLabel": label,
        "authoriserPinned": auth_pinned,
        "atPinnedAuthoriser": at_pinned,
    })
}

/// Health of one registry token. Every token's on-chain `name`/`symbol`/`decimals`
/// must match the registry EXACTLY (verbatim, not normalised). A *wrapped* token
/// (one that declares an `unwrappedAddress`) additionally must have `asset()`
/// pointing at that underlying and every linked address it declares (unwrapped /
/// legacy / receipt) deployed — a *plain* collateral token (e.g. USDC, which
/// declares no `unwrappedAddress`) is judged on identity alone. `ok` = all
/// applicable checks confirmed; `mismatch` = an identity field differs; `wiring` =
/// `asset()` or a declared linked address is wrong; `unknown` = a core read failed.
///
/// The receipt vault's live authoriser is resolved against the pinned one; it
/// does not affect `status`.
pub fn token_health(spec: &TokenSpec, live: &TokenLive) -> serde_json::Value {
    let TokenSpec {
        symbol,
        name,
        decimals,
        address,
        unwrapped,
        legacy,
        receipt,
        auth_pinned,
        in_migration,
    } = *spec;
    let name_ok = live.name.as_deref().map(|n| n == name);
    let symbol_ok = live.symbol.as_deref().map(|s| s == symbol);
    let decimals_ok = live.decimals.map(|d| d as u64 == decimals);
    // asset() is only meaningful for a wrapped token, checked against its unwrapped.
    let asset_ok = match (live.asset.as_deref(), unwrapped) {
        (Some(a), Some(u)) => Some(a.eq_ignore_ascii_case(u)),
        _ => None,
    };
    let wrapped = unwrapped.is_some();
    let bad = |o: Option<bool>| o == Some(false);
    let ok = |o: Option<bool>| o == Some(true);
    // A DECLARED linked address (Some) must be deployed; an undeclared one (None —
    // newer wrapped tokens carry no legacy) is simply not applicable, so its
    // deployed flag is ignored.
    let link_bad = |addr: Option<&str>, dep: Option<bool>| addr.is_some() && dep == Some(false);
    let link_pending = |addr: Option<&str>, dep: Option<bool>| addr.is_some() && dep != Some(true);

    let identity_bad = bad(name_ok) || bad(symbol_ok) || bad(decimals_ok);
    let identity_ok = ok(name_ok) && ok(symbol_ok) && ok(decimals_ok);
    let wiring_bad = wrapped
        && (asset_ok == Some(false)
            || link_bad(unwrapped, live.unwrapped_deployed)
            || link_bad(legacy, live.legacy_deployed)
            || link_bad(receipt, live.receipt_deployed));
    let wiring_pending = wrapped
        && (asset_ok != Some(true)
            || link_pending(unwrapped, live.unwrapped_deployed)
            || link_pending(legacy, live.legacy_deployed)
            || link_pending(receipt, live.receipt_deployed));

    let status = if live.name.is_none() {
        "unknown"
    } else if identity_bad {
        "mismatch"
    } else if wiring_bad {
        "wiring"
    } else if !identity_ok || wiring_pending {
        "unknown"
    } else {
        "ok"
    };

    // Authoriser (receipt-vault only): resolve who it actually is, and whether it
    // already sits at the V4-clone target the pending setAuthorizer bundle sets.
    let (authoriser_label, at_pinned_authoriser) = if wrapped {
        resolve_authoriser(live.authoriser.as_deref(), auth_pinned)
    } else {
        ("n/a", None)
    };

    json!({
        "symbol": symbol,
        "name": name,
        "address": address,
        "status": status,
        "wrapped": wrapped,
        "nameOk": name_ok,
        "symbolOk": symbol_ok,
        "decimalsOk": decimals_ok,
        // The live on-chain values, so a mismatch can show what it IS now next to
        // what the registry says it SHOULD be — not just a red ✗.
        "liveName": live.name,
        "liveSymbol": live.symbol,
        "liveDecimals": live.decimals,
        "assetOk": asset_ok,
        "asset": live.asset,
        "unwrapped": unwrapped,
        "legacy": legacy,
        "receipt": receipt,
        "unwrappedDeployed": live.unwrapped_deployed,
        "legacyDeployed": live.legacy_deployed,
        "receiptDeployed": live.receipt_deployed,
        "authoriser": live.authoriser,
        "authoriserLabel": authoriser_label,
        "authoriserPinned": auth_pinned,
        "atPinnedAuthoriser": at_pinned_authoriser,
        "inMigrationSet": in_migration,
    })
}

/// Assemble the `deploymentTokens` document (sorted by symbol; `None` if empty).
/// `authoriser` is the pinned authoriser and whether it is deployed.
pub fn build_tokens(
    org: &str,
    repo: &str,
    network: &str,
    rpc_host: &str,
    authoriser: serde_json::Value,
    reconcile: serde_json::Value,
    mut tokens: Vec<serde_json::Value>,
) -> Option<serde_json::Value> {
    // An empty token list is REPORTED, not swallowed. It means the registry and
    // the governed set did not intersect — almost always because the governed
    // set failed to parse after an upstream refactor — and the reconcile
    // counts carried below are exactly what identify that. Returning None here
    // made the whole section disappear from the dashboard, taking its own
    // diagnosis with it, so the outage looked like a missing feature.

    tokens.sort_by(|a, b| a["symbol"].as_str().cmp(&b["symbol"].as_str()));
    let total = tokens.len();
    let ok = tokens.iter().filter(|t| t["status"] == "ok").count();
    let wrapped = tokens.iter().filter(|t| t["wrapped"] == true).count();
    let at_pinned = tokens
        .iter()
        .filter(|t| t["atPinnedAuthoriser"] == true)
        .count();
    Some(json!({
        "org": org,
        "repo": repo,
        "network": network,
        "rpcHost": rpc_host,
        "total": total,
        "ok": ok,
        "authoriser": authoriser,
        "wrappedCount": wrapped,
        "atPinnedAuthoriser": at_pinned,
        "reconcile": reconcile,
        "tokens": tokens,
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
    const TIMELOCK: &str = "0x48ba1371a78e6cc54157c63721756ab444510db3";
    const PINNED: &str = "0x2df5cfe6d688ef9ff1b7c59a499d254b1527b286";
    const V1: &str = "0xe7573879d73455dc92cb4087fa8177594387cbcd";
    const ZERO: &str = "0x0000000000000000000000000000000000000000";

    fn pins(
        safe: Option<&str>,
        legacy: Option<&str>,
        timelock: Option<&str>,
        owner: Option<&str>,
        imp: Option<&str>,
    ) -> serde_json::Value {
        beacon_health(
            "Receipt beacon",
            Some("0x86e9".into()),
            safe,
            legacy,
            timelock,
            Some(PINNED),
            Some(V1),
            owner.map(str::to_string),
            imp.map(str::to_string),
        )
    }

    fn bh(owner: Option<&str>, imp: Option<&str>) -> serde_json::Value {
        pins(Some(SAFE), Some(LEGACY), Some(TIMELOCK), owner, imp)
    }

    #[test]
    fn a_governed_beacon_at_the_pinned_impl_is_healthy() {
        let b = bh(
            Some(&SAFE.to_uppercase().replace("0X", "0x")),
            Some(&PINNED.to_uppercase().replace("0X", "0x")),
        );
        assert_eq!(
            (b["ownerLabel"].as_str(), b["implVersion"].as_str()),
            (Some("safe"), Some("pinned"))
        );
        assert_eq!(
            (b["atPinned"].as_bool(), b["status"].as_str()),
            (Some(true), Some("healthy"))
        );
        assert_eq!(b["pinnedImpl"], PINNED);
        let t = bh(Some(TIMELOCK), Some(PINNED));
        assert_eq!(
            (t["ownerLabel"].as_str(), t["status"].as_str()),
            (Some("timelock"), Some("healthy"))
        );
    }

    #[test]
    fn a_governed_beacon_off_the_pinned_impl_is_behind() {
        let v1 = bh(Some(SAFE), Some(V1));
        assert_eq!(
            (v1["implVersion"].as_str(), v1["atPinned"].as_bool()),
            (Some("v1"), Some(false))
        );
        assert_eq!(v1["status"], "behind");
        let other = bh(
            Some(TIMELOCK),
            Some("0x00000000000000000000000000000000000000ff"),
        );
        assert_eq!(
            (other["implVersion"].as_str(), other["status"].as_str()),
            (Some("unknown"), Some("behind"))
        );
    }

    #[test]
    fn an_ungoverned_owner_is_drift() {
        let l = bh(Some(LEGACY), Some(PINNED));
        assert_eq!(
            (l["ownerLabel"].as_str(), l["status"].as_str()),
            (Some("legacy"), Some("drift"))
        );
        let f = bh(
            Some("0x00000000000000000000000000000000000000ff"),
            Some(PINNED),
        );
        assert_eq!(
            (f["ownerLabel"].as_str(), f["status"].as_str()),
            (Some("foreign"), Some("drift"))
        );
    }

    #[test]
    fn owner_precedence_is_safe_then_legacy_then_timelock() {
        assert_eq!(
            pins(
                Some(SAFE),
                Some(LEGACY),
                Some(SAFE),
                Some(SAFE),
                Some(PINNED)
            )["ownerLabel"],
            "safe"
        );
        let l = pins(
            Some(SAFE),
            Some(LEGACY),
            Some(LEGACY),
            Some(LEGACY),
            Some(PINNED),
        );
        assert_eq!(
            (l["ownerLabel"].as_str(), l["status"].as_str()),
            (Some("legacy"), Some("drift"))
        );
    }

    #[test]
    fn an_absent_pin_matches_nothing() {
        let t = pins(Some(SAFE), Some(LEGACY), None, Some(TIMELOCK), Some(PINNED));
        assert_eq!(
            (t["ownerLabel"].as_str(), t["status"].as_str()),
            (Some("foreign"), Some("drift"))
        );
        let z = pins(None, None, None, Some(ZERO), Some(PINNED));
        assert_eq!(
            (z["ownerLabel"].as_str(), z["status"].as_str()),
            (Some("foreign"), Some("drift"))
        );
    }

    #[test]
    fn a_failed_read_or_missing_impl_pin_is_unknown() {
        for b in [bh(None, Some(PINNED)), bh(Some(SAFE), None)] {
            assert_eq!(b["status"], "unknown");
        }
        assert_eq!(bh(None, Some(PINNED))["ownerLabel"], "unknown");
        assert_eq!(bh(Some(SAFE), None)["implVersion"], "unknown");
        let unpinned = beacon_health(
            "b",
            None,
            Some(SAFE),
            None,
            None,
            None,
            Some(V1),
            Some(SAFE.into()),
            Some(V1.into()),
        );
        assert_eq!(
            (
                unpinned["status"].as_str(),
                unpinned["implVersion"].as_str()
            ),
            (Some("unknown"), Some("v1"))
        );
        assert!(unpinned["atPinned"].is_null());
    }

    #[test]
    fn build_beacons_counts_healthy_and_keeps_slot_order() {
        let v = build_beacons(
            "o",
            "r",
            "base",
            "host",
            Some(SAFE),
            None,
            vec![bh(Some(SAFE), Some(V1)), bh(Some(TIMELOCK), Some(PINNED))],
        );
        assert_eq!(
            (v["total"].as_u64(), v["healthy"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(v["beacons"][0]["status"], "behind");
        assert_eq!(
            (v["safeOwner"].as_str(), v["timelockOwner"].is_null()),
            (Some(SAFE), true)
        );
        assert_eq!(
            (v["network"].as_str(), v["rpcHost"].as_str()),
            (Some("base"), Some("host"))
        );
    }

    fn tl(
        name: Option<&str>,
        sym: Option<&str>,
        dec: Option<u8>,
        asset: Option<&str>,
        dep: Option<bool>,
    ) -> TokenLive {
        TokenLive {
            name: name.map(str::to_string),
            symbol: sym.map(str::to_string),
            decimals: dec,
            asset: asset.map(str::to_string),
            unwrapped_deployed: dep,
            legacy_deployed: dep,
            receipt_deployed: dep,
            authoriser: None,
        }
    }

    // A minimal wrapped-token spec (no legacy/receipt/authoriser pin).
    fn spec<'a>(
        symbol: &'a str,
        name: &'a str,
        decimals: u64,
        address: &'a str,
        unwrapped: Option<&'a str>,
    ) -> TokenSpec<'a> {
        TokenSpec {
            symbol,
            name,
            decimals,
            address,
            unwrapped,
            legacy: None,
            receipt: None,
            auth_pinned: None,
            in_migration: unwrapped.map(|_| true),
        }
    }

    #[test]
    fn token_ok_when_identity_and_wiring_confirmed() {
        let live = tl(
            Some("Wrapped NVIDIA"),
            Some("wtNVDA"),
            Some(18),
            Some("0x7271"),
            Some(true),
        );
        let t = token_health(
            &spec("wtNVDA", "Wrapped NVIDIA", 18, "0xfb5b", Some("0x7271")),
            &live,
        );
        assert_eq!(t["status"], "ok");
        assert_eq!(t["nameOk"], true);
        assert_eq!(t["assetOk"], true);
    }

    #[test]
    fn token_mismatch_on_symbol() {
        let live = tl(
            Some("Wrapped NVIDIA"),
            Some("wtWRONG"),
            Some(18),
            Some("0x7271"),
            Some(true),
        );
        let t = token_health(
            &spec("wtNVDA", "Wrapped NVIDIA", 18, "0xfb5b", Some("0x7271")),
            &live,
        );
        assert_eq!(t["status"], "mismatch");
        assert_eq!(t["symbolOk"], false);
        // the live on-chain values ride along so the page can show the diff
        assert_eq!(t["liveSymbol"], "wtWRONG");
        assert_eq!(t["liveName"], "Wrapped NVIDIA");
    }

    #[test]
    fn token_wiring_when_asset_or_linked_wrong() {
        // asset() points at the wrong underlying
        let bad_asset = tl(
            Some("N"),
            Some("wtNVDA"),
            Some(18),
            Some("0xbeef"),
            Some(true),
        );
        assert_eq!(
            token_health(
                &spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271")),
                &bad_asset
            )["status"],
            "wiring"
        );
        // a DECLARED linked address that isn't deployed on-chain
        let mut nodep = tl(
            Some("N"),
            Some("wtNVDA"),
            Some(18),
            Some("0x7271"),
            Some(true),
        );
        nodep.receipt_deployed = Some(false);
        let mut s = spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271"));
        s.receipt = Some("0xrec");
        assert_eq!(token_health(&s, &nodep)["status"], "wiring");
    }

    #[test]
    fn token_undeclared_legacy_does_not_block_ok() {
        // a wrapped token with NO legacy address (newer issuance): its legacy
        // deploy flag is irrelevant and must not keep it out of `ok`.
        let mut live = tl(
            Some("N"),
            Some("wtNVDA"),
            Some(18),
            Some("0x7271"),
            Some(true),
        );
        live.legacy_deployed = None; // undeclared → unread
        let mut s = spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271"));
        s.receipt = Some("0xrec");
        assert_eq!(token_health(&s, &live)["status"], "ok");
    }

    #[test]
    fn token_plain_collateral_is_judged_on_identity_only() {
        // USDC-like: no unwrappedAddress, no asset(). Identity match alone = ok,
        // never dragged to `unknown`/`wiring` by absent wiring.
        let live = TokenLive {
            name: Some("USD Coin".into()),
            symbol: Some("USDC".into()),
            decimals: Some(6),
            ..Default::default()
        };
        let t = token_health(&spec("USDC", "USD Coin", 6, "0x8335", None), &live);
        assert_eq!(t["status"], "ok");
        assert_eq!(t["wrapped"], false);
        assert_eq!(t["authoriserLabel"], "n/a"); // no receipt vault, no authoriser
                                                 // a plain token whose on-chain decimals disagree is still a mismatch
        let bad = TokenLive {
            decimals: Some(18),
            ..live
        };
        assert_eq!(
            token_health(&spec("USDC", "USD Coin", 6, "0x8335", None), &bad)["status"],
            "mismatch"
        );
    }

    #[test]
    fn token_unknown_when_core_read_fails() {
        let t = token_health(
            &spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271")),
            &TokenLive::default(),
        );
        assert_eq!(t["status"], "unknown");
    }

    const AUTH: &str = "0x315b16faa6ee413fabca877d3851b3818369f0cd";

    #[test]
    fn token_authoriser_is_resolved_against_the_pin_without_affecting_status() {
        let mut s = spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271"));
        s.auth_pinned = Some(AUTH);
        let with = |a: Option<&str>| {
            let mut l = tl(
                Some("N"),
                Some("wtNVDA"),
                Some(18),
                Some("0x7271"),
                Some(true),
            );
            l.authoriser = a.map(str::to_string);
            token_health(&s, &l)
        };
        let t = with(Some(&AUTH.to_uppercase().replace("0X", "0x")));
        assert_eq!(
            (
                t["authoriserLabel"].as_str(),
                t["atPinnedAuthoriser"].as_bool()
            ),
            (Some("pinned"), Some(true))
        );
        assert_eq!(
            (t["authoriserPinned"].as_str(), t["status"].as_str()),
            (Some(AUTH), Some("ok"))
        );
        let f = with(Some("0xdead000000000000000000000000000000000001"));
        assert_eq!(
            (
                f["authoriserLabel"].as_str(),
                f["atPinnedAuthoriser"].as_bool()
            ),
            (Some("foreign"), Some(false))
        );
        assert_eq!(f["status"], "ok");
        assert_eq!(with(None)["authoriserLabel"], "unknown");
        let plain = token_health(
            &spec("USDC", "U", 6, "0x1", None),
            &tl(Some("U"), Some("USDC"), Some(6), None, None),
        );
        assert_eq!(
            (
                plain["authoriserLabel"].as_str(),
                plain["atPinnedAuthoriser"].is_null()
            ),
            (Some("n/a"), true)
        );
    }

    #[test]
    fn token_carries_migration_membership_both_directions() {
        // a wrapped token NOT in the migration set is flagged (won't be migrated)
        let mut s = spec("wtX", "X", 18, "0xaddr", Some("0xunw"));
        s.in_migration = Some(false);
        let live = tl(Some("X"), Some("wtX"), Some(18), Some("0xunw"), Some(true));
        assert_eq!(token_health(&s, &live)["inMigrationSet"], false);
        // a plain token has no vault → not applicable (null)
        let plain = token_health(
            &spec("USDC", "USD Coin", 6, "0x8335", None),
            &TokenLive {
                name: Some("USD Coin".into()),
                symbol: Some("USDC".into()),
                decimals: Some(6),
                ..Default::default()
            },
        );
        assert_eq!(plain["inMigrationSet"], serde_json::Value::Null);
    }

    /// An unreadable chain is REPORTED, not dropped. A dropped chain renders
    /// as no section, which reads as "this chain has no production beacons" —
    /// the same silent vanish that hid the token registry.
    #[test]
    fn beacons_unavailable_names_the_chain_and_the_reason() {
        let v = beacons_unavailable(
            "ethereum",
            "ethereum-rpc.publicnode.com",
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

    #[test]
    fn build_tokens_counts_ok_and_sorts_by_symbol() {
        let ok = token_health(
            &spec("wtZ", "Z", 18, "0x2", Some("0xu")),
            &tl(Some("Z"), Some("wtZ"), Some(18), Some("0xu"), Some(true)),
        );
        let bad = token_health(
            &spec("wtA", "A", 18, "0x1", Some("0xu")),
            &tl(
                Some("A"),
                Some("wtWRONG"),
                Some(18),
                Some("0xu"),
                Some(true),
            ),
        );
        let v = build_tokens(
            "o",
            "r",
            "base",
            "host",
            json!(null),
            json!(null),
            vec![ok, bad],
        )
        .unwrap();
        assert_eq!(v["total"], 2);
        assert_eq!(v["ok"], 1);
        assert_eq!(v["tokens"][0]["symbol"], "wtA");
    }

    #[test]
    fn build_tokens_counts_vaults_at_the_pinned_authoriser() {
        let mut s = spec("wtA", "A", 18, "0x1", Some("0xu"));
        s.auth_pinned = Some(AUTH);
        let mut l = tl(Some("A"), Some("wtA"), Some(18), Some("0xu"), Some(true));
        let off = token_health(&s, &l);
        l.authoriser = Some(AUTH.into());
        let on = token_health(&s, &l);
        let v = build_tokens(
            "o",
            "r",
            "base",
            "host",
            json!(null),
            json!(null),
            vec![on, off],
        )
        .unwrap();
        assert_eq!(
            (v["wrappedCount"].as_u64(), v["atPinnedAuthoriser"].as_u64()),
            (Some(2), Some(1))
        );
    }

    #[test]
    fn resolve_authoriser_labels_pinned_none_foreign_unknown() {
        let pinned = Some(AUTH);
        assert_eq!(resolve_authoriser(pinned, pinned), ("pinned", Some(true)));
        assert_eq!(
            resolve_authoriser(Some("0x0000000000000000000000000000000000000000"), pinned),
            ("none", Some(false))
        );
        assert_eq!(
            resolve_authoriser(Some("0x0000000000000000000000000000000000000001"), pinned),
            ("foreign", Some(false))
        );
        assert_eq!(resolve_authoriser(None, pinned), ("unknown", None));
        assert_eq!(resolve_authoriser(pinned, None), ("foreign", None));
    }

    #[test]
    fn extra_vault_carries_identity_and_the_pinned_authoriser() {
        let v = extra_vault(
            "0x3c0f",
            Some("Wrapped iShares iBonds ST0x".into()),
            Some("wtIBHG".into()),
            Some(true),
            Some("0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456"),
            Some(AUTH),
        );
        assert_eq!(
            (v["symbol"].as_str(), v["deployed"].as_bool()),
            (Some("wtIBHG"), Some(true))
        );
        assert_eq!(
            (
                v["authoriserLabel"].as_str(),
                v["atPinnedAuthoriser"].as_bool()
            ),
            (Some("foreign"), Some(false))
        );
        assert_eq!(v["authoriserPinned"], AUTH);
    }

    #[test]
    fn pointer_suites_group_every_generated_dir_and_skip_the_rest() {
        let s = |p: &str| (p.to_string(), format!("// {p}"));
        let sources = vec![
            s("src/generated/candidate/StoxReceipt.pointers.sol"),
            s("src/generated/0_1_30/ST0xOrchestrator.pointers.sol"),
            s("src/generated/0_1_1/StoxReceiptVault.sol"),
            s("src/generated/0_1_1/StoxReceipt.pointers.sol"),
            s("src/generated/LibProdDeployV4.sol"),
            s("src/generated/0_1_1/nested/X.sol"),
            s("src/generated/0_1_1/README.md"),
            s("src/lib/LibX.sol"),
        ];
        let got: Vec<(String, Vec<String>)> = pointer_suites(&sources)
            .into_iter()
            .map(|(suite, fs)| (suite, fs.into_iter().map(|(c, _)| c).collect()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "0.1.1".to_string(),
                    vec!["StoxReceipt".to_string(), "StoxReceiptVault".to_string()]
                ),
                ("0.1.30".to_string(), vec!["ST0xOrchestrator".to_string()]),
                ("candidate".to_string(), vec!["StoxReceipt".to_string()]),
            ]
        );
        assert_eq!(
            pointer_suites(&sources)[0].1[0].1,
            "// src/generated/0_1_1/StoxReceipt.pointers.sol"
        );
    }

    #[test]
    fn deployed_address_reads_only_the_deployed_address_literal() {
        let src = r#"
            address constant OTHER = address(0x1111111111111111111111111111111111111111);
            address constant DEPLOYED_ADDRESS = address(0xAbCdEf0000000000000000000000000000000001);
        "#;
        assert_eq!(
            deployed_address(src).as_deref(),
            Some("0xAbCdEf0000000000000000000000000000000001")
        );
        assert_eq!(
            deployed_address(
                "address constant X = address(0x1111111111111111111111111111111111111111);"
            ),
            None
        );
        assert_eq!(deployed_address("not solidity {"), None);
    }
}
