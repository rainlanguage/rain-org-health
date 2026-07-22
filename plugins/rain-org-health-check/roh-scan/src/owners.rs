//! Reads st0x.deploy's owner / privileged-address constants so the Deployments
//! page can enumerate who controls production. Pure parsing + assembly live here
//! and are unit-tested; the network fetch (`gh_file`) is in main.rs.
//!
//! The addresses are the volatile fact and come from the repo (so the dashboard
//! tracks the source of truth); the role labels, grouping, and "what it controls"
//! notes are the stable curation and live here. Each address is read from the
//! file that declares it as a LITERAL — never from an aliasing re-export — so
//! parsing never has to resolve `= OtherLib.CONST;`.

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

/// Assemble the `deploymentOwners` document from the four st0x.deploy library
/// sources. Returns `None` when the anchor (the Base token-owner Safe) can't be
/// resolved — i.e. the repo was unreachable or the constant moved — so the page
/// shows an honest "unavailable" state instead of a table of nulls.
///
/// - `safe_lib`  = `src/lib/LibSafeInvariants.sol` (the Safes, signers, threshold)
/// - `auth_lib`  = `src/lib/LibAuthoriserInvariants.sol` (authoriser + grantees)
/// - `v4_lib`    = `src/generated/LibProdDeployV4.sol` (deploy EOA, V4 clone)
/// - `overrides` = `src/lib/LibProdDeployV2BaseOverrides.sol` (bricked V2 beacons)
/// An address constant that resolved to the zero address — a pin declared but
/// not yet hydrated. Distinct from `None` (the constant is absent entirely).
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

pub fn build_owners(
    org: &str,
    repo: &str,
    safe_lib: &str,
    auth_lib: &str,
    v4_lib: &str,
    overrides: &str,
    onchain: Option<&OnChainSafe>,
    live_authoriser: Option<&str>,
) -> Option<serde_json::Value> {
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
                if is_unhydrated(v4_clone_ethereum.as_ref()) { "pin declared, clone not yet deployed" }
                else { "the Ethereum bootstrap's authoriser clone" }),
            entry("Service grantee", addr(auth_lib, "GRANTEE_SERVICE_1C66"), "", "active", "external service EOA granted deposit / withdraw / certify"),
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
            "o", "r", SAFE_LIB, AUTH_LIB, &v4_zero, OVERRIDES, None, None,
        )
        .unwrap();
        let auth = v["groups"][2]["entries"].as_array().unwrap();
        let eth = auth
            .iter()
            .find(|e| e["role"] == "V4 authoriser clone" && e["network"] == "ethereum")
            .expect("the ethereum row must exist even unhydrated");
        assert_eq!(eth["status"], "pending");
        assert!(
            eth["note"].as_str().unwrap().contains("not yet deployed"),
            "an unhydrated pin must say so: {}",
            eth["note"]
        );
    }

    #[test]
    fn build_owners_is_none_without_the_anchor() {
        // Repo unreachable / anchor constant moved → no owners doc at all.
        assert_eq!(
            build_owners("o", "r", "", AUTH_LIB, V4_LIB, OVERRIDES, None, None),
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
            safe_no_eth,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
        let v = build_owners("o", "r", three, AUTH_LIB, V4_LIB, OVERRIDES, None, None).unwrap();
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
        let v = build_owners("o", "r", SAFE_LIB, AUTH_LIB, V4_LIB, OVERRIDES, None, None).unwrap();
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
            SAFE_LIB,
            AUTH_LIB,
            V4_LIB,
            OVERRIDES,
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
}
