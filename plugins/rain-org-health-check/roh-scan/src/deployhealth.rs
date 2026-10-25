//! On-chain HEALTH of a pinned deployment suite: for each contract, is it
//! deployed at its pinned address, and does the live code match BOTH pins — the
//! exact `RUNTIME_CODE` bytes AND the `BYTECODE_HASH` keccak (EXTCODEHASH). Pure
//! parsing + comparison live here and are unit-tested; the RPC fetch is in main.
//!
//! Checking both cross-validates the two pins against each other and against the
//! chain: a `RUNTIME_CODE` that disagrees with its own `BYTECODE_HASH`, or either
//! disagreeing with the deployed code, is a finding.

use crate::expected::{ExpectedChain, ExpectedError, EXPECTED_STATE_PATH};
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
    /// The current prod authoriser a receipt vault should point at today.
    pub auth_current: Option<&'a str>,
    /// The authoriser st0x.deploy's CI expects for this token (`expected-state.json`).
    pub auth_target: Option<&'a str>,
    /// Whether this token's receipt vault is in the migration's governed set
    /// (the Base `receiptVaults` of `privileged-addresses.json`): `Some(true/false)` for a wrapped token,
    /// `None` for a plain token that has no vault to govern.
    pub in_migration: Option<bool>,
}

/// Resolve a live authoriser address against the current prod authoriser and the
/// expected authoriser → (`label`, `atTarget`). `label`: `target` (the expected one),
/// `current` (the pre-migration authoriser), `none` (address(0)), `foreign`
/// (something else), or `unknown` (unread).
pub fn resolve_authoriser(
    live: Option<&str>,
    current: Option<&str>,
    target: Option<&str>,
) -> (&'static str, Option<bool>) {
    let zero = "0x0000000000000000000000000000000000000000";
    let label = match live {
        None => "unknown",
        Some(a) if target.is_some_and(|t| a.eq_ignore_ascii_case(t)) => "target",
        Some(a) if current.is_some_and(|c| a.eq_ignore_ascii_case(c)) => "current",
        Some(a) if a.eq_ignore_ascii_case(zero) => "none",
        Some(_) => "foreign",
    };
    let at_target = match (live, target) {
        (Some(a), Some(t)) => Some(a.eq_ignore_ascii_case(t)),
        _ => None,
    };
    (label, at_target)
}

/// The Tokens section's authoriser summary: the current prod authoriser, and
/// each distinct authoriser `expected-state.json` routes Base tokens to with
/// whether it is deployed. `error` says why the expectation is unavailable.
pub fn authoriser_summary(
    current: Option<&str>,
    expected: Result<&&ExpectedChain, &ExpectedError>,
    deployed: impl Fn(&str) -> Option<bool>,
) -> serde_json::Value {
    let mut targets: Vec<&str> = Vec::new();
    if let Ok(c) = expected {
        for (_, a) in &c.authorisers {
            if !targets.iter().any(|t| t.eq_ignore_ascii_case(a)) {
                targets.push(a);
            }
        }
    }
    json!({
        "current": current,
        "source": EXPECTED_STATE_PATH,
        "error": expected.err().map(|e| e.to_string()),
        "targets": targets
            .iter()
            .map(|t| json!({"address": t, "deployed": deployed(t)}))
            .collect::<Vec<_>>(),
    })
}

/// A governed receipt vault that is in the migration set
/// (the Base `receiptVaults` of `privileged-addresses.json`) but NOT matched to any registry token — surfaced
/// with its on-chain identity and authoriser now-vs-target so the setAuthorizer
/// bundle can be reviewed in full even for vaults the registry doesn't list.
pub fn extra_vault(
    address: &str,
    name: Option<String>,
    symbol: Option<String>,
    deployed: Option<bool>,
    live_authoriser: Option<&str>,
    auth_current: Option<&str>,
    auth_target: Option<&str>,
) -> serde_json::Value {
    let (label, at_target) = resolve_authoriser(live_authoriser, auth_current, auth_target);
    json!({
        "address": address,
        "name": name,
        "symbol": symbol,
        "deployed": deployed,
        "authoriser": live_authoriser,
        "authoriserLabel": label,
        "authoriserTarget": auth_target,
        "atAuthoriserTarget": at_target,
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
/// The authoriser (read from the receipt vault) is resolved to WHO it is — the
/// `current` prod authoriser, the expected `target`, `none`, or `foreign` — and
/// reported NOW vs. target so a proposed `setAuthorizer` migration can be checked.
/// It does NOT affect `status`: sitting at the current authoriser pre-migration is
/// correct, not an error.
pub fn token_health(spec: &TokenSpec, live: &TokenLive) -> serde_json::Value {
    let TokenSpec {
        symbol,
        name,
        decimals,
        address,
        unwrapped,
        legacy,
        receipt,
        auth_current,
        auth_target,
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
    // already sits at the authoriser st0x.deploy expects for it.
    let (authoriser_label, at_auth_target) = if wrapped {
        resolve_authoriser(live.authoriser.as_deref(), auth_current, auth_target)
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
        // Authoriser provenance — for reviewing the setAuthorizer migration: what
        // it is NOW (resolved to current / target / none / foreign) vs. the target.
        "authoriser": live.authoriser,
        "authoriserLabel": authoriser_label,
        "authoriserTarget": auth_target,
        "atAuthoriserTarget": at_auth_target,
        // Registry→migration cross-check: is this token's receipt vault in the
        // migration's governed set? A wrapped token with `false` won't be migrated.
        "inMigrationSet": in_migration,
    })
}

/// Assemble the `deploymentTokens` document (sorted by symbol; `None` if empty).
/// `authoriser` is the section-level authoriser summary (the current authoriser
/// and the expected targets with whether each is deployed), so the page can
/// count how many vaults already sit at their expected authoriser.
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
    // How many wrapped tokens already sit at the authoriser target vs. total
    // wrapped — so the page can show migration progress.
    let wrapped = tokens.iter().filter(|t| t["wrapped"] == true).count();
    let at_target = tokens
        .iter()
        .filter(|t| t["atAuthoriserTarget"] == true)
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
        "atAuthoriserTarget": at_target,
        // Cross-check of the registry against the migration's authoritative vault
        // set (Base `receiptVaults` in `privileged-addresses.json`): extra governed vaults not in the registry,
        // and registry vaults not governed by the migration.
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

    // A minimal wrapped-token spec (no legacy/receipt/authoriser targets).
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
            auth_current: None,
            auth_target: None,
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

    #[test]
    fn token_authoriser_resolved_now_vs_target_without_affecting_status() {
        let cur = "0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456";
        let tgt = "0x315b16faa6ee413fabca877d3851b3818369f0cd";
        let mut s = spec("wtNVDA", "N", 18, "0xfb5b", Some("0x7271"));
        s.auth_current = Some(cur);
        s.auth_target = Some(tgt);
        let base = || {
            tl(
                Some("N"),
                Some("wtNVDA"),
                Some(18),
                Some("0x7271"),
                Some(true),
            )
        };

        // pre-migration: at the current prod authoriser → labelled `current`, NOT
        // at target, and the token is still `ok` (this is the correct pre-state).
        let mut at_current = base();
        at_current.authoriser = Some(cur.to_string());
        let t = token_health(&s, &at_current);
        assert_eq!(t["authoriserLabel"], "current");
        assert_eq!(t["atAuthoriserTarget"], false);
        assert_eq!(t["authoriserTarget"], tgt);
        assert_eq!(t["status"], "ok");

        // migrated: at the V4 clone → `target`, atAuthoriserTarget true.
        let mut at_target = base();
        at_target.authoriser = Some(tgt.to_string());
        let t2 = token_health(&s, &at_target);
        assert_eq!(t2["authoriserLabel"], "target");
        assert_eq!(t2["atAuthoriserTarget"], true);

        // an unexpected authoriser → `foreign`; a failed read → `unknown`.
        let mut foreign = base();
        foreign.authoriser = Some("0xdead000000000000000000000000000000000001".into());
        assert_eq!(token_health(&s, &foreign)["authoriserLabel"], "foreign");
        assert_eq!(token_health(&s, &base())["authoriserLabel"], "unknown");
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
    fn resolve_authoriser_labels_current_target_none_foreign_unknown() {
        let cur = Some("0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456");
        let tgt = Some("0x315b16faa6ee413fabca877d3851b3818369f0cd");
        assert_eq!(resolve_authoriser(cur, cur, tgt), ("current", Some(false)));
        assert_eq!(resolve_authoriser(tgt, cur, tgt), ("target", Some(true)));
        assert_eq!(
            resolve_authoriser(Some("0x0000000000000000000000000000000000000000"), cur, tgt),
            ("none", Some(false))
        );
        assert_eq!(resolve_authoriser(Some("0xdead"), cur, tgt).0, "foreign");
        assert_eq!(resolve_authoriser(None, cur, tgt), ("unknown", None));
    }

    #[test]
    fn extra_vault_carries_identity_and_authoriser_target() {
        let cur = "0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456";
        let tgt = "0x315b16faa6ee413fabca877d3851b3818369f0cd";
        let v = extra_vault(
            "0x3c0f",
            Some("Wrapped iShares iBonds ST0x".into()),
            Some("wtIBHG".into()),
            Some(true),
            Some(cur),
            Some(cur),
            Some(tgt),
        );
        assert_eq!(v["symbol"], "wtIBHG");
        assert_eq!(v["authoriserLabel"], "current");
        assert_eq!(v["atAuthoriserTarget"], false);
        assert_eq!(v["authoriserTarget"], tgt);
    }

    fn exp_chain(authorisers: &[(&str, &str)]) -> ExpectedChain {
        ExpectedChain {
            chain_id: 8453,
            network: "base".into(),
            beacon_set_deployer: None,
            beacons: vec![],
            safe: crate::expected::ExpectedSafe {
                address: SAFE.into(),
                owners: vec![SAFE.into()],
                threshold: 1,
            },
            authorisers: authorisers
                .iter()
                .map(|(v, a)| (v.to_string(), a.to_string()))
                .collect(),
        }
    }

    /// #218: one target per distinct expected authoriser, not one V4 clone.
    #[test]
    fn authoriser_summary_lists_each_distinct_expected_target() {
        const EU: &str = "0xDB9152e46c1D140DB6a6f814461f21c571F3275e";
        const US: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
        let c = exp_chain(&[("0x01", US), ("0x02", EU), ("0x03", &US.to_lowercase())]);
        let v = authoriser_summary(Some("0xc1"), Ok(&&c), |a| Some(a == EU));
        assert_eq!(v["current"], "0xc1");
        assert_eq!(v["source"], "expected-state.json");
        assert!(v["error"].is_null());
        assert_eq!(
            v["targets"],
            json!([{"address": US, "deployed": false}, {"address": EU, "deployed": true}])
        );
        assert!(v.get("target").is_none());
    }

    #[test]
    fn authoriser_summary_without_an_expectation_says_why() {
        let v = authoriser_summary(Some("0xc1"), Err(&ExpectedError::Empty), |_| {
            panic!("nothing to probe")
        });
        assert_eq!(v["error"], "expected-state.json could not be fetched");
        assert_eq!(v["targets"], json!([]));
    }
}
