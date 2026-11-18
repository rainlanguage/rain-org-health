//! Listing facts (`assetId`, `listingTicker`, `listingName`) compared against
//! st0x.registry, Nasdaq and GLEIF. Only inconsistencies are surfaced; no
//! source is treated as the winner.

use serde_json::json;
use std::collections::{HashMap, HashSet};

pub const GLEIF_ISIN_LEI_URL: &str = "https://mapping.gleif.org/api/v2/isin-lei/latest/download";
pub const NASDAQ_LISTED_URL: &str = "https://www.nasdaqtrader.com/dynamic/SymDir/nasdaqlisted.txt";
pub const NASDAQ_OTHER_URL: &str = "https://www.nasdaqtrader.com/dynamic/SymDir/otherlisted.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Consistent,
    Inconsistent,
    Unavailable,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Consistent => "consistent",
            Status::Inconsistent => "inconsistent",
            Status::Unavailable => "unavailable",
        }
    }
}

/// One registry entry, keyed by its wrapped vault address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryListing {
    pub isin: Option<String>,
    pub ticker: Option<String>,
    pub name: Option<String>,
}

/// `tokens[].address` (lowercased) → its listing facts.
pub fn parse_registry(body: &str) -> Option<HashMap<String, RegistryListing>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let tokens = v["tokens"].as_array()?;
    Some(
        tokens
            .iter()
            .filter_map(|t| {
                let address = t["address"].as_str()?.to_lowercase();
                let ext = &t["extensions"];
                let s = |v: &serde_json::Value| {
                    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
                };
                // The ticker lives only in `EXCHANGE:TICKER`.
                let ticker = s(&ext["tradingViewSymbol"])
                    .map(|tv| tv.rsplit(':').next().unwrap_or_default().to_string());
                // The registry `name` is the wrapped token's, so the listing name is what remains.
                let name = s(&t["name"]).map(|n| {
                    n.strip_prefix("Wrapped ")
                        .unwrap_or(&n)
                        .strip_suffix(" ST0x")
                        .unwrap_or(n.strip_prefix("Wrapped ").unwrap_or(&n))
                        .to_string()
                });
                Some((
                    address,
                    RegistryListing {
                        isin: s(&ext["isin"]),
                        ticker,
                        name,
                    },
                ))
            })
            .collect(),
    )
}

/// Nasdaq symbol directory: symbol → security name, from `nasdaqlisted.txt`
/// (`Symbol`) or `otherlisted.txt` (`ACT Symbol`). `None` when the header is
/// unrecognised or the `File Creation Time` trailer is missing (truncated read).
pub fn parse_nasdaq(body: &str) -> Option<HashMap<String, String>> {
    let mut lines = body.lines();
    let header: Vec<&str> = lines.next()?.split('|').collect();
    let sym = header
        .iter()
        .position(|h| *h == "Symbol" || *h == "ACT Symbol")?;
    let name = header.iter().position(|h| *h == "Security Name")?;
    let mut out = HashMap::new();
    let mut trailer = false;
    for line in lines {
        if line.starts_with("File Creation Time") {
            trailer = true;
            break;
        }
        let cols: Vec<&str> = line.split('|').collect();
        if let (Some(s), Some(n)) = (cols.get(sym), cols.get(name)) {
            out.insert(s.to_string(), n.to_string());
        }
    }
    trailer.then_some(out)
}

/// Streams GLEIF's `LEI,ISIN` CSV, keeping the `wanted` ISINs it lists. `None`
/// without the header or on a read error.
pub fn gleif_present(
    r: impl std::io::BufRead,
    wanted: &HashSet<String>,
) -> Option<HashSet<String>> {
    let mut lines = r.lines();
    if lines.next()?.ok()?.trim() != "LEI,ISIN" {
        return None;
    }
    let mut found = HashSet::new();
    for l in lines {
        let l = l.ok()?;
        if let Some(i) = l.split(',').nth(1).map(str::trim) {
            if wanted.contains(i) {
                found.insert(i.to_string());
            }
        }
    }
    Some(found)
}

/// The listing facts a token declares, all optional until every token carries them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declared<'a> {
    pub symbol: &'a str,
    pub wrapped_token_vault: &'a str,
    pub asset_id: Option<&'a str>,
    pub listing_ticker: Option<&'a str>,
    pub listing_name: Option<&'a str>,
}

/// What each source returned at scan time; `None` is an unreadable source.
#[derive(Debug, Default)]
pub struct Sources {
    pub registry: Option<HashMap<String, RegistryListing>>,
    pub nasdaq: Option<HashMap<String, String>>,
    pub gleif: Option<HashSet<String>>,
}

fn compare(ours: Option<&str>, theirs: Option<Option<&str>>) -> Status {
    match (ours, theirs) {
        (Some(o), Some(Some(t))) if o == t => Status::Consistent,
        (Some(_), Some(Some(_))) => Status::Inconsistent,
        _ => Status::Unavailable,
    }
}

fn row(
    t: &Declared,
    field: &str,
    ours: Option<&str>,
    source: &str,
    status: Status,
) -> serde_json::Value {
    json!({
        "token": t.symbol,
        "field": field,
        "value": ours,
        "source": source,
        "status": status.as_str(),
    })
}

/// Every (field, source) row for one token. Nasdaq's text is never emitted.
pub fn token_rows(t: &Declared, s: &Sources) -> Vec<serde_json::Value> {
    let reg = s
        .registry
        .as_ref()
        .map(|r| r.get(&t.wrapped_token_vault.to_lowercase()));
    let reg_field = |f: fn(&RegistryListing) -> Option<&str>| reg.map(|e| e.and_then(f));
    let nasdaq_name = t
        .listing_ticker
        .and_then(|tk| s.nasdaq.as_ref().map(|n| n.get(tk).map(String::as_str)));
    let nasdaq_ticker = match (t.listing_ticker, s.nasdaq.as_ref()) {
        (Some(tk), Some(n)) => {
            if n.contains_key(tk) {
                Status::Consistent
            } else {
                Status::Inconsistent
            }
        }
        _ => Status::Unavailable,
    };
    let gleif = match (t.asset_id, s.gleif.as_ref()) {
        (Some(i), Some(g)) => {
            if g.contains(i) {
                Status::Consistent
            } else {
                Status::Inconsistent
            }
        }
        _ => Status::Unavailable,
    };
    vec![
        row(
            t,
            "assetId",
            t.asset_id,
            "st0x.registry",
            compare(t.asset_id, reg_field(|e| e.isin.as_deref())),
        ),
        row(t, "assetId", t.asset_id, "GLEIF", gleif),
        row(
            t,
            "listingTicker",
            t.listing_ticker,
            "st0x.registry",
            compare(t.listing_ticker, reg_field(|e| e.ticker.as_deref())),
        ),
        row(
            t,
            "listingTicker",
            t.listing_ticker,
            "Nasdaq",
            nasdaq_ticker,
        ),
        row(
            t,
            "listingName",
            t.listing_name,
            "st0x.registry",
            compare(t.listing_name, reg_field(|e| e.name.as_deref())),
        ),
        row(
            t,
            "listingName",
            t.listing_name,
            "Nasdaq",
            compare(t.listing_name, nasdaq_name),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str = r#"{"tokens":[{"address":"0xAAAA","name":"Wrapped NVIDIA Corporation ST0x","extensions":{"isin":"US67066G1040","tradingViewSymbol":"NASDAQ:NVDA"}}]}"#;
    const NASDAQ: &str = "Symbol|Security Name|Market Category|Test Issue|Financial Status|Round Lot Size|ETF|NextShares\nNVDA|NVIDIA Corporation - Common Stock|Q|N|N|100|N|N\nFile Creation Time: 1118202612:00|||||||\n";
    const OTHER: &str = "ACT Symbol|Security Name|Exchange|CQS Symbol|ETF|Round Lot Size|Test Issue|NASDAQ Symbol\nIBM|International Business Machines Corporation Common Stock|N|IBM|N|100|N|IBM\nFile Creation Time: 1118202612:00|||||||\n";
    const GLEIF: &str = "LEI,ISIN\n549300S4KLFTLO7GSQ80,US67066G1040\n";

    fn nvda() -> Declared<'static> {
        Declared {
            symbol: "tNVDA",
            wrapped_token_vault: "0xaaaa",
            asset_id: Some("US67066G1040"),
            listing_ticker: Some("NVDA"),
            listing_name: Some("NVIDIA Corporation"),
        }
    }

    fn sources() -> Sources {
        Sources {
            registry: parse_registry(REGISTRY),
            nasdaq: parse_nasdaq(NASDAQ),
            gleif: gleif_present(GLEIF.as_bytes(), &wanted()),
        }
    }

    fn wanted() -> HashSet<String> {
        HashSet::from(["US67066G1040".to_string(), "US0000000000".to_string()])
    }

    fn status(rows: &[serde_json::Value], field: &str, source: &str) -> String {
        rows.iter()
            .find(|r| r["field"] == field && r["source"] == source)
            .unwrap()["status"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn registry_strips_wrapper_from_name_and_exchange_from_ticker() {
        let r = parse_registry(REGISTRY).unwrap();
        let e = &r["0xaaaa"];
        assert_eq!(e.name.as_deref(), Some("NVIDIA Corporation"));
        assert_eq!(e.ticker.as_deref(), Some("NVDA"));
        assert_eq!(e.isin.as_deref(), Some("US67066G1040"));
    }

    #[test]
    fn nasdaq_reads_both_directory_headers_and_requires_the_trailer() {
        assert!(parse_nasdaq(NASDAQ).unwrap().contains_key("NVDA"));
        assert!(parse_nasdaq(OTHER).unwrap().contains_key("IBM"));
        assert!(parse_nasdaq("Symbol|Security Name\nNVDA|x\n").is_none());
    }

    #[test]
    fn gleif_requires_its_header() {
        assert!(gleif_present(GLEIF.as_bytes(), &wanted())
            .unwrap()
            .contains("US67066G1040"));
        assert!(gleif_present("ISIN\nUS67066G1040\n".as_bytes(), &wanted()).is_none());
    }

    #[test]
    fn matching_sources_are_consistent_except_a_differing_nasdaq_name() {
        let rows = token_rows(&nvda(), &sources());
        assert_eq!(status(&rows, "assetId", "st0x.registry"), "consistent");
        assert_eq!(status(&rows, "assetId", "GLEIF"), "consistent");
        assert_eq!(
            status(&rows, "listingTicker", "st0x.registry"),
            "consistent"
        );
        assert_eq!(status(&rows, "listingTicker", "Nasdaq"), "consistent");
        assert_eq!(status(&rows, "listingName", "st0x.registry"), "consistent");
        assert_eq!(status(&rows, "listingName", "Nasdaq"), "inconsistent");
    }

    #[test]
    fn differing_values_are_inconsistent() {
        let t = Declared {
            asset_id: Some("US0000000000"),
            listing_ticker: Some("NVDX"),
            listing_name: Some("Other"),
            ..nvda()
        };
        let rows = token_rows(&t, &sources());
        for (f, s) in [
            ("assetId", "st0x.registry"),
            ("assetId", "GLEIF"),
            ("listingTicker", "st0x.registry"),
            ("listingTicker", "Nasdaq"),
            ("listingName", "st0x.registry"),
        ] {
            assert_eq!(status(&rows, f, s), "inconsistent", "{f} {s}");
        }
    }

    #[test]
    fn unreadable_sources_and_missing_fields_are_unavailable_never_consistent() {
        let rows = token_rows(&nvda(), &Sources::default());
        assert!(rows.iter().all(|r| r["status"] == "unavailable"));
        let t = Declared {
            asset_id: None,
            listing_ticker: None,
            listing_name: None,
            ..nvda()
        };
        let rows = token_rows(&t, &sources());
        assert!(rows.iter().all(|r| r["status"] == "unavailable"));
    }

    #[test]
    fn nasdaq_text_never_reaches_the_output() {
        let rows = token_rows(&nvda(), &sources());
        let out = serde_json::to_string(&rows).unwrap();
        assert!(!out.contains("Common Stock"));
    }
}
