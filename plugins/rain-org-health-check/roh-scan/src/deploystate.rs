//! The live state the deleted st0x.deploy scripts established
//! (rain-org-health#182), checked on every production chain: the token-owner
//! Safe, the V4 authoriser clone, the orchestrator instance, and the code hashes
//! of the frozen releases where `deploymentHealth` does not already check them.
//!
//! Those scripts will never run again (S01-Issuer/st0x.deploy#380), so a read
//! of each chain is the only ongoing evidence that their effects still hold.
//! Every expected value is read out of the deploy repo's own source, as the
//! owners and grants views read theirs. The exceptions are standard constants
//! the deploy repo does not declare (the ERC-1967 beacon slot, OpenZeppelin's
//! initializer slot, the ERC-1167 clone bytes, the Safe module sentinel) and the
//! deploy key, which no file in the deploy repo pins and the issue names.
//!
//! Parsing and the per-chain verdicts are pure and unit-tested here. The chain
//! is reached through [`ChainReads`], which main.rs implements over JSON-RPC.
//!
//! Every check is one row: `check` (what is asked), `match` (how `actual` is
//! compared to `expected`: `equal`, `anyOf`, `set` or `nonzero`), `expected`,
//! `actual`, and `status` (`pass`, `fail` or `unknown`). `unknown` means the
//! question could not be asked or answered. It is never a stand-in for `fail`.

use crate::deployhealth::parse_bytes32_constant;
use crate::owners::{
    parse_address_constant, parse_expected_grants, parse_uint_constant, resolve_ident, ChainPin,
    OwnerSources,
};
use crate::rpc::{keccak256_hex, role_id, word_address};
use alloy_primitives::hex;
use regex::Regex;
use serde_json::{json, Value};

/// The CI deploy key that ran the clone ceremonies. No file in st0x.deploy pins
/// it, so it is written here, as rain-org-health#182 names it.
pub const DEPLOY_KEY: &str = "0xE8c6eDE25f0E7fAfE8fBc34770FaBa27d56c0E76";

/// ERC-1967's beacon slot, `keccak256("eip1967.proxy.beacon") - 1`.
pub const ERC1967_BEACON_SLOT: &str =
    "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";

/// OpenZeppelin v5 `Initializable`'s ERC-7201 slot for
/// `openzeppelin.storage.Initializable`. Non-zero once `initialize` has run.
pub const INITIALIZABLE_SLOT: &str =
    "0xf0c57e16840df040f15088dc2f81fe391c3923bec73e23a9662efc9c229c6a00";

/// The ERC-1167 minimal-proxy runtime is these bytes around the 20-byte impl.
const ERC1167_PREFIX: &str = "363d3d373d3d3d363d73";
const ERC1167_SUFFIX: &str = "5af43d82803e903d91602b57fd5bf3";

/// Safe v1.4.1's `SENTINEL_MODULES`, the head of its module list. The deploy
/// repo writes it as `address(0x1)`, which an address-literal parse cannot read.
const SAFE_MODULES_SENTINEL: &str = "0x0000000000000000000000000000000000000001";
/// The page size the deploy repo's own module check asks for. One module on the
/// first page is already drift, so there is nothing to walk past it.
const SAFE_MODULES_PAGE: u64 = 10;
/// The Safe singleton pointer is storage slot 0.
const SAFE_SINGLETON_SLOT: &str = "0x0";

/// The roles the operator key must hold on the orchestrator (#182). The deploy
/// repo has no map of orchestrator grants: the script that made them is deleted.
const OPERATOR_ROLES: [&str; 2] = ["MINT", "BURN"];

/// OpenZeppelin `AccessControl`'s `DEFAULT_ADMIN_ROLE`, which is `bytes32(0)`
/// rather than the hash of a name.
pub const DEFAULT_ADMIN: &str = "DEFAULT_ADMIN";

/// The role id a role name is checked under.
pub fn role_word(name: &str) -> [u8; 32] {
    if name == DEFAULT_ADMIN {
        [0u8; 32]
    } else {
        role_id(name)
    }
}

/// What the scan can ask one chain. Every method answers `None` when the read
/// failed or could not be made, so an unanswered question stays apart from a
/// `false`.
pub trait ChainReads {
    /// Runtime code at `address`: `Some("0x")` when there is none.
    fn code(&self, address: &str) -> Option<String>;
    /// A raw storage word of `address`.
    fn storage(&self, address: &str, slot: &str) -> Option<[u8; 32]>;
    /// `hasRole(role, account)` on `contract`. A revert is `None`: a contract
    /// that does not answer has said nothing about the grant.
    fn has_role(&self, contract: &str, role: [u8; 32], account: &str) -> Option<bool>;
    /// A Safe's `getOwners()`, lowercase.
    fn owners(&self, safe: &str) -> Option<Vec<String>>;
    /// A Safe's `getThreshold()`.
    fn threshold(&self, safe: &str) -> Option<u64>;
    /// The first page of a Safe's module list from `start`.
    fn modules(&self, safe: &str, start: &str, page_size: u64) -> Option<Vec<String>>;
    /// The orchestrator's `vaultLogicIsExpected()`.
    fn vault_logic_is_expected(&self, orchestrator: &str) -> Option<bool>;
    /// An `address`-returning `eth_call` of `calldata` on `contract`, lowercase.
    /// A revert, or a return that is not one address, is `None`.
    fn call_address(&self, contract: &str, calldata: &str) -> Option<String>;
    /// A `string`-returning `eth_call` of `calldata` on `contract`. A revert, or
    /// a return that is not one ABI string, is `None`.
    fn call_string(&self, contract: &str, calldata: &str) -> Option<String>;
}

/// A chain the scanner has no endpoint for: every question goes unanswered, so
/// every check on it reads `unknown` rather than disappearing.
pub struct NoReads;

impl ChainReads for NoReads {
    fn code(&self, _: &str) -> Option<String> {
        None
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
    fn call_address(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn call_string(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Expected values, read from the deploy repo's source.
// ---------------------------------------------------------------------------

/// A named account, with the address its constant resolved to (`None` when it
/// did not resolve, which leaves every check on it `unknown`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    pub ident: String,
    pub address: Option<String>,
}

/// One frozen deployment: a pointer file the generated deploy lib imports from
/// a released version's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenPin {
    /// The release directory, as written (`0_1_30`).
    pub release: String,
    pub contract: String,
    /// The pointer file's path in the deploy repo.
    pub path: String,
    pub address: Option<String>,
    pub codehash: Option<String>,
}

/// The token-owner Safe policy (`LibSafeInvariants.assertTokenOwnerSafePolicy`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SafeExpect {
    pub threshold: Option<u64>,
    /// `STOX_TOKEN_OWNER_SAFE_OWNER_1..n`, as written.
    pub owners: Vec<String>,
    /// The SafeProxy code hashes accepted: L2 then L1.
    pub proxy_codehashes: Vec<String>,
    /// The canonical singletons accepted, each with its code hash: L2 then L1.
    pub singletons: Vec<(String, Option<String>)>,
    pub guard_slot: Option<String>,
    pub fallback_slot: Option<String>,
    pub fallback_handler: Option<String>,
}

/// Everything the per-chain checks compare against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Expectations {
    pub safe: SafeExpect,
    /// `STOX_PROD_AUTHORISER_V4_CLONE_CODEHASH`.
    pub clone_codehash: Option<String>,
    /// The 0.1.1 authoriser implementation the clone must delegate to.
    pub clone_impl: Option<String>,
    /// The distinct roles `expectedGrants` names, in source order.
    pub mapped_roles: Vec<String>,
    /// The grant map's constant grantees (every grantee but the Safe).
    pub grantees: Vec<Account>,
    /// `GRANTEE_SERVICE_1C66`, the retired signer.
    pub retired: Option<Account>,
    /// `GRANTEE_SERVICE_3D0C`, the operator key.
    pub operator: Option<Account>,
    /// `BEACON_INITIAL_OWNER`, the deploy EOA.
    pub deploy_eoa: Option<Account>,
    pub deploy_key: Account,
    /// `ST0X_ORCHESTRATOR_INSTANCE`.
    pub orchestrator: Option<String>,
    /// `ST0X_ORCHESTRATOR_BEACON`.
    pub orchestrator_beacon: Option<String>,
    /// The roles the orchestrator contract defines (`*_ROLE = keccak256("…")`).
    pub orchestrator_roles: Vec<String>,
    /// The roles the grant map gives the orchestrator instance on the clone.
    pub orchestrator_clone_roles: Vec<String>,
    pub frozen: Vec<FrozenPin>,
}

/// The pointer files the generated deploy lib imports from a released version's
/// directory, as `(release, contract, path)` in source order. A version-named
/// directory (`0_1_1`, `0_1_30`) is a frozen release; `candidate` is not, and a
/// release added later is picked up without a change here.
pub fn frozen_imports(v4_lib: &str) -> Vec<(String, String, String)> {
    let Ok(re) = Regex::new(r#"from\s*"\./(\d+_\d+_\d+)/([A-Za-z0-9_]+)\.pointers\.sol""#) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, String)> = Vec::new();
    for c in re.captures_iter(v4_lib) {
        let (release, contract) = (c[1].to_string(), c[2].to_string());
        let path = format!("src/generated/{release}/{contract}.pointers.sol");
        if !out.iter().any(|(_, _, p)| *p == path) {
            out.push((release, contract, path));
        }
    }
    out
}

/// A frozen pin from its pointer file's source: `DEPLOYED_ADDRESS` and
/// `BYTECODE_HASH`, each `None` when it did not parse (an unfetched file).
pub fn frozen_pin(release: &str, contract: &str, path: &str, pointer_src: &str) -> FrozenPin {
    FrozenPin {
        release: release.to_string(),
        contract: contract.to_string(),
        path: path.to_string(),
        address: parse_address_constant(pointer_src, "DEPLOYED_ADDRESS"),
        codehash: parse_bytes32_constant(pointer_src, "BYTECODE_HASH"),
    }
}

/// Resolve an address constant of the generated deploy lib: a literal, or an
/// alias of the `DEPLOYED_ADDRESS` it imports from a pointer file, read out of
/// that file.
pub fn resolve_generated_address(v4_lib: &str, frozen: &[FrozenPin], name: &str) -> Option<String> {
    if let Some(a) = parse_address_constant(v4_lib, name) {
        return Some(a);
    }
    let alias = Regex::new(&format!(
        r"\b{}\b\s*=\s*([A-Za-z_][A-Za-z0-9_]*)\s*;",
        regex::escape(name)
    ))
    .ok()?;
    let target = alias.captures(v4_lib)?.get(1)?.as_str().to_string();
    let import = Regex::new(r#"import\s*\{([^}]*)\}\s*from\s*"\./([^"]+)""#).ok()?;
    let imported = Regex::new(&format!(
        r"\bDEPLOYED_ADDRESS\s+as\s+{}\b",
        regex::escape(&target)
    ))
    .ok()?;
    let rel = import
        .captures_iter(v4_lib)
        .find(|c| imported.is_match(&c[1]))
        .map(|c| c[2].to_string())?;
    let path = format!("src/generated/{rel}");
    frozen.iter().find(|p| p.path == path)?.address.clone()
}

/// The role names a contract defines as `<X>_ROLE = keccak256("<NAME>")`, in
/// source order.
pub fn parse_role_names(src: &str) -> Vec<String> {
    let Ok(re) = Regex::new(r#"\b[A-Z][A-Z0-9_]*_ROLE\s*=\s*keccak256\(\s*"([A-Za-z0-9_]+)"\s*\)"#)
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for c in re.captures_iter(src) {
        if !out.iter().any(|r| r == &c[1]) {
            out.push(c[1].to_string());
        }
    }
    out
}

fn account(src: &OwnerSources, ident: &str) -> Account {
    Account {
        ident: ident.to_string(),
        address: resolve_ident(src, ident),
    }
}

/// Read every expectation out of the deploy repo's sources. `orchestrator_src`
/// is `src/concrete/ST0xOrchestrator.sol`; `frozen` are the pins
/// [`frozen_imports`] named, already read from their pointer files.
pub fn parse_expectations(
    src: &OwnerSources,
    orchestrator_src: &str,
    frozen: Vec<FrozenPin>,
) -> Expectations {
    let safe_lib = src.safe_lib;
    let mut owners = Vec::new();
    for i in 1..=64 {
        match parse_address_constant(safe_lib, &format!("STOX_TOKEN_OWNER_SAFE_OWNER_{i}")) {
            Some(a) => owners.push(a),
            None => break,
        }
    }
    let proxy_codehashes = [
        "SAFE_V1_4_1_L2_PROXY_CODEHASH",
        "SAFE_V1_4_1_L1_PROXY_CODEHASH",
    ]
    .iter()
    .filter_map(|n| parse_bytes32_constant(safe_lib, n))
    .collect();
    let singletons = ["SAFE_V1_4_1_L2_SINGLETON", "SAFE_V1_4_1_L1_SINGLETON"]
        .iter()
        .filter_map(|n| {
            parse_address_constant(safe_lib, n).map(|a| {
                (
                    a,
                    parse_bytes32_constant(safe_lib, &format!("{n}_CODEHASH")),
                )
            })
        })
        .collect();
    let safe = SafeExpect {
        threshold: parse_uint_constant(safe_lib, "STOX_TOKEN_OWNER_SAFE_THRESHOLD"),
        owners,
        proxy_codehashes,
        singletons,
        guard_slot: parse_bytes32_constant(safe_lib, "SAFE_GUARD_STORAGE_SLOT"),
        fallback_slot: parse_bytes32_constant(safe_lib, "SAFE_FALLBACK_HANDLER_STORAGE_SLOT"),
        fallback_handler: parse_address_constant(
            safe_lib,
            "SAFE_V1_4_1_COMPATIBILITY_FALLBACK_HANDLER",
        ),
    };

    let orchestrator = parse_address_constant(src.v4_lib, "ST0X_ORCHESTRATOR_INSTANCE");
    let map = parse_expected_grants(src.auth_lib);
    let mut mapped_roles: Vec<String> = Vec::new();
    let mut grantees: Vec<Account> = Vec::new();
    let mut orchestrator_clone_roles: Vec<String> = Vec::new();
    if let Some(map) = &map {
        for g in &map.grants {
            if !mapped_roles.contains(&g.role) {
                mapped_roles.push(g.role.clone());
            }
            if g.grantee == map.safe_param {
                continue;
            }
            let grantee = account(src, &g.grantee);
            let is_instance = matches!(
                (&grantee.address, &orchestrator),
                (Some(a), Some(o)) if a.eq_ignore_ascii_case(o)
            );
            if is_instance && !orchestrator_clone_roles.contains(&g.role) {
                orchestrator_clone_roles.push(g.role.clone());
            }
            if !grantees.iter().any(|a| a.ident == grantee.ident) {
                grantees.push(grantee);
            }
        }
    }
    let named = |ident: &str| {
        let a = account(src, ident);
        a.address.is_some().then_some(a)
    };

    Expectations {
        safe,
        clone_codehash: parse_bytes32_constant(
            src.v4_lib,
            "STOX_PROD_AUTHORISER_V4_CLONE_CODEHASH",
        ),
        clone_impl: resolve_generated_address(
            src.v4_lib,
            &frozen,
            "STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_0_1_1",
        ),
        mapped_roles,
        grantees,
        retired: named("GRANTEE_SERVICE_1C66"),
        operator: named("GRANTEE_SERVICE_3D0C"),
        deploy_eoa: named("BEACON_INITIAL_OWNER"),
        deploy_key: Account {
            ident: "DEPLOY_KEY".to_string(),
            address: Some(DEPLOY_KEY.to_string()),
        },
        orchestrator,
        orchestrator_beacon: parse_address_constant(src.v4_lib, "ST0X_ORCHESTRATOR_BEACON"),
        orchestrator_roles: parse_role_names(orchestrator_src),
        orchestrator_clone_roles,
        frozen,
    }
}

/// The code hash of an ERC-1167 clone of `implementation`: keccak256 over the
/// minimal-proxy runtime. `None` when `implementation` is not a 20-byte address.
pub fn erc1167_codehash(implementation: &str) -> Option<String> {
    let bare = implementation.strip_prefix("0x").unwrap_or(implementation);
    if bare.len() != 40 || !bare.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    keccak256_hex(&format!(
        "{ERC1167_PREFIX}{}{ERC1167_SUFFIX}",
        bare.to_lowercase()
    ))
}

// ---------------------------------------------------------------------------
// Verdicts.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    Unknown,
}

impl Verdict {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unknown => "unknown",
        }
    }

    pub(crate) fn of(ok: Option<bool>) -> Verdict {
        match ok {
            Some(true) => Verdict::Pass,
            Some(false) => Verdict::Fail,
            None => Verdict::Unknown,
        }
    }
}

/// One check row: `fields` names the check, the rest is its comparison.
pub(crate) fn row(
    mut fields: Value,
    how: &str,
    expected: Value,
    actual: Value,
    v: Verdict,
) -> Value {
    if let Some(o) = fields.as_object_mut() {
        o.insert("match".into(), how.into());
        o.insert("expected".into(), expected);
        o.insert("actual".into(), actual);
        o.insert("status".into(), v.token().into());
    }
    fields
}

pub(crate) fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// A storage word as shown: the address it holds, or the whole word when its
/// high bytes are set and it holds no address.
fn word_shown(w: &[u8; 32]) -> String {
    word_address(w).unwrap_or_else(|| format!("0x{}", hex::encode(w)))
}

/// Compare the code hash of `code` against the accepted hashes. `actual` is
/// the hash, `"no code"` for an address with none, or null when unread.
pub(crate) fn codehash_row(fields: Value, accepted: &[String], code: Option<&str>) -> Value {
    let (how, expected) = match accepted {
        [one] => ("equal", json!(one)),
        _ => ("anyOf", json!(accepted)),
    };
    let bare = code.map(|c| c.strip_prefix("0x").unwrap_or(c));
    let (actual, v) = match bare {
        None => (Value::Null, Verdict::Unknown),
        Some("") => (
            json!("no code"),
            if accepted.is_empty() {
                Verdict::Unknown
            } else {
                Verdict::Fail
            },
        ),
        Some(b) => match keccak256_hex(b) {
            None => (Value::Null, Verdict::Unknown),
            Some(h) => {
                let v = if accepted.is_empty() {
                    Verdict::Unknown
                } else {
                    Verdict::of(Some(accepted.iter().any(|e| same(e, &h))))
                };
                (json!(h), v)
            }
        },
    };
    row(fields, how, expected, actual, v)
}

/// A storage word that must hold `expected` as an address.
pub(crate) fn slot_address_row(
    fields: Value,
    expected: Option<&str>,
    word: Option<[u8; 32]>,
) -> Value {
    let actual = word.as_ref().map(word_shown);
    let v = match (expected, &actual) {
        (Some(e), Some(a)) => Verdict::of(Some(same(e, a))),
        _ => Verdict::Unknown,
    };
    row(fields, "equal", json!(expected), json!(actual), v)
}

/// A storage word that must be non-zero (an initializer that has run).
fn nonzero_row(fields: Value, word: Option<[u8; 32]>) -> Value {
    let actual = word.map(|w| format!("0x{}", hex::encode(w)));
    let v = Verdict::of(word.map(|w| w.iter().any(|b| *b != 0)));
    row(fields, "nonzero", Value::Null, json!(actual), v)
}

/// `hasRole(role, who)` on `contract` must be `expected`.
fn role_row(
    r: &dyn ChainReads,
    on: &str,
    contract: Option<&str>,
    role: &str,
    who: &Account,
    expected: bool,
) -> Value {
    let actual = match (contract, who.address.as_deref()) {
        (Some(c), Some(a)) => r.has_role(c, role_word(role), a),
        _ => None,
    };
    let v = match actual {
        Some(held) => Verdict::of(Some(held == expected)),
        None => Verdict::Unknown,
    };
    row(
        json!({"check": "role", "on": on, "role": role, "account": who.ident, "address": who.address}),
        "equal",
        json!(expected),
        json!(actual),
        v,
    )
}

/// `who` must hold none of `roles` on `contract`. `actual` lists the roles it
/// holds; `unread` the ones the chain did not answer for.
fn no_roles_row(
    r: &dyn ChainReads,
    on: &str,
    contract: Option<&str>,
    roles: &[String],
    who: &Account,
) -> Value {
    let mut held: Vec<&str> = Vec::new();
    let mut unread: Vec<&str> = Vec::new();
    for role in roles {
        let answer = match (contract, who.address.as_deref()) {
            (Some(c), Some(a)) => r.has_role(c, role_word(role), a),
            _ => None,
        };
        match answer {
            Some(true) => held.push(role),
            Some(false) => {}
            None => unread.push(role),
        }
    }
    let v = if !held.is_empty() {
        Verdict::Fail
    } else if roles.is_empty() || !unread.is_empty() {
        Verdict::Unknown
    } else {
        Verdict::Pass
    };
    row(
        json!({"check": "noRoles", "on": on, "account": who.ident, "address": who.address, "roles": roles, "unread": unread}),
        "set",
        json!([]),
        json!(held),
        v,
    )
}

/// Roll a subject's rows into a verdict with its tallies.
pub(crate) fn subject(address: Option<&str>, checks: Vec<Value>) -> Value {
    let count = |s: &str| checks.iter().filter(|c| c["status"] == s).count();
    let (passed, failed, unknown) = (count("pass"), count("fail"), count("unknown"));
    json!({
        "address": address,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "total": checks.len(),
        "state": rollup(passed, failed, unknown),
        "checks": checks,
    })
}

/// `fail` on any failure, `pass` only when every check passed, else `unknown`.
pub(crate) fn rollup(passed: usize, failed: usize, unknown: usize) -> &'static str {
    if failed > 0 {
        "fail"
    } else if passed > 0 && unknown == 0 {
        "pass"
    } else {
        "unknown"
    }
}

/// The token-owner Safe against the pinned policy.
fn safe_checks(exp: &SafeExpect, safe: Option<&str>, r: &dyn ChainReads) -> Vec<Value> {
    let mut out = Vec::new();

    let threshold = safe.and_then(|s| r.threshold(s));
    let v = match (exp.threshold, threshold) {
        (Some(e), Some(a)) => Verdict::of(Some(e == a)),
        _ => Verdict::Unknown,
    };
    out.push(row(
        json!({"check": "threshold"}),
        "equal",
        json!(exp.threshold),
        json!(threshold),
        v,
    ));

    // Exactly the pinned owners, in any order: owner order is a linked-list
    // artifact of how each chain's Safe was built, not policy.
    let owners = safe.and_then(|s| r.owners(s));
    let (missing, unexpected, v) = match &owners {
        None => (Vec::new(), Vec::new(), Verdict::Unknown),
        Some(live) => {
            let missing: Vec<&String> = exp
                .owners
                .iter()
                .filter(|e| !live.iter().any(|a| same(a, e)))
                .collect();
            let unexpected: Vec<&String> = live
                .iter()
                .filter(|a| !exp.owners.iter().any(|e| same(a, e)))
                .collect();
            let v = if exp.owners.is_empty() {
                Verdict::Unknown
            } else {
                Verdict::of(Some(
                    missing.is_empty() && unexpected.is_empty() && live.len() == exp.owners.len(),
                ))
            };
            (missing, unexpected, v)
        }
    };
    out.push(row(
        json!({"check": "owners", "missing": missing, "unexpected": unexpected}),
        "set",
        json!(exp.owners),
        json!(owners),
        v,
    ));

    let code = safe.and_then(|s| r.code(s));
    out.push(codehash_row(
        json!({"check": "proxyCodehash"}),
        &exp.proxy_codehashes,
        code.as_deref(),
    ));

    let word = safe.and_then(|s| r.storage(s, SAFE_SINGLETON_SLOT));
    let singleton = word.as_ref().map(word_shown);
    let canonical = singleton
        .as_deref()
        .and_then(|s| exp.singletons.iter().find(|(a, _)| same(a, s)));
    let expected: Vec<&String> = exp.singletons.iter().map(|(a, _)| a).collect();
    let v = match &singleton {
        Some(_) if !expected.is_empty() => Verdict::of(Some(canonical.is_some())),
        _ => Verdict::Unknown,
    };
    out.push(row(
        json!({"check": "singleton"}),
        "anyOf",
        json!(expected),
        json!(singleton),
        v,
    ));

    // The singleton's code hash is the one pinned for the singleton the Safe
    // points at. A non-canonical singleton has no hash to compare with, and has
    // already failed above.
    let singleton_row = json!({"check": "singletonCodehash", "address": singleton});
    out.push(match canonical {
        Some((address, Some(hash))) => {
            let code = r.code(address);
            codehash_row(singleton_row, std::slice::from_ref(hash), code.as_deref())
        }
        _ => row(
            singleton_row,
            "equal",
            Value::Null,
            Value::Null,
            Verdict::Unknown,
        ),
    });

    let modules = safe.and_then(|s| r.modules(s, SAFE_MODULES_SENTINEL, SAFE_MODULES_PAGE));
    out.push(row(
        json!({"check": "modules"}),
        "set",
        json!([]),
        json!(modules),
        Verdict::of(modules.as_ref().map(Vec::is_empty)),
    ));

    let guard = match (safe, &exp.guard_slot) {
        (Some(s), Some(slot)) => r.storage(s, slot),
        _ => None,
    };
    out.push(row(
        json!({"check": "guard", "slot": exp.guard_slot}),
        "equal",
        json!("0x0000000000000000000000000000000000000000"),
        json!(guard.as_ref().map(word_shown)),
        Verdict::of(guard.map(|w| w.iter().all(|b| *b == 0))),
    ));

    let fallback = match (safe, &exp.fallback_slot) {
        (Some(s), Some(slot)) => r.storage(s, slot),
        _ => None,
    };
    out.push(slot_address_row(
        json!({"check": "fallbackHandler", "slot": exp.fallback_slot}),
        exp.fallback_handler.as_deref(),
        fallback,
    ));
    out
}

/// The accounts no `DEFAULT_ADMIN` may sit on: the chain Safe, every constant
/// grantee of the map, the retired signer, the deploy key and the deploy EOA.
fn known_accounts(exp: &Expectations, safe: Option<&str>) -> Vec<Account> {
    let mut out = vec![Account {
        ident: "safe".to_string(),
        address: safe.map(str::to_string),
    }];
    let rest = exp
        .grantees
        .iter()
        .chain(exp.retired.iter())
        .chain(std::iter::once(&exp.deploy_key))
        .chain(exp.deploy_eoa.iter());
    for a in rest {
        if !out.iter().any(|o| o.ident == a.ident) {
            out.push(a.clone());
        }
    }
    out
}

/// The V4 authoriser clone: its code, its initializer, and the accounts that
/// must hold nothing on it. That every row of the grant map holds is the
/// `deploymentGrants` view, which asks it of every chain already.
fn authoriser_checks(
    exp: &Expectations,
    safe: Option<&str>,
    clone: Option<&str>,
    r: &dyn ChainReads,
) -> Vec<Value> {
    let mut out = Vec::new();
    let code = clone.and_then(|c| r.code(c));
    out.push(codehash_row(
        json!({"check": "codehash"}),
        exp.clone_codehash.as_slice(),
        code.as_deref(),
    ));
    let derived: Vec<String> = exp
        .clone_impl
        .as_deref()
        .and_then(erc1167_codehash)
        .into_iter()
        .collect();
    out.push(codehash_row(
        json!({"check": "erc1167Clone", "implementation": exp.clone_impl}),
        &derived,
        code.as_deref(),
    ));
    out.push(nonzero_row(
        json!({"check": "initialized", "slot": INITIALIZABLE_SLOT}),
        clone.and_then(|c| r.storage(c, INITIALIZABLE_SLOT)),
    ));
    for who in known_accounts(exp, safe) {
        out.push(role_row(r, "authoriser", clone, DEFAULT_ADMIN, &who, false));
    }
    let holders = std::iter::once(&exp.deploy_key)
        .chain(exp.deploy_eoa.iter())
        .chain(exp.retired.iter());
    for who in holders {
        out.push(no_roles_row(r, "authoriser", clone, &exp.mapped_roles, who));
    }
    out
}

/// The orchestrator instance: what it proxies, its initializer, who administers
/// and operates it, its grants on the clone, and who must hold nothing on it.
fn orchestrator_checks(
    exp: &Expectations,
    safe: Option<&str>,
    clone: Option<&str>,
    r: &dyn ChainReads,
) -> Vec<Value> {
    let instance = exp.orchestrator.as_deref();
    let mut out = Vec::new();
    out.push(slot_address_row(
        json!({"check": "beacon", "slot": ERC1967_BEACON_SLOT}),
        exp.orchestrator_beacon.as_deref(),
        instance.and_then(|i| r.storage(i, ERC1967_BEACON_SLOT)),
    ));
    out.push(nonzero_row(
        json!({"check": "initialized", "slot": INITIALIZABLE_SLOT}),
        instance.and_then(|i| r.storage(i, INITIALIZABLE_SLOT)),
    ));
    let safe_account = Account {
        ident: "safe".to_string(),
        address: safe.map(str::to_string),
    };
    out.push(role_row(
        r,
        "orchestrator",
        instance,
        DEFAULT_ADMIN,
        &safe_account,
        true,
    ));
    let logic = instance.and_then(|i| r.vault_logic_is_expected(i));
    out.push(row(
        json!({"check": "vaultLogicIsExpected"}),
        "equal",
        json!(true),
        json!(logic),
        Verdict::of(logic),
    ));
    let operator = exp.operator.clone().unwrap_or(Account {
        ident: "GRANTEE_SERVICE_3D0C".to_string(),
        address: None,
    });
    for role in OPERATOR_ROLES {
        out.push(role_row(r, "orchestrator", instance, role, &operator, true));
    }
    let instance_account = Account {
        ident: "ST0X_ORCHESTRATOR_INSTANCE".to_string(),
        address: exp.orchestrator.clone(),
    };
    if exp.orchestrator_clone_roles.is_empty() {
        // The grant map is where the instance's clone roles come from; a map
        // that names none for it has left this check nothing to ask.
        out.push(row(
            json!({"check": "role", "on": "authoriser", "role": null, "account": instance_account.ident, "address": instance_account.address}),
            "equal",
            json!(true),
            Value::Null,
            Verdict::Unknown,
        ));
    }
    for role in &exp.orchestrator_clone_roles {
        out.push(role_row(
            r,
            "authoriser",
            clone,
            role,
            &instance_account,
            true,
        ));
    }
    let roles: Vec<String> = std::iter::once(DEFAULT_ADMIN.to_string())
        .chain(exp.orchestrator_roles.iter().cloned())
        .collect();
    let holders = std::iter::once(&exp.deploy_key).chain(exp.deploy_eoa.iter());
    for who in holders {
        out.push(no_roles_row(r, "orchestrator", instance, &roles, who));
    }
    out
}

/// Each frozen deployment's live code hash against its pointer file's
/// `BYTECODE_HASH`, skipping the releases `covered` names (checked on this chain
/// by `deploymentHealth`).
fn frozen_checks(exp: &Expectations, covered: &[&str], r: &dyn ChainReads) -> Vec<Value> {
    exp.frozen
        .iter()
        .filter(|p| !covered.contains(&p.release.as_str()))
        .map(|p| {
            let code = p.address.as_deref().and_then(|a| r.code(a));
            codehash_row(
                json!({"check": "codehash", "release": p.release, "contract": p.contract, "address": p.address}),
                p.codehash.as_slice(),
                code.as_deref(),
            )
        })
        .collect()
}

/// One chain's live-state document. `covered` names the frozen releases
/// `deploymentHealth` already checks on this chain.
pub fn chain_doc(
    exp: &Expectations,
    pin: &ChainPin,
    covered: &[&str],
    r: &dyn ChainReads,
) -> Value {
    let safe = pin.safe.as_deref();
    let clone = pin.authoriser.as_deref();
    let subjects = [
        (
            "tokenOwnerSafe",
            subject(safe, safe_checks(&exp.safe, safe, r)),
        ),
        (
            "authoriser",
            subject(clone, authoriser_checks(exp, safe, clone, r)),
        ),
        (
            "orchestrator",
            subject(
                exp.orchestrator.as_deref(),
                orchestrator_checks(exp, safe, clone, r),
            ),
        ),
        ("frozen", subject(None, frozen_checks(exp, covered, r))),
    ];
    let sum = |k: &str| {
        subjects
            .iter()
            .map(|(_, s)| s[k].as_u64().unwrap_or(0) as usize)
            .sum::<usize>()
    };
    let (passed, failed, unknown) = (sum("passed"), sum("failed"), sum("unknown"));
    let covered_present: Vec<&str> = covered
        .iter()
        .copied()
        .filter(|c| exp.frozen.iter().any(|p| p.release == *c))
        .collect();
    let mut doc = json!({
        "network": pin.network,
        "rpcHost": pin.rpc_host,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "total": passed + failed + unknown,
        "state": rollup(passed, failed, unknown),
        // Frozen releases this chain leaves to `deploymentHealth`.
        "frozenCoveredByDeploymentHealth": covered_present,
    });
    if let Some(o) = doc.as_object_mut() {
        for (k, s) in subjects {
            o.insert(k.into(), s);
        }
    }
    doc
}

/// Assemble the `deploymentState` document from the per-chain documents.
/// `None` when there are no chains (the deploy lib was unreadable), so the page
/// shows nothing rather than an empty table that reads as "nothing to check".
pub fn build_state(org: &str, repo: &str, chains: Vec<Value>) -> Option<Value> {
    if chains.is_empty() {
        return None;
    }
    let sum = |k: &str| {
        chains
            .iter()
            .map(|c| c[k].as_u64().unwrap_or(0) as usize)
            .sum::<usize>()
    };
    let (passed, failed, unknown) = (sum("passed"), sum("failed"), sum("unknown"));
    Some(json!({
        "org": org,
        "repo": repo,
        "deployKey": DEPLOY_KEY,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "total": passed + failed + unknown,
        "state": rollup(passed, failed, unknown),
        "chains": chains,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owners::parse_chain_pins;
    use alloy_primitives::{keccak256, U256};
    use std::collections::{HashMap, HashSet};

    // Production values as st0x.deploy pins them (3c6db51), so the fixture
    // exercises the real shapes. Code hashes that need a preimage are the
    // keccak of stand-in code written below.
    const BASE_SAFE: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const SAFE: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const BASE_CLONE: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
    const CLONE: &str = "0x66566cc91dEAf818859bD4b09B7903ac48998157";
    const IMPL: &str = "0x2EA0d35d0B1F57C42e6130f298930228bCbFDe9b";
    const CLONE_CODEHASH: &str =
        "0x2089950d3cc1112dd66a58adcfadeadc490b50053ac67be8bc676b4a2dcd1717";
    const INSTANCE: &str = "0x3A7387a484d87Aa8bBA45E98AAB401Ce4FBF03E2";
    const ORCH_BEACON: &str = "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613";
    const ORCH_IMPL: &str = "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72";
    const OPERATOR: &str = "0x3d0CD66EFA66c05d86c3d4316B03eAE87ab9E8aE";
    const RETIRED: &str = "0x1c66D6708914C40239D54919320b4C48cAE3D1A9";
    const EOA: &str = "0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b";
    const L2_SINGLETON: &str = "0x29fcB43b46531BcA003ddC8FCB67FFE91900C762";
    const L1_SINGLETON: &str = "0x41675C099F32341bf84BFc5382aF534df5C7461a";
    const FALLBACK: &str = "0xfd0732Dc9E303f09fCEf3a7388Ad10A83459Ec99";
    const GUARD_SLOT: &str = "0x4a204f620c8c5ccdca3fd54d003badd85ba500436a431f0cbda4f558c93c34c8";
    const FALLBACK_SLOT: &str =
        "0x6c9a6c4a39284e37ed1cf53d337577d14212a4870fb976a4366c693b939918d5";
    const OWNERS: [&str; 6] = [
        "0x4746095B1Ea1A84446d34448f44e74D3d51f92F2",
        "0xceC2cb8B8EE4000FFA3F8a7f8E0Fa0A3E3DAb72d",
        "0x8D5901d8aE48101B59400235ad8614A2e0510466",
        "0xC1C89b7f5448F447d59f920456A9610f6b2544bC",
        "0xAB92b327c97A6E7461cBd76E2a789E5e106FF87e",
        "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
    ];

    // Stand-in runtime code: L2/L1 proxy, L2/L1 singleton, two frozen impls.
    const L2_PROXY_CODE: &str = "0xa2";
    const L1_PROXY_CODE: &str = "0xa1";
    const L2_SINGLETON_CODE: &str = "0xb2";
    const L1_SINGLETON_CODE: &str = "0xb1";
    const AUTH_IMPL_CODE: &str = "0xc1";
    const ORCH_IMPL_CODE: &str = "0xc2";

    fn h(code: &str) -> String {
        keccak256_hex(code).unwrap()
    }

    fn clone_code(implementation: &str) -> String {
        format!(
            "0x{ERC1167_PREFIX}{}{ERC1167_SUFFIX}",
            implementation.trim_start_matches("0x").to_lowercase()
        )
    }

    fn safe_lib() -> String {
        let owners: String = OWNERS
            .iter()
            .enumerate()
            .map(|(i, o)| {
                format!(
                    "    address internal constant STOX_TOKEN_OWNER_SAFE_OWNER_{} = {o};\n",
                    i + 1
                )
            })
            .collect();
        format!(
            r"
    address internal constant SAFE_V1_4_1_COMPATIBILITY_FALLBACK_HANDLER = {FALLBACK};
    address internal constant SAFE_V1_4_1_L2_SINGLETON = {L2_SINGLETON};
    bytes32 internal constant SAFE_V1_4_1_L2_PROXY_CODEHASH =
        {};
    bytes32 internal constant SAFE_V1_4_1_L2_SINGLETON_CODEHASH =
        {};
    address internal constant SAFE_V1_4_1_L1_SINGLETON = {L1_SINGLETON};
    bytes32 internal constant SAFE_V1_4_1_L1_PROXY_CODEHASH =
        {};
    bytes32 internal constant SAFE_V1_4_1_L1_SINGLETON_CODEHASH =
        {};
    address internal constant STOX_TOKEN_OWNER_SAFE = {BASE_SAFE};
    address internal constant STOX_TOKEN_OWNER_SAFE_ETHEREUM = {SAFE};
    uint256 internal constant STOX_TOKEN_OWNER_SAFE_THRESHOLD = 3;
{owners}
    bytes32 internal constant SAFE_GUARD_STORAGE_SLOT =
        {GUARD_SLOT};
    bytes32 internal constant SAFE_FALLBACK_HANDLER_STORAGE_SLOT =
        {FALLBACK_SLOT};
    address internal constant SAFE_MODULES_SENTINEL = address(0x1);
",
            h(L2_PROXY_CODE),
            h(L2_SINGLETON_CODE),
            h(L1_PROXY_CODE),
            h(L1_SINGLETON_CODE),
        )
    }

    // The real map's shape: a no-arg overload, the one-arg entry delegating to
    // the two-arg body, and grantees that are aliases of other libs' pins.
    fn auth_lib() -> String {
        format!(
            r#"
    address internal constant STOX_PROD_AUTHORISER = LibProdDeployV4.STOX_PROD_AUTHORISER_V4_CLONE;
    bytes32 internal constant DEFAULT_ADMIN_ROLE = bytes32(0);
    address internal constant GRANTEE_TOKEN_OWNER_SAFE = LibSafeInvariants.STOX_TOKEN_OWNER_SAFE;
    address internal constant GRANTEE_SERVICE_1C66 = {RETIRED};
    address internal constant GRANTEE_SERVICE_3D0C = {OPERATOR};
    address internal constant GRANTEE_ORCHESTRATOR = LibProdDeployV4.ST0X_ORCHESTRATOR_INSTANCE;
    function expectedGrants() internal pure returns (RoleGrant[] memory grants) {{
        grants = expectedGrants(GRANTEE_TOKEN_OWNER_SAFE);
    }}
    function expectedGrants(address tokenOwnerSafe) internal pure returns (RoleGrant[] memory grants) {{
        grants = expectedGrants(tokenOwnerSafe, tokenOwnerSafe);
    }}
    function expectedGrants(address tokenOwnerSafe, address adminHolder)
        internal
        pure
        returns (RoleGrant[] memory grants)
    {{
        grants = new RoleGrant[](7);
        grants[0] = RoleGrant(keccak256("DEPOSIT_ADMIN"), adminHolder);
        grants[1] = RoleGrant(keccak256("DEPOSIT"), tokenOwnerSafe);
        grants[2] = RoleGrant(keccak256("CERTIFY"), tokenOwnerSafe);
        grants[3] = RoleGrant(keccak256("DEPOSIT"), GRANTEE_SERVICE_3D0C);
        grants[4] = RoleGrant(keccak256("CERTIFY"), GRANTEE_SERVICE_3D0C);
        grants[5] = RoleGrant(keccak256("DEPOSIT"), GRANTEE_ORCHESTRATOR);
        grants[6] = RoleGrant(keccak256("WITHDRAW"), GRANTEE_ORCHESTRATOR);
    }}
"#
        )
    }

    fn v4_lib() -> String {
        format!(
            r#"
import {{
    DEPLOYED_ADDRESS as STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_ADDRESS_0_1_1_GEN,
    BYTECODE_HASH as STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_CODEHASH_0_1_1_GEN
}} from "./0_1_1/StoxOffchainAssetReceiptVaultAuthorizerV1.pointers.sol";
import {{
    DEPLOYED_ADDRESS as ST0X_ORCHESTRATOR_ADDRESS_0_1_30_GEN,
    BYTECODE_HASH as ST0X_ORCHESTRATOR_CODEHASH_0_1_30_GEN
}} from "./0_1_30/ST0xOrchestrator.pointers.sol";
import {{
    DEPLOYED_ADDRESS as STOX_RECEIPT_ADDRESS_CANDIDATE_GEN
}} from "./candidate/StoxReceipt.pointers.sol";
library LibProdDeployV4 {{
    address constant BEACON_INITIAL_OWNER = address({EOA});
    address constant STOX_PROD_AUTHORISER_V4_CLONE = address({BASE_CLONE});
    bytes32 constant STOX_PROD_AUTHORISER_V4_CLONE_CODEHASH =
        {CLONE_CODEHASH};
    address constant STOX_PROD_AUTHORISER_V4_CLONE_ETHEREUM = address({CLONE});
    address constant ST0X_ORCHESTRATOR_BEACON = address({ORCH_BEACON});
    address constant ST0X_ORCHESTRATOR_INSTANCE = address({INSTANCE});
    address constant STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_0_1_1 =
        STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_ADDRESS_0_1_1_GEN;
}}
"#
        )
    }

    const ORCHESTRATOR_SRC: &str = r#"
    bytes32 public constant MINT_ROLE = keccak256("MINT");
    bytes32 public constant BURN_ROLE = keccak256("BURN");
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY");
    bytes32 public constant MINT_AUTH_TYPEHASH =
        keccak256("MintAuth(address token,address recipient,uint256 amount,bytes32 nonce)");
"#;

    fn pointer(address: &str, code: &str) -> String {
        format!(
            "bytes32 constant BYTECODE_HASH = bytes32({});\naddress constant DEPLOYED_ADDRESS = address({address});\n",
            h(code)
        )
    }

    fn frozen() -> Vec<FrozenPin> {
        frozen_imports(&v4_lib())
            .into_iter()
            .map(|(release, contract, path)| {
                let src = match contract.as_str() {
                    "StoxOffchainAssetReceiptVaultAuthorizerV1" => pointer(IMPL, AUTH_IMPL_CODE),
                    _ => pointer(ORCH_IMPL, ORCH_IMPL_CODE),
                };
                frozen_pin(&release, &contract, &path, &src)
            })
            .collect()
    }

    fn expectations() -> Expectations {
        let (safe, auth, v4) = (safe_lib(), auth_lib(), v4_lib());
        let src = OwnerSources {
            safe_lib: &safe,
            auth_lib: &auth,
            v4_lib: &v4,
            overrides: "",
        };
        parse_expectations(&src, ORCHESTRATOR_SRC, frozen())
    }

    fn pins() -> Vec<ChainPin> {
        let mut pins = parse_chain_pins(&v4_lib(), &safe_lib());
        for p in pins.iter_mut() {
            p.rpc_host = Some(format!("{}.example", p.network));
        }
        pins
    }

    fn norm(s: &str) -> String {
        format!("{:0>64}", s.trim_start_matches("0x").to_lowercase())
    }

    fn word_of(address: &str) -> [u8; 32] {
        let mut w = [0u8; 32];
        hex::decode_to_slice(norm(address), &mut w).unwrap();
        w
    }

    /// One way to break a healthy chain, applied to its fake.
    type Breach = Box<dyn Fn(&mut Fake)>;

    /// A chain held in memory. Unset storage reads zero and an address with no
    /// code set reads as empty, as on a real chain; the Safe getters and the
    /// orchestrator's lock answer only where set.
    #[derive(Default, Clone)]
    struct Fake {
        code: HashMap<String, String>,
        storage: HashMap<(String, String), [u8; 32]>,
        granted: HashSet<(String, [u8; 32], String)>,
        reverting: HashSet<String>,
        owners: HashMap<String, Vec<String>>,
        threshold: HashMap<String, u64>,
        modules: HashMap<String, Vec<String>>,
        vault_logic: HashMap<String, bool>,
    }

    impl Fake {
        fn set_code(&mut self, address: &str, code: &str) {
            self.code.insert(address.to_lowercase(), code.to_string());
        }
        fn set_storage(&mut self, address: &str, slot: &str, word: [u8; 32]) {
            self.storage
                .insert((address.to_lowercase(), norm(slot)), word);
        }
        fn grant(&mut self, contract: &str, role: &str, account: &str) {
            self.granted.insert((
                contract.to_lowercase(),
                role_word(role),
                account.to_lowercase(),
            ));
        }
        fn revoke(&mut self, contract: &str, role: &str, account: &str) {
            self.granted.remove(&(
                contract.to_lowercase(),
                role_word(role),
                account.to_lowercase(),
            ));
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
        fn storage(&self, address: &str, slot: &str) -> Option<[u8; 32]> {
            Some(
                self.storage
                    .get(&(address.to_lowercase(), norm(slot)))
                    .copied()
                    .unwrap_or([0u8; 32]),
            )
        }
        fn has_role(&self, contract: &str, role: [u8; 32], account: &str) -> Option<bool> {
            if self.reverting.contains(&contract.to_lowercase()) {
                return None;
            }
            Some(
                self.granted
                    .contains(&(contract.to_lowercase(), role, account.to_lowercase())),
            )
        }
        fn owners(&self, safe: &str) -> Option<Vec<String>> {
            self.owners.get(&safe.to_lowercase()).cloned()
        }
        fn threshold(&self, safe: &str) -> Option<u64> {
            self.threshold.get(&safe.to_lowercase()).copied()
        }
        fn modules(&self, safe: &str, start: &str, page_size: u64) -> Option<Vec<String>> {
            assert_eq!(start, SAFE_MODULES_SENTINEL);
            assert_eq!(page_size, 10);
            self.modules.get(&safe.to_lowercase()).cloned()
        }
        fn vault_logic_is_expected(&self, orchestrator: &str) -> Option<bool> {
            self.vault_logic.get(&orchestrator.to_lowercase()).copied()
        }
        fn call_address(&self, _: &str, _: &str) -> Option<String> {
            None
        }
        fn call_string(&self, _: &str, _: &str) -> Option<String> {
            None
        }
    }

    /// The Ethereum chain in the state the deleted scripts left it: an L1 Safe
    /// in policy, the clone, the orchestrator, both frozen impls. Owners are
    /// returned in reverse, since order is not policy.
    fn healthy() -> Fake {
        let mut f = Fake::default();
        f.set_code(SAFE, L1_PROXY_CODE);
        f.set_code(L1_SINGLETON, L1_SINGLETON_CODE);
        f.set_code(L2_SINGLETON, L2_SINGLETON_CODE);
        f.set_storage(SAFE, "0x0", word_of(L1_SINGLETON));
        f.set_storage(SAFE, FALLBACK_SLOT, word_of(FALLBACK));
        f.owners.insert(
            SAFE.to_lowercase(),
            OWNERS.iter().rev().map(|o| o.to_lowercase()).collect(),
        );
        f.threshold.insert(SAFE.to_lowercase(), 3);
        f.modules.insert(SAFE.to_lowercase(), Vec::new());

        f.set_code(CLONE, &clone_code(IMPL));
        f.set_storage(CLONE, INITIALIZABLE_SLOT, word_of("0x1"));
        for (role, who) in [
            ("DEPOSIT_ADMIN", SAFE),
            ("DEPOSIT", SAFE),
            ("CERTIFY", SAFE),
            ("DEPOSIT", OPERATOR),
            ("CERTIFY", OPERATOR),
            ("DEPOSIT", INSTANCE),
            ("WITHDRAW", INSTANCE),
        ] {
            f.grant(CLONE, role, who);
        }

        f.set_storage(INSTANCE, ERC1967_BEACON_SLOT, word_of(ORCH_BEACON));
        f.set_storage(INSTANCE, INITIALIZABLE_SLOT, word_of("0x1"));
        f.grant(INSTANCE, DEFAULT_ADMIN, SAFE);
        f.grant(INSTANCE, "MINT", OPERATOR);
        f.grant(INSTANCE, "BURN", OPERATOR);
        f.vault_logic.insert(INSTANCE.to_lowercase(), true);

        f.set_code(IMPL, AUTH_IMPL_CODE);
        f.set_code(ORCH_IMPL, ORCH_IMPL_CODE);
        f
    }

    fn ethereum() -> ChainPin {
        pins()
            .into_iter()
            .find(|p| p.network == "ethereum")
            .unwrap()
    }

    fn doc_for(f: &Fake) -> Value {
        chain_doc(&expectations(), &ethereum(), &[], f)
    }

    /// The one row of `subject` that `pred` picks.
    fn only<'a>(doc: &'a Value, subject: &str, pred: impl Fn(&Value) -> bool) -> &'a Value {
        let hits: Vec<&Value> = doc[subject]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| pred(c))
            .collect();
        assert_eq!(hits.len(), 1, "{subject}: {hits:?}");
        hits[0]
    }

    fn check<'a>(doc: &'a Value, subject: &str, name: &str) -> &'a Value {
        only(doc, subject, |c| c["check"] == name)
    }

    fn role<'a>(doc: &'a Value, subject: &str, role: &str, account: &str) -> &'a Value {
        only(doc, subject, |c| {
            c["check"] == "role" && c["role"] == role && c["account"] == account
        })
    }

    fn no_roles<'a>(doc: &'a Value, subject: &str, account: &str) -> &'a Value {
        only(doc, subject, |c| {
            c["check"] == "noRoles" && c["account"] == account
        })
    }

    fn failing(doc: &Value) -> Vec<String> {
        let mut out = Vec::new();
        for s in ["tokenOwnerSafe", "authoriser", "orchestrator", "frozen"] {
            for c in doc[s]["checks"].as_array().unwrap() {
                if c["status"] != "pass" {
                    out.push(format!("{s}:{}:{}", c["check"], c["status"]));
                }
            }
        }
        out
    }

    #[test]
    fn the_frozen_releases_are_the_version_dirs_the_deploy_lib_imports() {
        let imports = frozen_imports(&v4_lib());
        assert_eq!(
            imports,
            vec![
                (
                    "0_1_1".to_string(),
                    "StoxOffchainAssetReceiptVaultAuthorizerV1".to_string(),
                    "src/generated/0_1_1/StoxOffchainAssetReceiptVaultAuthorizerV1.pointers.sol"
                        .to_string()
                ),
                (
                    "0_1_30".to_string(),
                    "ST0xOrchestrator".to_string(),
                    "src/generated/0_1_30/ST0xOrchestrator.pointers.sol".to_string()
                ),
            ],
            "candidate is not a frozen release"
        );
        // A file imported twice is one deployment.
        let twice = format!(
            "{}\nimport {{ BYTECODE_HASH as X }} from \"./0_1_30/ST0xOrchestrator.pointers.sol\";",
            v4_lib()
        );
        assert_eq!(frozen_imports(&twice).len(), 2);
    }

    #[test]
    fn a_frozen_pin_reads_its_pointer_file() {
        let p = frozen_pin("0_1_30", "X", "p", &pointer(ORCH_IMPL, ORCH_IMPL_CODE));
        assert_eq!(p.address.as_deref(), Some(ORCH_IMPL));
        assert_eq!(p.codehash, Some(h(ORCH_IMPL_CODE)));
        let unread = frozen_pin("0_1_30", "X", "p", "");
        assert_eq!((unread.address, unread.codehash), (None, None));
    }

    #[test]
    fn a_generated_address_resolves_through_its_import_alias() {
        let v4 = v4_lib();
        let pins = frozen();
        assert_eq!(
            resolve_generated_address(
                &v4,
                &pins,
                "STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_0_1_1"
            )
            .as_deref(),
            Some(IMPL)
        );
        // A literal needs no pointer file.
        assert_eq!(
            resolve_generated_address(&v4, &[], "ST0X_ORCHESTRATOR_INSTANCE").as_deref(),
            Some(INSTANCE)
        );
        // An alias whose pointer file was not read resolves to nothing, not to
        // some other file's address.
        let others: Vec<FrozenPin> = pins
            .into_iter()
            .filter(|p| p.contract != "StoxOffchainAssetReceiptVaultAuthorizerV1")
            .collect();
        assert_eq!(
            resolve_generated_address(
                &v4,
                &others,
                "STOX_OFFCHAIN_ASSET_RECEIPT_VAULT_AUTHORIZER_V1_0_1_1"
            ),
            None
        );
        assert_eq!(resolve_generated_address(&v4, &frozen(), "NOPE"), None);
    }

    #[test]
    fn role_names_are_the_role_constants_only() {
        assert_eq!(
            parse_role_names(ORCHESTRATOR_SRC),
            vec!["MINT", "BURN", "EMERGENCY"]
        );
        assert!(parse_role_names("").is_empty());
    }

    /// The clone code hash derived from the 0.1.1 authoriser impl is the one the
    /// deploy repo pins, `STOX_PROD_AUTHORISER_V4_CLONE_CODEHASH`.
    #[test]
    fn the_erc1167_hash_of_the_impl_is_the_pinned_clone_codehash() {
        assert_eq!(erc1167_codehash(IMPL).as_deref(), Some(CLONE_CODEHASH));
        assert_eq!(
            erc1167_codehash(&IMPL.to_lowercase()).as_deref(),
            Some(CLONE_CODEHASH)
        );
        assert_eq!(erc1167_codehash("0x2EA0"), None);
        assert_eq!(erc1167_codehash(&format!("{IMPL}zz")), None);
    }

    /// The standard slots written here are the ones their definitions give.
    #[test]
    fn the_standard_slots_match_their_definitions() {
        let beacon = U256::from_be_bytes(keccak256("eip1967.proxy.beacon").0) - U256::from(1);
        assert_eq!(
            format!("0x{}", hex::encode(beacon.to_be_bytes::<32>())),
            ERC1967_BEACON_SLOT
        );
        // ERC-7201: keccak256(abi.encode(uint256(keccak256(id)) - 1)) & ~0xff.
        let inner =
            U256::from_be_bytes(keccak256("openzeppelin.storage.Initializable").0) - U256::from(1);
        let mut slot = keccak256(inner.to_be_bytes::<32>()).0;
        slot[31] = 0;
        assert_eq!(format!("0x{}", hex::encode(slot)), INITIALIZABLE_SLOT);
        assert_eq!(role_word(DEFAULT_ADMIN), [0u8; 32]);
        assert_eq!(role_word("MINT"), role_id("MINT"));
    }

    #[test]
    fn every_expectation_comes_out_of_the_source() {
        let e = expectations();
        assert_eq!(e.safe.threshold, Some(3));
        assert_eq!(e.safe.owners, OWNERS.to_vec());
        assert_eq!(
            e.safe.proxy_codehashes,
            vec![h(L2_PROXY_CODE), h(L1_PROXY_CODE)]
        );
        assert_eq!(
            e.safe.singletons,
            vec![
                (L2_SINGLETON.to_string(), Some(h(L2_SINGLETON_CODE))),
                (L1_SINGLETON.to_string(), Some(h(L1_SINGLETON_CODE))),
            ]
        );
        assert_eq!(e.safe.guard_slot.as_deref(), Some(GUARD_SLOT));
        assert_eq!(e.safe.fallback_slot.as_deref(), Some(FALLBACK_SLOT));
        assert_eq!(e.safe.fallback_handler.as_deref(), Some(FALLBACK));
        assert_eq!(e.clone_codehash.as_deref(), Some(CLONE_CODEHASH));
        assert_eq!(e.clone_impl.as_deref(), Some(IMPL));
        assert_eq!(
            e.mapped_roles,
            vec!["DEPOSIT_ADMIN", "DEPOSIT", "CERTIFY", "WITHDRAW"]
        );
        assert_eq!(
            e.grantees,
            vec![
                Account {
                    ident: "GRANTEE_SERVICE_3D0C".into(),
                    address: Some(OPERATOR.into())
                },
                Account {
                    ident: "GRANTEE_ORCHESTRATOR".into(),
                    address: Some(INSTANCE.into())
                },
            ]
        );
        assert_eq!(
            e.retired.as_ref().and_then(|a| a.address.as_deref()),
            Some(RETIRED)
        );
        assert_eq!(
            e.operator.as_ref().and_then(|a| a.address.as_deref()),
            Some(OPERATOR)
        );
        assert_eq!(
            e.deploy_eoa.as_ref().and_then(|a| a.address.as_deref()),
            Some(EOA)
        );
        assert_eq!(e.deploy_key.address.as_deref(), Some(DEPLOY_KEY));
        assert_eq!(e.orchestrator.as_deref(), Some(INSTANCE));
        assert_eq!(e.orchestrator_beacon.as_deref(), Some(ORCH_BEACON));
        assert_eq!(e.orchestrator_roles, vec!["MINT", "BURN", "EMERGENCY"]);
        assert_eq!(e.orchestrator_clone_roles, vec!["DEPOSIT", "WITHDRAW"]);
        assert_eq!(e.frozen.len(), 2);
    }

    #[test]
    fn a_chain_in_the_state_the_scripts_left_passes_every_check() {
        let doc = doc_for(&healthy());
        assert_eq!(failing(&doc), Vec::<String>::new());
        assert_eq!(doc["state"], "pass");
        assert_eq!(doc["network"], "ethereum");
        assert_eq!(doc["rpcHost"], "ethereum.example");
        assert_eq!(doc["tokenOwnerSafe"]["address"], SAFE);
        assert_eq!(doc["authoriser"]["address"], CLONE);
        assert_eq!(doc["orchestrator"]["address"], INSTANCE);
        // 8 Safe checks; 3 clone-code checks, 6 accounts without DEFAULT_ADMIN
        // and 3 holding nothing; 10 on the orchestrator; 2 frozen.
        assert_eq!(doc["tokenOwnerSafe"]["total"], 8);
        assert_eq!(doc["authoriser"]["total"], 12);
        assert_eq!(doc["orchestrator"]["total"], 10);
        assert_eq!(doc["frozen"]["total"], 2);
        assert_eq!(doc["total"], 32);
        assert_eq!(doc["passed"], 32);
        let admins: Vec<&str> = doc["authoriser"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["check"] == "role")
            .map(|c| c["account"].as_str().unwrap())
            .collect();
        assert_eq!(
            admins,
            vec![
                "safe",
                "GRANTEE_SERVICE_3D0C",
                "GRANTEE_ORCHESTRATOR",
                "GRANTEE_SERVICE_1C66",
                "DEPLOY_KEY",
                "BEACON_INITIAL_OWNER"
            ]
        );
        let orch_roles = &no_roles(&doc, "orchestrator", "DEPLOY_KEY")["roles"];
        assert_eq!(
            *orch_roles,
            json!(["DEFAULT_ADMIN", "MINT", "BURN", "EMERGENCY"])
        );
        let clone_roles = &no_roles(&doc, "authoriser", "GRANTEE_SERVICE_1C66")["roles"];
        assert_eq!(
            *clone_roles,
            json!(["DEPOSIT_ADMIN", "DEPOSIT", "CERTIFY", "WITHDRAW"])
        );
    }

    #[test]
    fn the_safe_owner_set_must_be_exact_but_not_ordered() {
        let mut f = healthy();
        let mut swapped: Vec<String> = OWNERS[..5].iter().map(|o| o.to_lowercase()).collect();
        swapped.push("0x000000000000000000000000000000000000beef".into());
        f.owners.insert(SAFE.to_lowercase(), swapped);
        let doc = doc_for(&f);
        let owners = check(&doc, "tokenOwnerSafe", "owners");
        assert_eq!(owners["status"], "fail");
        assert_eq!(owners["missing"], json!([OWNERS[5]]));
        assert_eq!(
            owners["unexpected"],
            json!(["0x000000000000000000000000000000000000beef"])
        );
        assert_eq!(doc["state"], "fail");

        // A seventh owner on top of the six is not the pinned set either.
        let mut f = healthy();
        let mut seven: Vec<String> = OWNERS.iter().map(|o| o.to_lowercase()).collect();
        seven.push("0x000000000000000000000000000000000000beef".into());
        f.owners.insert(SAFE.to_lowercase(), seven);
        assert_eq!(
            check(&doc_for(&f), "tokenOwnerSafe", "owners")["status"],
            "fail"
        );
    }

    #[test]
    fn each_safe_policy_breach_fails_its_own_check() {
        let cases: Vec<(&str, Breach)> = vec![
            (
                "threshold",
                Box::new(|f| {
                    f.threshold.insert(SAFE.to_lowercase(), 2);
                }),
            ),
            ("proxyCodehash", Box::new(|f| f.set_code(SAFE, "0xdead"))),
            (
                "modules",
                Box::new(|f| {
                    f.modules.insert(
                        SAFE.to_lowercase(),
                        vec!["0x000000000000000000000000000000000000beef".into()],
                    );
                }),
            ),
            (
                "guard",
                Box::new(|f| {
                    f.set_storage(SAFE, GUARD_SLOT, word_of("0xbeef"));
                }),
            ),
            (
                "fallbackHandler",
                Box::new(|f| {
                    f.set_storage(SAFE, FALLBACK_SLOT, word_of("0xbeef"));
                }),
            ),
            (
                "singletonCodehash",
                Box::new(|f| f.set_code(L1_SINGLETON, "0xdead")),
            ),
        ];
        for (name, breach) in cases {
            let mut f = healthy();
            breach(&mut f);
            let doc = doc_for(&f);
            assert_eq!(
                failing(&doc),
                vec![format!("tokenOwnerSafe:\"{name}\":\"fail\"")],
                "{name}"
            );
        }
    }

    #[test]
    fn an_l2_safe_passes_against_the_l2_pins() {
        let mut f = healthy();
        f.set_code(SAFE, L2_PROXY_CODE);
        f.set_storage(SAFE, "0x0", word_of(L2_SINGLETON));
        let doc = doc_for(&f);
        assert_eq!(failing(&doc), Vec::<String>::new());
        let hash = check(&doc, "tokenOwnerSafe", "singletonCodehash");
        assert_eq!(hash["expected"], h(L2_SINGLETON_CODE));
        assert_eq!(hash["address"], L2_SINGLETON.to_lowercase());
    }

    /// A singleton that is neither canonical one fails, and its code hash has
    /// nothing to be compared with: unknown, not a second failure.
    #[test]
    fn a_foreign_singleton_fails_and_leaves_its_codehash_unknown() {
        let mut f = healthy();
        f.set_storage(SAFE, "0x0", word_of("0xbeef"));
        let doc = doc_for(&f);
        assert_eq!(check(&doc, "tokenOwnerSafe", "singleton")["status"], "fail");
        let hash = check(&doc, "tokenOwnerSafe", "singletonCodehash");
        assert_eq!(hash["status"], "unknown");
        assert_eq!(hash["expected"], Value::Null);
    }

    #[test]
    fn a_safe_with_no_code_fails_rather_than_reading_unknown() {
        let mut f = healthy();
        f.code.remove(&SAFE.to_lowercase());
        let doc = doc_for(&f);
        let proxy = check(&doc, "tokenOwnerSafe", "proxyCodehash");
        assert_eq!(proxy["status"], "fail");
        assert_eq!(proxy["actual"], "no code");
        assert_eq!(proxy["match"], "anyOf");
    }

    #[test]
    fn clone_code_that_is_not_the_pinned_clone_fails_both_hashes() {
        let mut f = healthy();
        f.set_code(
            CLONE,
            &clone_code("0x000000000000000000000000000000000000beef"),
        );
        let doc = doc_for(&f);
        assert_eq!(
            failing(&doc),
            vec![
                "authoriser:\"codehash\":\"fail\"",
                "authoriser:\"erc1167Clone\":\"fail\""
            ]
        );
        assert_eq!(
            check(&doc, "authoriser", "erc1167Clone")["implementation"],
            IMPL
        );
    }

    #[test]
    fn an_uninitialized_clone_or_instance_fails() {
        let mut f = healthy();
        f.set_storage(CLONE, INITIALIZABLE_SLOT, [0u8; 32]);
        f.set_storage(INSTANCE, INITIALIZABLE_SLOT, [0u8; 32]);
        let doc = doc_for(&f);
        assert_eq!(
            failing(&doc),
            vec![
                "authoriser:\"initialized\":\"fail\"",
                "orchestrator:\"initialized\":\"fail\""
            ]
        );
    }

    #[test]
    fn a_default_admin_on_a_known_account_fails() {
        for who in [SAFE, OPERATOR, INSTANCE, RETIRED, DEPLOY_KEY, EOA] {
            let mut f = healthy();
            f.grant(CLONE, DEFAULT_ADMIN, who);
            let doc = doc_for(&f);
            let failed = failing(&doc);
            assert_eq!(failed, vec!["authoriser:\"role\":\"fail\""], "{who}");
        }
        let mut f = healthy();
        f.grant(CLONE, DEFAULT_ADMIN, OPERATOR);
        let row = role(
            &doc_for(&f),
            "authoriser",
            DEFAULT_ADMIN,
            "GRANTEE_SERVICE_3D0C",
        )
        .clone();
        assert_eq!(row["actual"], true);
        assert_eq!(row["expected"], false);
    }

    #[test]
    fn a_mapped_role_on_the_deploy_key_eoa_or_retired_signer_fails() {
        for (who, ident) in [
            (DEPLOY_KEY, "DEPLOY_KEY"),
            (EOA, "BEACON_INITIAL_OWNER"),
            (RETIRED, "GRANTEE_SERVICE_1C66"),
        ] {
            let mut f = healthy();
            f.grant(CLONE, "CERTIFY", who);
            let doc = doc_for(&f);
            let row = no_roles(&doc, "authoriser", ident);
            assert_eq!(row["status"], "fail", "{ident}");
            assert_eq!(row["actual"], json!(["CERTIFY"]));
            assert_eq!(failing(&doc).len(), 1, "{ident}");
        }
    }

    /// A clone that reverts on `hasRole` has said nothing: every role check on
    /// it is unknown, and none of them passes.
    #[test]
    fn a_reverting_has_role_is_unknown_not_a_pass() {
        let mut f = healthy();
        f.reverting.insert(CLONE.to_lowercase());
        let doc = doc_for(&f);
        let row = no_roles(&doc, "authoriser", "DEPLOY_KEY");
        assert_eq!(row["status"], "unknown");
        assert_eq!(row["unread"], row["roles"]);
        assert_eq!(
            role(&doc, "authoriser", DEFAULT_ADMIN, "safe")["status"],
            "unknown"
        );
        assert_eq!(
            role(
                &doc,
                "orchestrator",
                "DEPOSIT",
                "ST0X_ORCHESTRATOR_INSTANCE"
            )["status"],
            "unknown"
        );
        assert_eq!(doc["failed"], 0);
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn each_orchestrator_breach_fails_its_own_check() {
        let cases: Vec<(&str, Breach)> = vec![
            (
                "beacon",
                Box::new(|f| f.set_storage(INSTANCE, ERC1967_BEACON_SLOT, word_of("0xbeef"))),
            ),
            (
                "vaultLogicIsExpected",
                Box::new(|f| {
                    f.vault_logic.insert(INSTANCE.to_lowercase(), false);
                }),
            ),
            (
                "role",
                Box::new(|f| f.revoke(INSTANCE, DEFAULT_ADMIN, SAFE)),
            ),
            ("role", Box::new(|f| f.revoke(INSTANCE, "BURN", OPERATOR))),
            ("role", Box::new(|f| f.revoke(CLONE, "WITHDRAW", INSTANCE))),
            ("noRoles", Box::new(|f| f.grant(INSTANCE, "EMERGENCY", EOA))),
            (
                "noRoles",
                Box::new(|f| f.grant(INSTANCE, DEFAULT_ADMIN, DEPLOY_KEY)),
            ),
        ];
        for (name, breach) in cases {
            let mut f = healthy();
            breach(&mut f);
            assert_eq!(
                failing(&doc_for(&f)),
                vec![format!("orchestrator:\"{name}\":\"fail\"")],
                "{name}"
            );
        }
    }

    #[test]
    fn frozen_code_is_checked_except_where_deployment_health_covers_it() {
        let mut f = healthy();
        f.set_code(ORCH_IMPL, "0xdead");
        let doc = doc_for(&f);
        let row = only(&doc, "frozen", |c| c["contract"] == "ST0xOrchestrator");
        assert_eq!(row["status"], "fail");
        assert_eq!(row["release"], "0_1_30");
        assert_eq!(row["expected"], h(ORCH_IMPL_CODE));
        assert_eq!(row["actual"], h("0xdead"));
        assert_eq!(doc["frozenCoveredByDeploymentHealth"], json!([]));

        let covered = chain_doc(&expectations(), &ethereum(), &["0_1_1"], &healthy());
        let releases: Vec<&Value> = covered["frozen"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| &c["release"])
            .collect();
        assert_eq!(releases, vec!["0_1_30"]);
        assert_eq!(covered["frozenCoveredByDeploymentHealth"], json!(["0_1_1"]));
    }

    /// A chain with no endpoint keeps every check, each unknown: a chain that
    /// vanished would read as "nothing deployed there".
    #[test]
    fn an_unreachable_chain_keeps_every_check_as_unknown() {
        let doc = chain_doc(&expectations(), &ethereum(), &[], &NoReads);
        assert_eq!(doc["total"], 32);
        assert_eq!(doc["unknown"], 32);
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn a_chain_without_an_authoriser_pin_leaves_the_clone_checks_unknown() {
        let mut pin = ethereum();
        pin.authoriser = None;
        let doc = chain_doc(&expectations(), &pin, &[], &healthy());
        assert_eq!(doc["authoriser"]["unknown"], doc["authoriser"]["total"]);
        assert_eq!(doc["authoriser"]["state"], "unknown");
        assert_eq!(doc["tokenOwnerSafe"]["state"], "pass");
    }

    /// Unread source leaves every check that compares against it with nothing
    /// to compare: unknown, never a pass on an empty expectation. What still
    /// answers is only what stands on no source: the empty module list, the
    /// standard initializer slot, and no `DEFAULT_ADMIN` for the pinned Safe or
    /// the deploy key.
    #[test]
    fn unread_source_is_unknown_not_a_pass() {
        let empty = OwnerSources {
            safe_lib: "",
            auth_lib: "",
            v4_lib: "",
            overrides: "",
        };
        let e = parse_expectations(&empty, "", Vec::new());
        let doc = chain_doc(&e, &ethereum(), &[], &healthy());
        let passed: Vec<String> = ["tokenOwnerSafe", "authoriser", "orchestrator", "frozen"]
            .iter()
            .flat_map(|s| doc[s]["checks"].as_array().unwrap().iter())
            .filter(|c| c["status"] == "pass")
            .map(|c| {
                let check = c["check"].as_str().unwrap();
                match c["account"].as_str() {
                    Some(a) => format!("{check} {a}"),
                    None => check.to_string(),
                }
            })
            .collect();
        assert_eq!(
            passed,
            ["modules", "initialized", "role safe", "role DEPLOY_KEY"]
        );
        assert_eq!(doc["failed"], 0);
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn the_document_sums_its_chains() {
        assert_eq!(build_state("o", "r", Vec::new()), None);
        let e = expectations();
        let mut bad = healthy();
        bad.threshold.insert(SAFE.to_lowercase(), 1);
        let docs: Vec<Value> = pins()
            .iter()
            .map(|p| {
                if p.network == "ethereum" {
                    chain_doc(&e, p, &[], &bad)
                } else {
                    chain_doc(&e, p, &["0_1_1"], &NoReads)
                }
            })
            .collect();
        let doc = build_state("S01-Issuer", "st0x.deploy", docs).unwrap();
        assert_eq!(doc["deployKey"], DEPLOY_KEY);
        assert_eq!(doc["chains"][0]["network"], "base");
        assert_eq!(doc["chains"][1]["network"], "ethereum");
        assert_eq!(doc["failed"], 1);
        assert_eq!(doc["passed"], 31);
        assert_eq!(doc["unknown"], 31);
        assert_eq!(doc["total"], 63);
        assert_eq!(doc["state"], "fail");
    }
}
