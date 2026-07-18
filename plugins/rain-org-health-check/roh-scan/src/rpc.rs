//! Typed ABI encode/decode + keccak for the on-chain reads, via alloy's `sol!`
//! macro — so calldata construction and return decoding aren't hand-rolled. The
//! curl transport + RPC fallback stay in main.rs; this module is the pure ABI
//! layer (calldata builders, return decoders, the JSON-RPC result/error split)
//! and is unit-tested against known encodings.

use alloy_primitives::{hex, keccak256};
use alloy_sol_types::{sol, SolCall};

sol! {
    function getOwners() external view returns (address[]);
    function getThreshold() external view returns (uint256);
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
    function owner() external view returns (address);
    function implementation() external view returns (address);
}

/// The outcome of a `bool`-returning `eth_call` (i.e. `supportsInterface`): a
/// decoded value, an on-chain revert (the contract doesn't implement it), or an
/// undetermined result (RPC failure / malformed reply).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CallClass {
    True,
    False,
    Reverted,
    Unknown,
}

/// keccak256 of the bytes represented by `hex_str` (with or without `0x`), as a
/// lowercase `0x…` string — Ethereum's codehash. `None` if not valid hex.
pub fn keccak256_hex(hex_str: &str) -> Option<String> {
    let bytes = hex::decode(hex_str.strip_prefix("0x").unwrap_or(hex_str)).ok()?;
    Some(format!("0x{}", hex::encode(keccak256(bytes))))
}

fn to_hex(calldata: Vec<u8>) -> String {
    format!("0x{}", hex::encode(calldata))
}

fn result_bytes(result_hex: &str) -> Option<Vec<u8>> {
    hex::decode(result_hex.strip_prefix("0x").unwrap_or(result_hex)).ok()
}

// ---- calldata builders (0x-hex) ----

pub fn get_owners_calldata() -> String {
    to_hex(getOwnersCall {}.abi_encode())
}
pub fn get_threshold_calldata() -> String {
    to_hex(getThresholdCall {}.abi_encode())
}
pub fn supports_interface_calldata(interface_id: [u8; 4]) -> String {
    to_hex(
        supportsInterfaceCall {
            interfaceId: interface_id.into(),
        }
        .abi_encode(),
    )
}
pub fn owner_calldata() -> String {
    to_hex(ownerCall {}.abi_encode())
}
pub fn implementation_calldata() -> String {
    to_hex(implementationCall {}.abi_encode())
}

// ---- return decoders (from the `eth_call` result hex) ----

/// Decode a `getOwners()` return → the owner addresses as lowercase `0x…`.
pub fn decode_owners(result_hex: &str) -> Option<Vec<String>> {
    let bytes = result_bytes(result_hex)?;
    getOwnersCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0.iter().map(|a| a.to_string().to_lowercase()).collect())
}

/// Decode a `uint256` return that fits a `u64` (the Safe threshold).
pub fn decode_uint(result_hex: &str) -> Option<u64> {
    let bytes = result_bytes(result_hex)?;
    getThresholdCall::abi_decode_returns(&bytes, false)
        .ok()
        .and_then(|r| r._0.try_into().ok())
}

/// Decode a single `address` return (`owner()` / `implementation()`) as
/// lowercase `0x…`.
pub fn decode_address(result_hex: &str) -> Option<String> {
    let bytes = result_bytes(result_hex)?;
    ownerCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0.to_string().to_lowercase())
}

/// Classify a JSON-RPC reply for a `bool`-returning call: `result` → True/False,
/// `error` (execution reverted) → Reverted, anything else → Unknown. The
/// revert-vs-failure split is the whole point (a beacon reverting on
/// `supportsInterface` is a stable "absent", not a transient failure).
pub fn classify_bool(body: &[u8]) -> CallClass {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return CallClass::Unknown;
    };
    if v.get("error").is_some() {
        return CallClass::Reverted;
    }
    match v.get("result").and_then(|r| r.as_str()) {
        Some(hex_str) => match decode_bool(hex_str) {
            Some(true) => CallClass::True,
            Some(false) => CallClass::False,
            None => CallClass::Unknown,
        },
        None => CallClass::Unknown,
    }
}

/// The `result` hex from a JSON-RPC reply (`None` on an error / malformed body).
pub fn result_hex(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("result")?.as_str().map(str::to_string)
}

fn decode_bool(result_hex: &str) -> Option<bool> {
    let bytes = result_bytes(result_hex)?;
    supportsInterfaceCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak256_of_empty_is_the_known_vector() {
        assert_eq!(
            keccak256_hex("0x").unwrap(),
            "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert_eq!(keccak256_hex(""), keccak256_hex("0x"));
        assert_eq!(keccak256_hex("xyz"), None);
    }

    #[test]
    fn calldata_selectors_are_correct() {
        // 4-byte selectors are keccak(signature)[..4].
        assert!(get_owners_calldata().starts_with("0xa0e67e2b")); // getOwners()
        assert!(get_threshold_calldata().starts_with("0xe75235b8")); // getThreshold()
        assert!(owner_calldata().starts_with("0x8da5cb5b")); // owner()
        assert!(implementation_calldata().starts_with("0x5c60da1b")); // implementation()
        let s = supports_interface_calldata([0x01, 0xff, 0xc9, 0xa7]);
        assert!(s.starts_with("0x01ffc9a7")); // supportsInterface(bytes4) selector
                                              // the bytes4 arg is left-aligned in the 32-byte word after the selector
        assert!(s.contains("01ffc9a700000000000000000000000000000000000000000000000000000000"));
    }

    #[test]
    fn decodes_a_getowners_return() {
        // offset(0x20) | len(2) | addr1 | addr2
        let hex = "0x\
            0000000000000000000000000000000000000000000000000000000000000020\
            0000000000000000000000000000000000000000000000000000000000000002\
            0000000000000000000000004746095b1ea1a84446d34448f44e74d3d51f92f2\
            000000000000000000000000cec2cb8b8ee4000ffa3f8a7f8e0fa0a3e3dab72d";
        assert_eq!(
            decode_owners(hex).unwrap(),
            vec![
                "0x4746095b1ea1a84446d34448f44e74d3d51f92f2".to_string(),
                "0xcec2cb8b8ee4000ffa3f8a7f8e0fa0a3e3dab72d".to_string(),
            ]
        );
        assert_eq!(decode_owners("0x1234"), None);
    }

    #[test]
    fn decodes_uint_and_address() {
        let three = "0x0000000000000000000000000000000000000000000000000000000000000003";
        assert_eq!(decode_uint(three), Some(3));
        let addr = "0x000000000000000000000000e70d821f3462a074e63b42d0aac6523faae1d611";
        assert_eq!(
            decode_address(addr),
            Some("0xe70d821f3462a074e63b42d0aac6523faae1d611".to_string())
        );
    }

    #[test]
    fn classify_bool_splits_true_false_revert_unknown() {
        let t =
            br#"{"result":"0x0000000000000000000000000000000000000000000000000000000000000001"}"#;
        let f =
            br#"{"result":"0x0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let rev = br#"{"error":{"code":3,"message":"execution reverted"}}"#;
        assert_eq!(classify_bool(t), CallClass::True);
        assert_eq!(classify_bool(f), CallClass::False);
        assert_eq!(classify_bool(rev), CallClass::Reverted);
        assert_eq!(classify_bool(b"not json"), CallClass::Unknown);
    }
}
