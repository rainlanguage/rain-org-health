//! On-chain HEALTH of a pinned deployment suite: for each contract, is it
//! deployed at its pinned address, and does the live code match BOTH pins — the
//! exact `RUNTIME_CODE` bytes AND the `BYTECODE_HASH` keccak (EXTCODEHASH). Pure
//! parsing + comparison live here and are unit-tested; the RPC fetch is in main.
//!
//! Checking both cross-validates the two pins against each other and against the
//! chain: a `RUNTIME_CODE` that disagrees with its own `BYTECODE_HASH`, or either
//! disagreeing with the deployed code, is a finding.

use crate::expected::{
    ExpectedChain, ExpectedError, ExpectedQsep, ExpectedToken, ExpectedV2, ExpectedZoltu,
    EXPECTED_STATE_PATH,
};
use crate::migrations::RenameUnlocated;
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
/// owner st0x.deploy emits in `expected-state.json` (the chain's timelock)
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
///   implementation; `drift` on any other read owner, whatever the
///   implementation; else `error` when the target implementation could not be
///   resolved; `unknown` when a live read failed; `behind` on another
///   implementation.
///
/// A Safe-owned beacon is drift: a beacon owner can repoint every production
/// proxy in one transaction, so a beacon on the Safe bypasses the timelock delay.
pub fn beacon_health(
    name: &str,
    address: Option<String>,
    owners: &BeaconOwners,
    target_impl: Result<Option<&str>, &RenameUnlocated>,
    v1_impl: Option<&str>,
    live_owner: Option<String>,
    live_impl: Option<String>,
) -> serde_json::Value {
    let is = |o: &str, pin: Option<&str>| pin.is_some_and(|p| o.eq_ignore_ascii_case(p));
    // An all-zero Solidity constant would match a beacon answering the zero address.
    let is_const = |o: &str, pin: Option<&str>| is(o, pin.filter(|p| hydrated_pin(p)));
    let owner_label = match live_owner.as_deref() {
        None => "unknown",
        Some(o) if is(o, Some(owners.expected)) => "timelock",
        Some(o) if is(o, owners.safe) => "safe",
        Some(o) if is_const(o, owners.legacy) => "legacy",
        Some(_) => "foreign",
    };
    let reason = target_impl.err().map(|e| e.to_string());
    let target_impl = target_impl.ok().flatten();
    let at_target = match (live_impl.as_deref(), target_impl) {
        (Some(l), Some(t)) => Some(l.eq_ignore_ascii_case(t)),
        _ => None,
    };
    let impl_version = match live_impl.as_deref() {
        None => "unknown",
        Some(_) if at_target == Some(true) => "expected",
        Some(l) if is_const(l, v1_impl) => "V1",
        Some(_) => "unknown",
    };
    let status = if live_owner.is_some() && owner_label != "timelock" {
        "drift"
    } else if reason.is_some() {
        "error"
    } else if live_owner.is_none() || at_target.is_none() {
        "unknown"
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
        "targetImpl": target_impl,
        "targetOwner": owners.expected,
        "atTarget": at_target,
        "status": status,
        "reason": reason,
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
        "safeOwner": safe,
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

/// One chain's infra read live; `None` is a failed read, a codehash
/// `Some("none")` no code. `beacon_codehashes` follows `ExpectedChain.beacons`.
#[derive(Debug, Clone, Default)]
pub struct InfraLive {
    pub beacon_set_deployer_has_code: Option<String>,
    pub beacon_codehashes: Vec<Option<String>>,
    pub timelock_codehash: Option<String>,
    pub timelock_min_delay: Option<String>,
    pub orchestrator_set_deployer_has_code: Option<String>,
    pub orchestrator_beacon_codehash: Option<String>,
    pub orchestrator_beacon_implementation: Option<String>,
    pub orchestrator_beacon_owner: Option<String>,
    pub orchestrator_instance_beacon: Option<String>,
    pub authoriser_codehash: Option<String>,
    pub eu_authoriser_codehash: Option<String>,
}

/// Every infra row `c` pins, against `live`.
pub fn infra_rows(c: &ExpectedChain, live: InfraLive) -> Vec<serde_json::Value> {
    let mut rows = vec![infra_row(
        "beaconSetDeployer",
        "hasCode",
        Some(&c.beacon_set_deployer),
        Some("true"),
        live.beacon_set_deployer_has_code,
    )];
    let mut beacon_codehashes = live.beacon_codehashes.into_iter();
    for b in &c.beacons {
        rows.push(infra_row(
            b.name,
            "codehash",
            Some(&b.address),
            Some(&b.codehash),
            beacon_codehashes.next().flatten(),
        ));
    }
    let tl = &c.timelock.code.address;
    let min_delay = c.timelock.min_delay.to_string();
    let o = &c.orchestrator;
    rows.extend([
        infra_row(
            "timelock",
            "codehash",
            Some(tl),
            Some(&c.timelock.code.codehash),
            live.timelock_codehash,
        ),
        infra_row(
            "timelock",
            "minDelay",
            Some(tl),
            Some(&min_delay),
            live.timelock_min_delay,
        ),
        infra_row(
            "orchestrator",
            "setDeployer hasCode",
            Some(&o.set_deployer),
            Some("true"),
            live.orchestrator_set_deployer_has_code,
        ),
        infra_row(
            "orchestrator",
            "beacon codehash",
            Some(&o.beacon),
            Some(&o.beacon_codehash),
            live.orchestrator_beacon_codehash,
        ),
        infra_row(
            "orchestrator",
            "beacon implementation",
            Some(&o.beacon),
            Some(&o.implementation),
            live.orchestrator_beacon_implementation,
        ),
        infra_row(
            "orchestrator",
            "beacon owner",
            Some(&o.beacon),
            Some(&o.owner),
            live.orchestrator_beacon_owner,
        ),
        infra_row(
            "orchestrator",
            "instance beacon",
            Some(&o.instance),
            Some(&o.beacon),
            live.orchestrator_instance_beacon,
        ),
        infra_row(
            "authoriser",
            "codehash",
            Some(&c.authoriser.address),
            Some(&c.authoriser.codehash),
            live.authoriser_codehash,
        ),
        infra_row(
            "euAuthoriser",
            "codehash",
            Some(&c.eu_authoriser.address),
            Some(&c.eu_authoriser.codehash),
            live.eu_authoriser_codehash,
        ),
    ]);
    rows
}

/// One Zoltu suite's codehash row; `live` as for [`infra_row`].
pub fn zoltu_row(z: &ExpectedZoltu, live: Option<String>) -> serde_json::Value {
    infra_row(
        &format!("zoltu {}", z.key),
        "codehash",
        Some(&z.code.address),
        Some(z.code.codehash.as_str()),
        live,
    )
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
    pub receipt_manager: Option<String>,
    pub receipt_beacon: Option<String>,
    pub receipt_codehash: Option<String>,
    pub wrapped_token_vault_name: Option<String>,
    pub wrapped_token_vault_symbol: Option<String>,
    pub wrapped_token_vault_decimals: Option<u8>,
    pub wrapped_token_vault_asset: Option<String>,
    pub wrapped_token_vault_beacon: Option<String>,
    pub wrapped_token_vault_codehash: Option<String>,
}

/// The pins a token and qsep share.
pub struct Legs<'a> {
    pub receipt_vault: &'a str,
    pub receipt: &'a str,
    pub wrapped_token_vault: &'a str,
    pub receipt_vault_receipt: &'a str,
    pub receipt_manager: &'a str,
    pub wrapped_token_vault_asset: &'a str,
    pub receipt_vault_beacon: &'a str,
    pub receipt_beacon: &'a str,
    pub wrapped_token_vault_beacon: &'a str,
    pub receipt_vault_codehash: &'a str,
    pub receipt_codehash: &'a str,
    pub wrapped_token_vault_codehash: &'a str,
    pub owner: &'a str,
    pub authoriser: &'a str,
}

impl<'a> From<&'a ExpectedToken> for Legs<'a> {
    fn from(t: &'a ExpectedToken) -> Self {
        Legs {
            receipt_vault: &t.receipt_vault,
            receipt: &t.receipt,
            wrapped_token_vault: &t.wrapped_token_vault,
            receipt_vault_receipt: &t.receipt_vault_receipt,
            receipt_manager: &t.receipt_manager,
            wrapped_token_vault_asset: &t.wrapped_token_vault_asset,
            receipt_vault_beacon: &t.receipt_vault_beacon,
            receipt_beacon: &t.receipt_beacon,
            wrapped_token_vault_beacon: &t.wrapped_token_vault_beacon,
            receipt_vault_codehash: &t.receipt_vault_codehash,
            receipt_codehash: &t.receipt_codehash,
            wrapped_token_vault_codehash: &t.wrapped_token_vault_codehash,
            owner: &t.owner,
            authoriser: &t.authoriser,
        }
    }
}

impl<'a> From<&'a ExpectedQsep> for Legs<'a> {
    fn from(q: &'a ExpectedQsep) -> Self {
        Legs {
            receipt_vault: &q.receipt_vault,
            receipt: &q.receipt,
            wrapped_token_vault: &q.wrapped_token_vault,
            receipt_vault_receipt: &q.receipt_vault_receipt,
            receipt_manager: &q.receipt_manager,
            wrapped_token_vault_asset: &q.wrapped_token_vault_asset,
            receipt_vault_beacon: &q.receipt_vault_beacon,
            receipt_beacon: &q.receipt_beacon,
            wrapped_token_vault_beacon: &q.wrapped_token_vault_beacon,
            receipt_vault_codehash: &q.receipt_vault_codehash,
            receipt_codehash: &q.receipt_codehash,
            wrapped_token_vault_codehash: &q.wrapped_token_vault_codehash,
            owner: &q.owner,
            authoriser: &q.authoriser,
        }
    }
}

/// The leg rows a token and qsep share, under `id`.
pub fn leg_rows(id: &str, l: Legs, live: &TokenLive) -> Vec<serde_json::Value> {
    let pin = |p: &str| Some(p.to_string());
    let checks: [(&str, &str, &str, &Option<String>); 11] = [
        (
            "receiptVault receipt",
            l.receipt_vault,
            l.receipt_vault_receipt,
            &live.receipt_vault_receipt,
        ),
        (
            "receiptVault owner",
            l.receipt_vault,
            l.owner,
            &live.receipt_vault_owner,
        ),
        (
            "receiptVault authoriser",
            l.receipt_vault,
            l.authoriser,
            &live.receipt_vault_authoriser,
        ),
        (
            "receiptVault beacon",
            l.receipt_vault,
            l.receipt_vault_beacon,
            &live.receipt_vault_beacon,
        ),
        (
            "receiptVault codehash",
            l.receipt_vault,
            l.receipt_vault_codehash,
            &live.receipt_vault_codehash,
        ),
        (
            "receipt manager",
            l.receipt,
            l.receipt_manager,
            &live.receipt_manager,
        ),
        (
            "receipt beacon",
            l.receipt,
            l.receipt_beacon,
            &live.receipt_beacon,
        ),
        (
            "receipt codehash",
            l.receipt,
            l.receipt_codehash,
            &live.receipt_codehash,
        ),
        (
            "wrappedTokenVault asset",
            l.wrapped_token_vault,
            l.wrapped_token_vault_asset,
            &live.wrapped_token_vault_asset,
        ),
        (
            "wrappedTokenVault beacon",
            l.wrapped_token_vault,
            l.wrapped_token_vault_beacon,
            &live.wrapped_token_vault_beacon,
        ),
        (
            "wrappedTokenVault codehash",
            l.wrapped_token_vault,
            l.wrapped_token_vault_codehash,
            &live.wrapped_token_vault_codehash,
        ),
    ];
    checks
        .into_iter()
        .map(|(field, address, expected, live)| {
            infra_row(
                id,
                field,
                Some(address),
                pin(expected).as_deref(),
                live.clone(),
            )
        })
        .collect()
}

type RowExpected<'a> = Result<Option<String>, &'a RenameUnlocated>;

/// One token's rows. `renamed` is whether the chain records the file's
/// `renameMigration` (`Ok(None)`: unread, so the renamable rows are unknown;
/// `Err`: the renamable rows are `error`).
pub fn token_rows(
    t: &ExpectedToken,
    renamed: Result<Option<bool>, &RenameUnlocated>,
    live: TokenLive,
) -> Vec<serde_json::Value> {
    let pick = |name: &str, renamed_name: &str| {
        renamed.map(|r| r.map(|r| if r { renamed_name } else { name }.to_string()))
    };
    let rv = t.receipt_vault.as_str();
    let wv = t.wrapped_token_vault.as_str();
    let id = format!("token {}", t.symbol);
    let checks: [(&str, &str, RowExpected, Option<String>); 6] = [
        (
            "receiptVault name",
            rv,
            pick(&t.name, &t.renamed_name),
            live.receipt_vault_name.clone(),
        ),
        (
            "receiptVault symbol",
            rv,
            Ok(Some(t.symbol.clone())),
            live.receipt_vault_symbol.clone(),
        ),
        (
            "receiptVault decimals",
            rv,
            Ok(Some(t.decimals.to_string())),
            live.receipt_vault_decimals.map(|d| d.to_string()),
        ),
        (
            "wrappedTokenVault name",
            wv,
            pick(
                &t.wrapped_token_vault_name,
                &t.wrapped_token_vault_renamed_name,
            ),
            live.wrapped_token_vault_name.clone(),
        ),
        (
            "wrappedTokenVault symbol",
            wv,
            pick(
                &t.wrapped_token_vault_symbol,
                &t.wrapped_token_vault_renamed_symbol,
            ),
            live.wrapped_token_vault_symbol.clone(),
        ),
        (
            "wrappedTokenVault decimals",
            wv,
            Ok(Some(t.wrapped_token_vault_decimals.to_string())),
            live.wrapped_token_vault_decimals.map(|d| d.to_string()),
        ),
    ];
    let mut rows: Vec<serde_json::Value> = checks
        .into_iter()
        .map(|(field, address, expected, live)| match expected {
            Ok(e) => infra_row(&id, field, Some(address), e.as_deref(), live),
            Err(e) => {
                let mut row = infra_row(&id, field, Some(address), None, live);
                row["status"] = json!("error");
                row["reason"] = json!(e.to_string());
                row
            }
        })
        .collect();
    rows.extend(leg_rows(&id, t.into(), &live));
    rows
}

/// The qsep's rows.
pub fn qsep_rows(q: &ExpectedQsep, live: &TokenLive) -> Vec<serde_json::Value> {
    leg_rows("qsep", q.into(), live)
}

/// One v2 beacon's live `implementation()` and `owner()`.
pub struct V2BeaconLive {
    pub implementation: Option<String>,
    pub owner: Option<String>,
}

/// One chain's v2 rows; `codehashes` and `beacons` are the live reads in the
/// order of `v.deployments` and `v.beacons`.
pub fn v2_rows(
    v: &ExpectedV2,
    codehashes: Vec<Option<String>>,
    beacons: Vec<V2BeaconLive>,
) -> Vec<serde_json::Value> {
    let mut rows: Vec<serde_json::Value> = v
        .deployments
        .iter()
        .zip(codehashes)
        .map(|(z, live)| zoltu_row(z, live))
        .collect();
    for (b, live) in v.beacons.iter().zip(beacons) {
        let a = Some(b.address.as_str());
        rows.push(infra_row(
            b.name,
            "implementation",
            a,
            Some(&b.implementation),
            live.implementation,
        ));
        rows.push(infra_row(b.name, "owner", a, Some(&b.owner), live.owner));
    }
    rows
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

pub const REGISTRY_ORG: &str = "ST0x-Technology";
pub const REGISTRY_REPO: &str = "st0x.registry";

pub fn registry_list_path(network: &str) -> String {
    format!("token-lists/{network}.json")
}

/// Per expected-state chain, its st0x.registry token list against its governed
/// receipt vaults, both ways. `list` returns a path's body ("" if unreadable).
pub fn registry_reconcile(
    chains: &Result<Vec<ExpectedChain>, ExpectedError>,
    list: impl Fn(&str) -> String,
) -> Vec<serde_json::Value> {
    let chains = match chains {
        Err(e) => {
            return vec![json!({
                "registryOrg": REGISTRY_ORG,
                "registryRepo": REGISTRY_REPO,
                "expectedState": EXPECTED_STATE_PATH,
                "unavailable": true,
                "reason": e.to_string(),
            })]
        }
        Ok(chains) => chains,
    };
    chains
        .iter()
        .map(|c| {
            let path = registry_list_path(&c.network);
            let mut doc = json!({
                "network": c.network,
                "registryOrg": REGISTRY_ORG,
                "registryRepo": REGISTRY_REPO,
                "registryPath": path,
                "expectedState": EXPECTED_STATE_PATH,
            });
            let body = list(&path);
            let tokens = if body.is_empty() {
                Err(format!("no readable registry list at {path}"))
            } else {
                serde_json::from_str::<serde_json::Value>(&body)
                    .map_err(|_| format!("{path} is not valid JSON"))
                    .and_then(|v| {
                        v["tokens"]
                            .as_array()
                            .cloned()
                            .ok_or_else(|| format!("{path} has no tokens array"))
                    })
            };
            let tokens = match tokens {
                Err(reason) => {
                    doc["unavailable"] = json!(true);
                    doc["reason"] = json!(reason);
                    return doc;
                }
                Ok(t) => t,
            };
            let vault = |t: &serde_json::Value| {
                t["extensions"]["unwrappedAddress"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase)
            };
            let governed: std::collections::HashSet<String> = c
                .tokens
                .iter()
                .map(|t| t.receipt_vault.to_lowercase())
                .collect();
            let registry: std::collections::HashSet<String> =
                tokens.iter().filter_map(vault).collect();
            let missing: Vec<serde_json::Value> = tokens
                .iter()
                .filter_map(|t| {
                    let v = vault(t);
                    if v.as_ref().is_some_and(|v| governed.contains(v)) {
                        return None;
                    }
                    let reason = if v.is_none() {
                        "no receipt vault (collateral)"
                    } else {
                        "receipt vault not in expected-state"
                    };
                    Some(json!({
                        "symbol": t["symbol"],
                        "name": t["name"],
                        "address": t["address"],
                        "receiptVault": v,
                        "wrapped": v.is_some(),
                        "reason": reason,
                    }))
                })
                .collect();
            let extra: Vec<serde_json::Value> = c
                .tokens
                .iter()
                .filter(|t| !registry.contains(&t.receipt_vault.to_lowercase()))
                .map(|t| json!({ "symbol": t.symbol, "address": t.receipt_vault }))
                .collect();
            doc["governedCount"] = json!(governed.len());
            doc["registryTokenCount"] = json!(tokens.len());
            doc["missingFromMigration"] = json!(missing);
            doc["extraVaults"] = json!(extra);
            doc
        })
        .collect()
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
            Ok(Some(target)),
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

    /// The expected owner is what expected-state.json pins, so it wins over the Safe label
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

    /// An unhydrated Solidity constant is a placeholder, not an address: a
    /// beacon answering the zero address is never labelled legacy or V1 by one,
    /// in any spelling.
    #[test]
    fn an_unhydrated_constant_never_labels_a_zero_owner_or_impl() {
        for spelling in [
            ZERO,
            "0X0000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000",
            "",
        ] {
            let zeros = BeaconOwners {
                legacy: Some(spelling),
                ..OWNERS
            };
            let b = beacon_health(
                "Receipt beacon",
                Some("0x86e9".into()),
                &zeros,
                Ok(Some(TARGET)),
                Some(spelling),
                Some(ZERO.into()),
                Some(ZERO.into()),
            );
            assert_eq!(b["ownerLabel"], "foreign", "{spelling:?}");
            assert_eq!(b["implVersion"], "unknown", "{spelling:?}");
            assert_eq!(b["status"], "drift");
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
            Ok(Some(TARGET)),
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
        let v = build_beacons("o", "r", "base", "host", Some(SAFE), vec![a, b]).unwrap();
        assert!(v["timelockOwner"].is_null());
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

    const FIXTURE: &str = include_str!("../testdata/expected-state.json");
    const OTHER: &str = "0x35f9fa9d80aaf2b0fb27f0ff015641b3408d7456";

    fn base() -> crate::expected::ExpectedChain {
        crate::expected::parse(FIXTURE)
            .unwrap()
            .into_iter()
            .find(|c| c.network == "base")
            .unwrap()
    }

    fn base_v2() -> ExpectedV2 {
        crate::expected::parse_v2(FIXTURE)
            .unwrap()
            .into_iter()
            .find(|c| c.network == "base")
            .unwrap()
    }

    /// Every leg read live at its pin.
    fn matching(l: Legs) -> TokenLive {
        let s = |v: &str| Some(v.to_string());
        TokenLive {
            receipt_vault_receipt: s(l.receipt_vault_receipt),
            receipt_vault_owner: s(l.owner),
            receipt_vault_authoriser: s(l.authoriser),
            receipt_vault_beacon: s(l.receipt_vault_beacon),
            receipt_vault_codehash: s(l.receipt_vault_codehash),
            receipt_manager: s(l.receipt_manager),
            receipt_beacon: s(l.receipt_beacon),
            receipt_codehash: s(l.receipt_codehash),
            wrapped_token_vault_asset: s(l.wrapped_token_vault_asset),
            wrapped_token_vault_beacon: s(l.wrapped_token_vault_beacon),
            wrapped_token_vault_codehash: s(l.wrapped_token_vault_codehash),
            ..TokenLive::default()
        }
    }

    fn row<'a>(rows: &'a [serde_json::Value], field: &str) -> &'a serde_json::Value {
        rows.iter()
            .find(|r| r["field"] == field)
            .unwrap_or_else(|| panic!("no {field} row"))
    }

    const LEG_FIELDS: [&str; 11] = [
        "receiptVault receipt",
        "receiptVault owner",
        "receiptVault authoriser",
        "receiptVault beacon",
        "receiptVault codehash",
        "receipt manager",
        "receipt beacon",
        "receipt codehash",
        "wrappedTokenVault asset",
        "wrappedTokenVault beacon",
        "wrappedTokenVault codehash",
    ];

    /// Base's receipt-vault beacon is pinned to its renamed implementation once
    /// the chain records the rename, to the original before, and to neither
    /// while the rename is unread.
    #[test]
    fn a_base_beacon_is_checked_against_the_implementation_the_rename_picks() {
        let c = base();
        let b = &c.beacons[1];
        assert_ne!(b.implementation, b.renamed_implementation);
        let owners = BeaconOwners {
            expected: &b.owner,
            safe: Some(c.safe.address.as_str()),
            legacy: None,
        };
        let check = |renamed: Option<bool>, live_impl: &str| {
            beacon_health(
                b.name,
                Some(b.address.clone()),
                &owners,
                Ok(b.expected_implementation(renamed)),
                None,
                Some(b.owner.to_lowercase()),
                Some(live_impl.to_lowercase()),
            )
        };
        let renamed = check(Some(true), &b.renamed_implementation);
        assert_eq!(renamed["status"], "healthy");
        assert_eq!(renamed["implVersion"], "expected");
        assert_eq!(renamed["ownerLabel"], "timelock");
        assert_eq!(renamed["targetImpl"], b.renamed_implementation.as_str());
        let original = check(Some(false), &b.implementation);
        assert_eq!(original["status"], "healthy");
        assert_eq!(original["targetImpl"], b.implementation.as_str());
        assert_eq!(check(Some(true), &b.implementation)["status"], "behind");
        assert_eq!(
            check(Some(false), &b.renamed_implementation)["status"],
            "behind"
        );
        let unread = check(None, &b.renamed_implementation);
        assert_eq!(unread["status"], "unknown");
        assert!(unread["targetImpl"].is_null());
        assert!(unread["atTarget"].is_null());
        assert_eq!(unread["implVersion"], "unknown");
    }

    /// A Base token's rows: its metadata, the renamable names at the rename's
    /// pick, and every leg, the receipt checked against its own manager pin.
    #[test]
    fn a_base_token_checks_its_receipt_manager_and_the_names_the_rename_picks() {
        let c = base();
        let t = &c.tokens[0];
        let live = TokenLive {
            receipt_vault_name: Some(t.renamed_name.clone()),
            receipt_vault_symbol: Some(t.symbol.clone()),
            receipt_vault_decimals: Some(18),
            wrapped_token_vault_name: Some(t.wrapped_token_vault_renamed_name.clone()),
            wrapped_token_vault_symbol: Some(t.wrapped_token_vault_renamed_symbol.clone()),
            wrapped_token_vault_decimals: Some(18),
            ..matching(t.into())
        };
        let rows = token_rows(t, Ok(Some(true)), live.clone());
        let fields: Vec<&str> = rows.iter().map(|r| r["field"].as_str().unwrap()).collect();
        let mut want = vec![
            "receiptVault name",
            "receiptVault symbol",
            "receiptVault decimals",
            "wrappedTokenVault name",
            "wrappedTokenVault symbol",
            "wrappedTokenVault decimals",
        ];
        want.extend(LEG_FIELDS);
        assert_eq!(fields, want);
        for r in &rows {
            assert_eq!(r["component"], "token tMSTR", "{r}");
            assert_eq!(r["status"], "healthy", "{r}");
        }
        let m = row(&rows, "receipt manager");
        assert_eq!(m["address"], t.receipt.as_str());
        assert_eq!(m["expected"], t.receipt_manager.as_str());

        let before = token_rows(t, Ok(Some(false)), live.clone());
        let name = row(&before, "receiptVault name");
        assert_eq!(name["expected"], t.name.as_str());
        assert_eq!(name["status"], "drift");
        assert_eq!(
            row(&before, "wrappedTokenVault name")["expected"],
            t.wrapped_token_vault_name.as_str()
        );
        assert_eq!(
            row(&before, "wrappedTokenVault symbol")["expected"],
            t.wrapped_token_vault_symbol.as_str()
        );
        assert_eq!(row(&before, "receiptVault symbol")["status"], "healthy");

        let unread = token_rows(t, Ok(None), live.clone());
        for f in [
            "receiptVault name",
            "wrappedTokenVault name",
            "wrappedTokenVault symbol",
        ] {
            assert!(row(&unread, f)["expected"].is_null(), "{f}");
            assert_eq!(row(&unread, f)["status"], "unknown", "{f}");
        }
        assert_eq!(row(&unread, "receipt manager")["status"], "healthy");

        let mut managed = t.clone();
        managed.receipt_manager = OTHER.into();
        let m = token_rows(&managed, Ok(Some(true)), live.clone());
        assert_eq!(row(&m, "receipt manager")["expected"], OTHER);
        assert_eq!(row(&m, "receipt manager")["status"], "drift");
        let foreign = TokenLive {
            receipt_manager: Some(OTHER.into()),
            ..live
        };
        assert_eq!(
            row(&token_rows(t, Ok(Some(true)), foreign), "receipt manager")["status"],
            "drift"
        );
    }

    fn unlocated(c: &crate::expected::ExpectedChain) -> RenameUnlocated {
        RenameUnlocated {
            id: c.rename_migration.clone(),
            network: c.network.clone(),
        }
    }

    const BASE_UNLOCATED: &str = "expected-state.json renameMigration \
        0xdb5ef986dc4f7bfd8e32c1ce772cfd820f03b142b96044f33c019782c97273e2 \
        is not in migrations.json for base";

    /// With no target to compare against, every reading on the expected or an unread owner is
    /// `error`, the owner still labelled.
    #[test]
    fn an_unlocated_rename_makes_a_beacon_error_with_the_reason() {
        let c = base();
        let b = &c.beacons[1];
        let e = unlocated(&c);
        let owners = BeaconOwners {
            expected: &b.owner,
            safe: Some(c.safe.address.as_str()),
            legacy: Some(LEGACY),
        };
        let check = |target: Result<Option<&str>, &RenameUnlocated>,
                     owner: Option<&str>,
                     imp: Option<&str>| {
            beacon_health(
                b.name,
                Some(b.address.clone()),
                &owners,
                target,
                Some(V1),
                owner.map(str::to_string),
                imp.map(str::to_string),
            )
        };
        for (owner, imp, label, version) in [
            (
                Some(b.owner.as_str()),
                Some(b.renamed_implementation.as_str()),
                "timelock",
                "unknown",
            ),
            (
                Some(b.owner.as_str()),
                Some(b.implementation.as_str()),
                "timelock",
                "unknown",
            ),
            (Some(b.owner.as_str()), Some(V1), "timelock", "V1"),
            (None, Some(V1), "unknown", "V1"),
            (None, None, "unknown", "unknown"),
        ] {
            let at = format!("{owner:?} {imp:?}");
            let got = check(Err(&e), owner, imp);
            assert_eq!(got["status"], "error", "{at}");
            assert_eq!(got["reason"], BASE_UNLOCATED, "{at}");
            assert_eq!(got["ownerLabel"], label, "{at}");
            assert_eq!(got["owner"], json!(owner), "{at}");
            assert_eq!(got["implementation"], json!(imp), "{at}");
            assert_eq!(got["implVersion"], version, "{at}");
            assert!(got["targetImpl"].is_null(), "{at}");
            assert!(got["atTarget"].is_null(), "{at}");
            assert_eq!(got["targetOwner"], b.owner.as_str(), "{at}");
            assert_eq!(got["name"], b.name, "{at}");
            assert_eq!(got["address"], b.address.as_str(), "{at}");
        }
        for target in [Ok(Some(b.implementation.as_str())), Ok(None)] {
            let got = check(
                target,
                Some(b.owner.as_str()),
                Some(b.implementation.as_str()),
            );
            assert_ne!(got["status"], "error", "{got}");
            assert!(got["reason"].is_null(), "{got}");
        }
    }

    /// Any read owner other than the expected one is drift before error or unknown: an unlocated
    /// rename, an unread implementation or no target never mask a Safe, legacy or foreign owner.
    #[test]
    fn a_wrong_owner_is_drift_whatever_the_implementation_state() {
        let c = base();
        let b = &c.beacons[1];
        let e = unlocated(&c);
        let owners = BeaconOwners {
            expected: &b.owner,
            safe: Some(c.safe.address.as_str()),
            legacy: Some(LEGACY),
        };
        for (owner, label) in [
            (c.safe.address.as_str(), "safe"),
            (LEGACY, "legacy"),
            (OTHER, "foreign"),
        ] {
            for (target, imp, reason) in [
                (Err(&e), Some(V1), json!(BASE_UNLOCATED)),
                (Err(&e), None, json!(BASE_UNLOCATED)),
                (Ok(Some(b.implementation.as_str())), None, json!(null)),
                (Ok(None), Some(b.implementation.as_str()), json!(null)),
                (Ok(None), None, json!(null)),
            ] {
                let at = format!("{label} {target:?} {imp:?}");
                let got = beacon_health(
                    b.name,
                    Some(b.address.clone()),
                    &owners,
                    target,
                    Some(V1),
                    Some(owner.to_string()),
                    imp.map(str::to_string),
                );
                assert_eq!(got["status"], "drift", "{at}");
                assert_eq!(got["ownerLabel"], label, "{at}");
                assert_eq!(got["reason"], reason, "{at}");
            }
        }
    }

    /// Only the renamable rows depend on the rename; every other row is the row a located rename gives.
    #[test]
    fn an_unlocated_rename_makes_only_a_tokens_renamable_rows_error() {
        let c = base();
        let t = &c.tokens[0];
        let e = unlocated(&c);
        let live = TokenLive {
            receipt_vault_name: Some(t.renamed_name.clone()),
            receipt_vault_symbol: Some(t.symbol.clone()),
            receipt_vault_decimals: Some(18),
            wrapped_token_vault_name: Some(t.wrapped_token_vault_renamed_name.clone()),
            wrapped_token_vault_symbol: Some(t.wrapped_token_vault_renamed_symbol.clone()),
            wrapped_token_vault_decimals: Some(7),
            receipt_manager: Some(OTHER.into()),
            receipt_codehash: None,
            ..matching(t.into())
        };
        let renamable = [
            ("receiptVault name", t.renamed_name.as_str()),
            (
                "wrappedTokenVault name",
                t.wrapped_token_vault_renamed_name.as_str(),
            ),
            (
                "wrappedTokenVault symbol",
                t.wrapped_token_vault_renamed_symbol.as_str(),
            ),
        ];
        let rows = token_rows(t, Err(&e), live.clone());
        for (field, live_name) in renamable {
            let r = row(&rows, field);
            assert_eq!(r["status"], "error", "{field}");
            assert_eq!(r["reason"], BASE_UNLOCATED, "{field}");
            assert!(r["expected"].is_null(), "{field}");
            assert_eq!(r["live"], live_name, "{field}");
            assert_eq!(r["component"], "token tMSTR", "{field}");
        }
        assert_eq!(
            rows.iter().filter(|r| r["status"] == "error").count(),
            renamable.len()
        );
        for located in [Ok(Some(true)), Ok(Some(false)), Ok(None)] {
            let want = token_rows(t, located, live.clone());
            assert_eq!(rows.len(), want.len());
            for (got, want) in rows.iter().zip(&want) {
                let field = want["field"].as_str().unwrap();
                assert_eq!(got["field"], field);
                if renamable.iter().all(|(f, _)| *f != field) {
                    assert_eq!(got, want, "{field}");
                }
            }
        }
        assert_eq!(row(&rows, "receiptVault symbol")["status"], "healthy");
        assert_eq!(row(&rows, "wrappedTokenVault decimals")["status"], "drift");
        assert_eq!(row(&rows, "receipt manager")["status"], "drift");
        assert_eq!(row(&rows, "receipt codehash")["status"], "unknown");

        let unread = token_rows(
            t,
            Err(&e),
            TokenLive {
                receipt_vault_name: None,
                ..live
            },
        );
        let name = row(&unread, "receiptVault name");
        assert_eq!(name["status"], "error");
        assert_eq!(name["reason"], BASE_UNLOCATED);
        assert!(name["live"].is_null());
    }

    /// Base's tQSEP rows: its owner, authoriser and legs on the shared Base
    /// beacons, under `qsep`.
    #[test]
    fn base_qsep_checks_its_owner_authoriser_and_legs() {
        let c = base();
        let q = c.qsep.as_ref().unwrap();
        let live = matching(q.into());
        let rows = qsep_rows(q, &live);
        assert_eq!(
            rows.iter()
                .map(|r| r["field"].as_str().unwrap())
                .collect::<Vec<_>>(),
            LEG_FIELDS
        );
        for r in &rows {
            assert_eq!(r["component"], "qsep", "{r}");
            assert_eq!(r["status"], "healthy", "{r}");
        }
        let at = |f: &str| {
            let r = row(&rows, f);
            (
                r["address"].as_str().unwrap().to_string(),
                r["expected"].as_str().unwrap().to_string(),
            )
        };
        assert_eq!(
            at("receiptVault owner"),
            (q.receipt_vault.clone(), c.timelock.code.address.clone())
        );
        assert_eq!(
            at("receiptVault authoriser"),
            (q.receipt_vault.clone(), c.authoriser.address.clone())
        );
        assert_eq!(
            at("receiptVault beacon"),
            (q.receipt_vault.clone(), c.beacons[1].address.clone())
        );
        assert_eq!(
            at("receipt beacon"),
            (q.receipt.clone(), c.beacons[0].address.clone())
        );
        assert_eq!(
            at("wrappedTokenVault beacon"),
            (q.wrapped_token_vault.clone(), c.beacons[2].address.clone())
        );
        assert_eq!(
            at("wrappedTokenVault asset"),
            (q.wrapped_token_vault.clone(), q.receipt_vault.clone())
        );
        assert_eq!(
            at("receipt manager"),
            (q.receipt.clone(), q.receipt_manager.clone())
        );
        let safe_owned = TokenLive {
            receipt_vault_owner: Some(c.safe.address.clone()),
            receipt_vault_authoriser: None,
            ..live
        };
        let drifted = qsep_rows(q, &safe_owned);
        assert_eq!(row(&drifted, "receiptVault owner")["status"], "drift");
        assert_eq!(
            row(&drifted, "receiptVault authoriser")["status"],
            "unknown"
        );
    }

    /// Each qsep pin reaches its own row.
    #[test]
    fn each_qsep_pin_reaches_its_own_row() {
        let a = |i: usize| format!("0x{i:040x}");
        let h = |i: usize| format!("0x{i:064x}");
        let q = ExpectedQsep {
            receipt_vault: a(1),
            receipt: a(2),
            wrapped_token_vault: a(3),
            receipt_vault_receipt: a(4),
            receipt_manager: a(5),
            wrapped_token_vault_asset: a(6),
            receipt_vault_beacon: a(7),
            receipt_beacon: a(8),
            wrapped_token_vault_beacon: a(9),
            receipt_vault_codehash: h(10),
            receipt_codehash: h(11),
            wrapped_token_vault_codehash: h(12),
            owner: a(13),
            authoriser: a(14),
        };
        let rows = qsep_rows(&q, &TokenLive::default());
        let got: Vec<(String, String)> = rows
            .iter()
            .map(|r| {
                (
                    r["address"].as_str().unwrap().to_string(),
                    r["expected"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let want = [
            (1, a(4)),
            (1, a(13)),
            (1, a(14)),
            (1, a(7)),
            (1, h(10)),
            (2, a(5)),
            (2, a(8)),
            (2, h(11)),
            (3, a(6)),
            (3, a(9)),
            (3, h(12)),
        ]
        .map(|(at, pin)| (a(at), pin));
        assert_eq!(got, want);
        assert!(rows.iter().all(|r| r["status"] == "unknown"));
    }

    /// Each token pin and live read reaches its own row.
    #[test]
    fn each_token_pin_and_read_reaches_its_own_row() {
        let a = |i: usize| format!("0x{i:040x}");
        let h = |i: usize| format!("0x{i:064x}");
        let t = ExpectedToken {
            underlying: "U".into(),
            name: "N".into(),
            renamed_name: "RN".into(),
            symbol: "S".into(),
            decimals: 6,
            wrapped_token_vault_name: "WN".into(),
            wrapped_token_vault_renamed_name: "WRN".into(),
            wrapped_token_vault_symbol: "WS".into(),
            wrapped_token_vault_renamed_symbol: "WRS".into(),
            wrapped_token_vault_decimals: 18,
            receipt_vault: a(1),
            receipt: a(2),
            wrapped_token_vault: a(3),
            receipt_vault_receipt: a(4),
            receipt_manager: a(5),
            wrapped_token_vault_asset: a(6),
            receipt_vault_beacon: a(7),
            receipt_beacon: a(8),
            wrapped_token_vault_beacon: a(9),
            receipt_vault_codehash: h(10),
            receipt_codehash: h(11),
            wrapped_token_vault_codehash: h(12),
            owner: a(13),
            authoriser: a(14),
            asset_id: None,
            listing_ticker: None,
            listing_name: None,
            region: None,
        };
        let r = |i: usize| Some(format!("r{i}"));
        let live = TokenLive {
            receipt_vault_name: r(1),
            receipt_vault_symbol: r(2),
            receipt_vault_decimals: Some(1),
            receipt_vault_receipt: r(4),
            receipt_vault_owner: r(5),
            receipt_vault_authoriser: r(6),
            receipt_vault_beacon: r(7),
            receipt_vault_codehash: r(8),
            receipt_manager: r(9),
            receipt_beacon: r(10),
            receipt_codehash: r(11),
            wrapped_token_vault_name: r(12),
            wrapped_token_vault_symbol: r(13),
            wrapped_token_vault_decimals: Some(2),
            wrapped_token_vault_asset: r(15),
            wrapped_token_vault_beacon: r(16),
            wrapped_token_vault_codehash: r(17),
        };
        let rows = token_rows(&t, Ok(Some(false)), live);
        let got: Vec<[String; 4]> = rows
            .iter()
            .map(|r| {
                ["field", "address", "expected", "live"].map(|k| r[k].as_str().unwrap().to_string())
            })
            .collect();
        let want = [
            ("receiptVault name", 1, "N".to_string(), "r1"),
            ("receiptVault symbol", 1, "S".to_string(), "r2"),
            ("receiptVault decimals", 1, "6".to_string(), "1"),
            ("wrappedTokenVault name", 3, "WN".to_string(), "r12"),
            ("wrappedTokenVault symbol", 3, "WS".to_string(), "r13"),
            ("wrappedTokenVault decimals", 3, "18".to_string(), "2"),
            ("receiptVault receipt", 1, a(4), "r4"),
            ("receiptVault owner", 1, a(13), "r5"),
            ("receiptVault authoriser", 1, a(14), "r6"),
            ("receiptVault beacon", 1, a(7), "r7"),
            ("receiptVault codehash", 1, h(10), "r8"),
            ("receipt manager", 2, a(5), "r9"),
            ("receipt beacon", 2, a(8), "r10"),
            ("receipt codehash", 2, h(11), "r11"),
            ("wrappedTokenVault asset", 3, a(6), "r15"),
            ("wrappedTokenVault beacon", 3, a(9), "r16"),
            ("wrappedTokenVault codehash", 3, h(12), "r17"),
        ]
        .map(|(f, at, pin, l)| [f.to_string(), a(at), pin, l.to_string()]);
        assert_eq!(got, want);
    }

    /// The authoriser and EU authoriser rows each carry their own codehash pin.
    #[test]
    fn authoriser_rows_carry_their_own_codehash() {
        let mut c = base();
        c.eu_authoriser.codehash = format!("0x{:064x}", 1);
        let rows = infra_rows(&c, InfraLive::default());
        let pin = |component: &str| {
            rows.iter().find(|r| r["component"] == component).unwrap()["expected"].clone()
        };
        assert_eq!(pin("authoriser"), c.authoriser.codehash.as_str());
        assert_eq!(pin("euAuthoriser"), c.eu_authoriser.codehash.as_str());
    }

    /// A row is healthy only on a case-insensitive match to a pin; no pin or no
    /// live read is unknown, never healthy.
    #[test]
    fn infra_row_compares_live_to_its_pin() {
        let pin = "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E";
        let r = infra_row("c", "f", Some(OTHER), Some(pin), Some(pin.to_lowercase()));
        assert_eq!(r["status"], "healthy");
        assert_eq!(r["component"], "c");
        assert_eq!(r["field"], "f");
        assert_eq!(r["address"], OTHER);
        assert_eq!(r["expected"], pin);
        assert_eq!(r["live"], pin.to_lowercase());
        assert_eq!(
            infra_row("c", "f", None, Some(pin), Some(OTHER.into()))["status"],
            "drift"
        );
        assert_eq!(
            infra_row("c", "f", None, Some(pin), None)["status"],
            "unknown"
        );
        assert_eq!(
            infra_row("c", "f", None, None, Some(pin.into()))["status"],
            "unknown"
        );
    }

    /// Base's infra rows, in order, at the fixture's pins: a matching read is
    /// healthy, and each read reaches only its own row, drifting on another
    /// value and unknown when the read failed.
    #[test]
    fn base_infra_rows_pin_each_fact_and_check_its_own_read() {
        let c = base();
        let timelock = "0x48ba1371A78E6cC54157c63721756ab444510DB3";
        let beacon_hash = "0x8e95867e52db417944afd90f3b6c3c980962831e8a944e7f6958ba8f8cc10630";
        let orch_beacon = "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613";
        let orch_hash = "0x448cd06335de9e79ecdc51aa7c6647926860a1976f407de6f79f363b85ccaf2b";
        let clone_hash = "0x2089950d3cc1112dd66a58adcfadeadc490b50053ac67be8bc676b4a2dcd1717";
        let want: [(&str, &str, &str, &str); 14] = [
            (
                "beaconSetDeployer",
                "hasCode",
                "0x2191981Ca2477B745870cC307cbEB4cB2967ACe3",
                "true",
            ),
            (
                "Receipt beacon",
                "codehash",
                "0x86e93c39B095be0B0054C8488E26466Ee027D79a",
                beacon_hash,
            ),
            (
                "Receipt-vault beacon",
                "codehash",
                "0xEa084c8F4331CDF3328E772781b59F8A24F28F1A",
                beacon_hash,
            ),
            (
                "Wrapped-token-vault beacon",
                "codehash",
                "0x4c2d2d3Bf1232bf0d3FB7123007A9B8444637bC8",
                beacon_hash,
            ),
            ("Orchestrator beacon", "codehash", orch_beacon, orch_hash),
            (
                "timelock",
                "codehash",
                timelock,
                "0xb6234401d5b271d66957815831adcde1e66f80a4c7a9d01881c7eecb70e96993",
            ),
            ("timelock", "minDelay", timelock, "172800"),
            (
                "orchestrator",
                "setDeployer hasCode",
                "0x945D0Faf6f268e805246907507ef1044e70E75D7",
                "true",
            ),
            ("orchestrator", "beacon codehash", orch_beacon, orch_hash),
            (
                "orchestrator",
                "beacon implementation",
                orch_beacon,
                "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72",
            ),
            ("orchestrator", "beacon owner", orch_beacon, timelock),
            (
                "orchestrator",
                "instance beacon",
                "0x3A7387a484d87Aa8bBA45E98AAB401Ce4FBF03E2",
                orch_beacon,
            ),
            (
                "authoriser",
                "codehash",
                "0x315b16faa6eE413faBCa877d3851B3818369f0cD",
                clone_hash,
            ),
            (
                "euAuthoriser",
                "codehash",
                "0xDB9152e46c1D140DB6a6f814461f21c571F3275e",
                clone_hash,
            ),
        ];
        let live = |v: &dyn Fn(usize) -> Option<String>| InfraLive {
            beacon_set_deployer_has_code: v(0),
            beacon_codehashes: (1..5).map(v).collect(),
            timelock_codehash: v(5),
            timelock_min_delay: v(6),
            orchestrator_set_deployer_has_code: v(7),
            orchestrator_beacon_codehash: v(8),
            orchestrator_beacon_implementation: v(9),
            orchestrator_beacon_owner: v(10),
            orchestrator_instance_beacon: v(11),
            authoriser_codehash: v(12),
            eu_authoriser_codehash: v(13),
        };
        let pinned = |j: usize| Some(want[j].3.to_lowercase());
        let statuses = |rows: &[serde_json::Value]| {
            rows.iter()
                .map(|r| r["status"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };

        let rows = infra_rows(&c, live(&pinned));
        let got: Vec<(&str, &str, &str, &str)> = rows
            .iter()
            .map(|r| {
                (
                    r["component"].as_str().unwrap(),
                    r["field"].as_str().unwrap(),
                    r["address"].as_str().unwrap(),
                    r["expected"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(got, want);
        assert_eq!(statuses(&rows), vec!["healthy"; want.len()]);

        for i in 0..want.len() {
            for (read, status) in [(Some(OTHER.to_string()), "drift"), (None, "unknown")] {
                let rows = infra_rows(&c, live(&|j| if j == i { read.clone() } else { pinned(j) }));
                let mut exp = vec!["healthy"; want.len()];
                exp[i] = status;
                assert_eq!(statuses(&rows), exp, "row {i} read {read:?}");
            }
        }

        let mut short = live(&pinned);
        short.beacon_codehashes.truncate(2);
        let mut exp = vec!["healthy"; want.len()];
        exp[3] = "unknown";
        exp[4] = "unknown";
        assert_eq!(statuses(&infra_rows(&c, short)), exp);
    }

    /// A Zoltu suite's codehash is its row; no code reads `none` and drifts.
    #[test]
    fn zoltu_row_checks_the_suite_codehash() {
        let v = base_v2();
        let z = &v.deployments[0];
        let r = zoltu_row(z, Some(z.code.codehash.clone()));
        assert_eq!(r["component"], format!("zoltu {}", z.key));
        assert_eq!(r["field"], "codehash");
        assert_eq!(r["address"], z.code.address.as_str());
        assert_eq!(r["expected"], z.code.codehash.as_str());
        assert_eq!(r["status"], "healthy");
        assert_eq!(zoltu_row(z, Some("none".into()))["status"], "drift");
        assert_eq!(zoltu_row(z, None)["status"], "unknown");
    }

    /// Base's v2 rows: every deployment's codehash in file order, then each
    /// beacon's implementation and owner; the chain's document counts them.
    #[test]
    fn base_v2_rows_check_each_deployment_and_beacon_and_build_infra_counts_them() {
        let v = base_v2();
        let n = v.deployments.len();
        let mut hashes: Vec<Option<String>> = v
            .deployments
            .iter()
            .map(|z| Some(z.code.codehash.clone()))
            .collect();
        hashes[1] = Some("none".into());
        hashes[2] = None;
        let mut beacons: Vec<V2BeaconLive> = v
            .beacons
            .iter()
            .map(|b| V2BeaconLive {
                implementation: Some(b.implementation.to_lowercase()),
                owner: Some(b.owner.to_lowercase()),
            })
            .collect();
        beacons[2].owner = Some(OTHER.into());
        let rows = v2_rows(&v, hashes, beacons);
        assert_eq!(rows.len(), n + 6);
        for (r, z) in rows.iter().zip(&v.deployments) {
            assert_eq!(r["component"], format!("zoltu {}", z.key));
            assert_eq!(r["address"], z.code.address.as_str());
            assert_eq!(r["expected"], z.code.codehash.as_str());
        }
        assert_eq!(
            rows[..3]
                .iter()
                .map(|r| r["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["healthy", "drift", "unknown"]
        );
        for (i, b) in v.beacons.iter().enumerate() {
            let imp = &rows[n + 2 * i];
            let own = &rows[n + 2 * i + 1];
            assert_eq!(imp["component"], b.name);
            assert_eq!(imp["field"], "implementation");
            assert_eq!(imp["address"], b.address.as_str());
            assert_eq!(imp["expected"], b.implementation.as_str());
            assert_eq!(own["component"], b.name);
            assert_eq!(own["field"], "owner");
            assert_eq!(own["address"], b.address.as_str());
            assert_eq!(own["expected"], b.owner.as_str());
        }
        assert_eq!(rows[n]["status"], "healthy");
        assert_eq!(rows[n + 1]["status"], "healthy");
        assert_eq!(rows[n + 5]["status"], "drift");

        let doc = build_infra("S01-Issuer", "st0x.deploy", "base", "host", rows.clone());
        assert_eq!(doc["org"], "S01-Issuer");
        assert_eq!(doc["repo"], "st0x.deploy");
        assert_eq!(doc["network"], "base");
        assert_eq!(doc["rpcHost"], "host");
        assert_eq!(doc["source"], EXPECTED_STATE_PATH);
        assert_eq!(doc["total"], n + 6);
        assert_eq!(doc["healthy"], n + 3);
        assert_eq!(doc["rows"], serde_json::Value::Array(rows));
    }

    const WRAPPED: &str = "0x4c3df5a6ef6a7d2b0c1e8f9a0b1c2d3e4f5a6b7c";
    const COLLATERAL: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

    /// How st0x.registry lists a governed token: its receipt vault as `unwrappedAddress`.
    fn listed(t: &ExpectedToken) -> serde_json::Value {
        json!({
            "symbol": t.wrapped_token_vault_symbol,
            "name": t.wrapped_token_vault_name,
            "address": t.wrapped_token_vault,
            "extensions": { "unwrappedAddress": t.receipt_vault },
        })
    }

    fn list_of(tokens: Vec<serde_json::Value>) -> String {
        json!({ "name": "ST0x", "tokens": tokens }).to_string()
    }

    fn reconcile_one(c: &ExpectedChain, body: String) -> serde_json::Value {
        let docs = registry_reconcile(&Ok(vec![c.clone()]), |p| {
            assert_eq!(p, format!("token-lists/{}.json", c.network));
            body.clone()
        });
        assert_eq!(docs.len(), 1);
        docs.into_iter().next().unwrap()
    }

    #[test]
    fn an_expected_state_error_is_one_unavailable_doc_with_its_reason() {
        for e in [
            ExpectedError::Empty,
            ExpectedError::Malformed {
                chain: "8453".into(),
                field: "tokens",
            },
        ] {
            let docs = registry_reconcile(&Err(e.clone()), |p| panic!("read {p}"));
            assert_eq!(docs.len(), 1);
            let d = &docs[0];
            assert_eq!(d["unavailable"], true);
            assert_eq!(d["reason"], e.to_string());
            assert_eq!(d["registryOrg"], "ST0x-Technology");
            assert_eq!(d["registryRepo"], "st0x.registry");
            assert_eq!(d["expectedState"], EXPECTED_STATE_PATH);
            assert!(d["network"].is_null());
            assert!(d["missingFromMigration"].is_null());
            assert!(d["extraVaults"].is_null());
        }
    }

    #[test]
    fn a_chain_without_a_readable_list_is_unavailable_with_its_reason_not_skipped() {
        let chains = crate::expected::parse(FIXTURE).unwrap();
        let networks: Vec<String> = chains.iter().map(|c| c.network.clone()).collect();
        let mut sorted = networks.clone();
        sorted.sort();
        assert_eq!(sorted, ["base", "bsc", "ethereum", "hyperevm", "robinhood"]);
        let at = |n: &str| networks.iter().position(|x| x == n).unwrap();
        let read = std::cell::RefCell::new(Vec::new());
        let docs = registry_reconcile(&Ok(chains.clone()), |p| {
            read.borrow_mut().push(p.to_string());
            match p {
                "token-lists/ethereum.json" => String::new(),
                "token-lists/robinhood.json" => "not json".to_string(),
                "token-lists/bsc.json" => r#"{"name":"ST0x"}"#.to_string(),
                "token-lists/hyperevm.json" => r#"{"tokens":{}}"#.to_string(),
                _ => list_of(vec![]),
            }
        });
        let paths: Vec<String> = networks
            .iter()
            .map(|n| format!("token-lists/{n}.json"))
            .collect();
        assert_eq!(*read.borrow(), paths);
        assert_eq!(docs.len(), chains.len());
        for (d, (n, p)) in docs.iter().zip(networks.iter().zip(&paths)) {
            assert_eq!(d["network"], n.as_str());
            assert_eq!(d["registryPath"], p.as_str());
            assert_eq!(d["registryOrg"], "ST0x-Technology");
            assert_eq!(d["registryRepo"], "st0x.registry");
            assert_eq!(d["expectedState"], EXPECTED_STATE_PATH);
        }
        for (n, reason) in [
            (
                "ethereum",
                "no readable registry list at token-lists/ethereum.json",
            ),
            ("robinhood", "token-lists/robinhood.json is not valid JSON"),
            ("bsc", "token-lists/bsc.json has no tokens array"),
            ("hyperevm", "token-lists/hyperevm.json has no tokens array"),
        ] {
            let d = &docs[at(n)];
            assert_eq!(d["unavailable"], true, "{n}");
            assert_eq!(d["reason"], reason);
            for k in [
                "governedCount",
                "registryTokenCount",
                "missingFromMigration",
                "extraVaults",
            ] {
                assert!(d[k].is_null(), "{n} carries {k}");
            }
        }
        let base = &docs[at("base")];
        assert!(base["unavailable"].is_null());
        assert!(base["reason"].is_null());
        assert_eq!(base["registryTokenCount"], 0);
        assert_eq!(
            base["extraVaults"].as_array().unwrap().len(),
            chains[at("base")].tokens.len()
        );
    }

    #[test]
    fn a_governed_vault_missing_from_the_registry_is_an_extra_vault() {
        let c = base();
        let mut tokens: Vec<_> = c.tokens.iter().map(listed).collect();
        tokens.remove(1);
        let d = reconcile_one(&c, list_of(tokens));
        assert_eq!(
            d["extraVaults"],
            json!([{ "symbol": c.tokens[1].symbol, "address": c.tokens[1].receipt_vault }])
        );
        assert_eq!(d["missingFromMigration"], json!([]));
    }

    #[test]
    fn a_registry_token_whose_vault_is_not_governed_is_missing_from_migration() {
        let c = base();
        let mut tokens: Vec<_> = c.tokens.iter().map(listed).collect();
        tokens.insert(
            2,
            json!({
                "symbol": "wtXYZ",
                "name": "XYZ ST0x",
                "address": WRAPPED,
                "extensions": { "unwrappedAddress": OTHER },
            }),
        );
        let d = reconcile_one(&c, list_of(tokens));
        assert_eq!(
            d["missingFromMigration"],
            json!([{
                "symbol": "wtXYZ",
                "name": "XYZ ST0x",
                "address": WRAPPED,
                "receiptVault": OTHER,
                "wrapped": true,
                "reason": "receipt vault not in expected-state",
            }])
        );
        assert_eq!(d["extraVaults"], json!([]));
    }

    #[test]
    fn an_empty_unwrapped_address_is_collateral() {
        let c = base();
        let mut tokens: Vec<_> = c.tokens.iter().map(listed).collect();
        tokens.push(json!({
            "symbol": "USDC",
            "name": "USD Coin",
            "address": COLLATERAL,
            "extensions": { "unwrappedAddress": "" },
        }));
        let d = reconcile_one(&c, list_of(tokens));
        assert_eq!(
            d["missingFromMigration"],
            json!([{
                "symbol": "USDC",
                "name": "USD Coin",
                "address": COLLATERAL,
                "receiptVault": null,
                "wrapped": false,
                "reason": "no receipt vault (collateral)",
            }])
        );
        assert_eq!(d["extraVaults"], json!([]));
    }

    #[test]
    fn registry_vaults_match_governed_vaults_case_insensitively() {
        let c = base();
        let cases: [fn(&str) -> String; 2] = [
            |a| format!("0x{}", a[2..].to_uppercase()),
            |a| a.to_lowercase(),
        ];
        for case in cases {
            assert!(c
                .tokens
                .iter()
                .any(|t| case(&t.receipt_vault) != t.receipt_vault));
            let tokens = c
                .tokens
                .iter()
                .map(|t| {
                    let mut e = listed(t);
                    e["extensions"]["unwrappedAddress"] = json!(case(&t.receipt_vault));
                    e
                })
                .collect();
            let d = reconcile_one(&c, list_of(tokens));
            assert_eq!(d["missingFromMigration"], json!([]));
            assert_eq!(d["extraVaults"], json!([]));
        }
    }

    #[test]
    fn counts_are_governed_vaults_and_every_registry_token() {
        let c = base();
        let n = c.tokens.len();
        assert!(n > 5);
        let mut tokens: Vec<_> = c.tokens.iter().skip(2).map(listed).collect();
        tokens.push(json!({ "symbol": "wtXYZ", "address": WRAPPED, "extensions": { "unwrappedAddress": OTHER } }));
        tokens.push(json!({ "symbol": "USDC", "address": COLLATERAL, "extensions": { "unwrappedAddress": "" } }));
        tokens.push(
            json!({ "symbol": "WETH", "address": OTHER, "extensions": { "unwrappedAddress": "" } }),
        );
        let d = reconcile_one(&c, list_of(tokens));
        assert_eq!(d["network"], "base");
        assert_eq!(d["registryPath"], "token-lists/base.json");
        assert!(d["unavailable"].is_null());
        assert_eq!(d["governedCount"], n);
        assert_eq!(d["registryTokenCount"], n + 1);
        assert_eq!(
            d["missingFromMigration"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["symbol"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["wtXYZ", "USDC", "WETH"]
        );
        assert_eq!(
            d["extraVaults"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["symbol"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [c.tokens[0].symbol.as_str(), c.tokens[1].symbol.as_str()]
        );
    }

    /// Two tokens on one receipt vault (in any case) are one governed vault, and
    /// an unlisted governed vault is reported at its receipt vault.
    #[test]
    fn governed_count_is_distinct_vaults_and_extras_carry_the_receipt_vault() {
        let mut c = base();
        c.tokens.truncate(2);
        c.tokens[0].receipt_manager = OTHER.to_string();
        c.tokens[1].receipt_vault = format!("0x{}", c.tokens[0].receipt_vault[2..].to_uppercase());
        let d = reconcile_one(&c, list_of(vec![]));
        assert_eq!(d["governedCount"], 1);
        assert_eq!(
            d["extraVaults"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["address"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                c.tokens[0].receipt_vault.as_str(),
                c.tokens[1].receipt_vault.as_str()
            ]
        );
    }
}
