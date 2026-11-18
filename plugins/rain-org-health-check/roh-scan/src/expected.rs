//! st0x.deploy's published `expected-state.json`: per chain, the facts it emits,
//! checked live here — each beacon's codehash, implementation and owner, the token-owner
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedBeacon {
    pub name: &'static str,
    pub address: String,
    pub codehash: String,
    pub implementation: String,
    pub renamed_implementation: String,
    pub owner: String,
}

impl ExpectedBeacon {
    /// The implementation pinned for whether the chain records the rename;
    /// `None` (unread) pins nothing.
    pub fn expected_implementation(&self, renamed: Option<bool>) -> Option<&str> {
        match renamed {
            Some(true) => Some(&self.renamed_implementation),
            Some(false) => Some(&self.implementation),
            None => None,
        }
    }
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
    pub receipt_manager: String,
    pub wrapped_token_vault_asset: String,
    pub receipt_vault_beacon: String,
    pub receipt_beacon: String,
    pub wrapped_token_vault_beacon: String,
    pub receipt_vault_codehash: String,
    pub receipt_codehash: String,
    pub wrapped_token_vault_codehash: String,
    pub owner: String,
    pub authoriser: String,
    pub asset_id: Option<String>,
    pub listing_ticker: Option<String>,
    pub listing_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChain {
    pub chain_id: u64,
    pub network: String,
    pub beacon_set_deployer: String,
    pub beacons: Vec<ExpectedBeacon>,
    pub safe: ExpectedSafe,
    pub timelock: ExpectedTimelock,
    pub orchestrator: ExpectedOrchestrator,
    pub authoriser: ExpectedCode,
    pub eu_authoriser: ExpectedCode,
    pub zoltu: Vec<ExpectedZoltu>,
    pub rename_migration: String,
    /// tQSEP, present exactly on the reference network.
    pub qsep: Option<ExpectedQsep>,
    /// In file order.
    pub tokens: Vec<ExpectedToken>,
}

/// tQSEP: outside the production token list, on the shared reference-network beacons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedQsep {
    pub receipt_vault: String,
    pub receipt: String,
    pub wrapped_token_vault: String,
    pub receipt_vault_receipt: String,
    pub receipt_manager: String,
    pub wrapped_token_vault_asset: String,
    pub receipt_vault_beacon: String,
    pub receipt_beacon: String,
    pub wrapped_token_vault_beacon: String,
    pub receipt_vault_codehash: String,
    pub receipt_codehash: String,
    pub wrapped_token_vault_codehash: String,
    pub owner: String,
    pub authoriser: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedError {
    Empty,
    NotJson,
    UnsupportedVersion(Option<u64>),
    NoChains,
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

/// The `section` object's chain entries from a body of the supported version.
fn chain_entries(
    body: &str,
    section: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ExpectedError> {
    if body.trim().is_empty() {
        return Err(ExpectedError::Empty);
    }
    let mut doc: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ExpectedError::NotJson)?;
    let version = doc["version"].as_u64();
    if version != Some(SCHEMA_VERSION) {
        return Err(ExpectedError::UnsupportedVersion(version));
    }
    match doc[section].take() {
        serde_json::Value::Object(chains) if !chains.is_empty() => Ok(chains),
        _ => Err(ExpectedError::NoChains),
    }
}

/// Field validators for one chain entry, keyed by its chain id.
struct ChainFields<'a>(&'a str);

impl ChainFields<'_> {
    fn bad(&self, field: &'static str) -> ExpectedError {
        ExpectedError::Malformed {
            chain: self.0.to_string(),
            field,
        }
    }

    fn addr(&self, v: &serde_json::Value, field: &'static str) -> Result<String, ExpectedError> {
        v.as_str()
            .filter(|a| is_address(a))
            .map(str::to_string)
            .ok_or_else(|| self.bad(field))
    }

    fn nonzero(&self, v: &serde_json::Value, field: &'static str) -> Result<String, ExpectedError> {
        self.addr(v, field).and_then(|a| {
            if is_zero(&a) {
                Err(self.bad(field))
            } else {
                Ok(a)
            }
        })
    }

    fn codehash(
        &self,
        v: &serde_json::Value,
        field: &'static str,
    ) -> Result<String, ExpectedError> {
        v.as_str()
            .filter(|h| is_bytes32(h) && !is_zero(h))
            .map(str::to_string)
            .ok_or_else(|| self.bad(field))
    }
}

/// Parse the file. Base first, then by chain id.
///
/// Every expected value must be a non-zero address: a zero expectation would
/// match a contract answering the zero address and report it as on target, and
/// a zero codehash would match an address with no account.
pub fn parse(body: &str) -> Result<Vec<ExpectedChain>, ExpectedError> {
    let chains = chain_entries(body, "chains")?;
    let mut out = Vec::with_capacity(chains.len());
    for (key, c) in &chains {
        let f = ChainFields(key);
        let code = |v: &serde_json::Value, field: &'static str, hash_field: &'static str| {
            Ok::<_, ExpectedError>(ExpectedCode {
                address: f.nonzero(&v["address"], field)?,
                codehash: f.codehash(&v["codehash"], hash_field)?,
            })
        };
        let list = |v: &serde_json::Value, field: &'static str| {
            v.as_array().cloned().ok_or_else(|| f.bad(field))
        };
        let chain_id = key.parse::<u64>().map_err(|_| f.bad("chain id"))?;
        let network = c["network"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| f.bad("network"))?
            .to_string();
        let beacon_set_deployer = f.nonzero(&c["beaconSetDeployer"], "beaconSetDeployer")?;
        let beacons_raw = c["beacons"]
            .as_array()
            .filter(|b| b.len() == BEACON_NAMES.len())
            .ok_or_else(|| f.bad("beacons"))?;
        let beacons = beacons_raw
            .iter()
            .zip(BEACON_NAMES)
            .map(|(b, name)| {
                Ok(ExpectedBeacon {
                    name,
                    address: f.nonzero(&b["beacon"], "beacons[].beacon")?,
                    codehash: f.codehash(&b["codehash"], "beacons[].codehash")?,
                    implementation: f.nonzero(&b["implementation"], "beacons[].implementation")?,
                    renamed_implementation: f.nonzero(
                        &b["renamedImplementation"],
                        "beacons[].renamedImplementation",
                    )?,
                    owner: f.nonzero(&b["owner"], "beacons[].owner")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let s = &c["safe"];
        let owners = s["owners"]
            .as_array()
            .filter(|o| !o.is_empty())
            .ok_or_else(|| f.bad("safe.owners"))?
            .iter()
            .map(|o| f.nonzero(o, "safe.owners"))
            .collect::<Result<Vec<_>, _>>()?;
        let threshold = s["threshold"]
            .as_u64()
            .filter(|t| *t >= 1 && *t as usize <= owners.len())
            .ok_or_else(|| f.bad("safe.threshold"))?;
        let proxy_codehashes = list(&s["proxyCodehashes"], "safe.proxyCodehashes")?
            .iter()
            .map(|h| f.codehash(h, "safe.proxyCodehashes"))
            .collect::<Result<Vec<_>, _>>()?;
        if proxy_codehashes.is_empty() {
            return Err(f.bad("safe.proxyCodehashes"));
        }
        let singletons = list(&s["singletons"], "safe.singletons")?
            .iter()
            .map(|a| f.nonzero(a, "safe.singletons"))
            .collect::<Result<Vec<_>, _>>()?;
        if singletons.is_empty() {
            return Err(f.bad("safe.singletons"));
        }
        let singleton_codehashes = list(&s["singletonCodehashes"], "safe.singletonCodehashes")?
            .iter()
            .map(|h| f.codehash(h, "safe.singletonCodehashes"))
            .collect::<Result<Vec<_>, _>>()?;
        if singleton_codehashes.len() != singletons.len() {
            return Err(f.bad("safe.singletonCodehashes"));
        }
        let version = s["version"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| f.bad("safe.version"))?
            .to_string();
        let modules = list(&s["modules"], "safe.modules")?
            .iter()
            .map(|a| f.nonzero(a, "safe.modules"))
            .collect::<Result<Vec<_>, _>>()?;
        let guard = f.addr(&s["guard"], "safe.guard")?;
        let safe = ExpectedSafe {
            address: f.nonzero(&s["address"], "safe.address")?,
            owners,
            threshold,
            proxy_codehashes,
            singletons,
            singleton_codehashes,
            version,
            modules,
            guard: (!is_zero(&guard)).then_some(guard),
            fallback_handler: f.nonzero(&s["fallbackHandler"], "safe.fallbackHandler")?,
            fallback_handler_codehash: f.codehash(
                &s["fallbackHandlerCodehash"],
                "safe.fallbackHandlerCodehash",
            )?,
        };
        let t = &c["timelock"];
        let timelock = ExpectedTimelock {
            code: code(t, "timelock.address", "timelock.codehash")?,
            min_delay: t["minDelay"]
                .as_u64()
                .ok_or_else(|| f.bad("timelock.minDelay"))?,
        };
        let o = &c["orchestrator"];
        let orchestrator = ExpectedOrchestrator {
            set_deployer: f.nonzero(&o["setDeployer"], "orchestrator.setDeployer")?,
            beacon: f.nonzero(&o["beacon"], "orchestrator.beacon")?,
            beacon_codehash: f.codehash(&o["beaconCodehash"], "orchestrator.beaconCodehash")?,
            implementation: f.nonzero(&o["implementation"], "orchestrator.implementation")?,
            owner: f.nonzero(&o["owner"], "orchestrator.owner")?,
            instance: f.nonzero(&o["instance"], "orchestrator.instance")?,
        };
        let authoriser = code(
            &c["authoriser"],
            "authoriser.address",
            "authoriser.codehash",
        )?;
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
                        .ok_or_else(|| f.bad("zoltu[].key"))?
                        .to_string(),
                    code: code(z, "zoltu[].address", "zoltu[].codehash")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rename_migration = f.codehash(&c["renameMigration"], "renameMigration")?;
        let text = |v: &serde_json::Value, field: &'static str| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| f.bad(field))
        };
        // Optional until st0x.deploy carries listing facts for every token.
        let opt_text =
            |v: &serde_json::Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
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
                        .ok_or_else(|| f.bad("tokens[].decimals"))?,
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
                        .ok_or_else(|| f.bad("tokens[].wrappedTokenVaultDecimals"))?,
                    receipt_vault: f.nonzero(&t["receiptVault"], "tokens[].receiptVault")?,
                    receipt: f.nonzero(&t["receipt"], "tokens[].receipt")?,
                    wrapped_token_vault: f
                        .nonzero(&t["wrappedTokenVault"], "tokens[].wrappedTokenVault")?,
                    receipt_vault_receipt: f
                        .nonzero(&t["receiptVaultReceipt"], "tokens[].receiptVaultReceipt")?,
                    receipt_manager: f.nonzero(&t["receiptManager"], "tokens[].receiptManager")?,
                    wrapped_token_vault_asset: f.nonzero(
                        &t["wrappedTokenVaultAsset"],
                        "tokens[].wrappedTokenVaultAsset",
                    )?,
                    receipt_vault_beacon: f
                        .nonzero(&t["receiptVaultBeacon"], "tokens[].receiptVaultBeacon")?,
                    receipt_beacon: f.nonzero(&t["receiptBeacon"], "tokens[].receiptBeacon")?,
                    wrapped_token_vault_beacon: f.nonzero(
                        &t["wrappedTokenVaultBeacon"],
                        "tokens[].wrappedTokenVaultBeacon",
                    )?,
                    receipt_vault_codehash: f
                        .codehash(&t["receiptVaultCodehash"], "tokens[].receiptVaultCodehash")?,
                    receipt_codehash: f
                        .codehash(&t["receiptCodehash"], "tokens[].receiptCodehash")?,
                    wrapped_token_vault_codehash: f.codehash(
                        &t["wrappedTokenVaultCodehash"],
                        "tokens[].wrappedTokenVaultCodehash",
                    )?,
                    owner: f.nonzero(&t["owner"], "tokens[].owner")?,
                    authoriser: f.nonzero(&t["authoriser"], "tokens[].authoriser")?,
                    asset_id: opt_text(&t["assetId"]),
                    listing_ticker: opt_text(&t["listingTicker"]),
                    listing_name: opt_text(&t["listingName"]),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let qsep = match (network == REFERENCE_NETWORK, &c["qsep"]) {
            (false, serde_json::Value::Null) => None,
            (false, _) => return Err(f.bad("qsep")),
            (true, q) => Some(ExpectedQsep {
                receipt_vault: f.nonzero(&q["receiptVault"], "qsep.receiptVault")?,
                receipt: f.nonzero(&q["receipt"], "qsep.receipt")?,
                wrapped_token_vault: f
                    .nonzero(&q["wrappedTokenVault"], "qsep.wrappedTokenVault")?,
                receipt_vault_receipt: f
                    .nonzero(&q["receiptVaultReceipt"], "qsep.receiptVaultReceipt")?,
                receipt_manager: f.nonzero(&q["receiptManager"], "qsep.receiptManager")?,
                wrapped_token_vault_asset: f
                    .nonzero(&q["wrappedTokenVaultAsset"], "qsep.wrappedTokenVaultAsset")?,
                receipt_vault_beacon: f
                    .nonzero(&q["receiptVaultBeacon"], "qsep.receiptVaultBeacon")?,
                receipt_beacon: f.nonzero(&q["receiptBeacon"], "qsep.receiptBeacon")?,
                wrapped_token_vault_beacon: f.nonzero(
                    &q["wrappedTokenVaultBeacon"],
                    "qsep.wrappedTokenVaultBeacon",
                )?,
                receipt_vault_codehash: f
                    .codehash(&q["receiptVaultCodehash"], "qsep.receiptVaultCodehash")?,
                receipt_codehash: f.codehash(&q["receiptCodehash"], "qsep.receiptCodehash")?,
                wrapped_token_vault_codehash: f.codehash(
                    &q["wrappedTokenVaultCodehash"],
                    "qsep.wrappedTokenVaultCodehash",
                )?,
                owner: f.nonzero(&q["owner"], "qsep.owner")?,
                authoriser: f.nonzero(&q["authoriser"], "qsep.authoriser")?,
            }),
        };
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
            qsep,
            tokens,
        });
    }
    out.sort_by_key(|c| (c.network != REFERENCE_NETWORK, c.chain_id));
    Ok(out)
}

/// The file's v2 beacon order.
pub const V2_BEACON_NAMES: [(&str, &str); 3] = [
    ("receiptBeacon", "Receipt beacon"),
    ("receiptVaultBeacon", "Receipt-vault beacon"),
    ("wrappedTokenVaultBeacon", "Wrapped-token-vault beacon"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedV2Beacon {
    pub name: &'static str,
    pub address: String,
    pub implementation: String,
    pub owner: String,
}

/// One chain's v2 deployments and beacons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedV2 {
    pub chain_id: u64,
    pub network: String,
    pub beacon_set_deployer: String,
    pub deployments: Vec<ExpectedZoltu>,
    pub beacons: Vec<ExpectedV2Beacon>,
}

/// Parse the file's `v2`, by chain id.
pub fn parse_v2(body: &str) -> Result<Vec<ExpectedV2>, ExpectedError> {
    let chains = chain_entries(body, "v2")?;
    let mut out = Vec::with_capacity(chains.len());
    for (key, c) in &chains {
        let f = ChainFields(key);
        let deployments = c["deployments"]
            .as_array()
            .filter(|d| !d.is_empty())
            .ok_or_else(|| f.bad("v2.deployments"))?
            .iter()
            .map(|d| {
                Ok(ExpectedZoltu {
                    key: d["key"]
                        .as_str()
                        .filter(|k| !k.is_empty())
                        .ok_or_else(|| f.bad("v2.deployments[].key"))?
                        .to_string(),
                    code: ExpectedCode {
                        address: f.nonzero(&d["address"], "v2.deployments[].address")?,
                        codehash: f.codehash(&d["codehash"], "v2.deployments[].codehash")?,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let beacons = V2_BEACON_NAMES
            .iter()
            .map(|&(field, name)| {
                let b = &c[field];
                Ok(ExpectedV2Beacon {
                    name,
                    address: f.nonzero(&b["beacon"], "v2 beacon")?,
                    implementation: f.nonzero(&b["implementation"], "v2 beacon implementation")?,
                    owner: f.nonzero(&b["owner"], "v2 beacon owner")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(ExpectedV2 {
            chain_id: key.parse::<u64>().map_err(|_| f.bad("chain id"))?,
            network: c["network"]
                .as_str()
                .filter(|n| !n.is_empty())
                .ok_or_else(|| f.bad("v2.network"))?
                .to_string(),
            beacon_set_deployer: f.nonzero(&c["beaconSetDeployer"], "v2.beaconSetDeployer")?,
            deployments,
            beacons,
        });
    }
    out.sort_by_key(|c| (c.network != REFERENCE_NETWORK, c.chain_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const FIXTURE: &str = include_str!("../testdata/expected-state.json");

    const ZERO: &str = "0x0000000000000000000000000000000000000000";
    const DEPLOYER: &str = "0xd64246e6b25F745f005E6233e050C9B879E660Dc";
    const BASE_DEPLOYER: &str = "0x2191981Ca2477B745870cC307cbEB4cB2967ACe3";
    const ETH_TIMELOCK: &str = "0x831E4e1bB2b9a67C00b7d17F252A18a22cd0bD2B";
    const BASE_TIMELOCK: &str = "0x48ba1371A78E6cC54157c63721756ab444510DB3";
    const ETH_SAFE: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const BASE_SAFE: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const SAFE_OWNERS: [&str; 6] = [
        "0x4746095B1Ea1A84446d34448f44e74D3d51f92F2",
        "0xceC2cb8B8EE4000FFA3F8a7f8E0Fa0A3E3DAb72d",
        "0x8D5901d8aE48101B59400235ad8614A2e0510466",
        "0xC1C89b7f5448F447d59f920456A9610f6b2544bC",
        "0xAB92b327c97A6E7461cBd76E2a789E5e106FF87e",
        "0x5CCd3cE683b66ff271DDB8915fF528b8fcFa23c2",
    ];
    const SAFE_PROXY_HASHES: [&str; 2] = [
        "0xd7d408ebcd99b2b70be43e20253d6d92a8ea8fab29bd3be7f55b10032331fb4c",
        "0xb89c1b3bdf2cf8827818646bce9a8f6e372885f8c55e5c07acbd307cb133b000",
    ];
    const SAFE_SINGLETONS: [&str; 2] = [
        "0x41675C099F32341bf84BFc5382aF534df5C7461a",
        "0x29fcB43b46531BcA003ddC8FCB67FFE91900C762",
    ];
    const SAFE_SINGLETON_HASHES: [&str; 2] = [
        "0x1fe2df852ba3299d6534ef416eefa406e56ced995bca886ab7a553e6d0c5e1c4",
        "0xb1f926978a0f44a2c0ec8fe822418ae969bd8c3f18d61e5103100339894f81ff",
    ];
    const FALLBACK_HANDLER: &str = "0xfd0732Dc9E303f09fCEf3a7388Ad10A83459Ec99";
    const FALLBACK_HANDLER_HASH: &str =
        "0x7c6007a5d711cea8dfd5d91f5940ec29c7f200fe511eb1fc1397b367af3c42f9";
    const BEACON_HASH: &str = "0x8e95867e52db417944afd90f3b6c3c980962831e8a944e7f6958ba8f8cc10630";
    const ORCH_BEACON: &str = "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613";
    const ORCH_BEACON_HASH: &str =
        "0x448cd06335de9e79ecdc51aa7c6647926860a1976f407de6f79f363b85ccaf2b";
    const ORCH_IMPL: &str = "0x1c3a4D12F88Bd39303Bb8510C6c1fa8Ad37A4b72";
    const ORCH_SET_DEPLOYER: &str = "0x945D0Faf6f268e805246907507ef1044e70E75D7";
    const ORCH_INSTANCE: &str = "0x3A7387a484d87Aa8bBA45E98AAB401Ce4FBF03E2";
    const RECEIPT_IMPL: &str = "0xB092f127Bd44F19BFa96e78dB4BeaF1066D2914E";
    const RECEIPT_VAULT_IMPL: &str = "0x52174538a83b18462c5cF898016539B086c920C7";
    const WTV_IMPL: &str = "0x0D99e0174DbF885ceD6AE8dEb939b0F890450099";
    const RECEIPT_VAULT_RENAMED_IMPL: &str = "0xCAA2aA64D8a599EAb2d5E7Fd30621D9Ca2f05A56";
    const WTV_RENAMED_IMPL: &str = "0x8330588c9A52Fd2Bd5c3E05A980F12d895c91Fe4";
    const BASE_RECEIPT_BEACON: &str = "0x86e93c39B095be0B0054C8488E26466Ee027D79a";
    const BASE_RECEIPT_VAULT_BEACON: &str = "0xEa084c8F4331CDF3328E772781b59F8A24F28F1A";
    const BASE_WTV_BEACON: &str = "0x4c2d2d3Bf1232bf0d3FB7123007A9B8444637bC8";
    const ETH_RECEIPT_BEACON: &str = "0xAcE121AE30D754536863a546f41b147bE11202db";
    const ETH_RECEIPT_VAULT_BEACON: &str = "0x24e98B8f8f951b1868DF88762Cef589AEdBC01eB";
    const ETH_WTV_BEACON: &str = "0x9FD790f65CA3aF2772358c653F097f0a4c7EE7d2";
    const CLONE_HASH: &str = "0x2089950d3cc1112dd66a58adcfadeadc490b50053ac67be8bc676b4a2dcd1717";
    const TIMELOCK_HASH: &str =
        "0xb6234401d5b271d66957815831adcde1e66f80a4c7a9d01881c7eecb70e96993";
    const RENAME: &str = "0xdb5ef986dc4f7bfd8e32c1ce772cfd820f03b142b96044f33c019782c97273e2";
    const RECEIPT_PROXY_HASH: &str =
        "0x3a6a276173fa980d807d8cdc6e760a76be57ee8cc0d6b8522ca651e7d6e649b6";
    const RECEIPT_VAULT_PROXY_HASH: &str =
        "0x458736ea3d3ad05745640e02a8e1110d707f793096e3ac278c304110f6d54689";
    const WTV_PROXY_HASH: &str =
        "0x8227665ab0d770ac6d47cc6b17924f8e64c93ca12f3b4281e8949b67d36f3e44";
    const BASE_V4_CLONE: &str = "0x315b16faa6eE413faBCa877d3851B3818369f0cD";
    const BASE_EU_CLONE: &str = "0xDB9152e46c1D140DB6a6f814461f21c571F3275e";
    const ETH_V4_CLONE: &str = "0x66566cc91dEAf818859bD4b09B7903ac48998157";
    const ETH_EU_CLONE: &str = "0x8Fc06579571A105C5a699FA11d95b9c73747f8eb";
    const QSEP_RECEIPT_VAULT: &str = "0x4A9a9FC94a507559481270d0BFf3315Ab92Fcefa";
    const QSEP_RECEIPT: &str = "0x311aA382B864799b3dE5C69478f841229D353B78";
    const QSEP_WTV: &str = "0x849E389982209F901B7B9ffCA57ae4c9EeB11c0d";

    /// The file's qsep keys, in `ExpectedQsep` field order.
    const QSEP_KEYS: [&str; 14] = [
        "receiptVault",
        "receipt",
        "wrappedTokenVault",
        "receiptVaultReceipt",
        "receiptManager",
        "wrappedTokenVaultAsset",
        "receiptVaultBeacon",
        "receiptBeacon",
        "wrappedTokenVaultBeacon",
        "receiptVaultCodehash",
        "receiptCodehash",
        "wrappedTokenVaultCodehash",
        "owner",
        "authoriser",
    ];

    struct Token {
        underlying: &'static str,
        name: &'static str,
        underlying_name: &'static str,
        receipt_vault: &'static str,
        receipt: &'static str,
        wrapped_token_vault: &'static str,
        authoriser: &'static str,
    }

    const BASE_MSTR: Token = Token {
        underlying: "MSTR",
        name: "MicroStrategy Incorporated ST0x",
        underlying_name: "Strategy Inc",
        receipt_vault: "0x013b782F402d61aa1004CCA95b9f5Bb402c9d5FE",
        receipt: "0x1c1fEF6f7b8e576219554b1d11c8aF29D00C0cEC",
        wrapped_token_vault: "0xFF05E1bD696900dc6A52CA35Ca61Bb1024eDa8e2",
        authoriser: BASE_V4_CLONE,
    };
    const BASE_MC: Token = Token {
        underlying: "MC",
        name: "LVMH Moët Hennessy Louis Vuitton SE ST0x",
        underlying_name: "LVMH Moët Hennessy Louis Vuitton SE",
        receipt_vault: "0x6eC2421d74F2AF61b7EE7bCCAE25A3b6a7961Aa0",
        receipt: "0xC3fD55A007CB7e4549A4b5064691c75609b1411b",
        wrapped_token_vault: "0x378a04202D25659763A02dA5FE77c0a35e64Ce1D",
        authoriser: BASE_EU_CLONE,
    };
    const ETH_MSTR: Token = Token {
        underlying: "MSTR",
        name: "MicroStrategy Incorporated ST0x",
        underlying_name: "Strategy Inc",
        receipt_vault: "0x8500189061e2206Bc33Bf04DC10fFB1Fe7dED637",
        receipt: "0xE3772C8695c2cf3dcAA2Dd29759f4Bb91a342763",
        wrapped_token_vault: "0xd9fE7488B86D3aEaf457b181C744BD1A5a120833",
        authoriser: ETH_V4_CLONE,
    };
    const ETH_MC: Token = Token {
        underlying: "MC",
        name: "LVMH Moët Hennessy Louis Vuitton SE ST0x",
        underlying_name: "LVMH Moët Hennessy Louis Vuitton SE",
        receipt_vault: "0x4d4D633bf04ABF6e4cF152C7c5c1ecD906Ab3de4",
        receipt: "0x9835e52FAFA9A92678C8A9420034706B1edD431e",
        wrapped_token_vault: "0x5Ce8b84756dDdE011734F04dC4bA2930C981B443",
        authoriser: ETH_EU_CLONE,
    };

    fn beacon(at: &str, codehash: &str, implementation: &str, renamed: &str, owner: &str) -> Value {
        json!({
            "beacon": at,
            "codehash": codehash,
            "implementation": implementation,
            "renamedImplementation": renamed,
            "owner": owner,
        })
    }

    /// The pins a token and qsep share. `beacons` is `[receiptVaultBeacon,
    /// receiptBeacon, wrappedTokenVaultBeacon]`; the receipt's manager is its
    /// receipt vault.
    fn legs(
        receipt_vault: &str,
        receipt: &str,
        wrapped_token_vault: &str,
        beacons: [&str; 3],
        owner: &str,
        authoriser: &str,
    ) -> Value {
        json!({
            "receiptVault": receipt_vault,
            "receipt": receipt,
            "wrappedTokenVault": wrapped_token_vault,
            "receiptVaultReceipt": receipt,
            "receiptManager": receipt_vault,
            "wrappedTokenVaultAsset": receipt_vault,
            "receiptVaultBeacon": beacons[0],
            "receiptBeacon": beacons[1],
            "wrappedTokenVaultBeacon": beacons[2],
            "receiptVaultCodehash": RECEIPT_VAULT_PROXY_HASH,
            "receiptCodehash": RECEIPT_PROXY_HASH,
            "wrappedTokenVaultCodehash": WTV_PROXY_HASH,
            "owner": owner,
            "authoriser": authoriser,
        })
    }

    fn token(t: &Token, beacons: [&str; 3], owner: &str) -> Value {
        let symbol = format!("t{}", t.underlying);
        let renamed = format!("{} (ST0x)", t.underlying_name);
        let mut v = json!({
            "underlying": t.underlying,
            "name": t.name,
            "renamedName": renamed,
            "symbol": symbol,
            "decimals": 18,
            "wrappedTokenVaultName": format!("Wrapped {}", t.name),
            "wrappedTokenVaultRenamedName": renamed,
            "wrappedTokenVaultSymbol": format!("w{symbol}"),
            "wrappedTokenVaultRenamedSymbol": format!("{}s", t.underlying),
            "wrappedTokenVaultDecimals": 18,
        });
        let l = legs(
            t.receipt_vault,
            t.receipt,
            t.wrapped_token_vault,
            beacons,
            owner,
            t.authoriser,
        );
        v.as_object_mut()
            .unwrap()
            .extend(l.as_object().unwrap().clone());
        v
    }

    struct Chain {
        network: &'static str,
        deployer: &'static str,
        timelock: &'static str,
        safe: &'static str,
        beacons: [&'static str; 4],
        authoriser: &'static str,
        eu_authoriser: &'static str,
        tokens: [&'static Token; 2],
    }

    /// One chain as `BuildExpectedState.sol` writes it, with two tokens and two
    /// Zoltu suites; the reference network also carries tQSEP.
    fn chain(c: &Chain) -> Value {
        let impls = [RECEIPT_IMPL, RECEIPT_VAULT_IMPL, WTV_IMPL, ORCH_IMPL];
        let renamed = [
            RECEIPT_IMPL,
            RECEIPT_VAULT_RENAMED_IMPL,
            WTV_RENAMED_IMPL,
            ORCH_IMPL,
        ];
        let hashes = [BEACON_HASH, BEACON_HASH, BEACON_HASH, ORCH_BEACON_HASH];
        let beacons: Vec<Value> = (0..4)
            .map(|i| beacon(c.beacons[i], hashes[i], impls[i], renamed[i], c.timelock))
            .collect();
        let leg_beacons = [c.beacons[1], c.beacons[0], c.beacons[2]];
        let mut v = json!({
            "network": c.network,
            "beaconSetDeployer": c.deployer,
            "beacons": beacons,
            "safe": {
                "address": c.safe,
                "owners": SAFE_OWNERS,
                "threshold": 3,
                "proxyCodehashes": SAFE_PROXY_HASHES,
                "singletons": SAFE_SINGLETONS,
                "singletonCodehashes": SAFE_SINGLETON_HASHES,
                "version": "1.4.1",
                "modules": [],
                "guard": ZERO,
                "fallbackHandler": FALLBACK_HANDLER,
                "fallbackHandlerCodehash": FALLBACK_HANDLER_HASH,
            },
            "timelock": {"address": c.timelock, "codehash": TIMELOCK_HASH, "minDelay": 172800},
            "orchestrator": {
                "setDeployer": ORCH_SET_DEPLOYER,
                "beacon": ORCH_BEACON,
                "beaconCodehash": ORCH_BEACON_HASH,
                "implementation": ORCH_IMPL,
                "owner": c.timelock,
                "instance": ORCH_INSTANCE,
            },
            "authoriser": {"address": c.authoriser, "codehash": CLONE_HASH},
            "euAuthoriser": {"address": c.eu_authoriser, "codehash": CLONE_HASH},
            "zoltu": [
                {
                    "key": "stox-receipt@0_1_1",
                    "address": "0x2dF5cFE6d688EF9fF1B7c59A499D254b1527b286",
                    "codehash": "0x06fffbad12ea80897d55aab5d4f1cd3f34f674237db44a148cc133334a0cca54",
                },
                {
                    "key": "stox-receipt@0_1_30",
                    "address": RECEIPT_IMPL,
                    "codehash": "0xba9e672e00e89c68294ae67b2d99a80f7509a6b10da809bf84b5c2e578579862",
                },
            ],
            "renameMigration": RENAME,
            "tokens": c.tokens.map(|t| token(t, leg_beacons, c.timelock)),
        });
        if c.network == REFERENCE_NETWORK {
            v["qsep"] = legs(
                QSEP_RECEIPT_VAULT,
                QSEP_RECEIPT,
                QSEP_WTV,
                leg_beacons,
                c.timelock,
                c.authoriser,
            );
        }
        v
    }

    const ETHEREUM: Chain = Chain {
        network: "ethereum",
        deployer: DEPLOYER,
        timelock: ETH_TIMELOCK,
        safe: ETH_SAFE,
        beacons: [
            ETH_RECEIPT_BEACON,
            ETH_RECEIPT_VAULT_BEACON,
            ETH_WTV_BEACON,
            ORCH_BEACON,
        ],
        authoriser: ETH_V4_CLONE,
        eu_authoriser: ETH_EU_CLONE,
        tokens: [&ETH_MSTR, &ETH_MC],
    };
    const BASE: Chain = Chain {
        network: "base",
        deployer: BASE_DEPLOYER,
        timelock: BASE_TIMELOCK,
        safe: BASE_SAFE,
        beacons: [
            BASE_RECEIPT_BEACON,
            BASE_RECEIPT_VAULT_BEACON,
            BASE_WTV_BEACON,
            ORCH_BEACON,
        ],
        authoriser: BASE_V4_CLONE,
        eu_authoriser: BASE_EU_CLONE,
        tokens: [&BASE_MSTR, &BASE_MC],
    };

    fn doc(chains: &[(&str, Value)]) -> Value {
        let chains: serde_json::Map<String, Value> = chains
            .iter()
            .map(|(id, c)| (id.to_string(), c.clone()))
            .collect();
        json!({"version": 1, "chains": chains})
    }

    /// Ethereum first, so a Base-only edit is the first error the parse meets.
    fn good() -> Value {
        doc(&[("1", chain(&ETHEREUM)), ("8453", chain(&BASE))])
    }

    fn set(mut d: Value, at: &str, v: Value) -> Value {
        *d.pointer_mut(at).unwrap_or_else(|| panic!("no {at}")) = v;
        d
    }

    fn remove(mut d: Value, at: &str) -> Value {
        let (parent, key) = at.rsplit_once('/').unwrap();
        d.pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .and_then(|o| o.remove(key))
            .unwrap_or_else(|| panic!("no {at}"));
        d
    }

    fn with(at: &str, v: Value) -> Value {
        set(good(), at, v)
    }

    fn without(at: &str) -> Value {
        remove(good(), at)
    }

    fn malformed(body: &Value) -> &'static str {
        match parse(&body.to_string()) {
            Err(ExpectedError::Malformed { field, .. }) => field,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parses_every_expectation_base_first() {
        let got = parse(&good().to_string()).unwrap();
        assert_eq!(
            got.iter().map(|c| c.chain_id).collect::<Vec<_>>(),
            [8453, 1]
        );
        let base = &got[0];
        assert_eq!(base.network, "base");
        assert_eq!(base.beacon_set_deployer, BASE_DEPLOYER);
        assert_eq!(
            base.beacons.iter().map(|b| b.name).collect::<Vec<_>>(),
            BEACON_NAMES
        );
        assert_eq!(
            base.beacons[1],
            ExpectedBeacon {
                name: "Receipt-vault beacon",
                address: BASE_RECEIPT_VAULT_BEACON.into(),
                codehash: BEACON_HASH.into(),
                implementation: RECEIPT_VAULT_IMPL.into(),
                renamed_implementation: RECEIPT_VAULT_RENAMED_IMPL.into(),
                owner: BASE_TIMELOCK.into(),
            }
        );
        assert_eq!(base.beacons[0].implementation, RECEIPT_IMPL);
        assert_eq!(base.beacons[2].renamed_implementation, WTV_RENAMED_IMPL);
        assert_eq!(base.beacons[2].codehash, BEACON_HASH);
        assert_eq!(base.beacons[3].codehash, ORCH_BEACON_HASH);
        assert!(base.beacons.iter().all(|b| b.owner == BASE_TIMELOCK));

        let eth = &got[1];
        assert_eq!(eth.beacon_set_deployer, DEPLOYER);
        assert_eq!(
            eth.beacons
                .iter()
                .map(|b| b.address.as_str())
                .collect::<Vec<_>>(),
            [
                ETH_RECEIPT_BEACON,
                ETH_RECEIPT_VAULT_BEACON,
                ETH_WTV_BEACON,
                ORCH_BEACON
            ]
        );
        assert!(eth.beacons.iter().all(|b| b.owner == ETH_TIMELOCK));
    }

    /// The rename moves the receipt-vault beacon onto another implementation;
    /// an unread rename pins neither.
    #[test]
    fn the_rename_picks_the_implementation_a_beacon_is_expected_at() {
        let got = parse(&good().to_string()).unwrap();
        let b = &got[0].beacons[1];
        assert_eq!(
            b.expected_implementation(Some(true)),
            Some(RECEIPT_VAULT_RENAMED_IMPL)
        );
        assert_eq!(
            b.expected_implementation(Some(false)),
            Some(RECEIPT_VAULT_IMPL)
        );
        assert_eq!(b.expected_implementation(None), None);
    }

    #[test]
    fn parses_the_safe_lock_down() {
        let got = parse(&good().to_string()).unwrap();
        let strings = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            got[0].safe,
            ExpectedSafe {
                address: BASE_SAFE.into(),
                owners: strings(&SAFE_OWNERS),
                threshold: 3,
                proxy_codehashes: strings(&SAFE_PROXY_HASHES),
                singletons: strings(&SAFE_SINGLETONS),
                singleton_codehashes: strings(&SAFE_SINGLETON_HASHES),
                version: "1.4.1".into(),
                modules: vec![],
                guard: None,
                fallback_handler: FALLBACK_HANDLER.into(),
                fallback_handler_codehash: FALLBACK_HANDLER_HASH.into(),
            }
        );
        assert_eq!(got[1].safe.address, ETH_SAFE);
        let module = "0x2dF5cFE6d688EF9fF1B7c59A499D254b1527b286";
        let got = parse(
            &with(
                "/chains/8453/safe/guard",
                json!("0x29fcB43b46531BcA003ddC8FCB67FFE91900C762"),
            )
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            got[0].safe.guard.as_deref(),
            Some("0x29fcB43b46531BcA003ddC8FCB67FFE91900C762")
        );
        let got = parse(&with("/chains/8453/safe/modules", json!([module])).to_string()).unwrap();
        assert_eq!(got[0].safe.modules, [module]);
    }

    #[test]
    fn parses_the_timelock_orchestrator_authorisers_and_zoltu_suites() {
        let got = parse(&good().to_string()).unwrap();
        let base = &got[0];
        assert_eq!(
            base.timelock,
            ExpectedTimelock {
                code: ExpectedCode {
                    address: BASE_TIMELOCK.into(),
                    codehash: TIMELOCK_HASH.into(),
                },
                min_delay: 172800,
            }
        );
        assert_eq!(
            base.orchestrator,
            ExpectedOrchestrator {
                set_deployer: ORCH_SET_DEPLOYER.into(),
                beacon: ORCH_BEACON.into(),
                beacon_codehash: ORCH_BEACON_HASH.into(),
                implementation: ORCH_IMPL.into(),
                owner: BASE_TIMELOCK.into(),
                instance: ORCH_INSTANCE.into(),
            }
        );
        assert_eq!(
            base.authoriser,
            ExpectedCode {
                address: BASE_V4_CLONE.into(),
                codehash: CLONE_HASH.into(),
            }
        );
        assert_eq!(base.eu_authoriser.address, BASE_EU_CLONE);
        assert_eq!(got[1].authoriser.address, ETH_V4_CLONE);
        assert_eq!(got[1].eu_authoriser.address, ETH_EU_CLONE);
        assert_eq!(
            base.zoltu
                .iter()
                .map(|z| z.key.as_str())
                .collect::<Vec<_>>(),
            ["stox-receipt@0_1_1", "stox-receipt@0_1_30"]
        );
        assert_eq!(
            base.zoltu[1].code,
            ExpectedCode {
                address: RECEIPT_IMPL.into(),
                codehash: "0xba9e672e00e89c68294ae67b2d99a80f7509a6b10da809bf84b5c2e578579862"
                    .into(),
            }
        );
        assert_eq!(base.rename_migration, RENAME);
    }

    #[test]
    fn parses_each_token_in_file_order() {
        let got = parse(&good().to_string()).unwrap();
        let base = &got[0];
        assert_eq!(
            base.tokens
                .iter()
                .map(|t| t.underlying.as_str())
                .collect::<Vec<_>>(),
            ["MSTR", "MC"]
        );
        assert_eq!(
            base.tokens[0],
            ExpectedToken {
                underlying: "MSTR".into(),
                name: "MicroStrategy Incorporated ST0x".into(),
                renamed_name: "Strategy Inc (ST0x)".into(),
                symbol: "tMSTR".into(),
                decimals: 18,
                wrapped_token_vault_name: "Wrapped MicroStrategy Incorporated ST0x".into(),
                wrapped_token_vault_renamed_name: "Strategy Inc (ST0x)".into(),
                wrapped_token_vault_symbol: "wtMSTR".into(),
                wrapped_token_vault_renamed_symbol: "MSTRs".into(),
                wrapped_token_vault_decimals: 18,
                receipt_vault: BASE_MSTR.receipt_vault.into(),
                receipt: BASE_MSTR.receipt.into(),
                wrapped_token_vault: BASE_MSTR.wrapped_token_vault.into(),
                receipt_vault_receipt: BASE_MSTR.receipt.into(),
                receipt_manager: BASE_MSTR.receipt_vault.into(),
                wrapped_token_vault_asset: BASE_MSTR.receipt_vault.into(),
                receipt_vault_beacon: BASE_RECEIPT_VAULT_BEACON.into(),
                receipt_beacon: BASE_RECEIPT_BEACON.into(),
                wrapped_token_vault_beacon: BASE_WTV_BEACON.into(),
                receipt_vault_codehash: RECEIPT_VAULT_PROXY_HASH.into(),
                receipt_codehash: RECEIPT_PROXY_HASH.into(),
                wrapped_token_vault_codehash: WTV_PROXY_HASH.into(),
                owner: BASE_TIMELOCK.into(),
                authoriser: BASE_V4_CLONE.into(),
                asset_id: None,
                listing_ticker: None,
                listing_name: None,
            }
        );
        assert_eq!(
            base.tokens[1].name,
            "LVMH Moët Hennessy Louis Vuitton SE ST0x"
        );
        assert_eq!(base.tokens[1].authoriser, BASE_EU_CLONE);
        assert_eq!(got[1].tokens[0].receipt_beacon, ETH_RECEIPT_BEACON);
        assert_eq!(
            got[1].tokens[0].receipt_vault_beacon,
            ETH_RECEIPT_VAULT_BEACON
        );
    }

    /// The manager is read from its own key, not assumed to be the receipt vault.
    #[test]
    fn a_token_receipt_manager_is_read_from_its_own_key() {
        let manager = "0x2191981Ca2477B745870cC307cbEB4cB2967ACe3";
        let got = parse(&with("/chains/8453/tokens/1/receiptManager", json!(manager)).to_string())
            .unwrap();
        assert_eq!(got[0].tokens[1].receipt_manager, manager);
        assert_eq!(got[0].tokens[1].receipt_vault, BASE_MC.receipt_vault);
        assert_eq!(got[0].tokens[0].receipt_manager, BASE_MSTR.receipt_vault);
    }

    #[test]
    fn parses_qsep_on_the_reference_network() {
        let got = parse(&good().to_string()).unwrap();
        assert_eq!(
            got[0].qsep,
            Some(ExpectedQsep {
                receipt_vault: QSEP_RECEIPT_VAULT.into(),
                receipt: QSEP_RECEIPT.into(),
                wrapped_token_vault: QSEP_WTV.into(),
                receipt_vault_receipt: QSEP_RECEIPT.into(),
                receipt_manager: QSEP_RECEIPT_VAULT.into(),
                wrapped_token_vault_asset: QSEP_RECEIPT_VAULT.into(),
                receipt_vault_beacon: BASE_RECEIPT_VAULT_BEACON.into(),
                receipt_beacon: BASE_RECEIPT_BEACON.into(),
                wrapped_token_vault_beacon: BASE_WTV_BEACON.into(),
                receipt_vault_codehash: RECEIPT_VAULT_PROXY_HASH.into(),
                receipt_codehash: RECEIPT_PROXY_HASH.into(),
                wrapped_token_vault_codehash: WTV_PROXY_HASH.into(),
                owner: BASE_TIMELOCK.into(),
                authoriser: BASE_V4_CLONE.into(),
            })
        );
        assert_eq!(got[1].qsep, None);
    }

    #[test]
    fn each_qsep_field_is_read_from_its_own_key() {
        let a = |i: usize| format!("0x{i:040x}");
        let h = |i: usize| format!("0x{i:064x}");
        let mut d = good();
        for (i, key) in QSEP_KEYS.into_iter().enumerate() {
            let v = if key.ends_with("Codehash") {
                h(i + 1)
            } else {
                a(i + 1)
            };
            d["chains"]["8453"]["qsep"][key] = json!(v);
        }
        let got = parse(&d.to_string()).unwrap();
        assert_eq!(
            got[0].qsep,
            Some(ExpectedQsep {
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
            })
        );
    }

    /// tQSEP lives on the reference network alone: listed anywhere else it is
    /// malformed, and the reference network without it is missing its fields.
    #[test]
    fn qsep_is_required_on_the_reference_network_and_rejected_elsewhere() {
        let q = good()["chains"]["8453"]["qsep"].clone();
        for listed in [q, json!({}), json!("none")] {
            let mut d = good();
            d["chains"]["1"]["qsep"] = listed.clone();
            assert_eq!(malformed(&d), "qsep", "{listed}");
        }
        let mut d = good();
        d["chains"]["1"]["qsep"] = Value::Null;
        assert_eq!(parse(&d.to_string()).unwrap()[1].qsep, None);
        assert_eq!(
            malformed(&without("/chains/8453/qsep")),
            "qsep.receiptVault"
        );
        assert_eq!(
            malformed(&with("/chains/8453/qsep", Value::Null)),
            "qsep.receiptVault"
        );
    }

    #[test]
    fn rejects_a_zero_or_missing_qsep_field_by_name() {
        let zero_hash = format!("0x{}", "0".repeat(64));
        for key in QSEP_KEYS {
            let at = format!("/chains/8453/qsep/{key}");
            let field = format!("qsep.{key}");
            let zero = if key.ends_with("Codehash") {
                json!(zero_hash)
            } else {
                json!(ZERO)
            };
            assert_eq!(malformed(&with(&at, zero)), field, "{at} zero");
            assert_eq!(malformed(&with(&at, json!("0x4A9a"))), field, "{at} short");
            assert_eq!(malformed(&without(&at)), field, "{at} missing");
        }
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
            parse(&with("/version", json!(2)).to_string()),
            Err(ExpectedError::UnsupportedVersion(Some(2)))
        );
        assert_eq!(
            parse(r#"{"version": 1, "chains": {}}"#),
            Err(ExpectedError::NoChains)
        );
        assert_eq!(parse(r#"{"version": 1}"#), Err(ExpectedError::NoChains));
    }

    /// A zero expectation would match a contract answering the zero address, and
    /// a zero codehash would match an address with no account.
    #[test]
    fn rejects_a_zero_or_malformed_value_in_every_field() {
        let b = "/chains/8453";
        let s = "/chains/8453/safe";
        let t = "/chains/8453/tokens/0";
        let zero_hash = format!("0x{}", "0".repeat(64));
        let cases = [
            (format!("{b}/network"), json!(""), "network"),
            (
                format!("{b}/beaconSetDeployer"),
                json!("0x0"),
                "beaconSetDeployer",
            ),
            (
                format!("{b}/beacons/2/beacon"),
                json!("0x9FD7"),
                "beacons[].beacon",
            ),
            (
                format!("{b}/beacons/0/beacon"),
                json!(ZERO),
                "beacons[].beacon",
            ),
            (
                format!("{b}/beacons/0/codehash"),
                json!(zero_hash),
                "beacons[].codehash",
            ),
            (
                format!("{b}/beacons/0/codehash"),
                json!("0x8e95"),
                "beacons[].codehash",
            ),
            (
                format!("{b}/beacons/0/implementation"),
                json!(ZERO),
                "beacons[].implementation",
            ),
            (
                format!("{b}/beacons/1/renamedImplementation"),
                json!(ZERO),
                "beacons[].renamedImplementation",
            ),
            (
                format!("{b}/beacons/2/renamedImplementation"),
                json!("0x8330"),
                "beacons[].renamedImplementation",
            ),
            (
                format!("{b}/beacons/3/owner"),
                json!(ZERO),
                "beacons[].owner",
            ),
            (format!("{s}/address"), json!(ZERO), "safe.address"),
            (format!("{s}/owners/2"), json!(ZERO), "safe.owners"),
            (format!("{s}/owners/2"), json!("0x5CCd"), "safe.owners"),
            (format!("{s}/owners"), json!([]), "safe.owners"),
            (format!("{s}/threshold"), json!(0), "safe.threshold"),
            (format!("{s}/threshold"), json!(7), "safe.threshold"),
            (
                format!("{s}/proxyCodehashes"),
                json!([]),
                "safe.proxyCodehashes",
            ),
            (
                format!("{s}/proxyCodehashes/1"),
                json!(zero_hash),
                "safe.proxyCodehashes",
            ),
            (format!("{s}/singletons"), json!([]), "safe.singletons"),
            (format!("{s}/singletons/0"), json!(ZERO), "safe.singletons"),
            (
                format!("{s}/singletonCodehashes"),
                json!([SAFE_SINGLETON_HASHES[0]]),
                "safe.singletonCodehashes",
            ),
            (
                format!("{s}/singletonCodehashes/1"),
                json!(zero_hash),
                "safe.singletonCodehashes",
            ),
            (format!("{s}/version"), json!(""), "safe.version"),
            (format!("{s}/modules"), json!([ZERO]), "safe.modules"),
            (format!("{s}/modules"), json!("none"), "safe.modules"),
            (format!("{s}/guard"), json!("0x0"), "safe.guard"),
            (
                format!("{s}/fallbackHandler"),
                json!(ZERO),
                "safe.fallbackHandler",
            ),
            (
                format!("{s}/fallbackHandlerCodehash"),
                json!(zero_hash),
                "safe.fallbackHandlerCodehash",
            ),
            (
                format!("{b}/timelock/address"),
                json!(ZERO),
                "timelock.address",
            ),
            (
                format!("{b}/timelock/codehash"),
                json!(zero_hash),
                "timelock.codehash",
            ),
            (
                format!("{b}/timelock/minDelay"),
                json!("2 days"),
                "timelock.minDelay",
            ),
            (
                format!("{b}/timelock/minDelay"),
                json!(-1),
                "timelock.minDelay",
            ),
            (
                format!("{b}/orchestrator/setDeployer"),
                json!(ZERO),
                "orchestrator.setDeployer",
            ),
            (
                format!("{b}/orchestrator/beacon"),
                json!(ZERO),
                "orchestrator.beacon",
            ),
            (
                format!("{b}/orchestrator/beaconCodehash"),
                json!(zero_hash),
                "orchestrator.beaconCodehash",
            ),
            (
                format!("{b}/orchestrator/implementation"),
                json!(ZERO),
                "orchestrator.implementation",
            ),
            (
                format!("{b}/orchestrator/owner"),
                json!(ZERO),
                "orchestrator.owner",
            ),
            (
                format!("{b}/orchestrator/instance"),
                json!(ZERO),
                "orchestrator.instance",
            ),
            (
                format!("{b}/authoriser/address"),
                json!(ZERO),
                "authoriser.address",
            ),
            (
                format!("{b}/authoriser/codehash"),
                json!(zero_hash),
                "authoriser.codehash",
            ),
            (
                format!("{b}/euAuthoriser/address"),
                json!(ZERO),
                "euAuthoriser.address",
            ),
            (
                format!("{b}/euAuthoriser/codehash"),
                json!(zero_hash),
                "euAuthoriser.codehash",
            ),
            (format!("{b}/zoltu"), json!({}), "zoltu"),
            (format!("{b}/zoltu/0/key"), json!(""), "zoltu[].key"),
            (
                format!("{b}/zoltu/0/address"),
                json!(ZERO),
                "zoltu[].address",
            ),
            (
                format!("{b}/zoltu/1/codehash"),
                json!(zero_hash),
                "zoltu[].codehash",
            ),
            (
                format!("{b}/renameMigration"),
                json!(zero_hash),
                "renameMigration",
            ),
            (
                format!("{b}/renameMigration"),
                json!(ZERO),
                "renameMigration",
            ),
            (format!("{b}/tokens"), json!({}), "tokens"),
            (format!("{t}/underlying"), json!(""), "tokens[].underlying"),
            (format!("{t}/name"), json!(""), "tokens[].name"),
            (
                format!("{t}/renamedName"),
                json!(""),
                "tokens[].renamedName",
            ),
            (format!("{t}/symbol"), json!(""), "tokens[].symbol"),
            (format!("{t}/decimals"), json!("18"), "tokens[].decimals"),
            (
                format!("{t}/wrappedTokenVaultName"),
                json!(""),
                "tokens[].wrappedTokenVaultName",
            ),
            (
                format!("{t}/wrappedTokenVaultRenamedName"),
                json!(""),
                "tokens[].wrappedTokenVaultRenamedName",
            ),
            (
                format!("{t}/wrappedTokenVaultSymbol"),
                json!(""),
                "tokens[].wrappedTokenVaultSymbol",
            ),
            (
                format!("{t}/wrappedTokenVaultRenamedSymbol"),
                json!(""),
                "tokens[].wrappedTokenVaultRenamedSymbol",
            ),
            (
                format!("{t}/wrappedTokenVaultDecimals"),
                json!(1.5),
                "tokens[].wrappedTokenVaultDecimals",
            ),
            (
                format!("{t}/receiptVault"),
                json!(ZERO),
                "tokens[].receiptVault",
            ),
            (format!("{t}/receipt"), json!(ZERO), "tokens[].receipt"),
            (
                format!("{t}/wrappedTokenVault"),
                json!(ZERO),
                "tokens[].wrappedTokenVault",
            ),
            (
                format!("{t}/receiptVaultReceipt"),
                json!(ZERO),
                "tokens[].receiptVaultReceipt",
            ),
            (
                format!("{t}/receiptManager"),
                json!(ZERO),
                "tokens[].receiptManager",
            ),
            (
                format!("{t}/receiptManager"),
                json!("0x013b"),
                "tokens[].receiptManager",
            ),
            (
                format!("{t}/wrappedTokenVaultAsset"),
                json!(ZERO),
                "tokens[].wrappedTokenVaultAsset",
            ),
            (
                format!("{t}/receiptVaultBeacon"),
                json!("0xEa08"),
                "tokens[].receiptVaultBeacon",
            ),
            (
                format!("{t}/receiptVaultBeacon"),
                json!(ZERO),
                "tokens[].receiptVaultBeacon",
            ),
            (
                format!("{t}/receiptBeacon"),
                json!(ZERO),
                "tokens[].receiptBeacon",
            ),
            (
                format!("{t}/wrappedTokenVaultBeacon"),
                json!(ZERO),
                "tokens[].wrappedTokenVaultBeacon",
            ),
            (
                format!("{t}/receiptVaultCodehash"),
                json!(zero_hash),
                "tokens[].receiptVaultCodehash",
            ),
            (
                format!("{t}/receiptCodehash"),
                json!(zero_hash),
                "tokens[].receiptCodehash",
            ),
            (
                format!("{t}/wrappedTokenVaultCodehash"),
                json!(zero_hash),
                "tokens[].wrappedTokenVaultCodehash",
            ),
            (format!("{t}/owner"), json!(ZERO), "tokens[].owner"),
            (
                format!("{t}/authoriser"),
                json!(ZERO),
                "tokens[].authoriser",
            ),
            (
                format!("{b}/beacons/0/beacon"),
                json!(BEACON_HASH),
                "beacons[].beacon",
            ),
            (
                format!("{b}/beacons/0/codehash"),
                json!(BASE_SAFE),
                "beacons[].codehash",
            ),
            (
                format!("{s}/singletonCodehashes"),
                json!([
                    SAFE_SINGLETON_HASHES[0],
                    SAFE_SINGLETON_HASHES[1],
                    BEACON_HASH
                ]),
                "safe.singletonCodehashes",
            ),
        ];
        for (at, v, field) in cases {
            assert_eq!(malformed(&with(&at, v.clone())), field, "{at} = {v}");
        }
    }

    #[test]
    fn rejects_a_missing_field_by_name() {
        let cases = [
            ("/chains/8453/network", "network"),
            ("/chains/8453/beaconSetDeployer", "beaconSetDeployer"),
            ("/chains/8453/beacons", "beacons"),
            ("/chains/8453/beacons/0/codehash", "beacons[].codehash"),
            (
                "/chains/8453/beacons/1/renamedImplementation",
                "beacons[].renamedImplementation",
            ),
            ("/chains/8453/safe/owners", "safe.owners"),
            ("/chains/8453/safe/threshold", "safe.threshold"),
            ("/chains/8453/safe/proxyCodehashes", "safe.proxyCodehashes"),
            ("/chains/8453/safe/singletons", "safe.singletons"),
            (
                "/chains/8453/safe/singletonCodehashes",
                "safe.singletonCodehashes",
            ),
            ("/chains/8453/safe/version", "safe.version"),
            ("/chains/8453/safe/modules", "safe.modules"),
            ("/chains/8453/safe/guard", "safe.guard"),
            ("/chains/8453/safe/fallbackHandler", "safe.fallbackHandler"),
            (
                "/chains/8453/safe/fallbackHandlerCodehash",
                "safe.fallbackHandlerCodehash",
            ),
            ("/chains/8453/timelock", "timelock.address"),
            ("/chains/8453/timelock/minDelay", "timelock.minDelay"),
            ("/chains/8453/orchestrator", "orchestrator.setDeployer"),
            ("/chains/8453/authoriser", "authoriser.address"),
            (
                "/chains/8453/euAuthoriser/codehash",
                "euAuthoriser.codehash",
            ),
            ("/chains/8453/zoltu", "zoltu"),
            ("/chains/8453/zoltu/0/key", "zoltu[].key"),
            ("/chains/8453/renameMigration", "renameMigration"),
            ("/chains/8453/tokens", "tokens"),
            ("/chains/8453/tokens/1/renamedName", "tokens[].renamedName"),
            ("/chains/8453/tokens/1/decimals", "tokens[].decimals"),
            (
                "/chains/8453/tokens/1/receiptManager",
                "tokens[].receiptManager",
            ),
            (
                "/chains/8453/tokens/1/receiptBeacon",
                "tokens[].receiptBeacon",
            ),
            ("/chains/8453/tokens/1/authoriser", "tokens[].authoriser"),
            ("/chains/8453/qsep", "qsep.receiptVault"),
        ];
        for (at, field) in cases {
            assert_eq!(malformed(&without(at)), field, "{at}");
        }
    }

    #[test]
    fn a_zero_beacon_set_deployer_is_malformed_on_every_chain() {
        for id in ["1", "8453"] {
            let body = with(&format!("/chains/{id}/beaconSetDeployer"), json!(ZERO));
            match parse(&body.to_string()) {
                Err(ExpectedError::Malformed { chain, field }) => {
                    assert_eq!((chain.as_str(), field), (id, "beaconSetDeployer"));
                }
                other => panic!("chain {id}: expected Malformed, got {other:?}"),
            }
        }
    }

    /// `parse` and `parse_v2` share one address and one codehash validator, so
    /// each rejects exactly the same values and accepts the same mixed case.
    #[test]
    fn parse_and_parse_v2_reject_the_same_zero_and_malformed_values() {
        let mut both = good();
        both["v2"] = v2_good()["v2"].clone();
        let field = |r: Result<usize, ExpectedError>| match r {
            Err(ExpectedError::Malformed { field, .. }) => Some(field),
            Ok(_) => None,
            Err(e) => panic!("expected Malformed, got {e:?}"),
        };
        let v1 = |at: &str, v: &Value| {
            field(parse(&set(both.clone(), at, v.clone()).to_string()).map(|c| c.len()))
        };
        let v2 = |at: &str, v: &Value| {
            field(parse_v2(&set(both.clone(), at, v.clone()).to_string()).map(|c| c.len()))
        };
        let hex = |n: usize| format!("0x{}", "aB".repeat(n / 2));
        let bad_addresses = [
            json!(ZERO),
            json!(hex(38)),
            json!(hex(42)),
            json!(format!("0X{}", "aB".repeat(20))),
            json!(format!("0x{}gA", "aB".repeat(19))),
            json!(format!("00{}", "aB".repeat(20))),
            json!(8453),
            Value::Null,
        ];
        let bad_hashes = [
            json!(format!("0x{}", "0".repeat(64))),
            json!(hex(62)),
            json!(hex(66)),
            json!(format!("0X{}", "aB".repeat(32))),
            json!(format!("0x{}gA", "aB".repeat(31))),
            json!(1),
            Value::Null,
        ];
        let (addr1, addr2) = (
            "/chains/8453/beaconSetDeployer",
            "/v2/8453/beaconSetDeployer",
        );
        let (hash1, hash2) = (
            "/chains/8453/zoltu/0/codehash",
            "/v2/8453/deployments/0/codehash",
        );
        for v in &bad_addresses {
            assert_eq!(v1(addr1, v), Some("beaconSetDeployer"), "{v}");
            assert_eq!(v2(addr2, v), Some("v2.beaconSetDeployer"), "{v}");
        }
        for v in &bad_hashes {
            assert_eq!(v1(hash1, v), Some("zoltu[].codehash"), "{v}");
            assert_eq!(v2(hash2, v), Some("v2.deployments[].codehash"), "{v}");
        }
        let (addr, hash) = (json!(hex(40)), json!(hex(64)));
        assert_eq!((v1(addr1, &addr), v2(addr2, &addr)), (None, None));
        assert_eq!((v1(hash1, &hash), v2(hash2, &hash)), (None, None));
    }

    /// The file names every beacon by address, so a zero beacon or token leg
    /// beacon is malformed even on a chain with a beacon-set deployer.
    #[test]
    fn a_zero_beacon_is_malformed_on_a_chain_with_a_deployer() {
        for slot in 0..BEACON_NAMES.len() {
            let body = with(&format!("/chains/1/beacons/{slot}/beacon"), json!(ZERO));
            assert_eq!(malformed(&body), "beacons[].beacon", "slot {slot}");
        }
        for leg in [
            "receiptVaultBeacon",
            "receiptBeacon",
            "wrappedTokenVaultBeacon",
        ] {
            let body = with(&format!("/chains/1/tokens/0/{leg}"), json!(ZERO));
            assert_eq!(malformed(&body), format!("tokens[].{leg}"));
        }
    }

    #[test]
    fn rejects_a_chain_keyed_by_anything_but_its_id() {
        let mut d = good();
        let chains = d["chains"].as_object_mut().unwrap();
        let base = chains.remove("8453").unwrap();
        chains.insert("base".into(), base);
        assert_eq!(malformed(&d), "chain id");
    }

    #[test]
    fn rejects_a_beacon_list_of_the_wrong_length() {
        let mut short = good();
        short["chains"]["8453"]["beacons"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(malformed(&short), "beacons");
        let mut long = good();
        let extra = long["chains"]["8453"]["beacons"][0].clone();
        long["chains"]["8453"]["beacons"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        assert_eq!(malformed(&long), "beacons");
        let body = r#"{"version": 1, "chains": {"8453": {"network": "base", "beaconSetDeployer": "0x2191981Ca2477B745870cC307cbEB4cB2967ACe3"}}}"#;
        assert_eq!(malformed(&serde_json::from_str(body).unwrap()), "beacons");
    }

    /// An empty token, Zoltu or module list is a chain with none, not a gap.
    #[test]
    fn accepts_empty_token_zoltu_and_module_lists() {
        let mut d = good();
        d["chains"]["8453"]["tokens"] = json!([]);
        d["chains"]["8453"]["zoltu"] = json!([]);
        let got = parse(&d.to_string()).unwrap();
        assert!(got[0].tokens.is_empty());
        assert!(got[0].zoltu.is_empty());
        assert!(got[0].safe.modules.is_empty());
    }

    #[test]
    fn a_threshold_equal_to_the_owner_count_is_accepted() {
        let got = parse(&with("/chains/8453/safe/threshold", json!(6)).to_string()).unwrap();
        assert_eq!(got[0].safe.threshold, 6);
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
            ExpectedError::Malformed {
                chain: "8453".into(),
                field: "safe.owners"
            }
            .to_string(),
            "expected-state.json chain 8453 has a missing or malformed safe.owners"
        );
    }

    const V2_DEPLOYER: &str = "0x0C5154C4861908Bd5a6FD6fFCB063e9869ceFa41";
    const V2_RECEIPT: &str = "0xbAB0E6b7B5dDA86FB8ba81c00aEA0Ceb8b73686b";
    const V2_RECEIPT_HASH: &str =
        "0x14348054f718979709402d1892155361f5ea99d8e7267823fcac9c7763bcefab";
    const V2_RECEIPT_VAULT: &str = "0xc95dB340A7a100881626475d41BFf70857Aa920D";
    const V2_WTV_BEACON: &str = "0x846a468e6fDA529D282D60df7D1EE785EB954600";

    /// `(file key, name, beacon, implementation, owner)`, as Base's v2 beacons.
    const V2_BEACONS: [(&str, &str, &str, &str, &str); 3] = [
        (
            "receiptBeacon",
            "Receipt beacon",
            "0x7EFeCb081f3A14Bc86cFA45373a23121a5D90Ec1",
            "0xE7573879D73455Dc92cB4087Fa8177594387CbCD",
            V2_RECEIPT,
        ),
        (
            "receiptVaultBeacon",
            "Receipt-vault beacon",
            "0x7328C39029f6Ee7Ff8d48932FFB4eCD44b6Fbb8C",
            "0x8EFfCe5Ebb047F215dF1d8522c32c7C9DE239f39",
            V2_RECEIPT_VAULT,
        ),
        (
            "wrappedTokenVaultBeacon",
            "Wrapped-token-vault beacon",
            V2_WTV_BEACON,
            "0xb438a1eA1550fd199d67D67a69B71F4324bB8660",
            "0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b",
        ),
    ];

    fn v2_base_beacons() -> Vec<ExpectedV2Beacon> {
        V2_BEACONS
            .iter()
            .map(
                |&(_, name, beacon, implementation, owner)| ExpectedV2Beacon {
                    name,
                    address: beacon.into(),
                    implementation: implementation.into(),
                    owner: owner.into(),
                },
            )
            .collect()
    }

    fn v2_chain(network: &str) -> Value {
        let mut v = json!({
            "network": network,
            "beaconSetDeployer": V2_DEPLOYER,
            "deployments": [
                {"key": "StoxReceipt", "address": V2_RECEIPT, "codehash": V2_RECEIPT_HASH},
                {"key": "StoxWrappedTokenVaultBeacon", "address": V2_WTV_BEACON, "codehash": BEACON_HASH},
            ],
        });
        for (key, _, beacon, implementation, owner) in V2_BEACONS {
            v[key] = json!({"beacon": beacon, "implementation": implementation, "owner": owner});
        }
        v
    }

    /// Off chain-id order, with Base keyed above two of the others.
    fn v2_good() -> Value {
        json!({
            "version": 1,
            "v2": {
                "42161": v2_chain("arbitrum"),
                "137": v2_chain("polygon"),
                "8453": v2_chain("base"),
                "14": v2_chain("flare"),
            },
        })
    }

    fn v2_malformed(body: &Value) -> &'static str {
        match parse_v2(&body.to_string()) {
            Err(ExpectedError::Malformed { field, .. }) => field,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parses_v2_deployments_and_beacons_base_first() {
        let got = parse_v2(&v2_good().to_string()).unwrap();
        assert_eq!(
            got.iter().map(|c| c.chain_id).collect::<Vec<_>>(),
            [8453, 14, 137, 42161]
        );
        assert_eq!(
            got.iter().map(|c| c.network.as_str()).collect::<Vec<_>>(),
            ["base", "flare", "polygon", "arbitrum"]
        );
        let base = &got[0];
        assert_eq!(base.beacon_set_deployer, V2_DEPLOYER);
        assert_eq!(
            base.deployments,
            [
                ExpectedZoltu {
                    key: "StoxReceipt".into(),
                    code: ExpectedCode {
                        address: V2_RECEIPT.into(),
                        codehash: V2_RECEIPT_HASH.into(),
                    },
                },
                ExpectedZoltu {
                    key: "StoxWrappedTokenVaultBeacon".into(),
                    code: ExpectedCode {
                        address: V2_WTV_BEACON.into(),
                        codehash: BEACON_HASH.into(),
                    },
                },
            ]
        );
        assert_eq!(base.beacons, v2_base_beacons());
    }

    /// The v1 chains and the v2 deployments sit side by side in one file; each
    /// reader reads only its own.
    #[test]
    fn parse_and_parse_v2_each_read_only_their_own_section() {
        let mut both = good();
        both["v2"] = v2_good()["v2"].clone();
        assert_eq!(parse(&both.to_string()).unwrap().len(), 2);
        assert_eq!(parse_v2(&both.to_string()).unwrap().len(), 4);
        assert_eq!(parse_v2(&good().to_string()), Err(ExpectedError::NoChains));
        assert_eq!(parse(&v2_good().to_string()), Err(ExpectedError::NoChains));
    }

    #[test]
    fn parse_v2_rejects_unreadable_files() {
        assert_eq!(parse_v2(""), Err(ExpectedError::Empty));
        assert_eq!(parse_v2(" \n"), Err(ExpectedError::Empty));
        assert_eq!(parse_v2("{"), Err(ExpectedError::NotJson));
        assert_eq!(
            parse_v2(r#"{"v2": {}}"#),
            Err(ExpectedError::UnsupportedVersion(None))
        );
        assert_eq!(
            parse_v2(&set(v2_good(), "/version", json!(2)).to_string()),
            Err(ExpectedError::UnsupportedVersion(Some(2)))
        );
        assert_eq!(
            parse_v2(r#"{"version": 1, "v2": {}}"#),
            Err(ExpectedError::NoChains)
        );
        assert_eq!(parse_v2(r#"{"version": 1}"#), Err(ExpectedError::NoChains));
    }

    #[test]
    fn parse_v2_rejects_a_zero_or_malformed_value_in_every_field() {
        let c = "/v2/8453";
        let zero_hash = format!("0x{}", "0".repeat(64));
        let cases = [
            (format!("{c}/network"), json!(""), "v2.network"),
            (format!("{c}/network"), json!(8453), "v2.network"),
            (
                format!("{c}/beaconSetDeployer"),
                json!(ZERO),
                "v2.beaconSetDeployer",
            ),
            (
                format!("{c}/beaconSetDeployer"),
                json!("0x0C51"),
                "v2.beaconSetDeployer",
            ),
            (format!("{c}/deployments"), json!([]), "v2.deployments"),
            (format!("{c}/deployments"), json!({}), "v2.deployments"),
            (
                format!("{c}/deployments/0/key"),
                json!(""),
                "v2.deployments[].key",
            ),
            (
                format!("{c}/deployments/0/address"),
                json!(ZERO),
                "v2.deployments[].address",
            ),
            (
                format!("{c}/deployments/1/address"),
                json!("0x846a"),
                "v2.deployments[].address",
            ),
            (
                format!("{c}/deployments/1/codehash"),
                json!(zero_hash),
                "v2.deployments[].codehash",
            ),
            (
                format!("{c}/deployments/0/codehash"),
                json!("0x1434"),
                "v2.deployments[].codehash",
            ),
            (
                format!("{c}/receiptBeacon/beacon"),
                json!(ZERO),
                "v2 beacon",
            ),
            (
                format!("{c}/receiptVaultBeacon/implementation"),
                json!(ZERO),
                "v2 beacon implementation",
            ),
            (
                format!("{c}/wrappedTokenVaultBeacon/owner"),
                json!(ZERO),
                "v2 beacon owner",
            ),
            (
                format!("{c}/wrappedTokenVaultBeacon/owner"),
                json!("0x8E4b"),
                "v2 beacon owner",
            ),
        ];
        for (at, v, field) in cases {
            assert_eq!(
                v2_malformed(&set(v2_good(), &at, v.clone())),
                field,
                "{at} = {v}"
            );
        }
    }

    #[test]
    fn parse_v2_rejects_a_missing_field_by_name() {
        let cases = [
            ("/v2/8453/network", "v2.network"),
            ("/v2/8453/beaconSetDeployer", "v2.beaconSetDeployer"),
            ("/v2/8453/deployments", "v2.deployments"),
            ("/v2/8453/deployments/0/key", "v2.deployments[].key"),
            (
                "/v2/8453/deployments/1/codehash",
                "v2.deployments[].codehash",
            ),
            ("/v2/8453/receiptBeacon", "v2 beacon"),
            ("/v2/8453/receiptVaultBeacon/beacon", "v2 beacon"),
            (
                "/v2/8453/wrappedTokenVaultBeacon/implementation",
                "v2 beacon implementation",
            ),
            ("/v2/8453/receiptBeacon/owner", "v2 beacon owner"),
        ];
        for (at, field) in cases {
            assert_eq!(v2_malformed(&remove(v2_good(), at)), field, "{at}");
        }
    }

    #[test]
    fn parse_v2_rejects_a_chain_keyed_by_anything_but_its_id() {
        let mut d = v2_good();
        let chains = d["v2"].as_object_mut().unwrap();
        let base = chains.remove("8453").unwrap();
        chains.insert("base".into(), base);
        assert_eq!(v2_malformed(&d), "chain id");
    }

    /// st0x.deploy's published file reads whole: every chain, Base first, tQSEP
    /// on Base alone, and every beacon at its own non-zero address.
    #[test]
    fn parses_the_published_file() {
        let got = parse(FIXTURE).unwrap();
        assert_eq!(
            got.iter().map(|c| c.chain_id).collect::<Vec<_>>(),
            [8453, 1, 56, 999, 4663]
        );
        for c in &got {
            assert_eq!(
                c.qsep.is_some(),
                c.network == REFERENCE_NETWORK,
                "{}",
                c.network
            );
            assert_eq!(c.safe.guard, None, "{}", c.network);
        }
        let base = &got[0];
        assert_eq!(
            base.beacons
                .iter()
                .map(|b| b.renamed_implementation.as_str())
                .collect::<Vec<_>>(),
            [
                RECEIPT_IMPL,
                RECEIPT_VAULT_RENAMED_IMPL,
                WTV_RENAMED_IMPL,
                ORCH_IMPL
            ]
        );
        let mstr = &base.tokens[0];
        assert_eq!(mstr.symbol, "tMSTR");
        assert_eq!(mstr.receipt_manager, BASE_MSTR.receipt_vault);
        let qsep = base.qsep.as_ref().unwrap();
        assert_eq!(qsep.receipt_vault, QSEP_RECEIPT_VAULT);
        assert_eq!(qsep.receipt_manager, QSEP_RECEIPT_VAULT);
        assert_eq!(qsep.owner, BASE_TIMELOCK);
        let eth = &got[1];
        assert_eq!(
            eth.beacons
                .iter()
                .map(|b| b.address.as_str())
                .collect::<Vec<_>>(),
            [
                ETH_RECEIPT_BEACON,
                ETH_RECEIPT_VAULT_BEACON,
                ETH_WTV_BEACON,
                ORCH_BEACON
            ]
        );
        assert_eq!(eth.tokens[0].receipt_manager, ETH_MSTR.receipt_vault);
    }

    #[test]
    fn parses_the_published_v2_deployments() {
        let got = parse_v2(FIXTURE).unwrap();
        assert_eq!(
            got.iter().map(|c| c.chain_id).collect::<Vec<_>>(),
            [8453, 14, 137, 42161, 84532]
        );
        let base = &got[0];
        assert_eq!(base.network, "base");
        assert_eq!(base.beacon_set_deployer, V2_DEPLOYER);
        assert_eq!(base.deployments.len(), 9);
        assert_eq!(base.deployments[0].key, "StoxReceipt");
        assert_eq!(base.deployments[0].code.address, V2_RECEIPT);
        assert_eq!(base.deployments[0].code.codehash, V2_RECEIPT_HASH);
        assert_eq!(base.beacons, v2_base_beacons());
    }
}
