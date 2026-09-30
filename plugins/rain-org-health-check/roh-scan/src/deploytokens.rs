//! The production tokens on every chain (rain-org-health#182): each chain's
//! pinned token table (`LibTokenInvariants.productionTokens<Chain>()`) checked
//! against that chain, token by token. This is the `deploymentPinnedTokens`
//! view; `deploymentTokens` stays the st0x.registry reconciliation (#90), which
//! reads a different source and asks a different question.
//!
//! Each token is a receipt, a receipt vault and a wrapped-token vault. All
//! three must have code and proxy the chain's receipt, receipt-vault and
//! wrapped beacons (the ones `deploybeacons` resolves); the vault and receipt
//! must name each other; the wrapped vault's asset must be the vault; the vault
//! must answer to the chain's V4 authoriser clone and be owned by the chain's
//! token-owner Safe; and the vault's `name()` and `symbol()` must equal, byte
//! for byte, the canonical config `LibProdTokenConfig.productionTokenConfigs()`
//! gives its underlying.
//!
//! Every expected value is read out of the deploy repo. Rows share
//! `deploystate`'s shape and verdicts: `pass`, `fail` or `unknown`, where
//! `unknown` means the question could not be asked or answered and is never a
//! stand-in for `fail`.

use crate::deploybeacons::address_row;
use crate::deployhealth::function_body;
use crate::deploystate::{
    rollup, row, slot_address_row, subject, ChainReads, Verdict, ERC1967_BEACON_SLOT,
};
use crate::owners::{parse_address_constant, ChainPin};
use crate::rpc;
use regex::Regex;
use serde_json::{json, Value};

/// The token tables.
pub const TOKEN_INVARIANTS: &str = "src/lib/LibTokenInvariants.sol";
/// The canonical names and symbols.
pub const TOKEN_CONFIG: &str = "src/lib/LibProdTokenConfig.sol";

/// `src` with every `//` comment cut, so a commented-out row or constant is not
/// read as a live one. Neither table holds a `//` inside a string.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split at top-level commas.
fn split_args(s: &str) -> Vec<String> {
    let (mut depth, mut cur, mut out) = (0usize, String::new(), Vec::new());
    let mut in_str = false;
    for ch in s.chars() {
        match ch {
            '"' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth = depth.saturating_sub(1),
            ',' if !in_str && depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur.trim().to_string());
    out
}

/// One address argument of a table row: as written, and what it resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub written: String,
    pub address: Option<String>,
}

/// One row of a token table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPin {
    pub index: usize,
    pub underlying: String,
    pub receipt: Arg,
    pub receipt_vault: Arg,
    pub wrapped: Arg,
}

/// A chain's token table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TokenTable {
    /// The function the table was read from; `None` when the chain has none.
    pub function: Option<String>,
    /// The length it allocates (`new TokenInstance[](N)`).
    pub declared: Option<usize>,
    pub tokens: Vec<TokenPin>,
}

/// The table function for `network`: the `productionTokens<Suffix>` whose
/// suffix, lowercased, is the network (`HyperEvm` is `hyperevm`).
pub fn table_function(tok_lib: &str, network: &str) -> Option<String> {
    let re = Regex::new(r"function\s+productionTokens(\w+)\s*\(").ok()?;
    for c in re.captures_iter(tok_lib) {
        if c[1].to_lowercase() == network {
            return Some(format!("productionTokens{}", &c[1]));
        }
    }
    None
}

/// Resolve one argument: `address(0x…)`, a bare `0x…`, or a constant of the
/// same library.
fn resolve_arg(lib: &str, written: &str) -> Arg {
    let lit = Regex::new(r"^(?:address\(\s*)?(0x[0-9a-fA-F]{40})\s*\)?$").ok();
    let ident = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").ok();
    let address = match (lit, ident) {
        (Some(l), _) if l.is_match(written) => l.captures(written).map(|c| c[1].to_string()),
        (_, Some(i)) if i.is_match(written) => parse_address_constant(lib, written),
        _ => None,
    };
    Arg {
        written: written.to_string(),
        address,
    }
}

/// Read `network`'s token table out of `LibTokenInvariants`.
pub fn parse_table(tok_lib: &str, network: &str) -> TokenTable {
    let lib = strip_line_comments(tok_lib);
    let Some(function) = table_function(&lib, network) else {
        return TokenTable::default();
    };
    let Some(body) = function_body(&lib, &function) else {
        return TokenTable {
            function: Some(function),
            ..TokenTable::default()
        };
    };
    let declared = Regex::new(r"new\s+TokenInstance\[\]\(\s*(\d+)\s*\)")
        .ok()
        .and_then(|re| re.captures(body))
        .and_then(|c| c[1].parse().ok());
    let mut tokens = Vec::new();
    if let Ok(re) = Regex::new(r"(?s)tokens\[(\d+)\]\s*=\s*TokenInstance\((.*?)\)\s*;") {
        for c in re.captures_iter(body) {
            let args = split_args(&c[2]);
            let (Ok(index), [u, a, b, w]) = (c[1].parse::<usize>(), args.as_slice()) else {
                continue;
            };
            let Some(underlying) = u.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
                continue;
            };
            tokens.push(TokenPin {
                index,
                underlying: underlying.to_string(),
                receipt: resolve_arg(&lib, a),
                receipt_vault: resolve_arg(&lib, b),
                wrapped: resolve_arg(&lib, w),
            });
        }
    }
    TokenTable {
        function: Some(function),
        declared,
        tokens,
    }
}

/// A token's canonical ERC-20 identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenConfig {
    pub underlying: String,
    pub name: String,
    pub symbol: String,
}

/// `LibProdTokenConfig.productionTokenConfigs()`: the rows it holds and the
/// length it allocates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigSet {
    /// The length it allocates (`new TokenConfig[](N)`).
    pub declared: Option<usize>,
    pub configs: Vec<TokenConfig>,
}

impl ConfigSet {
    /// Every allocated row was read. Only then does an underlying with no row
    /// mean the config lacks it, rather than that the scan missed it.
    pub fn complete(&self) -> bool {
        self.declared == Some(self.configs.len()) && !self.configs.is_empty()
    }
}

/// A Solidity string literal's body, unescaped for the escapes the config uses
/// or might (`\"`, `\\`, `\'`).
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut cs = s.chars();
    while let Some(c) = cs.next() {
        if c == '\\' {
            if let Some(n) = cs.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Read `LibProdTokenConfig.productionTokenConfigs()`. A `unicode"…"` literal
/// is read as the UTF-8 it holds, as solc encodes it.
pub fn parse_configs(cfg_lib: &str) -> ConfigSet {
    let lib = strip_line_comments(cfg_lib);
    let Some(body) = function_body(&lib, "productionTokenConfigs") else {
        return ConfigSet::default();
    };
    let declared = Regex::new(r"new\s+TokenConfig\[\]\(\s*(\d+)\s*\)")
        .ok()
        .and_then(|re| re.captures(body))
        .and_then(|c| c[1].parse().ok());
    let s = r#"(?:unicode)?"((?:[^"\\]|\\.)*)""#;
    let Ok(re) = Regex::new(&format!(
        r"configs\[\d+\]\s*=\s*TokenConfig\(\s*{s}\s*,\s*{s}\s*,\s*{s}\s*\)\s*;"
    )) else {
        return ConfigSet::default();
    };
    let configs = re
        .captures_iter(body)
        .map(|c| TokenConfig {
            underlying: unescape(&c[1]),
            name: unescape(&c[2]),
            symbol: unescape(&c[3]),
        })
        .collect();
    ConfigSet { declared, configs }
}

// ---------------------------------------------------------------------------
// Per-chain verdicts.
// ---------------------------------------------------------------------------

/// `address` must have code. `actual` is its size, `"no code"`, or null when
/// unread.
fn code_row(fields: Value, code: Option<&str>) -> Value {
    let bare = code.map(|c| c.strip_prefix("0x").unwrap_or(c));
    let (actual, v) = match bare {
        None => (Value::Null, Verdict::Unknown),
        Some("") => (json!("no code"), Verdict::Fail),
        Some(b) => (json!(format!("{} bytes", b.len() / 2)), Verdict::Pass),
    };
    row(fields, "nonzero", Value::Null, actual, v)
}

/// A string read that must equal `expected` byte for byte.
fn string_row(fields: Value, expected: Option<&str>, actual: Option<&str>) -> Value {
    let v = match (expected, actual) {
        (Some(e), Some(a)) => Verdict::of(Some(e == a)),
        _ => Verdict::Unknown,
    };
    row(fields, "equal", json!(expected), json!(actual), v)
}

/// The table itself: it must allocate a non-zero length, and every slot of it
/// must have been read, once. A table the scan read short of its declared
/// length fails rather than passing on the rows it did read.
fn table_row(t: &TokenTable) -> Value {
    let parsed = t.tokens.len();
    let mut seen: Vec<usize> = t.tokens.iter().map(|p| p.index).collect();
    seen.sort_unstable();
    seen.dedup();
    let v = match (&t.function, t.declared) {
        (None, _) | (Some(_), None) => Verdict::Unknown,
        (Some(_), Some(n)) => Verdict::of(Some(
            n > 0 && parsed == n && seen.len() == n && seen.last() == Some(&(n - 1)),
        )),
    };
    row(
        json!({"check": "table", "function": t.function}),
        "equal",
        json!({"entries": t.declared}),
        json!({"entries": parsed, "distinctIndices": seen.len()}),
        v,
    )
}

/// One token's document.
fn token_doc(
    t: &TokenPin,
    configs: &ConfigSet,
    beacons: &[Option<String>],
    pin: &ChainPin,
    r: &dyn ChainReads,
) -> Value {
    let (receipt, vault, wrapped) = (
        t.receipt.address.as_deref(),
        t.receipt_vault.address.as_deref(),
        t.wrapped.address.as_deref(),
    );
    let beacon = |i: usize| beacons.get(i).cloned().flatten();
    let call = |on: Option<&str>, data: &str| on.and_then(|a| r.call_address(a, data));
    let text = |on: Option<&str>, data: &str| on.and_then(|a| r.call_string(a, data));
    let mut checks = Vec::new();
    for (contract, at) in [
        ("receipt", receipt),
        ("receiptVault", vault),
        ("wrappedTokenVault", wrapped),
    ] {
        checks.push(code_row(
            json!({"check": "code", "contract": contract}),
            at.and_then(|a| r.code(a)).as_deref(),
        ));
    }
    for (i, (contract, at)) in [
        ("receipt", receipt),
        ("receiptVault", vault),
        ("wrappedTokenVault", wrapped),
    ]
    .into_iter()
    .enumerate()
    {
        checks.push(slot_address_row(
            json!({"check": "beacon", "contract": contract}),
            beacon(i).as_deref(),
            at.and_then(|a| r.storage(a, ERC1967_BEACON_SLOT)),
        ));
    }
    for (check, on, data, expected) in [
        ("receipt", vault, rpc::receipt_calldata(), receipt),
        ("manager", receipt, rpc::manager_calldata(), vault),
        ("asset", wrapped, rpc::asset_calldata(), vault),
        (
            "authoriser",
            vault,
            rpc::authorizer_calldata(),
            pin.authoriser.as_deref(),
        ),
        ("owner", vault, rpc::owner_calldata(), pin.safe.as_deref()),
    ] {
        checks.push(address_row(
            json!({"check": check}),
            expected,
            call(on, &data).as_deref(),
        ));
    }
    // A found config passes. A missing one fails only when every allocated
    // config row was read; otherwise the scan may have missed it, so unknown.
    let config = configs
        .configs
        .iter()
        .find(|c| c.underlying == t.underlying);
    let v = match config {
        Some(_) => Verdict::Pass,
        None if configs.complete() => Verdict::Fail,
        None => Verdict::Unknown,
    };
    checks.push(row(
        json!({"check": "config"}),
        "equal",
        json!(t.underlying),
        json!(config.map(|c| &c.underlying)),
        v,
    ));
    let live_name = text(vault, &rpc::name_calldata());
    let live_symbol = text(vault, &rpc::symbol_calldata());
    checks.push(string_row(
        json!({"check": "name"}),
        config.map(|c| c.name.as_str()),
        live_name.as_deref(),
    ));
    checks.push(string_row(
        json!({"check": "symbol"}),
        config.map(|c| c.symbol.as_str()),
        live_symbol.as_deref(),
    ));
    let mut doc = subject(vault, checks);
    if let Some(o) = doc.as_object_mut() {
        for (k, v) in [
            ("index", json!(t.index)),
            ("underlying", json!(t.underlying)),
            ("receipt", json!(receipt)),
            ("receiptVault", json!(vault)),
            ("wrappedTokenVault", json!(wrapped)),
            ("name", json!(config.map(|c| &c.name))),
            ("symbol", json!(config.map(|c| &c.symbol))),
        ] {
            o.insert(k.into(), v);
        }
    }
    doc
}

/// One chain's token block. `beacons` are the chain's beacons in
/// `LibBeaconInvariants` index order (receipt, receipt vault, wrapped, …), as
/// `deploybeacons::chain_doc` found them.
pub fn chain_doc(
    table: &TokenTable,
    configs: &ConfigSet,
    pin: &ChainPin,
    beacons: &[Option<String>],
    r: &dyn ChainReads,
) -> Value {
    let table_check = table_row(table);
    let tokens: Vec<Value> = table
        .tokens
        .iter()
        .map(|t| token_doc(t, configs, beacons, pin, r))
        .collect();
    let sum = |k: &str| {
        tokens
            .iter()
            .map(|d| d[k].as_u64().unwrap_or(0) as usize)
            .sum::<usize>()
    };
    let one = |s: &str| usize::from(table_check["status"] == s);
    let (passed, failed, unknown) = (
        sum("passed") + one("pass"),
        sum("failed") + one("fail"),
        sum("unknown") + one("unknown"),
    );
    json!({
        "network": pin.network,
        "rpcHost": pin.rpc_host,
        "function": table.function,
        "safe": pin.safe,
        "authoriser": pin.authoriser,
        "beacons": beacons,
        "declared": table.declared,
        "tokenCount": tokens.len(),
        "tokensPassed": tokens.iter().filter(|t| t["state"] == "pass").count(),
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "total": passed + failed + unknown,
        "state": rollup(passed, failed, unknown),
        "table": table_check,
        "tokens": tokens,
    })
}

/// Assemble the `deploymentPinnedTokens` document. `None` when there are no
/// chains, so the page shows nothing rather than an empty table that reads as
/// "nothing to check".
pub fn build_tokens(
    org: &str,
    repo: &str,
    configs: &ConfigSet,
    chains: Vec<Value>,
) -> Option<Value> {
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
        "source": format!("{TOKEN_INVARIANTS} productionTokens<Chain>()"),
        "configSource": format!("{TOKEN_CONFIG} productionTokenConfigs()"),
        "configCount": configs.configs.len(),
        "configDeclared": configs.declared,
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
    use crate::deploystate::NoReads;
    use alloy_primitives::hex;
    use std::collections::HashMap;

    // Production values as st0x.deploy pins them (3c6db51), Ethereum.
    const SAFE: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const CLONE: &str = "0x66566cc91dEAf818859bD4b09B7903ac48998157";
    const EOA: &str = "0x8E4bdeec7CEB9570D440676345dA1dCe10329f5b";
    const RECEIPT_BEACON: &str = "0xace121ae30d754536863a546f41b147be11202db";
    const VAULT_BEACON: &str = "0x24e98b8f8f951b1868df88762cef589aedbc01eb";
    const WRAPPED_BEACON: &str = "0x9FD790f65CA3aF2772358c653F097f0a4c7EE7d2";
    const ORCH_BEACON: &str = "0xb9DCd744b0413Dff0EDC70A5B229c7aa03734613";
    /// (receipt, receipt vault, wrapped-token vault).
    const MSTR: [&str; 3] = [
        "0xE3772C8695c2cf3dcAA2Dd29759f4Bb91a342763",
        "0x8500189061e2206Bc33Bf04DC10fFB1Fe7dED637",
        "0xd9fE7488B86D3aEaf457b181C744BD1A5a120833",
    ];
    const TSLA: [&str; 3] = [
        "0x3a3E00d6fb65E686941f77DD375ca677Ad7772c2",
        "0xB41fD00d0bA60D9Ae8dCE405cB6AAd5710E5F84d",
        "0x550499e28A3CE8cb1dB3e3Db23fDD0357eD3ff48",
    ];

    /// A token lib in the three shapes the chains' tables are written in:
    /// named constants (Base), `address(0x…)` (Ethereum) and bare literals
    /// (HyperEVM, Robinhood, BSC). BSC has no table here.
    fn tok_lib() -> String {
        let [r, v, w] = MSTR;
        let [tr, tv, tw] = TSLA;
        format!(
            r#"
library LibTokenInvariants {{
    address internal constant MSTR_RECEIPT = address({r});
    address internal constant MSTR_RECEIPT_VAULT = address({v});
    address internal constant MSTR_WRAPPED_TOKEN_VAULT = address({w});
    function productionTokensBase() internal pure returns (TokenInstance[] memory tokens) {{
        tokens = new TokenInstance[](1);
        tokens[0] = TokenInstance("MSTR", MSTR_RECEIPT, MSTR_RECEIPT_VAULT, MSTR_WRAPPED_TOKEN_VAULT);
    }}
    function productionTokensEthereum() internal pure returns (TokenInstance[] memory tokens) {{
        // Deployed 2026-07-22. tokens[5] = TokenInstance("OLD", {r}, {v}, {w});
        tokens = new TokenInstance[](2);
        tokens[0] = TokenInstance(
            "MSTR",
            address({r}),
            address({v}),
            address({w})
        );
        tokens[1] = TokenInstance(
            "TSLA",
            address({tr}),
            address({tv}),
            address({tw})
        );
    }}
    function productionTokensHyperEvm() internal pure returns (TokenInstance[] memory tokens) {{
        tokens = new TokenInstance[](1);
        tokens[0] = TokenInstance(
            "MSTR",
            {r},
            {v},
            {w}
        );
    }}
}}
"#
        )
    }

    /// The config in its written forms: plain, a leading space kept verbatim
    /// (SGOV), a `unicode"…"` literal, and escaped quotes.
    fn cfg_lib(declared: usize) -> String {
        format!(
            r#"
library LibProdTokenConfig {{
    function productionTokenConfigs() internal pure returns (TokenConfig[] memory configs) {{
        configs = new TokenConfig[]({declared});
        configs[0] = TokenConfig("MSTR", "MicroStrategy Incorporated ST0x", "tMSTR");
        configs[1] = TokenConfig("TSLA", "Tesla Inc ST0x", "tTSLA");
        configs[2] = TokenConfig("SGOV", " iShares 0-3 Month Treasury Bond ETF ST0x", "tSGOV");
        // solc rejects a bare non-ASCII string literal.
        configs[3] = TokenConfig("MC.PA", unicode"LVMH Moët Hennessy Louis Vuitton SE ST0x", "tMC.PA");
        configs[4] = TokenConfig("Q", "A \"quoted\" name", "tQ");
    }}
}}
"#
        )
    }

    fn configs() -> ConfigSet {
        parse_configs(&cfg_lib(5))
    }

    fn word_of(address: &str) -> [u8; 32] {
        let mut w = [0u8; 32];
        hex::decode_to_slice(
            format!("{:0>64}", address.trim_start_matches("0x").to_lowercase()),
            &mut w,
        )
        .unwrap();
        w
    }

    /// A chain held in memory. An address with no code set reads as empty and
    /// an unset beacon slot as zero, as on a real chain; calls answer only
    /// where set.
    #[derive(Default, Clone)]
    struct Fake {
        code: HashMap<String, String>,
        beacon: HashMap<String, [u8; 32]>,
        calls: HashMap<(String, String), String>,
        strings: HashMap<(String, String), String>,
    }

    impl Fake {
        fn answer(&mut self, on: &str, data: String, address: &str) {
            self.calls
                .insert((on.to_lowercase(), data), address.to_lowercase());
        }
        fn say(&mut self, on: &str, data: String, text: &str) {
            self.strings
                .insert((on.to_lowercase(), data), text.to_string());
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
            assert_eq!(slot, ERC1967_BEACON_SLOT);
            Some(
                self.beacon
                    .get(&address.to_lowercase())
                    .copied()
                    .unwrap_or([0u8; 32]),
            )
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
        fn call_address(&self, contract: &str, calldata: &str) -> Option<String> {
            self.calls
                .get(&(contract.to_lowercase(), calldata.to_string()))
                .cloned()
        }
        fn call_string(&self, contract: &str, calldata: &str) -> Option<String> {
            self.strings
                .get(&(contract.to_lowercase(), calldata.to_string()))
                .cloned()
        }
    }

    /// `f` with `token` wired as the #182 end state leaves it.
    fn wire(f: &mut Fake, token: [&str; 3], name: &str, symbol: &str) {
        let [receipt, vault, wrapped] = token;
        for (at, beacon) in [
            (receipt, RECEIPT_BEACON),
            (vault, VAULT_BEACON),
            (wrapped, WRAPPED_BEACON),
        ] {
            f.code.insert(at.to_lowercase(), "0x3d3d".to_string());
            f.beacon.insert(at.to_lowercase(), word_of(beacon));
        }
        f.answer(vault, rpc::receipt_calldata(), receipt);
        f.answer(receipt, rpc::manager_calldata(), vault);
        f.answer(wrapped, rpc::asset_calldata(), vault);
        f.answer(vault, rpc::authorizer_calldata(), CLONE);
        f.answer(vault, rpc::owner_calldata(), SAFE);
        f.say(vault, rpc::name_calldata(), name);
        f.say(vault, rpc::symbol_calldata(), symbol);
    }

    fn healthy() -> Fake {
        let mut f = Fake::default();
        wire(&mut f, MSTR, "MicroStrategy Incorporated ST0x", "tMSTR");
        wire(&mut f, TSLA, "Tesla Inc ST0x", "tTSLA");
        f
    }

    fn pin(network: &str) -> ChainPin {
        ChainPin {
            network: network.to_string(),
            authoriser: Some(CLONE.to_string()),
            safe: Some(SAFE.to_string()),
            rpc_host: None,
        }
    }

    fn beacons() -> Vec<Option<String>> {
        [RECEIPT_BEACON, VAULT_BEACON, WRAPPED_BEACON, ORCH_BEACON]
            .iter()
            .map(|b| Some(b.to_string()))
            .collect()
    }

    fn ethereum_doc(f: &Fake) -> Value {
        chain_doc(
            &parse_table(&tok_lib(), "ethereum"),
            &configs(),
            &pin("ethereum"),
            &beacons(),
            f,
        )
    }

    /// The rows of `doc`'s tokens that did not pass, as `check` or
    /// `check contract`.
    fn not_passed(doc: &Value) -> Vec<String> {
        doc["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["checks"].as_array().unwrap().iter())
            .filter(|c| c["status"] != "pass")
            .map(|c| {
                let check = c["check"].as_str().unwrap();
                match c["contract"].as_str() {
                    Some(k) => format!("{check} {k}"),
                    None => check.to_string(),
                }
            })
            .collect()
    }

    #[test]
    fn each_chain_names_its_own_table_function() {
        let lib = tok_lib();
        assert_eq!(
            table_function(&lib, "base").as_deref(),
            Some("productionTokensBase")
        );
        assert_eq!(
            table_function(&lib, "hyperevm").as_deref(),
            Some("productionTokensHyperEvm")
        );
        assert_eq!(table_function(&lib, "bsc"), None);
    }

    #[test]
    fn reads_the_three_row_shapes_the_tables_are_written_in() {
        let lib = tok_lib();
        for (network, n) in [("base", 1), ("ethereum", 2), ("hyperevm", 1)] {
            let t = parse_table(&lib, network);
            assert_eq!(t.declared, Some(n), "{network}");
            assert_eq!(t.tokens.len(), n, "{network}");
            let m = &t.tokens[0];
            assert_eq!(m.index, 0);
            assert_eq!(m.underlying, "MSTR");
            for (arg, want) in [
                (&m.receipt, MSTR[0]),
                (&m.receipt_vault, MSTR[1]),
                (&m.wrapped, MSTR[2]),
            ] {
                assert!(
                    arg.address
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(want)),
                    "{network}: {arg:?}"
                );
            }
        }
        let eth = parse_table(&lib, "ethereum");
        assert_eq!(eth.tokens[1].underlying, "TSLA");
        assert!(eth.tokens[1]
            .wrapped
            .address
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(TSLA[2])));
    }

    /// The Ethereum fixture's comment names a `tokens[5]` row; it is not one.
    #[test]
    fn a_commented_out_row_is_not_read() {
        let t = parse_table(&tok_lib(), "ethereum");
        assert!(t.tokens.iter().all(|p| p.underlying != "OLD"));
        assert_eq!(table_row(&t)["status"], "pass");
    }

    #[test]
    fn a_table_read_short_of_its_length_fails() {
        let full = parse_table(&tok_lib(), "ethereum");
        let mut short = full.clone();
        short.tokens.truncate(1);
        assert_eq!(table_row(&short)["status"], "fail");
        let mut repeated = full.clone();
        repeated.tokens[1].index = 0;
        assert_eq!(table_row(&repeated)["status"], "fail");
        let mut empty = full.clone();
        empty.declared = Some(0);
        empty.tokens.clear();
        assert_eq!(table_row(&empty)["status"], "fail");
        let mut undeclared = full;
        undeclared.declared = None;
        assert_eq!(table_row(&undeclared)["status"], "unknown");
        assert_eq!(table_row(&TokenTable::default())["status"], "unknown");
    }

    #[test]
    fn configs_are_read_verbatim() {
        let c = configs();
        assert_eq!(c.declared, Some(5));
        assert!(c.complete());
        let by = |u: &str| c.configs.iter().find(|x| x.underlying == u).unwrap();
        assert_eq!(by("SGOV").name, " iShares 0-3 Month Treasury Bond ETF ST0x");
        assert_eq!(by("MC.PA").name, "LVMH Moët Hennessy Louis Vuitton SE ST0x");
        assert_eq!(by("MC.PA").symbol, "tMC.PA");
        assert_eq!(by("Q").name, "A \"quoted\" name");
        assert!(!parse_configs(&cfg_lib(6)).complete());
        assert_eq!(parse_configs(""), ConfigSet::default());
    }

    #[test]
    fn a_healthy_chain_passes_every_check() {
        let doc = ethereum_doc(&healthy());
        assert_eq!(not_passed(&doc), Vec::<String>::new());
        assert_eq!(doc["tokens"][0]["total"], 14);
        assert_eq!(doc["tokenCount"], 2);
        assert_eq!(doc["tokensPassed"], 2);
        // Two tokens of 14 rows, and the table row.
        assert_eq!(doc["passed"], 29);
        assert_eq!(doc["total"], 29);
        assert_eq!(doc["state"], "pass");
    }

    #[test]
    fn each_wiring_breach_fails_its_own_check() {
        let [receipt, vault, wrapped] = MSTR;
        let other = "0x000000000000000000000000000000000000dead";
        type Breach = Box<dyn Fn(&mut Fake)>;
        let cases: Vec<(&str, Breach)> = vec![
            (
                "code wrappedTokenVault",
                Box::new(move |f| {
                    f.code.remove(&wrapped.to_lowercase());
                }),
            ),
            (
                "beacon receipt",
                Box::new(move |f| {
                    f.beacon
                        .insert(receipt.to_lowercase(), word_of(ORCH_BEACON));
                }),
            ),
            (
                "receipt",
                Box::new(move |f| f.answer(vault, rpc::receipt_calldata(), other)),
            ),
            (
                "manager",
                Box::new(move |f| f.answer(receipt, rpc::manager_calldata(), other)),
            ),
            (
                "asset",
                Box::new(move |f| f.answer(wrapped, rpc::asset_calldata(), other)),
            ),
            (
                "authoriser",
                Box::new(move |f| f.answer(vault, rpc::authorizer_calldata(), other)),
            ),
            (
                "owner",
                Box::new(move |f| f.answer(vault, rpc::owner_calldata(), EOA)),
            ),
            (
                "name",
                Box::new(move |f| {
                    f.say(
                        vault,
                        rpc::name_calldata(),
                        "MicroStrategy Incorporated ST0x ",
                    )
                }),
            ),
            (
                "symbol",
                Box::new(move |f| f.say(vault, rpc::symbol_calldata(), "TMSTR")),
            ),
        ];
        for (want, breach) in cases {
            let mut f = healthy();
            breach(&mut f);
            let doc = ethereum_doc(&f);
            assert_eq!(not_passed(&doc), vec![want.to_string()], "{want}");
            assert_eq!(doc["failed"], 1, "{want}");
            assert_eq!(doc["state"], "fail", "{want}");
            assert_eq!(doc["tokens"][0]["state"], "fail", "{want}");
            assert_eq!(doc["tokens"][1]["state"], "pass", "{want}");
        }
    }

    /// An underlying the config lacks fails only when every config row was
    /// read: a config the scan read short may hold it.
    #[test]
    fn a_missing_config_fails_only_when_every_config_was_read() {
        let mut table = parse_table(&tok_lib(), "hyperevm");
        table.tokens[0].underlying = "ZZZ".to_string();
        let doc = |c: &ConfigSet| chain_doc(&table, c, &pin("hyperevm"), &beacons(), &healthy());
        let complete = doc(&configs());
        let status = |d: &Value, check: &str| {
            d["tokens"][0]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["check"] == check)
                .unwrap()["status"]
                .clone()
        };
        assert_eq!(status(&complete, "config"), "fail");
        assert_eq!(status(&complete, "name"), "unknown");
        assert_eq!(status(&complete, "symbol"), "unknown");
        let short = doc(&parse_configs(&cfg_lib(6)));
        assert_eq!(status(&short, "config"), "unknown");
        assert_eq!(short["failed"], 0);
    }

    /// A chain with no endpoint keeps every token row, each unknown. Only what
    /// asks the source alone still answers: the table row, and whether each
    /// underlying has a config.
    #[test]
    fn an_unreachable_chain_keeps_every_check_as_unknown() {
        let doc = ethereum_doc_with(&NoReads);
        let passed: Vec<String> = doc["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["checks"].as_array().unwrap().iter())
            .filter(|c| c["status"] == "pass")
            .map(|c| c["check"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(passed, ["config", "config"]);
        assert_eq!(doc["unknown"], 26);
        assert_eq!(doc["passed"], 3);
        assert_eq!(doc["failed"], 0);
        assert_eq!(doc["table"]["status"], "pass");
        assert_eq!(doc["state"], "unknown");
    }

    fn ethereum_doc_with(r: &dyn ChainReads) -> Value {
        chain_doc(
            &parse_table(&tok_lib(), "ethereum"),
            &configs(),
            &pin("ethereum"),
            &beacons(),
            r,
        )
    }

    /// Beacons the beacon view could not find leave the beacon rows unknown,
    /// never a pass against nothing.
    #[test]
    fn unfound_beacons_leave_the_beacon_rows_unknown() {
        let doc = chain_doc(
            &parse_table(&tok_lib(), "ethereum"),
            &configs(),
            &pin("ethereum"),
            &[],
            &healthy(),
        );
        let unknown: Vec<String> = not_passed(&doc);
        assert_eq!(unknown.len(), 6);
        assert!(unknown.iter().all(|c| c.starts_with("beacon ")));
        assert_eq!(doc["failed"], 0);
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn a_chain_without_a_table_is_unknown() {
        let doc = chain_doc(
            &parse_table(&tok_lib(), "bsc"),
            &configs(),
            &pin("bsc"),
            &beacons(),
            &healthy(),
        );
        assert_eq!(doc["tokenCount"], 0);
        assert_eq!(doc["table"]["status"], "unknown");
        assert_eq!(doc["state"], "unknown");
    }

    #[test]
    fn the_document_sums_its_chains() {
        let c = configs();
        assert_eq!(build_tokens("o", "r", &c, Vec::new()), None);
        let mut bad = healthy();
        bad.answer(MSTR[1], rpc::owner_calldata(), EOA);
        let docs = vec![ethereum_doc(&healthy()), ethereum_doc(&bad)];
        let doc = build_tokens("S01-Issuer", "st0x.deploy", &c, docs).unwrap();
        assert_eq!(doc["configCount"], 5);
        assert_eq!(doc["configDeclared"], 5);
        assert_eq!(doc["passed"], 57);
        assert_eq!(doc["failed"], 1);
        assert_eq!(doc["total"], 58);
        assert_eq!(doc["state"], "fail");
    }
}
