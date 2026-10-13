//! Names for addresses, per chain, from every `address` constant the deploy
//! repo pins, so the Deployments page can say WHO an address is instead of
//! printing it bare.
//!
//! The set is a category, not a list: every `address … constant` declared in
//! the sources handed in is a name, so a pin added to the deploy repo is named
//! here without a change to this file. Declarations are read with
//! [`solang_parser`] — the lexer `forge fmt` uses — so a commented-out pin is
//! not a pin and a declaration wrapped over two lines is still one.
//!
//! A constant is either a literal (`= 0x…` / `= address(0x…)`) or an alias of
//! another constant (`= OtherLib.NAME` / `= NAME`). Aliases are followed to
//! their literal; one whose target is not among the sources read (an imported
//! generated pointer, say) has no address here and names nothing.
//!
//! One address means different things on different chains, so names are
//! scoped per chain by the deploy repo's own suffix convention — the one
//! `owners::parse_chain_pins` already reads:
//! - `NAME_<CHAIN>` is that chain's pin.
//! - A bare `NAME` beside a `NAME_<CHAIN>` sibling in the same library is the
//!   reference chain's ([`owners::REFERENCE_NETWORK`]) pin of the same thing.
//! - A bare `NAME` with no such sibling is the same address on every chain —
//!   the shape of an EOA or a deterministic deployment.
//! - An alias with no suffix of its own means what its target means.

use crate::owners;
use serde_json::{json, Map, Value};
use solang_parser::pt::{
    ContractPart, Expression, SourceUnitPart, Type, VariableAttribute, VariableDefinition,
};

/// What a constant is set to.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PinValue {
    /// A 20-byte literal, as written.
    Literal(String),
    /// Another constant: `(Some(library), name)` for `Lib.NAME`, `(None, name)`
    /// for a bare `NAME`, which is looked up in the declaring library.
    Alias(Option<String>, String),
}

/// One `address … constant` declaration.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Pin {
    /// The declaring library / contract, or `""` for a file-level constant.
    pub contract: String,
    pub name: String,
    pub value: PinValue,
}

fn literal(e: &Expression) -> Option<String> {
    match e {
        Expression::HexNumberLiteral(_, s, None) | Expression::AddressLiteral(_, s) => {
            let hex = s.strip_prefix("0x")?;
            (hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| format!("0x{hex}"))
        }
        _ => None,
    }
}

fn pin_value(e: &Expression) -> Option<PinValue> {
    if let Some(l) = literal(e) {
        return Some(PinValue::Literal(l));
    }
    match e {
        // `address(0x…)`, the form most pins are written in.
        Expression::FunctionCall(_, f, args)
            if matches!(
                **f,
                Expression::Type(_, Type::Address | Type::AddressPayable)
            ) && args.len() == 1 =>
        {
            literal(&args[0]).map(PinValue::Literal)
        }
        Expression::MemberAccess(_, base, member) => match &**base {
            Expression::Variable(lib) => {
                Some(PinValue::Alias(Some(lib.name.clone()), member.name.clone()))
            }
            _ => None,
        },
        Expression::Variable(v) => Some(PinValue::Alias(None, v.name.clone())),
        _ => None,
    }
}

fn pin_of(contract: &str, v: &VariableDefinition) -> Option<Pin> {
    if !matches!(
        v.ty,
        Expression::Type(_, Type::Address | Type::AddressPayable)
    ) {
        return None;
    }
    if !v
        .attrs
        .iter()
        .any(|a| matches!(a, VariableAttribute::Constant(_)))
    {
        return None;
    }
    Some(Pin {
        contract: contract.to_string(),
        name: v.name.as_ref()?.name.clone(),
        value: pin_value(v.initializer.as_ref()?)?,
    })
}

/// Every `address … constant` declared in one Solidity source, in source
/// order. `None` when the source does not parse — unread, not "pins nothing".
pub fn parse_pins(src: &str) -> Option<Vec<Pin>> {
    let (unit, _) = solang_parser::parse(src, 0).ok()?;
    let mut out = Vec::new();
    for part in &unit.0 {
        match part {
            SourceUnitPart::VariableDefinition(v) => out.extend(pin_of("", v)),
            SourceUnitPart::ContractDefinition(c) => {
                let Some(cname) = c.name.as_ref().map(|i| i.name.as_str()) else {
                    continue;
                };
                for p in &c.parts {
                    if let ContractPart::VariableDefinition(v) = p {
                        out.extend(pin_of(cname, v));
                    }
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// The chain a constant's own name pins it to, by its `_<CHAIN>` suffix.
fn own_chain<'a>(name: &str, chains: &'a [String]) -> Option<&'a String> {
    chains.iter().find(|c| {
        name.strip_suffix(&c.to_uppercase())
            .and_then(|stem| stem.strip_suffix('_'))
            .is_some_and(|stem| !stem.is_empty())
    })
}

/// Follow `pins[i]` to its literal: `(address, chain)`, the chain `None` when
/// the name holds on every chain. `None` for a dangling alias, an alias cycle,
/// or a zero (unhydrated) pin, which is a placeholder, not an address.
fn resolve(
    pins: &[Pin],
    i: usize,
    chains: &[String],
    depth: usize,
) -> Option<(String, Option<String>)> {
    if depth > pins.len() {
        return None; // a cycle
    }
    let p = &pins[i];
    let own = own_chain(&p.name, chains).cloned();
    match &p.value {
        PinValue::Literal(a) => {
            if a[2..].chars().all(|c| c == '0') {
                return None;
            }
            let chain = own.or_else(|| {
                let has_sibling = chains.iter().any(|c| {
                    let sibling = format!("{}_{}", p.name, c.to_uppercase());
                    pins.iter()
                        .any(|q| q.contract == p.contract && q.name == sibling)
                });
                has_sibling.then(|| owners::REFERENCE_NETWORK.to_string())
            });
            Some((a.to_lowercase(), chain))
        }
        PinValue::Alias(lib, name) => {
            let in_lib = lib.as_deref().unwrap_or(&p.contract);
            let j = pins
                .iter()
                .position(|q| q.contract == in_lib && q.name == *name)
                .or_else(|| {
                    // A bare name the library does not declare: a file-level
                    // constant, if exactly one is declared under that name.
                    if lib.is_some() {
                        return None;
                    }
                    let mut it = pins
                        .iter()
                        .enumerate()
                        .filter(|(_, q)| q.contract.is_empty() && q.name == *name);
                    match (it.next(), it.next()) {
                        (Some((j, _)), None) => Some(j),
                        _ => None,
                    }
                })?;
            let (address, chain) = resolve(pins, j, chains, depth + 1)?;
            Some((address, own.or(chain)))
        }
    }
}

/// The `addressNames` map: chain → lowercase address → every constant naming
/// it there, best name first.
///
/// `sources` is `(path, body)` per file read, in the order to prefer names in;
/// `chains` is the chain vocabulary (`owners::parse_chain_pins`' networks).
/// Within one address the order is: a literal declaration before an alias of
/// it (the literal is where the deploy repo states the address); a pin scoped
/// to the chain before one that holds on every chain; then source order.
///
/// @return `(map, unread)` — `unread` lists the paths that were empty or did
/// not parse, whose pins are therefore missing from the map.
pub fn build_address_names(
    sources: &[(String, String)],
    chains: &[String],
) -> (Value, Vec<String>) {
    let mut pins: Vec<Pin> = Vec::new();
    let mut origin: Vec<usize> = Vec::new();
    let mut unread = Vec::new();
    for (k, (path, body)) in sources.iter().enumerate() {
        match parse_pins(body).filter(|_| !body.trim().is_empty()) {
            Some(ps) => {
                origin.extend(std::iter::repeat_n(k, ps.len()));
                pins.extend(ps);
            }
            None => unread.push(path.clone()),
        }
    }
    let mut ranked: Vec<(&String, String, (bool, bool, usize))> = Vec::new();
    for (i, p) in pins.iter().enumerate() {
        let Some((address, chain)) = resolve(&pins, i, chains, 0) else {
            continue;
        };
        let rank = (matches!(p.value, PinValue::Alias(..)), chain.is_none(), i);
        for c in chains {
            if chain.as_ref().is_none_or(|rc| rc == c) {
                ranked.push((c, address.clone(), rank));
            }
        }
    }
    ranked.sort_by_key(|x| x.2);
    let mut by_chain: Map<String, Value> = chains
        .iter()
        .map(|c| (c.clone(), Value::Object(Map::new())))
        .collect();
    for (chain, address, (_, _, i)) in ranked {
        let p = &pins[i];
        let entry = json!({
            "name": p.name,
            "contract": p.contract,
            "path": sources[origin[i]].0,
        });
        if let Some(table) = by_chain.get_mut(chain).and_then(Value::as_object_mut) {
            if let Value::Array(list) = table.entry(address).or_insert_with(|| json!([])) {
                list.push(entry);
            }
        }
    }
    (Value::Object(by_chain), unread)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chains() -> Vec<String> {
        ["base", "ethereum", "hyperevm"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn names(map: &Value, chain: &str, address: &str) -> Vec<String> {
        map[chain][address]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|e| {
                        format!(
                            "{}.{}",
                            e["contract"].as_str().unwrap(),
                            e["name"].as_str().unwrap()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn src(path: &str, body: &str) -> (String, String) {
        (path.to_string(), body.to_string())
    }

    const A: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "0xcccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn parses_literal_and_alias_forms_and_ignores_the_rest() {
        let pins = parse_pins(
            r#"
            pragma solidity ^0.8.25;
            import {X as Y_GEN} from "./x.sol";
            address constant FILE_LEVEL = 0xAaAaaAaaaAaaaAaaAaaAAaaaaaAaAAaaaaAaAaAa;
            library LibA {
                /// address internal constant DOC = 0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb;
                // address internal constant COMMENTED = address(0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb);
                address internal constant WRAPPED = address(0xBbBbBBBbbBbBbBbbBbbBBbBbBbbBbBBbbbbBbBbb);
                address constant WRAPPED_OVER_LINES =
                    address(0xcccccccccccccccccccccccccccccccccccccccc);
                address public constant MEMBER = LibB.OTHER;
                address internal constant BARE = WRAPPED;
                address payable internal constant PAYABLE = payable(address(0xcccccccccccccccccccccccccccccccccccccccc));
                bytes32 internal constant HASH = 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;
                uint256 internal constant N = 5;
                address internal immutable_like = address(0xcccccccccccccccccccccccccccccccccccccccc);
                address internal constant SHORT = address(0x1);
            }
            "#,
        )
        .unwrap();
        let got: Vec<(String, String, PinValue)> = pins
            .into_iter()
            .map(|p| (p.contract, p.name, p.value))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "".into(),
                    "FILE_LEVEL".into(),
                    PinValue::Literal("0xAaAaaAaaaAaaaAaaAaaAAaaaaaAaAAaaaaAaAaAa".into())
                ),
                (
                    "LibA".into(),
                    "WRAPPED".into(),
                    PinValue::Literal("0xBbBbBBBbbBbBbBbbBbbBBbBbBbbBbBBbbbbBbBbb".into())
                ),
                (
                    "LibA".into(),
                    "WRAPPED_OVER_LINES".into(),
                    PinValue::Literal(C.into())
                ),
                (
                    "LibA".into(),
                    "MEMBER".into(),
                    PinValue::Alias(Some("LibB".into()), "OTHER".into())
                ),
                (
                    "LibA".into(),
                    "BARE".into(),
                    PinValue::Alias(None, "WRAPPED".into())
                ),
            ]
        );
    }

    #[test]
    fn unparseable_source_is_unread_not_empty() {
        assert_eq!(parse_pins("library L { address constant X = ; "), None);
        let (map, unread) = build_address_names(
            &[src("bad.sol", "library {"), src("empty.sol", "  ")],
            &chains(),
        );
        assert_eq!(unread, vec!["bad.sol".to_string(), "empty.sol".to_string()]);
        assert_eq!(map["base"], json!({}));
    }

    #[test]
    fn suffixed_pins_name_only_their_chain_and_bare_siblings_the_reference_chain() {
        let (map, unread) = build_address_names(
            &[src(
                "src/lib/LibT.sol",
                &format!(
                    "library LibT {{
                        address internal constant TL = address({A});
                        address internal constant TL_ETHEREUM = address({B});
                        address internal constant TL_HYPEREVM = address({B});
                    }}"
                ),
            )],
            &chains(),
        );
        assert!(unread.is_empty());
        assert_eq!(names(&map, "base", A), vec!["LibT.TL"]);
        assert!(names(&map, "ethereum", A).is_empty(), "base pin leaked");
        assert!(names(&map, "hyperevm", A).is_empty(), "base pin leaked");
        assert_eq!(names(&map, "ethereum", B), vec!["LibT.TL_ETHEREUM"]);
        assert_eq!(names(&map, "hyperevm", B), vec!["LibT.TL_HYPEREVM"]);
        assert!(
            names(&map, "base", B).is_empty(),
            "chain pin leaked to base"
        );
    }

    #[test]
    fn a_bare_pin_with_no_chain_sibling_holds_on_every_chain() {
        let (map, _) = build_address_names(
            &[src(
                "L.sol",
                &format!(
                    "library L {{
                        address internal constant SIGNER = address({A});
                        address internal constant SIGNER_2 = address({B});
                    }}"
                ),
            )],
            &chains(),
        );
        for c in chains() {
            assert_eq!(names(&map, &c, A), vec!["L.SIGNER"], "{c}");
            // `_2` is not a chain, so SIGNER_2 is not SIGNER's sibling.
            assert_eq!(names(&map, &c, B), vec!["L.SIGNER_2"], "{c}");
        }
    }

    #[test]
    fn a_sibling_in_another_library_does_not_scope_a_bare_pin() {
        let (map, _) = build_address_names(
            &[
                src(
                    "L.sol",
                    &format!("library L {{ address constant X = address({A}); }}"),
                ),
                src(
                    "M.sol",
                    &format!("library M {{ address constant X_ETHEREUM = address({B}); }}"),
                ),
            ],
            &chains(),
        );
        assert_eq!(names(&map, "ethereum", A), vec!["L.X"]);
    }

    #[test]
    fn aliases_resolve_across_libraries_and_inherit_scope_unless_suffixed() {
        let (map, _) = build_address_names(
            &[
                src(
                    "Safe.sol",
                    &format!(
                        "library LibSafe {{
                            address internal constant SAFE = address({A});
                            address internal constant SAFE_ETHEREUM = address({B});
                        }}"
                    ),
                ),
                src(
                    "Auth.sol",
                    "library LibAuth {
                        address internal constant GRANTEE_SAFE = LibSafe.SAFE;
                        address internal constant SAFE_HYPEREVM = LibSafe.SAFE_ETHEREUM;
                        address internal constant LOCAL = GRANTEE_SAFE;
                        address internal constant DANGLING = LibGen.MISSING;
                        address internal constant LOOP_A = LOOP_B;
                        address internal constant LOOP_B = LOOP_A;
                    }",
                ),
            ],
            &chains(),
        );
        // The literal first, then its aliases in source order.
        assert_eq!(
            names(&map, "base", A),
            vec!["LibSafe.SAFE", "LibAuth.GRANTEE_SAFE", "LibAuth.LOCAL"]
        );
        // GRANTEE_SAFE has no suffix: it means what SAFE means, Base only.
        assert!(names(&map, "ethereum", A).is_empty());
        assert!(names(&map, "hyperevm", A).is_empty());
        // SAFE_HYPEREVM's own suffix wins over its target's.
        assert_eq!(names(&map, "hyperevm", B), vec!["LibAuth.SAFE_HYPEREVM"]);
        assert_eq!(names(&map, "ethereum", B), vec!["LibSafe.SAFE_ETHEREUM"]);
        let all = map.to_string();
        for n in ["DANGLING", "LOOP_A", "LOOP_B"] {
            assert!(!all.contains(n), "{n} named something");
        }
    }

    #[test]
    fn a_bare_alias_falls_back_to_a_unique_file_level_constant() {
        let (map, _) = build_address_names(
            &[
                src("P.sol", &format!("address constant PTR = address({C});")),
                src("L.sol", "library L { address constant VIA = PTR; }"),
                src(
                    "Q.sol",
                    &format!("address constant TWICE = address({A});\naddress constant TWICE = address({B});"),
                ),
                src("M.sol", "library M { address constant AMBIG = TWICE; }"),
            ],
            &chains(),
        );
        assert_eq!(names(&map, "ethereum", C), vec![".PTR", "L.VIA"]);
        assert!(
            !map.to_string().contains("AMBIG"),
            "an ambiguous name resolved"
        );
    }

    #[test]
    fn scoped_names_rank_before_every_chain_names_and_zero_pins_name_nothing() {
        let (map, _) = build_address_names(
            &[src(
                "L.sol",
                &format!(
                    "library L {{
                        address internal constant SHARED = address({A});
                        address internal constant TL = address({B});
                        address internal constant TL_ETHEREUM = address({A});
                        address internal constant CANCELLER = address(0);
                        address internal constant CANCELLER_ZERO = address(0x0000000000000000000000000000000000000000);
                    }}"
                ),
            )],
            &chains(),
        );
        assert_eq!(
            names(&map, "ethereum", A),
            vec!["L.TL_ETHEREUM", "L.SHARED"]
        );
        assert_eq!(names(&map, "base", A), vec!["L.SHARED"]);
        assert!(!map.to_string().contains("CANCELLER"));
    }

    #[test]
    fn entries_carry_the_declaring_path_and_addresses_key_lowercase() {
        let (map, _) = build_address_names(
            &[src(
                "src/lib/L.sol",
                "library L { address constant X = 0xAbCdEfabcdefABCDEFabcdefABCDEFabcdefABCD; }",
            )],
            &["base".to_string()],
        );
        assert_eq!(
            map,
            json!({"base": {"0xabcdefabcdefabcdefabcdefabcdefabcdefabcd": [
                {"name": "X", "contract": "L", "path": "src/lib/L.sol"}
            ]}})
        );
    }
}
