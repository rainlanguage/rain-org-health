//! st0x.deploy's published `expected-state.json`: per chain, what its CI asserts
//! live — each beacon's implementation and owner, the token-owner Safe's owners
//! and threshold, and each token's authoriser. The scan expects exactly these.

use crate::owners::REFERENCE_NETWORK;

pub const EXPECTED_STATE_PATH: &str = "expected-state.json";

/// The only schema version this reader understands.
const SCHEMA_VERSION: u64 = 1;

/// The file's beacon index order.
pub const BEACON_NAMES: [&str; 4] = [
    "Receipt beacon",
    "Receipt-vault beacon",
    "Wrapped-token-vault beacon",
    "Orchestrator beacon",
];

/// Where a beacon's address comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeaconAt {
    Address(String),
    /// The file's `0x0`: read from the chain's beacon-set deployer.
    DeployerReceipt,
    DeployerReceiptVault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedBeacon {
    pub name: &'static str,
    pub at: BeaconAt,
    pub implementation: String,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSafe {
    pub address: String,
    pub owners: Vec<String>,
    pub threshold: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChain {
    pub chain_id: u64,
    pub network: String,
    /// `None` when the file carries the zero address.
    pub beacon_set_deployer: Option<String>,
    pub beacons: Vec<ExpectedBeacon>,
    pub safe: ExpectedSafe,
    /// `(receipt vault lowercased, authoriser)`, in file order.
    pub authorisers: Vec<(String, String)>,
}

impl ExpectedChain {
    pub fn authoriser_for(&self, receipt_vault: &str) -> Option<&str> {
        self.authorisers
            .iter()
            .find(|(v, _)| v.eq_ignore_ascii_case(receipt_vault))
            .map(|(_, a)| a.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedError {
    Empty,
    NotJson,
    UnsupportedVersion(Option<u64>),
    NoChains,
    NoReferenceChain,
    Malformed { chain: String, field: &'static str },
}

impl std::fmt::Display for ExpectedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpectedError::Empty => write!(f, "{EXPECTED_STATE_PATH} could not be fetched"),
            ExpectedError::NotJson => write!(f, "{EXPECTED_STATE_PATH} is not valid JSON"),
            ExpectedError::UnsupportedVersion(v) => write!(
                f,
                "{EXPECTED_STATE_PATH} has schema version {}, this scanner reads {SCHEMA_VERSION}",
                v.map_or("none".to_string(), |v| v.to_string())
            ),
            ExpectedError::NoChains => write!(f, "{EXPECTED_STATE_PATH} lists no chains"),
            ExpectedError::NoReferenceChain => {
                write!(f, "{EXPECTED_STATE_PATH} has no {REFERENCE_NETWORK} chain")
            }
            ExpectedError::Malformed { chain, field } => write!(
                f,
                "{EXPECTED_STATE_PATH} chain {chain} has a missing or malformed {field}"
            ),
        }
    }
}

fn is_address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

fn is_zero(s: &str) -> bool {
    s[2..].chars().all(|c| c == '0')
}

/// Parse the file. Base first, then by chain id.
///
/// Every expected value must be a non-zero address: a zero expectation would
/// match a contract answering the zero address and report it as on target.
pub fn parse(body: &str) -> Result<Vec<ExpectedChain>, ExpectedError> {
    if body.trim().is_empty() {
        return Err(ExpectedError::Empty);
    }
    let doc: serde_json::Value = serde_json::from_str(body).map_err(|_| ExpectedError::NotJson)?;
    let version = doc["version"].as_u64();
    if version != Some(SCHEMA_VERSION) {
        return Err(ExpectedError::UnsupportedVersion(version));
    }
    let chains = doc["chains"]
        .as_object()
        .filter(|m| !m.is_empty())
        .ok_or(ExpectedError::NoChains)?;
    let mut out = Vec::with_capacity(chains.len());
    for (key, c) in chains {
        let bad = |field| ExpectedError::Malformed {
            chain: key.clone(),
            field,
        };
        let addr = |v: &serde_json::Value, field: &'static str| -> Result<String, ExpectedError> {
            v.as_str()
                .filter(|a| is_address(a))
                .map(str::to_string)
                .ok_or_else(|| bad(field))
        };
        let nonzero = |v: &serde_json::Value, field: &'static str| {
            addr(v, field).and_then(|a| if is_zero(&a) { Err(bad(field)) } else { Ok(a) })
        };
        let chain_id = key.parse::<u64>().map_err(|_| bad("chain id"))?;
        let network = c["network"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| bad("network"))?
            .to_string();
        let deployer = addr(&c["beaconSetDeployer"], "beaconSetDeployer")?;
        let beacon_set_deployer = (!is_zero(&deployer)).then_some(deployer);
        let beacons_raw = c["beacons"]
            .as_array()
            .filter(|b| b.len() == BEACON_NAMES.len())
            .ok_or_else(|| bad("beacons"))?;
        let beacons = beacons_raw
            .iter()
            .zip(BEACON_NAMES)
            .enumerate()
            .map(|(i, (b, name))| {
                let beacon = addr(&b["beacon"], "beacons[].beacon")?;
                let at = match (is_zero(&beacon), i) {
                    (false, _) => BeaconAt::Address(beacon),
                    (true, _) if beacon_set_deployer.is_none() => {
                        return Err(bad("beaconSetDeployer"))
                    }
                    (true, 0) => BeaconAt::DeployerReceipt,
                    (true, 1) => BeaconAt::DeployerReceiptVault,
                    (true, _) => return Err(bad("beacons[].beacon")),
                };
                Ok(ExpectedBeacon {
                    name,
                    at,
                    implementation: nonzero(&b["implementation"], "beacons[].implementation")?,
                    owner: nonzero(&b["owner"], "beacons[].owner")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let s = &c["safe"];
        let owners = s["owners"]
            .as_array()
            .filter(|o| !o.is_empty())
            .ok_or_else(|| bad("safe.owners"))?
            .iter()
            .map(|o| nonzero(o, "safe.owners"))
            .collect::<Result<Vec<_>, _>>()?;
        let threshold = s["threshold"]
            .as_u64()
            .filter(|t| *t >= 1 && *t as usize <= owners.len())
            .ok_or_else(|| bad("safe.threshold"))?;
        let safe = ExpectedSafe {
            address: nonzero(&s["address"], "safe.address")?,
            owners,
            threshold,
        };
        let authorisers = c["tokens"]
            .as_array()
            .ok_or_else(|| bad("tokens"))?
            .iter()
            .map(|t| {
                Ok((
                    nonzero(&t["receiptVault"], "tokens[].receiptVault")?.to_lowercase(),
                    nonzero(&t["authoriser"], "tokens[].authoriser")?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(ExpectedChain {
            chain_id,
            network,
            beacon_set_deployer,
            beacons,
            safe,
            authorisers,
        });
    }
    out.sort_by_key(|c| (c.network != REFERENCE_NETWORK, c.chain_id));
    Ok(out)
}

/// The reference chain's entry, which the Base-only sections read.
pub fn reference(chains: &[ExpectedChain]) -> Result<&ExpectedChain, ExpectedError> {
    chains
        .iter()
        .find(|c| c.network == REFERENCE_NETWORK)
        .ok_or(ExpectedError::NoReferenceChain)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: &str = "0x0000000000000000000000000000000000000000";
    const DEPLOYER: &str = "0xd64246e6b25F745f005E6233e050C9B879E660Dc";
    const ETH_TIMELOCK: &str = "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B";
    const BASE_TIMELOCK: &str = "0x48ba1371A78E6cC54157c63721756ab444510DB3";
    const EU_CLONE: &str = "0xDB9152e46c1D140DB6a6f814461f21c571F3275e";
    const V4_CLONE: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
    const MC_VAULT: &str = "0x6eC2421d74F2AF61b7EE7bCCAE25A3b6a7961Aa0";

    fn beacon(b: &str, imp: &str, owner: &str) -> String {
        format!(r#"{{"beacon": "{b}", "implementation": "{imp}", "owner": "{owner}"}}"#)
    }

    /// Fields as st0x.deploy#510's `expected-state.json` carries them, trimmed
    /// to two chains and two tokens.
    fn chain(
        network: &str,
        deployer: &str,
        beacons: [String; 4],
        owner: &str,
        extra: &str,
    ) -> String {
        format!(
            r#"{{"network": "{network}", "beaconSetDeployer": "{deployer}", "beacons": [{}],
                "safe": {{"address": "0xe70d821f3462a074e63b42d0AaC6523faAe1d611",
                          "owners": ["0x4746095B1Ea1A84446d34448f44e74D3d51f92F2", "0xceC2cb8B8EE4000FFA3F8a7f8E0Fa0A3E3DAb72d", "{owner}"],
                          "threshold": 2}},
                "tokens": [{{"underlying": "MC", "receiptVault": "{MC_VAULT}", "authoriser": "{EU_CLONE}"}},
                           {{"underlying": "MSTR", "receiptVault": "0x8500189061e2206Bc33Bf04DC10fFB1Fe7dED637", "authoriser": "{V4_CLONE}"}}]
                {extra}}}"#,
            beacons.join(",")
        )
    }

    fn eth_beacons() -> [String; 4] {
        [
            beacon(
                ZERO,
                "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E",
                ETH_TIMELOCK,
            ),
            beacon(
                ZERO,
                "0x52174538a83b18462c5cF898016539B086c920C7",
                ETH_TIMELOCK,
            ),
            beacon(
                "0x9FD790f65CA3aF2772358c653F097f0a4c7EE7d2",
                "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099",
                ETH_TIMELOCK,
            ),
            beacon(
                "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613",
                "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72",
                ETH_TIMELOCK,
            ),
        ]
    }

    fn base_beacons() -> [String; 4] {
        [
            beacon(
                "0x86e93c39B095be0B0054C8488E26466Ee027D79a",
                "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E",
                BASE_TIMELOCK,
            ),
            beacon(
                "0xEa084c8F4331CDF3328E772781b59F8A24F28F1A",
                "0x52174538a83b18462c5cF898016539B086c920C7",
                BASE_TIMELOCK,
            ),
            beacon(
                "0x4c2d2d3Bf1232bf0d3FB7123007A9B8444637bC8",
                "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099",
                BASE_TIMELOCK,
            ),
            beacon(
                "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613",
                "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72",
                BASE_TIMELOCK,
            ),
        ]
    }

    fn doc(chains: &[(&str, String)]) -> String {
        let body: Vec<String> = chains
            .iter()
            .map(|(id, c)| format!(r#""{id}": {c}"#))
            .collect();
        format!(r#"{{"version": 1, "chains": {{{}}}}}"#, body.join(","))
    }

    fn good() -> String {
        doc(&[
            (
                "1",
                chain(
                    "ethereum",
                    DEPLOYER,
                    eth_beacons(),
                    "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
                    "",
                ),
            ),
            (
                "8453",
                chain(
                    "base",
                    ZERO,
                    base_beacons(),
                    "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
                    "",
                ),
            ),
        ])
    }

    #[test]
    fn parses_every_expectation_base_first() {
        let got = parse(&good()).unwrap();
        assert_eq!(
            got.iter().map(|c| c.chain_id).collect::<Vec<_>>(),
            [8453, 1]
        );
        let base = &got[0];
        assert_eq!(base.network, "base");
        assert_eq!(base.beacon_set_deployer, None);
        assert_eq!(
            base.beacons.iter().map(|b| b.name).collect::<Vec<_>>(),
            BEACON_NAMES
        );
        assert_eq!(
            base.beacons[1].at,
            BeaconAt::Address("0xEa084c8F4331CDF3328E772781b59F8A24F28F1A".into())
        );
        assert_eq!(
            base.beacons[0].implementation,
            "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E"
        );
        assert!(base.beacons.iter().all(|b| b.owner == BASE_TIMELOCK));
        assert_eq!(base.safe.owners.len(), 3);
        assert_eq!(base.safe.threshold, 2);
        assert_eq!(
            base.safe.address,
            "0xe70d821f3462a074e63b42d0AaC6523faAe1d611"
        );

        let eth = &got[1];
        assert_eq!(eth.beacon_set_deployer.as_deref(), Some(DEPLOYER));
        assert_eq!(eth.beacons[0].at, BeaconAt::DeployerReceipt);
        assert_eq!(eth.beacons[1].at, BeaconAt::DeployerReceiptVault);
        assert_eq!(
            eth.beacons[3].at,
            BeaconAt::Address("0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613".into())
        );
        assert!(eth.beacons.iter().all(|b| b.owner == ETH_TIMELOCK));
    }

    /// #218: each token's own authoriser, matched by receipt vault in any case.
    #[test]
    fn authoriser_for_routes_by_receipt_vault() {
        let got = parse(&good()).unwrap();
        let base = reference(&got).unwrap();
        assert_eq!(base.authoriser_for(MC_VAULT), Some(EU_CLONE));
        assert_eq!(
            base.authoriser_for(&MC_VAULT.to_uppercase().replace("0X", "0x")),
            Some(EU_CLONE)
        );
        assert_eq!(
            base.authoriser_for("0x8500189061e2206bc33bf04dc10ffb1fe7ded637"),
            Some(V4_CLONE)
        );
        assert_eq!(
            base.authoriser_for("0x0000000000000000000000000000000000000001"),
            None
        );
    }

    #[test]
    fn reference_needs_a_base_chain() {
        let only_eth = doc(&[(
            "1",
            chain(
                "ethereum",
                DEPLOYER,
                eth_beacons(),
                "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
                "",
            ),
        )]);
        let got = parse(&only_eth).unwrap();
        assert_eq!(reference(&got), Err(ExpectedError::NoReferenceChain));
    }

    #[test]
    fn rejects_unreadable_files() {
        assert_eq!(parse(""), Err(ExpectedError::Empty));
        assert_eq!(parse("  \n"), Err(ExpectedError::Empty));
        assert_eq!(parse("{"), Err(ExpectedError::NotJson));
        assert_eq!(
            parse(r#"{"chains": {}}"#),
            Err(ExpectedError::UnsupportedVersion(None))
        );
        assert_eq!(
            parse(&good().replacen(r#""version": 1"#, r#""version": 2"#, 1)),
            Err(ExpectedError::UnsupportedVersion(Some(2)))
        );
        assert_eq!(
            parse(r#"{"version": 1, "chains": {}}"#),
            Err(ExpectedError::NoChains)
        );
        assert_eq!(parse(r#"{"version": 1}"#), Err(ExpectedError::NoChains));
    }

    fn malformed(body: &str) -> &'static str {
        match parse(body) {
            Err(ExpectedError::Malformed { field, .. }) => field,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    /// A zero expectation would match a contract answering the zero address.
    #[test]
    fn rejects_zero_and_malformed_expectations() {
        let owner = "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2";
        let base = |beacons: [String; 4], owner: &str, extra: &str| {
            doc(&[("8453", chain("base", ZERO, beacons, owner, extra))])
        };
        let mut b = base_beacons();
        b[0] = beacon(
            "0x86e93c39B095be0B0054C8488E26466Ee027D79a",
            ZERO,
            BASE_TIMELOCK,
        );
        assert_eq!(malformed(&base(b, owner, "")), "beacons[].implementation");
        let mut b = base_beacons();
        b[3] = beacon(
            "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613",
            "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72",
            ZERO,
        );
        assert_eq!(malformed(&base(b, owner, "")), "beacons[].owner");
        let mut b = base_beacons();
        b[2] = beacon(
            "0x9FD7",
            "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099",
            BASE_TIMELOCK,
        );
        assert_eq!(malformed(&base(b, owner, "")), "beacons[].beacon");
        // A zero beacon needs a deployer to resolve it from.
        let mut b = base_beacons();
        b[0] = beacon(
            ZERO,
            "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E",
            BASE_TIMELOCK,
        );
        assert_eq!(malformed(&base(b, owner, "")), "beaconSetDeployer");
        // Only the receipt and receipt-vault slots resolve through the deployer.
        for slot in [2, 3] {
            let mut b = eth_beacons();
            b[slot] = beacon(
                ZERO,
                "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099",
                ETH_TIMELOCK,
            );
            let body = doc(&[("1", chain("ethereum", DEPLOYER, b, owner, ""))]);
            assert_eq!(malformed(&body), "beacons[].beacon", "slot {slot}");
        }
        assert_eq!(malformed(&base(base_beacons(), ZERO, "")), "safe.owners");
        assert_eq!(
            malformed(&base(base_beacons(), "0x5CCd", "")),
            "safe.owners"
        );
        let three = good();
        assert_eq!(
            malformed(&three.replace(r#""threshold": 2"#, r#""threshold": 0"#)),
            "safe.threshold"
        );
        assert_eq!(
            malformed(&three.replace(r#""threshold": 2"#, r#""threshold": 4"#)),
            "safe.threshold"
        );
        assert!(parse(&three.replace(r#""threshold": 2"#, r#""threshold": 3"#)).is_ok());
        assert_eq!(
            malformed(&three.replacen("0xe70d821f3462a074e63b42d0AaC6523faAe1d611", ZERO, 1)),
            "safe.address"
        );
        assert_eq!(
            malformed(&three.replace(EU_CLONE, ZERO)),
            "tokens[].authoriser"
        );
        assert_eq!(
            malformed(&three.replace(MC_VAULT, ZERO)),
            "tokens[].receiptVault"
        );
        assert_eq!(
            malformed(&three.replace(r#""network": "base""#, r#""network": """#)),
            "network"
        );
        assert_eq!(
            malformed(&three.replace(r#""8453":"#, r#""base":"#)),
            "chain id"
        );
        assert_eq!(
            malformed(&three.replace(
                &format!(r#""beaconSetDeployer": "{ZERO}""#),
                r#""beaconSetDeployer": "0x0""#
            )),
            "beaconSetDeployer"
        );
    }

    #[test]
    fn rejects_a_beacon_list_of_the_wrong_length_and_missing_sections() {
        let three = doc(&[(
            "8453",
            chain(
                "base",
                ZERO,
                base_beacons(),
                "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
                "",
            ),
        )]);
        let short = three.replace(&format!(",{}", base_beacons()[3]), "");
        assert_eq!(malformed(&short), "beacons");
        let body = r#"{"version": 1, "chains": {"8453": {"network": "base", "beaconSetDeployer": "0x0000000000000000000000000000000000000000"}}}"#;
        assert_eq!(malformed(body), "beacons");
        let no_tokens = three.replace(r#""tokens": ["#, r#""tokenz": ["#);
        assert_eq!(malformed(&no_tokens), "tokens");
        let no_owners = three.replace(r#""owners": ["#, r#""ownerz": ["#);
        assert_eq!(malformed(&no_owners), "safe.owners");
    }

    #[test]
    fn errors_name_the_file() {
        assert_eq!(
            ExpectedError::Empty.to_string(),
            "expected-state.json could not be fetched"
        );
        assert_eq!(
            ExpectedError::UnsupportedVersion(Some(2)).to_string(),
            "expected-state.json has schema version 2, this scanner reads 1"
        );
        assert_eq!(
            ExpectedError::UnsupportedVersion(None).to_string(),
            "expected-state.json has schema version none, this scanner reads 1"
        );
        assert_eq!(
            ExpectedError::NoReferenceChain.to_string(),
            "expected-state.json has no base chain"
        );
        assert_eq!(
            ExpectedError::Malformed {
                chain: "8453".into(),
                field: "safe.owners"
            }
            .to_string(),
            "expected-state.json chain 8453 has a missing or malformed safe.owners"
        );
    }
}
