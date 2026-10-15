//! The deploy repo's `privileged-addresses.json`: every address that controls
//! production, per chain, as the deploy repo publishes it.

use serde_json::{Map, Value};

pub const PATH: &str = "privileged-addresses.json";
pub const VERSION: u64 = 1;

/// The schema's beacon order: `chains.<id>.beacons[i]` is the `BEACON_SLOTS[i]` beacon.
pub const BEACON_SLOTS: [&str; 4] = [
    "Receipt beacon",
    "Receipt-vault beacon",
    "Wrapped-token-vault beacon",
    "Orchestrator beacon",
];

/// The beacon-set deployer getter for a slot whose `beacon` is `address(0)`.
pub fn beacon_getter_calldata(slot: usize) -> Option<String> {
    match slot {
        0 => Some(crate::rpc::receipt_beacon_calldata()),
        1 => Some(crate::rpc::receipt_vault_beacon_calldata()),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Beacon {
    pub beacon: Option<String>,
    pub implementation: Option<String>,
    pub v1_implementation: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Chain {
    pub chain_id: u64,
    pub network: String,
    pub safe: Option<String>,
    pub timelock: Option<String>,
    pub authoriser: Option<String>,
    pub beacon_set_deployer: Option<String>,
    pub beacons: Vec<Beacon>,
    pub receipt_vaults: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PrivilegedAddresses {
    pub threshold: u64,
    pub owners: Vec<String>,
    pub authoriser_implementation: Option<String>,
    pub beacon_owner_initial: Option<String>,
    pub beacon_owner_v2_receipt: Option<String>,
    pub beacon_owner_v2_vault: Option<String>,
    /// Sorted by chain id.
    pub chains: Vec<Chain>,
}

impl PrivilegedAddresses {
    pub fn chain(&self, network: &str) -> Option<&Chain> {
        self.chains.iter().find(|c| c.network == network)
    }
}

fn address(v: &Value, at: &str) -> Result<Option<String>, String> {
    let s = v
        .as_str()
        .filter(|s| s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{at} is not an address"))?;
    Ok(Some(s.to_string()).filter(|s| !s[2..].chars().all(|c| c == '0')))
}

fn nonzero(v: &Value, at: &str) -> Result<String, String> {
    address(v, at)?.ok_or_else(|| format!("{at} is address(0)"))
}

fn object<'a>(v: &'a Value, at: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object().ok_or_else(|| format!("{at} is not an object"))
}

fn array<'a>(v: &'a Value, at: &str) -> Result<&'a Vec<Value>, String> {
    v.as_array().ok_or_else(|| format!("{at} is not a list"))
}

fn chain(id: &str, v: &Value) -> Result<Chain, String> {
    let at = format!("chains[{id:?}]");
    let chain_id = Some(id)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{at} is not a decimal chain id"))?;
    let c = object(v, &at)?;
    let network = c["network"]
        .as_str()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("{at} names no network"))?
        .to_string();
    let f = |k: &str| address(&c[k], &format!("{at}.{k}"));
    let raw_beacons = array(&c["beacons"], &format!("{at}.beacons"))?;
    if raw_beacons.len() != BEACON_SLOTS.len() {
        return Err(format!("{at}.beacons has {} entries, not {}", raw_beacons.len(), BEACON_SLOTS.len()));
    }
    let beacons = raw_beacons
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let bat = format!("{at}.beacons[{i}]");
            object(b, &bat)?;
            Ok(Beacon {
                beacon: address(&b["beacon"], &format!("{bat}.beacon"))?,
                implementation: address(&b["implementation"], &format!("{bat}.implementation"))?,
                v1_implementation: address(&b["v1Implementation"], &format!("{bat}.v1Implementation"))?,
            })
        })
        .collect::<Result<_, String>>()?;
    let receipt_vaults = array(&c["receiptVaults"], &format!("{at}.receiptVaults"))?
        .iter()
        .enumerate()
        .map(|(i, a)| nonzero(a, &format!("{at}.receiptVaults[{i}]")))
        .collect::<Result<_, String>>()?;
    Ok(Chain {
        chain_id,
        network,
        safe: f("safe")?,
        timelock: f("timelock")?,
        authoriser: f("authoriser")?,
        beacon_set_deployer: f("beaconSetDeployer")?,
        beacons,
        receipt_vaults,
    })
}

/// Strict: one malformed field rejects the whole file, so a dropped entry can
/// never read as an absent one.
pub fn parse(body: &str) -> Result<PrivilegedAddresses, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "it is not JSON".to_string())?;
    let root = object(&v, "it")?;
    if root.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(format!(
            "it is version {}; this scan reads version {VERSION}",
            root.get("version").unwrap_or(&Value::Null)
        ));
    }
    let policy = object(&v["safePolicy"], "safePolicy")?;
    let owners: Vec<String> = array(&policy["owners"], "safePolicy.owners")?
        .iter()
        .enumerate()
        .map(|(i, a)| nonzero(a, &format!("safePolicy.owners[{i}]")))
        .collect::<Result<_, String>>()?;
    if owners.is_empty() {
        return Err("safePolicy.owners is empty".into());
    }
    let threshold = policy["threshold"]
        .as_u64()
        .filter(|t| (1..=owners.len() as u64).contains(t))
        .ok_or_else(|| format!("safePolicy.threshold is not between 1 and {}", owners.len()))?;
    let beacon_owners = object(&v["beaconOwners"], "beaconOwners")?;
    let bo = |k: &str| address(&beacon_owners[k], &format!("beaconOwners.{k}"));
    let mut chains: Vec<Chain> = object(&v["chains"], "chains")?
        .iter()
        .map(|(id, c)| chain(id, c))
        .collect::<Result<_, String>>()?;
    if chains.is_empty() {
        return Err("chains is empty".into());
    }
    chains.sort_by_key(|c| c.chain_id);
    for (i, c) in chains.iter().enumerate() {
        if chains[..i].iter().any(|p| p.network == c.network) {
            return Err(format!("chains[\"{}\"] repeats network {}", c.chain_id, c.network));
        }
    }
    Ok(PrivilegedAddresses {
        threshold,
        owners,
        authoriser_implementation: address(&v["authoriserImplementation"], "authoriserImplementation")?,
        beacon_owner_initial: bo("initial")?,
        beacon_owner_v2_receipt: bo("v2BaseReceipt")?,
        beacon_owner_v2_vault: bo("v2BaseVault")?,
        chains,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;

    pub const A: &str = "0x00000000000000000000000000000000000000a1";
    pub const B: &str = "0x00000000000000000000000000000000000000b2";
    pub const Z: &str = "0x0000000000000000000000000000000000000000";

    fn beacons() -> Value {
        json!([
            {"beacon": Z, "implementation": A, "v1Implementation": Z},
            {"beacon": Z, "implementation": A, "v1Implementation": Z},
            {"beacon": B, "implementation": A, "v1Implementation": B},
            {"beacon": B, "implementation": A, "v1Implementation": Z},
        ])
    }

    pub fn doc() -> Value {
        json!({
            "version": 1,
            "safePolicy": {"threshold": 2, "owners": [A, B]},
            "authoriserImplementation": A,
            "beaconOwners": {"initial": A, "v2BaseReceipt": B, "v2BaseVault": Z},
            "chains": {
                "8453": {"network": "base", "safe": A, "timelock": B, "authoriser": A,
                         "beaconSetDeployer": Z, "beacons": beacons(), "receiptVaults": [A, B]},
                "1": {"network": "ethereum", "safe": B, "timelock": Z, "authoriser": B,
                      "beaconSetDeployer": A, "beacons": beacons(), "receiptVaults": [B]},
            },
        })
    }

    fn rejects(edit: impl Fn(&mut Value), why: &str) {
        let mut d = doc();
        edit(&mut d);
        let err = parse(&d.to_string()).expect_err(why);
        assert!(err.contains(why), "{err:?} does not mention {why:?}");
    }

    #[test]
    fn parses_every_field() {
        let p = parse(&doc().to_string()).unwrap();
        assert_eq!(p.threshold, 2);
        assert_eq!(p.owners, vec![A, B]);
        assert_eq!(p.authoriser_implementation.as_deref(), Some(A));
        assert_eq!(p.beacon_owner_initial.as_deref(), Some(A));
        assert_eq!(p.beacon_owner_v2_receipt.as_deref(), Some(B));
        assert_eq!(p.beacon_owner_v2_vault, None);
        assert_eq!(p.chains.iter().map(|c| c.chain_id).collect::<Vec<_>>(), vec![1, 8453]);
        let base = p.chain("base").unwrap();
        assert_eq!(
            (base.safe.as_deref(), base.timelock.as_deref(), base.authoriser.as_deref(), base.beacon_set_deployer.as_deref()),
            (Some(A), Some(B), Some(A), None)
        );
        assert_eq!(base.receipt_vaults, vec![A, B]);
        assert_eq!(base.beacons.len(), 4);
        assert_eq!(base.beacons[0], Beacon { beacon: None, implementation: Some(A.into()), v1_implementation: None });
        assert_eq!(base.beacons[2].v1_implementation.as_deref(), Some(B));
        let eth = p.chain("ethereum").unwrap();
        assert_eq!((eth.timelock.as_deref(), eth.beacon_set_deployer.as_deref()), (None, Some(A)));
        assert!(p.chain("bsc").is_none());
    }

    #[test]
    fn keeps_the_checksum_as_written() {
        let mut d = doc();
        d["chains"]["1"]["safe"] = json!("0xAbCdEf0000000000000000000000000000000001");
        let p = parse(&d.to_string()).unwrap();
        assert_eq!(p.chain("ethereum").unwrap().safe.as_deref(), Some("0xAbCdEf0000000000000000000000000000000001"));
    }

    #[test]
    fn rejects_a_malformed_file() {
        assert_eq!(parse("nope").unwrap_err(), "it is not JSON");
        assert!(parse("[]").unwrap_err().contains("not an object"));
        rejects(|d| d["version"] = json!(2), "version 2");
        rejects(|d| d.as_object_mut().unwrap().remove("version").map(|_| ()).unwrap(), "version null");
        rejects(|d| d["safePolicy"]["owners"] = json!([]), "safePolicy.owners is empty");
        rejects(|d| d["safePolicy"]["owners"][1] = json!(Z), "safePolicy.owners[1] is address(0)");
        rejects(|d| d["safePolicy"]["owners"][0] = json!("0x12"), "safePolicy.owners[0] is not an address");
        rejects(|d| d["safePolicy"]["threshold"] = json!(0), "threshold is not between 1 and 2");
        rejects(|d| d["safePolicy"]["threshold"] = json!(3), "threshold is not between 1 and 2");
        rejects(|d| d["safePolicy"] = json!(null), "safePolicy is not an object");
        rejects(|d| d["beaconOwners"]["initial"] = json!(7), "beaconOwners.initial is not an address");
        rejects(|d| d["authoriserImplementation"] = json!("0xzz00000000000000000000000000000000000000"), "authoriserImplementation is not an address");
        rejects(|d| d["chains"] = json!({}), "chains is empty");
        rejects(|d| d["chains"]["0x1"] = d["chains"]["1"].clone(), "chains[\"0x1\"] is not a decimal chain id");
        rejects(|d| d["chains"][""] = d["chains"]["1"].clone(), "is not a decimal chain id");
        rejects(|d| d["chains"]["10"] = d["chains"]["1"].clone(), "chains[\"10\"] repeats network ethereum");
        rejects(|d| d["chains"]["1"]["network"] = json!(""), "chains[\"1\"] names no network");
        rejects(|d| d["chains"]["1"]["safe"] = json!(null), "chains[\"1\"].safe is not an address");
        rejects(|d| d["chains"]["1"]["timelock"] = json!(1), "chains[\"1\"].timelock is not an address");
        rejects(|d| d["chains"]["1"]["authoriser"] = json!(""), "chains[\"1\"].authoriser is not an address");
        rejects(|d| d["chains"]["1"]["beaconSetDeployer"] = json!(""), "chains[\"1\"].beaconSetDeployer is not an address");
        rejects(|d| d["chains"]["1"]["beacons"].as_array_mut().unwrap().truncate(3), "beacons has 3 entries, not 4");
        rejects(|d| d["chains"]["1"]["beacons"][0] = json!(A), "chains[\"1\"].beacons[0] is not an object");
        rejects(|d| d["chains"]["1"]["beacons"][3]["beacon"] = json!(1), "beacons[3].beacon is not an address");
        rejects(|d| d["chains"]["1"]["beacons"][3]["implementation"] = json!(1), "beacons[3].implementation is not an address");
        rejects(|d| d["chains"]["1"]["beacons"][3]["v1Implementation"] = json!(1), "beacons[3].v1Implementation is not an address");
        rejects(|d| d["chains"]["1"]["receiptVaults"] = json!(A), "chains[\"1\"].receiptVaults is not a list");
        rejects(|d| d["chains"]["1"]["receiptVaults"][0] = json!(Z), "chains[\"1\"].receiptVaults[0] is address(0)");
        rejects(|d| d["chains"]["1"] = json!([]), "chains[\"1\"] is not an object");
    }

    #[test]
    fn the_two_getter_slots_name_their_getters() {
        assert_eq!(beacon_getter_calldata(0), Some(crate::rpc::receipt_beacon_calldata()));
        assert_eq!(beacon_getter_calldata(1), Some(crate::rpc::receipt_vault_beacon_calldata()));
        assert_eq!(beacon_getter_calldata(2), None);
        assert_eq!(beacon_getter_calldata(3), None);
    }

    #[test]
    fn parses_the_published_file() {
        let body = include_str!("../tests/fixtures/privileged-addresses.json");
        let p = parse(body).unwrap();
        assert_eq!(p.owners.len(), 6);
        assert!(p.chain(crate::owners::REFERENCE_NETWORK).is_some());
    }
}
