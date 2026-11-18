//! The deploy repo's migrations joined with each registry's `appliedV5` and its line's `headV5`; the page has no RPC.

use crate::rpc;
use alloy_primitives::{Address, FixedBytes};
use serde_json::{json, Map, Value};

pub const PATH: &str = "migrations.json";
const SCHEMA_VERSION: u64 = 2;

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
    executed_at: u64,
    onto: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Migration {
    script: String,
    id: String,
    chains: Vec<(u64, ChainEntry)>,
}

impl Migration {
    fn on(&self, chain_id: u64) -> Option<&ChainEntry> {
        self.chains
            .iter()
            .find(|(c, _)| *c == chain_id)
            .map(|(_, e)| e)
    }
}

/// Every migration sits on the one `(writer, namespace)` line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    genesis: String,
    writer: String,
    namespace: String,
    networks: Vec<Network>,
    migrations: Vec<Migration>,
}

impl Manifest {
    fn record(&self, id: &str) -> Record {
        Record {
            writer: self.writer.clone(),
            namespace: self.namespace.clone(),
            id: id.to_string(),
        }
    }

    fn find(&self, id: &str) -> Option<&Migration> {
        self.migrations
            .iter()
            .find(|x| x.id.eq_ignore_ascii_case(id))
    }
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
    let writer = address(&root, "writer", "root")?;
    let namespace = word(&root, "namespace", "root")?;
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
        let id = word(m, "id", &at)?;
        let mut chains = Vec::new();
        for (k, c) in object(m, "chains", &at)? {
            let cat = format!("{at}.chains.{k}");
            chains.push((
                chain_id(k, &cat)?,
                ChainEntry {
                    executed_at: field(c, "executedAt", &cat)?
                        .as_u64()
                        .ok_or_else(|| format!("{cat}.executedAt is not a timestamp"))?,
                    onto: word(c, "onto", &cat)?,
                },
            ));
        }
        migrations.push(Migration { script, id, chains });
    }
    Ok(Manifest {
        genesis,
        writer,
        namespace,
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

/// The registry and `appliedV5` key for migration `id` on `network`: the file's one line.
pub fn locate(src: &str, network: &str, id: &str) -> Option<(Network, Record)> {
    let m = parse(src).ok()?;
    let net = m.networks.iter().find(|n| n.name == network)?.clone();
    m.find(id)?;
    Some((net, m.record(id)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameUnlocated {
    pub id: String,
    pub network: String,
}

impl std::fmt::Display for RenameUnlocated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "expected-state.json renameMigration {} is not in migrations.json for {}",
            self.id, self.network
        )
    }
}

/// Whether `id` is applied on `network`: `Ok(None)` when migrations.json or `appliedV5` could not be read.
pub fn renamed(
    src: &str,
    network: &str,
    id: &str,
    call: EthCalls,
) -> Result<Option<bool>, RenameUnlocated> {
    if parse(src).is_err() {
        return Ok(None);
    }
    let (net, r) = locate(src, network, id).ok_or_else(|| RenameUnlocated {
        id: id.to_string(),
        network: network.to_string(),
    })?;
    Ok(read_applied(&net, &[r], call)
        .pop()
        .flatten()
        .map(|t| t > 0))
}

/// The block the heads were read at, and one slot per `(writer, namespace)` line; `None` where the read did not answer.
pub type ReadHeads<'a> =
    &'a dyn Fn(&Network, &[(String, String)]) -> (Option<u64>, Vec<Option<String>>);

pub fn read_heads(
    net: &Network,
    lines: &[(String, String)],
    call: EthCalls,
) -> Vec<Option<String>> {
    let encoded: Vec<Option<String>> = lines
        .iter()
        .map(|(w, ns)| rpc::head_v5_calldata(w, ns))
        .collect();
    let calls: Vec<(String, String)> = encoded
        .iter()
        .flatten()
        .map(|data| (net.registry.clone(), data.clone()))
        .collect();
    let mut answers = call(&calls).into_iter();
    encoded
        .iter()
        .map(|data| {
            data.as_ref()
                .and_then(|_| answers.next().flatten())
                .and_then(|hex| rpc::decode_word(&hex))
        })
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

/// Indices into `m.migrations`: the chain's line from genesis along `onto`, then those with no entry on the chain; `Err` says why the line cannot be followed.
fn line_order(m: &Manifest, chain_id: u64) -> Result<Vec<usize>, String> {
    let on: Vec<(usize, &str)> = m
        .migrations
        .iter()
        .enumerate()
        .filter_map(|(i, x)| x.on(chain_id).map(|e| (i, e.onto.as_str())))
        .collect();
    let script = |i: usize| m.migrations[i].script.as_str();
    let mut line: Vec<usize> = Vec::new();
    let mut tip = m.genesis.as_str();
    loop {
        let next: Vec<usize> = on
            .iter()
            .filter(|(_, o)| o.eq_ignore_ascii_case(tip))
            .map(|(i, _)| *i)
            .collect();
        match next.as_slice() {
            [] => break,
            [i] if line.contains(i) => {
                return Err(format!("the line cycles back to {}", script(*i)));
            }
            [i] => {
                line.push(*i);
                tip = m.migrations[*i].id.as_str();
            }
            many => {
                let names: Vec<&str> = many.iter().map(|&i| script(i)).collect();
                return Err(format!(
                    "the line forks at {tip}: {} all go onto it",
                    names.join(", ")
                ));
            }
        }
    }
    if let Some((i, onto)) = on.iter().find(|(i, _)| !line.contains(i)) {
        return Err(format!(
            "{} goes onto {onto}, which is not on the line from genesis",
            script(*i)
        ));
    }
    line.extend((0..m.migrations.len()).filter(|&i| m.migrations[i].on(chain_id).is_none()));
    Ok(line)
}

fn network_json(
    m: &Manifest,
    net: &Network,
    rpc_host: Option<String>,
    read: ReadApplied,
    heads: ReadHeads,
) -> Value {
    let genesis = |id: &str| id.eq_ignore_ascii_case(&m.genesis);
    let mut asked: Vec<Record> = Vec::new();
    for mig in &m.migrations {
        let Some(entry) = mig.on(net.chain_id) else {
            continue;
        };
        let onto = (!genesis(&entry.onto)).then(|| m.record(&entry.onto));
        for r in std::iter::once(m.record(&mig.id)).chain(onto) {
            if !asked.iter().any(|a| a.same(&r)) {
                asked.push(r);
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
    let order = line_order(m, net.chain_id);
    let rows: Vec<Value> = order
        .clone()
        .unwrap_or_else(|_| (0..m.migrations.len()).collect())
        .into_iter()
        .map(|i| &m.migrations[i])
        .map(|mig| {
            let Some(entry) = mig.on(net.chain_id) else {
                return json!({
                    "script": mig.script,
                    "namespace": m.namespace,
                    "id": mig.id,
                    "writer": m.writer,
                    "state": "unknown",
                    "appliedAt": null,
                    "executedAt": null,
                    "waitsOn": [],
                });
            };
            let at = applied(&m.record(&mig.id));
            let onto = if genesis(&entry.onto) {
                json!({
                    "writer": m.writer,
                    "namespace": m.namespace,
                    "id": entry.onto,
                    "script": null,
                    "state": "genesis",
                    "appliedAt": null,
                    "executedAt": null,
                })
            } else {
                let target = m.find(&entry.onto);
                let executed = target
                    .and_then(|x| x.on(net.chain_id))
                    .map_or(0, |e| e.executed_at);
                let pa = applied(&m.record(&entry.onto));
                json!({
                    "writer": m.writer,
                    "namespace": m.namespace,
                    "id": entry.onto,
                    "script": target.map(|x| x.script.clone()),
                    "state": state(pa, executed),
                    "appliedAt": pa.map_or(Value::Null, when),
                    "executedAt": when(executed),
                })
            };
            json!({
                "script": mig.script,
                "namespace": m.namespace,
                "id": mig.id,
                "writer": m.writer,
                "state": state(at, entry.executed_at),
                "appliedAt": at.map_or(Value::Null, when),
                "executedAt": when(entry.executed_at),
                "waitsOn": [onto],
            })
        })
        .collect();
    json!({
        "chainId": net.chain_id,
        "network": net.name,
        "registry": net.registry,
        "rpcHost": rpc_host,
        "unread": (0..asked.len()).filter(|i| answers.get(*i).copied().flatten().is_none()).count(),
        "lineOrder": order.is_ok(),
        "lineOrderReason": order.err(),
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
    const ID_4: &str = "0x0000000000000000000000000000000000000000000000000000000000000004";
    const SAFE_X: &str = "0x3840aeDaEc8e82f79d8F6a8F6ADCa271E13E0329";
    const SAFE_Y: &str = "0xe70d821f3462a074e63b42d0AaC6523faAe1d611";
    const REGISTRY: &str = "0xc89F95eC7e626CFBef37a1F502A69BaF2022F8A8";

    fn entry(executed_at: u64, onto: &str) -> Value {
        json!({"executedAt": executed_at, "onto": onto})
    }

    /// Keys sorted and chain ids as string keys, as the producer writes them.
    fn fixture() -> Value {
        json!({
            "genesis": GENESIS,
            "migrations": [
                {
                    "chains": {
                        "10": entry(1_700_000_000, GENESIS),
                        "7": entry(1_700_000_500, GENESIS),
                    },
                    "id": ID_1, "script": "script/one.s.sol"
                },
                {
                    "chains": {"10": entry(0, ID_1), "7": entry(0, ID_1)},
                    "id": ID_2, "script": "script/two.s.sol"
                },
                {
                    "chains": {"10": entry(0, ID_2), "7": entry(0, ID_1)},
                    "id": ID_3, "script": "script/three.s.sol"
                }
            ],
            "namespace": NS_A,
            "networks": {
                "10": {"network": "tenchain", "registry": REGISTRY},
                "7": {"network": "sevenchain", "registry": REGISTRY}
            },
            "version": 2,
            "writer": SAFE_X
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
    fn every_migration_is_listed_on_every_network() {
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
                vec!["script/one.s.sol", "script/two.s.sol", "script/three.s.sol"]
            );
        }
        assert_eq!(net(&doc, "sevenchain")["rpcHost"], "rpc.sevenchain");
        assert_eq!(net(&doc, "tenchain")["rpcHost"], Value::Null);
    }

    fn scripts(doc: &Value, network: &str) -> Vec<String> {
        net(doc, network)["migrations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["script"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn each_network_lists_its_own_line_from_genesis_then_the_migrations_it_lacks() {
        let f = json!({
            "genesis": GENESIS,
            "migrations": [
                {
                    "chains": {"10": entry(0, ID_2), "7": entry(0, ID_1)},
                    "id": ID_3, "script": "script/c.s.sol"
                },
                {
                    "chains": {"10": entry(0, GENESIS), "7": entry(0, GENESIS)},
                    "id": ID_1, "script": "script/a.s.sol"
                },
                {
                    "chains": {"10": entry(0, ID_3)},
                    "id": ID_4, "script": "script/d.s.sol"
                },
                {
                    "chains": {"10": entry(0, ID_1), "7": entry(0, ID_3)},
                    "id": ID_2, "script": "script/b.s.sol"
                }
            ],
            "namespace": NS_A,
            "networks": {
                "10": {"network": "tenchain", "registry": REGISTRY},
                "7": {"network": "sevenchain", "registry": REGISTRY}
            },
            "version": 2,
            "writer": SAFE_X
        });
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        assert_eq!(
            scripts(&doc, "tenchain"),
            ["script/a.s.sol", "script/b.s.sol", "script/c.s.sol", "script/d.s.sol"]
        );
        assert_eq!(
            scripts(&doc, "sevenchain"),
            ["script/a.s.sol", "script/c.s.sol", "script/b.s.sol", "script/d.s.sol"]
        );
        for n in ["tenchain", "sevenchain"] {
            assert_eq!(net(&doc, n)["lineOrder"], true, "{n}");
            assert_eq!(net(&doc, n)["lineOrderReason"], Value::Null, "{n}");
        }
    }

    #[test]
    fn a_forked_line_is_listed_in_file_order_with_the_fork_named() {
        let mut f = fixture();
        f["migrations"].as_array_mut().unwrap().reverse();
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let seven = net(&doc, "sevenchain");
        assert_eq!(seven["lineOrder"], false);
        assert_eq!(
            seven["lineOrderReason"],
            format!("the line forks at {ID_1}: script/three.s.sol, script/two.s.sol all go onto it")
        );
        assert_eq!(
            scripts(&doc, "sevenchain"),
            ["script/three.s.sol", "script/two.s.sol", "script/one.s.sol"]
        );
        assert_eq!(net(&doc, "tenchain")["lineOrder"], true);
        assert_eq!(
            scripts(&doc, "tenchain"),
            ["script/one.s.sol", "script/two.s.sol", "script/three.s.sol"]
        );
    }

    #[test]
    fn a_broken_line_is_listed_in_file_order_with_the_break_named() {
        let mut f = fixture();
        f["migrations"][2]["chains"]["10"]["onto"] = Value::from(ID_4);
        f["migrations"].as_array_mut().unwrap().reverse();
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let ten = net(&doc, "tenchain");
        assert_eq!(ten["lineOrder"], false);
        assert_eq!(
            ten["lineOrderReason"],
            format!("script/three.s.sol goes onto {ID_4}, which is not on the line from genesis")
        );
        assert_eq!(
            scripts(&doc, "tenchain"),
            ["script/three.s.sol", "script/two.s.sol", "script/one.s.sol"]
        );
    }

    #[test]
    fn a_cycle_is_listed_in_file_order_with_the_cycle_named() {
        let mut f = fixture();
        f["migrations"][1]["id"] = Value::from(ID_1);
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let ten = net(&doc, "tenchain");
        assert_eq!(ten["lineOrder"], false);
        assert_eq!(
            ten["lineOrderReason"],
            "the line cycles back to script/two.s.sol"
        );
        assert_eq!(
            scripts(&doc, "tenchain"),
            ["script/one.s.sol", "script/two.s.sol", "script/three.s.sol"]
        );
    }

    #[test]
    fn applied_carries_the_registry_time_and_the_files_line() {
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_A, ID_2, 1_791_457_413)]);
        let doc = render(&fixture().to_string(), &reg);
        let two = row(&doc, "sevenchain", "script/two.s.sol");
        assert_eq!(two["state"], "applied");
        assert_eq!(two["appliedAt"], 1_791_457_413u64);
        assert_eq!(two["writer"], SAFE_X);
        assert_eq!(two["namespace"], NS_A);
        assert_eq!(two["id"], ID_2);
        let other = row(&doc, "tenchain", "script/two.s.sol");
        assert_eq!(other["state"], "notApplied");
        assert_eq!(other["appliedAt"], Value::Null);
        assert_eq!(other["writer"], SAFE_X);
    }

    #[test]
    fn a_record_under_another_writer_or_namespace_does_not_count() {
        for (w, ns) in [(SAFE_Y, NS_A), (SAFE_X, NS_B)] {
            let reg = Registry::new(vec![("sevenchain", w, ns, ID_2, 5)]);
            let doc = render(&fixture().to_string(), &reg);
            assert_eq!(
                row(&doc, "sevenchain", "script/two.s.sol")["state"],
                "notApplied",
                "{w} {ns}"
            );
        }
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
    fn the_files_line_is_listed_once_with_its_head() {
        let head = format!("0x{}", "ab".repeat(32));
        let asked = RefCell::new(Vec::new());
        let doc = build(
            "acme",
            "deploy",
            &fixture().to_string(),
            &host,
            &|_, r| vec![None; r.len()],
            &|n, l| {
                asked.borrow_mut().push((n.name.clone(), l.to_vec()));
                (Some(77), vec![Some(head.clone())])
            },
        );
        let line = vec![(SAFE_X.to_string(), NS_A.to_string())];
        assert_eq!(
            *asked.borrow(),
            vec![
                ("sevenchain".to_string(), line.clone()),
                ("tenchain".to_string(), line),
            ]
        );
        for n in ["sevenchain", "tenchain"] {
            assert_eq!(
                net(&doc, n)["lines"],
                json!([{"writer": SAFE_X, "namespace": NS_A, "headBlock": 77, "head": head}]),
                "{n}"
            );
        }
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
    fn an_unencodable_line_is_unread_and_unasked_while_the_others_are_read() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let lines = vec![
            (SAFE_X.to_string(), NS_A.to_string()),
            ("safe".to_string(), NS_A.to_string()),
            (SAFE_Y.to_string(), NS_B.to_string()),
        ];
        let sent = RefCell::new(Vec::new());
        let got = read_heads(&net, &lines, &|calls| {
            sent.borrow_mut().extend_from_slice(calls);
            calls
                .iter()
                .enumerate()
                .map(|(i, _)| Some(format!("0x{}", ["ab", "cd", "ef"][i].repeat(32))))
                .collect()
        });
        assert_eq!(
            got,
            vec![
                Some(format!("0x{}", "ab".repeat(32))),
                None,
                Some(format!("0x{}", "cd".repeat(32))),
            ]
        );
        let want: Vec<(String, String)> = [&lines[0], &lines[2]]
            .iter()
            .map(|(w, ns)| (REGISTRY.to_string(), rpc::head_v5_calldata(w, ns).unwrap()))
            .collect();
        assert_eq!(*sent.borrow(), want);
    }

    #[test]
    fn lines_none_of_which_encode_are_all_unread_and_none_asked() {
        let net = Network {
            chain_id: 7,
            name: "sevenchain".to_string(),
            registry: REGISTRY.to_string(),
        };
        let lines = vec![
            ("safe".to_string(), NS_A.to_string()),
            (SAFE_X.to_string(), "ns".to_string()),
        ];
        let sent = RefCell::new(Vec::new());
        let got = read_heads(&net, &lines, &|calls| {
            sent.borrow_mut().extend_from_slice(calls);
            vec![Some(format!("0x{}", "ab".repeat(32))); 2]
        });
        assert_eq!(got, vec![None, None]);
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn each_record_is_asked_once_and_genesis_never() {
        let reg = Registry::new(vec![]);
        render(&fixture().to_string(), &reg);
        let want: Vec<Record> = [ID_1, ID_2, ID_3]
            .iter()
            .map(|id| Record {
                writer: SAFE_X.into(),
                namespace: NS_A.into(),
                id: (*id).into(),
            })
            .collect();
        let asked = reg.asked.borrow();
        for n in ["sevenchain", "tenchain"] {
            assert_eq!(&asked.iter().find(|(x, _)| x == n).unwrap().1, &want, "{n}");
        }
    }

    #[test]
    fn waits_on_is_the_migration_the_chain_applies_it_onto() {
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
        let w = &row(&doc, "tenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["script"], "script/two.s.sol");
        assert_eq!(w["id"], ID_2);
        assert_eq!(w["state"], "notApplied");
        assert_eq!(w["executedAt"], Value::Null);
        let reg = Registry::new(vec![("sevenchain", SAFE_X, NS_A, ID_1, 1_700_000_900)]);
        let doc = render(&fixture().to_string(), &reg);
        let w = &row(&doc, "sevenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["state"], "applied");
        assert_eq!(w["appliedAt"], 1_700_000_900u64);
    }

    #[test]
    fn a_migration_onto_genesis_waits_on_genesis() {
        let doc = render(&fixture().to_string(), &Registry::new(vec![]));
        let w = &row(&doc, "sevenchain", "script/one.s.sol")["waitsOn"][0];
        assert_eq!(w["state"], "genesis");
        assert_eq!(w["id"], GENESIS);
        assert_eq!(w["namespace"], NS_A);
        assert_eq!(w["script"], Value::Null);
    }

    #[test]
    fn an_onto_the_file_does_not_list_is_unnamed_not_dropped() {
        let mut f = fixture();
        f["migrations"][2]["chains"]["7"]["onto"] = Value::from(ID_4);
        let reg = Registry::new(vec![]);
        let doc = render(&f.to_string(), &reg);
        let w = &row(&doc, "sevenchain", "script/three.s.sol")["waitsOn"][0];
        assert_eq!(w["script"], Value::Null);
        assert_eq!(w["id"], ID_4);
        assert_eq!(w["state"], "notApplied");
        let asked = reg.asked.borrow();
        let seven = &asked.iter().find(|(n, _)| n == "sevenchain").unwrap().1;
        assert!(seven.iter().any(|r| r.id == ID_4));
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
        f["migrations"][1]["chains"]
            .as_object_mut()
            .unwrap()
            .remove("10");
        let doc = render(&f.to_string(), &Registry::new(vec![]));
        let two = row(&doc, "tenchain", "script/two.s.sol");
        assert_eq!(two["state"], "unknown");
        assert_eq!(two["executedAt"], Value::Null);
        assert_eq!(two["waitsOn"], json!([]));
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
                Box::new(|f| f["version"] = Value::from(1)),
                "not schema version 2",
            ),
            (
                Box::new(|f| {
                    f.as_object_mut().unwrap().remove("version");
                }),
                "not schema version 2",
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
                Box::new(|f| f["writer"] = Value::from("0x1")),
                "root.writer is not an address",
            ),
            (
                Box::new(|f| {
                    f.as_object_mut().unwrap().remove("namespace");
                }),
                "root has no namespace",
            ),
            (
                Box::new(|f| f["namespace"] = Value::from("0x1")),
                "root.namespace is not 32 bytes of hex",
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
                "migrations.0.id is not a string",
            ),
            (
                Box::new(|f| {
                    f["migrations"][1].as_object_mut().unwrap().remove("id");
                }),
                "migrations.1 has no id",
            ),
            (
                Box::new(|f| f["migrations"][1]["chains"] = json!([])),
                "migrations.1.chains is not an object",
            ),
            (
                Box::new(|f| f["migrations"][1]["chains"]["7"]["onto"] = Value::from("0x1")),
                "migrations.1.chains.7.onto is not 32 bytes of hex",
            ),
            (
                Box::new(|f| {
                    f["migrations"][1]["chains"]["7"]
                        .as_object_mut()
                        .unwrap()
                        .remove("onto");
                }),
                "migrations.1.chains.7 has no onto",
            ),
            (
                Box::new(|f| f["migrations"][2]["chains"]["7"]["executedAt"] = Value::from(-1)),
                "migrations.2.chains.7.executedAt is not a timestamp",
            ),
            (
                Box::new(|f| f["migrations"][2]["chains"]["x"] = entry(0, ID_1)),
                "migrations.2.chains.x is keyed \"x\", not a chain id",
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

    #[test]
    fn locate_names_the_network_registry_and_the_files_line_record() {
        let src = fixture().to_string();
        let (net, rec) = locate(&src, "sevenchain", ID_2).unwrap();
        assert_eq!(
            net,
            Network {
                chain_id: 7,
                name: "sevenchain".into(),
                registry: REGISTRY.into(),
            }
        );
        assert_eq!(
            rec,
            Record {
                writer: SAFE_X.into(),
                namespace: NS_A.into(),
                id: ID_2.into(),
            }
        );
        let (net, rec) = locate(&src, "tenchain", ID_3).unwrap();
        assert_eq!(net.chain_id, 10);
        assert_eq!(rec.writer, SAFE_X);
        assert_eq!(rec.namespace, NS_A);
        assert_eq!(rec.id, ID_3);
    }

    /// The migration id matches in any case and the record keeps the id as asked.
    #[test]
    fn locate_matches_the_migration_id_ignoring_case() {
        let mut f = fixture();
        let upper = format!("0x{}", ID_2[2..].to_uppercase().replace('2', "A"));
        let lower = upper.to_lowercase();
        f["migrations"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .for_each(|m| {
                if m["id"] == ID_2 {
                    m["id"] = json!(upper);
                }
            });
        let (_, rec) = locate(&f.to_string(), "sevenchain", &lower).unwrap();
        assert_eq!(rec.writer, SAFE_X);
        assert_eq!(rec.namespace, NS_A);
        assert_eq!(rec.id, lower);
    }

    #[test]
    fn locate_is_none_for_an_unknown_network_or_id_or_an_unreadable_file() {
        let src = fixture().to_string();
        assert_eq!(locate(&src, "elevenchain", ID_1), None);
        assert_eq!(locate(&src, "sevenchain", GENESIS), None);
        assert_eq!(locate(&src, "sevenchain", ID_4), None);
        assert_eq!(locate("{", "sevenchain", ID_1), None);
        let mut f = fixture();
        f["version"] = Value::from(1);
        assert_eq!(locate(&f.to_string(), "sevenchain", ID_1), None);
    }

    fn applied_at(t: u64) -> Option<String> {
        Some(format!("0x{t:064x}"))
    }

    #[test]
    fn renamed_is_unlocated_when_the_file_places_no_record_for_the_id_on_the_network() {
        let src = fixture().to_string();
        let asked = RefCell::new(0);
        let call = |c: &[(String, String)]| {
            *asked.borrow_mut() += 1;
            vec![applied_at(1); c.len()]
        };
        let err = renamed(&src, "sevenchain", GENESIS, &call).unwrap_err();
        assert_eq!(
            err,
            RenameUnlocated {
                id: GENESIS.into(),
                network: "sevenchain".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "expected-state.json renameMigration \
             0xd85a7f14bb19a07649810ac09029cc9c29667e4059cb83e258bc52e213365d3f \
             is not in migrations.json for sevenchain"
        );
        assert_eq!(
            renamed(&src, "elevenchain", ID_1, &call),
            Err(RenameUnlocated {
                id: ID_1.into(),
                network: "elevenchain".into(),
            })
        );
        assert_eq!(*asked.borrow(), 0);
    }

    /// An unreadable file is a failed read, never an unlocated id.
    #[test]
    fn renamed_is_unread_when_the_file_cannot_be_read() {
        let mut off_schema = fixture();
        off_schema["version"] = Value::from(1);
        let mut no_registry = fixture();
        no_registry["networks"]["7"]["registry"] = Value::from("nope");
        let asked = RefCell::new(0);
        let call = |c: &[(String, String)]| {
            *asked.borrow_mut() += 1;
            vec![applied_at(1); c.len()]
        };
        for src in [
            String::new(),
            "{".to_string(),
            off_schema.to_string(),
            no_registry.to_string(),
        ] {
            assert_eq!(renamed(&src, "sevenchain", ID_1, &call), Ok(None), "{src}");
            assert_eq!(
                renamed(&src, "sevenchain", GENESIS, &call),
                Ok(None),
                "{src}"
            );
        }
        assert_eq!(*asked.borrow(), 0);
    }

    #[test]
    fn renamed_is_unread_when_applied_does_not_answer() {
        let src = fixture().to_string();
        for answer in [vec![], vec![None], vec![Some("0x01".to_string())]] {
            assert_eq!(
                renamed(&src, "sevenchain", ID_2, &|_| answer.clone()),
                Ok(None),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn renamed_is_whether_the_chains_registry_records_the_id_applied() {
        let src = fixture().to_string();
        let sent = RefCell::new(Vec::new());
        let ask = |network: &str, id: &str, t: u64| {
            renamed(&src, network, id, &|c| {
                sent.borrow_mut().extend_from_slice(c);
                vec![applied_at(t)]
            })
        };
        assert_eq!(ask("sevenchain", ID_2, 1_791_457_413), Ok(Some(true)));
        assert_eq!(ask("sevenchain", ID_2, 1), Ok(Some(true)));
        assert_eq!(ask("sevenchain", ID_2, 0), Ok(Some(false)));
        let seven = (
            REGISTRY.to_string(),
            rpc::applied_v5_calldata(SAFE_X, NS_A, ID_2).unwrap(),
        );
        assert_eq!(*sent.borrow(), vec![seven.clone(), seven.clone(), seven]);

        sent.borrow_mut().clear();
        assert_eq!(ask("tenchain", ID_3, 1_700_000_000), Ok(Some(true)));
        assert_eq!(
            *sent.borrow(),
            vec![(
                REGISTRY.to_string(),
                rpc::applied_v5_calldata(SAFE_X, NS_A, ID_3).unwrap(),
            )]
        );
    }
}
