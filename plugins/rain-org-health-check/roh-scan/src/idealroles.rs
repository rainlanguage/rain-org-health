//! The deploy repo's `ideal-roles.json`: the `(role, grantee)` pairs each
//! contract is expected to hold, per chain. Parsed as strictly as the page's
//! `idealParse`.

use serde_json::Value;

pub const PATH: &str = "ideal-roles.json";
pub const VERSION: u64 = 1;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Expected {
    /// `0x` + 64 lowercase hex.
    pub role: String,
    pub role_name: Option<String>,
    /// Lowercase.
    pub grantee: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Contract {
    /// Lowercase.
    pub address: String,
    pub expected: Vec<Expected>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IdealChain {
    pub network: String,
    pub contracts: Vec<Contract>,
}

impl IdealChain {
    pub fn contract(&self, address: &str) -> Option<&Contract> {
        self.contracts
            .iter()
            .find(|c| c.address.eq_ignore_ascii_case(address))
    }
}

fn hex(v: &Value, digits: usize) -> Option<String> {
    v.as_str()
        .filter(|s| {
            s.len() == 2 + digits
                && s.starts_with("0x")
                && s[2..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .map(str::to_lowercase)
}

pub fn parse(body: &str) -> Result<Vec<IdealChain>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "it is not JSON".to_string())?;
    if !v.is_object() {
        return Err("it is not a JSON object".into());
    }
    if v["version"].as_u64() != Some(VERSION) {
        return Err(format!(
            "it is version {}; this scan reads version {VERSION}",
            v["version"]
        ));
    }
    let chains = v["chains"].as_object().ok_or("it has no chains object")?;
    let mut out: Vec<IdealChain> = Vec::new();
    for (id, c) in chains {
        let at = format!("chains[{id:?}]");
        let network = c["network"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("{at} names no network"))?;
        if out.iter().any(|o| o.network == network) {
            return Err(format!("{at} repeats network {network}"));
        }
        let mut contracts = Vec::new();
        for (i, k) in c["contracts"]
            .as_array()
            .ok_or_else(|| format!("{at} has no contracts list"))?
            .iter()
            .enumerate()
        {
            let kat = format!("{at}.contracts[{i}]");
            let address = hex(&k["address"], 40).ok_or_else(|| format!("{kat} has no address"))?;
            let mut expected = Vec::new();
            for (j, p) in k["expected"]
                .as_array()
                .ok_or_else(|| format!("{kat} has no expected list"))?
                .iter()
                .enumerate()
            {
                let pat = format!("{kat}.expected[{j}]");
                expected.push(Expected {
                    role: hex(&p["role"], 64)
                        .ok_or_else(|| format!("{pat} has no bytes32 role"))?,
                    role_name: p["roleName"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                    grantee: hex(&p["grantee"], 40)
                        .ok_or_else(|| format!("{pat} has no grantee address"))?,
                });
            }
            contracts.push(Contract { address, expected });
        }
        out.push(IdealChain {
            network: network.to_string(),
            contracts,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ROLE: &str = "0x1ae915b310cb86de75afe5db1721d474dd0a8617151f7524866025476454bc02";

    fn doc() -> Value {
        json!({"version": 1, "chains": {"8453": {"network": "base", "contracts": [
            {"address": "0x00000000000000000000000000000000000000Aa", "expected": [
                {"role": ROLE.to_uppercase().replace("0X", "0x"), "grantee": "0x00000000000000000000000000000000000000Bb", "roleName": "DEPOSIT"},
                {"role": ROLE, "grantee": "0x00000000000000000000000000000000000000cc"},
            ]},
        ]}}})
    }

    fn rejects(edit: impl Fn(&mut Value), why: &str) {
        let mut d = doc();
        edit(&mut d);
        let err = parse(&d.to_string()).expect_err(why);
        assert!(err.contains(why), "{err:?} does not mention {why:?}");
    }

    #[test]
    fn parses_and_lowercases() {
        let chains = parse(&doc().to_string()).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].network, "base");
        let k = chains[0]
            .contract("0x00000000000000000000000000000000000000AA")
            .unwrap();
        assert_eq!(k.address, "0x00000000000000000000000000000000000000aa");
        assert_eq!(
            k.expected,
            vec![
                Expected {
                    role: ROLE.into(),
                    role_name: Some("DEPOSIT".into()),
                    grantee: "0x00000000000000000000000000000000000000bb".into()
                },
                Expected {
                    role: ROLE.into(),
                    role_name: None,
                    grantee: "0x00000000000000000000000000000000000000cc".into()
                },
            ]
        );
        assert!(chains[0]
            .contract("0x00000000000000000000000000000000000000ab")
            .is_none());
    }

    #[test]
    fn one_malformed_entry_rejects_the_file() {
        assert_eq!(parse("{").unwrap_err(), "it is not JSON");
        assert_eq!(parse("[]").unwrap_err(), "it is not a JSON object");
        rejects(|d| d["version"] = json!(2), "it is version 2");
        rejects(|d| d["chains"] = json!([]), "it has no chains object");
        rejects(
            |d| d["chains"]["8453"]["network"] = json!(""),
            "chains[\"8453\"] names no network",
        );
        rejects(
            |d| d["chains"]["1"] = d["chains"]["8453"].clone(),
            "repeats network base",
        );
        rejects(
            |d| d["chains"]["8453"]["contracts"] = json!({}),
            "has no contracts list",
        );
        rejects(
            |d| d["chains"]["8453"]["contracts"][0]["address"] = json!("0x12"),
            "contracts[0] has no address",
        );
        rejects(
            |d| d["chains"]["8453"]["contracts"][0]["expected"] = json!(null),
            "contracts[0] has no expected list",
        );
        rejects(
            |d| d["chains"]["8453"]["contracts"][0]["expected"][1]["role"] = json!("0x01"),
            "expected[1] has no bytes32 role",
        );
        rejects(
            |d| d["chains"]["8453"]["contracts"][0]["expected"][0]["grantee"] = json!(ROLE),
            "expected[0] has no grantee address",
        );
    }

    #[test]
    fn parses_the_published_file() {
        let chains = parse(include_str!("../tests/fixtures/ideal-roles.json")).unwrap();
        assert!(chains
            .iter()
            .any(|c| c.network == crate::owners::REFERENCE_NETWORK));
        assert!(chains.iter().all(|c| !c.contracts.is_empty()));
    }
}
