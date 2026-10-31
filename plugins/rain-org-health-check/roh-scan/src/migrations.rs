//! The deploy repo's migrations joined with each registry's `appliedV5` and each line's `headV5`; the page has no RPC.

use crate::rpc;
use alloy_primitives::{Address, FixedBytes};
use serde_json::{json, Map, Value};

pub const PATH: &str = "migrations.json";
const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub writer: String,
    pub namespace: String,
    pub id: String,
}

impl Record {
    fn same(&self, other: &Record) -> bool {
        self.writer.eq_ignore_ascii_case(&other.writer)
            && self.namespace.eq_ignore_ascii_case(&other.namespace)
            && self.id.eq_ignore_ascii_case(&other.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub chain_id: u64,
    pub name: String,
    pub registry: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainEntry {
    writer: String,
    executed_at: u64,
    prerequisites: Vec<Record>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Migration {
    script: String,
    key: Option<(String, String)>,
    chains: Vec<(u64, ChainEntry)>,
}

impl Migration {
    fn on(&self, chain_id: u64) -> Option<&ChainEntry> {
        self.chains
            .iter()
            .find(|(c, _)| *c == chain_id)
            .map(|(_, e)| e)
    }

    fn is(&self, namespace: &str, id: &str) -> bool {
        self.key
            .as_ref()
            .is_some_and(|(n, i)| n.eq_ignore_ascii_case(namespace) && i.eq_ignore_ascii_case(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    genesis: String,
    networks: Vec<Network>,
    migrations: Vec<Migration>,
}

fn field<'a>(v: &'a Value, key: &str, at: &str) -> Result<&'a Value, String> {
    v.get(key).ok_or_else(|| format!("{at} has no {key}"))
}

fn string(v: &Value, key: &str, at: &str) -> Result<String, String> {
    field(v, key, at)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{at}.{key} is not a string"))
}

fn address(v: &Value, key: &str, at: &str) -> Result<String, String> {
    let s = string(v, key, at)?;
    s.parse::<Address>()
        .map_err(|_| format!("{at}.{key} is not an address"))?;
    Ok(s)
}

fn word(v: &Value, key: &str, at: &str) -> Result<String, String> {
    let s = string(v, key, at)?;
    s.parse::<FixedBytes<32>>()
        .map_err(|_| format!("{at}.{key} is not 32 bytes of hex"))?;
    Ok(s)
}

fn object<'a>(v: &'a Value, key: &str, at: &str) -> Result<&'a Map<String, Value>, String> {
    field(v, key, at)?
        .as_object()
        .ok_or_else(|| format!("{at}.{key} is not an object"))
}

fn chain_id(key: &str, at: &str) -> Result<u64, String> {
    key.parse::<u64>()
        .map_err(|_| format!("{at} is keyed {key:?}, not a chain id"))
}

/// Strict: a file read only in part is unavailable, never shown in part.
fn parse(src: &str) -> Result<Manifest, String> {
    let root: Value = serde_json::from_str(src).map_err(|e| format!("not JSON ({e})"))?;
    match root.get("version").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => {}
        _ => return Err(format!("not schema version {SCHEMA_VERSION}")),
    }
    let genesis = word(&root, "genesis", "root")?;
    let mut networks = Vec::new();
    for (k, n) in object(&root, "networks", "root")? {
        let at = format!("networks.{k}");
        networks.push(Network {
            chain_id: chain_id(k, &at)?,
            name: string(n, "network", &at)?,
            registry: address(n, "registry", &at)?,
        });
    }
    networks.sort_by_key(|n| n.chain_id);
    let rows = field(&root, "migrations", "root")?
        .as_array()
        .ok_or("root.migrations is not an array")?;
    let mut migrations = Vec::new();
    for (i, m) in rows.iter().enumerate() {
        let at = format!("migrations.{i}");
        let script = string(m, "script", &at)?;
        let chains_in = object(m, "chains", &at)?;
        let key = match (
            field(m, "namespace", &at)?.is_null(),
            field(m, "id", &at)?.is_null(),
        ) {
            (true, true) if chains_in.is_empty() => None,
            (true, true) => return Err(format!("{at} has no id but lists chains")),
            (false, false) => Some((word(m, "namespace", &at)?, word(m, "id", &at)?)),
            _ => return Err(format!("{at} has one of namespace and id null")),
        };
        let mut chains = Vec::new();
        for (k, c) in chains_in {
            let cat = format!("{at}.chains.{k}");
            let mut prerequisites = Vec::new();
            let pre = field(c, "prerequisites", &cat)?
                .as_array()
                .ok_or_else(|| format!("{cat}.prerequisites is not an array"))?;
            for (j, p) in pre.iter().enumerate() {
                let pat = format!("{cat}.prerequisites.{j}");
                prerequisites.push(Record {
                    writer: address(p, "writer", &pat)?,
                    namespace: word(p, "namespace", &pat)?,
                    id: word(p, "id", &pat)?,
                });
            }
            chains.push((
                chain_id(k, &cat)?,
                ChainEntry {
                    writer: address(c, "writer", &cat)?,
                    executed_at: field(c, "executedAt", &cat)?
                        .as_u64()
                        .ok_or_else(|| format!("{cat}.executedAt is not a timestamp"))?,
                    prerequisites,
                },
            ));
        }
        migrations.push(Migration {
            script,
            key,
            chains,
        });
    }
    Ok(Manifest {
        genesis,
        networks,
        migrations,
    })
}

/// One slot per record; `None` where the read did not answer.
pub type ReadApplied<'a> = &'a dyn Fn(&Network, &[Record]) -> Vec<Option<u64>>;

/// `(to, calldata)` pairs in, each raw result out.
pub type EthCalls<'a> = &'a dyn Fn(&[(String, String)]) -> Vec<Option<String>>;

pub fn read_applied(net: &Network, records: &[Record], call: EthCalls) -> Vec<Option<u64>> {
    let Some(calls) = records
        .iter()
        .map(|r| {
            rpc::applied_v5_calldata(&r.writer, &r.namespace, &r.id)
                .map(|data| (net.registry.clone(), data))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return vec![None; records.len()];
    };
    call(&calls)
        .into_iter()
        .map(|hex| hex.as_deref().and_then(rpc::decode_uint))
        .collect()
}

/// The block the heads were read at, and one slot per `(writer, namespace)` line; `None` where the read did not answer.
pub type ReadHeads<'a> =
    &'a dyn Fn(&Network, &[(String, String)]) -> (Option<u64>, Vec<Option<String>>);

pub fn read_heads(
    net: &Network,
    lines: &[(String, String)],
    call: EthCalls,
) -> Vec<Option<String>> {
    let Some(calls) = lines
        .iter()
        .map(|(w, ns)| rpc::head_v5_calldata(w, ns).map(|data| (net.registry.clone(), data)))
        .collect::<Option<Vec<_>>>()
    else {
        return vec![None; lines.len()];
    };
    call(&calls)
        .into_iter()
        .map(|hex| hex.as_deref().and_then(rpc::decode_word))
        .collect()
}

/// Registry 0 for a migration the file says ran is a missing record, not one that has not run.
fn state(applied: Option<u64>, executed_at: u64) -> &'static str {
    match applied {
        None => "unknown",
        Some(t) if t > 0 => "applied",
        Some(_) if executed_at > 0 => "notRecorded",
        Some(_) => "notApplied",
    }
}

fn when(t: u64) -> Value {
    if t > 0 {
        Value::from(t)
    } else {
        Value::Null
    }
}

fn network_json(
    m: &Manifest,
    net: &Network,
    rpc_host: Option<String>,
    read: ReadApplied,
    heads: ReadHeads,
) -> Value {
    let genesis = |r: &Record| r.id.eq_ignore_ascii_case(&m.genesis);
    let mut asked: Vec<Record> = Vec::new();
    for mig in &m.migrations {
        let (Some((namespace, id)), Some(entry)) = (&mig.key, mig.on(net.chain_id)) else {
            continue;
        };
        let own = Record {
            writer: entry.writer.clone(),
            namespace: namespace.clone(),
            id: id.clone(),
        };
        for r in std::iter::once(&own).chain(entry.prerequisites.iter().filter(|p| !genesis(p))) {
            if !asked.iter().any(|a| a.same(r)) {
                asked.push(r.clone());
            }
        }
    }
    let answers = read(net, &asked);
    let mut lines: Vec<(String, String)> = Vec::new();
    for r in &asked {
        if !lines.iter().any(|(w, ns)| {
            w.eq_ignore_ascii_case(&r.writer) && ns.eq_ignore_ascii_case(&r.namespace)
        }) {
            lines.push((r.writer.clone(), r.namespace.clone()));
        }
    }
    let (head_block, head_answers) = heads(net, &lines);
    let applied = |r: &Record| {
        asked
            .iter()
            .position(|a| a.same(r))
            .and_then(|i| answers.get(i).copied().flatten())
    };
    let rows: Vec<Value> = m
        .migrations
        .iter()
        .map(|mig| {
            let Some((namespace, id)) = &mig.key else {
                return json!({
                    "script": mig.script,
                    "namespace": null,
                    "id": null,
                    "writer": null,
                    "state": "unregistered",
                    "appliedAt": null,
                    "executedAt": null,
                    "waitsOn": [],
                });
            };
            let Some(entry) = mig.on(net.chain_id) else {
                return json!({
                    "script": mig.script,
                    "namespace": namespace,
                    "id": id,
                    "writer": null,
                    "state": "unknown",
                    "appliedAt": null,
                    "executedAt": null,
                    "waitsOn": [],
                });
            };
            let at = applied(&Record {
                writer: entry.writer.clone(),
                namespace: namespace.clone(),
                id: id.clone(),
            });
            let waits: Vec<Value> = entry
                .prerequisites
                .iter()
                .map(|p| {
                    if genesis(p) {
                        return json!({
                            "writer": p.writer,
                            "namespace": p.namespace,
                            "id": p.id,
                            "script": null,
                            "state": "genesis",
                            "appliedAt": null,
                            "executedAt": null,
                        });
                    }
                    let target = m.migrations.iter().find(|x| x.is(&p.namespace, &p.id));
                    let executed = target
                        .and_then(|x| x.on(net.chain_id))
                        .map_or(0, |e| e.executed_at);
                    let pa = applied(p);
                    json!({
                        "writer": p.writer,
                        "namespace": p.namespace,
                        "id": p.id,
                        "script": target.map(|x| x.script.clone()),
                        "state": state(pa, executed),
                        "appliedAt": pa.map_or(Value::Null, when),
                        "executedAt": when(executed),
                    })
                })
                .collect();
            json!({
                "script": mig.script,
                "namespace": namespace,
                "id": id,
                "writer": entry.writer,
                "state": state(at, entry.executed_at),
                "appliedAt": at.map_or(Value::Null, when),
                "executedAt": when(entry.executed_at),
                "waitsOn": waits,
            })
        })
        .collect();
    json!({
        "chainId": net.chain_id,
        "network": net.name,
        "registry": net.registry,
        "rpcHost": rpc_host,
        "unread": (0..asked.len()).filter(|i| answers.get(*i).copied().flatten().is_none()).count(),
        "migrations": rows,
        "lines": lines
            .iter()
            .enumerate()
            .map(|(i, (w, ns))| {
                let head = head_answers.get(i).cloned().flatten();
                json!({
                    "writer": w,
                    "namespace": ns,
                    "headBlock": head.as_ref().and(head_block),
                    "head": head,
                })
            })
            .collect::<Vec<_>>(),
    })
}

pub fn build(
    org: &str,
    repo: &str,
    src: &str,
    rpc_host: &dyn Fn(&str) -> Option<String>,
    read: ReadApplied,
    heads: ReadHeads,
) -> Value {
    let mut doc = json!({"org": org, "repo": repo, "path": PATH});
    let parsed = if src.trim().is_empty() {
        Err("the scan could not fetch it".to_string())
    } else {
        parse(src)
    };
    match parsed {
        Err(reason) => doc["unavailable"] = Value::from(format!("{PATH}: {reason}")),
        Ok(m) => {
            doc["genesis"] = Value::from(m.genesis.clone());
            doc["networks"] = m
                .networks
                .iter()
                .map(|n| network_json(&m, n, rpc_host(&n.name), read, heads))
                .collect();
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const GENESIS: &str = "0xd85a7f14bb19a07649810ac09029cc9c29667e4059cb83e258bc52e213365d3f";
    const NS_A: &str = "0x00000000000000000000000000000000000000000000000000000000000000aa";
    const NS_B: &str = "0x00000000000000000000000000000000000000000000000000000000000000bb";
    const ID_1: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
    const ID_2: &str = "0x0000000000000000000000000000000000000000000000000000000000000002";
    const ID_3: &str = "0x0000000000000000000000000000000000000000000000000000000000000003";
    const SAFE_X: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const SAFE_Y: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const REGISTRY: &str = "0xc89F95eC7e626CFBef37a1F502A69BaF2022F8A8";

    fn pre(writer: &str, ns: &str, id: &str) -> Value {
        json!({"id": id, "namespace": ns, "writer": writer})
    }

    fn entry(writer: &str, executed_at: u64, prerequisites: Vec<Value>) -> Value {
        json!({"executedAt": executed_at, "prerequisites": prerequisites, "writer": writer})
    }

    /// Keys sorted and chain ids as string keys, as the producer writes them.
    fn fixture() -> Value {
        json!({
            "genesis": GENESIS,
            "migrations": [
                {
                    "chains": {
                        "10": entry(SAFE_Y, 1_700_000_000, vec![pre(SAFE_Y, NS_A, GENESIS)]),
                        "7": entry(SAFE_X, 1_700_000_500, vec![pre(SAFE_X, NS_A, GENESIS)]),
                    },
                    "id": ID_1, "namespace": NS_A, "script": "script/one.s.sol"
                },
                {"chains": {}, "id": null, "namespace": null, "script": "script/unrecorded.s.sol"},
                {
                    "chains": {
                        "10": entry(SAFE_Y, 0, vec![pre(SAFE_Y, NS_B, GENESIS)]),
                        "7": entry(SAFE_X, 0, vec![pre(SAFE_X, NS_B, GENESIS)]),
                    },
                    "id": ID_2, "namespace": NS_B, "script": "script/two.s.sol"
                },
                {
                    "chains": {
                        "10": entry(SAFE_Y, 0, vec![pre(SAFE_Y, NS_A, ID_1)]),
                        "7": entry(SAFE_X, 0, vec![pre(SAFE_X, NS_A, ID_1)]),
                    },
                    "id": ID_3, "namespace": NS_A, "script": "script/three.s.sol"
                }
            ],
            "networks": {
                "10": {"network": "tenchain", "registry": REGISTRY},
                "7": {"network": "sevenchain", "registry": REGISTRY}
            },
            "version": 1
        })
    }

    /// `(network, writer, namespace, id, appliedAt)`.
    type Entry = (&'static str, &'static str, &'static str, &'static str, u64);

    struct Registry {
        table: Vec<Entry>,
        asked: RefCell<Vec<(String, Vec<Record>)>>,
    }

    impl Registry {
        fn new(table: Vec<Entry>) -> Self {
            Registry {
                table,
                asked: RefCell::new(Vec::new()),
            }
        }

        fn answer(&self, net: &Network, records: &[Record]) -> Vec<Option<u64>> {
            self.asked
                .borrow_mut()
                .push((net.name.clone(), records.to_vec()));
            records
                .iter()
                .map(|r| {
                    Some(
                        self.table
                            .iter()
                            .find(|(n, w, ns, i, _)| {
                                *n == net.name
                                    && w.eq_ignore_ascii_case(&r.writer)
                                    && ns.eq_ignore_ascii_case(&r.namespace)
                                    && i.eq_ignore_ascii_case(&r.id)
                            })
                            .map_or(0, |(_, _, _, _, t)| *t),
                    )
                })
                .collect()
        }
    }

    fn host(name: &str) -> Option<String> {
        (name != "tenchain").then(|| format!("rpc.{name}"))
    }

    fn no_heads(_: &Network, lines: &[(String, String)]) -> (Option<u64>, Vec<Option<String>>) {
        (None, vec![None; lines.len()])
    }

    fn render(src: &str, registry: &Registry) -> Value {
        build(
            "acme",
            "deploy",
            src,
            &host,
            &|n, r| registry.answer(n, r),
            &no_heads,
        )
    }

    fn net<'a>(doc: &'a Value, name: &str) -> &'a Value {
        doc["networks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["network"] == name)
            .unwrap()
    }

    fn row<'a>(doc: &'a Value, network: &str, script: &str) -> &'a Value {
        net(doc, network)["migrations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["script"] == script)
            .unwrap()
    }

    #[test]
    fn every_migration_is_listed_on_every_network_in_file_order() {
        let doc = render(&fixture().to_string(), &Registry::new(vec![]));
        assert_eq!(doc["org"], "acme");
        assert_eq!(doc["repo"], "deploy");
        assert_eq!(doc["path"], PATH);
        assert_eq!(doc["genesis"], GENESIS);
        assert!(doc.get("unavailable").is_none());
        let nets = doc["networks"].as_array().unwrap();
        // Numeric chain-id order, not the file's string-sorted "10" < "7".
        let ids: Vec<u64> = nets
            .iter()
            .map(|n| n["chainId"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![7, 10]);
        for n in nets {
            assert_eq!(n["registry"], REGISTRY);
            let scripts: Vec<&str> = n["migrations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["script"].as_str().unwrap())
                .collect();
            assert_eq!(
                scripts,
                vec![
                    "script/one.s.sol",
                    "script/unrecorded.s.sol",
                    "script/two.s.sol",
                    "script/three.s.sol"
                ]
            );
        }
        assert_eq!(net(&doc, "sevenchain")["rpcHost"], "rpc.sevenchain");
        assert_eq!(net(&doc, "tenchain")["rpcHost"], Value::Null);
    }

    #[test]
    fn applied_carries_the_registry_time_and_the_chain_writer() {
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_B, ID_2, 1_791_457_413)]);
        let doc = render(&fixture().to_string(), &reg);
        let two = row(&doc, "sevenchain", "script/two.s.sol");
        assert_eq!(two["state"], "applied");
        assert_eq!(two["appliedAt"], 1_791_457_413u64);
        assert_eq!(two["writer"], SAFE_X);
        assert_eq!(two["namespace"], NS_B);
        assert_eq!(two["id"], ID_2);
        let other = row(&doc, "tenchain", "script/two.s.sol");
        assert_eq!(other["state"], "notApplied");
        assert_eq!(other["appliedAt"], Value::Null);
        assert_eq!(other["writer"], SAFE_Y);
    }

    #[test]
    fn a_record_under_another_writer_does_not_count() {
        let reg = Registry::new(vec![("sevenchain", SAFE_Y, NS_B, ID_2, 5)]);
        let doc = render(&fixture().to_string(), &reg);
        assert_eq!(
            row(&doc, "sevenchain", "script/two.s.sol")["state"],
            "notApplied"
        );
    }

    #[test]
    fn the_same_id_in_another_namespace_is_its_own_record() {
        let mut f = fixture();
        f["migrations"][2]["id"] = Value::from(ID_1);
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_A, ID_1, 1_700_000_900)]);
        let doc = render(&f.to_string(), &reg);
        assert_eq!(
            row(&doc, "sevenchain", "script/one.s.sol")["state"],
            "applied"
        );
        assert_eq!(
            row(&doc, "sevenchain", "script/two.s.sol")["state"],
            "notApplied"
        );
    }

    #[test]
    fn a_prerequisite_under_another_writer_is_its_own_record() {
        let mut f = fixture();
        f["migrations"][3]["chains"]["7"]["prerequisites"] = json!([pre(SAFE_Y, NS_B, ID_2)]);
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_B, ID_2, 1_700_000_900)]);
        let doc = render(&f.to_string(), &reg);
        assert_eq!(
            row(&doc, "sevenchain", "script/two.s.sol")["state"],
            "applied"
        );
        let w = &row(&doc, "sevenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["writer"], SAFE_Y);
        assert_eq!(w["state"], "notApplied");
    }

    #[test]
    fn applied_is_asked_of_the_registry_and_decoded() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let records = vec![
            Record {
                writer: SAFE_X.to_string(),
                namespace: NS_A.to_string(),
                id: ID_1.to_string(),
            },
            Record {
                writer: SAFE_Y.to_string(),
                namespace: NS_B.to_string(),
                id: ID_2.to_string(),
            },
            Record {
                writer: SAFE_Y.to_string(),
                namespace: NS_B.to_string(),
                id: ID_3.to_string(),
            },
        ];
        let sent = RefCell::new(Vec::new());
        let got = read_applied(&net, &records, &|calls| {
            sent.borrow_mut().extend_from_slice(calls);
            vec![
                Some(format!("0x{:064x}", 1_791_457_413u64)),
                None,
                Some("0x01".to_string()),
            ]
        });
        assert_eq!(got, vec![Some(1_791_457_413), None, None]);
        let want: Vec<(String, String)> = records
            .iter()
            .map(|r| {
                (
                    REGISTRY.to_string(),
                    rpc::applied_v5_calldata(&r.writer, &r.namespace, &r.id).unwrap(),
                )
            })
            .collect();
        assert_eq!(*sent.borrow(), want);
    }

    #[test]
    fn a_record_that_cannot_be_encoded_asks_nothing() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let records = vec![
            Record {
                writer: SAFE_X.to_string(),
                namespace: NS_A.to_string(),
                id: ID_1.to_string(),
            },
            Record {
                writer: "safe".to_string(),
                namespace: NS_A.to_string(),
                id: ID_2.to_string(),
            },
        ];
        let called = RefCell::new(false);
        let got = read_applied(&net, &records, &|_| {
            *called.borrow_mut() = true;
            vec![Some(format!("0x{:064x}", 1u64)); 2]
        });
        assert_eq!(got, vec![None, None]);
        assert!(!*called.borrow());
    }

    #[test]
    fn each_line_the_asked_records_sit_on_is_listed_once_with_its_head() {
        let mut f = fixture();
        f["migrations"][3]["chains"]["7"]["writer"] = Value::from(SAFE_X.to_lowercase());
        f["migrations"][3]["chains"]["7"]["prerequisites"] = json!([pre(SAFE_Y, NS_B, ID_2)]);
        let head = format!("0x{}", "ab".repeat(32));
        let asked = RefCell::new(Vec::new());
        let doc = build(
            "acme",
            "deploy",
            &f.to_string(),
            &host,
            &|_, r| vec![None; r.len()],
            &|n, l| {
                asked.borrow_mut().push((n.name.clone(), l.to_vec()));
                (Some(77), vec![Some(head.clone()), None])
            },
        );
        let line = |w: &str, ns: &str| (w.to_string(), ns.to_string());
        assert_eq!(
            *asked.borrow(),
            vec![
                (
                    "sevenchain".to_string(),
                    vec![line(SAFE_X, NS_A), line(SAFE_X, NS_B), line(SAFE_Y, NS_B)]
                ),
                (
                    "tenchain".to_string(),
                    vec![line(SAFE_Y, NS_A), line(SAFE_Y, NS_B)]
                ),
            ]
        );
        assert_eq!(
            net(&doc, "sevenchain")["lines"],
            json!([
                {"writer": SAFE_X, "namespace": NS_A, "headBlock": 77, "head": head},
                {"writer": SAFE_X, "namespace": NS_B, "headBlock": null, "head": null},
                {"writer": SAFE_Y, "namespace": NS_B, "headBlock": null, "head": null}
            ])
        );
        assert_eq!(
            net(&doc, "tenchain")["lines"],
            json!([
                {"writer": SAFE_Y, "namespace": NS_A, "headBlock": 77, "head": head},
                {"writer": SAFE_Y, "namespace": NS_B, "headBlock": null, "head": null}
            ])
        );
    }

    #[test]
    fn a_head_read_at_no_known_block_carries_no_block() {
        let head = format!("0x{}", "ab".repeat(32));
        let doc = build(
            "acme",
            "deploy",
            &fixture().to_string(),
            &host,
            &|_, r| vec![None; r.len()],
            &|_, l| (None, vec![Some(head.clone()); l.len()]),
        );
        for n in ["sevenchain", "tenchain"] {
            let lines = net(&doc, n)["lines"].as_array().unwrap().clone();
            assert!(!lines.is_empty(), "{n}");
            for l in lines {
                assert_eq!(l["head"], Value::from(head.clone()), "{n}");
                assert_eq!(l["headBlock"], Value::Null, "{n}");
            }
        }
    }

    #[test]
    fn heads_are_asked_of_the_registry_and_decoded() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let lines = vec![
            (SAFE_X.to_string(), NS_A.to_string()),
            (SAFE_Y.to_string(), NS_B.to_string()),
            (SAFE_Y.to_string(), NS_A.to_string()),
        ];
        let sent = RefCell::new(Vec::new());
        let got = read_heads(&net, &lines, &|calls| {
            sent.borrow_mut().extend_from_slice(calls);
            vec![
                Some(format!("0x{}", "AB".repeat(32))),
                None,
                Some("0x01".to_string()),
            ]
        });
        assert_eq!(
            got,
            vec![Some(format!("0x{}", "ab".repeat(32))), None, None]
        );
        let want: Vec<(String, String)> = lines
            .iter()
            .map(|(w, ns)| (REGISTRY.to_string(), rpc::head_v5_calldata(w, ns).unwrap()))
            .collect();
        assert_eq!(*sent.borrow(), want);
    }

    #[test]
    fn a_line_that_cannot_be_encoded_asks_no_head() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let lines = vec![
            (SAFE_X.to_string(), NS_A.to_string()),
            ("safe".to_string(), NS_A.to_string()),
        ];
        let called = RefCell::new(false);
        let got = read_heads(&net, &lines, &|_| {
            *called.borrow_mut() = true;
            vec![Some(format!("0x{}", "ab".repeat(32))); 2]
        });
        assert_eq!(got, vec![None, None]);
        assert!(!*called.borrow());
    }

    #[test]
    fn executed_but_unrecorded_is_not_recorded_with_when_it_ran() {
        let doc = render(&fixture().to_string(), &Registry::new(vec![]));
        let one = row(&doc, "sevenchain", "script/one.s.sol");
        assert_eq!(one["state"], "notRecorded");
        assert_eq!(one["executedAt"], 1_700_000_500u64);
        assert_eq!(one["appliedAt"], Value::Null);
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_A, ID_1, 1_700_000_500)]);
        let doc = render(&fixture().to_string(), &reg);
        let one = row(&doc, "sevenchain", "script/one.s.sol");
        assert_eq!(one["state"], "applied");
        assert_eq!(one["appliedAt"], 1_700_000_500u64);
    }

    #[test]
    fn a_migration_with_no_id_is_unregistered_and_never_asked() {
        let reg = Registry::new(vec![]);
        let doc = render(&fixture().to_string(), &reg);
        for n in ["sevenchain", "tenchain"] {
            let r = row(&doc, n, "script/unrecorded.s.sol");
            assert_eq!(r["state"], "unregistered");
            assert_eq!(r["id"], Value::Null);
            assert_eq!(r["namespace"], Value::Null);
            assert_eq!(r["writer"], Value::Null);
            assert_eq!(r["executedAt"], Value::Null);
        }
        for (_, records) in reg.asked.borrow().iter() {
            assert!(records.iter().all(|r| r.id != GENESIS));
        }
    }

    #[test]
    fn each_record_is_asked_once_and_genesis_never() {
        let reg = Registry::new(vec![]);
        render(&fixture().to_string(), &reg);
        let asked = reg.asked.borrow();
        let seven = &asked.iter().find(|(n, _)| n == "sevenchain").unwrap().1;
        let want: Vec<Record> = [ID_1, ID_2, ID_3]
            .iter()
            .map(|id| Record {
                writer: SAFE_X.into(),
                namespace: if *id == ID_2 { NS_B } else { NS_A }.into(),
                id: (*id).into(),
            })
            .collect();
        assert_eq!(seven, &want);
    }

    #[test]
    fn waits_on_names_the_prerequisite_and_its_own_state() {
        let doc = render(&fixture().to_string(), &Registry::new(vec![]));
        let three = row(&doc, "sevenchain", "script/three.s.sol");
        assert_eq!(three["state"], "notApplied");
        let w = &three["waitsOn"].as_array().unwrap()[0];
        assert_eq!(w["script"], "script/one.s.sol");
        assert_eq!(w["id"], ID_1);
        assert_eq!(w["namespace"], NS_A);
        assert_eq!(w["writer"], SAFE_X);
        assert_eq!(w["state"], "notRecorded");
        assert_eq!(w["executedAt"], 1_700_000_500u64);
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_A, ID_1, 1_700_000_900)]);
        let doc = render(&fixture().to_string(), &reg);
        let w = &row(&doc, "sevenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["state"], "applied");
        assert_eq!(w["appliedAt"], 1_700_000_900u64);
    }

    #[test]
    fn a_genesis_prerequisite_is_first_in_line_and_not_asked() {
        let doc = render(&fixture().to_string(), &Registry::new(vec![]));
        let w = &row(&doc, "sevenchain", "script/two.s.sol")["waitsOn"][0];
        assert_eq!(w["state"], "genesis");
        assert_eq!(w["namespace"], NS_B);
        assert_eq!(w["script"], Value::Null);
    }

    #[test]
    fn a_prerequisite_the_file_does_not_list_is_unnamed_not_dropped() {
        let mut f = fixture();
        f["migrations"][3]["chains"]["7"]["prerequisites"][0]["id"] = Value::from(ID_2);
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let w = &row(&doc, "sevenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["script"], Value::Null);
        assert_eq!(w["id"], ID_2);
        assert_eq!(w["state"], "notApplied");
    }

    #[test]
    fn an_unanswered_read_is_unknown_never_not_applied() {
        let doc = build(
            "acme",
            "deploy",
            &fixture().to_string(),
            &host,
            &|_, r| vec![None; r.len()],
            &no_heads,
        );
        for n in ["sevenchain", "tenchain"] {
            assert_eq!(net(&doc, n)["unread"], 3);
            assert_eq!(row(&doc, n, "script/two.s.sol")["state"], "unknown");
            assert_eq!(row(&doc, n, "script/one.s.sol")["state"], "unknown");
            assert_eq!(
                row(&doc, n, "script/three.s.sol")["waitsOn"][0]["state"],
                "unknown"
            );
            assert_eq!(
                row(&doc, n, "script/unrecorded.s.sol")["state"],
                "unregistered"
            );
        }
    }

    #[test]
    fn a_short_answer_leaves_the_tail_unknown() {
        let doc = build(
            "acme",
            "deploy",
            &fixture().to_string(),
            &host,
            &|_, _| vec![Some(9)],
            &no_heads,
        );
        let n = "sevenchain";
        assert_eq!(row(&doc, n, "script/one.s.sol")["state"], "applied");
        assert_eq!(row(&doc, n, "script/two.s.sol")["state"], "unknown");
        assert_eq!(row(&doc, n, "script/three.s.sol")["state"], "unknown");
        assert_eq!(net(&doc, n)["unread"], 2);
    }

    #[test]
    fn a_chain_the_migration_does_not_list_is_unknown() {
        let mut f = fixture();
        f["migrations"][2]["chains"]
            .as_object_mut()
            .unwrap()
            .remove("10");
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let two = row(&doc, "tenchain", "script/two.s.sol");
        assert_eq!(two["state"], "unknown");
        assert_eq!(two["writer"], Value::Null);
        assert_eq!(two["id"], ID_2);
    }

    #[test]
    fn an_unfetched_file_is_unavailable() {
        for src in ["", "  \n"] {
            let doc = render(src, &Registry::new(vec![]));
            assert_eq!(
                doc["unavailable"],
                "migrations.json: the scan could not fetch it"
            );
            assert!(doc.get("networks").is_none());
            assert_eq!(doc["org"], "acme");
        }
    }

    #[test]
    fn a_file_off_schema_is_unavailable_with_where() {
        type Mutate = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(Mutate, &str)> = vec![
            (
                Box::new(|f| f["version"] = Value::from(2)),
                "not schema version 1",
            ),
            (
                Box::new(|f| {
                    f.as_object_mut().unwrap().remove("version");
                }),
                "not schema version 1",
            ),
            (
                Box::new(|f| f["genesis"] = Value::from("0x12")),
                "root.genesis is not 32 bytes of hex",
            ),
            (
                Box::new(|f| {
                    f.as_object_mut().unwrap().remove("genesis");
                }),
                "root has no genesis",
            ),
            (
                Box::new(|f| f["networks"] = json!([])),
                "root.networks is not an object",
            ),
            (
                Box::new(|f| f["networks"]["x"] = json!({"network": "x", "registry": REGISTRY})),
                "networks.x is keyed \"x\", not a chain id",
            ),
            (
                Box::new(|f| f["networks"]["7"]["registry"] = Value::from("nope")),
                "networks.7.registry is not an address",
            ),
            (
                Box::new(|f| f["networks"]["7"]["network"] = Value::from(7)),
                "networks.7.network is not a string",
            ),
            (
                Box::new(|f| f["migrations"] = json!({})),
                "root.migrations is not an array",
            ),
            (
                Box::new(|f| f["migrations"][0]["script"] = Value::Null),
                "migrations.0.script is not a string",
            ),
            (
                Box::new(|f| f["migrations"][0]["id"] = Value::Null),
                "migrations.0 has one of namespace and id null",
            ),
            (
                Box::new(|f| f["migrations"][1]["chains"]["7"] = entry(SAFE_X, 0, vec![])),
                "migrations.1 has no id but lists chains",
            ),
            (
                Box::new(|f| f["migrations"][2]["namespace"] = Value::from("0x1")),
                "migrations.2.namespace is not 32 bytes of hex",
            ),
            (
                Box::new(|f| f["migrations"][2]["chains"]["7"]["writer"] = Value::from("0x1")),
                "migrations.2.chains.7.writer is not an address",
            ),
            (
                Box::new(|f| f["migrations"][2]["chains"]["7"]["executedAt"] = Value::from(-1)),
                "migrations.2.chains.7.executedAt is not a timestamp",
            ),
            (
                Box::new(|f| f["migrations"][2]["chains"]["7"]["prerequisites"] = json!({})),
                "migrations.2.chains.7.prerequisites is not an array",
            ),
            (
                Box::new(|f| {
                    f["migrations"][2]["chains"]["7"]["prerequisites"][0]["id"] = Value::Null
                }),
                "migrations.2.chains.7.prerequisites.0.id is not a string",
            ),
            (
                Box::new(|f| f["migrations"][3]["chains"]["x"] = entry(SAFE_X, 0, vec![])),
                "migrations.3.chains.x is keyed \"x\", not a chain id",
            ),
        ];
        for (mutate, why) in cases {
            let mut f = fixture();
            mutate(&mut f);
            let doc = render(&f.to_string(), &Registry::new(vec![]));
            assert_eq!(
                doc["unavailable"],
                format!("migrations.json: {why}"),
                "{why}"
            );
            assert!(doc.get("networks").is_none(), "{why}");
        }
        let doc = render("{", &Registry::new(vec![]));
        assert!(doc["unavailable"]
            .as_str()
            .unwrap()
            .starts_with("migrations.json: not JSON ("));
    }
}
