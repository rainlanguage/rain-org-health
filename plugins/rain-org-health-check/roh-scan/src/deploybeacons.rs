//! The production beacons on every chain (rain-org-health#182): the four
//! beacons each chain's tokens and orchestrator proxy through, each checked for
//! its code hash, its owner (strictly that chain's token-owner Safe) and the
//! implementation it serves.
//!
//! Which beacons, in which order, and the code hash each must have are read
//! from `LibBeaconInvariants` and the per-chain beacon-set libraries it
//! dispatches to (`prodBeaconsForChainId`, `prodBeaconCodehashesForChainId`),
//! following each library's imports to the constants it names. A beacon the
//! 0.1.1 set deployer creates is pinned nowhere as a constant, so it is read
//! live from the deployer getter the library calls.
//!
//! The implementation targets are the one thing the deploy repo does not
//! declare: `LibProdBeacons*.implementations()` still lists the 0.1.1 set the
//! beacons have since been repointed from. [`TARGET_IMPLS`] names the
//! `LibProdDeployV4` constants the #182 predicate checks against, and their
//! addresses are read out of the deploy repo like every other value.
//!
//! Rows share `deploystate`'s shape and verdicts: `pass`, `fail` or `unknown`,
//! where `unknown` means the question could not be asked or answered and is
//! never a stand-in for `fail`.

use crate::deployhealth::{function_body, parse_bytes32_constant};
use crate::deploystate::{
    codehash_row, resolve_generated_address, rollup, row, same, subject, ChainReads, FrozenPin,
    Verdict,
};
use crate::owners::ChainPin;
use crate::rpc;
use alloy_primitives::{hex, keccak256};
use regex::Regex;
use serde_json::{json, Value};

/// Where the beacon invariants live; every other beacon source is reached from
/// its imports.
pub const BEACON_INVARIANTS: &str = "src/lib/LibBeaconInvariants.sol";

/// The implementation each beacon must serve, by its `*_BEACON_INDEX` name, as
/// the `LibProdDeployV4` constant that pins it. Written here because the deploy
/// repo's own list (`LibProdBeacons*.implementations()`) is stale at 0.1.1; the
/// names are the ones the #182 predicate checks. The wrapped-token vault has no
/// 0.1.30 release, so 0.1.1 is its newest.
pub const TARGET_IMPLS: [(&str, &str); 4] = [
    ("RECEIPT", "STOX_RECEIPT_0_1_30"),
    ("RECEIPT_VAULT", "STOX_RECEIPT_VAULT_0_1_30"),
    ("WRAPPED_TOKEN_VAULT", "STOX_WRAPPED_TOKEN_VAULT_0_1_1"),
    ("ORCHESTRATOR", "ST0X_ORCHESTRATOR_0_1_30"),
];

// ---------------------------------------------------------------------------
// Source parsing.
// ---------------------------------------------------------------------------

/// The relative imports of `src`, as `(local name, repo path)`, with paths
/// resolved against `dir` (the directory `src` lives in).
pub fn imports(src: &str, dir: &str) -> Vec<(String, String)> {
    let Ok(re) = Regex::new(r#"import\s*\{([^}]*)\}\s*from\s*"(\.[^"]*)""#) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in re.captures_iter(src) {
        let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        for seg in c[2].split('/') {
            match seg {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        let path = parts.join("/");
        for name in c[1].split(',') {
            if let Some(local) = name.split_whitespace().last() {
                out.push((local.to_string(), path.clone()));
            }
        }
    }
    out
}

/// The entries of the first `return [ … ];` array in `body`, split at
/// top-level commas.
fn return_array(body: &str) -> Vec<String> {
    let Some(start) = body.find("return") else {
        return Vec::new();
    };
    let rest = &body[start..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let (mut depth, mut cur, mut out) = (0usize, String::new(), Vec::new());
    for ch in rest[open + 1..].chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ']' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                return out;
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    Vec::new()
}

/// `prodBeaconsForChainId`'s dispatch, as `(network, beacon-set library)`. The
/// network is the chain-id constant's prefix, lowercased (`BASE_CHAIN_ID` is
/// `base`), which is how the chain pins name chains.
pub fn parse_chain_sets(beacon_inv: &str) -> Vec<(String, String)> {
    let Some(body) = function_body(beacon_inv, "prodBeaconsForChainId") else {
        return Vec::new();
    };
    let Ok(re) = Regex::new(
        r"(?s)chainId\s*==\s*(?:\w+\.)?([A-Z0-9]+)_CHAIN_ID\s*\)\s*\{[^}]*?return\s+(\w+)\.beacons\(\s*\)",
    ) else {
        return Vec::new();
    };
    re.captures_iter(body)
        .map(|c| (c[1].to_lowercase(), c[2].to_string()))
        .collect()
}

/// The `*_BEACON_INDEX` names, by index (`RECEIPT` at 0). A gap is `None`.
pub fn parse_beacon_indices(beacon_inv: &str) -> Vec<Option<String>> {
    let Ok(re) = Regex::new(r"\b([A-Z][A-Z0-9_]*)_BEACON_INDEX\s*=\s*(\d+)\s*;") else {
        return Vec::new();
    };
    let mut out: Vec<Option<String>> = Vec::new();
    for c in re.captures_iter(beacon_inv) {
        let Ok(i) = c[2].parse::<usize>() else {
            continue;
        };
        if i >= 64 {
            continue;
        }
        if out.len() <= i {
            out.resize(i + 1, None);
        }
        out[i] = Some(c[1].to_string());
    }
    out
}

/// The code hash each beacon must have, index-aligned, from
/// `prodBeaconCodehashesForChainId`. An entry whose constant did not resolve is
/// `None`.
pub fn parse_beacon_codehashes(beacon_inv: &str) -> Vec<Option<String>> {
    let Some(body) = function_body(beacon_inv, "prodBeaconCodehashesForChainId") else {
        return Vec::new();
    };
    return_array(body)
        .iter()
        .map(|e| {
            let name = e.rsplit('.').next().unwrap_or(e).trim();
            parse_bytes32_constant(beacon_inv, name)
        })
        .collect()
}

/// One entry of a beacon-set library's `beacons()` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeaconRef {
    /// `Lib.CONST`.
    Constant { lib: String, name: String },
    /// `address(<deployer>.<getter>())`, the deployer held in `Lib.CONST`.
    Getter {
        lib: String,
        name: String,
        getter: String,
    },
    /// An entry of a shape this parser does not read. Kept, so the entries
    /// after it keep their index.
    Unparsed(String),
}

/// The entries of a beacon-set library's `beacons()`, in order.
pub fn parse_beacon_refs(set_lib: &str) -> Vec<BeaconRef> {
    let Some(body) = function_body(set_lib, "beacons") else {
        return Vec::new();
    };
    let (Ok(local), Ok(getter), Ok(constant)) = (
        Regex::new(r"\b(\w+)\s*=\s*\w+\(\s*(\w+)\.(\w+)\s*\)\s*;"),
        Regex::new(r"^address\(\s*(\w+)\.(\w+)\(\s*\)\s*\)$"),
        Regex::new(r"^(\w+)\.(\w+)$"),
    ) else {
        return Vec::new();
    };
    let deployers: Vec<(String, String, String)> = local
        .captures_iter(body)
        .map(|c| (c[1].to_string(), c[2].to_string(), c[3].to_string()))
        .collect();
    return_array(body)
        .into_iter()
        .map(|e| {
            if let Some(c) = getter.captures(&e) {
                if let Some((_, lib, name)) = deployers.iter().find(|(v, _, _)| *v == c[1]) {
                    return BeaconRef::Getter {
                        lib: lib.clone(),
                        name: name.clone(),
                        getter: c[2].to_string(),
                    };
                }
            }
            match constant.captures(&e) {
                Some(c) => BeaconRef::Constant {
                    lib: c[1].to_string(),
                    name: c[2].to_string(),
                },
                None => BeaconRef::Unparsed(e),
            }
        })
        .collect()
}

/// Calldata for a no-argument getter, by name.
pub fn getter_calldata(getter: &str) -> String {
    let h = keccak256(format!("{getter}()").as_bytes());
    format!("0x{}", hex::encode(&h[..4]))
}

/// The beacon's page label from its index name: `RECEIPT_VAULT` is
/// `Receipt-vault beacon`.
pub fn label(index_name: &str) -> String {
    let lower = index_name.to_lowercase().replace('_', "-");
    let mut cs = lower.chars();
    match cs.next() {
        Some(f) => format!("{}{} beacon", f.to_uppercase(), cs.as_str()),
        None => "Beacon".to_string(),
    }
}

/// A release suffix as a version: `STOX_RECEIPT_0_1_30` is `0.1.30`.
fn version_of(constant: &str) -> Option<String> {
    let re = Regex::new(r"_(\d+)_(\d+)_(\d+)$").ok()?;
    let c = re.captures(constant)?;
    Some(format!("{}.{}.{}", &c[1], &c[2], &c[3]))
}

/// Where one beacon's address comes from, resolved as far as source allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeaconAt {
    /// A pinned constant (`None`: it did not resolve).
    Pinned {
        source: String,
        address: Option<String>,
    },
    /// Read live from a deployer's getter (`deployer` `None`: its constant did
    /// not resolve).
    Getter {
        source: String,
        deployer: Option<String>,
        getter: String,
    },
    /// An entry the parser did not read.
    Unparsed(String),
}

/// What one beacon position must look like, on every chain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slot {
    /// `RECEIPT`, `RECEIPT_VAULT`, … from the `*_BEACON_INDEX` constants.
    pub index_name: Option<String>,
    pub codehash: Option<String>,
    /// The `LibProdDeployV4` constant of the target implementation.
    pub target_const: Option<String>,
    pub target_impl: Option<String>,
    pub target_version: Option<String>,
}

/// A beacon-set library and its resolved entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeaconSet {
    pub lib: String,
    pub beacons: Vec<BeaconAt>,
}

/// Everything the per-chain beacon checks compare against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BeaconsExpect {
    /// `(network, beacon-set library)`.
    pub chains: Vec<(String, String)>,
    pub sets: Vec<BeaconSet>,
    pub slots: Vec<Slot>,
    /// `BEACON_INITIAL_OWNER`, so an owner still on the deploy EOA is named.
    pub deploy_eoa: Option<String>,
    pub frozen: Vec<FrozenPin>,
}

/// Read the beacon expectations. `fetch` reads a repo path (`""` when it could
/// not be read); `v4_lib` is `src/generated/LibProdDeployV4.sol`, where the
/// target implementations are pinned; `frozen` are the release pins the
/// generated lib's aliases resolve through.
pub fn parse_expect(
    fetch: &dyn Fn(&str) -> String,
    v4_lib: &str,
    frozen: &[FrozenPin],
    deploy_eoa: Option<String>,
) -> BeaconsExpect {
    let inv = fetch(BEACON_INVARIANTS);
    let inv_dir = BEACON_INVARIANTS.rsplit_once('/').map_or("", |(d, _)| d);
    let inv_imports = imports(&inv, inv_dir);
    let chains = parse_chain_sets(&inv);
    let mut cache: Vec<(String, String)> = Vec::new();
    let mut read = |path: &str| -> String {
        if let Some((_, s)) = cache.iter().find(|(p, _)| p == path) {
            return s.clone();
        }
        let s = fetch(path);
        cache.push((path.to_string(), s.clone()));
        s
    };

    let mut sets: Vec<BeaconSet> = Vec::new();
    for (_, lib) in &chains {
        if sets.iter().any(|s| s.lib == *lib) {
            continue;
        }
        let Some((_, path)) = inv_imports.iter().find(|(n, _)| n == lib) else {
            sets.push(BeaconSet {
                lib: lib.clone(),
                beacons: Vec::new(),
            });
            continue;
        };
        let src = read(path);
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let lib_imports = imports(&src, &dir);
        let mut resolve = |lib: &str, name: &str| -> Option<String> {
            let (_, p) = lib_imports.iter().find(|(n, _)| n == lib)?;
            resolve_generated_address(&read(p), frozen, name)
        };
        let beacons = parse_beacon_refs(&src)
            .into_iter()
            .map(|r| match r {
                BeaconRef::Constant { lib, name } => BeaconAt::Pinned {
                    address: resolve(&lib, &name),
                    source: format!("{lib}.{name}"),
                },
                BeaconRef::Getter { lib, name, getter } => BeaconAt::Getter {
                    deployer: resolve(&lib, &name),
                    source: format!("{lib}.{name}.{getter}()"),
                    getter,
                },
                BeaconRef::Unparsed(e) => BeaconAt::Unparsed(e),
            })
            .collect();
        sets.push(BeaconSet {
            lib: lib.clone(),
            beacons,
        });
    }

    let indices = parse_beacon_indices(&inv);
    let codehashes = parse_beacon_codehashes(&inv);
    let n = sets
        .iter()
        .map(|s| s.beacons.len())
        .chain([indices.len(), codehashes.len()])
        .max()
        .unwrap_or(0);
    let slots = (0..n)
        .map(|i| {
            let index_name = indices.get(i).cloned().flatten();
            let target_const = index_name.as_deref().and_then(|ix| {
                TARGET_IMPLS
                    .iter()
                    .find(|(k, _)| *k == ix)
                    .map(|(_, c)| c.to_string())
            });
            Slot {
                codehash: codehashes.get(i).cloned().flatten(),
                target_impl: target_const
                    .as_deref()
                    .and_then(|c| resolve_generated_address(v4_lib, frozen, c)),
                target_version: target_const.as_deref().and_then(version_of),
                target_const,
                index_name,
            }
        })
        .collect();

    BeaconsExpect {
        chains,
        sets,
        slots,
        deploy_eoa,
        frozen: frozen.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Per-chain verdicts.
// ---------------------------------------------------------------------------

/// An `address` read that must equal `expected`.
pub(crate) fn address_row(fields: Value, expected: Option<&str>, actual: Option<&str>) -> Value {
    let v = match (expected, actual) {
        (Some(e), Some(a)) => Verdict::of(Some(same(e, a))),
        _ => Verdict::Unknown,
    };
    row(fields, "equal", json!(expected), json!(actual), v)
}

/// Who a live owner is: the chain's `safe`, the deploy EOA (`legacy`),
/// `foreign`, or `unknown` when it (or the Safe pin) was not read.
fn owner_label(owner: Option<&str>, safe: Option<&str>, eoa: Option<&str>) -> &'static str {
    match owner {
        None => "unknown",
        Some(o) if safe.is_some_and(|s| same(s, o)) => "safe",
        Some(o) if eoa.is_some_and(|e| same(e, o)) => "legacy",
        Some(_) if safe.is_none() => "unknown",
        Some(_) => "foreign",
    }
}

/// The release a live implementation is: the target's version, the release of
/// the frozen pin at that address, `unrecognised`, or `unknown` when unread.
fn impl_version(live: Option<&str>, slot: &Slot, frozen: &[FrozenPin]) -> String {
    let Some(l) = live else {
        return "unknown".to_string();
    };
    if slot.target_impl.as_deref().is_some_and(|t| same(t, l)) {
        if let Some(v) = &slot.target_version {
            return v.clone();
        }
    }
    frozen
        .iter()
        .find(|p| p.address.as_deref().is_some_and(|a| same(a, l)))
        .map(|p| p.release.replace('_', "."))
        .unwrap_or_else(|| "unrecognised".to_string())
}

/// One beacon's document, and the address it was found at.
fn beacon_doc(
    exp: &BeaconsExpect,
    index: usize,
    at: &BeaconAt,
    safe: Option<&str>,
    r: &dyn ChainReads,
) -> (Value, Option<String>) {
    let slot = exp.slots.get(index).cloned().unwrap_or_default();
    let (source, address) = match at {
        BeaconAt::Pinned { source, address } => (source.clone(), address.clone()),
        BeaconAt::Getter {
            source,
            deployer,
            getter,
        } => (
            source.clone(),
            deployer
                .as_deref()
                .and_then(|d| r.call_address(d, &getter_calldata(getter))),
        ),
        BeaconAt::Unparsed(e) => (e.clone(), None),
    };
    let a = address.as_deref();
    let code = a.and_then(|a| r.code(a));
    let owner = a.and_then(|a| r.call_address(a, &rpc::owner_calldata()));
    let implementation = a.and_then(|a| r.call_address(a, &rpc::implementation_calldata()));
    let accepted: Vec<String> = slot.codehash.iter().cloned().collect();
    let checks = vec![
        codehash_row(json!({"check": "codehash"}), &accepted, code.as_deref()),
        address_row(json!({"check": "owner"}), safe, owner.as_deref()),
        address_row(
            json!({"check": "implementation", "target": slot.target_const}),
            slot.target_impl.as_deref(),
            implementation.as_deref(),
        ),
    ];
    let failed = |c: &str| {
        checks
            .iter()
            .any(|x| x["check"] == c && x["status"] == "fail")
    };
    let status = if failed("owner") {
        "drift"
    } else if failed("implementation") {
        "behind"
    } else if failed("codehash") {
        "mismatch"
    } else if checks.iter().all(|x| x["status"] == "pass") {
        "healthy"
    } else {
        "unknown"
    };
    let at_target = match (implementation.as_deref(), slot.target_impl.as_deref()) {
        (Some(l), Some(t)) => Some(same(l, t)),
        _ => None,
    };
    let live_codehash = checks[0]["actual"].clone();
    let mut doc = subject(a, checks);
    if let Some(o) = doc.as_object_mut() {
        let name = slot
            .index_name
            .as_deref()
            .map(label)
            .unwrap_or_else(|| format!("Beacon {index}"));
        for (k, v) in [
            ("name", json!(name)),
            ("index", json!(index)),
            ("source", json!(source)),
            ("owner", json!(owner)),
            (
                "ownerLabel",
                json!(owner_label(
                    owner.as_deref(),
                    safe,
                    exp.deploy_eoa.as_deref()
                )),
            ),
            ("implementation", json!(implementation)),
            (
                "implVersion",
                json!(impl_version(implementation.as_deref(), &slot, &exp.frozen)),
            ),
            ("targetImpl", json!(slot.target_impl)),
            ("targetVersion", json!(slot.target_version)),
            ("atTarget", json!(at_target)),
            ("codehash", live_codehash),
            ("expectedCodehash", json!(slot.codehash)),
            ("status", json!(status)),
        ] {
            o.insert(k.into(), v);
        }
    }
    (doc, address)
}

/// A chain whose beacon set could not be read at all. Emitted instead of
/// dropping the chain: a dropped chain renders as no section, which reads as
/// "this chain has no production beacons" rather than "this broke".
pub fn beacons_unavailable(network: &str, rpc_host: Option<&str>, reason: &str) -> Value {
    json!({
        "network": network,
        "rpcHost": rpc_host,
        "unavailable": true,
        "reason": reason,
    })
}

/// One chain's `deploymentBeacons` block, and the beacon addresses it found in
/// index order (`None` where unread), for the token checks that must proxy
/// through them.
pub fn chain_doc(
    org: &str,
    repo: &str,
    exp: &BeaconsExpect,
    pin: &ChainPin,
    r: &dyn ChainReads,
) -> (Value, Vec<Option<String>>) {
    let host = pin.rpc_host.as_deref();
    let Some((_, lib)) = exp.chains.iter().find(|(n, _)| *n == pin.network) else {
        let reason = "LibBeaconInvariants.prodBeaconsForChainId names no beacon set for this chain";
        return (beacons_unavailable(&pin.network, host, reason), Vec::new());
    };
    let set = exp.sets.iter().find(|s| s.lib == *lib);
    let Some(set) = set.filter(|s| !s.beacons.is_empty()) else {
        let reason = format!("the scan could not read {lib}.beacons()");
        return (beacons_unavailable(&pin.network, host, &reason), Vec::new());
    };
    let safe = pin.safe.as_deref();
    let (docs, addresses): (Vec<Value>, Vec<Option<String>>) = set
        .beacons
        .iter()
        .enumerate()
        .map(|(i, at)| beacon_doc(exp, i, at, safe, r))
        .unzip();
    let sum = |k: &str| {
        docs.iter()
            .map(|d| d[k].as_u64().unwrap_or(0) as usize)
            .sum::<usize>()
    };
    let (passed, failed, unknown) = (sum("passed"), sum("failed"), sum("unknown"));
    let mut versions: Vec<&str> = Vec::new();
    for d in &docs {
        if let Some(v) = d["targetVersion"].as_str() {
            if !versions.contains(&v) {
                versions.push(v);
            }
        }
    }
    let doc = json!({
        "org": org,
        "repo": repo,
        "network": pin.network,
        "rpcHost": host,
        "safeOwner": safe,
        "beaconSet": lib,
        "targetVersion": versions.join(" / "),
        "total": docs.len(),
        "healthy": docs.iter().filter(|d| d["status"] == "healthy").count(),
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "checkTotal": passed + failed + unknown,
        "state": rollup(passed, failed, unknown),
        "beacons": docs,
    });
    (doc, addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploystate::{frozen_pin, NoReads};
    use crate::rpc::keccak256_hex;
    use std::collections::HashMap;

    // Production values as st0x.deploy pins them (3c6db51). The receipt and
    // receipt-vault beacons off Base are pinned nowhere, so they are stand-ins
    // the fake set deployer answers with.
    const BASE_SAFE: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const SAFE: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const EOA: &str = "0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b";
    const BASE_RECEIPT_BEACON: &str = "0x86e93c39B095be0B0054C8488E26466Ee027D79a";
    const BASE_VAULT_BEACON: &str = "0xEa084c8F4331CDF3328E772781b59F8A24F28F1A";
    const BASE_WRAPPED_BEACON: &str = "0x4c2d2d3Bf1232bf0d3FB7123007A9B8444637bC8";
    const ORCH_BEACON: &str = "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613";
    const SET_DEPLOYER: &str = "0xd64246e6b25F745f005E6233e050C9B879E660Dc";
    const WRAPPED_BEACON: &str = "0x9FD790f65CA3aF2772358c653F097f0a4c7EE7d2";
    const RECEIPT_BEACON: &str = "0x000000000000000000000000000000000000bea1";
    const VAULT_BEACON: &str = "0x000000000000000000000000000000000000bea2";
    const RECEIPT_IMPL: &str = "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E";
    const VAULT_IMPL: &str = "0x52174538a83b18462c5cF898016539B086c920C7";
    const WRAPPED_IMPL: &str = "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099";
    const ORCH_IMPL: &str = "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72";
    const RECEIPT_IMPL_0_1_1: &str = "0x00000000000000000000000000000000000a0111";
    const VAULT_IMPL_0_1_1: &str = "0x00000000000000000000000000000000000a0112";

    // Stand-in runtime code for the V1-build token beacons and the 0.1.30
    // orchestrator beacon.
    const TOKEN_BEACON_CODE: &str = "0xbe01";
    const ORCH_BEACON_CODE: &str = "0xbe30";

    fn h(code: &str) -> String {
        keccak256_hex(code).unwrap()
    }

    fn beacon_invariants() -> String {
        format!(
            r#"
import {{IBeacon}} from "@openzeppelin-contracts-5.6.1/proxy/beacon/IBeacon.sol";
import {{LibProdBeaconsBase}} from "./LibProdBeaconsBase.sol";
import {{LibProdBeacons0_1_1}} from "./LibProdBeacons0_1_1.sol";
import {{LibSafeInvariants}} from "./LibSafeInvariants.sol";
library LibBeaconInvariants {{
    bytes32 internal constant UPGRADEABLE_BEACON_CODEHASH =
        {};
    bytes32 internal constant UPGRADEABLE_BEACON_CODEHASH_0_1_30 =
        {};
    uint256 internal constant RECEIPT_BEACON_INDEX = 0;
    uint256 internal constant RECEIPT_VAULT_BEACON_INDEX = 1;
    uint256 internal constant WRAPPED_TOKEN_VAULT_BEACON_INDEX = 2;
    uint256 internal constant ORCHESTRATOR_BEACON_INDEX = 3;
    function prodBeaconCodehashesForChainId(uint256) internal pure returns (bytes32[4] memory) {{
        return [
            UPGRADEABLE_BEACON_CODEHASH,
            UPGRADEABLE_BEACON_CODEHASH,
            UPGRADEABLE_BEACON_CODEHASH,
            UPGRADEABLE_BEACON_CODEHASH_0_1_30
        ];
    }}
    function prodBeaconsForChainId(uint256 chainId) internal view returns (address[4] memory) {{
        if (chainId == LibSafeInvariants.BASE_CHAIN_ID) {{
            return LibProdBeaconsBase.beacons();
        }}
        if (chainId == LibSafeInvariants.ETHEREUM_CHAIN_ID) {{
            return LibProdBeacons0_1_1.beacons();
        }}
        if (chainId == LibSafeInvariants.HYPEREVM_CHAIN_ID) {{
            // HyperEVM bootstraps at 0.1.1 too.
            return LibProdBeacons0_1_1.beacons();
        }}
        if (chainId == LibSafeInvariants.ROBINHOOD_CHAIN_ID) {{
            return LibProdBeacons0_1_1.beacons();
        }}
        if (chainId == LibSafeInvariants.BSC_CHAIN_ID) {{
            // BNB Smart Chain too.
            return LibProdBeacons0_1_1.beacons();
        }}
        revert UnsupportedChainForProdBeacons(chainId);
    }}
}}
"#,
            h(TOKEN_BEACON_CODE),
            h(ORCH_BEACON_CODE),
        )
    }

    // `implementations()` is the stale 0.1.1 list the targets must not come from.
    const BASE_SET: &str = r#"
import {LibProdDeployV1} from "./LibProdDeployV1.sol";
import {LibProdDeployV4} from "../generated/LibProdDeployV4.sol";
library LibProdBeaconsBase {
    function beacons() internal pure returns (address[4] memory) {
        return [
            LibProdDeployV1.STOX_RECEIPT_BEACON_V1,
            LibProdDeployV1.STOX_RECEIPT_VAULT_BEACON_V1,
            LibProdDeployV1.STOX_WRAPPED_TOKEN_VAULT_BEACON_V1,
            LibProdDeployV4.ST0X_ORCHESTRATOR_BEACON
        ];
    }
    function implementations() internal pure returns (address[4] memory) {
        return [
            LibProdDeployV4.STOX_RECEIPT_0_1_1,
            LibProdDeployV4.STOX_RECEIPT_VAULT_0_1_1,
            LibProdDeployV4.STOX_WRAPPED_TOKEN_VAULT_0_1_1,
            LibProdDeployV4.ST0X_ORCHESTRATOR_0_1_30
        ];
    }
}
"#;

    const SET_0_1_1: &str = r#"
import {IST0xVaultBeaconSet} from "../interface/IST0xVaultBeaconSet.sol";
import {LibProdDeployV4} from "../generated/LibProdDeployV4.sol";
library LibProdBeacons0_1_1 {
    function beacons() internal view returns (address[4] memory) {
        IST0xVaultBeaconSet deployer =
            IST0xVaultBeaconSet(LibProdDeployV4.STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_BEACON_SET_DEPLOYER_0_1_1);
        return [
            address(deployer.iReceiptBeacon()),
            address(deployer.iOffchainAssetReceiptVaultBeacon()),
            LibProdDeployV4.STOX_WRAPPED_TOKEN_VAULT_BEACON_0_1_1,
            LibProdDeployV4.ST0X_ORCHESTRATOR_BEACON
        ];
    }
}
"#;

    fn v1_lib() -> String {
        format!(
            "library LibProdDeployV1 {{
    address constant STOX_RECEIPT_BEACON_V1 = address({BASE_RECEIPT_BEACON});
    address constant STOX_RECEIPT_VAULT_BEACON_V1 = address({BASE_VAULT_BEACON});
    address constant STOX_WRAPPED_TOKEN_VAULT_BEACON_V1 = address({BASE_WRAPPED_BEACON});
}}"
        )
    }

    // (release, contract, alias stem, deployed address), in the generated lib's
    // import order.
    const POINTERS: [(&str, &str, &str, &str); 8] = [
        ("0_1_1", "StoxReceipt", "STOX_RECEIPT", RECEIPT_IMPL_0_1_1),
        (
            "0_1_1",
            "StoxReceiptVault",
            "STOX_RECEIPT_VAULT",
            VAULT_IMPL_0_1_1,
        ),
        (
            "0_1_1",
            "StoxWrappedTokenVault",
            "STOX_WRAPPED_TOKEN_VAULT",
            WRAPPED_IMPL,
        ),
        (
            "0_1_1",
            "StoxWrappedTokenVaultBeacon",
            "STOX_WRAPPED_TOKEN_VAULT_BEACON",
            WRAPPED_BEACON,
        ),
        (
            "0_1_1",
            "StoxOffchainAssetReceiptVaultBeaconSetDeployer",
            "STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_BEACON_SET_DEPLOYER",
            SET_DEPLOYER,
        ),
        ("0_1_30", "StoxReceipt", "STOX_RECEIPT", RECEIPT_IMPL),
        (
            "0_1_30",
            "StoxReceiptVault",
            "STOX_RECEIPT_VAULT",
            VAULT_IMPL,
        ),
        ("0_1_30", "ST0xOrchestrator", "ST0X_ORCHESTRATOR", ORCH_IMPL),
    ];

    fn v4_lib() -> String {
        let mut s = String::new();
        for (release, contract, stem, _) in POINTERS {
            s.push_str(&format!(
                "import {{\n    DEPLOYED_ADDRESS as {stem}_ADDRESS_{release}_GEN,\n    BYTECODE_HASH as {stem}_CODEHASH_{release}_GEN\n}} from \"./{release}/{contract}.pointers.sol\";\n"
            ));
        }
        s.push_str("library LibProdDeployV4 {\n");
        s.push_str(&format!(
            "    address constant BEACON_INITIAL_OWNER = address({EOA});\n    address constant ST0X_ORCHESTRATOR_BEACON = address({ORCH_BEACON});\n"
        ));
        for (release, _, stem, _) in POINTERS {
            s.push_str(&format!(
                "    address constant {stem}_{release} =\n        {stem}_ADDRESS_{release}_GEN;\n"
            ));
        }
        s.push_str("}\n");
        s
    }

    fn frozen() -> Vec<FrozenPin> {
        POINTERS
            .iter()
            .map(|(release, contract, _, address)| {
                let path = format!("src/generated/{release}/{contract}.pointers.sol");
                let src = format!("address constant DEPLOYED_ADDRESS = address({address});");
                frozen_pin(release, contract, &path, &src)
            })
            .collect()
    }

    fn sources() -> HashMap<String, String> {
        [
            (BEACON_INVARIANTS.to_string(), beacon_invariants()),
            (
                "src/lib/LibProdBeaconsBase.sol".to_string(),
                BASE_SET.to_string(),
            ),
            (
                "src/lib/LibProdBeacons0_1_1.sol".to_string(),
                SET_0_1_1.to_string(),
            ),
            ("src/lib/LibProdDeployV1.sol".to_string(), v1_lib()),
            ("src/generated/LibProdDeployV4.sol".to_string(), v4_lib()),
        ]
        .into_iter()
        .collect()
    }

    fn expect_from(src: &HashMap<String, String>) -> BeaconsExpect {
        let fetch = |p: &str| src.get(p).cloned().unwrap_or_default();
        let v4 = fetch("src/generated/LibProdDeployV4.sol");
        parse_expect(&fetch, &v4, &frozen(), Some(EOA.to_lowercase()))
    }

    fn expect() -> BeaconsExpect {
        expect_from(&sources())
    }

    fn pin(network: &str) -> ChainPin {
        ChainPin {
            network: network.to_string(),
            authoriser: None,
            safe: Some(if network == "base" { BASE_SAFE } else { SAFE }.to_string()),
            rpc_host: Some("rpc.example".to_string()),
        }
    }

    /// A chain held in memory. An address with no code set reads as empty, as
    /// on a real chain; a call answers only where set.
    #[derive(Default, Clone)]
    struct Fake {
        code: HashMap<String, String>,
        calls: HashMap<(String, String), String>,
    }

    impl Fake {
        fn answer(&mut self, contract: &str, calldata: &str, value: &str) {
            self.calls.insert(
                (contract.to_lowercase(), calldata.to_string()),
                value.to_lowercase(),
            );
        }
        fn forget(&mut self, contract: &str, calldata: &str) {
            self.calls
                .remove(&(contract.to_lowercase(), calldata.to_string()));
        }
        fn beacon(&mut self, at: &str, code: &str, owner: &str, implementation: &str) {
            self.code.insert(at.to_lowercase(), code.to_string());
            self.answer(at, &rpc::owner_calldata(), owner);
            self.answer(at, &rpc::implementation_calldata(), implementation);
        }
    }

    impl ChainReads for Fake {
        fn code(&self, address: &str) -> Option<String> {
            Some(
                self.code
                    .get(&address.to_lowercase())
                    .cloned()
                    .unwrap_or_else(|| "0x".to_string()),
            )
        }
        fn storage(&self, _: &str, _: &str) -> Option<[u8; 32]> {
            None
        }
        fn has_role(&self, _: &str, _: [u8; 32], _: &str) -> Option<bool> {
            None
        }
        fn owners(&self, _: &str) -> Option<Vec<String>> {
            None
        }
        fn threshold(&self, _: &str) -> Option<u64> {
            None
        }
        fn modules(&self, _: &str, _: &str, _: u64) -> Option<Vec<String>> {
            None
        }
        fn vault_logic_is_expected(&self, _: &str) -> Option<bool> {
            None
        }
        fn call_address(&self, contract: &str, calldata: &str) -> Option<String> {
            self.calls
                .get(&(contract.to_lowercase(), calldata.to_string()))
                .cloned()
        }
        fn call_string(&self, _: &str, _: &str) -> Option<String> {
            None
        }
    }

    /// Every beacon on `network` as the #182 end state leaves it.
    fn healthy(network: &str) -> Fake {
        let safe = if network == "base" { BASE_SAFE } else { SAFE };
        let mut f = Fake::default();
        let (receipt, vault, wrapped) = if network == "base" {
            (BASE_RECEIPT_BEACON, BASE_VAULT_BEACON, BASE_WRAPPED_BEACON)
        } else {
            f.answer(
                SET_DEPLOYER,
                &getter_calldata("iReceiptBeacon"),
                RECEIPT_BEACON,
            );
            f.answer(
                SET_DEPLOYER,
                &getter_calldata("iOffchainAssetReceiptVaultBeacon"),
                VAULT_BEACON,
            );
            (RECEIPT_BEACON, VAULT_BEACON, WRAPPED_BEACON)
        };
        f.beacon(receipt, TOKEN_BEACON_CODE, safe, RECEIPT_IMPL);
        f.beacon(vault, TOKEN_BEACON_CODE, safe, VAULT_IMPL);
        f.beacon(wrapped, TOKEN_BEACON_CODE, safe, WRAPPED_IMPL);
        f.beacon(ORCH_BEACON, ORCH_BEACON_CODE, safe, ORCH_IMPL);
        f
    }

    fn doc(network: &str, r: &dyn ChainReads) -> Value {
        chain_doc("o", "r", &expect(), &pin(network), r).0
    }

    fn beacon<'a>(d: &'a Value, name: &str) -> &'a Value {
        d["beacons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    fn status_of<'a>(b: &'a Value, check: &str) -> &'a Value {
        &b["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["check"] == check)
            .unwrap()["status"]
    }

    #[test]
    fn reads_the_chain_dispatch_indices_and_codehashes() {
        let inv = beacon_invariants();
        let base = ("base".to_string(), "LibProdBeaconsBase".to_string());
        let other = |n: &str| (n.to_string(), "LibProdBeacons0_1_1".to_string());
        assert_eq!(
            parse_chain_sets(&inv),
            vec![
                base,
                other("ethereum"),
                other("hyperevm"),
                other("robinhood"),
                other("bsc")
            ]
        );
        let names: Vec<Option<String>> = [
            "RECEIPT",
            "RECEIPT_VAULT",
            "WRAPPED_TOKEN_VAULT",
            "ORCHESTRATOR",
        ]
        .iter()
        .map(|s| Some(s.to_string()))
        .collect();
        assert_eq!(parse_beacon_indices(&inv), names);
        let (t, o) = (Some(h(TOKEN_BEACON_CODE)), Some(h(ORCH_BEACON_CODE)));
        assert_eq!(
            parse_beacon_codehashes(&inv),
            vec![t.clone(), t.clone(), t, o]
        );
    }

    #[test]
    fn reads_constant_and_getter_entries_in_order() {
        let c = |lib: &str, name: &str| BeaconRef::Constant {
            lib: lib.to_string(),
            name: name.to_string(),
        };
        assert_eq!(
            parse_beacon_refs(BASE_SET),
            vec![
                c("LibProdDeployV1", "STOX_RECEIPT_BEACON_V1"),
                c("LibProdDeployV1", "STOX_RECEIPT_VAULT_BEACON_V1"),
                c("LibProdDeployV1", "STOX_WRAPPED_TOKEN_VAULT_BEACON_V1"),
                c("LibProdDeployV4", "ST0X_ORCHESTRATOR_BEACON"),
            ]
        );
        let g = |getter: &str| BeaconRef::Getter {
            lib: "LibProdDeployV4".to_string(),
            name: "STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_BEACON_SET_DEPLOYER_0_1_1".to_string(),
            getter: getter.to_string(),
        };
        assert_eq!(
            parse_beacon_refs(SET_0_1_1),
            vec![
                g("iReceiptBeacon"),
                g("iOffchainAssetReceiptVaultBeacon"),
                c("LibProdDeployV4", "STOX_WRAPPED_TOKEN_VAULT_BEACON_0_1_1"),
                c("LibProdDeployV4", "ST0X_ORCHESTRATOR_BEACON"),
            ]
        );
        // An entry of an unread shape keeps its place.
        let odd = "function beacons() internal pure returns (address[2] memory) { return [foo(1, 2), L.X]; }";
        assert_eq!(
            parse_beacon_refs(odd),
            vec![BeaconRef::Unparsed("foo(1, 2)".to_string()), c("L", "X")]
        );
    }

    #[test]
    fn imports_resolve_relative_to_the_importing_file() {
        assert_eq!(
            imports(BASE_SET, "src/lib"),
            vec![
                (
                    "LibProdDeployV1".to_string(),
                    "src/lib/LibProdDeployV1.sol".to_string()
                ),
                (
                    "LibProdDeployV4".to_string(),
                    "src/generated/LibProdDeployV4.sol".to_string()
                ),
            ]
        );
        let multi = r#"import {A, B as C} from "../x/Y.sol"; import {Z} from "forge-std/Z.sol";"#;
        assert_eq!(
            imports(multi, "src/lib"),
            vec![
                ("A".to_string(), "src/x/Y.sol".to_string()),
                ("C".to_string(), "src/x/Y.sol".to_string()),
            ]
        );
    }

    /// The four hardcoded target names resolve, through the generated lib's
    /// aliases, to the 0.1.30 receipt, receipt-vault and orchestrator impls and
    /// the 0.1.1 wrapped-token-vault impl: the #182 targets.
    #[test]
    fn targets_resolve_to_the_issue_implementations() {
        let e = expect();
        let got: Vec<(Option<String>, Option<String>, Option<String>)> = e
            .slots
            .iter()
            .map(|s| {
                (
                    s.index_name.clone(),
                    s.target_impl.clone(),
                    s.target_version.clone(),
                )
            })
            .collect();
        let want = |ix: &str, a: &str, v: &str| {
            (
                Some(ix.to_string()),
                Some(a.to_string()),
                Some(v.to_string()),
            )
        };
        assert_eq!(
            got,
            vec![
                want("RECEIPT", RECEIPT_IMPL, "0.1.30"),
                want("RECEIPT_VAULT", VAULT_IMPL, "0.1.30"),
                want("WRAPPED_TOKEN_VAULT", WRAPPED_IMPL, "0.1.1"),
                want("ORCHESTRATOR", ORCH_IMPL, "0.1.30"),
            ]
        );
        // Not the stale implementations() list.
        assert_ne!(e.slots[0].target_impl.as_deref(), Some(RECEIPT_IMPL_0_1_1));
    }

    /// The two getters are the only way the 0.1.1 receipt and receipt-vault
    /// beacons are addressable. A wrong selector reverts, which reads as an
    /// unread beacon rather than a bad selector, so the encoding is pinned
    /// against `cast sig` output.
    #[test]
    fn getter_calldata_is_the_selector_of_the_named_getter() {
        assert_eq!(getter_calldata("iReceiptBeacon"), "0x2c9b7f40");
        assert_eq!(
            getter_calldata("iOffchainAssetReceiptVaultBeacon"),
            "0x2f77a1c1"
        );
        assert_eq!(getter_calldata("owner"), rpc::owner_calldata());
    }

    #[test]
    fn labels_come_from_the_index_names() {
        assert_eq!(label("RECEIPT"), "Receipt beacon");
        assert_eq!(label("RECEIPT_VAULT"), "Receipt-vault beacon");
        assert_eq!(label("WRAPPED_TOKEN_VAULT"), "Wrapped-token-vault beacon");
        assert_eq!(label("ORCHESTRATOR"), "Orchestrator beacon");
    }

    #[test]
    fn the_end_state_passes_every_row_on_every_chain() {
        for network in ["base", "ethereum", "hyperevm", "robinhood", "bsc"] {
            let (d, addresses) = chain_doc("o", "r", &expect(), &pin(network), &healthy(network));
            assert_eq!(d["state"], "pass", "{network}: {d:#}");
            assert_eq!(
                (d["passed"].as_u64(), d["checkTotal"].as_u64()),
                (Some(12), Some(12))
            );
            assert_eq!(
                (d["healthy"].as_u64(), d["total"].as_u64()),
                (Some(4), Some(4))
            );
            assert_eq!(d["targetVersion"], "0.1.30 / 0.1.1");
            let wrapped = beacon(&d, "Wrapped-token-vault beacon");
            assert_eq!(wrapped["implVersion"], "0.1.1");
            assert_eq!(wrapped["ownerLabel"], "safe");
            let expected: Vec<Option<String>> = if network == "base" {
                [
                    BASE_RECEIPT_BEACON,
                    BASE_VAULT_BEACON,
                    BASE_WRAPPED_BEACON,
                    ORCH_BEACON,
                ]
            } else {
                [RECEIPT_BEACON, VAULT_BEACON, WRAPPED_BEACON, ORCH_BEACON]
            }
            .iter()
            .map(|a| Some(a.to_lowercase()))
            .collect();
            let got: Vec<Option<String>> = addresses
                .iter()
                .map(|a| a.as_ref().map(|s| s.to_lowercase()))
                .collect();
            assert_eq!(got, expected);
        }
        let d = doc("ethereum", &healthy("ethereum"));
        assert_eq!(
            beacon(&d, "Receipt beacon")["source"],
            "LibProdDeployV4.STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_BEACON_SET_DEPLOYER_0_1_1.iReceiptBeacon()"
        );
    }

    #[test]
    fn the_owner_must_be_the_chain_safe_itself() {
        // Another chain's Safe is not this chain's Safe.
        let mut f = healthy("ethereum");
        f.answer(WRAPPED_BEACON, &rpc::owner_calldata(), BASE_SAFE);
        let d = doc("ethereum", &f);
        let b = beacon(&d, "Wrapped-token-vault beacon");
        assert_eq!(
            (&b["status"], &b["ownerLabel"], &b["state"]),
            (&json!("drift"), &json!("foreign"), &json!("fail"))
        );
        assert_eq!(status_of(b, "owner"), "fail");
        assert_eq!(d["state"], "fail");

        let mut f = healthy("base");
        f.answer(ORCH_BEACON, &rpc::owner_calldata(), EOA);
        let b = beacon(&doc("base", &f), "Orchestrator beacon").clone();
        assert_eq!(
            (&b["status"], &b["ownerLabel"]),
            (&json!("drift"), &json!("legacy"))
        );
    }

    #[test]
    fn a_beacon_still_on_0_1_1_is_behind() {
        let mut f = healthy("bsc");
        f.answer(
            RECEIPT_BEACON,
            &rpc::implementation_calldata(),
            RECEIPT_IMPL_0_1_1,
        );
        let d = doc("bsc", &f);
        let b = beacon(&d, "Receipt beacon");
        assert_eq!(
            (&b["status"], &b["implVersion"], &b["atTarget"]),
            (&json!("behind"), &json!("0.1.1"), &json!(false))
        );
        assert_eq!(status_of(b, "implementation"), "fail");

        f.answer(
            RECEIPT_BEACON,
            &rpc::implementation_calldata(),
            "0x00000000000000000000000000000000deadbeef",
        );
        let d = doc("bsc", &f);
        assert_eq!(beacon(&d, "Receipt beacon")["implVersion"], "unrecognised");
    }

    #[test]
    fn a_wrong_or_missing_codehash_fails() {
        let mut f = healthy("robinhood");
        // The token-beacon build where the orchestrator's belongs.
        f.code
            .insert(ORCH_BEACON.to_lowercase(), TOKEN_BEACON_CODE.to_string());
        let d = doc("robinhood", &f);
        let b = beacon(&d, "Orchestrator beacon");
        assert_eq!(
            (&b["status"], status_of(b, "codehash")),
            (&json!("mismatch"), &json!("fail"))
        );

        let mut f = healthy("robinhood");
        f.code.remove(&WRAPPED_BEACON.to_lowercase());
        let d = doc("robinhood", &f);
        let b = beacon(&d, "Wrapped-token-vault beacon");
        assert_eq!(
            (&b["codehash"], status_of(b, "codehash")),
            (&json!("no code"), &json!("fail"))
        );
    }

    #[test]
    fn an_unread_row_does_not_hide_a_known_failure() {
        let mut f = healthy("hyperevm");
        f.forget(VAULT_BEACON, &rpc::owner_calldata());
        f.answer(
            VAULT_BEACON,
            &rpc::implementation_calldata(),
            VAULT_IMPL_0_1_1,
        );
        let d = doc("hyperevm", &f);
        let b = beacon(&d, "Receipt-vault beacon");
        assert_eq!(status_of(b, "owner"), "unknown");
        assert_eq!(
            (&b["status"], &b["state"], &d["state"]),
            (&json!("behind"), &json!("fail"), &json!("fail"))
        );
    }

    #[test]
    fn an_unread_beacon_is_unknown_not_failed() {
        // The set deployer does not answer, so neither token beacon is found.
        let mut f = healthy("ethereum");
        f.forget(SET_DEPLOYER, &getter_calldata("iReceiptBeacon"));
        let (d, addresses) = chain_doc("o", "r", &expect(), &pin("ethereum"), &f);
        let b = beacon(&d, "Receipt beacon");
        assert_eq!(
            (&b["address"], &b["status"], &b["unknown"]),
            (&Value::Null, &json!("unknown"), &json!(3))
        );
        assert_eq!(addresses[0], None);
        assert_eq!((&d["state"], &d["failed"]), (&json!("unknown"), &json!(0)));

        // No endpoint at all: every row unknown, none failed.
        let d = doc("bsc", &NoReads);
        assert_eq!(
            (&d["unknown"], &d["failed"], &d["state"]),
            (&json!(12), &json!(0), &json!("unknown"))
        );

        // No Safe pin: the owner row cannot be judged.
        let mut p = pin("base");
        p.safe = None;
        let d = chain_doc("o", "r", &expect(), &p, &healthy("base")).0;
        assert_eq!(status_of(beacon(&d, "Receipt beacon"), "owner"), "unknown");
    }

    #[test]
    fn a_chain_without_a_beacon_set_is_reported_unavailable() {
        let d = doc("sepolia", &NoReads);
        assert_eq!(
            (&d["network"], &d["unavailable"]),
            (&json!("sepolia"), &json!(true))
        );

        let mut src = sources();
        src.remove("src/lib/LibProdBeacons0_1_1.sol");
        let (d, addresses) = chain_doc("o", "r", &expect_from(&src), &pin("bsc"), &NoReads);
        assert_eq!(d["unavailable"], true);
        assert_eq!(
            d["reason"],
            "the scan could not read LibProdBeacons0_1_1.beacons()"
        );
        assert!(addresses.is_empty());
    }
}
