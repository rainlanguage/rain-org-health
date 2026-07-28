//! Reads st0x.deploy's owner / privileged-address constants so the Deployments
//! page can enumerate who controls production. Pure parsing + assembly live here
//! and are unit-tested; the network fetch (`gh_file`) is in main.rs.
//!
//! The addresses are the volatile fact and come from the repo (so the dashboard
//! tracks the source of truth); the role labels, grouping, and "what it controls"
//! notes are the stable curation and live here. Each address is read from the
//! file that declares it as a LITERAL — never from an aliasing re-export — so
//! parsing never has to resolve `= OtherLib.CONST;`.
//!
//! `build_owners` covers the named pins — the Safes, the signers, the authoriser
//! clones. `build_grants` covers the `(role, grantee)` map: who holds DEPOSIT /
//! WITHDRAW / CERTIFY on those authorisers, and on which chains it is live. The
//! second is derived END TO END — grantees, roles and chains all read out of the
//! deploy repo — because the thing it reports on is a set that grows, and a
//! curated list of hot keys is only ever as complete as somebody's memory.

use regex::Regex;
use serde_json::json;

/// Extract `[visibility] constant NAME = <value>;` and return the 20-byte hex
/// literal exactly as written (EIP-55 checksum preserved). Handles both the
/// `address(0x…)`-wrapped and bare `0x…` forms. Returns `None` for an aliased RHS
/// (`= OtherLib.CONST;`) or a missing constant.
///
/// The `\b…\b` around the name stops a prefix match: `STOX_TOKEN_OWNER_SAFE` does
/// not match inside `STOX_TOKEN_OWNER_SAFE_ETHEREUM` (no word boundary before the
/// `_`). The trailing non-hex char stops a 40-of-64 match against a `bytes32`.
pub fn parse_address_constant(src: &str, name: &str) -> Option<String> {
    let pattern = format!(
        r"\b{}\b\s*=\s*(?:address\(\s*)?(0x[0-9a-fA-F]{{40}})[^0-9a-fA-F]",
        regex::escape(name),
    );
    let re = Regex::new(&pattern).ok()?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract a `uint256 [visibility] constant NAME = <n>;` integer. Used for the
/// Safe signature threshold.
pub fn parse_uint_constant(src: &str, name: &str) -> Option<u64> {
    let pattern = format!(r"\b{}\b\s*=\s*(\d+)", regex::escape(name));
    let re = Regex::new(&pattern).ok()?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

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

/// An address constant that resolved to the zero address — a pin declared but
/// not yet hydrated. Distinct from `None` (the constant is absent entirely).
///
/// This is a fact about the SOURCE, not the chain. With no address there is
/// nothing to look up, so an unhydrated pin cannot imply the contract is
/// undeployed — it frequently is deployed, and the pin is simply behind.
fn is_unhydrated(pin: Option<&String>) -> bool {
    pin.is_some_and(|a| a.trim_start_matches("0x").chars().all(|c| c == '0'))
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

/// The st0x.deploy library sources the owners document is assembled from.
/// Grouped so adding a source is a field, not another positional argument in a
/// list where `&str`s are already indistinguishable at the call site.
pub struct OwnerSources<'a> {
    /// `src/lib/LibSafeInvariants.sol` — the Safes, signers, threshold.
    pub safe_lib: &'a str,
    /// `src/lib/LibAuthoriserInvariants.sol` — authoriser + grantees.
    pub auth_lib: &'a str,
    /// `src/generated/LibProdDeployV4.sol` — deploy EOA, V4 clones.
    pub v4_lib: &'a str,
    /// `src/lib/LibProdDeployV2BaseOverrides.sol` — bricked V2 beacons.
    pub overrides: &'a str,
}

/// Assemble the `deploymentOwners` document from the st0x.deploy library
/// sources. Returns `None` when the anchor (the Base token-owner Safe) can't be
/// resolved — i.e. the repo was unreachable or the constant moved — so the page
/// shows an honest "unavailable" state instead of a table of nulls.
///
/// `live_authoriser` is the `authorizer()` a production vault actually returns;
/// `None` when that read failed, which leaves every clone's status `unknown`
/// rather than falling back to a literal.
pub fn build_owners(
    org: &str,
    repo: &str,
    src: &OwnerSources,
    onchain: Option<&OnChainSafe>,
    live_authoriser: Option<&str>,
) -> Option<serde_json::Value> {
    let (safe_lib, auth_lib, v4_lib, overrides) =
        (src.safe_lib, src.auth_lib, src.v4_lib, src.overrides);
    let addr = parse_address_constant;

    // Anchor: without the Base Safe there is nothing meaningful to show.
    let base_safe = addr(safe_lib, "STOX_TOKEN_OWNER_SAFE")?;
    let eth_safe = addr(safe_lib, "STOX_TOKEN_OWNER_SAFE_ETHEREUM");
    let threshold = parse_uint_constant(safe_lib, "STOX_TOKEN_OWNER_SAFE_THRESHOLD").unwrap_or(3);

    // The live owner set (lowercased — getOwners returns unchecksummed) for the
    // declared-vs-actual comparison, or None when the RPC didn't answer.
    let onchain_owners: Option<Vec<String>> = onchain
        .and_then(|o| o.owners.as_ref())
        .map(|v| v.iter().map(|a| a.to_lowercase()).collect());

    // Read the declared roster from the constants (walk _1, _2, … until an index
    // is undefined, so the count tracks the actual Safe), and record for each
    // whether it is present in the live getOwners() set.
    let mut signers: Vec<serde_json::Value> = Vec::new();
    let mut declared_lower: Vec<String> = Vec::new();
    for i in 1..=64 {
        let Some(a) = addr(safe_lib, &format!("STOX_TOKEN_OWNER_SAFE_OWNER_{i}")) else {
            break;
        };
        let al = a.to_lowercase();
        let on_chain = match &onchain_owners {
            None => "unverified",
            Some(set) if set.contains(&al) => "match",
            Some(_) => "missing",
        };
        declared_lower.push(al);
        signers.push(json!({
            "role": format!("Signer {i}"), "address": a, "network": "",
            "status": "active", "note": "", "onChain": on_chain,
        }));
    }
    let signer_count = signers.len();

    // Any live owner NOT in the declared set is unexpected — surface the drift
    // rather than hide it.
    if let Some(set) = &onchain_owners {
        for oc in set.iter().filter(|oc| !declared_lower.contains(oc)) {
            signers.push(json!({
                "role": "Unexpected on-chain owner", "address": oc, "network": "base",
                "status": "extra", "onChain": "extra",
                "note": "present in the live Safe getOwners() but not in the declared constants",
            }));
        }
    }

    // Provenance verdict for the roster + threshold: the declared set matches the
    // live set iff they are the same size and every declared owner is on-chain.
    let verification = onchain.map(|o| {
        let signer_match = onchain_owners
            .as_ref()
            .map(|set| set.len() == signer_count && declared_lower.iter().all(|d| set.contains(d)));
        let threshold_match = o.threshold.map(|t| t == threshold);
        // The verdict needs BOTH calls: a threshold mismatch — or a threshold RPC
        // that didn't answer — must not read as verified. So `reachable` means both
        // answered, and `match` requires the roster AND the threshold to agree.
        let reachable = signer_match.is_some() && threshold_match.is_some();
        json!({
            "reachable": reachable,
            "network": o.network,
            "safe": o.safe,
            "rpcHost": o.rpc_host,
            "onChainCount": o.owners.as_ref().map(|v| v.len()),
            "match": signer_match
                .zip(threshold_match)
                .map(|(owners, thr)| owners && thr),
            "threshold": {
                "declared": threshold,
                "onChain": o.threshold,
                "match": threshold_match,
            },
        })
    });

    let safe = json!({
        "id": "safe",
        "title": "Upgrade authority — token-owner Safe",
        "note": format!("{threshold}-of-{signer_count} Gnosis Safe, replicated per chain. Current owner of every production beacon (power to upgrade all proxies) and holder of every authoriser admin role."),
        "entries": [
            entry("Base Safe", Some(base_safe), "base", "active", "beacon owner + authoriser admin"),
            entry("Ethereum Safe", eth_safe, "ethereum", "active", "same policy, per-chain address"),
        ],
    });

    let signers_group = json!({
        "id": "signers",
        "title": format!("Safe signers ({threshold}-of-{signer_count})"),
        "note": "Declared in the st0x.deploy constants and checked against the live Safe getOwners() on Base.",
        "verification": verification,
        "entries": signers,
    });

    // Which clone is live is READ FROM THE CHAIN, not asserted here: the two
    // Base clones trade places during the V4 migration, and a hardcoded
    // active/pending pair goes stale the moment the swap lands — while the
    // token rows on the same page keep reporting the truth from `authorizer()`.
    let v3_clone = addr(auth_lib, "STOX_PROD_AUTHORISER");
    let v4_clone = addr(v4_lib, "STOX_PROD_AUTHORISER_V4_CLONE");
    let v4_clone_ethereum = addr(v4_lib, "STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM");
    let live_note = match live_authoriser {
        Some(_) => "Which clone is live is read from a production vault's authorizer() on Base.",
        None => "The live authorizer() read failed, so no clone is marked active — status unknown, not assumed.",
    };
    let authoriser = json!({
        "id": "authoriser",
        "title": "Operational access — authoriser",
        "note": format!("Every production receipt vault delegates deposit / withdraw / certify authorization to this authoriser. {live_note}"),
        "entries": [
            entry("V3-era authoriser clone", v3_clone.clone(), "base",
                authoriser_status(v3_clone.as_ref(), live_authoriser, "migrated"),
                "the pre-V4 authorizer() target"),
            entry("Authoriser implementation", addr(auth_lib, "STOX_PROD_AUTHORISER_IMPL"), "base", "active", "implementation behind the clone"),
            entry("V4 authoriser clone", v4_clone.clone(), "base",
                authoriser_status(v4_clone.as_ref(), live_authoriser, "pending"),
                "the V4 upgrade rewires every vault onto this"),
            // Ethereum's clone is a nonce-based CloneFactory deploy, so its
            // address cannot be known ahead of the broadcast. The row is
            // rendered either way: an absent chain reads as "we do not deploy
            // there", which is the opposite of what an unhydrated pin means.
            entry("V4 authoriser clone", v4_clone_ethereum.clone(), "ethereum",
                if v4_clone_ethereum.is_none() { "unknown" }
                else if is_unhydrated(v4_clone_ethereum.as_ref()) { "pending" }
                else { "active" },
                if is_unhydrated(v4_clone_ethereum.as_ref()) { "pin not yet hydrated — this says nothing about whether the clone exists" }
                else { "the Ethereum bootstrap's authoriser clone" }),
            // The grantees that hold roles ON these authorisers are NOT listed
            // here. They used to be — one hand-written row naming one service
            // constant — and that row could only ever describe the grantee
            // somebody remembered to type. They are derived from the pinned
            // grant map instead; see `build_grants`.
        ],
    });

    let historical = json!({
        "id": "historical",
        "title": "Historical & bricked",
        "note": "Defined in the constants but not live control of production.",
        "entries": [
            entry("Deploy-time initial owner", addr(v4_lib, "BEACON_INITIAL_OWNER"), "", "migrated", "held the beacons at deploy; ownership since migrated to the Safe"),
            entry("V2 receipt beacon owner", addr(overrides, "RECEIPT_BEACON_OWNER"), "base", "bricked", "owned by the token contract itself — the V2 beacon can no longer be upgraded"),
            entry("V2 vault beacon owner", addr(overrides, "VAULT_BEACON_OWNER"), "base", "bricked", "owned by the token contract itself — the V2 beacon can no longer be upgraded"),
        ],
    });

    Some(json!({
        "repo": repo,
        "org": org,
        "threshold": threshold,
        "signerCount": signer_count,
        "groups": [safe, signers_group, authoriser, historical],
    }))
}

// ---------------------------------------------------------------------------
// Authoriser role grants (#143)
//
// Who can UPGRADE production is the Safe, and the page already carried it. Who
// can MOVE VALUE is a different set — service EOAs holding DEPOSIT / WITHDRAW /
// CERTIFY on the authoriser — and it was absent.
//
// It is DERIVED, never transcribed. `LibAuthoriserInvariants.expectedGrants` is
// the one place the `(role, grantee)` pairs live: the same map the on-chain
// assertions iterate. So this reads that map and reports exactly what it says.
// A grantee added there appears here, and one removed disappears, with no change
// to this file — which is the only version of the feature worth having, because
// the whole hazard is a hot key nobody remembered to add to a list.
// ---------------------------------------------------------------------------

/// One `(role, grantee)` pair exactly as the pinned map writes it: the role NAME
/// from `keccak256("…")`, and the grantee IDENTIFIER — either a constant name or
/// the function's own per-chain Safe parameter.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GrantPin {
    pub role: String,
    pub grantee: String,
}

/// The pinned grant map, parsed out of the Safe-parametric `expectedGrants`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GrantMap {
    /// The Safe parameter's name. A grantee slot filled by it is THAT chain's
    /// token-owner Safe, not a constant — which is how one map describes every
    /// chain.
    pub safe_param: String,
    /// Source order, which is the order the map is read in and the order the
    /// page lists grantees in.
    pub grants: Vec<GrantPin>,
    /// The length the source declares (`new RoleGrant[](13)`). Kept so a map
    /// whose entries did not all parse reports a shortfall instead of quietly
    /// looking smaller than it is.
    pub declared: Option<usize>,
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

/// The body of the first `function <name>(<sig>)` whose signature matches
/// `sig_contains`, by brace matching from the signature's `{`.
fn function_body<'a>(src: &'a str, name: &str, sig_contains: &str) -> Option<(&'a str, &'a str)> {
    let mut from = 0usize;
    loop {
        let at = src[from..].find(&format!("function {name}"))? + from;
        let open_paren = src[at..].find('(')? + at;
        let close_paren = src[open_paren..].find(')')? + open_paren;
        let params = &src[open_paren + 1..close_paren];
        let open = src[close_paren..].find('{')? + close_paren;
        // Brace-match the body. Role names carry no braces, so counting is
        // enough here — this parses one known library, not arbitrary Solidity.
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        if params.contains(sig_contains) {
            return Some((params, &src[open + 1..end]));
        }
        from = end;
    }
}

/// Parse the Safe-parametric `expectedGrants(address …)` map.
///
/// The no-arg overload just delegates to this one with Base's Safe, so the
/// parametric body is where every `(role, grantee)` pair actually is — and its
/// Safe parameter is what makes the same map describe every chain.
pub fn parse_expected_grants(src: &str) -> Option<GrantMap> {
    let (params, body) = function_body(src, "expectedGrants", "address")?;
    let safe_param = Regex::new(r"address\s+(?:memory\s+|calldata\s+)?([A-Za-z_][A-Za-z0-9_]*)")
        .ok()?
        .captures(params)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())?;
    let declared = Regex::new(r"new\s+RoleGrant\[\]\s*\(\s*(\d+)\s*\)")
        .ok()
        .and_then(|re| re.captures(body))
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok());
    let pair = Regex::new(
        r#"RoleGrant\(\s*keccak256\(\s*"([A-Za-z0-9_]+)"\s*\)\s*,\s*([A-Za-z_][A-Za-z0-9_.]*)\s*\)"#,
    )
    .ok()?;
    let grants: Vec<GrantPin> = pair
        .captures_iter(body)
        .map(|c| GrantPin {
            role: c[1].to_string(),
            grantee: c[2].to_string(),
        })
        .collect();
    if grants.is_empty() {
        return None;
    }
    Some(GrantMap {
        safe_param,
        grants,
        declared,
    })
}

/// A chain the deploy repo pins production state for: its authoriser (the
/// contract the grants live on) and its token-owner Safe (the per-chain grantee
/// the map is parameterised by).
///
/// `rpc_host` is `None` for a chain the scanner has no endpoint set for. A chain
/// pinned in the source but not yet reachable from here still appears — with its
/// pins and an unread status — because a chain that vanishes reads as "we do not
/// deploy there", which is the opposite of "its rollout has not reached us yet".
pub struct ChainPin {
    pub network: String,
    pub authoriser: Option<String>,
    pub safe: Option<String>,
    pub rpc_host: Option<String>,
}

/// Read the chains from the generated deploy lib's authoriser-clone pins:
/// `STOX_PROD_AUTHORISER_V4_CLONE` is the home chain (Base) and each
/// `…_<CHAIN>` suffix is another, paired with that chain's
/// `STOX_TOKEN_OWNER_SAFE[_<CHAIN>]`. Derived for the same reason the grantees
/// are: the rollout this page reports on is adding chains, and a hand-listed
/// pair of chains would stop being the truth the moment one lands.
///
/// An unhydrated (all-zero) authoriser pin yields `None` — there is nothing to
/// ask a chain about — while the chain itself still appears.
pub fn parse_chain_pins(v4_lib: &str, safe_lib: &str) -> Vec<ChainPin> {
    // The `[^0-9a-fA-F]` tail is what keeps a bytes32 `…_CODEHASH` out: 64 hex
    // chars cannot end after 40.
    let Ok(re) = Regex::new(
        r"\bSTOX_PROD_AUTHORISER_V4_CLONE(_[A-Z0-9_]+)?\s*=\s*(?:address\(\s*)?(0x[0-9a-fA-F]{40})[^0-9a-fA-F]",
    ) else {
        return Vec::new();
    };
    let mut out: Vec<ChainPin> = Vec::new();
    for c in re.captures_iter(v4_lib) {
        let suffix = c.get(1).map(|m| m.as_str().to_string());
        let network = match &suffix {
            None => "base".to_string(),
            Some(s) => s.trim_start_matches('_').to_lowercase(),
        };
        if out.iter().any(|p| p.network == network) {
            continue;
        }
        let authoriser = Some(c[2].to_string()).filter(|a| !is_unhydrated(Some(a)));
        let safe_const = match &suffix {
            None => "STOX_TOKEN_OWNER_SAFE".to_string(),
            Some(s) => format!("STOX_TOKEN_OWNER_SAFE{s}"),
        };
        out.push(ChainPin {
            network,
            authoriser,
            safe: parse_address_constant(safe_lib, &safe_const),
            rpc_host: None,
        });
    }
    out
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

/// Assemble the authoriser role-grant document: every `(role, grantee)` pair the
/// deploy repo pins, grouped by grantee, checked per chain.
///
/// `check(network, authoriser, role, grantee_address)` is the on-chain
/// `hasRole` probe, injected so the assembly is testable without a network.
/// It is only called where there IS something to ask — a chain with an
/// authoriser pin, an endpoint, and a grantee whose address resolved.
///
/// `None` when the map cannot be read, so the page shows nothing rather than an
/// empty table that would read as "no key holds these roles".
pub fn build_grants(
    org: &str,
    repo: &str,
    src: &OwnerSources,
    chains: &[ChainPin],
    check: &dyn Fn(&str, &str, &str, &str) -> GrantOnChain,
) -> Option<serde_json::Value> {
    let map = parse_expected_grants(src.auth_lib)?;
    // Grantee identifiers in first-appearance order — the order the map is
    // written in, so the page's order is the source's order.
    let mut idents: Vec<&str> = Vec::new();
    for g in &map.grants {
        if !idents.contains(&g.grantee.as_str()) {
            idents.push(&g.grantee);
        }
    }
    // Per-chain tallies, accumulated as the rows are built so the banner counts
    // the rows the page actually shows rather than a separately-derived figure.
    let mut tally: Vec<(usize, usize, usize)> = vec![(0, 0, 0); chains.len()];

    let mut grantees: Vec<serde_json::Value> = Vec::new();
    for ident in &idents {
        let is_safe = *ident == map.safe_param;
        // A constant grantee is one address on every chain; the Safe slot is the
        // chain's own Safe. Both are read from the source, neither is typed here.
        let fixed = if is_safe {
            None
        } else {
            resolve_ident(src, ident)
        };
        let roles: Vec<&str> = {
            let mut seen: Vec<&str> = Vec::new();
            for g in map.grants.iter().filter(|g| g.grantee == **ident) {
                if !seen.contains(&g.role.as_str()) {
                    seen.push(&g.role);
                }
            }
            seen
        };
        let mut role_rows: Vec<serde_json::Value> = Vec::new();
        for role in &roles {
            let mut per_chain: Vec<serde_json::Value> = Vec::new();
            for (i, chain) in chains.iter().enumerate() {
                let address = if is_safe {
                    chain.safe.clone()
                } else {
                    fixed.clone()
                };
                let status = match (&chain.authoriser, &chain.rpc_host, &address) {
                    (Some(auth), Some(_), Some(a)) => check(&chain.network, auth, role, a),
                    _ => GrantOnChain::Unknown,
                };
                match status {
                    GrantOnChain::Granted => tally[i].0 += 1,
                    GrantOnChain::NotGranted => tally[i].1 += 1,
                    GrantOnChain::Unknown => tally[i].2 += 1,
                }
                per_chain.push(json!({
                    "network": chain.network,
                    "address": address,
                    "status": status.token(),
                }));
            }
            role_rows.push(json!({
                "role": role,
                "admin": is_admin_role(role),
                "chains": per_chain,
            }));
        }
        grantees.push(json!({
            // The constant's own name, verbatim — it greps straight back to the
            // line in the deploy repo that put this key on the page.
            "ident": ident,
            "kind": if is_safe { "safe" } else { "constant" },
            // null for the Safe: its address is per chain, and each row carries
            // the one it was checked against.
            "address": fixed,
            "roles": role_rows,
        }));
    }

    let chain_docs: Vec<serde_json::Value> = chains
        .iter()
        .zip(&tally)
        .map(|(c, (granted, missing, unknown))| {
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
            })
        })
        .collect();

    Some(json!({
        "org": org,
        "repo": repo,
        "source": "src/lib/LibAuthoriserInvariants.sol",
        "function": "expectedGrants(address)",
        "pinnedCount": map.grants.len(),
        "declaredCount": map.declared,
        "chains": chain_docs,
        "grantees": grantees,
    }))
}

/// Resolve a grantee identifier to its address literal across the deploy repo's
/// libraries, following at most one re-export hop (`X = OtherLib.Y;`). A grantee
/// declared in one lib and aliased into the grant map's lib still resolves, so
/// where the constant lives is not a thing this page depends on.
fn resolve_ident(src: &OwnerSources, ident: &str) -> Option<String> {
    let sources = [src.auth_lib, src.safe_lib, src.v4_lib, src.overrides];
    if let Some(a) = sources
        .iter()
        .find_map(|s| parse_address_constant(s, ident))
    {
        return Some(a);
    }
    let alias = Regex::new(&format!(
        r"\b{}\b\s*=\s*(?:[A-Za-z_][A-Za-z0-9_]*\.)?([A-Za-z_][A-Za-z0-9_]*)\s*;",
        regex::escape(ident)
    ))
    .ok()?;
    let target = sources
        .iter()
        .find_map(|s| alias.captures(s).and_then(|c| c.get(1)))
        .map(|m| m.as_str().to_string())?;
    sources
        .iter()
        .find_map(|s| parse_address_constant(s, &target))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative fragments in the three RHS forms the real repo uses.
    const SAFE_LIB: &str = r#"
        library LibSafeInvariants {
            address internal constant STOX_TOKEN_OWNER_SAFE = 0xe70d821f3462a074e63b42d0AaC6523faAe1d611;
            uint256 internal constant STOX_TOKEN_OWNER_SAFE_THRESHOLD = 3;
            address internal constant STOX_TOKEN_OWNER_SAFE_ETHEREUM = 0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_1 = 0x4746095B1Ea1A84446d34448f44e74D3d51f92F2;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_2 = 0xceC2cb8B8EE4000FFA3F8a7f8E0Fa0A3E3DAb72d;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_3 = 0x8D5901d8aE48101B59400235ad8614A2e0510466;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_4 = 0xC1C89b7f5448F447d59f920456A9610f6b2544bC;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_5 = 0xAB92b327c97A6E7461cBd76E2a789E5e106FF87e;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_6 = 0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2;
            address internal constant SAFE_MODULES_SENTINEL = address(0x1);
        }
    "#;
    const AUTH_LIB: &str = r#"
        address internal constant STOX_PROD_AUTHORISER = 0x35f9fA9d80aAF2B0fB27f0FF015641B3408d7456;
        address internal constant STOX_PROD_AUTHORISER_IMPL = 0x2B4A510c3619d5E888095BFE9f95902D32dA5556;
        address internal constant GRANTEE_SERVICE_1C66 = 0x1c66D6708914C40239D54919320b4C48cAE3D1A9;
        bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);
    "#;
    const V4_LIB: &str = r#"
        address constant BEACON_INITIAL_OWNER = address(0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b);
        address constant STOX_PROD_AUTHORISER_V4_CLONE = address(0x315b16faa6eE413faBCa877d3851B3818369f0cD);
    "#;
    const OVERRIDES: &str = r#"
        address constant RECEIPT_BEACON_OWNER = address(0xbAB0E6b7B5dDA86FB8ba81c00aEA0Ceb8b73686b);
        address constant VAULT_BEACON_OWNER = address(0xc95dB340A7a100881626475d41BFf70857Aa920D);
    "#;

    #[test]
    fn parses_bare_hex_form() {
        assert_eq!(
            parse_address_constant(SAFE_LIB, "STOX_TOKEN_OWNER_SAFE"),
            Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611".to_string())
        );
    }

    #[test]
    fn parses_address_wrapped_form() {
        assert_eq!(
            parse_address_constant(V4_LIB, "BEACON_INITIAL_OWNER"),
            Some("0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b".to_string())
        );
    }

    #[test]
    fn checksum_casing_is_preserved_verbatim() {
        // The address is emitted for explorer links + eyeballing, so its EIP-55
        // casing must survive parsing unchanged.
        let a = parse_address_constant(AUTH_LIB, "STOX_PROD_AUTHORISER").unwrap();
        assert_eq!(a, "0x35f9fA9d80aAF2B0fB27f0FF015641B3408d7456");
    }

    #[test]
    fn prefix_name_does_not_match_a_longer_sibling() {
        // STOX_TOKEN_OWNER_SAFE must return the Safe, NOT the _ETHEREUM / _OWNER_n
        // / _THRESHOLD value that shares its prefix.
        assert_eq!(
            parse_address_constant(SAFE_LIB, "STOX_TOKEN_OWNER_SAFE"),
            Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611".to_string())
        );
        // And the sibling resolves to its own distinct value.
        assert_eq!(
            parse_address_constant(SAFE_LIB, "STOX_TOKEN_OWNER_SAFE_ETHEREUM"),
            Some("0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329".to_string())
        );
    }

    #[test]
    fn missing_constant_is_none() {
        assert_eq!(parse_address_constant(SAFE_LIB, "NOPE_NOT_HERE"), None);
    }

    #[test]
    fn threshold_parses_as_uint() {
        assert_eq!(
            parse_uint_constant(SAFE_LIB, "STOX_TOKEN_OWNER_SAFE_THRESHOLD"),
            Some(3)
        );
    }

    #[test]
    fn build_owners_assembles_all_groups() {
        let v = build_owners(
            "S01-Issuer",
            "st0x.deploy",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .expect("anchor resolves");
        assert_eq!(v["repo"], "st0x.deploy");
        assert_eq!(v["threshold"], 3);
        assert_eq!(v["signerCount"], 6);
        let groups = v["groups"].as_array().unwrap();
        let ids: Vec<&str> = groups.iter().map(|g| g["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["safe", "signers", "authoriser", "historical"]);
        // Base Safe address surfaced in the safe group.
        assert_eq!(
            groups[0]["entries"][0]["address"],
            "0xe70d821f3462a074e63b42d0AaC6523faAe1d611"
        );
        // All six signers.
        assert_eq!(groups[1]["entries"].as_array().unwrap().len(), 6);
        // Bricked V2 owners carry the bricked status.
        assert_eq!(groups[3]["entries"][1]["status"], "bricked");
        // Without a live authorizer() reading no clone claims to be active.
        let auth = groups[2]["entries"].as_array().unwrap();
        let v4 = auth
            .iter()
            .find(|e| e["role"] == "V4 authoriser clone" && e["network"] == "base")
            .unwrap();
        assert_eq!(v4["status"], "unknown");
    }

    /// The whole point of reading `authorizer()`: whichever clone the vaults
    /// actually delegate to is the active one, and the other is labelled by
    /// which side of the swap it sits on. A hardcoded pair gets this backwards
    /// the moment the migration lands.
    #[test]
    fn authoriser_status_follows_the_live_reading() {
        let v3 = Some("0x35f9fA9d80aAF2B0fB27f0FF015641B3408d7456".to_string());
        let v4 = Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD".to_string());
        // Pre-swap: vaults still point at the V3-era clone.
        assert_eq!(
            authoriser_status(
                v3.as_ref(),
                Some("0x35f9fA9d80aAF2B0fB27f0FF015641B3408d7456"),
                "migrated"
            ),
            "active"
        );
        assert_eq!(
            authoriser_status(
                v4.as_ref(),
                Some("0x35f9fA9d80aAF2B0fB27f0FF015641B3408d7456"),
                "pending"
            ),
            "pending"
        );
        // Post-swap: the same two pins swap roles with no code change.
        assert_eq!(
            authoriser_status(
                v3.as_ref(),
                Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD"),
                "migrated"
            ),
            "migrated"
        );
        assert_eq!(
            authoriser_status(
                v4.as_ref(),
                Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD"),
                "active"
            ),
            "active"
        );
    }

    /// Checksum casing differs between the Solidity constant and an RPC reply,
    /// and a case-sensitive compare would report the live clone as superseded.
    #[test]
    fn authoriser_status_ignores_address_casing() {
        let pin = Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD".to_string());
        assert_eq!(
            authoriser_status(
                pin.as_ref(),
                Some("0x315b16faa6ee413fabca877d3851b3818369f0cd"),
                "pending"
            ),
            "active"
        );
    }

    /// A failed RPC must never let a stale literal stand in for a live answer.
    #[test]
    fn authoriser_status_is_unknown_without_a_live_reading() {
        let pin = Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD".to_string());
        assert_eq!(authoriser_status(pin.as_ref(), None, "pending"), "unknown");
        assert_eq!(
            authoriser_status(None, Some("0x315b16"), "pending"),
            "unknown"
        );
    }

    /// Ethereum's clone is nonce-deployed, so its pin sits at address(0) until
    /// the bootstrap runs. That is "declared, not deployed" — not "no such
    /// chain", which is what omitting the row would say.
    #[test]
    fn ethereum_authoriser_row_is_rendered_before_the_clone_exists() {
        let v4_zero = format!("{V4_LIB}\n    address constant STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM = address(0x0000000000000000000000000000000000000000);\n");
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: &v4_zero,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .unwrap();
        let auth = v["groups"][2]["entries"].as_array().unwrap();
        let eth = auth
            .iter()
            .find(|e| e["role"] == "V4 authoriser clone" && e["network"] == "ethereum")
            .expect("the ethereum row must exist even unhydrated");
        assert_eq!(eth["status"], "pending");
        let note = eth["note"].as_str().unwrap();
        assert!(
            note.contains("not yet hydrated"),
            "an unhydrated pin must say so: {note}"
        );
        // The scan reads the PIN. With no address it has nothing to look up, so
        // it must not report on the clone's existence either way — the Ethereum
        // clone was live for some time before this pin caught up.
        assert!(
            !note.contains("not yet deployed"),
            "must not claim anything about deployment: {note}"
        );
    }

    #[test]
    fn build_owners_is_none_without_the_anchor() {
        // Repo unreachable / anchor constant moved → no owners doc at all.
        assert_eq!(
            build_owners(
                "o",
                "r",
                &OwnerSources {
                    safe_lib: "",
                    auth_lib: AUTH_LIB,
                    v4_lib: V4_LIB,
                    overrides: OVERRIDES
                },
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn unresolved_entry_becomes_null_not_dropped() {
        // The Ethereum Safe missing from the source surfaces as a null address in
        // its row rather than vanishing.
        let safe_no_eth = "address internal constant STOX_TOKEN_OWNER_SAFE = 0xe70d821f3462a074e63b42d0AaC6523faAe1d611;";
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: safe_no_eth,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .unwrap();
        let eth = &v["groups"][0]["entries"][1];
        assert_eq!(eth["role"], "Ethereum Safe");
        assert!(eth["address"].is_null());
    }

    #[test]
    fn signer_count_is_read_from_the_constants_not_a_fixed_six() {
        // Three signer constants -> three signers read: proves the roster size
        // comes from the source, not a hardcoded 6. Threshold absent -> 3.
        let three = "
            address internal constant STOX_TOKEN_OWNER_SAFE = 0xe70d821f3462a074e63b42d0AaC6523faAe1d611;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_1 = 0x1111111111111111111111111111111111111111;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_2 = 0x2222222222222222222222222222222222222222;
            address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_3 = 0x3333333333333333333333333333333333333333;
        ";
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: three,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(v["signerCount"], 3);
        assert_eq!(v["groups"][1]["entries"].as_array().unwrap().len(), 3);
        assert_eq!(v["groups"][1]["title"], "Safe signers (3-of-3)");
    }

    // ---- declared-vs-actual verification ----

    fn onchain(owners: Option<Vec<&str>>, threshold: Option<u64>) -> OnChainSafe {
        OnChainSafe {
            network: "base".into(),
            safe: "0xe70d821f3462a074e63b42d0AaC6523faAe1d611".into(),
            rpc_host: "mainnet.base.org".into(),
            owners: owners.map(|v| v.into_iter().map(str::to_string).collect()),
            threshold,
        }
    }

    // The SAFE_LIB roster, lowercased as getOwners returns it.
    const LIVE_ROSTER: [&str; 6] = [
        "0x4746095b1ea1a84446d34448f44e74d3d51f92f2",
        "0xcec2cb8b8ee4000ffa3f8a7f8e0fa0a3e3dab72d",
        "0x8d5901d8ae48101b59400235ad8614a2e0510466",
        "0xc1c89b7f5448f447d59f920456a9610f6b2544bc",
        "0xab92b327c97a6e7461cbd76e2a789e5e106ff87e",
        "0x5ccd3ce683b66ff271ddb8915ff528b8fcfa23c2",
    ];

    #[test]
    fn onchain_match_marks_every_signer_verified() {
        let oc = onchain(Some(LIVE_ROSTER.to_vec()), Some(3));
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            Some(&oc),
            None,
        )
        .unwrap();
        let sg = &v["groups"][1];
        assert_eq!(sg["verification"]["match"], true);
        assert_eq!(sg["verification"]["reachable"], true);
        assert_eq!(sg["verification"]["threshold"]["match"], true);
        let entries = sg["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 6);
        assert!(entries.iter().all(|e| e["onChain"] == "match"));
    }

    #[test]
    fn onchain_drift_flags_missing_and_extra_owners() {
        // Live set drops signer 6 and adds an owner not in the constants; on-chain
        // threshold (2) also differs from the declared 3.
        let mut live: Vec<&str> = LIVE_ROSTER[..5].to_vec();
        live.push("0xdeadbeef00000000000000000000000000000001");
        let oc = onchain(Some(live), Some(2));
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            Some(&oc),
            None,
        )
        .unwrap();
        let sg = &v["groups"][1];
        assert_eq!(sg["verification"]["match"], false);
        assert_eq!(sg["verification"]["threshold"]["match"], false);
        let entries = sg["entries"].as_array().unwrap();
        let s6 = entries.iter().find(|e| e["role"] == "Signer 6").unwrap();
        assert_eq!(s6["onChain"], "missing", "declared but absent on-chain");
        let extra = entries.iter().find(|e| e["status"] == "extra").unwrap();
        assert_eq!(
            extra["address"],
            "0xdeadbeef00000000000000000000000000000001"
        );
        assert_eq!(extra["onChain"], "extra");
    }

    #[test]
    fn unreachable_rpc_leaves_signers_unverified() {
        let oc = onchain(None, None); // RPC failed
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            Some(&oc),
            None,
        )
        .unwrap();
        let sg = &v["groups"][1];
        assert_eq!(sg["verification"]["reachable"], false);
        assert!(sg["verification"]["match"].is_null());
        let entries = sg["entries"].as_array().unwrap();
        assert!(entries.iter().all(|e| e["onChain"] == "unverified"));
        assert_eq!(entries.len(), 6, "still shows the declared roster");
    }

    #[test]
    fn no_onchain_omits_verification() {
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .unwrap();
        assert!(v["groups"][1]["verification"].is_null());
    }

    #[test]
    fn threshold_mismatch_alone_fails_the_verdict() {
        // Roster fully matches but the on-chain threshold differs: the overall
        // verdict must be false — never a green "verified" — even though every
        // signer row is a match.
        let oc = onchain(Some(LIVE_ROSTER.to_vec()), Some(4));
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            Some(&oc),
            None,
        )
        .unwrap();
        let sg = &v["groups"][1];
        assert_eq!(sg["verification"]["reachable"], true);
        assert_eq!(
            sg["verification"]["match"], false,
            "threshold drift fails the verdict"
        );
        assert_eq!(sg["verification"]["threshold"]["match"], false);
        let entries = sg["entries"].as_array().unwrap();
        assert!(
            entries.iter().all(|e| e["onChain"] == "match"),
            "roster itself is fine"
        );
    }

    #[test]
    fn partial_rpc_is_not_reachable() {
        // Owners answered but the threshold call didn't: not reachable and no
        // verdict, so the page shows "incomplete" rather than a green banner.
        let oc = onchain(Some(LIVE_ROSTER.to_vec()), None);
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            Some(&oc),
            None,
        )
        .unwrap();
        let sg = &v["groups"][1];
        assert_eq!(
            sg["verification"]["reachable"], false,
            "one call missing => not reachable"
        );
        assert!(sg["verification"]["match"].is_null());
        assert!(sg["verification"]["threshold"]["onChain"].is_null());
    }

    // ---- authoriser role grants (#143) ----

    /// The shape the real `LibAuthoriserInvariants` carries: a Safe-parametric
    /// map with `_ADMIN` roles on the chain's Safe, action roles on a service
    /// constant, and the same action roles held directly by the Safe.
    const GRANT_LIB: &str = r#"
        library LibAuthoriserInvariants {
            address internal constant GRANTEE_TOKEN_OWNER_SAFE = LibSafeInvariants.STOX_TOKEN_OWNER_SAFE;
            address internal constant GRANTEE_SERVICE_1C66 = 0x1c66D6708914C40239D54919320b4C48cAE3D1A9;
            bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);

            function expectedGrants() internal pure returns (RoleGrant[] memory grants) {
                grants = expectedGrants(GRANTEE_TOKEN_OWNER_SAFE);
            }

            function expectedGrants(address tokenOwnerSafe) internal pure returns (RoleGrant[] memory grants) {
                grants = new RoleGrant[](7);
                grants[0] = RoleGrant(keccak256("DEPOSIT_ADMIN"), tokenOwnerSafe);
                grants[1] = RoleGrant(keccak256("WITHDRAW_ADMIN"), tokenOwnerSafe);
                grants[2] = RoleGrant(keccak256("DEPOSIT"), GRANTEE_SERVICE_1C66);
                grants[3] = RoleGrant(keccak256("WITHDRAW"), GRANTEE_SERVICE_1C66);
                grants[4] = RoleGrant(keccak256("CERTIFY"), GRANTEE_SERVICE_1C66);
                grants[5] = RoleGrant(keccak256("DEPOSIT"), tokenOwnerSafe);
                grants[6] = RoleGrant(keccak256("WITHDRAW"), tokenOwnerSafe);
            }
        }
    "#;

    const V4_CHAINS: &str = r#"
        address constant STOX_PROD_AUTHORISER_V4_CLONE = address(0x315b16faa6eE413faBCa877d3851B3818369f0cD);
        bytes32 constant STOX_PROD_AUTHORISER_V4_CLONE_CODEHASH = 0x9d1c1a4b2f8f0e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a3928170615;
        address constant STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM = address(0x66566cc91dEAf818859bD4b09B7903ac48998157);
    "#;

    fn grant_sources<'a>(auth: &'a str) -> OwnerSources<'a> {
        OwnerSources {
            safe_lib: SAFE_LIB,
            auth_lib: auth,
            v4_lib: V4_LIB,
            overrides: OVERRIDES,
        }
    }

    /// Two chains with everything pinned, so a test only has to vary the probe.
    fn two_chains() -> Vec<ChainPin> {
        vec![
            ChainPin {
                network: "base".into(),
                authoriser: Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD".into()),
                safe: Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611".into()),
                rpc_host: Some("mainnet.base.org".into()),
            },
            ChainPin {
                network: "ethereum".into(),
                authoriser: Some("0x66566cc91dEAf818859bD4b09B7903ac48998157".into()),
                safe: Some("0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329".into()),
                rpc_host: Some("ethereum-rpc.publicnode.com".into()),
            },
        ]
    }

    fn all_granted(_n: &str, _a: &str, _r: &str, _g: &str) -> GrantOnChain {
        GrantOnChain::Granted
    }

    /// Find a grantee doc by the constant name it was derived from.
    fn grantee<'a>(v: &'a serde_json::Value, ident: &str) -> Option<&'a serde_json::Value> {
        v["grantees"]
            .as_array()?
            .iter()
            .find(|g| g["ident"] == ident)
    }

    /// The status of one grantee's role on one chain.
    fn status(v: &serde_json::Value, ident: &str, role: &str, network: &str) -> String {
        grantee(v, ident)
            .and_then(|g| g["roles"].as_array())
            .and_then(|rs| rs.iter().find(|r| r["role"] == role))
            .and_then(|r| r["chains"].as_array())
            .and_then(|cs| cs.iter().find(|c| c["network"] == network))
            .and_then(|c| c["status"].as_str())
            .unwrap_or("<absent>")
            .to_string()
    }

    /// The owners view must carry no hand-written grantee row. It used to: one
    /// entry naming one service constant, which could only ever describe the key
    /// somebody typed. Grantees come from the map now, and a row typed back in
    /// here would be a second list to forget to update.
    #[test]
    fn the_authoriser_group_carries_no_hand_written_grantee_row() {
        let v = build_owners(
            "o",
            "r",
            &OwnerSources {
                safe_lib: SAFE_LIB,
                auth_lib: AUTH_LIB,
                v4_lib: V4_LIB,
                overrides: OVERRIDES,
            },
            None,
            None,
        )
        .unwrap();
        let auth = v["groups"][2]["entries"].as_array().unwrap();
        assert!(
            !auth
                .iter()
                .any(|e| e["address"] == "0x1c66D6708914C40239D54919320b4C48cAE3D1A9"),
            "a grantee address must not be pinned by hand in the owners groups"
        );
        assert!(
            auth.iter().all(|e| e["role"] != "Service grantee"),
            "the hand-written grantee row is gone"
        );
    }

    #[test]
    fn parses_the_pinned_grant_map_in_source_order() {
        let m = parse_expected_grants(GRANT_LIB).expect("the parametric overload parses");
        assert_eq!(m.safe_param, "tokenOwnerSafe");
        assert_eq!(m.declared, Some(7));
        assert_eq!(m.grants.len(), 7);
        assert_eq!(
            m.grants[2],
            GrantPin {
                role: "DEPOSIT".into(),
                grantee: "GRANTEE_SERVICE_1C66".into()
            }
        );
        // The no-arg overload delegates; parsing IT would yield one bogus pair.
        assert!(m
            .grants
            .iter()
            .all(|g| g.role != "GRANTEE_TOKEN_OWNER_SAFE"));
    }

    /// The check the issue names: a service EOA added to `expectedGrants()`
    /// reaches the page with NO change to this repo. The fixture below differs
    /// from `GRANT_LIB` only by the lines a future PR would add to the deploy
    /// repo — if this passes, the grantee list is derived, not transcribed.
    #[test]
    fn a_third_service_eoa_appears_with_no_dashboard_change() {
        let before = build_grants(
            "o",
            "r",
            &grant_sources(GRANT_LIB),
            &two_chains(),
            &all_granted,
        )
        .expect("map parses");
        assert!(
            grantee(&before, "GRANTEE_SERVICE_3D0C").is_none(),
            "not in the map yet"
        );
        let after_lib = GRANT_LIB
            .replace(
                "bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);",
                "address internal constant GRANTEE_SERVICE_3D0C = 0x3d0CD66EFA66c05d86c3d4316B03eAE87ab9E8aE;\n\
                 bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);",
            )
            .replace(
                "grants = new RoleGrant[](7);",
                "grants = new RoleGrant[](9);",
            )
            .replace(
                r#"grants[6] = RoleGrant(keccak256("WITHDRAW"), tokenOwnerSafe);"#,
                "grants[6] = RoleGrant(keccak256(\"WITHDRAW\"), tokenOwnerSafe);\n\
                 grants[7] = RoleGrant(keccak256(\"DEPOSIT\"), GRANTEE_SERVICE_3D0C);\n\
                 grants[8] = RoleGrant(keccak256(\"CERTIFY\"), GRANTEE_SERVICE_3D0C);",
            );
        let after = build_grants(
            "o",
            "r",
            &grant_sources(&after_lib),
            &two_chains(),
            &all_granted,
        )
        .expect("map parses");
        let g = grantee(&after, "GRANTEE_SERVICE_3D0C").expect("the new EOA is listed");
        assert_eq!(g["address"], "0x3d0CD66EFA66c05d86c3d4316B03eAE87ab9E8aE");
        assert_eq!(g["kind"], "constant");
        let roles: Vec<&str> = g["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["DEPOSIT", "CERTIFY"], "its roles, not a fixed trio");
        assert_eq!(
            status(&after, "GRANTEE_SERVICE_3D0C", "DEPOSIT", "base"),
            "granted"
        );
        assert_eq!(after["pinnedCount"], 9);
        // …and revoking one removes it, which is the same property read backwards.
        let revoked = GRANT_LIB.replace(
            r#"grants[4] = RoleGrant(keccak256("CERTIFY"), GRANTEE_SERVICE_1C66);"#,
            "",
        );
        let after_revoke = build_grants(
            "o",
            "r",
            &grant_sources(&revoked),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        assert_eq!(
            status(&after_revoke, "GRANTEE_SERVICE_1C66", "CERTIFY", "base"),
            "<absent>",
            "a pair dropped from the map is gone from the page"
        );
        assert_eq!(
            status(&after_revoke, "GRANTEE_SERVICE_1C66", "DEPOSIT", "base"),
            "granted",
            "its other roles are untouched"
        );
    }

    #[test]
    fn the_safe_slot_resolves_to_each_chains_own_safe() {
        let v = build_grants(
            "o",
            "r",
            &grant_sources(GRANT_LIB),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        let safe = grantee(&v, "tokenOwnerSafe").expect("the Safe grantee is listed");
        assert_eq!(safe["kind"], "safe");
        assert!(
            safe["address"].is_null(),
            "one address would be wrong on one of the chains"
        );
        let deposit = safe["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "DEPOSIT")
            .unwrap();
        let by_net = |n: &str| {
            deposit["chains"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["network"] == n)
                .unwrap()["address"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(by_net("base"), "0xe70d821f3462a074e63b42d0AaC6523faAe1d611");
        assert_eq!(
            by_net("ethereum"),
            "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329"
        );
    }

    /// The Safe holds `_ADMIN` roles AND the action roles, so admin-vs-action is
    /// a property of the ROLE, not a partition of the principals. Splitting the
    /// page on it at the top level would have to file the Safe twice or lie.
    #[test]
    fn admin_and_action_split_by_role_name_and_the_safe_is_in_both() {
        let v = build_grants(
            "o",
            "r",
            &grant_sources(GRANT_LIB),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        let safe = grantee(&v, "tokenOwnerSafe").unwrap();
        let flags: Vec<(&str, bool)> = safe["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["role"].as_str().unwrap(), r["admin"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            flags,
            [
                ("DEPOSIT_ADMIN", true),
                ("WITHDRAW_ADMIN", true),
                ("DEPOSIT", false),
                ("WITHDRAW", false)
            ]
        );
        let svc = grantee(&v, "GRANTEE_SERVICE_1C66").unwrap();
        assert!(
            svc["roles"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["admin"] == false),
            "a service EOA holds no admin role"
        );
        assert!(is_admin_role("CANCEL_CORPORATE_ACTION_ADMIN"));
        assert!(!is_admin_role("CERTIFY"));
    }

    /// Per-chain status is the point of the section: the same key is granted on
    /// one chain and not the other, and that reads as a rollout, not a fault.
    #[test]
    fn per_chain_status_reports_a_rollout_in_progress() {
        let base_only = |network: &str, _a: &str, _r: &str, _g: &str| {
            if network == "base" {
                GrantOnChain::Granted
            } else {
                GrantOnChain::NotGranted
            }
        };
        let v = build_grants(
            "o",
            "r",
            &grant_sources(GRANT_LIB),
            &two_chains(),
            &base_only,
        )
        .unwrap();
        assert_eq!(
            status(&v, "GRANTEE_SERVICE_1C66", "DEPOSIT", "base"),
            "granted"
        );
        assert_eq!(
            status(&v, "GRANTEE_SERVICE_1C66", "DEPOSIT", "ethereum"),
            "missing"
        );
        let chains = v["chains"].as_array().unwrap();
        let base = chains.iter().find(|c| c["network"] == "base").unwrap();
        let eth = chains.iter().find(|c| c["network"] == "ethereum").unwrap();
        assert_eq!(base["state"], "live");
        assert_eq!(base["granted"], 7);
        assert_eq!(base["total"], 7);
        assert_eq!(
            eth["state"], "unprovisioned",
            "a chain whose bundle has not run is a rollout state, not a drift verdict"
        );
        assert_eq!(eth["granted"], 0);
        assert_eq!(eth["missing"], 7);
    }

    /// A failed probe is never a revoked grant. The two are opposite readings of
    /// the same pixel, and only one of them is a reason to page somebody.
    #[test]
    fn an_unreadable_chain_is_unknown_not_missing() {
        let mut chains = two_chains();
        chains[1].rpc_host = None; // no endpoint for this chain yet
        let v = build_grants("o", "r", &grant_sources(GRANT_LIB), &chains, &all_granted).unwrap();
        assert_eq!(
            status(&v, "GRANTEE_SERVICE_1C66", "DEPOSIT", "ethereum"),
            "unknown"
        );
        let eth = v["chains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["network"] == "ethereum")
            .unwrap();
        assert_eq!(eth["state"], "unknown");
        assert_eq!(eth["missing"], 0, "unread is not absent");
        assert!(eth["rpcHost"].is_null(), "and the reason is on the record");

        // Same again for a chain with no authoriser pin to ask.
        let mut unpinned = two_chains();
        unpinned[1].authoriser = None;
        let v2 =
            build_grants("o", "r", &grant_sources(GRANT_LIB), &unpinned, &all_granted).unwrap();
        assert_eq!(
            status(&v2, "GRANTEE_SERVICE_1C66", "CERTIFY", "ethereum"),
            "unknown"
        );
        let failing = |_n: &str, _a: &str, _r: &str, _g: &str| GrantOnChain::Unknown;
        let v3 =
            build_grants("o", "r", &grant_sources(GRANT_LIB), &two_chains(), &failing).unwrap();
        assert_eq!(
            status(&v3, "GRANTEE_SERVICE_1C66", "CERTIFY", "base"),
            "unknown"
        );
        assert_eq!(v3["chains"][0]["state"], "unknown");
    }

    #[test]
    fn chains_are_read_from_the_clone_pins_not_a_fixed_pair() {
        let pins = parse_chain_pins(V4_CHAINS, SAFE_LIB);
        let nets: Vec<&str> = pins.iter().map(|p| p.network.as_str()).collect();
        assert_eq!(
            nets,
            ["base", "ethereum"],
            "no codehash pin leaks in as a chain"
        );
        assert_eq!(
            pins[0].authoriser.as_deref(),
            Some("0x315b16faa6eE413faBCa877d3851B3818369f0cD")
        );
        assert_eq!(
            pins[0].safe.as_deref(),
            Some("0xe70d821f3462a074e63b42d0AaC6523faAe1d611")
        );
        assert_eq!(
            pins[1].safe.as_deref(),
            Some("0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329"),
            "each chain takes its own Safe constant"
        );
        // A chain the deploy repo adds shows up on its own.
        let grown = format!(
            "{V4_CHAINS}\n address constant STOX_PROD_AUTHORISER_V4_CLONE_HYPEREVM = address(0x00000000219ab540356cBB839Cbe05303d7705Fa);\n"
        );
        let grown_pins = parse_chain_pins(&grown, SAFE_LIB);
        assert_eq!(grown_pins.len(), 3);
        assert_eq!(grown_pins[2].network, "hyperevm");
        assert!(
            grown_pins[2].safe.is_none(),
            "its Safe constant is not there yet, which is a gap to show, not a row to drop"
        );
    }

    /// An unhydrated clone pin gives nothing to query. The chain still appears —
    /// dropping it would read as "we do not deploy there".
    #[test]
    fn an_unhydrated_clone_pin_leaves_the_chain_unqueried() {
        let zeroed = V4_CHAINS.replace(
            "address(0x66566cc91dEAf818859bD4b09B7903ac48998157)",
            "address(0x0000000000000000000000000000000000000000)",
        );
        let pins = parse_chain_pins(&zeroed, SAFE_LIB);
        assert_eq!(pins.len(), 2, "the chain is still listed");
        assert_eq!(pins[1].network, "ethereum");
        assert!(pins[1].authoriser.is_none(), "nothing to ask");
    }

    #[test]
    fn an_unparsable_map_yields_no_document() {
        assert!(
            build_grants(
                "o",
                "r",
                &grant_sources("library L {}"),
                &two_chains(),
                &all_granted
            )
            .is_none(),
            "an empty table would read as 'no key holds these roles'"
        );
    }

    /// A grantee re-exported from another library still resolves, so which lib
    /// declares a constant is not something this page depends on.
    #[test]
    fn an_aliased_grantee_constant_resolves_through_the_re_export() {
        let aliased = GRANT_LIB.replace(
            "address internal constant GRANTEE_SERVICE_1C66 = 0x1c66D6708914C40239D54919320b4C48cAE3D1A9;",
            "address internal constant GRANTEE_SERVICE_1C66 = LibElsewhere.SERVICE_KEY;",
        );
        let mut src = grant_sources(&aliased);
        let elsewhere =
            "address internal constant SERVICE_KEY = 0x1c66D6708914C40239D54919320b4C48cAE3D1A9;";
        src.overrides = elsewhere;
        let v = build_grants("o", "r", &src, &two_chains(), &all_granted).unwrap();
        assert_eq!(
            grantee(&v, "GRANTEE_SERVICE_1C66").unwrap()["address"],
            "0x1c66D6708914C40239D54919320b4C48cAE3D1A9"
        );
    }

    /// A grantee whose constant cannot be resolved is shown with a null address
    /// and an unread status — never silently dropped, and never asked about
    /// under some other address.
    #[test]
    fn an_unresolvable_grantee_is_listed_without_an_address() {
        let dangling = GRANT_LIB.replace(
            "address internal constant GRANTEE_SERVICE_1C66 = 0x1c66D6708914C40239D54919320b4C48cAE3D1A9;",
            "",
        );
        let v = build_grants(
            "o",
            "r",
            &grant_sources(&dangling),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        let g = grantee(&v, "GRANTEE_SERVICE_1C66").expect("still listed");
        assert!(g["address"].is_null());
        assert_eq!(
            status(&v, "GRANTEE_SERVICE_1C66", "DEPOSIT", "base"),
            "unknown"
        );
    }

    /// The map's declared length travels with it, so a body that parsed short
    /// can be called out rather than reading as a smaller map.
    #[test]
    fn a_short_parse_is_visible_against_the_declared_length() {
        let v = build_grants(
            "o",
            "r",
            &grant_sources(GRANT_LIB),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        assert_eq!(v["pinnedCount"], 7);
        assert_eq!(v["declaredCount"], 7);
        let dropped = GRANT_LIB.replace(
            r#"grants[3] = RoleGrant(keccak256("WITHDRAW"), GRANTEE_SERVICE_1C66);"#,
            "grants[3] = someOtherThing();",
        );
        let v2 = build_grants(
            "o",
            "r",
            &grant_sources(&dropped),
            &two_chains(),
            &all_granted,
        )
        .unwrap();
        assert_eq!(v2["pinnedCount"], 6);
        assert_eq!(v2["declaredCount"], 7, "the shortfall is reportable");
    }
}
