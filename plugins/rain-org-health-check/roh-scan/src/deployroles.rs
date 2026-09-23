//! Role membership on each chain's V4 authoriser clone and orchestrator
//! instance (rain-org-health#182), rebuilt from their `RoleGranted` /
//! `RoleRevoked` history and compared to the expected set exactly. A grant the
//! deploy repo does not expect fails as surely as an expected one that is
//! missing. `hasRole` only answers for a pair someone thought to ask about; the
//! event history names every pair ever granted, so it is what finds a stray.
//!
//! The expected sets come out of st0x.deploy. On the clone they are the rows of
//! `LibAuthoriserInvariants.expectedGrants`, parsed as `deploymentGrants`
//! parses them. On the orchestrator they are `DEFAULT_ADMIN` for the chain Safe
//! and the operator roles for `GRANTEE_SERVICE_3D0C`, as `deploymentState`
//! checks them.
//!
//! Where a contract's whole history is read, the verdict is `pass` or `fail`.
//! HyperEVM and BSC have no keyless source that serves their whole history, so
//! there only the deploy-key ceremony windows are read, from the contract's
//! creation block. That gives the membership as of the window's last block, and
//! its verdict is `partial`, never `pass`: nothing after the window is read.
//!
//! A read that failed or may be incomplete is `unknown`, never `fail`. So is a
//! history that contradicts itself: OpenZeppelin `AccessControl` emits
//! `RoleGranted` only for a role not held and `RoleRevoked` only for one held,
//! so a grant of a held role or a revoke of an unheld one means an event
//! between them is missing.

use crate::deploystate::{role_word, same, Account, Expectations, DEFAULT_ADMIN, OPERATOR_ROLES};
use crate::owners::{parse_expected_grants, resolve_ident, ChainPin, OwnerSources};
use crate::rpc::{decode_role_event, Log, RoleChange, ROLE_GRANTED_TOPIC, ROLE_REVOKED_TOPIC};
use alloy_primitives::hex;
use serde_json::{json, Value};

/// The deploy repo file the clone's expected set is read from.
pub const AUTH_LIB: &str = "src/lib/LibAuthoriserInvariants.sol";

/// The deploy-key ceremony windows read where a chain's whole history cannot
/// be: `(network, contract, first block, last block)`, inclusive. Each starts
/// at the contract's creation block, so the membership rebuilt from it is the
/// membership as of its last block. The issue author set these (#182); no file
/// in st0x.deploy pins a creation block.
pub const CEREMONY_WINDOWS: &[(&str, &str, u64, u64)] = &[
    ("hyperevm", "authoriser", 41_325_390, 41_327_389),
    ("hyperevm", "orchestrator", 44_389_509, 44_391_508),
    ("bsc", "authoriser", 121_144_118, 121_146_117),
    ("bsc", "orchestrator", 121_162_568, 121_164_567),
];

/// The most requests one log read may take. A range that would need more is not
/// read at all: it is `unknown`, not a slow partial answer.
pub const MAX_LOG_REQUESTS: u64 = 400;

/// The ceremony window read for `on` (`authoriser` or `orchestrator`) on
/// `network`, or `None` where the whole history is read.
pub fn window(network: &str, on: &str) -> Option<(u64, u64)> {
    CEREMONY_WINDOWS
        .iter()
        .find(|(n, o, _, _)| *n == network && *o == on)
        .map(|(_, _, from, to)| (*from, *to))
}

/// The inclusive `from..=to` range cut into requests of at most `span` blocks.
/// `None` for an inverted range, a zero span, or more than
/// [`MAX_LOG_REQUESTS`] requests.
pub fn spans(from: u64, to: u64, span: u64) -> Option<Vec<(u64, u64)>> {
    if from > to || span == 0 {
        return None;
    }
    let n = (to - from) / span + 1;
    if n > MAX_LOG_REQUESTS {
        return None;
    }
    Some(
        (0..n)
            .map(|i| {
                let start = from + i * span;
                (start, start.saturating_add(span - 1).min(to))
            })
            .collect(),
    )
}

/// Where one chain's role events are read from.
pub trait LogReads {
    /// The host(s) the logs come from, as shown.
    fn source(&self) -> Option<String>;
    /// The last block the source serves. A whole-history read runs to it.
    /// `None` when it could not be read, or when the source cannot vouch that
    /// its history up to it is whole.
    fn head(&self) -> Option<u64>;
    /// Every log `address` emitted with first topic `topic0` in the inclusive
    /// range. `None` when any part of the read failed or may be cut short.
    fn logs(&self, address: &str, topic0: [u8; 32], from: u64, to: u64) -> Option<Vec<Log>>;
}

/// A chain with no log source: every membership on it is `unknown`.
pub struct NoLogs;

impl LogReads for NoLogs {
    fn source(&self) -> Option<String> {
        None
    }
    fn head(&self) -> Option<u64> {
        None
    }
    fn logs(&self, _: &str, _: [u8; 32], _: u64, _: u64) -> Option<Vec<Log>> {
        None
    }
}

/// Who an expected role is held by: the chain's own Safe, or a named constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grantee {
    Safe,
    Named(Account),
}

/// The expected sets and the names the page labels ids and addresses with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleExpect {
    /// The clone's `(role, grantee)` rows in map order. `None` when the grant
    /// map did not parse.
    pub clone: Option<Vec<(String, Grantee)>>,
    /// The orchestrator's `(role, grantee)` rows.
    pub orchestrator: Vec<(String, Grantee)>,
    /// `ST0X_ORCHESTRATOR_INSTANCE`.
    pub instance: Option<String>,
    /// Role names, to show a role id by name.
    pub roles: Vec<String>,
    /// Named accounts, to show an address by name. The Safe is per chain.
    pub accounts: Vec<Account>,
}

/// Read the expected sets out of the deploy repo's sources.
pub fn parse_expect(src: &OwnerSources, exp: &Expectations) -> RoleExpect {
    let clone = parse_expected_grants(src.auth_lib).map(|map| {
        map.grants
            .iter()
            .map(|g| {
                let who = if g.grantee == map.safe_param {
                    Grantee::Safe
                } else {
                    Grantee::Named(Account {
                        ident: g.grantee.clone(),
                        address: resolve_ident(src, &g.grantee),
                    })
                };
                (g.role.clone(), who)
            })
            .collect()
    });
    let operator = exp.operator.clone().unwrap_or(Account {
        ident: "GRANTEE_SERVICE_3D0C".to_string(),
        address: None,
    });
    let orchestrator = std::iter::once((DEFAULT_ADMIN.to_string(), Grantee::Safe))
        .chain(
            OPERATOR_ROLES
                .iter()
                .map(|r| (r.to_string(), Grantee::Named(operator.clone()))),
        )
        .collect();

    let mut roles: Vec<String> = vec![DEFAULT_ADMIN.to_string()];
    let named = exp
        .mapped_roles
        .iter()
        .chain(exp.orchestrator_roles.iter())
        .map(String::as_str)
        .chain(OPERATOR_ROLES);
    for r in named {
        // An `X_ADMIN` role admins the action role `X`, which the map may not
        // grant to anyone but a stray grant could still name.
        for name in std::iter::once(r).chain(r.strip_suffix("_ADMIN")) {
            if !roles.iter().any(|k| k == name) {
                roles.push(name.to_string());
            }
        }
    }

    let mut accounts: Vec<Account> = Vec::new();
    let instance = Account {
        ident: "ST0X_ORCHESTRATOR_INSTANCE".to_string(),
        address: exp.orchestrator.clone(),
    };
    let known = exp
        .grantees
        .iter()
        .chain(exp.retired.iter())
        .chain(exp.operator.iter())
        .chain(std::iter::once(&exp.deploy_key))
        .chain(exp.deploy_eoa.iter())
        .chain(std::iter::once(&instance));
    for a in known {
        if a.address.is_some() && !accounts.iter().any(|k| k.ident == a.ident) {
            accounts.push(a.clone());
        }
    }

    RoleExpect {
        clone,
        orchestrator,
        instance: exp.orchestrator.clone(),
        roles,
        accounts,
    }
}

/// A role held, who holds it, and the grant that put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub role: [u8; 32],
    /// Lowercase `0x…`.
    pub account: String,
    pub block: u64,
    /// The `sender` of the `RoleGranted` event, lowercase `0x…`.
    pub granted_by: String,
}

/// A membership rebuilt from a contract's role events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// In the order the roles were granted.
    pub members: Vec<Held>,
    pub granted: usize,
    pub revoked: usize,
}

/// Rebuild the membership of `contract` from its `RoleGranted` and
/// `RoleRevoked` logs over `from..=to`, replayed in chain order. `Err` names
/// why the logs cannot be a whole history: a log from another address, of
/// another event, outside the range, of the wrong shape, two logs at one
/// position, or a grant or revoke the membership so far contradicts. A log the
/// node marks `removed` was retracted by a reorg, so it is not history.
pub fn rebuild(
    contract: &str,
    from: u64,
    to: u64,
    granted: &[Log],
    revoked: &[Log],
) -> Result<History, String> {
    let mut events = Vec::new();
    for (topic, logs) in [(ROLE_GRANTED_TOPIC, granted), (ROLE_REVOKED_TOPIC, revoked)] {
        for log in logs.iter().filter(|l| !l.removed) {
            if !same(&log.address, contract) {
                return Err(format!(
                    "a log from {} in a read of {contract}",
                    log.address
                ));
            }
            if log.topics.first() != Some(&topic) {
                return Err(format!(
                    "a log of another event at block {}",
                    log.block_number
                ));
            }
            if log.block_number < from || log.block_number > to {
                return Err(format!(
                    "a log at block {} outside the blocks asked, {from} to {to}",
                    log.block_number
                ));
            }
            let event = decode_role_event(log).ok_or_else(|| {
                format!(
                    "a role event of the wrong shape at block {}",
                    log.block_number
                )
            })?;
            events.push((log.block_number, log.log_index, event));
        }
    }
    events.sort_by_key(|(block, index, _)| (*block, *index));
    if let Some(w) = events
        .windows(2)
        .find(|w| (w[0].0, w[0].1) == (w[1].0, w[1].1))
    {
        return Err(format!("two logs at block {} index {}", w[0].0, w[0].1));
    }
    let mut members: Vec<Held> = Vec::new();
    let (mut n_granted, mut n_revoked) = (0, 0);
    for (block, _, e) in events {
        let at = members
            .iter()
            .position(|m| m.role == e.role && m.account == e.account);
        match (e.change, at) {
            (RoleChange::Granted, None) => {
                n_granted += 1;
                members.push(Held {
                    role: e.role,
                    account: e.account,
                    block,
                    granted_by: e.sender,
                });
            }
            (RoleChange::Revoked, Some(i)) => {
                n_revoked += 1;
                members.remove(i);
            }
            (RoleChange::Granted, Some(_)) => {
                return Err(format!(
                    "a grant at block {block} of a role already held: a revoke before it is missing"
                ))
            }
            (RoleChange::Revoked, None) => {
                return Err(format!(
                    "a revoke at block {block} of a role not held: a grant before it is missing"
                ))
            }
        }
    }
    Ok(History {
        members,
        granted: n_granted,
        revoked: n_revoked,
    })
}

/// One expected `(role, account)` pair on one chain.
struct Want {
    role: String,
    id: [u8; 32],
    ident: String,
    /// Lowercase; `None` when the grantee did not resolve.
    address: Option<String>,
}

fn role_label(re: &RoleExpect, id: &[u8; 32]) -> String {
    re.roles
        .iter()
        .find(|name| role_word(name) == *id)
        .cloned()
        .unwrap_or_else(|| format!("0x{}", hex::encode(id)))
}

fn account_label(re: &RoleExpect, safe: Option<&str>, address: &str) -> Option<String> {
    if safe.is_some_and(|s| same(s, address)) {
        return Some("safe".to_string());
    }
    re.accounts
        .iter()
        .find(|a| a.address.as_deref().is_some_and(|x| same(x, address)))
        .map(|a| a.ident.clone())
}

fn held_json(re: &RoleExpect, safe: Option<&str>, h: &Held) -> Value {
    json!({
        "role": role_label(re, &h.role),
        "roleId": format!("0x{}", hex::encode(h.role)),
        "account": account_label(re, safe, &h.account),
        "address": h.account,
        "block": h.block,
        "grantedBy": h.granted_by,
        "grantedByAccount": account_label(re, safe, &h.granted_by),
    })
}

fn want_json(w: &Want) -> Value {
    json!({
        "role": w.role,
        "roleId": format!("0x{}", hex::encode(w.id)),
        "account": w.ident,
        "address": w.address,
    })
}

const READ_FAILED: &str = "a log read failed or may be incomplete";

/// The membership check of one contract on one chain.
fn membership(
    re: &RoleExpect,
    on: &str,
    contract: Option<&str>,
    pin: &ChainPin,
    expected: Option<&[(String, Grantee)]>,
    r: &dyn LogReads,
) -> Value {
    let safe = pin.safe.as_deref();
    let window = window(&pin.network, on);
    let wants: Option<Vec<Want>> = expected.map(|rows| {
        let mut out: Vec<Want> = Vec::new();
        for (role, who) in rows {
            let (ident, address) = match who {
                Grantee::Safe => ("safe".to_string(), safe.map(str::to_lowercase)),
                Grantee::Named(a) => (a.ident.clone(), a.address.as_deref().map(str::to_lowercase)),
            };
            let id = role_word(role);
            let dup = address.is_some() && out.iter().any(|o| o.id == id && o.address == address);
            if !dup {
                out.push(Want {
                    role: role.clone(),
                    id,
                    ident,
                    address,
                });
            }
        }
        out
    });
    let range: Result<(u64, u64), String> = match window {
        Some(w) => Ok(w),
        None => r
            .head()
            .map(|head| (0, head))
            .ok_or_else(|| "the log source's latest block was not read".to_string()),
    };
    let history = match (contract, &range) {
        (None, _) => Err("no address is pinned for this contract".to_string()),
        (_, Err(e)) => Err(e.clone()),
        (Some(c), Ok((from, to))) => {
            match (
                r.logs(c, ROLE_GRANTED_TOPIC, *from, *to),
                r.logs(c, ROLE_REVOKED_TOPIC, *from, *to),
            ) {
                (Some(g), Some(v)) => rebuild(c, *from, *to, &g, &v),
                _ => Err(READ_FAILED.to_string()),
            }
        }
    };
    let resolved: Result<&[Want], &str> = match &wants {
        None => Err("the expected set did not parse out of the deploy repo"),
        Some(w) if w.iter().any(|x| x.address.is_none()) => {
            Err("an expected grantee's address did not resolve")
        }
        Some(w) => Ok(w),
    };

    let (status, reason, extra, missing) = match (&history, resolved) {
        (Err(e), _) => ("unknown", Some(e.clone()), Value::Null, Value::Null),
        (Ok(_), Err(why)) => ("unknown", Some(why.to_string()), Value::Null, Value::Null),
        (Ok(h), Ok(w)) => {
            let expects = |m: &Held| {
                w.iter()
                    .any(|x| x.id == m.role && x.address.as_deref() == Some(m.account.as_str()))
            };
            let extra: Vec<Value> = h
                .members
                .iter()
                .filter(|m| !expects(m))
                .map(|m| held_json(re, safe, m))
                .collect();
            let missing: Vec<Value> = w
                .iter()
                .filter(|x| {
                    !h.members
                        .iter()
                        .any(|m| m.role == x.id && x.address.as_deref() == Some(m.account.as_str()))
                })
                .map(want_json)
                .collect();
            let status = if window.is_some() {
                "partial"
            } else if extra.is_empty() && missing.is_empty() {
                "pass"
            } else {
                "fail"
            };
            (status, None, json!(extra), json!(missing))
        }
    };
    let (from, to) = match range {
        Ok((f, t)) => (Some(f), Some(t)),
        Err(_) => (None, None),
    };
    json!({
        "check": "membership",
        "on": on,
        "address": contract,
        "coverage": if window.is_some() { "window" } else { "full" },
        "fromBlock": from,
        "toBlock": to,
        "match": "set",
        "expected": wants.as_ref().map(|w| w.iter().map(want_json).collect::<Vec<_>>()),
        "actual": history.as_ref().ok().map(|h| h.members.iter().map(|m| held_json(re, safe, m)).collect::<Vec<_>>()),
        "extra": extra,
        "missing": missing,
        "granted": history.as_ref().ok().map(|h| h.granted),
        "revoked": history.as_ref().ok().map(|h| h.revoked),
        "reason": reason,
        "status": status,
    })
}

/// `fail` on any failure; else `unknown` on any unknown; else `partial` on any
/// windowed read; `pass` only when every check passed on a whole history.
fn rollup(passed: usize, failed: usize, unknown: usize, partial: usize) -> &'static str {
    if failed > 0 {
        "fail"
    } else if unknown > 0 {
        "unknown"
    } else if partial > 0 {
        "partial"
    } else if passed > 0 {
        "pass"
    } else {
        "unknown"
    }
}

fn tallies(statuses: &[&str]) -> (usize, usize, usize, usize) {
    let count = |s: &str| statuses.iter().filter(|x| **x == s).count();
    (
        count("pass"),
        count("fail"),
        count("unknown"),
        count("partial"),
    )
}

/// One chain's membership document: the clone and the orchestrator instance.
pub fn chain_doc(re: &RoleExpect, pin: &ChainPin, r: &dyn LogReads) -> Value {
    let authoriser = membership(
        re,
        "authoriser",
        pin.authoriser.as_deref(),
        pin,
        re.clone.as_deref(),
        r,
    );
    let orchestrator = membership(
        re,
        "orchestrator",
        re.instance.as_deref(),
        pin,
        Some(&re.orchestrator),
        r,
    );
    let statuses: Vec<&str> = [&authoriser, &orchestrator]
        .iter()
        .map(|c| c["status"].as_str().unwrap_or("unknown"))
        .collect();
    let (passed, failed, unknown, partial) = tallies(&statuses);
    json!({
        "network": pin.network,
        "rpcHost": pin.rpc_host,
        "logSource": r.source(),
        "safe": pin.safe,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "partial": partial,
        "total": statuses.len(),
        "state": rollup(passed, failed, unknown, partial),
        "authoriser": authoriser,
        "orchestrator": orchestrator,
    })
}

/// Assemble the `deploymentRoleMembership` document. `None` when there are no
/// chains, so the page shows nothing rather than an empty table.
pub fn build_roles(org: &str, repo: &str, chains: Vec<Value>) -> Option<Value> {
    if chains.is_empty() {
        return None;
    }
    let sum = |k: &str| {
        chains
            .iter()
            .map(|c| c[k].as_u64().unwrap_or(0) as usize)
            .sum::<usize>()
    };
    let (passed, failed, unknown, partial) =
        (sum("passed"), sum("failed"), sum("unknown"), sum("partial"));
    let windows: Vec<Value> = CEREMONY_WINDOWS
        .iter()
        .map(|(network, on, from, to)| {
            json!({"network": network, "on": on, "fromBlock": from, "toBlock": to})
        })
        .collect();
    Some(json!({
        "org": org,
        "repo": repo,
        "source": AUTH_LIB,
        "function": "expectedGrants(address)",
        "windows": windows,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "partial": partial,
        "total": passed + failed + unknown + partial,
        "state": rollup(passed, failed, unknown, partial),
        "chains": chains,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploystate::{parse_expectations, DEPLOY_KEY};
    use crate::owners::parse_chain_pins;
    use std::cell::RefCell;

    // Production addresses as st0x.deploy pins them (3c6db51).
    const BASE_SAFE: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const SAFE: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const BASE_CLONE: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
    const CLONE: &str = "0x66566cc91dEAf818859bD4b09B7903ac48998157";
    const INSTANCE: &str = "0x3A7387a484d87Aa8bBA45E98AAB401Ce4FBF03E2";
    const OPERATOR: &str = "0x3d0CD66EFA66c05d86c3d4316B03eAE87ab9E8aE";
    const RETIRED: &str = "0x1c66D6708914C40239D54919320b4C48cAE3D1A9";
    const EOA: &str = "0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b";
    const FACTORY: &str = "0x444acc29d63fa643e8adcc35fd9aa6de111dcb39";
    const STRANGER: &str = "0x00000000000000000000000000000000000000aa";

    fn safe_lib() -> String {
        format!(
            "
    address internal constant STOX_TOKEN_OWNER_SAFE = {BASE_SAFE};
    address internal constant STOX_TOKEN_OWNER_SAFE_ETHEREUM = {SAFE};
    address internal constant STOX_TOKEN_OWNER_SAFE_HYPEREVM = {SAFE};
"
        )
    }

    // The real map's shape: the one-arg entry delegating to the two-arg body.
    fn auth_lib(extra_row: &str) -> String {
        format!(
            r#"
    address internal constant GRANTEE_SERVICE_1C66 = {RETIRED};
    address internal constant GRANTEE_SERVICE_3D0C = {OPERATOR};
    address internal constant GRANTEE_ORCHESTRATOR = LibProdDeployV4.ST0X_ORCHESTRATOR_INSTANCE;
    function expectedGrants(address tokenOwnerSafe) internal pure returns (RoleGrant[] memory grants) {{
        grants = expectedGrants(tokenOwnerSafe, tokenOwnerSafe);
    }}
    function expectedGrants(address tokenOwnerSafe, address adminHolder)
        internal
        pure
        returns (RoleGrant[] memory grants)
    {{
        grants = new RoleGrant[](6);
        grants[0] = RoleGrant(keccak256("DEPOSIT_ADMIN"), adminHolder);
        grants[1] = RoleGrant(keccak256("WITHDRAW_ADMIN"), adminHolder);
        grants[2] = RoleGrant(keccak256("DEPOSIT"), tokenOwnerSafe);
        grants[3] = RoleGrant(keccak256("DEPOSIT"), GRANTEE_SERVICE_3D0C);
        grants[4] = RoleGrant(keccak256("DEPOSIT"), GRANTEE_ORCHESTRATOR);
        grants[5] = RoleGrant(keccak256("WITHDRAW"), GRANTEE_ORCHESTRATOR);
        {extra_row}
    }}
"#
        )
    }

    fn v4_lib() -> String {
        format!(
            "
library LibProdDeployV4 {{
    address constant BEACON_INITIAL_OWNER = address({EOA});
    address constant STOX_PROD_AUTHORISER_V4_CLONE = address({BASE_CLONE});
    address constant STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM = address({CLONE});
    address constant STOX_PROD_AUTHORISER_V4_CLONE_HYPEREVM = address({CLONE});
    address constant ST0X_ORCHESTRATOR_INSTANCE = address({INSTANCE});
}}
"
        )
    }

    const ORCHESTRATOR_SRC: &str = r#"
    bytes32 public constant MINT_ROLE = keccak256("MINT");
    bytes32 public constant BURN_ROLE = keccak256("BURN");
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY");
"#;

    fn expect_with(extra_row: &str) -> RoleExpect {
        let (safe, auth, v4) = (safe_lib(), auth_lib(extra_row), v4_lib());
        let src = OwnerSources {
            safe_lib: &safe,
            auth_lib: &auth,
            v4_lib: &v4,
            overrides: "",
        };
        parse_expect(
            &src,
            &parse_expectations(&src, ORCHESTRATOR_SRC, Vec::new()),
        )
    }

    fn expect() -> RoleExpect {
        expect_with("")
    }

    fn pin(network: &str) -> ChainPin {
        let mut p = parse_chain_pins(&v4_lib(), &safe_lib())
            .into_iter()
            .find(|p| p.network == network)
            .unwrap();
        p.rpc_host = Some(format!("{network}.example"));
        p
    }

    fn word(address: &str) -> [u8; 32] {
        let mut w = [0u8; 32];
        hex::decode_to_slice(
            format!("{:0>64}", address.trim_start_matches("0x").to_lowercase()),
            &mut w,
        )
        .unwrap();
        w
    }

    fn log(
        contract: &str,
        block: u64,
        index: u64,
        change: RoleChange,
        role: &str,
        account: &str,
    ) -> Log {
        let topic = match change {
            RoleChange::Granted => ROLE_GRANTED_TOPIC,
            RoleChange::Revoked => ROLE_REVOKED_TOPIC,
        };
        Log {
            address: contract.to_lowercase(),
            topics: vec![topic, role_word(role), word(account), word(FACTORY)],
            data: Vec::new(),
            block_number: block,
            log_index: index,
            removed: false,
        }
    }

    /// A chain's role history held in memory.
    #[derive(Default)]
    struct Fake {
        head: Option<u64>,
        logs: Vec<Log>,
        fail: bool,
        asked: RefCell<Vec<(String, u64, u64)>>,
    }

    impl Fake {
        fn at(
            &mut self,
            contract: &str,
            block: u64,
            change: RoleChange,
            role: &str,
            account: &str,
        ) {
            let index = self.logs.len() as u64;
            self.logs
                .push(log(contract, block, index, change, role, account));
        }
        fn grant(&mut self, contract: &str, block: u64, role: &str, account: &str) {
            self.at(contract, block, RoleChange::Granted, role, account);
        }
        fn revoke(&mut self, contract: &str, block: u64, role: &str, account: &str) {
            self.at(contract, block, RoleChange::Revoked, role, account);
        }
    }

    impl LogReads for Fake {
        fn source(&self) -> Option<String> {
            Some("logs.example".to_string())
        }
        fn head(&self) -> Option<u64> {
            self.head
        }
        fn logs(&self, address: &str, topic0: [u8; 32], from: u64, to: u64) -> Option<Vec<Log>> {
            self.asked
                .borrow_mut()
                .push((address.to_lowercase(), from, to));
            if self.fail {
                return None;
            }
            Some(
                self.logs
                    .iter()
                    .filter(|l| {
                        same(&l.address, address)
                            && l.topics[0] == topic0
                            && (from..=to).contains(&l.block_number)
                    })
                    .cloned()
                    .collect(),
            )
        }
    }

    /// The ceremonies as they ran: the deploy key takes the admin roles, grants
    /// the map (and the since-retired signer), hands the admin roles to the
    /// Safe and renounces its own; the orchestrator's deployer does the same
    /// with `DEFAULT_ADMIN`. `start` is each contract's creation block.
    fn healthy(clone: &str, safe: &str, start: (u64, u64)) -> Fake {
        let (c, o) = start;
        let mut f = Fake {
            head: Some(o + 5_000),
            ..Fake::default()
        };
        for admin in ["DEPOSIT_ADMIN", "WITHDRAW_ADMIN"] {
            f.grant(clone, c, admin, DEPLOY_KEY);
        }
        f.grant(clone, c + 1, "DEPOSIT", safe);
        f.grant(clone, c + 1, "DEPOSIT", OPERATOR);
        f.grant(clone, c + 1, "DEPOSIT", RETIRED);
        for admin in ["DEPOSIT_ADMIN", "WITHDRAW_ADMIN"] {
            f.grant(clone, c + 2, admin, safe);
            f.revoke(clone, c + 3, admin, DEPLOY_KEY);
        }
        f.revoke(clone, c + 500, "DEPOSIT", RETIRED);
        f.grant(INSTANCE, o, DEFAULT_ADMIN, DEPLOY_KEY);
        f.grant(INSTANCE, o + 1, "MINT", OPERATOR);
        f.grant(INSTANCE, o + 1, "BURN", OPERATOR);
        f.grant(INSTANCE, o + 1, DEFAULT_ADMIN, safe);
        f.revoke(INSTANCE, o + 2, DEFAULT_ADMIN, DEPLOY_KEY);
        f.grant(clone, o + 3, "DEPOSIT", INSTANCE);
        f.grant(clone, o + 3, "WITHDRAW", INSTANCE);
        f
    }

    fn pairs(v: &Value) -> Vec<(String, Option<String>)> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["role"].as_str().unwrap().to_string(),
                    e["account"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    fn p(role: &str, account: &str) -> (String, Option<String>) {
        (role.to_string(), Some(account.to_string()))
    }

    #[test]
    fn expected_sets_come_from_the_deploy_repo() {
        let re = expect();
        let clone = re.clone.as_ref().unwrap();
        assert_eq!(clone.len(), 6);
        assert_eq!(clone[0], ("DEPOSIT_ADMIN".to_string(), Grantee::Safe));
        assert_eq!(clone[2], ("DEPOSIT".to_string(), Grantee::Safe));
        assert_eq!(
            clone[3].1,
            Grantee::Named(Account {
                ident: "GRANTEE_SERVICE_3D0C".into(),
                address: Some(OPERATOR.into())
            })
        );
        assert_eq!(
            clone[4].1,
            Grantee::Named(Account {
                ident: "GRANTEE_ORCHESTRATOR".into(),
                address: Some(INSTANCE.into())
            }),
            "an alias of the generated lib's pin resolves"
        );
        let operator = Grantee::Named(Account {
            ident: "GRANTEE_SERVICE_3D0C".into(),
            address: Some(OPERATOR.into()),
        });
        assert_eq!(
            re.orchestrator,
            vec![
                (DEFAULT_ADMIN.to_string(), Grantee::Safe),
                ("MINT".to_string(), operator.clone()),
                ("BURN".to_string(), operator),
            ]
        );
        assert_eq!(re.instance.as_deref(), Some(INSTANCE));
        for name in [
            "DEFAULT_ADMIN",
            "DEPOSIT_ADMIN",
            "DEPOSIT",
            "WITHDRAW",
            "EMERGENCY",
        ] {
            assert!(re.roles.iter().any(|r| r == name), "{name}");
        }
    }

    #[test]
    fn a_whole_history_that_matches_the_map_passes() {
        let f = healthy(CLONE, SAFE, (1_000, 2_000));
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        assert_eq!(doc["state"], "pass");
        assert_eq!(doc["logSource"], "logs.example");
        let auth = &doc["authoriser"];
        assert_eq!(auth["status"], "pass", "{auth}");
        assert_eq!(auth["coverage"], "full");
        assert_eq!(
            (auth["fromBlock"].as_u64(), auth["toBlock"].as_u64()),
            (Some(0), Some(7_000))
        );
        assert_eq!(
            (auth["granted"].as_u64(), auth["revoked"].as_u64()),
            (Some(9), Some(3))
        );
        assert_eq!(
            pairs(&auth["actual"]),
            vec![
                p("DEPOSIT", "safe"),
                p("DEPOSIT", "GRANTEE_SERVICE_3D0C"),
                p("DEPOSIT_ADMIN", "safe"),
                p("WITHDRAW_ADMIN", "safe"),
                p("DEPOSIT", "GRANTEE_ORCHESTRATOR"),
                p("WITHDRAW", "GRANTEE_ORCHESTRATOR"),
            ]
        );
        assert_eq!(auth["actual"][0]["block"], 1_001);
        assert_eq!(auth["actual"][0]["grantedBy"], FACTORY);
        assert_eq!(auth["extra"], json!([]));
        assert_eq!(auth["missing"], json!([]));
        let orch = &doc["orchestrator"];
        assert_eq!(orch["status"], "pass", "{orch}");
        assert_eq!(orch["address"], INSTANCE);
        assert_eq!(
            pairs(&orch["expected"]),
            vec![
                p("DEFAULT_ADMIN", "safe"),
                p("MINT", "GRANTEE_SERVICE_3D0C"),
                p("BURN", "GRANTEE_SERVICE_3D0C"),
            ]
        );
        assert!(f
            .asked
            .borrow()
            .iter()
            .all(|(_, from, to)| (*from, *to) == (0, 7_000)));
    }

    /// One way to break a healthy history, applied to its fake.
    type Breach = Box<dyn Fn(&mut Fake)>;

    /// `hasRole` on the map's rows cannot see a role nobody listed. The
    /// history can, and a stray grant must fail the chain.
    #[test]
    fn a_grant_the_map_does_not_expect_fails() {
        let cases: Vec<(&str, Breach, (&str, &str))> = vec![
            (
                "orchestrator",
                Box::new(|f| f.grant(INSTANCE, 9_000, "EMERGENCY", EOA)),
                ("EMERGENCY", "BEACON_INITIAL_OWNER"),
            ),
            (
                "authoriser",
                Box::new(|f| f.grant(CLONE, 9_000, "DEPOSIT_ADMIN", DEPLOY_KEY)),
                ("DEPOSIT_ADMIN", "DEPLOY_KEY"),
            ),
            (
                "authoriser",
                Box::new(|f| f.grant(CLONE, 9_000, "WITHDRAW", RETIRED)),
                ("WITHDRAW", "GRANTEE_SERVICE_1C66"),
            ),
            (
                "authoriser",
                Box::new(|f| f.grant(CLONE, 9_000, "WITHDRAW", SAFE)),
                ("WITHDRAW", "safe"),
            ),
        ];
        for (on, breach, (role, who)) in cases {
            let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
            f.head = Some(10_000);
            breach(&mut f);
            let doc = chain_doc(&expect(), &pin("ethereum"), &f);
            assert_eq!(doc[on]["status"], "fail", "{role} {who}: {}", doc[on]);
            assert_eq!(pairs(&doc[on]["extra"]), vec![p(role, who)]);
            assert_eq!(doc[on]["missing"], json!([]));
            assert_eq!(doc["state"], "fail");
        }

        let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
        f.head = Some(10_000);
        f.grant(CLONE, 9_000, "SOMETHING_NEW", STRANGER);
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        let extra = &doc["authoriser"]["extra"][0];
        assert_eq!(doc["authoriser"]["status"], "fail");
        assert_eq!(
            extra["role"],
            format!("0x{}", hex::encode(role_word("SOMETHING_NEW"))),
            "a role no source names shows as its id"
        );
        assert_eq!(extra["account"], Value::Null);
        assert_eq!(extra["address"], STRANGER);
    }

    #[test]
    fn an_expected_grant_that_is_gone_fails() {
        let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
        f.revoke(INSTANCE, 3_000, "BURN", OPERATOR);
        f.revoke(CLONE, 3_000, "WITHDRAW_ADMIN", SAFE);
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        assert_eq!(doc["orchestrator"]["status"], "fail");
        assert_eq!(
            pairs(&doc["orchestrator"]["missing"]),
            vec![p("BURN", "GRANTEE_SERVICE_3D0C")]
        );
        assert_eq!(
            pairs(&doc["authoriser"]["missing"]),
            vec![p("WITHDRAW_ADMIN", "safe")]
        );
        assert_eq!(doc["authoriser"]["extra"], json!([]));
        assert_eq!(doc["failed"], 2);
    }

    /// A history that was not read, or not read whole, is `unknown`, even with
    /// a stray grant sitting in the part that was.
    #[test]
    fn an_unread_history_is_unknown_never_fail() {
        let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
        f.grant(INSTANCE, 3_000, "EMERGENCY", EOA);
        f.fail = true;
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        for on in ["authoriser", "orchestrator"] {
            assert_eq!(doc[on]["status"], "unknown");
            assert_eq!(doc[on]["actual"], Value::Null, "no partial list");
            assert_eq!(doc[on]["extra"], Value::Null);
            assert_eq!(doc[on]["reason"], READ_FAILED);
        }
        assert_eq!(doc["state"], "unknown");
        assert_eq!(doc["unknown"], 2);

        let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
        f.head = None;
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        assert_eq!(doc["authoriser"]["status"], "unknown");
        assert_eq!(doc["authoriser"]["toBlock"], Value::Null);
        assert!(f.asked.borrow().is_empty(), "no range, so nothing is asked");

        let doc = chain_doc(&expect(), &pin("ethereum"), &NoLogs);
        assert_eq!(doc["state"], "unknown");
        assert_eq!(doc["logSource"], Value::Null);
    }

    /// A dropped event shows as a history that contradicts itself.
    #[test]
    fn a_history_missing_an_event_is_unknown() {
        let mut f = healthy(CLONE, SAFE, (1_000, 2_000));
        f.logs
            .retain(|l| !(l.topics[0] == ROLE_GRANTED_TOPIC && l.topics[2] == word(RETIRED)));
        let doc = chain_doc(&expect(), &pin("ethereum"), &f);
        let auth = &doc["authoriser"];
        assert_eq!(auth["status"], "unknown");
        assert!(
            auth["reason"]
                .as_str()
                .unwrap()
                .contains("a grant before it is missing"),
            "{auth}"
        );
        assert_eq!(auth["actual"], Value::Null);
        assert_eq!(doc["orchestrator"]["status"], "pass");
    }

    #[test]
    fn an_expected_grantee_that_does_not_resolve_is_unknown() {
        let re = expect_with(r#"grants[6] = RoleGrant(keccak256("CERTIFY"), GRANTEE_NOWHERE);"#);
        let f = healthy(CLONE, SAFE, (1_000, 2_000));
        let doc = chain_doc(&re, &pin("ethereum"), &f);
        assert_eq!(doc["authoriser"]["status"], "unknown");
        assert_eq!(
            doc["authoriser"]["reason"],
            "an expected grantee's address did not resolve"
        );
        assert!(
            doc["authoriser"]["actual"].is_array(),
            "the history still shows"
        );

        let mut re = expect();
        re.clone = None;
        let doc = chain_doc(&re, &pin("ethereum"), &f);
        assert_eq!(doc["authoriser"]["status"], "unknown");
        assert_eq!(doc["authoriser"]["expected"], Value::Null);
    }

    /// A ceremony window is the membership as of its last block. Whatever it
    /// shows, it is `partial`: a window that matches the map says nothing about
    /// after it, and one that differs may have been put right after it.
    #[test]
    fn a_ceremony_window_is_partial_never_pass_or_fail() {
        let (c, o) = (41_325_390, 44_389_509);
        let f = healthy(CLONE, SAFE, (c, o));
        let doc = chain_doc(&expect(), &pin("hyperevm"), &f);
        let auth = &doc["authoriser"];
        assert_eq!(auth["coverage"], "window");
        assert_eq!(
            (auth["fromBlock"].as_u64(), auth["toBlock"].as_u64()),
            (Some(c), Some(41_327_389))
        );
        assert_eq!(auth["status"], "partial");
        // The instance's clone roles were granted after the window.
        assert_eq!(
            pairs(&auth["missing"]),
            vec![
                p("DEPOSIT", "GRANTEE_ORCHESTRATOR"),
                p("WITHDRAW", "GRANTEE_ORCHESTRATOR")
            ]
        );
        // The retired signer's revoke fell inside it.
        assert_eq!(auth["extra"], json!([]));
        assert_eq!(doc["orchestrator"]["status"], "partial");
        assert_eq!(doc["orchestrator"]["toBlock"], 44_391_508);
        assert!(f
            .asked
            .borrow()
            .contains(&(CLONE.to_lowercase(), c, 41_327_389)));
        assert!(f
            .asked
            .borrow()
            .contains(&(INSTANCE.to_lowercase(), o, 44_391_508)));
        assert_eq!(doc["state"], "partial");
        assert_eq!(doc["partial"], 2);

        let mut f = healthy(CLONE, SAFE, (c, o));
        f.grant(INSTANCE, o + 10, "EMERGENCY", EOA);
        let doc = chain_doc(&expect(), &pin("hyperevm"), &f);
        assert_eq!(doc["orchestrator"]["status"], "partial");
        assert_eq!(
            pairs(&doc["orchestrator"]["extra"]),
            vec![p("EMERGENCY", "BEACON_INITIAL_OWNER")]
        );

        let mut f = healthy(CLONE, SAFE, (c, o));
        f.fail = true;
        let doc = chain_doc(&expect(), &pin("hyperevm"), &f);
        assert_eq!(doc["authoriser"]["status"], "unknown");
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn the_document_rolls_up_every_chain() {
        let whole = chain_doc(
            &expect(),
            &pin("ethereum"),
            &healthy(CLONE, SAFE, (1_000, 2_000)),
        );
        let window = chain_doc(
            &expect(),
            &pin("hyperevm"),
            &healthy(CLONE, SAFE, (41_325_390, 44_389_509)),
        );
        let doc = build_roles("S01-Issuer", "st0x.deploy", vec![whole.clone(), window]).unwrap();
        assert_eq!(doc["state"], "partial");
        assert_eq!(
            (
                doc["passed"].as_u64(),
                doc["partial"].as_u64(),
                doc["total"].as_u64()
            ),
            (Some(2), Some(2), Some(4))
        );
        assert_eq!(doc["source"], AUTH_LIB);
        assert_eq!(
            doc["windows"].as_array().unwrap().len(),
            CEREMONY_WINDOWS.len()
        );
        assert_eq!(build_roles("o", "r", vec![whole]).unwrap()["state"], "pass");
        assert_eq!(build_roles("o", "r", Vec::new()), None);
        assert_eq!(rollup(1, 0, 1, 1), "unknown");
        assert_eq!(rollup(1, 1, 1, 1), "fail");
        assert_eq!(rollup(0, 0, 0, 0), "unknown");
    }

    #[test]
    fn rebuild_replays_both_lists_in_chain_order() {
        let granted = vec![
            log(CLONE, 10, 0, RoleChange::Granted, "DEPOSIT", SAFE),
            log(CLONE, 12, 0, RoleChange::Granted, "DEPOSIT", SAFE),
        ];
        let revoked = vec![log(CLONE, 11, 4, RoleChange::Revoked, "DEPOSIT", SAFE)];
        let h = rebuild(CLONE, 0, 20, &granted, &revoked).unwrap();
        assert_eq!((h.granted, h.revoked), (2, 1));
        assert_eq!(h.members.len(), 1);
        assert_eq!(h.members[0].block, 12);

        let mut retracted = log(CLONE, 13, 0, RoleChange::Granted, "WITHDRAW", SAFE);
        retracted.removed = true;
        let h = rebuild(CLONE, 0, 20, &[retracted], &[]).unwrap();
        assert!(h.members.is_empty(), "a reorged-out log is not history");
    }

    #[test]
    fn rebuild_refuses_logs_that_cannot_be_the_asked_history() {
        let ok = log(CLONE, 10, 0, RoleChange::Granted, "DEPOSIT", SAFE);
        let err =
            |granted: &[Log], revoked: &[Log]| rebuild(CLONE, 5, 20, granted, revoked).unwrap_err();

        let foreign = log(BASE_CLONE, 10, 1, RoleChange::Granted, "DEPOSIT", SAFE);
        assert!(err(&[ok.clone(), foreign], &[]).contains("a log from"));
        assert!(
            err(&[], std::slice::from_ref(&ok)).contains("another event"),
            "a grant in the revoke list"
        );
        let early = log(CLONE, 4, 0, RoleChange::Granted, "DEPOSIT", SAFE);
        assert!(err(&[early], &[]).contains("outside the blocks asked"));
        let late = log(CLONE, 21, 0, RoleChange::Granted, "DEPOSIT", SAFE);
        assert!(err(&[late], &[]).contains("outside the blocks asked"));
        let mut shape = ok.clone();
        shape.data = vec![1];
        assert!(err(&[shape], &[]).contains("wrong shape"));
        let twin = log(CLONE, 10, 0, RoleChange::Granted, "WITHDRAW", SAFE);
        assert!(err(&[ok.clone(), twin], &[]).contains("two logs at block 10 index 0"));
        let again = log(CLONE, 11, 0, RoleChange::Granted, "DEPOSIT", SAFE);
        assert!(err(&[ok, again], &[]).contains("a revoke before it is missing"));
        let unheld = log(CLONE, 11, 0, RoleChange::Revoked, "DEPOSIT", SAFE);
        assert!(err(&[], &[unheld]).contains("a grant before it is missing"));
    }

    #[test]
    fn spans_cut_an_inclusive_range() {
        let s = spans(41_325_390, 41_327_389, 100).unwrap();
        assert_eq!(s.len(), 20);
        assert_eq!(s[0], (41_325_390, 41_325_489));
        assert_eq!(s[19], (41_327_290, 41_327_389));
        assert!(
            s.windows(2).all(|w| w[1].0 == w[0].1 + 1),
            "no gap, no overlap"
        );
        assert_eq!(spans(7, 7, 25), Some(vec![(7, 7)]));
        assert_eq!(spans(0, 1_000, 300).unwrap().last(), Some(&(900, 1_000)));
        assert_eq!(spans(0, 68_000_000, u64::MAX), Some(vec![(0, 68_000_000)]));
        assert_eq!(spans(5, 4, 1), None);
        assert_eq!(spans(0, 1, 0), None);
        assert_eq!(
            spans(0, MAX_LOG_REQUESTS * 25 - 1, 25).map(|s| s.len() as u64),
            Some(MAX_LOG_REQUESTS)
        );
        assert_eq!(spans(0, MAX_LOG_REQUESTS * 25, 25), None);
    }

    #[test]
    fn the_windows_are_the_ones_the_issue_set() {
        assert_eq!(
            window("hyperevm", "authoriser"),
            Some((41_325_390, 41_327_389))
        );
        assert_eq!(
            window("hyperevm", "orchestrator"),
            Some((44_389_509, 44_391_508))
        );
        assert_eq!(
            window("bsc", "authoriser"),
            Some((121_144_118, 121_146_117))
        );
        assert_eq!(
            window("bsc", "orchestrator"),
            Some((121_162_568, 121_164_567))
        );
        for network in ["base", "ethereum", "robinhood"] {
            assert_eq!(window(network, "authoriser"), None);
            assert_eq!(window(network, "orchestrator"), None);
        }
    }
}
