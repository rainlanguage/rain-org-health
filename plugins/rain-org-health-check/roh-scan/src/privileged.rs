//! st0x.deploy's published `privileged-addresses.json`: per chain, the
//! token-owner Safe, the governance timelock and the receipt vaults the deploy
//! repo pins.

use crate::owners::REFERENCE_NETWORK;

pub const PRIVILEGED_ADDRESSES_PATH: &str = "privileged-addresses.json";

/// The only schema version this reader understands.
const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegedChain {
    pub chain_id: u64,
    pub network: String,
    /// `None` when the file carries the zero address.
    pub safe: Option<String>,
    /// `None` when the file carries the zero address.
    pub timelock: Option<String>,
    /// Lowercased, in file order.
    pub receipt_vaults: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivilegedError {
    Empty,
    NotJson,
    UnsupportedVersion(Option<u64>),
    NoChains,
    Malformed { chain: String, field: &'static str },
}

impl std::fmt::Display for PrivilegedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrivilegedError::Empty => write!(f, "{PRIVILEGED_ADDRESSES_PATH} could not be fetched"),
            PrivilegedError::NotJson => write!(f, "{PRIVILEGED_ADDRESSES_PATH} is not valid JSON"),
            PrivilegedError::UnsupportedVersion(v) => write!(
                f,
                "{PRIVILEGED_ADDRESSES_PATH} has schema version {}, this scanner reads {SCHEMA_VERSION}",
                v.map_or("none".to_string(), |v| v.to_string())
            ),
            PrivilegedError::NoChains => write!(f, "{PRIVILEGED_ADDRESSES_PATH} lists no chains"),
            PrivilegedError::Malformed { chain, field } => write!(
                f,
                "{PRIVILEGED_ADDRESSES_PATH} chain {chain} has a missing or malformed {field}"
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
pub fn parse(body: &str) -> Result<Vec<PrivilegedChain>, PrivilegedError> {
    if body.trim().is_empty() {
        return Err(PrivilegedError::Empty);
    }
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|_| PrivilegedError::NotJson)?;
    let version = doc["version"].as_u64();
    if version != Some(SCHEMA_VERSION) {
        return Err(PrivilegedError::UnsupportedVersion(version));
    }
    let chains = doc["chains"]
        .as_object()
        .filter(|m| !m.is_empty())
        .ok_or(PrivilegedError::NoChains)?;
    let mut out = Vec::with_capacity(chains.len());
    for (key, c) in chains {
        let bad = |field| PrivilegedError::Malformed {
            chain: key.clone(),
            field,
        };
        let chain_id = key.parse::<u64>().map_err(|_| bad("chain id"))?;
        let network = c["network"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| bad("network"))?
            .to_string();
        let pin = |field: &'static str| -> Result<Option<String>, PrivilegedError> {
            let a = c[field]
                .as_str()
                .filter(|a| is_address(a))
                .ok_or_else(|| bad(field))?;
            Ok((!is_zero(a)).then(|| a.to_string()))
        };
        let safe = pin("safe")?;
        let timelock = pin("timelock")?;
        let receipt_vaults = c["receiptVaults"]
            .as_array()
            .ok_or_else(|| bad("receiptVaults"))?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|a| is_address(a) && !is_zero(a))
                    .map(str::to_lowercase)
                    .ok_or_else(|| bad("receiptVaults"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(PrivilegedChain {
            chain_id,
            network,
            safe,
            timelock,
            receipt_vaults,
        });
    }
    out.sort_by_key(|c| (c.network != REFERENCE_NETWORK, c.chain_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_SAFE: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const BASE_TIMELOCK: &str = "0x48ba1371A78E6cC54157c63721756ab444510DB3";

    fn doc(chains: &str) -> String {
        format!(r#"{{"version": 1, "chains": {{{chains}}}}}"#)
    }

    fn chain(id: u64, network: &str, vaults: &str) -> String {
        format!(
            r#""{id}": {{"network": "{network}", "safe": "{BASE_SAFE}", "timelock": "{BASE_TIMELOCK}", "receiptVaults": [{vaults}]}}"#
        )
    }

    #[test]
    fn reads_each_chains_pins_and_vaults_base_first() {
        let body = doc(&[
            chain(1, "ethereum", r#""0x8500189061e2206Bc33Bf04DC10fFB1Fe7dED637""#),
            chain(
                8453,
                "base",
                r#""0x31F55e6dCF09E706Ba10e4cBB455dE0Fbf932BcA", "0x6eC2421d74F2AF61b7EE7bCCAE25A3b6a7961Aa0""#,
            ),
            chain(56, "bsc", ""),
        ]
        .join(","));
        let got = parse(&body).expect("parses");
        let order: Vec<_> = got.iter().map(|c| c.network.as_str()).collect();
        assert_eq!(order, ["base", "ethereum", "bsc"]);
        assert_eq!(got[0].chain_id, 8453);
        assert_eq!(got[0].safe.as_deref(), Some(BASE_SAFE));
        assert_eq!(got[0].timelock.as_deref(), Some(BASE_TIMELOCK));
        assert_eq!(
            got[0].receipt_vaults,
            [
                "0x31f55e6dcf09e706ba10e4cbb455de0fbf932bca",
                "0x6ec2421d74f2af61b7ee7bccae25a3b6a7961aa0"
            ]
        );
        assert!(got[2].receipt_vaults.is_empty());
    }

    #[test]
    fn a_zero_pin_is_absent_not_an_owner() {
        let body = doc(&format!(
            r#""999": {{"network": "hyperevm", "safe": "0x0000000000000000000000000000000000000000", "timelock": "0x0000000000000000000000000000000000000000", "receiptVaults": []}}"#
        ));
        let got = parse(&body).expect("parses");
        assert_eq!(got[0].safe, None);
        assert_eq!(got[0].timelock, None);
    }

    #[test]
    fn unreadable_input_is_an_error_never_an_empty_list() {
        assert_eq!(parse(""), Err(PrivilegedError::Empty));
        assert_eq!(parse("  \n"), Err(PrivilegedError::Empty));
        assert_eq!(parse("{not json"), Err(PrivilegedError::NotJson));
        assert_eq!(
            parse(r#"{"chains": {}}"#),
            Err(PrivilegedError::UnsupportedVersion(None))
        );
        assert_eq!(
            parse(r#"{"version": 2, "chains": {}}"#),
            Err(PrivilegedError::UnsupportedVersion(Some(2)))
        );
        assert_eq!(parse(&doc("")), Err(PrivilegedError::NoChains));
        assert_eq!(
            parse(r#"{"version": 1}"#),
            Err(PrivilegedError::NoChains)
        );
    }

    #[test]
    fn a_malformed_field_names_the_chain_and_field() {
        let cases = [
            (
                r#""x": {"network": "base", "safe": "0x0000000000000000000000000000000000000001", "timelock": "0x0000000000000000000000000000000000000001", "receiptVaults": []}"#,
                "chain id",
            ),
            (
                r#""8453": {"safe": "0x0000000000000000000000000000000000000001", "timelock": "0x0000000000000000000000000000000000000001", "receiptVaults": []}"#,
                "network",
            ),
            (
                r#""8453": {"network": "base", "safe": "0x01", "timelock": "0x0000000000000000000000000000000000000001", "receiptVaults": []}"#,
                "safe",
            ),
            (
                r#""8453": {"network": "base", "safe": "0x0000000000000000000000000000000000000001", "receiptVaults": []}"#,
                "timelock",
            ),
            (
                r#""8453": {"network": "base", "safe": "0x0000000000000000000000000000000000000001", "timelock": "0x0000000000000000000000000000000000000001"}"#,
                "receiptVaults",
            ),
            (
                r#""8453": {"network": "base", "safe": "0x0000000000000000000000000000000000000001", "timelock": "0x0000000000000000000000000000000000000001", "receiptVaults": ["0xzz"]}"#,
                "receiptVaults",
            ),
            (
                r#""8453": {"network": "base", "safe": "0x0000000000000000000000000000000000000001", "timelock": "0x0000000000000000000000000000000000000001", "receiptVaults": ["0x0000000000000000000000000000000000000000"]}"#,
                "receiptVaults",
            ),
        ];
        for (c, field) in cases {
            match parse(&doc(c)) {
                Err(PrivilegedError::Malformed { field: f, .. }) => assert_eq!(f, field, "{c}"),
                other => panic!("{c}: expected Malformed {field}, got {other:?}"),
            }
        }
    }
}
