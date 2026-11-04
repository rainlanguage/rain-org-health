//! st0x.deploy's published `expected-state.json`: per chain, what its CI asserts
//! live — each beacon's codehash, implementation and owner, the token-owner
//! Safe's owners and threshold, the timelock, the orchestrator, both authoriser
//! clones, every Zoltu suite, and each token's metadata, legs, beacons, proxy
//! codehashes, owner and authoriser. The scan expects exactly these.

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
    pub codehash: String,
    pub implementation: String,
    pub owner: String,
}

/// A contract the file pins by address and runtime codehash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedCode {
    pub address: String,
    pub codehash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedTimelock {
    pub code: ExpectedCode,
    pub min_delay: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedOrchestrator {
    pub set_deployer: String,
    pub beacon: String,
    pub beacon_codehash: String,
    pub implementation: String,
    pub owner: String,
    pub instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSafe {
    pub address: String,
    pub owners: Vec<String>,
    pub threshold: u64,
    /// Any one of these is on target.
    pub proxy_codehashes: Vec<String>,
    /// Index-paired with `singleton_codehashes`; any one pair is on target.
    pub singletons: Vec<String>,
    pub singleton_codehashes: Vec<String>,
    pub version: String,
    pub modules: Vec<String>,
    /// `None` when the file carries the zero address: no guard set.
    pub guard: Option<String>,
    pub fallback_handler: String,
    pub fallback_handler_codehash: String,
}

/// A released Zoltu suite declared on the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedZoltu {
    pub key: String,
    pub code: ExpectedCode,
}

/// Each `renamed_*` applies once the chain records `rename_migration`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedToken {
    pub underlying: String,
    pub name: String,
    pub renamed_name: String,
    pub symbol: String,
    pub decimals: u64,
    pub wrapped_token_vault_name: String,
    pub wrapped_token_vault_renamed_name: String,
    pub wrapped_token_vault_symbol: String,
    pub wrapped_token_vault_renamed_symbol: String,
    pub wrapped_token_vault_decimals: u64,
    pub receipt_vault: String,
    pub receipt: String,
    pub wrapped_token_vault: String,
    pub receipt_vault_receipt: String,
    pub wrapped_token_vault_asset: String,
    pub receipt_vault_beacon: BeaconAt,
    pub receipt_beacon: BeaconAt,
    pub wrapped_token_vault_beacon: String,
    pub receipt_vault_codehash: String,
    pub receipt_codehash: String,
    pub wrapped_token_vault_codehash: String,
    pub owner: String,
    pub authoriser: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChain {
    pub chain_id: u64,
    pub network: String,
    /// `None` when the file carries the zero address.
    pub beacon_set_deployer: Option<String>,
    pub beacons: Vec<ExpectedBeacon>,
    pub safe: ExpectedSafe,
    pub timelock: ExpectedTimelock,
    pub orchestrator: ExpectedOrchestrator,
    pub authoriser: ExpectedCode,
    pub eu_authoriser: ExpectedCode,
    pub zoltu: Vec<ExpectedZoltu>,
    pub rename_migration: String,
    /// In file order.
    pub tokens: Vec<ExpectedToken>,
}

impl ExpectedChain {
    pub fn authoriser_for(&self, receipt_vault: &str) -> Option<&str> {
        self.tokens
            .iter()
            .find(|t| t.receipt_vault.eq_ignore_ascii_case(receipt_vault))
            .map(|t| t.authoriser.as_str())
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

fn is_bytes32(s: &str) -> bool {
    s.len() == 66 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

fn is_zero(s: &str) -> bool {
    s[2..].chars().all(|c| c == '0')
}

/// Parse the file. Base first, then by chain id.
///
/// Every expected value must be a non-zero address: a zero expectation would
/// match a contract answering the zero address and report it as on target, and
/// a zero codehash would match an address with no account.
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
        let codehash = |v: &serde_json::Value, field: &'static str| {
            v.as_str()
                .filter(|h| is_bytes32(h) && !is_zero(h))
                .map(str::to_string)
                .ok_or_else(|| bad(field))
        };
        let code = |v: &serde_json::Value, field: &'static str, hash_field: &'static str| {
            Ok::<_, ExpectedError>(ExpectedCode {
                address: nonzero(&v["address"], field)?,
                codehash: codehash(&v["codehash"], hash_field)?,
            })
        };
        let list = |v: &serde_json::Value, field: &'static str| {
            v.as_array().cloned().ok_or_else(|| bad(field))
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
                    codehash: codehash(&b["codehash"], "beacons[].codehash")?,
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
        let proxy_codehashes = list(&s["proxyCodehashes"], "safe.proxyCodehashes")?
            .iter()
            .map(|h| codehash(h, "safe.proxyCodehashes"))
            .collect::<Result<Vec<_>, _>>()?;
        if proxy_codehashes.is_empty() {
            return Err(bad("safe.proxyCodehashes"));
        }
        let singletons = list(&s["singletons"], "safe.singletons")?
            .iter()
            .map(|a| nonzero(a, "safe.singletons"))
            .collect::<Result<Vec<_>, _>>()?;
        if singletons.is_empty() {
            return Err(bad("safe.singletons"));
        }
        let singleton_codehashes = list(&s["singletonCodehashes"], "safe.singletonCodehashes")?
            .iter()
            .map(|h| codehash(h, "safe.singletonCodehashes"))
            .collect::<Result<Vec<_>, _>>()?;
        if singleton_codehashes.len() != singletons.len() {
            return Err(bad("safe.singletonCodehashes"));
        }
        let version = s["version"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| bad("safe.version"))?
            .to_string();
        let modules = list(&s["modules"], "safe.modules")?
            .iter()
            .map(|a| nonzero(a, "safe.modules"))
            .collect::<Result<Vec<_>, _>>()?;
        let guard = addr(&s["guard"], "safe.guard")?;
        let safe = ExpectedSafe {
            address: nonzero(&s["address"], "safe.address")?,
            owners,
            threshold,
            proxy_codehashes,
            singletons,
            singleton_codehashes,
            version,
            modules,
            guard: (!is_zero(&guard)).then_some(guard),
            fallback_handler: nonzero(&s["fallbackHandler"], "safe.fallbackHandler")?,
            fallback_handler_codehash: codehash(
                &s["fallbackHandlerCodehash"],
                "safe.fallbackHandlerCodehash",
            )?,
        };
        let t = &c["timelock"];
        let timelock = ExpectedTimelock {
            code: code(t, "timelock.address", "timelock.codehash")?,
            min_delay: t["minDelay"]
                .as_u64()
                .ok_or_else(|| bad("timelock.minDelay"))?,
        };
        let o = &c["orchestrator"];
        let orchestrator = ExpectedOrchestrator {
            set_deployer: nonzero(&o["setDeployer"], "orchestrator.setDeployer")?,
            beacon: nonzero(&o["beacon"], "orchestrator.beacon")?,
            beacon_codehash: codehash(&o["beaconCodehash"], "orchestrator.beaconCodehash")?,
            implementation: nonzero(&o["implementation"], "orchestrator.implementation")?,
            owner: nonzero(&o["owner"], "orchestrator.owner")?,
            instance: nonzero(&o["instance"], "orchestrator.instance")?,
        };
        let authoriser = code(&c["authoriser"], "authoriser.address", "authoriser.codehash")?;
        let eu_authoriser = code(
            &c["euAuthoriser"],
            "euAuthoriser.address",
            "euAuthoriser.codehash",
        )?;
        let zoltu = list(&c["zoltu"], "zoltu")?
            .iter()
            .map(|z| {
                Ok(ExpectedZoltu {
                    key: z["key"]
                        .as_str()
                        .filter(|k| !k.is_empty())
                        .ok_or_else(|| bad("zoltu[].key"))?
                        .to_string(),
                    code: code(z, "zoltu[].address", "zoltu[].codehash")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rename_migration = codehash(&c["renameMigration"], "renameMigration")?;
        let text = |v: &serde_json::Value, field: &'static str| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| bad(field))
        };
        let leg = |v: &serde_json::Value, field: &'static str, via: BeaconAt| {
            let a = addr(v, field)?;
            match (is_zero(&a), &beacon_set_deployer) {
                (false, _) => Ok(BeaconAt::Address(a)),
                (true, Some(_)) => Ok(via),
                (true, None) => Err(bad(field)),
            }
        };
        let tokens = list(&c["tokens"], "tokens")?
            .iter()
            .map(|t| {
                Ok(ExpectedToken {
                    underlying: text(&t["underlying"], "tokens[].underlying")?,
                    name: text(&t["name"], "tokens[].name")?,
                    renamed_name: text(&t["renamedName"], "tokens[].renamedName")?,
                    symbol: text(&t["symbol"], "tokens[].symbol")?,
                    decimals: t["decimals"]
                        .as_u64()
                        .ok_or_else(|| bad("tokens[].decimals"))?,
                    wrapped_token_vault_name: text(
                        &t["wrappedTokenVaultName"],
                        "tokens[].wrappedTokenVaultName",
                    )?,
                    wrapped_token_vault_renamed_name: text(
                        &t["wrappedTokenVaultRenamedName"],
                        "tokens[].wrappedTokenVaultRenamedName",
                    )?,
                    wrapped_token_vault_symbol: text(
                        &t["wrappedTokenVaultSymbol"],
                        "tokens[].wrappedTokenVaultSymbol",
                    )?,
                    wrapped_token_vault_renamed_symbol: text(
                        &t["wrappedTokenVaultRenamedSymbol"],
                        "tokens[].wrappedTokenVaultRenamedSymbol",
                    )?,
                    wrapped_token_vault_decimals: t["wrappedTokenVaultDecimals"]
                        .as_u64()
                        .ok_or_else(|| bad("tokens[].wrappedTokenVaultDecimals"))?,
                    receipt_vault: nonzero(&t["receiptVault"], "tokens[].receiptVault")?,
                    receipt: nonzero(&t["receipt"], "tokens[].receipt")?,
                    wrapped_token_vault: nonzero(
                        &t["wrappedTokenVault"],
                        "tokens[].wrappedTokenVault",
                    )?,
                    receipt_vault_receipt: nonzero(
                        &t["receiptVaultReceipt"],
                        "tokens[].receiptVaultReceipt",
                    )?,
                    wrapped_token_vault_asset: nonzero(
                        &t["wrappedTokenVaultAsset"],
                        "tokens[].wrappedTokenVaultAsset",
                    )?,
                    receipt_vault_beacon: leg(
                        &t["receiptVaultBeacon"],
                        "tokens[].receiptVaultBeacon",
                        BeaconAt::DeployerReceiptVault,
                    )?,
                    receipt_beacon: leg(
                        &t["receiptBeacon"],
                        "tokens[].receiptBeacon",
                        BeaconAt::DeployerReceipt,
                    )?,
                    wrapped_token_vault_beacon: nonzero(
                        &t["wrappedTokenVaultBeacon"],
                        "tokens[].wrappedTokenVaultBeacon",
                    )?,
                    receipt_vault_codehash: codehash(
                        &t["receiptVaultCodehash"],
                        "tokens[].receiptVaultCodehash",
                    )?,
                    receipt_codehash: codehash(&t["receiptCodehash"], "tokens[].receiptCodehash")?,
                    wrapped_token_vault_codehash: codehash(
                        &t["wrappedTokenVaultCodehash"],
                        "tokens[].wrappedTokenVaultCodehash",
                    )?,
                    owner: nonzero(&t["owner"], "tokens[].owner")?,
                    authoriser: nonzero(&t["authoriser"], "tokens[].authoriser")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(ExpectedChain {
            chain_id,
            network,
            beacon_set_deployer,
            beacons,
            safe,
            timelock,
            orchestrator,
            authoriser,
            eu_authoriser,
            zoltu,
            rename_migration,
            tokens,
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
    const PROXY_HASH: &str = "0xd7d408ebcd99b2b70be43e20253d6d92a8ea8fab29bd3be7f55b10032331fb4c";
    const SINGLETON_HASH: &str = "0x1fe2df852ba3299d6534ef416eefa406e56ced995bca886ab7a553e6d0c5e1c4";

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
                          "threshold": 2,
                          "proxyCodehashes": ["{PROXY_HASH}"],
                          "singletons": ["0x41675C099F32341bf84BFc5382aF534df5C7461a", "0x29fcB43b46531BcA003ddC8FCB67FFE91900C762"],
                          "singletonCodehashes": ["{SINGLETON_HASH}", "{SINGLETON_HASH}"],
                          "version": "1.4.1", "modules": [], "guard": "{ZERO}",
                          "fallbackHandler": "0xfd0732Dc9E303f09fCEf3a7388Ad10A83459Ec99",
                          "fallbackHandlerCodehash": "{SINGLETON_HASH}"}},
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
        // An empty roster is named as the roster, not as the threshold it makes impossible.
        let open = r#""owners": ["#;
        let start = three.find(open).unwrap() + open.len();
        let end = start + three[start..].find(']').unwrap();
        let empty_owners = format!("{}{}", &three[..start], &three[end..]);
        assert_eq!(malformed(&empty_owners), "safe.owners");
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
