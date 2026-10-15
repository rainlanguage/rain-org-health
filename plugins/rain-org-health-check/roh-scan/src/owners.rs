//! The Deployments page's owners and role-grant views, assembled from the
//! deploy repo's `privileged-addresses.json` and `ideal-roles.json`. Pure
//! assembly lives here and is unit-tested; the fetches are in main.rs.

use crate::idealroles::IdealChain;
use crate::privileged::PrivilegedAddresses;
use serde_json::json;

/// On-chain readback of a Safe, for the declared-vs-actual provenance view. Each
/// field is `None` when its RPC call failed, so the dashboard can show "on-chain
/// unavailable" without dropping the declared constants.
pub struct OnChainSafe {
    pub network: String,
    pub safe: String,
    pub rpc_host: String,
    pub owners: Option<Vec<String>>,
    pub threshold: Option<u64>,
}

fn entry(
    role: &str,
    address: Option<String>,
    network: &str,
    status: &str,
    note: &str,
) -> serde_json::Value {
    json!({
        "role": role,
        // Option<String> serialises to null when the constant did not resolve, so
        // drift (a renamed/removed constant) surfaces as a gap rather than silently
        // dropping the row.
        "address": address,
        "network": network,
        "status": status,
        "note": note,
    })
}

/// Whether an authoriser pin is the one production vaults actually delegate to.
///
/// The two Base clones swap roles over the course of the V4 migration, so
/// hardcoding "active" and "pending" freezes the page at whatever was true the
/// day it was written — and it silently disagrees with the token rows on the
/// same page, which read `authorizer()` live. `live` is that live value.
///
/// `not_live` is what this pin means when it is NOT the live one: the V3-era
/// clone has been superseded, the V4 clone has not taken over yet. Without a
/// live reading the honest answer is `unknown`, never a guess.
pub fn authoriser_status(
    pin: Option<&String>,
    live: Option<&str>,
    not_live: &'static str,
) -> &'static str {
    match (pin, live) {
        (None, _) | (_, None) => "unknown",
        (Some(p), Some(l)) if p.eq_ignore_ascii_case(l) => "active",
        _ => not_live,
    }
}

/// A role that ADMINS another role rather than performing an action.
///
/// The hierarchy is the source's own: every action role is admin'd by its
/// `<ROLE>_ADMIN` (`DEFAULT_ADMIN_ROLE` is deliberately held by nobody). So the
/// admin/action split is read off the role NAME rather than listed here — a new
/// action role lands on the action side by itself.
pub fn is_admin_role(role: &str) -> bool {
    role.ends_with("_ADMIN")
}

/// The chain a bare (unsuffixed) per-chain pin belongs to: the deploy repo
/// writes the home chain's pin as `NAME` and every other chain's as
/// `NAME_<CHAIN>`.
pub const REFERENCE_NETWORK: &str = "base";

/// A chain the deploy repo pins production state for: its authoriser (the
/// contract the grants live on) and its token-owner Safe (the per-chain grantee
/// the map is parameterised by).
///
/// `rpc_host` is `None` for a chain the scanner has no endpoint set for. A chain
/// pinned in the source but not yet reachable from here still appears — with its
/// pins and an unread status — because a chain that vanishes reads as "we do not
/// deploy there", which is the opposite of "its rollout has not reached us yet".
#[derive(Clone)]
pub struct ChainPin {
    pub network: String,
    pub authoriser: Option<String>,
    pub safe: Option<String>,
    /// The chain's governance timelock — what the admin-holder slot resolves to
    /// once the migration has moved the `_ADMIN` roles onto it. Pinned per chain
    /// for the same reason the Safe is: the address differs by chain.
    ///
    /// Present whether or not the migration has executed there. Nothing here
    /// claims it holds anything; `hasRole` is what answers that, so the page
    /// reads `missing` on a chain the bundle has not reached and `granted` on
    /// one it has.
    pub admin_holder: Option<String>,
    pub rpc_host: Option<String>,
}

/// Asks one chain for every vault's `owner()`, answering one slot per vault in
/// the order given.
///
/// A whole chain at once rather than a vault at a time, because the caller
/// answers it with ONE JSON-RPC batch; see `build_vault_owners`.
pub type ReadOwners<'a> = &'a dyn Fn(&str, &[String]) -> Vec<Option<String>>;

/// Who owns a governed vault, as the chain answered.
///
/// The labels are relative to that chain's own pins, so `Timelock` on Ethereum
/// and `Timelock` on Base are different addresses and both read as moved.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VaultOwner {
    /// The chain's token-owner Safe — the pre-migration holder.
    Safe,
    /// The chain's governance timelock — the post-migration holder.
    Timelock,
    /// Neither pin. Not a fault by itself, but it is the one answer nobody
    /// predicted, so it is never folded into `Unknown`.
    Other,
    /// This chain pins no timelock, so there is nothing to recognise a moved
    /// vault by. Held apart from `Other` because they are different claims: a
    /// chain that publishes an authoriser but no timelock yet is mid-rollout,
    /// and calling that the same red as an owner
    /// nobody pinned would report the rollout as a fault — the distinction the
    /// grant rows already make, for the same reason.
    Unpinned,
    /// No answer: no code at the address on this chain, or the call failed.
    /// Never a stand-in for "not moved".
    Unknown,
}

impl VaultOwner {
    fn token(self) -> &'static str {
        match self {
            VaultOwner::Safe => "safe",
            VaultOwner::Timelock => "timelock",
            VaultOwner::Other => "other",
            VaultOwner::Unpinned => "unpinned",
            VaultOwner::Unknown => "unknown",
        }
    }
}

/// Ownership of every governed vault, per chain, labelled against that chain's
/// Safe and timelock.
///
/// The governance migration moves these by `transferOwnership`, one call per
/// vault, and nothing else on this page reads a vault's `owner()` — only
/// production beacons are owner-checked. So a migration that moved sixty-odd
/// vaults left no trace here: the page would show the pre-migration owner
/// indefinitely with nothing marking it stale.
///
/// `read_owners` takes a whole chain's vaults at once, not one at a time, and
/// is injected for the reason `build_grants` injects its probe: the labelling
/// is what can be got wrong, and it is worth testing without a network.
///
/// Per chain rather than per vault because the caller answers it with ONE
/// JSON-RPC batch. A vault is deployed on one chain, so asking every chain
/// about every vault is mostly asking for an answer that cannot exist — those
/// land as `Unknown` and are reported as unread, which is honest, but it is a
/// large number of calls to learn nothing and each one costs a subprocess.
/// @param vaults The governed vault addresses, lowercased.
/// @param chains The chains to ask, carrying their own Safe and timelock pins.
/// @param read_owners Asks one chain for every vault's `owner()`, answering in
/// the order given, one slot per vault.
/// @return The per-chain rows and tallies, or `None` with no vaults to report.
pub fn build_vault_owners(
    vaults_for: &dyn Fn(&str) -> Vec<String>,
    chains: &[ChainPin],
    read_owners: ReadOwners,
) -> Option<serde_json::Value> {
    if chains.is_empty() {
        return None;
    }
    let eq = |a: &Option<String>, b: &str| {
        a.as_deref()
            .map(|x| x.eq_ignore_ascii_case(b))
            .unwrap_or(false)
    };
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for chain in chains {
        let mut vault_rows: Vec<serde_json::Value> = Vec::new();
        let (mut safe, mut timelock, mut other, mut unknown, mut unpinned) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        // A short answer is not a partial truth: a probe that returned fewer
        // slots than it was asked about has not said which vaults it covered,
        // so the tail reads unread rather than borrowing another slot's owner.
        // This chain's OWN vaults. A vault is deployed on one chain, so a
        // shared list means four chains in five are asked about addresses with
        // no code on them — every answer `Unknown`, nothing reported, and the
        // one chain the migration actually touched never asked about at all.
        let vaults = vaults_for(&chain.network);
        let owners = read_owners(&chain.network, &vaults);
        for (i, vault) in vaults.iter().enumerate() {
            let owner = owners.get(i).cloned().flatten();
            let label = match &owner {
                None => VaultOwner::Unknown,
                Some(o) if eq(&chain.safe, o) => VaultOwner::Safe,
                Some(o) if eq(&chain.admin_holder, o) => VaultOwner::Timelock,
                // Nothing pinned to recognise a move by, so this is not an
                // unexpected owner — it is an unanswerable question.
                Some(_) if chain.admin_holder.is_none() => VaultOwner::Unpinned,
                Some(_) => VaultOwner::Other,
            };
            match label {
                VaultOwner::Safe => safe += 1,
                VaultOwner::Timelock => timelock += 1,
                VaultOwner::Other => other += 1,
                VaultOwner::Unpinned => unpinned += 1,
                VaultOwner::Unknown => unknown += 1,
            }
            vault_rows.push(json!({
                "vault": vault,
                "owner": owner,
                "label": label.token(),
            }));
        }
        rows.push(json!({
            "network": chain.network,
            "safe": chain.safe,
            "timelock": chain.admin_holder,
            "vaults": vault_rows,
            "counts": json!({
                "safe": safe,
                "timelock": timelock,
                "other": other,
                "unknown": unknown,
                "unpinned": unpinned,
            }),
        }));
    }
    Some(json!({ "chains": rows }))
}

/// What the chain said about one pinned `(role, grantee)` pair.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum GrantOnChain {
    Granted,
    NotGranted,
    /// The question could not be asked or answered — a failed call, or no
    /// endpoint for this chain. Never a stand-in for "not granted".
    Unknown,
}

impl GrantOnChain {
    fn token(self) -> &'static str {
        match self {
            GrantOnChain::Granted => "granted",
            GrantOnChain::NotGranted => "missing",
            GrantOnChain::Unknown => "unknown",
        }
    }
}

/// Roll a chain's per-grant results into one word.
///
/// `unprovisioned` and `partial` are rollout states, not faults: #280's model
/// has a chain's pins red until its provisioning bundle executes. "Granted on
/// Base, not yet on Ethereum" is a true reading of a rollout in flight, and the
/// page colours it as one.
fn chain_state(granted: usize, missing: usize, unknown: usize) -> &'static str {
    if granted > 0 && missing == 0 && unknown == 0 {
        "live"
    } else if granted > 0 {
        "partial"
    } else if missing > 0 {
        // Nothing granted and at least one pin read back absent: the chain's
        // provisioning has not run. A state to watch, not a fault to fix.
        "unprovisioned"
    } else {
        "unknown"
    }
}

/// The owners document when `privileged-addresses.json` could not be read.
pub fn owners_unavailable(org: &str, repo: &str, why: &str) -> serde_json::Value {
    json!({ "org": org, "repo": repo, "unavailable": why })
}

/// `live_authorisers` is `(network, authorizer())` of each chain's first
/// receipt vault; `None` where that read failed. `name_of(address)` is the
/// deploy repo's own name for an address on the reference chain.
pub fn build_owners(
    org: &str,
    repo: &str,
    facts: &PrivilegedAddresses,
    onchain: Option<&OnChainSafe>,
    live_authorisers: &[(String, Option<String>)],
    name_of: &dyn Fn(&str) -> Option<String>,
) -> serde_json::Value {
    let threshold = facts.threshold;
    let onchain_owners: Option<Vec<String>> = onchain
        .and_then(|o| o.owners.as_ref())
        .map(|v| v.iter().map(|a| a.to_lowercase()).collect());

    let mut signers: Vec<serde_json::Value> = Vec::new();
    let declared_lower: Vec<String> = facts.owners.iter().map(|a| a.to_lowercase()).collect();
    for (i, (a, al)) in facts.owners.iter().zip(&declared_lower).enumerate() {
        let on_chain = match &onchain_owners {
            None => "unverified",
            Some(set) if set.contains(al) => "match",
            Some(_) => "missing",
        };
        signers.push(json!({
            "role": name_of(a).unwrap_or_else(|| format!("Signer {}", i + 1)),
            "address": a, "network": "", "status": "active", "note": "", "onChain": on_chain,
        }));
    }
    let signer_count = signers.len();
    if let Some(set) = &onchain_owners {
        for oc in set.iter().filter(|oc| !declared_lower.contains(oc)) {
            signers.push(json!({
                "role": "Unexpected on-chain owner", "address": oc,
                "network": onchain.map(|o| o.network.as_str()).unwrap_or(""),
                "status": "extra", "onChain": "extra",
                "note": "present in the live Safe getOwners() but not in privileged-addresses.json",
            }));
        }
    }

    // Both calls must answer: a threshold mismatch or an unanswered threshold
    // read must not read as verified.
    let verification = onchain.map(|o| {
        let signer_match = onchain_owners
            .as_ref()
            .map(|set| set.len() == signer_count && declared_lower.iter().all(|d| set.contains(d)));
        let threshold_match = o.threshold.map(|t| t == threshold);
        json!({
            "reachable": signer_match.is_some() && threshold_match.is_some(),
            "network": o.network,
            "safe": o.safe,
            "rpcHost": o.rpc_host,
            "onChainCount": o.owners.as_ref().map(|v| v.len()),
            "match": signer_match.zip(threshold_match).map(|(owners, thr)| owners && thr),
            "threshold": { "declared": threshold, "onChain": o.threshold, "match": threshold_match },
        })
    });

    let safe = json!({
        "id": "safe",
        "title": "Upgrade authority — token-owner Safe",
        "note": format!("{threshold}-of-{signer_count} Gnosis Safe, replicated per chain. Owner of the production beacons and holder of the authoriser admin roles until the governance timelock takes them."),
        "entries": facts.chains.iter()
            .map(|c| entry("Token-owner Safe", c.safe.clone(), &c.network, "active", "beacon owner + authoriser admin"))
            .collect::<Vec<_>>(),
    });

    let verify_network = onchain
        .map(|o| o.network.as_str())
        .unwrap_or(REFERENCE_NETWORK);
    let signers_group = json!({
        "id": "signers",
        "title": format!("Safe signers ({threshold}-of-{signer_count})"),
        "note": format!("Declared in privileged-addresses.json and checked against the live Safe getOwners() on {verify_network}."),
        "verification": verification,
        "entries": signers,
    });

    let live_of = |network: &str| {
        live_authorisers
            .iter()
            .find(|(n, _)| n == network)
            .and_then(|(_, l)| l.as_deref())
    };
    let mut auth_entries: Vec<serde_json::Value> = facts
        .chains
        .iter()
        .map(|c| {
            entry(
                "Authoriser",
                c.authoriser.clone(),
                &c.network,
                authoriser_status(c.authoriser.as_ref(), live_of(&c.network), "pending"),
                "the authorizer() production receipt vaults should point at",
            )
        })
        .collect();
    auth_entries.push(entry(
        "Authoriser implementation",
        facts.authoriser_implementation.clone(),
        "",
        "active",
        "implementation behind the clones",
    ));
    let authoriser = json!({
        "id": "authoriser",
        "title": "Operational access — authoriser",
        "note": "Every production receipt vault delegates deposit / withdraw / certify authorization to its chain's authoriser. Whether it is live is read from that chain's first receipt vault's authorizer(); an unread chain is unknown, not assumed.",
        "entries": auth_entries,
    });

    let historical = json!({
        "id": "historical",
        "title": "Historical & bricked",
        "note": "Published in privileged-addresses.json but not live control of production.",
        "entries": [
            entry("Deploy-time initial owner", facts.beacon_owner_initial.clone(), "", "migrated", "held the beacons at deploy; ownership since migrated to the Safe"),
            entry("V2 receipt beacon owner", facts.beacon_owner_v2_receipt.clone(), REFERENCE_NETWORK, "bricked", "owned by the token contract itself — the V2 beacon can no longer be upgraded"),
            entry("V2 vault beacon owner", facts.beacon_owner_v2_vault.clone(), REFERENCE_NETWORK, "bricked", "owned by the token contract itself — the V2 beacon can no longer be upgraded"),
        ],
    });

    json!({
        "repo": repo,
        "org": org,
        "threshold": threshold,
        "signerCount": signer_count,
        "groups": [safe, signers_group, authoriser, historical],
    })
}

/// Every chain `privileged-addresses.json` publishes, with no RPC endpoint set.
pub fn chain_pins(facts: &PrivilegedAddresses) -> Vec<ChainPin> {
    facts
        .chains
        .iter()
        .map(|c| ChainPin {
            network: c.network.clone(),
            authoriser: c.authoriser.clone(),
            safe: c.safe.clone(),
            admin_holder: c.timelock.clone(),
            rpc_host: None,
        })
        .collect()
}

/// The authoriser role-grant document: every `(role, grantee)` pair
/// `ideal-roles.json` expects on each chain's authoriser, grouped by grantee,
/// checked per chain.
///
/// `check(network, authoriser, role_id, grantee)` is the `hasRole` probe.
/// `role_names` maps a role id to its name; `name_of(network, address)` names
/// an address.
pub fn build_grants(
    org: &str,
    repo: &str,
    ideal: &[IdealChain],
    chains: &[ChainPin],
    role_names: &[(String, String)],
    name_of: &dyn Fn(&str, &str) -> Option<String>,
    check: &dyn Fn(&str, &str, &str, &str) -> GrantOnChain,
) -> serde_json::Value {
    let eq = |a: &Option<String>, b: &str| a.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(b));
    struct Role {
        id: String,
        name: Option<String>,
        chains: Vec<serde_json::Value>,
    }
    struct Grantee {
        kind: &'static str,
        address: Option<String>,
        ident: Option<String>,
        roles: Vec<Role>,
    }
    let mut grantees: Vec<Grantee> = Vec::new();
    let mut tally: Vec<(usize, usize, usize)> = vec![(0, 0, 0); chains.len()];
    let mut safe_kept: Vec<Vec<String>> = vec![Vec::new(); chains.len()];
    let mut expected_count = 0usize;

    for (i, chain) in chains.iter().enumerate() {
        let Some(expected) = chain
            .authoriser
            .as_deref()
            .and_then(|auth| {
                ideal
                    .iter()
                    .find(|c| c.network == chain.network)?
                    .contract(auth)
            })
            .map(|k| &k.expected)
        else {
            continue;
        };
        for e in expected {
            expected_count += 1;
            let kind = if eq(&chain.safe, &e.grantee) {
                "safe"
            } else if eq(&chain.admin_holder, &e.grantee) {
                "admin-holder"
            } else {
                "constant"
            };
            let fixed = (kind == "constant").then(|| e.grantee.clone());
            let gi = match grantees
                .iter()
                .position(|g| g.kind == kind && g.address == fixed)
            {
                Some(gi) => gi,
                None => {
                    grantees.push(Grantee {
                        kind,
                        address: fixed,
                        ident: name_of(&chain.network, &e.grantee),
                        roles: Vec::new(),
                    });
                    grantees.len() - 1
                }
            };
            let name = e.role_name.clone().or_else(|| {
                role_names
                    .iter()
                    .find(|(id, _)| id.eq_ignore_ascii_case(&e.role))
                    .map(|(_, n)| n.clone())
            });
            let g = &mut grantees[gi];
            let ri = match g.roles.iter().position(|r| r.id == e.role) {
                Some(ri) => ri,
                None => {
                    g.roles.push(Role {
                        id: e.role.clone(),
                        name: name.clone(),
                        chains: Vec::new(),
                    });
                    g.roles.len() - 1
                }
            };
            let status = match (&chain.authoriser, &chain.rpc_host) {
                (Some(auth), Some(_)) => check(&chain.network, auth, &e.role, &e.grantee),
                _ => GrantOnChain::Unknown,
            };
            match status {
                GrantOnChain::Granted => tally[i].0 += 1,
                GrantOnChain::NotGranted => tally[i].1 += 1,
                GrantOnChain::Unknown => tally[i].2 += 1,
            }
            // An admin role expected on someone other than the Safe: does the
            // Safe still hold it? Catches a grant that landed without its renounce.
            let admin = name.as_deref().is_some_and(is_admin_role);
            let safe_holds = match (admin, kind, &chain.authoriser, &chain.rpc_host, &chain.safe) {
                (true, k, Some(auth), Some(_), Some(safe)) if k != "safe" => {
                    match check(&chain.network, auth, &e.role, safe) {
                        GrantOnChain::Granted => Some(true),
                        GrantOnChain::NotGranted => Some(false),
                        GrantOnChain::Unknown => None,
                    }
                }
                _ => None,
            };
            let label = name.clone().unwrap_or_else(|| e.role.clone());
            if safe_holds == Some(true) && !safe_kept[i].contains(&label) {
                safe_kept[i].push(label);
            }
            g.roles[ri].chains.push(json!({
                "network": chain.network,
                "address": e.grantee,
                "status": status.token(),
                "safeHolds": safe_holds,
            }));
        }
    }

    let chain_docs: Vec<serde_json::Value> = chains
        .iter()
        .zip(&tally)
        .zip(&safe_kept)
        .map(|((c, (granted, missing, unknown)), kept)| {
            json!({
                "network": c.network,
                "authoriser": c.authoriser,
                "safe": c.safe,
                "rpcHost": c.rpc_host,
                "granted": granted,
                "missing": missing,
                "unknown": unknown,
                "total": granted + missing + unknown,
                "state": chain_state(*granted, *missing, *unknown),
                "safeKept": kept,
            })
        })
        .collect();

    json!({
        "org": org,
        "repo": repo,
        "source": crate::idealroles::PATH,
        "pinnedCount": expected_count,
        "chains": chain_docs,
        "grantees": grantees.into_iter().map(|g| json!({
            "ident": g.ident,
            "kind": g.kind,
            "address": g.address,
            "roles": g.roles.into_iter().map(|r| json!({
                "role": r.name.clone().unwrap_or_else(|| r.id.clone()),
                "roleId": r.id,
                "admin": r.name.as_deref().is_some_and(is_admin_role),
                "chains": r.chains,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// The grants document when `ideal-roles.json` could not be read.
pub fn grants_unavailable(org: &str, repo: &str, why: &str) -> serde_json::Value {
    json!({ "org": org, "repo": repo, "source": crate::idealroles::PATH, "unavailable": why })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idealroles::{Contract, Expected};
    use crate::privileged::tests::{doc, A, B};

    const BASE_AUTH: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
    const SERVICE: &str = "0x1c66d6708914c40239d54919320b4c48cae3d1a9";

    fn facts() -> PrivilegedAddresses {
        crate::privileged::parse(&doc().to_string()).unwrap()
    }

    fn no_names(_: &str) -> Option<String> {
        None
    }

    fn group<'a>(v: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == id)
            .unwrap()
    }

    #[test]
    fn owners_come_from_the_file_in_its_order() {
        let names = |a: &str| (a == B).then(|| "STOX_TOKEN_OWNER_SAFE_OWNER_TDM".to_string());
        let live = vec![
            ("base".to_string(), Some(A.to_string())),
            ("ethereum".to_string(), Some(A.to_string())),
        ];
        let v = build_owners("o", "r", &facts(), None, &live, &names);
        assert_eq!(
            (v["threshold"].as_u64(), v["signerCount"].as_u64()),
            (Some(2), Some(2))
        );
        let s = group(&v, "signers");
        assert_eq!(s["title"], "Safe signers (2-of-2)");
        assert_eq!(s["entries"][0]["role"], "Signer 1");
        assert_eq!(s["entries"][0]["address"], A);
        assert_eq!(s["entries"][1]["role"], "STOX_TOKEN_OWNER_SAFE_OWNER_TDM");
        assert_eq!(s["entries"][1]["onChain"], "unverified");
        assert!(s["verification"].is_null());
        let safes = &group(&v, "safe")["entries"];
        assert_eq!(
            (safes[0]["network"].as_str(), safes[0]["address"].as_str()),
            (Some("ethereum"), Some(B))
        );
        assert_eq!(
            (safes[1]["network"].as_str(), safes[1]["address"].as_str()),
            (Some("base"), Some(A))
        );
        let auth = &group(&v, "authoriser")["entries"];
        assert_eq!(
            (auth[0]["network"].as_str(), auth[0]["status"].as_str()),
            (Some("ethereum"), Some("pending"))
        );
        assert_eq!(
            (auth[1]["network"].as_str(), auth[1]["status"].as_str()),
            (Some("base"), Some("active"))
        );
        assert_eq!(auth[2]["role"], "Authoriser implementation");
        assert_eq!(auth[2]["address"], A);
        let hist = &group(&v, "historical")["entries"];
        assert_eq!(hist[0]["address"], A);
        assert_eq!(hist[1]["address"], B);
        assert!(hist[2]["address"].is_null());
    }

    #[test]
    fn an_unread_authoriser_is_unknown() {
        let live = vec![("base".to_string(), None)];
        let v = build_owners("o", "r", &facts(), None, &live, &no_names);
        let auth = &group(&v, "authoriser")["entries"];
        assert_eq!(auth[0]["status"], "unknown");
        assert_eq!(auth[1]["status"], "unknown");
    }

    #[test]
    fn each_chains_authoriser_is_judged_by_that_chains_live_read() {
        let status = |base: &str, eth: &str| {
            let live = vec![
                ("base".to_string(), Some(base.to_string())),
                ("ethereum".to_string(), Some(eth.to_string())),
            ];
            let v = build_owners("o", "r", &facts(), None, &live, &no_names);
            let auth = &group(&v, "authoriser")["entries"];
            (
                auth[0]["status"].as_str().unwrap().to_string(),
                auth[1]["status"].as_str().unwrap().to_string(),
            )
        };
        assert_eq!(status(A, B), ("active".into(), "active".into()));
        assert_eq!(status(B, A), ("pending".into(), "pending".into()));
    }

    fn onchain(owners: Option<Vec<&str>>, threshold: Option<u64>) -> OnChainSafe {
        OnChainSafe {
            network: "base".into(),
            safe: A.into(),
            rpc_host: "h".into(),
            owners: owners.map(|v| v.into_iter().map(String::from).collect()),
            threshold,
        }
    }

    #[test]
    fn the_live_safe_verifies_roster_and_threshold() {
        let f = facts();
        let ok = build_owners(
            "o",
            "r",
            &f,
            Some(&onchain(
                Some(vec![&B.to_uppercase().replace("0X", "0x"), A]),
                Some(2),
            )),
            &[],
            &no_names,
        );
        let s = group(&ok, "signers");
        assert_eq!(s["verification"]["match"], true);
        assert_eq!(s["verification"]["reachable"], true);
        assert_eq!(s["entries"][1]["onChain"], "match");

        let drift = build_owners(
            "o",
            "r",
            &f,
            Some(&onchain(
                Some(vec![A, "0x00000000000000000000000000000000000000c3"]),
                Some(2),
            )),
            &[],
            &no_names,
        );
        let s = group(&drift, "signers");
        assert_eq!(s["verification"]["match"], false);
        assert_eq!(s["entries"][1]["onChain"], "missing");
        assert_eq!(s["entries"].as_array().unwrap().len(), 3);
        assert_eq!(s["entries"][2]["status"], "extra");
        assert_eq!(
            s["entries"][2]["address"],
            "0x00000000000000000000000000000000000000c3"
        );
        assert_eq!(s["entries"][2]["network"], "base");

        let superset = build_owners(
            "o",
            "r",
            &f,
            Some(&onchain(
                Some(vec![A, B, "0x00000000000000000000000000000000000000c3"]),
                Some(2),
            )),
            &[],
            &no_names,
        );
        let s = group(&superset, "signers");
        assert_eq!(s["entries"][1]["onChain"], "match");
        assert_eq!(s["verification"]["match"], false);

        let thr = build_owners(
            "o",
            "r",
            &f,
            Some(&onchain(Some(vec![A, B]), Some(1))),
            &[],
            &no_names,
        );
        assert_eq!(group(&thr, "signers")["verification"]["match"], false);
        assert_eq!(
            group(&thr, "signers")["verification"]["threshold"]["match"],
            false
        );

        let half = build_owners(
            "o",
            "r",
            &f,
            Some(&onchain(Some(vec![A, B]), None)),
            &[],
            &no_names,
        );
        assert_eq!(group(&half, "signers")["verification"]["reachable"], false);
        assert!(group(&half, "signers")["verification"]["match"].is_null());
        let none = build_owners("o", "r", &f, Some(&onchain(None, Some(2))), &[], &no_names);
        assert_eq!(group(&none, "signers")["verification"]["reachable"], false);
        assert_eq!(
            group(&none, "signers")["entries"][0]["onChain"],
            "unverified"
        );
    }

    #[test]
    fn chain_pins_carry_each_chains_own_addresses() {
        let pins = chain_pins(&facts());
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[1].network, "base");
        assert_eq!(
            (
                pins[1].safe.as_deref(),
                pins[1].admin_holder.as_deref(),
                pins[1].authoriser.as_deref()
            ),
            (Some(A), Some(B), Some(A))
        );
        assert_eq!(pins[0].admin_holder, None);
        assert!(pins.iter().all(|p| p.rpc_host.is_none()));
    }

    fn role(name: &str) -> String {
        crate::rpc::role_id_hex(name)
    }

    fn ideal_for(chains: &[ChainPin], pairs: &[(&str, &str)]) -> Vec<IdealChain> {
        chains
            .iter()
            .map(|c| IdealChain {
                network: c.network.clone(),
                contracts: vec![Contract {
                    address: c.authoriser.clone().unwrap().to_lowercase(),
                    expected: pairs
                        .iter()
                        .map(|(r, g)| {
                            let grantee = match *g {
                                "safe" => c.safe.clone().unwrap(),
                                "timelock" => c.admin_holder.clone().unwrap(),
                                other => other.to_string(),
                            };
                            Expected {
                                role: role(r),
                                role_name: None,
                                grantee: grantee.to_lowercase(),
                            }
                        })
                        .collect(),
                }],
            })
            .collect()
    }

    fn names() -> Vec<(String, String)> {
        ["DEPOSIT", "DEPOSIT_ADMIN", "CERTIFY"]
            .iter()
            .map(|n| (role(n), n.to_string()))
            .collect()
    }

    fn named(_: &str, a: &str) -> Option<String> {
        a.eq_ignore_ascii_case(SERVICE)
            .then(|| "GRANTEE_SERVICE_1C66".into())
    }

    fn grants(
        chains: &[ChainPin],
        pairs: &[(&str, &str)],
        check: &dyn Fn(&str, &str, &str, &str) -> GrantOnChain,
    ) -> serde_json::Value {
        build_grants(
            "o",
            "r",
            &ideal_for(chains, pairs),
            chains,
            &names(),
            &named,
            check,
        )
    }

    fn all_granted(_n: &str, _a: &str, _r: &str, _g: &str) -> GrantOnChain {
        GrantOnChain::Granted
    }

    #[test]
    fn grants_are_grouped_by_grantee_kind_with_each_chains_address() {
        let chains = two_chains();
        let v = grants(
            &chains,
            &[
                ("DEPOSIT", SERVICE),
                ("DEPOSIT_ADMIN", "timelock"),
                ("CERTIFY", "safe"),
                ("CERTIFY", SERVICE),
            ],
            &all_granted,
        );
        assert_eq!(v["source"], "ideal-roles.json");
        assert_eq!(v["pinnedCount"], 8);
        let g = v["grantees"].as_array().unwrap();
        assert_eq!(g.len(), 3);
        assert_eq!(
            (
                g[0]["kind"].as_str(),
                g[0]["ident"].as_str(),
                g[0]["address"].as_str()
            ),
            (
                Some("constant"),
                Some("GRANTEE_SERVICE_1C66"),
                Some(SERVICE)
            )
        );
        assert_eq!(
            g[0]["roles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["DEPOSIT", "CERTIFY"]
        );
        assert_eq!(g[1]["kind"], "admin-holder");
        assert!(g[1]["address"].is_null());
        assert_eq!(g[1]["roles"][0]["admin"], true);
        assert_eq!(g[1]["roles"][0]["roleId"], role("DEPOSIT_ADMIN"));
        let tl = &g[1]["roles"][0]["chains"];
        assert_eq!(
            tl[0]["address"],
            chains[0].admin_holder.clone().unwrap().to_lowercase()
        );
        assert_eq!(
            tl[1]["address"],
            chains[1].admin_holder.clone().unwrap().to_lowercase()
        );
        assert_eq!(g[2]["kind"], "safe");
        assert_eq!(g[2]["roles"][0]["admin"], false);
        assert_eq!(v["chains"][0]["state"], "live");
        assert_eq!(v["chains"][0]["total"], 4);
    }

    #[test]
    fn an_unnamed_role_shows_its_id_and_is_not_admin() {
        let chains = two_chains();
        let v = grants(&chains, &[("WITHDRAW_ADMIN", SERVICE)], &all_granted);
        let r = &v["grantees"][0]["roles"][0];
        assert_eq!(r["role"], role("WITHDRAW_ADMIN"));
        assert_eq!(r["admin"], false);
    }

    #[test]
    fn a_safe_that_kept_an_admin_role_is_reported() {
        let mut chains = two_chains();
        let safe0 = chains[0].safe.clone().unwrap();
        let check = move |_n: &str, _a: &str, _r: &str, g: &str| {
            if g.eq_ignore_ascii_case(&safe0) {
                GrantOnChain::Granted
            } else {
                GrantOnChain::NotGranted
            }
        };
        let v = grants(
            &chains,
            &[("DEPOSIT_ADMIN", "timelock"), ("DEPOSIT", SERVICE)],
            &check,
        );
        assert_eq!(v["chains"][0]["safeKept"], json!(["DEPOSIT_ADMIN"]));
        assert_eq!(v["chains"][1]["safeKept"], json!([]));
        assert_eq!(v["grantees"][0]["roles"][0]["chains"][0]["safeHolds"], true);
        assert_eq!(
            v["grantees"][0]["roles"][0]["chains"][1]["safeHolds"],
            false
        );
        assert!(v["grantees"][1]["roles"][0]["chains"][0]["safeHolds"].is_null());
        assert_eq!(v["chains"][1]["state"], "unprovisioned");

        chains[1].rpc_host = None;
        let v = grants(&chains, &[("DEPOSIT_ADMIN", "timelock")], &check);
        assert_eq!(v["chains"][1]["state"], "unknown");
        assert!(v["grantees"][0]["roles"][0]["chains"][1]["safeHolds"].is_null());
    }

    #[test]
    fn an_admin_role_the_safe_is_expected_to_hold_is_not_kept() {
        let v = grants(&two_chains(), &[("DEPOSIT_ADMIN", "safe")], &all_granted);
        assert_eq!(v["chains"][0]["safeKept"], json!([]));
        assert!(v["grantees"][0]["roles"][0]["chains"][0]["safeHolds"].is_null());
    }

    #[test]
    fn a_kept_admin_role_is_listed_once_however_many_grantees_expect_it() {
        let v = grants(
            &two_chains(),
            &[("DEPOSIT_ADMIN", "timelock"), ("DEPOSIT_ADMIN", SERVICE)],
            &all_granted,
        );
        assert_eq!(v["chains"][0]["safeKept"], json!(["DEPOSIT_ADMIN"]));
    }

    #[test]
    fn the_probe_is_asked_the_role_id_on_that_chains_authoriser() {
        let chains = two_chains();
        let seen = std::cell::RefCell::new(Vec::new());
        let check = |n: &str, a: &str, r: &str, g: &str| {
            seen.borrow_mut()
                .push((n.to_string(), a.to_string(), r.to_string(), g.to_string()));
            GrantOnChain::Granted
        };
        grants(&chains, &[("CERTIFY", SERVICE)], &check);
        assert_eq!(
            seen.into_inner(),
            vec![
                (
                    "base".into(),
                    BASE_AUTH.into(),
                    role("CERTIFY"),
                    SERVICE.into()
                ),
                (
                    "ethereum".into(),
                    chains[1].authoriser.clone().unwrap(),
                    role("CERTIFY"),
                    SERVICE.into()
                ),
            ]
        );
    }

    #[test]
    fn a_chain_absent_from_ideal_roles_has_nothing_expected() {
        let chains = two_chains();
        let ideal = ideal_for(&chains[..1], &[("DEPOSIT", SERVICE)]);
        let v = build_grants("o", "r", &ideal, &chains, &names(), &named, &all_granted);
        assert_eq!(v["chains"][1]["total"], 0);
        assert_eq!(v["chains"][1]["state"], "unknown");
        assert_eq!(
            v["grantees"][0]["roles"][0]["chains"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let mut no_auth = chains.clone();
        no_auth[0].authoriser = None;
        let v = build_grants("o", "r", &ideal, &no_auth, &names(), &named, &all_granted);
        assert_eq!(v["pinnedCount"], 0);
    }

    #[test]
    fn an_explicit_role_name_wins() {
        let chains = two_chains();
        let mut ideal = ideal_for(&chains, &[("DEPOSIT", SERVICE)]);
        ideal[0].contracts[0].expected[0].role_name = Some("CUSTOM_ADMIN".into());
        let v = build_grants("o", "r", &ideal, &chains, &names(), &named, &all_granted);
        assert_eq!(v["grantees"][0]["roles"][0]["role"], "CUSTOM_ADMIN");
        assert_eq!(v["grantees"][0]["roles"][0]["admin"], true);
    }

    #[test]
    fn unavailable_documents_say_why() {
        assert_eq!(
            owners_unavailable("o", "r", "HTTP 404")["unavailable"],
            "HTTP 404"
        );
        let g = grants_unavailable("o", "r", "it is not JSON");
        assert_eq!(
            (g["unavailable"].as_str(), g["source"].as_str()),
            (Some("it is not JSON"), Some("ideal-roles.json"))
        );
    }

    /// Two chains with everything pinned, so a test only has to vary the probe.
    fn two_chains() -> Vec<ChainPin> {
        vec![
            ChainPin {
                network: "base".into(),
                authoriser: Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD".into()),
                safe: Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611".into()),
                admin_holder: Some("0x48ba1371A78E6cC54157c63721756ab444510DB3".into()),
                rpc_host: Some("mainnet.base.org".into()),
            },
            ChainPin {
                network: "ethereum".into(),
                authoriser: Some("0x66566cc91dEAf818859bD4b09B7903ac48998157".into()),
                safe: Some("0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329".into()),
                admin_holder: Some("0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B".into()),
                rpc_host: Some("ethereum-rpc.publicnode.com".into()),
            },
        ]
    }
    /// A vault's owner MUST be labelled against THAT chain's pins.
    ///
    /// Base's timelock is not Ethereum's, so a single expected owner would
    /// label one chain's moved vaults as `other` — the one answer that reads as
    /// "nobody predicted this".
    #[test]
    fn a_vault_owner_is_labelled_against_its_own_chains_pins() {
        let vaults = vec!["0xaaa".to_string(), "0xbbb".to_string()];
        // base: still on the Safe. ethereum: moved to the timelock.
        let read = |network: &str, vs: &[String]| {
            vs.iter()
                .map(|v| match (network, v.as_str()) {
                    ("base", _) => Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611".to_string()),
                    ("ethereum", "0xaaa") => {
                        Some("0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B".to_string())
                    }
                    _ => None,
                })
                .collect()
        };
        let same = |_: &str| vaults.clone();
        let out = build_vault_owners(&same, &two_chains(), &read).expect("builds");
        // No page-wide `total`: the vault list is per chain now, so one
        // number across all of them would be the sum of five different sets.
        let chains = out["chains"].as_array().expect("chains");
        assert_eq!(chains[0]["counts"]["safe"], serde_json::json!(2));
        assert_eq!(chains[0]["counts"]["timelock"], serde_json::json!(0));
        // The Safe's own address on ethereum would be `safe`; the timelock's is
        // `timelock`; a vault with no code answers nothing and is `unknown`.
        assert_eq!(chains[1]["counts"]["timelock"], serde_json::json!(1));
        assert_eq!(chains[1]["counts"]["unknown"], serde_json::json!(1));
        assert_eq!(
            chains[1]["vaults"][0]["label"],
            serde_json::json!("timelock")
        );
        assert_eq!(
            chains[1]["vaults"][1]["label"],
            serde_json::json!("unknown")
        );
    }

    /// An owner that is neither pin MUST read `other`, never `unknown`.
    ///
    /// They mean opposite things: `unknown` is "we could not ask", `other` is
    /// "we asked and the answer was nobody we pinned".
    #[test]
    fn an_unpinned_owner_is_other_not_unknown() {
        let vaults = vec!["0xaaa".to_string()];
        let read = |_: &str, vs: &[String]| {
            vs.iter()
                .map(|_| Some("0x00000000000000000000000000000000deadbeef".to_string()))
                .collect()
        };
        let same = |_: &str| vaults.clone();
        let out = build_vault_owners(&same, &two_chains(), &read).expect("builds");
        let chains = out["chains"].as_array().expect("chains");
        assert_eq!(chains[0]["counts"]["other"], serde_json::json!(1));
        assert_eq!(chains[0]["counts"]["unknown"], serde_json::json!(0));
    }

    /// A chain with no timelock pinned MUST read `unpinned`, not `other`.
    ///
    /// Chains arrive one at a time, and one can carry an authoriser clone —
    /// which is what puts it in this list — before it carries a
    /// timelock in privileged-addresses.json. There is then nothing to recognise
    /// a moved vault by, so every vault not on the Safe would read `other`:
    /// red, for a rollout that has not reached the chain yet. The grant rows
    /// make the same distinction for the same reason.
    #[test]
    fn a_chain_with_no_timelock_pin_is_unpinned_not_other() {
        let vaults = vec!["0xaaa".to_string()];
        let mut chains = two_chains();
        chains[0].admin_holder = None;
        // An owner that is neither the Safe nor anything pinned.
        let read = |_: &str, vs: &[String]| {
            vs.iter()
                .map(|_| Some("0x00000000000000000000000000000000deadbeef".to_string()))
                .collect()
        };
        let same = |_: &str| vaults.clone();
        let out = build_vault_owners(&same, &chains, &read).expect("builds");
        let chains_out = out["chains"].as_array().expect("chains");
        assert_eq!(chains_out[0]["counts"]["unpinned"], serde_json::json!(1));
        assert_eq!(
            chains_out[0]["counts"]["other"],
            serde_json::json!(0),
            "a missing pin is not an unexpected owner"
        );
        // The chain that DOES pin a timelock still calls it out.
        assert_eq!(chains_out[1]["counts"]["other"], serde_json::json!(1));
    }

    /// No vaults MUST be `None` rather than an empty section, so the page does
    /// not render a table that asserts nothing.
    #[test]
    fn no_vaults_is_none() {
        let read = |_: &str, vs: &[String]| vec![None; vs.len()];
        let none = |_: &str| Vec::new();
        // No chains is the only "nothing to report"; an empty list for ONE chain
        // is that chain holding none, which the other chains still answer for.
        assert!(build_vault_owners(&none, &[], &read).is_none());
    }

    /// A probe that answers SHORT MUST leave the tail unread.
    ///
    /// A batch can come back with fewer results than it was asked for, and it
    /// does not say which calls it covered. Reading past the end by index, or
    /// zipping, would hand one vault the owner of another — a plausible
    /// address belonging to something else, which is worse than a gap.
    #[test]
    fn a_short_answer_leaves_the_tail_unread() {
        let vaults = vec!["0xaaa".to_string(), "0xbbb".to_string()];
        let read = |_: &str, _: &[String]| {
            vec![Some(
                "0xe70d821f3462a074e63b42d0AaC6523faAe1d611".to_string(),
            )]
        };
        let same = |_: &str| vaults.clone();
        let out = build_vault_owners(&same, &two_chains(), &read).expect("builds");
        let chains = out["chains"].as_array().expect("chains");
        assert_eq!(chains[0]["counts"]["safe"], serde_json::json!(1));
        assert_eq!(chains[0]["counts"]["unknown"], serde_json::json!(1));
        assert_eq!(chains[0]["vaults"][1]["owner"], serde_json::Value::Null);
    }
}
