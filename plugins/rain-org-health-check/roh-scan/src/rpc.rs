//! Typed ABI encode/decode + keccak for the on-chain reads, via alloy's `sol!`
//! macro — so calldata construction and return decoding aren't hand-rolled. The
//! curl transport + RPC fallback stay in main.rs; this module is the pure ABI
//! layer (calldata builders, return decoders, the JSON-RPC result/error split)
//! and is unit-tested against known encodings.

use alloy_primitives::{hex, keccak256, Address, FixedBytes};
use alloy_sol_types::{sol, SolCall};

sol! {
    function hasRole(bytes32 role, address account) external view returns (bool);
    function getOwners() external view returns (address[]);
    function getThreshold() external view returns (uint256);
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
    function owner() external view returns (address);
    function implementation() external view returns (address);
    function name() external view returns (string);
    function symbol() external view returns (string);
    function decimals() external view returns (uint8);
    function asset() external view returns (address);
    function authorizer() external view returns (address);
    function iReceiptBeacon() external view returns (address);
    function iOffchainAssetReceiptVaultBeacon() external view returns (address);
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
/// Ethereum's receipt and receipt-vault beacons have no generated address pin —
/// they are created in the 0.1.1 beacon-set deployer's constructor and only
/// readable from these two getters, so the in-use set is resolved live.
pub fn receipt_beacon_calldata() -> String {
    to_hex(iReceiptBeaconCall {}.abi_encode())
}
pub fn receipt_vault_beacon_calldata() -> String {
    to_hex(iOffchainAssetReceiptVaultBeaconCall {}.abi_encode())
}
pub fn name_calldata() -> String {
    to_hex(nameCall {}.abi_encode())
}
pub fn symbol_calldata() -> String {
    to_hex(symbolCall {}.abi_encode())
}
pub fn decimals_calldata() -> String {
    to_hex(decimalsCall {}.abi_encode())
}
pub fn asset_calldata() -> String {
    to_hex(assetCall {}.abi_encode())
}
pub fn authorizer_calldata() -> String {
    to_hex(authorizerCall {}.abi_encode())
}

/// The `bytes32` role id an OpenZeppelin `AccessControl` grant is keyed by:
/// `keccak256(<ROLE NAME>)` over the name's UTF-8 bytes, exactly as the Solidity
/// `keccak256("DEPOSIT")` in the pinned grant map computes it. Deriving the id
/// from the name (rather than pinning literals) is what lets a role added to
/// that map be checked here without a code change.
pub fn role_id(name: &str) -> [u8; 32] {
    keccak256(name.as_bytes()).into()
}

/// `hasRole(<role>, <account>)` calldata. `None` when `account` is not a
/// 20-byte hex address — an unparseable address must not silently encode as
/// the zero address, which would read back as "not granted" for a grantee
/// nobody checked.
pub fn has_role_calldata(role: [u8; 32], account: &str) -> Option<String> {
    let account: Address = account.parse().ok()?;
    Some(to_hex(
        hasRoleCall {
            role: FixedBytes(role),
            account,
        }
        .abi_encode(),
    ))
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

/// Decode a single `address` return (`owner()` / `implementation()` / `asset()`)
/// as lowercase `0x…`.
pub fn decode_address(result_hex: &str) -> Option<String> {
    let bytes = result_bytes(result_hex)?;
    ownerCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0.to_string().to_lowercase())
}

/// Decode a `string` return (`name()` / `symbol()`).
pub fn decode_string(result_hex: &str) -> Option<String> {
    let bytes = result_bytes(result_hex)?;
    nameCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0)
}

/// Decode a `uint8` return (`decimals()`).
pub fn decode_u8(result_hex: &str) -> Option<u8> {
    let bytes = result_bytes(result_hex)?;
    decimalsCall::abi_decode_returns(&bytes, false)
        .ok()
        .map(|r| r._0)
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

// ---- deployment-state reads (rain-org-health#182) ----

/// The reads the #182 deployment-state checks are built from: a Safe's module
/// list, a receipt's vault and a vault's receipt, the orchestrator's vault-logic
/// guard, raw storage slots (the Safe singleton, guard and fallback, the ERC-1967
/// beacon slot, the OpenZeppelin initializer slot), and the role-event history an
/// authoriser's membership is rebuilt from. Pure encode/decode like the rest of
/// this module. The checks that call them land separately; the `expect` goes
/// stale, and fails the lint gate, once every item in here is wired.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumed by the #182 deployment checks, which land after these reads"
    )
)]
mod deploy_reads {
    use super::{result_bytes, to_hex};
    use alloy_primitives::{hex, Address, U256};
    use alloy_sol_types::{sol, SolCall, SolEvent};

    sol! {
        function getModulesPaginated(address start, uint256 pageSize) external view returns (address[] array, address next);
        function receipt() external view returns (address);
        function manager() external view returns (address);
        function vaultLogicIsExpected() external view returns (bool);

        event RoleGranted(bytes32 indexed role, address indexed account, address indexed sender);
        event RoleRevoked(bytes32 indexed role, address indexed account, address indexed sender);
    }

    fn lower(a: &Address) -> String {
        a.to_string().to_lowercase()
    }

    /// `getModulesPaginated(<start>, <page_size>)` calldata: one page of a Safe's
    /// module linked list, from `start` (the `0x1` sentinel for its head). `None`
    /// when `start` is not a 20-byte address, rather than asking from
    /// `address(0)`, which is not a node of the list.
    pub fn get_modules_paginated_calldata(start: &str, page_size: u64) -> Option<String> {
        let start: Address = start.parse().ok()?;
        Some(to_hex(
            getModulesPaginatedCall {
                start,
                pageSize: U256::from(page_size),
            }
            .abi_encode(),
        ))
    }

    /// Decode a `getModulesPaginated` return → `(modules, next)`, lowercase
    /// `0x…`. A Safe with no modules answers an empty page.
    pub fn decode_modules_page(result_hex: &str) -> Option<(Vec<String>, String)> {
        let bytes = result_bytes(result_hex)?;
        getModulesPaginatedCall::abi_decode_returns(&bytes, false)
            .ok()
            .map(|r| (r.array.iter().map(lower).collect(), lower(&r.next)))
    }

    /// `receipt()` calldata: the receipt a receipt vault is paired with. The
    /// address comes back through `decode_address`.
    pub fn receipt_calldata() -> String {
        to_hex(receiptCall {}.abi_encode())
    }

    /// `manager()` calldata: the vault a receipt answers to, the other half of
    /// the pairing. The address comes back through `decode_address`.
    pub fn manager_calldata() -> String {
        to_hex(managerCall {}.abi_encode())
    }

    /// `vaultLogicIsExpected()` calldata: the orchestrator's own check that the
    /// vault beacons point at the implementations it was built for. The bool
    /// comes back through `classify_bool`, so a revert stays apart from `false`.
    pub fn vault_logic_is_expected_calldata() -> String {
        to_hex(vaultLogicIsExpectedCall {}.abi_encode())
    }

    /// A 32-byte word from hex of at most 64 digits, `0x` optional, left-padded:
    /// nodes differ on whether a storage read comes back zero-trimmed (`0x0`).
    fn word_from_hex(s: &str) -> Option<[u8; 32]> {
        let digits = s.strip_prefix("0x").unwrap_or(s);
        if digits.is_empty() || digits.len() > 64 {
            return None;
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(format!("{digits:0>64}"), &mut out).ok()?;
        Some(out)
    }

    /// `eth_getStorageAt` JSON-RPC payload for `slot` of `address` at the latest
    /// block, with the slot written out as a full word. `None` when the address
    /// or the slot is malformed.
    pub fn storage_at_payload(address: &str, slot: &str) -> Option<String> {
        let address: Address = address.parse().ok()?;
        let slot = word_from_hex(slot)?;
        Some(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"eth_getStorageAt","params":["{}","0x{}","latest"]}}"#,
            lower(&address),
            hex::encode(slot)
        ))
    }

    /// Decode an `eth_getStorageAt` result into its 32-byte word.
    pub fn decode_storage_word(result_hex: &str) -> Option<[u8; 32]> {
        word_from_hex(result_hex)
    }

    /// The address a 32-byte word holds (a storage slot, or an indexed event
    /// topic), lowercase `0x…`: its low 20 bytes, and only when its high 12 are
    /// zero. A word holding anything else is not an address, and truncating it
    /// into one would report a pointer that is not there.
    pub fn word_address(word: &[u8; 32]) -> Option<String> {
        if word[..12].iter().any(|b| *b != 0) {
            return None;
        }
        Some(lower(&Address::from_slice(&word[12..])))
    }

    /// `eth_getLogs` JSON-RPC payload: the logs `address` emitted with first
    /// topic `topic0`, over the inclusive block range. One address and one topic
    /// per request, because that is the most HyperEVM's public RPC accepts; the
    /// span limit differs per endpoint, so splitting the range is the caller's.
    /// `None` for a malformed address or an inverted range.
    pub fn logs_payload(
        address: &str,
        topic0: [u8; 32],
        from_block: u64,
        to_block: u64,
    ) -> Option<String> {
        let address: Address = address.parse().ok()?;
        if from_block > to_block {
            return None;
        }
        Some(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[{{"address":"{}","topics":["0x{}"],"fromBlock":"{from_block:#x}","toBlock":"{to_block:#x}"}}]}}"#,
            lower(&address),
            hex::encode(topic0)
        ))
    }

    /// One entry of an `eth_getLogs` result.
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct Log {
        pub address: String,
        pub topics: Vec<[u8; 32]>,
        pub data: Vec<u8>,
        pub block_number: u64,
        pub log_index: u64,
        /// Retracted by a reorg: the node is taking it back, so it is not history.
        pub removed: bool,
    }

    fn quantity(s: &str) -> Option<u64> {
        u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
    }

    fn decode_log(entry: &serde_json::Value) -> Option<Log> {
        let field = |k: &str| entry.get(k).and_then(|x| x.as_str());
        let address: Address = field("address")?.parse().ok()?;
        // A topic is a full word on the wire; a short one is a malformed reply,
        // not a zero-trimmed value.
        let topics = entry
            .get("topics")?
            .as_array()?
            .iter()
            .map(|t| {
                t.as_str()
                    .filter(|s| s.strip_prefix("0x").is_some_and(|d| d.len() == 64))
                    .and_then(word_from_hex)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Log {
            address: lower(&address),
            topics,
            data: result_bytes(field("data")?)?,
            block_number: quantity(field("blockNumber")?)?,
            log_index: quantity(field("logIndex")?)?,
            removed: entry
                .get("removed")
                .and_then(|r| r.as_bool())
                .unwrap_or(false),
        })
    }

    /// The logs in an `eth_getLogs` reply, in the order the node returned them.
    /// `None` on an `error` body (a span or rate limit), a missing `result`, or
    /// any entry that does not parse. Never a partial list: a membership rebuilt
    /// from one would read as complete, with the grant the dropped entry carried
    /// silently gone from it.
    pub fn decode_logs(body: &[u8]) -> Option<Vec<Log>> {
        let v: serde_json::Value = serde_json::from_slice(body).ok()?;
        if v.get("error").is_some() {
            return None;
        }
        v.get("result")?
            .as_array()?
            .iter()
            .map(decode_log)
            .collect()
    }

    /// `topic0` of OpenZeppelin `AccessControl`'s `RoleGranted(bytes32,address,address)`.
    pub const ROLE_GRANTED_TOPIC: [u8; 32] = RoleGranted::SIGNATURE_HASH.0;
    /// `topic0` of OpenZeppelin `AccessControl`'s `RoleRevoked(bytes32,address,address)`.
    pub const ROLE_REVOKED_TOPIC: [u8; 32] = RoleRevoked::SIGNATURE_HASH.0;

    /// Which way a role event moved membership.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum RoleChange {
        Granted,
        Revoked,
    }

    /// A `RoleGranted` / `RoleRevoked` event, decoded from its log.
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct RoleEvent {
        pub change: RoleChange,
        pub role: [u8; 32],
        pub account: String,
        pub sender: String,
    }

    /// Decode a `RoleGranted` / `RoleRevoked` log. All three parameters are
    /// indexed, so the event is exactly four topics and no data. `None` for any
    /// other event, or for one not of that shape (including an address topic
    /// with its high bytes set, which is not an address).
    pub fn decode_role_event(log: &Log) -> Option<RoleEvent> {
        let [t0, role, account, sender] = log.topics.as_slice() else {
            return None;
        };
        let change = match *t0 {
            ROLE_GRANTED_TOPIC => RoleChange::Granted,
            ROLE_REVOKED_TOPIC => RoleChange::Revoked,
            _ => return None,
        };
        if !log.data.is_empty() {
            return None;
        }
        Some(RoleEvent {
            change,
            role: *role,
            account: word_address(account)?,
            sender: word_address(sender)?,
        })
    }
}

#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "re-exports the #182 reads for their callers, which land after them"
    )
)]
pub use deploy_reads::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// The two getters are the ONLY way Ethereum's receipt and receipt-vault
    /// beacons are addressable — they have no generated pin. A wrong selector
    /// returns an on-chain revert, which the scan reports as an unreadable
    /// beacon rather than as a bad selector, so the encoding is pinned here
    /// against `cast sig` output.
    #[test]
    fn beacon_getter_selectors_match_their_signatures() {
        assert_eq!(receipt_beacon_calldata(), "0x2c9b7f40");
        assert_eq!(receipt_vault_beacon_calldata(), "0x2f77a1c1");
    }

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

    /// The role id is `keccak256(<NAME>)` over the name's UTF-8 bytes — the same
    /// thing `keccak256("DEPOSIT")` computes in the pinned Solidity grant map.
    /// Pinned against an INDEPENDENT keccak (a from-scratch Keccak-256, itself
    /// checked against the published `keccak256("")` and ERC-20 `Transfer` topic
    /// vectors), not against alloy re-deriving its own answer: a wrong id asks
    /// the chain about a role nobody holds and comes back a confident `false`,
    /// which is exactly the failure that reads as "the key lost its grant".
    #[test]
    fn role_ids_are_keccak_of_the_role_name() {
        let id = |n: &str| format!("0x{}", hex::encode(role_id(n)));
        assert_eq!(
            id("DEPOSIT"),
            "0x87a7811f4bfedea3d341ad165680ae306b01aaeacc205d227629cf157dd9f821"
        );
        assert_eq!(
            id("WITHDRAW"),
            "0x7a8dc26796a1e50e6e190b70259f58f6a4edd5b22280ceecc82b687b8e982869"
        );
        assert_eq!(
            id("CERTIFY"),
            "0x50a07cb25d0d864370863300b20987dfdae089abad71b607faf639d09d053391"
        );
        assert_eq!(
            id("DEPOSIT_ADMIN"),
            "0x1ae915b310cb86de75afe5db1721d474dd0a8617151f7524866025476454bc02"
        );
        // A role name that is a PREFIX of another must not share its id.
        assert_ne!(id("DEPOSIT"), id("DEPOSIT_ADMIN"));
    }

    #[test]
    fn has_role_calldata_encodes_role_then_account() {
        let cd = has_role_calldata(
            role_id("DEPOSIT"),
            "0x1c66D6708914C40239D54919320b4C48cAE3D1A9",
        )
        .unwrap();
        // selector | role word | account word (left-padded, lowercased hex)
        assert_eq!(
            cd,
            "0x91d14854\
             87a7811f4bfedea3d341ad165680ae306b01aaeacc205d227629cf157dd9f821\
             0000000000000000000000001c66d6708914c40239d54919320b4c48cae3d1a9"
        );
        // A grantee that is not an address yields no call at all — encoding it as
        // address(0) would come back `false` and read as a revoked grant.
        assert_eq!(
            has_role_calldata(role_id("DEPOSIT"), "tokenOwnerSafe"),
            None
        );
        assert_eq!(has_role_calldata(role_id("DEPOSIT"), "0xdeadbeef"), None);
    }

    #[test]
    fn token_calldata_selectors_and_string_uint8_decoders() {
        assert!(name_calldata().starts_with("0x06fdde03")); // name()
        assert!(symbol_calldata().starts_with("0x95d89b41")); // symbol()
        assert!(decimals_calldata().starts_with("0x313ce567")); // decimals()
        assert!(asset_calldata().starts_with("0x38d52e0f")); // asset()
        assert!(authorizer_calldata().starts_with("0xd09edf31")); // authorizer()
        assert_eq!(
            decode_u8("0x0000000000000000000000000000000000000000000000000000000000000012"),
            Some(18)
        );
        // an ABI `string` return: offset(0x20) | len(6) | "wtNVDA" right-padded
        let s = "0x\
            0000000000000000000000000000000000000000000000000000000000000020\
            0000000000000000000000000000000000000000000000000000000000000006\
            77744e5644410000000000000000000000000000000000000000000000000000";
        assert_eq!(decode_string(s).as_deref(), Some("wtNVDA"));
    }

    /// The #182 reads' calldata, pinned against `cast sig` / `cast calldata`
    /// output rather than alloy re-deriving its own: a wrong selector reverts,
    /// and a check would report a healthy contract as unreadable.
    #[test]
    fn deployment_read_selectors_match_their_signatures() {
        assert_eq!(
            get_modules_paginated_calldata("0x0000000000000000000000000000000000000001", 10)
                .unwrap(),
            "0xcc2f8452\
             0000000000000000000000000000000000000000000000000000000000000001\
             000000000000000000000000000000000000000000000000000000000000000a"
        );
        assert_eq!(
            get_modules_paginated_calldata("0x1", 10),
            None,
            "a cursor that is not a full address is not the sentinel"
        );
        assert_eq!(receipt_calldata(), "0xe1e6b898");
        assert_eq!(manager_calldata(), "0x481c6a75");
        assert_eq!(vault_logic_is_expected_calldata(), "0x752c9cf5");
    }

    #[test]
    fn decodes_a_modules_page() {
        // `cast abi-encode "f(address[],address)" "[0x1c66…]" 0x…01`
        let one = "0x\
            0000000000000000000000000000000000000000000000000000000000000040\
            0000000000000000000000000000000000000000000000000000000000000001\
            0000000000000000000000000000000000000000000000000000000000000001\
            0000000000000000000000001c66d6708914c40239d54919320b4c48cae3d1a9";
        assert_eq!(
            decode_modules_page(one),
            Some((
                vec!["0x1c66d6708914c40239d54919320b4c48cae3d1a9".to_string()],
                "0x0000000000000000000000000000000000000001".to_string()
            ))
        );
        // `cast abi-encode "f(address[],address)" "[]" 0x…01`: no modules.
        let empty = "0x\
            0000000000000000000000000000000000000000000000000000000000000040\
            0000000000000000000000000000000000000000000000000000000000000001\
            0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            decode_modules_page(empty),
            Some((
                Vec::new(),
                "0x0000000000000000000000000000000000000001".to_string()
            ))
        );
        assert_eq!(decode_modules_page("0x"), None);
    }

    #[test]
    fn storage_reads_ask_for_the_full_slot_and_decode_the_word() {
        let safe = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
        let parse = |p: Option<String>| -> serde_json::Value {
            serde_json::from_str(&p.expect("a payload")).unwrap()
        };
        let p = parse(storage_at_payload(safe, "0x0"));
        assert_eq!(p["method"], "eth_getStorageAt");
        assert_eq!(
            p["params"],
            serde_json::json!([
                "0xe70d821f3462a074e63b42d0aac6523faae1d611",
                "0x0000000000000000000000000000000000000000000000000000000000000000",
                "latest"
            ])
        );
        let guard_slot = "0x4a204f620c8c5ccdca3fd54d003badd85ba500436a431f0cbda4f558c93c34c8";
        assert_eq!(
            parse(storage_at_payload(safe, guard_slot))["params"][1],
            guard_slot
        );
        assert_eq!(storage_at_payload("0xdeadbeef", "0x0"), None);
        assert_eq!(
            storage_at_payload(safe, &format!("0x1{}", "0".repeat(64))),
            None,
            "65 digits is not a slot"
        );
        assert_eq!(storage_at_payload(safe, "0xzz"), None);

        let singleton = decode_storage_word(
            "0x00000000000000000000000029fcb43b46531bca003ddc8fcb67ffe91900c762",
        )
        .unwrap();
        assert_eq!(
            word_address(&singleton).as_deref(),
            Some("0x29fcb43b46531bca003ddc8fcb67ffe91900c762")
        );
        let zero = decode_storage_word("0x0").unwrap();
        assert_eq!(zero, [0u8; 32], "a zero-trimmed reply is the zero word");
        assert_eq!(
            word_address(&zero).as_deref(),
            Some("0x0000000000000000000000000000000000000000")
        );
        let dirty = decode_storage_word(
            "0x01000000000000000000000029fcb43b46531bca003ddc8fcb67ffe91900c762",
        )
        .unwrap();
        assert_eq!(
            word_address(&dirty),
            None,
            "a word with its high bytes set is not an address"
        );
        assert_eq!(decode_storage_word("0x"), None);
    }

    #[test]
    fn logs_payload_asks_one_address_and_one_topic_over_the_range() {
        let clone = "0x66566cc91dEAf818859bD4b09B7903ac48998157";
        // The HyperEVM clone ceremony window, 41,325,390..=41,327,389.
        let p: serde_json::Value = serde_json::from_str(
            &logs_payload(clone, ROLE_GRANTED_TOPIC, 41_325_390, 41_327_389).unwrap(),
        )
        .unwrap();
        assert_eq!(p["method"], "eth_getLogs");
        assert_eq!(
            p["params"],
            serde_json::json!([{
                "address": "0x66566cc91deaf818859bd4b09b7903ac48998157",
                "topics": ["0x2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0d"],
                "fromBlock": "0x276934e",
                "toBlock": "0x2769b1d"
            }])
        );
        assert!(
            logs_payload(clone, ROLE_GRANTED_TOPIC, 5, 5).is_some(),
            "a one-block span"
        );
        assert_eq!(logs_payload(clone, ROLE_GRANTED_TOPIC, 6, 5), None);
        assert_eq!(logs_payload("0x1", ROLE_GRANTED_TOPIC, 0, 5), None);
    }

    /// The first entry of a real Robinhood `eth_getLogs` reply for the V4
    /// authoriser clone's `RoleGranted` events (2026-09-22), verbatim: the deploy
    /// key's `CERTIFY_ADMIN` from the clone's initialisation.
    const ROBINHOOD_ROLE_GRANTED_ENTRY: &str = r#"{"address":"0x66566cc91deaf818859bd4b09b7903ac48998157","topics":["0x2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0d","0x48ece560b6811ee496fa3dedc7d5be3dfce8c5eb8f1cc18626507e158a23169b","0x000000000000000000000000e8c6ede25f0e7fafe8fbc34770faba27d56c0e76","0x000000000000000000000000444acc29d63fa643e8adcc35fd9aa6de111dcb39"],"data":"0x","blockNumber":"0x38f6cdd","transactionHash":"0x5a5bfe7ebe13edfe5c8048f18bb51cfc066a57b1c9030232c861a377bcf4643f","transactionIndex":"0x9","blockHash":"0xd5d8971ad23c27165a861df68d22e0c2a672b58917598a92fca01d5cb56a4162","blockTimestamp":"0x0","logIndex":"0x3a","removed":false}"#;

    fn logs_reply(entries: &[&str]) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":[{}]}}"#,
            entries.join(",")
        )
    }

    /// The topic hashes are pinned against `cast keccak` of the two signatures.
    /// A wrong one asks for events that never fire and comes back an empty
    /// history, which a membership rebuild would read as "nobody holds a role".
    #[test]
    fn role_event_topics_are_the_access_control_signature_hashes() {
        assert_eq!(
            hex::encode(ROLE_GRANTED_TOPIC),
            "2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0d"
        );
        assert_eq!(
            hex::encode(ROLE_REVOKED_TOPIC),
            "f6391f5c32d9c69d2a47ea670b442974b53935d1edc7fd64eb21e047a839171b"
        );
    }

    #[test]
    fn decodes_a_real_role_granted_log() {
        let logs = decode_logs(logs_reply(&[ROBINHOOD_ROLE_GRANTED_ENTRY]).as_bytes()).unwrap();
        assert_eq!(logs.len(), 1);
        let log = &logs[0];
        assert_eq!(log.address, "0x66566cc91deaf818859bd4b09b7903ac48998157");
        assert_eq!(log.block_number, 59_731_165);
        assert_eq!(log.log_index, 58);
        assert!(!log.removed);
        assert!(log.data.is_empty());
        assert_eq!(
            decode_role_event(log),
            Some(RoleEvent {
                change: RoleChange::Granted,
                role: role_id("CERTIFY_ADMIN"),
                account: "0xe8c6ede25f0e7fafe8fbc34770faba27d56c0e76".into(),
                sender: "0x444acc29d63fa643e8adcc35fd9aa6de111dcb39".into(),
            })
        );
    }

    #[test]
    fn role_event_decoding_takes_only_the_two_events_in_their_one_shape() {
        let granted = decode_logs(logs_reply(&[ROBINHOOD_ROLE_GRANTED_ENTRY]).as_bytes())
            .unwrap()
            .remove(0);
        let mut revoked = granted.clone();
        revoked.topics[0] = ROLE_REVOKED_TOPIC;
        assert_eq!(
            decode_role_event(&revoked).map(|e| e.change),
            Some(RoleChange::Revoked)
        );
        let mut other = granted.clone();
        other.topics[0] = [0x11; 32];
        assert_eq!(decode_role_event(&other), None, "some other event");
        let mut short = granted.clone();
        short.topics.pop();
        assert_eq!(decode_role_event(&short), None, "three topics");
        let mut long = granted.clone();
        long.topics.push([0; 32]);
        assert_eq!(decode_role_event(&long), None, "five topics");
        let mut with_data = granted.clone();
        with_data.data = vec![0];
        assert_eq!(decode_role_event(&with_data), None, "data it cannot carry");
        let mut dirty = granted.clone();
        dirty.topics[2][0] = 1;
        assert_eq!(
            decode_role_event(&dirty),
            None,
            "an account topic with its high bytes set is not an address"
        );
    }

    /// A log list is whole or absent. A refused span, a malformed entry or a
    /// pending one must not come back as a shorter history.
    #[test]
    fn decode_logs_is_all_or_nothing() {
        assert_eq!(
            decode_logs(br#"{"jsonrpc":"2.0","id":1,"result":[]}"#),
            Some(Vec::new()),
            "no events is a real answer"
        );
        assert_eq!(
            decode_logs(
                br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"block range too large"}}"#
            ),
            None,
            "a refused span is not an empty history"
        );
        let short_topic = ROBINHOOD_ROLE_GRANTED_ENTRY.replace(
            "\"0x000000000000000000000000e8c6ede25f0e7fafe8fbc34770faba27d56c0e76\"",
            "\"0xe8c6ede25f0e7fafe8fbc34770faba27d56c0e76\"",
        );
        assert_ne!(short_topic, ROBINHOOD_ROLE_GRANTED_ENTRY);
        assert_eq!(
            decode_logs(logs_reply(&[ROBINHOOD_ROLE_GRANTED_ENTRY, &short_topic]).as_bytes()),
            None,
            "one bad entry voids the list rather than dropping out of it"
        );
        let pending = ROBINHOOD_ROLE_GRANTED_ENTRY
            .replace(r#""blockNumber":"0x38f6cdd""#, r#""blockNumber":null"#);
        assert_ne!(pending, ROBINHOOD_ROLE_GRANTED_ENTRY);
        assert_eq!(decode_logs(logs_reply(&[&pending]).as_bytes()), None);
        assert_eq!(decode_logs(b"not json"), None);
    }
}
