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
pub fn build_owners(
    org: &str,
    repo: &str,
    safe_lib: &str,
    auth_lib: &str,
    v4_lib: &str,
    overrides: &str,
) -> Option<serde_json::Value> {
    let addr = parse_address_constant;

    // Anchor: without the Base Safe there is nothing meaningful to show.
    let base_safe = addr(safe_lib, "STOX_TOKEN_OWNER_SAFE")?;
    let eth_safe = addr(safe_lib, "STOX_TOKEN_OWNER_SAFE_ETHEREUM");
    let threshold = parse_uint_constant(safe_lib, "STOX_TOKEN_OWNER_SAFE_THRESHOLD").unwrap_or(3);

    // Read the signer roster from the constants rather than assuming its size:
    // walk STOX_TOKEN_OWNER_SAFE_OWNER_1, _2, … and stop at the first index that
    // isn't defined, so the count tracks the actual Safe if the roster ever
    // changes. The `..=64` is a belt-and-braces bound, far above any real multisig.
    let mut signers: Vec<serde_json::Value> = Vec::new();
    for i in 1..=64 {
        match addr(safe_lib, &format!("STOX_TOKEN_OWNER_SAFE_OWNER_{i}")) {
            Some(a) => signers.push(entry(&format!("Signer {i}"), Some(a), "", "active", "")),
            None => break,
        }
    }
    let signer_count = signers.len();

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
        "note": "The same EOAs govern the Safe on both chains.",
        "entries": signers,
    });

    let authoriser = json!({
        "id": "authoriser",
        "title": "Operational access — authoriser",
        "note": "Every production receipt vault delegates deposit / withdraw / certify authorization to this authoriser.",
        "entries": [
            entry("Live authoriser clone", addr(auth_lib, "STOX_PROD_AUTHORISER"), "base", "active", "the authorizer() target vaults point at"),
            entry("Authoriser implementation", addr(auth_lib, "STOX_PROD_AUTHORISER_IMPL"), "base", "active", "implementation behind the clone"),
            entry("V4 pending-swap clone", addr(v4_lib, "STOX_PROD_AUTHORISER_V4_CLONE"), "base", "pending", "the V4 upgrade rewires every vault onto this"),
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
        // The V4 clone is flagged pending, not active.
        let auth = groups[2]["entries"].as_array().unwrap();
        let v4 = auth
            .iter()
            .find(|e| e["role"] == "V4 pending-swap clone")
            .unwrap();
        assert_eq!(v4["status"], "pending");
    }

    #[test]
    fn build_owners_is_none_without_the_anchor() {
        // Repo unreachable / anchor constant moved → no owners doc at all.
        assert_eq!(
            build_owners("o", "r", "", AUTH_LIB, V4_LIB, OVERRIDES),
            None
        );
    }

    #[test]
    fn unresolved_entry_becomes_null_not_dropped() {
        // The Ethereum Safe missing from the source surfaces as a null address in
        // its row rather than vanishing.
        let safe_no_eth = "address internal constant STOX_TOKEN_OWNER_SAFE = 0xe70d821f3462a074e63b42d0AaC6523faAe1d611;";
        let v = build_owners("o", "r", safe_no_eth, AUTH_LIB, V4_LIB, OVERRIDES).unwrap();
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
        let v = build_owners("o", "r", three, AUTH_LIB, V4_LIB, OVERRIDES).unwrap();
        assert_eq!(v["signerCount"], 3);
        assert_eq!(v["groups"][1]["entries"].as_array().unwrap().len(), 3);
        assert_eq!(v["groups"][1]["title"], "Safe signers (3-of-3)");
    }
}
