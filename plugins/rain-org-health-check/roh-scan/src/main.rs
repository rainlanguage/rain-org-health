//! roh-scan — scan a GitHub org's repos for rainix/soldeer modernization-debt signals.
//! Signal detection lives in signals.rs (pure, tested); this file is the gh/network
//! orchestration and output rendering (text report + optional JSON).
//!
//! Usage:
//!   roh-scan [--json <path>] [repo ...]
//! Env: ORG (default rainlanguage), PAR (default 12), JSON_OUT (default site/health.json).

// `json!` nests one macro expansion per key; the health.json documents are
// wide enough to exceed the default 128-deep limit.
#![recursion_limit = "512"]

mod audit;
mod blobs;
mod commentloc;
mod deployhealth;
mod graph;
mod mutation;
mod owners;
mod protofire;
mod rpc;
mod signals;
use audit::{audit_sort_key, parse_last_audit, parse_runs_jsonl, LastAudit};
use protofire::{
    anchor_ref, changed_source_file_count, classify_anchor, classify_external_audit,
    counts_as_source_drift, days_between, is_stale, newest_pdf_index, source_drift, AuditAnchor,
    AuditPdf, CompareFile,
};
use signals::{detect_signals, foundry_package_name, RepoInputs};

use serde_json::json;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Current UTC time as an ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` string (via `date -u`,
/// matching the format `health.json` already stamps). "" if `date` is unavailable.
fn now_iso() -> String {
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn gh_stdout(args: &[&str]) -> Option<String> {
    let out = Command::new("gh").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The three distinguishable results of a `gh api` fetch. Keeping `NotFound`
/// (a genuine 404) separate from `Failed` (rate-limit/network/spawn error after
/// retries) is the crux of issue #52: an errored fetch must never masquerade as
/// an empty resource and become a false coverage claim.
#[derive(Debug, Clone, PartialEq)]
enum FetchOutcome {
    Found(String),
    NotFound,
    Failed,
}

/// The fetch seam. The real impl (`GhCli`) shells out to `gh`; tests inject a
/// scripted double so the audit path is exercised without network or subprocesses.
trait GhApi: Sync {
    fn api_jq(&self, args: &[&str]) -> FetchOutcome;
}

/// How a single failed (`non-zero exit`) `gh` invocation should be treated. A
/// `HTTP 404` in stderr (`gh: Not Found (HTTP 404)`) is a genuine absence; every
/// other failure (secondary-rate-limit 403/429, network, spawn error) is retried.
#[derive(Debug, PartialEq)]
enum GhFailure {
    NotFound,
    Retryable,
}

/// Classify a failed `gh` invocation from its stderr.
fn classify_gh_failure(stderr: &str) -> GhFailure {
    if stderr.contains("HTTP 404") {
        GhFailure::NotFound
    } else {
        GhFailure::Retryable
    }
}

/// The outcome of ONE fetch attempt, before the retry decision folds it into a
/// final `FetchOutcome`.
enum Attempt {
    Found(String),
    NotFound,
    Retryable,
}

/// Bounded-retry driver: run up to `max_attempts` attempts, retrying only a
/// `Retryable` result; return on the first `Found`/`NotFound`, and once retries
/// are exhausted a still-`Retryable` outcome becomes `Failed`. Pure — the closure
/// owns any sleeping/spawning, so tests drive it (and assert the attempt count)
/// without either.
fn retry_fetch(max_attempts: u32, mut attempt: impl FnMut(u32) -> Attempt) -> FetchOutcome {
    for i in 0..max_attempts {
        match attempt(i) {
            Attempt::Found(s) => return FetchOutcome::Found(s),
            Attempt::NotFound => return FetchOutcome::NotFound,
            Attempt::Retryable => continue,
        }
    }
    FetchOutcome::Failed
}

/// Real `gh` fetcher. `Command::output()` captures BOTH streams so a failure's
/// stderr can be classified. 12 concurrent `gh` subprocesses trip GitHub's
/// secondary rate limit (a 403 — issue #52), so a retryable failure backs off and
/// retries up to `max_attempts` before surfacing `Failed`.
struct GhCli {
    max_attempts: u32,
    backoff: std::time::Duration,
}

impl GhCli {
    fn new() -> Self {
        GhCli {
            max_attempts: 4,
            backoff: std::time::Duration::from_millis(500),
        }
    }
}

impl GhApi for GhCli {
    fn api_jq(&self, args: &[&str]) -> FetchOutcome {
        retry_fetch(self.max_attempts, |i| {
            if i > 0 {
                std::thread::sleep(self.backoff * i);
            }
            match Command::new("gh").args(args).output() {
                Ok(o) if o.status.success() => {
                    Attempt::Found(String::from_utf8_lossy(&o.stdout).into_owned())
                }
                Ok(o) => match classify_gh_failure(&String::from_utf8_lossy(&o.stderr)) {
                    GhFailure::NotFound => Attempt::NotFound,
                    GhFailure::Retryable => Attempt::Retryable,
                },
                // A spawn error (gh missing/OS pressure) is transient — retry it.
                Err(_) => Attempt::Retryable,
            }
        })
    }
}

/// Decode a `contents` API response's base64 body ("" on any failure — 404, non-file).
fn gh_file(org: &str, repo: &str, path: &str) -> String {
    let Some(raw) = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/contents/{path}"),
        "--jq",
        ".content",
    ]) else {
        return String::new();
    };
    let b64: String = raw.split_whitespace().collect(); // gh returns base64 with newlines
    use std::io::Write;
    // minimal base64 decode (std has none) — shell out to base64 for correctness parity with scan.sh
    let mut child = match Command::new("base64")
        .arg("-d")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(b64.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Keyless public Base RPC endpoints (each verified live, chainId 0x2105). A
/// burst of calls throttles any single one, so `curl_json` spreads load across
/// the whole set and falls through on failure rather than hammering one — the
/// single-RPC version is what left the ERC-165 probes rate-limited to `unknown`.
const BASE_RPCS: &[&str] = &[
    "https://mainnet.base.org",
    "https://base-rpc.publicnode.com",
    "https://base.drpc.org",
    "https://1rpc.io/base",
    "https://base-mainnet.public.blastapi.io",
];

/// Keyless public Ethereum mainnet endpoints (chainId 0x1). Ethereum carries
/// its own production deployment at addresses that differ from Base's, so a
/// read answered by the wrong chain's endpoint reports a live contract as not
/// deployed — a silent downgrade, not an error. Hence the chain travels with
/// the session rather than being implied by the caller.
/// Each verified live against the 0.1.1 beacon-set deployer: all four answer
/// `iReceiptBeacon()` with the same address the Solidity lib resolves.
/// `eth.llamarpc.com` (HTTP 521) and `eth.merkle.io` (429 under a burst) were
/// dropped rather than left in as endpoints that fail the whole chain's read.
const ETHEREUM_RPCS: &[&str] = &[
    "https://ethereum-rpc.publicnode.com",
    "https://eth.drpc.org",
    "https://1rpc.io/eth",
    "https://rpc.mevblocker.io",
];

/// The chains the scan reads production state from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chain {
    Base,
    Ethereum,
}

impl Chain {
    fn rpcs(self) -> &'static [&'static str] {
        match self {
            Chain::Base => BASE_RPCS,
            Chain::Ethereum => ETHEREUM_RPCS,
        }
    }

    /// The host reported alongside a verdict, so a reader can tell which chain
    /// (and which endpoint set) answered.
    fn rpc_host(self) -> &'static str {
        match self {
            Chain::Base => "mainnet.base.org",
            Chain::Ethereum => "ethereum-rpc.publicnode.com",
        }
    }
}

/// One entity's RPC context: which chain to ask, and where in that chain's set
/// to start asking.
#[derive(Clone, Copy)]
struct Session {
    chain: Chain,
    cursor: usize,
}

/// Rotates the endpoint each new session starts at, so different verdicts spread
/// across the RPC set instead of all hammering the first one.
static RPC_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// A per-entity starting endpoint. All the calls that make up ONE verdict — a
/// beacon's `owner()` + `implementation()`, a contract's two ERC-165 probes, the
/// Safe's `getOwners()` + `getThreshold()` — share a `session` so they hit the
/// SAME rpc and can't disagree with themselves (a flaky endpoint answering one
/// probe but not the other is what produced bogus "nonconformant" verdicts).
/// Each new session rotates the start, spreading load across the set.
fn rpc_session(chain: Chain) -> Session {
    Session {
        chain,
        cursor: RPC_CURSOR.fetch_add(1, Ordering::Relaxed),
    }
}

/// POST a JSON-RPC `payload` to Base, trying `BASE_RPCS` from `session` and
/// falling through on failure. Returns the first successful body. A revert is
/// HTTP 200 with an `error` body, so it returns from the first endpoint reached.
/// `None` only if every endpoint fails.
fn curl_json(session: Session, payload: &str) -> Option<Vec<u8>> {
    let rpcs = session.chain.rpcs();
    for i in 0..rpcs.len() {
        let rpc = rpcs[(session.cursor + i) % rpcs.len()];
        if let Ok(o) = Command::new("curl")
            .args([
                "-fsS",
                "-m",
                "25",
                "-X",
                "POST",
                rpc,
                "-H",
                "content-type: application/json",
                "-d",
                payload,
            ])
            .output()
        {
            if o.status.success() {
                return Some(o.stdout);
            }
        }
    }
    None
}

/// Build the `eth_call` JSON-RPC payload for `to` with `data` (0x-hex calldata).
fn eth_call_payload(to: &str, data: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{{"to":"{to}","data":"{data}"}},"latest"]}}"#
    )
}

/// A read-only `eth_call` on Base within `session` — the `result` hex, or `None`
/// on failure. The scan must survive an RPC hiccup, so callers degrade gracefully.
fn eth_call(session: Session, to: &str, data: &str) -> Option<String> {
    curl_json(session, &eth_call_payload(to, data)).and_then(|b| rpc::result_hex(&b))
}

/// `eth_getCode` for an address (within `session`) → the runtime bytecode hex
/// (`0x…`, or `0x` when there is no code), or `None` on failure.
fn eth_get_code(session: Session, address: &str) -> Option<String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"eth_getCode","params":["{address}","latest"]}}"#
    );
    curl_json(session, &payload).and_then(|b| rpc::result_hex(&b))
}

/// Whether `address` has deployed runtime code on Base (within `session`):
/// `Some(true)` if `eth_getCode` returns non-empty bytecode, `Some(false)` for
/// `0x` (an EOA / nothing there), `None` on RPC failure.
fn code_deployed(session: Session, address: &str) -> Option<bool> {
    eth_get_code(session, address).map(|code| {
        let hex = code.strip_prefix("0x").unwrap_or(&code);
        hex.chars().any(|c| c != '0')
    })
}

/// `supportsInterface(<id>)` within `session` → the classified bool. An on-chain
/// revert (the contract doesn't implement the function) is distinguished from an
/// RPC failure so ERC-165 absence stays stable rather than flickering to
/// `unknown` — reverts come back HTTP 200 with an `error` body, read by
/// `classify_bool`.
fn supports_interface(session: Session, address: &str, interface_id: [u8; 4]) -> rpc::CallClass {
    let data = rpc::supports_interface_calldata(interface_id);
    match curl_json(session, &eth_call_payload(address, &data)) {
        Some(body) => rpc::classify_bool(&body),
        None => rpc::CallClass::Unknown,
    }
}

fn fetch_inputs(org: &str, repo: &str) -> RepoInputs {
    // workflows: list, then concat every *.yml/*.yaml body
    let mut workflows = String::new();
    if let Some(names) = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/contents/.github/workflows"),
        "--jq",
        ".[].name",
    ]) {
        for name in names.lines() {
            let name = name.trim();
            if name.ends_with(".yml") || name.ends_with(".yaml") {
                workflows.push('\n');
                workflows.push_str(&gh_file(org, repo, &format!(".github/workflows/{name}")));
            }
        }
    }
    let foundry = gh_file(org, repo, "foundry.toml");

    // One soldeer registry lookup, only when a package name exists. It answers both
    // questions the scan has about the package: whether it is published at all (a
    // signal), and the newest revision that exists (the ceiling the graph judges a
    // dependant's pin against). Derived together from the one query so the two can
    // never disagree.
    let revision = foundry_package_name(&foundry).and_then(|pkg| soldeer_latest_revision(&pkg));
    let soldeer_published = revision.as_ref().map(|r| r.is_some());
    let soldeer_version = revision.flatten();

    RepoInputs {
        workflows,
        foundry,
        soldeer_published,
        soldeer_version,
    }
}

/// Read the audit skill's run stamp and return the whole-repo audit if present.
/// Prefers the append-only `.audit/runs.jsonl` (the last whole-repo line); falls
/// back to the single-object `.audit/last-run.json` for repos still on the earlier
/// format. `None` when the repo has never had a whole-repo audit (no stamp, empty,
/// or only a PR-/path-scoped one — see the `scope` gate in `audit`).
/// Open `audit`-labelled issue counts, keyed `org/repo` — the audit skill files
/// each finding as an issue labelled `audit`, so this is the outstanding-findings
/// backlog per repo, alongside WHEN the repo was last audited.
///
/// One search per ORG rather than a listing per repo (~40 calls → 3). The second
/// return is the set of orgs whose search actually succeeded: a repo in a
/// succeeded org with no hits genuinely has ZERO open findings, whereas a repo in
/// a FAILED org is unknown — reporting that as zero would claim a clean audit
/// backlog the scan never saw (the false-`never` trap of issue #52).
fn fetch_open_audit_issues(
    orgs: &[String],
) -> (
    std::collections::BTreeMap<String, usize>,
    std::collections::BTreeSet<String>,
) {
    let mut counts = std::collections::BTreeMap::new();
    let mut ok_orgs = std::collections::BTreeSet::new();
    for org in orgs {
        let Some(raw) = gh_stdout(&[
            "search",
            "issues",
            "--owner",
            org,
            "--label",
            "audit",
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "repository",
        ]) else {
            eprintln!("::warning::open audit issue search failed for {org}; counts left unknown");
            continue;
        };
        let Ok(serde_json::Value::Array(hits)) = serde_json::from_str::<serde_json::Value>(&raw)
        else {
            eprintln!("::warning::open audit issue search for {org} was not a JSON array");
            continue;
        };
        ok_orgs.insert(org.clone());
        for hit in hits {
            if let Some(name) = hit["repository"]["name"].as_str() {
                *counts.entry(format!("{org}/{name}")).or_insert(0usize) += 1;
            }
        }
    }
    (counts, ok_orgs)
}

/// The open-audit-issue count for one repo: `Some(n)` when its org was searched
/// successfully (absent ⇒ a genuine 0), `None` when that search failed.
fn open_audit_issues(
    counts: &std::collections::BTreeMap<String, usize>,
    ok_orgs: &std::collections::BTreeSet<String>,
    org: &str,
    repo: &str,
) -> Option<usize> {
    ok_orgs
        .contains(org)
        .then(|| counts.get(&format!("{org}/{repo}")).copied().unwrap_or(0))
}

/// Fetch a set of blobs from one repo, batching them into GraphQL queries.
///
/// A want absent from the returned map is unknown — the path did not exist at
/// that ref, the object was not a text blob, GitHub truncated it, or the query
/// itself failed. Callers charge unknowns to code, so no failure here can read
/// as "unchanged".
fn gh_blobs(
    org: &str,
    repo: &str,
    wants: &[blobs::Want],
) -> std::collections::BTreeMap<blobs::Want, String> {
    let mut out = std::collections::BTreeMap::new();
    for chunk in wants.chunks(blobs::BATCH) {
        let query = blobs::blob_query(org, repo, chunk);
        let Some(raw) = gh_stdout(&["api", "graphql", "-f", &format!("query={query}")]) else {
            eprintln!(
                "::warning::blob batch failed for {org}/{repo} ({} files); \
                 their drift is charged to code and reported unclassified",
                chunk.len()
            );
            continue;
        };
        let (fetched, errors) = blobs::parse_blob_response(&raw, chunk);
        // A rate-limited or unpermitted query answers HTTP 200 with an errors
        // array, so `gh` succeeds and the blobs simply go missing. Without this
        // the drift just reads unclassified with nothing saying why.
        for e in &errors {
            eprintln!("::warning::blob batch for {org}/{repo} returned a GraphQL error: {e}");
        }
        out.extend(fetched);
    }
    out
}

/// Fetch both versions of every changed non-test Solidity file in a compare, so
/// the drift can be split into code vs comment by lexing.
///
/// A file that the compare says was ADDED has no base version, and one that was
/// REMOVED has no head version; those are passed as an empty string rather than
/// `None`, because absence there is a fact, not a failed lookup. Only a genuine
/// fetch failure yields `None`, which the split charges to code.
fn fetch_source_versions(
    org: &str,
    repo: &str,
    base: &str,
    head: &str,
    files: &[(String, String, u64, u64)],
) -> Vec<protofire::SourceVersions> {
    let sources: Vec<_> = files
        .iter()
        .filter(|(path, _, _, _)| protofire::counts_as_source_drift(path))
        .collect();

    // One want per version we actually have to read, so an added file costs no
    // base lookup and a removed file costs no head lookup.
    let mut wants: Vec<blobs::Want> = Vec::new();
    for (path, status, _, _) in &sources {
        if *status != "added" {
            wants.push((base.to_string(), path.clone()));
        }
        if *status != "removed" {
            wants.push((head.to_string(), path.clone()));
        }
    }
    wants.sort();
    wants.dedup();
    let fetched = gh_blobs(org, repo, &wants);

    sources
        .into_iter()
        .map(
            |(path, status, additions, deletions)| protofire::SourceVersions {
                path: path.clone(),
                base: if status == "added" {
                    Some(String::new())
                } else {
                    fetched.get(&(base.to_string(), path.clone())).cloned()
                },
                head: if status == "removed" {
                    Some(String::new())
                } else {
                    fetched.get(&(head.to_string(), path.clone())).cloned()
                },
                additions: *additions,
                deletions: *deletions,
            },
        )
        .collect()
}

/// The newest adversarial-mutation run recorded for a repo, from the skill's
/// `audit/mutation-test-scans.json`. Absent/malformed ⇒ `None` (never run, as far
/// as this scan can tell) rather than a fabricated entry.
fn fetch_last_mutation(org: &str, repo: &str) -> Option<mutation::LastMutation> {
    let src = gh_file(org, repo, "audit/mutation-test-scans.json");
    if src.trim().is_empty() {
        return None;
    }
    mutation::parse_mutation_scans(&src)
}

fn fetch_last_audit(org: &str, repo: &str) -> Option<LastAudit> {
    // Parse with no HEAD first: a scoped/malformed/absent stamp returns None here,
    // so the extra `commits/HEAD` API call is skipped in those cases (org scale).
    let runs = gh_file(org, repo, ".audit/runs.jsonl");
    let mut audit = if runs.trim().is_empty() {
        None
    } else {
        parse_runs_jsonl(&runs, None)
    };
    // Transition fallback: only if runs.jsonl yielded nothing, try the earlier
    // single-object stamp. (Removable once every audited repo is on runs.jsonl.)
    if audit.is_none() {
        let legacy = gh_file(org, repo, ".audit/last-run.json");
        if !legacy.trim().is_empty() {
            audit = parse_last_audit(&legacy, None);
        }
    }
    let mut audit = audit?;
    // Confirmed a whole-repo stamp — now resolve HEAD to flag staleness. Staleness
    // is whether first-party SOURCE changed since `auditedCommit`, EXCLUDING
    // `.audit/`: the run's own stamp commit advances HEAD while touching only
    // `.audit/`, so a bare `HEAD != auditedCommit` check would mark every fresh
    // audit stale. Compare the trees and ignore `.audit/`-only changes.
    let head = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/commits/HEAD"),
        "--jq",
        ".sha",
    ]);
    audit.stale = match head.as_deref().map(str::trim) {
        None => None,
        Some(h) if h == audit.audited_commit => Some(false),
        // Classify each changed file by lexing both versions: a NatSpec-only edit
        // must not mark a fresh audit stale, which a filename-only check cannot
        // tell apart from a real code change.
        Some(h) => gh_stdout(&[
            "api",
            &format!("repos/{org}/{repo}/compare/{}...{h}", audit.audited_commit),
        ])
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            let (listed, compare_truncated) = audit::parse_compare_files(&v);
            // Non-source paths keep the old filename-only meaning (any change
            // outside `.audit/` counts); Solidity gets the comment-aware verdict.
            let versions = fetch_source_versions(org, repo, &audit.audited_commit, h, &listed);
            let classified: Vec<(String, Option<commentloc::LineDrift>)> = listed
                .iter()
                .map(|(path, _, _, _)| {
                    let drift = versions.iter().find(|v| &v.path == path).and_then(|v| {
                        match (&v.base, &v.head) {
                            (Some(b), Some(hd)) => commentloc::file_drift(b, hd),
                            _ => None,
                        }
                    });
                    (path.clone(), drift)
                })
                .collect();
            audit::stale_verdict(compare_truncated, &classified)
        }),
    };
    Some(audit)
}

/// One repo's assembled EXTERNAL (Protofire) audit situation. See `protofire.rs`
/// for the pure logic; this is the orchestrated result folded into `health.json`.
struct ProtofireResult {
    has_pdf: bool,
    external_audit: protofire::ExternalAudit,
    pdfs: Vec<AuditPdf>,
    audited_ref: Option<String>,
    anchor_kind: Option<&'static str>,
    tag_convention_absent: bool,
    audited_date: String,
    latest_tag: Option<String>,
    latest_tag_iso: Option<String>,
    /// `None` when the scan could establish neither drift nor tag recency.
    is_stale: Option<bool>,
    source_loc: Option<u64>,
    source_loc_added: Option<u64>,
    source_loc_removed: Option<u64>,
    files_changed: Option<u64>,
    commits_since: Option<u64>,
    source_drift_truncated: bool,
    /// Comment-vs-code split of the source drift. `None` when the patches were
    /// unavailable (truncated/omitted), in which case the drift is unclassified.
    comment_loc_added: Option<u64>,
    comment_loc_removed: Option<u64>,
    code_loc_added: Option<u64>,
    code_loc_removed: Option<u64>,
    /// False when some churn could not be classified and was charged to code.
    drift_fully_classified: bool,
    compare_url: Option<String>,
    /// Index into `pdfs` of the ONE PDF the anchor/drift are computed against (the
    /// newest under audit/protofire/); the panel shows only this one, not all (#62).
    reference_pdf_index: Option<usize>,
}

/// Outcome of listing a repo directory via the contents API: the parsed
/// `(type, path, name)` rows, a genuine 404 (`NotFound`), or a fetch failure
/// (`Failed`). `Failed` must NOT be read as an empty directory.
enum ContentsListing {
    Found(Vec<(String, String, String)>),
    NotFound,
    Failed,
}

/// List a repo directory via the contents API → typed listing. A 404 maps to
/// `NotFound` (the directory doesn't exist); a rate-limit/network error maps to
/// `Failed` so the caller can tell "genuinely absent" from "couldn't fetch".
fn gh_contents_entries<F: GhApi>(gh: &F, org: &str, repo: &str, path: &str) -> ContentsListing {
    let out = match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/contents/{path}"),
        "--jq",
        ".[]|[.type,.path,.name]|@tsv",
    ]) {
        FetchOutcome::Found(s) => s,
        FetchOutcome::NotFound => return ContentsListing::NotFound,
        FetchOutcome::Failed => return ContentsListing::Failed,
    };
    ContentsListing::Found(
        out.lines()
            .filter_map(|l| {
                let mut it = l.split('\t');
                Some((
                    it.next()?.to_string(),
                    it.next()?.to_string(),
                    it.next()?.to_string(),
                ))
            })
            .collect(),
    )
}

/// Walk the `audit/protofire/` dir (contents API, bounded depth) collecting every
/// `.pdf` blob as (filename, path). Formal Protofire audits sit flat directly in
/// `audit/protofire/`; the shallow depth cap tolerates an accidental extra level
/// while keeping a pathological tree from fanning out. Scoping to this dir
/// (rather than all of `audit/`) is deliberate: a non-Protofire report elsewhere
/// under `audit/` must NOT be counted as a Protofire audit. Unlike the whole-repo
/// trees API, the contents API never silently truncates on large repos.
///
/// Returns `true` iff listing THIS `dir` FAILED (a fetch error — as opposed to a
/// 404 / genuinely-empty dir). A failure while recursing into a SUBDIR is
/// tolerated (it does not set the flag): only the caller's inspection of the
/// top-level `audit/protofire` return decides the `unknown` state, so a flaky
/// nested listing can't poison a repo that has PDFs sitting at the top level.
fn collect_audit_pdfs<F: GhApi>(
    gh: &F,
    org: &str,
    repo: &str,
    dir: &str,
    depth: u8,
    acc: &mut Vec<(String, String)>,
) -> bool {
    if depth == 0 {
        return false;
    }
    let entries = match gh_contents_entries(gh, org, repo, dir) {
        ContentsListing::Found(entries) => entries,
        ContentsListing::NotFound => return false,
        ContentsListing::Failed => return true,
    };
    for (ty, path, name) in entries {
        if ty == "file" {
            if name.to_ascii_lowercase().ends_with(".pdf") {
                acc.push((name, path));
            }
        } else if ty == "dir" {
            // Tolerate a subdir listing failure — it must not flip the whole repo
            // to `unknown` when the top-level listing succeeded.
            let _ = collect_audit_pdfs(gh, org, repo, &path, depth - 1, acc);
        }
    }
    false
}

/// Does `sha` name a real commit in the repo? The resolution half of anchor
/// classification: `gh api repos/{o}/{r}/commits/{sha}` echoes the commit's SHA on
/// success and 404s on an unknown ref. It runs through the shared `GhApi` seam so it
/// shares the retry/backoff of the rest of the audit path; a hex-looking filename
/// token that isn't a real commit (`NotFound`) — or a fetch that `Failed` after
/// retries — falls back to unanchored rather than erroring.
fn commit_exists<F: GhApi>(gh: &F, org: &str, repo: &str, sha: &str) -> bool {
    match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/commits/{sha}"),
        "--jq",
        ".sha",
    ]) {
        FetchOutcome::Found(s) => !s.trim().is_empty(),
        FetchOutcome::NotFound | FetchOutcome::Failed => false,
    }
}

/// Committer date (ISO-8601 UTC) of a git ref — a commit SHA **or** a tag name —
/// via `gh api repos/{o}/{r}/commits/{ref}` (the endpoint resolves a tag ref to
/// its commit). This is the AUDITED state's real date, used to order one audit
/// PDF against another (see `AuditPdf::audit_date_iso`). Runs through the shared
/// `GhApi` seam so it inherits the audit path's retry/backoff. `None` when the ref
/// does not resolve (`NotFound`) or the fetch `Failed` after retries — the caller
/// then falls back to the PDF's own file-commit date.
fn commit_date<F: GhApi>(gh: &F, org: &str, repo: &str, git_ref: &str) -> Option<String> {
    match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/commits/{git_ref}"),
        "--jq",
        ".commit.committer.date",
    ]) {
        FetchOutcome::Found(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        FetchOutcome::Found(_) | FetchOutcome::NotFound | FetchOutcome::Failed => None,
    }
}

/// Commit date + sha of the commit that last touched (added) a path.
fn pdf_commit<F: GhApi>(gh: &F, org: &str, repo: &str, path: &str) -> (String, String) {
    let out = match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/commits?path={path}&per_page=1"),
        "--jq",
        "[.[0].commit.committer.date // \"\", .[0].sha // \"\"]|@tsv",
    ]) {
        FetchOutcome::Found(s) => s,
        FetchOutcome::NotFound | FetchOutcome::Failed => String::new(),
    };
    let mut it = out.trim_end_matches('\n').split('\t');
    (
        it.next().unwrap_or("").to_string(),
        it.next().unwrap_or("").to_string(),
    )
}

/// One GraphQL call → (default_branch, latest_tag, latest_tag_iso). "Latest" is
/// the tag whose commit date is newest (`TAG_COMMIT_DATE` DESC): the REST tags
/// list is NOT date-ordered, and resolving every tag's date would be O(tags) REST
/// calls (some repos carry 100+ tags). Empty tag fields ⇒ the repo has no tags.
fn repo_default_and_latest_tag<F: GhApi>(
    gh: &F,
    org: &str,
    repo: &str,
) -> (String, Option<String>, Option<String>) {
    const Q: &str = "query($o:String!,$n:String!){repository(owner:$o,name:$n){defaultBranchRef{name} refs(refPrefix:\"refs/tags/\",orderBy:{field:TAG_COMMIT_DATE,direction:DESC},first:1){nodes{name target{__typename ... on Commit{committedDate} ... on Tag{target{... on Commit{committedDate}}}}}}}}";
    const JQ: &str = ".data.repository | [(.defaultBranchRef.name // \"\"), (.refs.nodes[0].name // \"\"), (.refs.nodes[0].target.committedDate // .refs.nodes[0].target.target.committedDate // \"\")] | @tsv";
    let out = match gh.api_jq(&[
        "api",
        "graphql",
        "-f",
        &format!("query={Q}"),
        "-f",
        &format!("o={org}"),
        "-f",
        &format!("n={repo}"),
        "--jq",
        JQ,
    ]) {
        FetchOutcome::Found(s) => s,
        FetchOutcome::NotFound | FetchOutcome::Failed => String::new(),
    };
    let mut it = out.trim_end_matches('\n').split('\t');
    let branch = it.next().unwrap_or("").to_string();
    let tag = it.next().unwrap_or("").to_string();
    let iso = it.next().unwrap_or("").to_string();
    let branch = if branch.is_empty() {
        "main".to_string()
    } else {
        branch
    };
    let (tag, iso) = if tag.is_empty() {
        (None, None)
    } else {
        (Some(tag), (!iso.is_empty()).then_some(iso))
    };
    (branch, tag, iso)
}

/// `compare/{base}...{head}` → (base commit date, changed files, total commits,
/// files_truncated). Clone-free drift: GitHub returns per-file additions/deletions.
/// The files list is a single page (GitHub caps it at 300); `truncated` flags the
/// rare raindex-scale diff where the source-LOC total becomes a lower bound.
fn fetch_compare<F: GhApi>(
    gh: &F,
    org: &str,
    repo: &str,
    base: &str,
    head: &str,
) -> Option<(String, Vec<CompareFile>, u64, bool)> {
    let raw = match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/compare/{base}...{head}"),
    ]) {
        FetchOutcome::Found(s) => s,
        FetchOutcome::NotFound | FetchOutcome::Failed => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let base_date = v["base_commit"]["commit"]["committer"]["date"]
        .as_str()?
        .to_string();
    let total = v["total_commits"].as_u64().unwrap_or(0);
    let files: Vec<CompareFile> = v["files"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| CompareFile {
                    filename: f["filename"].as_str().unwrap_or("").to_string(),
                    additions: f["additions"].as_u64().unwrap_or(0),
                    deletions: f["deletions"].as_u64().unwrap_or(0),
                    status: f["status"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let truncated = files.len() >= audit::COMPARE_FILE_CAP;
    Some((base_date, files, total, truncated))
}

/// Every `(path, blob_sha)` blob in a repo's recursive git tree at `sha`. `None`
/// on a failed/absent fetch OR when GitHub truncated the tree (`.truncated`, the
/// 100k-entry cap): a truncated tree can't yield a reliable diff, so the caller
/// must treat drift as indeterminate rather than silently undercount.
fn fetch_tree_blobs<F: GhApi>(
    gh: &F,
    org: &str,
    repo: &str,
    sha: &str,
) -> Option<Vec<(String, String)>> {
    let raw = match gh.api_jq(&[
        "api",
        &format!("repos/{org}/{repo}/git/trees/{sha}?recursive=1"),
    ]) {
        FetchOutcome::Found(s) => s,
        FetchOutcome::NotFound | FetchOutcome::Failed => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if v["truncated"].as_bool().unwrap_or(false) {
        return None;
    }
    let blobs = v["tree"]
        .as_array()?
        .iter()
        .filter(|e| e["type"].as_str() == Some("blob"))
        .filter_map(|e| {
            Some((
                e["path"].as_str()?.to_string(),
                e["sha"].as_str()?.to_string(),
            ))
        })
        .collect();
    Some(blobs)
}

/// Accurate count of non-test `.sol` files changed `base…head` via two recursive
/// tree reads (the blob-sha diff, not the 300-file-capped compare). `None` when
/// either tree is unavailable or truncated — the caller then reports drift as
/// indeterminate rather than a false zero.
fn changed_sol_count<F: GhApi>(
    gh: &F,
    org: &str,
    repo: &str,
    base: &str,
    head: &str,
) -> Option<u64> {
    let base_tree = fetch_tree_blobs(gh, org, repo, base)?;
    let head_tree = fetch_tree_blobs(gh, org, repo, head)?;
    Some(changed_source_file_count(&base_tree, &head_tree))
}

/// Sum non-test Solidity LOC under a directory tree (skips `.git`). The pure
/// counting half of `count_source_loc`, so it is testable against a fixture dir
/// without cloning. Uses the SAME non-test-`.sol` predicate as the drift columns.
fn sum_sol_loc(root: &std::path::Path) -> u64 {
    fn walk(dir: &std::path::Path, root: &std::path::Path, acc: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // file_type() does NOT follow symlinks (unlike Path::is_dir); skip links
            // so a symlink cycle can't overflow the stack and a link can't read or
            // traverse outside the clone.
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some(".git") {
                    walk(&path, root, acc);
                }
            } else if ft.is_file() {
                if let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) {
                    if counts_as_source_drift(rel) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            *acc += content.lines().count() as u64;
                        }
                    }
                }
            }
        }
    }
    let mut loc = 0;
    walk(root, root, &mut loc);
    loc
}

/// Total non-test Solidity LOC at HEAD, counted from a shallow clone. #37 wants an
/// accurate line count, not the tree API's byte size, so we shallow-clone the repo
/// and count lines in every non-test `.sol` file. `None` on a failed clone
/// (private/missing/network) — the repo's LOC is then unknown, not zero. Unlike the
/// rest of the scan this is NOT clone-free; it is bounded to the repos that need a
/// full-source count (never-audited Solidity projects).
fn count_source_loc(org: &str, repo: &str) -> Option<u64> {
    let dir = std::env::temp_dir().join(format!(
        "rohloc-{}-{}",
        repo.replace(['/', '.'], "-"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let url = format!("https://github.com/{org}/{repo}");
    // Bound the clone via `timeout` so one stalled network connection can't block a
    // scan worker indefinitely (a non-zero exit — including the 124 timeout — is
    // treated as a failed clone, i.e. unknown LOC).
    let ok = Command::new("timeout")
        .args([
            "120",
            "git",
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--no-tags",
            "--quiet",
            &url,
        ])
        .arg(&dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let loc = ok.then(|| sum_sol_loc(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    loc
}

/// A `ProtofireResult` for a repo with no usable PDF, carrying only the coverage
/// `state`. `never` (genuinely absent) and `unknown` (fetch failed) share this
/// shape; the caller decides which by whether the top-level listing FAILED.
fn empty_protofire(state: protofire::ExternalAudit) -> ProtofireResult {
    ProtofireResult {
        has_pdf: false,
        external_audit: state,
        pdfs: Vec::new(),
        reference_pdf_index: None,
        audited_ref: None,
        anchor_kind: None,
        tag_convention_absent: false,
        audited_date: String::new(),
        latest_tag: None,
        latest_tag_iso: None,
        is_stale: None,
        comment_loc_added: None,
        comment_loc_removed: None,
        code_loc_added: None,
        code_loc_removed: None,
        drift_fully_classified: false,
        source_loc: None,
        source_loc_added: None,
        source_loc_removed: None,
        files_changed: None,
        commits_since: None,
        source_drift_truncated: false,
        compare_url: None,
    }
}

/// Assemble a repo's EXTERNAL (Protofire) audit situation: enumerate
/// `audit/protofire/` PDFs, pick the newest as the reference, parse its tag (or
/// fall back to its commit), find the newest tag, and quantify source-LOC drift
/// base…HEAD — all clone-free. A genuinely-empty listing ⇒ `never` (the coverage
/// gap); a FAILED listing ⇒ `unknown` (never a false `never` — issue #52).
fn fetch_protofire_audit<F: GhApi>(gh: &F, org: &str, repo: &str) -> ProtofireResult {
    let mut pdf_paths: Vec<(String, String)> = Vec::new();
    let failed = collect_audit_pdfs(gh, org, repo, "audit/protofire", 2, &mut pdf_paths);
    if pdf_paths.is_empty() {
        // A failed top-level listing is coverage-indeterminate (`unknown`), NOT a
        // confirmed gap (`never`). No fetch error may become a coverage claim.
        return empty_protofire(if failed {
            protofire::ExternalAudit::Unknown
        } else {
            protofire::ExternalAudit::Never
        });
    }
    // Resolve each PDF's commit date + sha, then order by path for stable output.
    let mut pdfs: Vec<AuditPdf> = pdf_paths
        .into_iter()
        .map(|(filename, path)| {
            let (iso, sha) = pdf_commit(gh, org, repo, &path);
            // Recency by the AUDITED commit/tag date (the audit's real date),
            // resolved from the filename's anchor ref. Falls back to the PDF's own
            // file-commit date when the name encodes no resolvable anchor — so a
            // bulk move that ties every PDF's file-commit date still orders them by
            // which audit is newer (see `newest_pdf_index`).
            let audit_date_iso = anchor_ref(&filename)
                .and_then(|r| commit_date(gh, org, repo, &r))
                .unwrap_or_else(|| iso.clone());
            AuditPdf {
                filename,
                path,
                last_commit_iso: iso,
                commit_sha: sha,
                audit_date_iso,
            }
        })
        .collect();
    pdfs.sort_by(|a, b| a.path.cmp(&b.path));
    let newest = newest_pdf_index(&pdfs).expect("non-empty");
    // Index of the one PDF the anchor/drift reference (the newest); panel shows only it (#62).
    let reference_pdf_index = Some(newest);

    // Classify the audited anchor the newest PDF's filename encodes: a `vX.Y.Z`
    // tag, a hex commit token that RESOLVES to a real commit, or neither. The
    // resolver is the I/O half — `commit_exists` guards a hex-looking token that
    // isn't actually a commit, so it falls back to unanchored (not an error).
    let anchor = classify_anchor(&pdfs[newest].filename, |sha| {
        commit_exists(gh, org, repo, sha)
    });
    let audited_ref = anchor.drift_base_ref().map(str::to_string);
    let anchor_kind = Some(anchor.kind());
    // Kept for the panel/consumers: true whenever the filename encodes no vX.Y.Z
    // tag — a commit-anchored PDF is still "tag convention absent" (no tag), it is
    // just anchored to a commit instead.
    let tag_convention_absent = !matches!(anchor, AuditAnchor::Tag(_));

    let (default_branch, latest_tag, latest_tag_iso) = repo_default_and_latest_tag(gh, org, repo);

    // Drift base: the audited anchor (tag or resolved commit) when the filename
    // encodes one, else the newest PDF's own commit (the unanchored fallback).
    let base = audited_ref
        .clone()
        .unwrap_or_else(|| pdfs[newest].commit_sha.clone());
    let cmp = if base.is_empty() {
        None
    } else {
        fetch_compare(gh, org, repo, &base, &default_branch)
    };

    let (
        audited_date,
        source_loc,
        source_loc_added,
        source_loc_removed,
        files_changed,
        commits_since,
        source_drift_truncated,
        // Comment-vs-code split of the same drift, when the patches were available.
        line_split,
    ) = match &cmp {
        // Compare truncated at GitHub's 300-file cap: its per-file +/− misses any
        // `.sol` sorted beyond the cap (a false zero for large repos like raindex).
        // Fall back to a tree blob-sha diff for an ACCURATE changed-`.sol` FILE
        // count; line-level drift isn't recoverable from trees, so leave +/− unknown.
        Some((base_date, _files, total, trunc)) if *trunc => (
            base_date.clone(),
            None,
            None,
            None,
            changed_sol_count(gh, org, repo, &base, &default_branch),
            Some(*total),
            true,
            None,
        ),
        Some((base_date, files, total, _)) => {
            let (added, removed, n) = source_drift(files);
            // Lexing both versions of each changed .sol is what makes the
            // comment/code split possible; a diff hunk cannot answer it.
            let versions = fetch_source_versions(
                org,
                repo,
                &base,
                &default_branch,
                &files
                    .iter()
                    .map(|f| {
                        (
                            f.filename.clone(),
                            f.status.clone(),
                            f.additions,
                            f.deletions,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            let split = protofire::source_drift_split(&versions);
            // Keep +/− separate; the combined `source_loc` is derived as the sum for
            // sorting, the staleness check, and JSON back-compat.
            (
                base_date.clone(),
                Some(added + removed),
                Some(added),
                Some(removed),
                Some(n),
                Some(*total),
                false,
                Some(split),
            )
        }
        // Compare unavailable → date the audit by the PDF's own commit, drift unknown.
        None => (
            pdfs[newest].last_commit_iso.clone(),
            None,
            None,
            None,
            None,
            None,
            false,
            None,
        ),
    };

    let has_tags = latest_tag.is_some();
    // None when the latest tag's date is unknown. A failed tag lookup is not
    // evidence that no newer tag exists, so it must not collapse to `false` —
    // paired with unmeasurable drift that is what reported a broken scan clean.
    // None when the tag date is unreadable; see protofire::newer_tag_exists.
    let newer_tag_exists = protofire::newer_tag_exists(latest_tag_iso.as_deref(), &audited_date);
    // Did the audited Solidity actually change? Prefer the line drift; when a
    // truncated compare leaves lines unknown, the tree-derived changed-file count
    // still answers it. Both unknown -> None, and is_stale falls back to tag
    // recency rather than treating unmeasured as zero.
    // Comment and whitespace churn is not source change: an audit covers the
    // contracts that are there, and rewording NatSpec changes none of them. So
    // prefer the CODE-only verdict; fall back to total LOC, then changed-file
    // count, only where the lines could not be classified (truncated/absent
    // patch) — unmeasured must never read as unchanged.
    let source_changed = line_split
        .as_ref()
        .map(|(d, _)| d.code_changed())
        .or(source_loc.map(|drift| drift > 0))
        .or(files_changed.map(|files| files > 0));
    let external_audit = classify_external_audit(true, has_tags, newer_tag_exists, source_changed);
    let stale = is_stale(newer_tag_exists, source_changed);

    // The GitHub compare-view URL for the audited drift (base…default branch);
    // None when either ref is empty, so the panel links only when it resolves.
    let compare_url = protofire::compare_url(org, repo, &base, &default_branch);

    ProtofireResult {
        has_pdf: true,
        external_audit,
        pdfs,
        reference_pdf_index,
        audited_ref,
        anchor_kind,
        tag_convention_absent,
        audited_date,
        latest_tag,
        latest_tag_iso,
        is_stale: stale,
        source_loc,
        source_loc_added,
        source_loc_removed,
        files_changed,
        commits_since,
        source_drift_truncated,
        comment_loc_added: line_split.as_ref().map(|(d, _)| d.comment_added),
        comment_loc_removed: line_split.as_ref().map(|(d, _)| d.comment_removed),
        code_loc_added: line_split.as_ref().map(|(d, _)| d.code_added),
        code_loc_removed: line_split.as_ref().map(|(d, _)| d.code_removed),
        drift_fully_classified: line_split.as_ref().map(|(_, ok)| *ok).unwrap_or(false),
        compare_url,
    }
}

/// One repo's scan result: modernization signals, last whole-repo audit (if any),
/// and the external (Protofire) audit situation.
struct RepoResult {
    name: String,
    /// The GitHub org this repo belongs to — carried per-repo so a multi-org scan
    /// builds correct links and can attribute each node to its org.
    org: String,
    signals: Vec<&'static str>,
    last_audit: Option<LastAudit>,
    last_mutation: Option<mutation::LastMutation>,
    protofire: ProtofireResult,
    has_foundry: bool,
    /// FULL non-test `.sol` LOC (#37), counted for never-audited Solidity repos;
    /// `None` for audited/non-Solidity repos (audited repos use drift instead).
    full_source_loc: Option<u64>,
    /// This repo's soldeer `[package].name` — what consumers name it by, so it
    /// is the audit graph's join key (#71).
    package: Option<String>,
    /// The newest revision of this repo's package published to the soldeer
    /// registry — the newest version a consumer can pin, and so what a dependant's
    /// pin is judged stale against (#79). `None` when unpublished or unknown, which
    /// flags nobody: there is no ceiling to be behind.
    version: Option<String>,
    /// This repo's `[dependencies]`, each with its pinned version (#71, #79).
    deps: Vec<graph::Dep>,
    /// False when `foundry.toml` would not parse: deps unknown, not absent.
    deps_known: bool,
}

/// Whether a repo belongs in the Protofire external-audit report. A Protofire
/// audit is a Solidity audit, so the report only concerns Foundry/Solidity
/// projects (proxied by a `foundry.toml`) plus anything already carrying a PDF; a
/// repo with neither (docs, subgraph, tooling, `.github`) is not a coverage gap
/// and must not inflate the never-audited count (issue #54).
fn in_protofire_report(has_pdf: bool, has_foundry: bool) -> bool {
    has_pdf || has_foundry
}

/// The newest revision the soldeer registry holds for `pkg`.
///
/// `Some(Some(v))` — published, `v` is the newest revision.
/// `Some(None)` — the registry answered and the package has no revisions.
/// `None` — the query failed. Unknown, which is NOT the same as unpublished, and
/// the two must not collapse: an unpublished package is a real finding, a failed
/// lookup is an absence of information.
fn soldeer_latest_revision(pkg: &str) -> Option<Option<String>> {
    // `limit=1` returns the newest revision, which is the only one the scan needs:
    // the newest version a consumer could pin.
    let url =
        format!("https://api.soldeer.xyz/api/v1/revision?project_name={pkg}&offset=0&limit=1");
    let out = Command::new("curl").args(["-fsSL", &url]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    latest_revision_from_response(&out.stdout)
}

/// The newest revision in a soldeer `/revision` response body, split out from the
/// fetch so the parse is a pure function of the bytes and testable without network.
///
/// Same tri-state as its caller: `Some(Some(v))` published, `Some(None)` answered
/// with no revisions, `None` unreadable — an unreadable answer is not an empty one.
fn latest_revision_from_response(body: &[u8]) -> Option<Option<String>> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let data = v.get("data")?.as_array()?;
    Some(
        data.first()
            .and_then(|rev| rev.get("version"))
            .and_then(|version| version.as_str())
            .map(str::to_string),
    )
}

/// Resolve where the dashboard JSON is written. A bare run POPULATES `site/health.json` (the scan
/// never print-and-discards by default); `JSON_OUT` overrides the default; `--json <path>` overrides both.
fn resolve_json_out(json_out_env: Option<String>, json_flag: Option<String>) -> String {
    json_flag
        .or(json_out_env)
        .unwrap_or_else(|| "site/health.json".into())
}

/// Fan `work` out over `repos` across up to `par` worker threads (work-stealing
/// via a shared cursor) and return a result for EVERY repo. Each result is
/// produced by `work` and carries its own repo identity, so completeness and the
/// result SET are independent of `par` — `par=1` and `par=N` yield the same set.
fn scan_repos<I, T, F>(items: Vec<I>, par: usize, work: F) -> Vec<T>
where
    I: Send + Sync,
    F: Fn(&I) -> T + Sync,
    T: Send,
{
    let total = items.len();
    if total == 0 {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<T>> = Mutex::new(Vec::new());
    let nworkers = par.clamp(1, total);
    std::thread::scope(|s| {
        for _ in 0..nworkers {
            s.spawn(|| loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= items.len() {
                    break;
                }
                let out = work(&items[idx]);
                results.lock().unwrap().push(out);
            });
        }
    });
    results.into_inner().unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut json_flag: Option<String> = None;
    let mut repos_arg: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json_flag = args.get(i + 1).cloned();
                i += 2;
            }
            r => {
                repos_arg.push(r.to_string());
                i += 1;
            }
        }
    }
    // POPULATE by default: a bare run writes site/health.json (never print-and-discard);
    // JSON_OUT overrides the default; --json <path> overrides both.
    let json_out = resolve_json_out(std::env::var("JSON_OUT").ok(), json_flag);
    // Multi-org: ORGS is a space-separated list; ORG stays as the single-org
    // fallback. All orgs merge into one scan so the graph joins cross-org edges
    // by package name — a consumer in one org standing on a dependency published
    // in another shows up as inherited ground (#79-followup: S01-Issuer/cyclo).
    let orgs: Vec<String> = std::env::var("ORGS")
        .ok()
        .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![std::env::var("ORG").unwrap_or_else(|_| "rainlanguage".into())]);
    let par: usize = std::env::var("PAR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);

    // A partial scan cannot produce a trustworthy graph: deps resolve to repos
    // across the SCANNED set, so an unscanned dependency is indistinguishable
    // from a third-party package and gets dropped — the graph would then report
    // falsely clear ground. Captured before `repos_arg` is consumed.
    let partial_scan = !repos_arg.is_empty();

    // Each item is (org, repo): the org is carried per-repo so cross-org scans
    // build the right GitHub links and headers, and repo-name collisions across
    // orgs (e.g. DefiLlama forks) stay distinct.
    let repo_pairs: Vec<(String, String)> = if !repos_arg.is_empty() {
        let o = orgs[0].clone();
        repos_arg.into_iter().map(|r| (o.clone(), r)).collect()
    } else {
        let mut v: Vec<(String, String)> = Vec::new();
        for org in &orgs {
            let names = gh_stdout(&[
                "repo",
                "list",
                org,
                "--no-archived",
                "--limit",
                "300",
                "--json",
                "name,isFork",
                "-q",
                ".[]|select(.isFork==false)|.name",
            ])
            .unwrap_or_default();
            for name in names.lines() {
                v.push((org.clone(), name.to_string()));
            }
        }
        v.sort();
        v
    };
    let total = repo_pairs.len();
    eprintln!(
        "Scanning {total} repos across {} org(s): {} (parallel={par})...",
        orgs.len(),
        orgs.join(", ")
    );

    // One shared `gh` fetcher borrowed by every worker (`GhApi: Sync`). It retries
    // the secondary-rate-limit failure that `par` concurrent subprocesses provoke,
    // so a transient error surfaces as `unknown`, never a false `never`.
    let gh = GhCli::new();
    let mut results: Vec<RepoResult> = scan_repos(repo_pairs, par, |(org, repo)| {
        let repo = repo.as_str();
        let inputs = fetch_inputs(org, repo);
        // A repo counts toward the Protofire (Solidity-audit) report only if it is a
        // Foundry/Solidity project — proxied by a foundry.toml, which fetch_inputs
        // already retrieves, so this gate adds no extra request (#54).
        let has_foundry = !inputs.foundry.trim().is_empty();
        let signals = detect_signals(&inputs);
        let last_audit = fetch_last_audit(org, repo);
        let last_mutation = fetch_last_mutation(org, repo);
        let protofire = fetch_protofire_audit(&gh, org, repo);
        // #37: a never-audited Solidity repo's FULL non-test .sol LOC (via a shallow
        // clone) quantifies the coverage gap; audited repos use their drift instead.
        // Only CONFIRMED never-audited repos get a LOC count. `!has_pdf` also matches
        // `unknown` (the audit fetch FAILED), and an errored lookup must not be
        // presented as a confirmed coverage gap with a source-LOC magnitude (cf #52).
        let full_source_loc =
            if has_foundry && protofire.external_audit == protofire::ExternalAudit::Never {
                count_source_loc(org, repo)
            } else {
                None
            };
        RepoResult {
            name: repo.to_string(),
            org: org.clone(),
            package: foundry_package_name(&inputs.foundry),
            // The published revision, NOT `[package].version` from HEAD: that field
            // is the next, unreleased version under the org's release lifecycle, so
            // judging pins against it marks every consumer stale for not pinning a
            // version that does not exist (#86).
            version: inputs.soldeer_version.clone(),
            // A manifest that will not parse leaves deps UNKNOWN — not none.
            // Collapsing the two lets a broken manifest read as clear ground.
            deps: graph::foundry_dependencies(&inputs.foundry).unwrap_or_default(),
            deps_known: graph::foundry_dependencies(&inputs.foundry).is_ok(),
            signals,
            last_audit,
            last_mutation,
            protofire,
            has_foundry,
            full_source_loc,
        }
    });
    // findings view (owned) so we can re-sort `results` for audit recency afterwards
    let mut findings: Vec<(String, String, Vec<&'static str>)> = results
        .iter()
        .filter(|r| !r.signals.is_empty())
        .map(|r| (r.name.clone(), r.org.clone(), r.signals.clone()))
        .collect();
    findings.sort_by(|a, b| (b.2.len(), &a.0).cmp(&(a.2.len(), &b.0)));

    // text report
    println!("\n================ rain org health: per-repo findings ================");
    if findings.is_empty() {
        println!("  (no findings — all clean)");
    } else {
        for (repo, _org, sigs) in &findings {
            println!("  {:<30} {}", repo, sigs.join(" "));
        }
    }
    println!("\n================ org-wide summary (repos affected) =================");
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, _, sigs) in &findings {
        for s in sigs {
            *counts.entry(s).or_insert(0) += 1;
        }
    }
    let mut summary: Vec<(&str, usize)> = counts.into_iter().collect();
    summary.sort_by(|a, b| (b.1, a.0).cmp(&(a.1, b.0)));
    for (sig, n) in &summary {
        println!("  {n:>3}  {sig}");
    }
    println!("\nrepos with findings: {} / {}", findings.len(), total);

    // audit recency: last WHOLE-REPO audit per repo (never-audited + stalest first)
    results.sort_by_key(|r| audit_sort_key(r.last_audit.as_ref(), &r.name));
    let audited = results.iter().filter(|r| r.last_audit.is_some()).count();
    println!("\n================ audit recency (last whole-repo audit) ============");
    for r in &results {
        match &r.last_audit {
            None => println!("  {:<30} never audited", r.name),
            Some(a) => {
                let ver = if a.skill_version.is_empty() {
                    String::new()
                } else {
                    format!("  v{}", a.skill_version)
                };
                let mark = if a.stale == Some(true) {
                    "  (stale: HEAD moved since)"
                } else {
                    ""
                };
                println!("  {:<30} {}{}{}", r.name, a.audited_at, ver, mark);
            }
        }
    }
    println!("\nwhole-repo audited: {audited} / {total}");

    // external (Protofire) audit coverage + drift: never-audited is the headline gap;
    // audited repos list their drift since the audited tag/PDF. Order: never-audited
    // first (the gap), then audited stale-first, then by source-LOC drift, then name.
    let now = now_iso();
    // Protofire coverage is a Solidity question: a repo that is not a Foundry/Solidity
    // project (no foundry.toml) and carries no PDF is excluded from the report entirely
    // — not listed and not counted as a coverage gap (#54).
    let mut pf_view: Vec<&RepoResult> = results
        .iter()
        .filter(|r| in_protofire_report(r.protofire.has_pdf, r.has_foundry))
        .collect();
    let protofire_total = pf_view.len();
    let externally_audited = pf_view.iter().filter(|r| r.protofire.has_pdf).count();
    pf_view.sort_by(|a, b| {
        let (pa, pb) = (&a.protofire, &b.protofire);
        (
            pa.has_pdf,
            std::cmp::Reverse(protofire::staleness_rank(pa.is_stale)),
            std::cmp::Reverse(pa.source_loc.unwrap_or(0)),
            &a.name,
        )
            .cmp(&(
                pb.has_pdf,
                std::cmp::Reverse(protofire::staleness_rank(pb.is_stale)),
                std::cmp::Reverse(pb.source_loc.unwrap_or(0)),
                &b.name,
            ))
    });
    println!("\n============ external audit coverage (Protofire PDFs under audit/protofire/) ====");
    println!("  externally audited:       {externally_audited} / {protofire_total}");
    println!(
        "  NEVER externally audited: {} / {protofire_total}  (the coverage gap)",
        protofire_total - externally_audited
    );
    for r in &pf_view {
        let p = &r.protofire;
        if !p.has_pdf {
            continue;
        }
        let refd = p.audited_ref.as_deref().unwrap_or("(unanchored)");
        // tag → [tag-anchored], commit → [commit-anchored], neither → [unanchored]
        // (never the malformed [unanchored-anchored]).
        let anchor_label = match p.anchor_kind {
            Some("unanchored") | None => "[unanchored]".to_string(),
            Some(kind) => format!("[{kind}-anchored]"),
        };
        let latest = p.latest_tag.as_deref().unwrap_or("(no tags)");
        let drift = match (
            p.source_loc_added,
            p.source_loc_removed,
            p.files_changed,
            p.commits_since,
        ) {
            (Some(added), Some(removed), Some(files), Some(commits)) => {
                let days = days_between(&p.audited_date, &now).unwrap_or(-1);
                let trunc = if p.source_drift_truncated { "+" } else { "" };
                format!(
                    "+{added}{trunc} / -{removed}{trunc} src LOC / {files} files / {commits} commits · {days}d"
                )
            }
            _ => "drift unavailable".to_string(),
        };
        println!(
            "  {:<28} {:<8} audited {refd} {anchor_label} → latest {latest} · {drift}",
            r.name,
            p.external_audit.as_str()
        );
    }

    // JSON output — always written (populate by default)
    {
        let path = json_out;
        // roh-scan is the producer of SCAN data only. It does NOT compute pipeline/FSM state and
        // does NOT call pr-review-report: the dashboard's FSM panel fetches issue-pr-cron's own
        // `human-queue.json` artifact at runtime (see CLAUDE.md — the dashboard is a consumer, not
        // a producer, of data). Do not re-add a `humanQueue` block to health.json here.
        //
        // `protofireAudits` is scan-cadence data (same producer/cadence as the rest of health.json),
        // so it belongs IN health.json — the dashboard already fetches it, no new artifact/fetch.
        // The audit skill's own backlog: open `audit`-labelled issues per repo,
        // reported next to WHEN the repo was last audited so a stamp isn't read as
        // "done" while its findings are still open. One search per org.
        let (audit_issue_counts, audit_issue_orgs) = fetch_open_audit_issues(&orgs);

        // #71: the first-party dependency DAG, for the whole org rather than one
        // entrypoint's slice, so the dashboard can answer blast radius for any
        // node. Nodes carry the audit verdict so the graph shows WHERE the sand
        // is; edges are consumer -> dependency.
        let graph_nodes: Vec<graph::Node> = results
            .iter()
            .map(|r| graph::Node {
                repo: r.name.clone(),
                package: r.package.clone(),
                version: r.version.clone(),
                deps: r.deps.clone(),
                deps_known: r.deps_known,
                audit: r.protofire.external_audit,
            })
            .collect();
        // Every Solidity repo is a node, including one with no first-party edge
        // either way. An isolated repo is not noise: unaudited with nothing
        // beneath it, it is an audit with ZERO blockers — the cheapest work on
        // the board, and dropping it would hide it. Non-Solidity repos are not
        // audit targets and stay out.
        let solidity: std::collections::BTreeSet<&str> = results
            .iter()
            .filter(|r| r.has_foundry)
            .map(|r| r.name.as_str())
            .collect();

        // A graph that looks complete and is not is worse than no graph, so each
        // way of being untrustworthy omits it and says why.
        let audit_graph = if partial_scan {
            eprintln!(
                "auditGraph omitted: a partial scan drops edges to unscanned repos, which would report falsely clear ground"
            );
            serde_json::Value::Null
        } else {
            match (
                graph::graph_edges(&graph_nodes),
                graph::blockers(&graph_nodes),
            ) {
                (Ok(edges), Ok(blockers)) => json!({
                    "nodes": graph_nodes.iter().zip(results.iter()).filter(|(n, _)| solidity.contains(n.repo.as_str())).map(|(n, r)| json!({
                        "repo": n.repo,
                        "org": r.org,
                        "package": n.package,
                        "audit": n.audit.as_str(),
                        "depsKnown": n.deps_known,
                        // When the audit skill last ran whole-repo here, and how
                        // many of its findings are still open — so the graph shows
                        // recency and outstanding work, not just a verdict.
                        "lastAudit": r.last_audit.as_ref().map_or(serde_json::Value::Null, |a| json!({
                            "auditedAt": a.audited_at,
                            "auditedCommit": a.audited_commit,
                            "skillVersion": a.skill_version,
                            "stale": a.stale,
                        })),
                        "openAuditIssues": open_audit_issues(&audit_issue_counts, &audit_issue_orgs, &r.org, &n.repo),
                        // Newest adversarial-mutation run for this repo (commit + when).
                        "lastMutation": r.last_mutation.as_ref().map_or(serde_json::Value::Null, |m| json!({
                            "timestamp": m.timestamp,
                            "commit": m.commit,
                            "skillVersion": m.skill_version,
                            "scope": m.scope,
                        })),
                        "blockedBy": blockers.get(&n.repo).cloned().unwrap_or_default(),
                        // The dependencies this repo pins below their current
                        // version — what an audit should move to latest (#79).
                        "staleDeps": edges.iter()
                            .filter(|e| e.from == n.repo && e.stale)
                            .map(|e| json!({"repo": e.to, "pinned": e.pinned, "latest": e.latest}))
                            .collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "edges": edges.iter().map(|e| json!({
                        "from": e.from,
                        "to": e.to,
                        "stale": e.stale,
                        "pinned": e.pinned,
                        "latest": e.latest,
                    })).collect::<Vec<_>>(),
                }),
                // Two repos publishing one package makes every edge for it point
                // at whichever landed last.
                (Err(d), _) | (_, Err(d)) => {
                    eprintln!(
                        "::error::auditGraph omitted: package {:?} is published by {} — every edge for it would target an arbitrary one",
                        d.package,
                        d.repos.join(" and ")
                    );
                    serde_json::Value::Null
                }
            }
        };

        // st0x.deploy's owner / privileged-address constants (#88), for the
        // Deployments page's "known owners" view. A one-off targeted read — the
        // pins live in a handful of libraries in one repo, not per-repo — so it is
        // fetched here rather than in the per-repo scan. `null` if unreachable.
        let deployment_owners = {
            let (org, repo) = ("S01-Issuer", "st0x.deploy");
            let safe = gh_file(org, repo, "src/lib/LibSafeInvariants.sol");
            let auth = gh_file(org, repo, "src/lib/LibAuthoriserInvariants.sol");
            let v4 = gh_file(org, repo, "src/generated/LibProdDeployV4.sol");
            let overrides = gh_file(org, repo, "src/lib/LibProdDeployV2BaseOverrides.sol");
            // Read the live Base Safe (getOwners + getThreshold) so the page can
            // show declared-constant vs on-chain provenance. Best-effort: on any
            // RPC failure the fields stay None and the section falls back to
            // constants-only.
            let onchain =
                owners::parse_address_constant(&safe, "STOX_TOKEN_OWNER_SAFE").map(|safe_addr| {
                    let s = rpc_session(Chain::Base);
                    let owners_live = eth_call(s, &safe_addr, &rpc::get_owners_calldata())
                        .and_then(|hex| rpc::decode_owners(&hex));
                    let threshold_live = eth_call(s, &safe_addr, &rpc::get_threshold_calldata())
                        .and_then(|hex| rpc::decode_uint(&hex));
                    owners::OnChainSafe {
                        network: "base".into(),
                        safe: safe_addr,
                        rpc_host: "mainnet.base.org".into(),
                        owners: owners_live,
                        threshold: threshold_live,
                    }
                });
            // Which authoriser clone is LIVE, read from a production receipt
            // vault rather than asserted from a constant. The two Base clones
            // trade places during the V4 migration, so a hardcoded
            // active/pending pair silently contradicts the token rows further
            // down this same page, which read `authorizer()` per token.
            let live_authoriser = {
                let tok_lib = gh_file(org, repo, "src/lib/LibTokenInvariants.sol");
                deployhealth::parse_receipt_vault_list(&tok_lib)
                    .addresses
                    .first()
                    .and_then(|vault| {
                        eth_call(rpc_session(Chain::Base), vault, &rpc::authorizer_calldata())
                    })
                    .and_then(|hex| rpc::decode_address(&hex))
            };
            owners::build_owners(
                org,
                repo,
                &safe,
                &auth,
                &v4,
                &overrides,
                onchain.as_ref(),
                live_authoriser.as_deref(),
            )
            .unwrap_or(serde_json::Value::Null)
        };

        // On-chain health of the pinned 0.1.1 suite on Base (#84): for each
        // generated pointer file, confirm the contract is deployed at its pinned
        // address and the live code matches BOTH the RUNTIME_CODE bytes and the
        // BYTECODE_HASH keccak. Best-effort — a failed eth_getCode marks that one
        // contract `unknown` rather than failing the scan.
        let deployment_health = {
            let (org, repo, version) = ("S01-Issuer", "st0x.deploy", "0.1.1");
            let dir = format!("src/generated/{}", version.replace('.', "_"));
            match gh_contents_entries(&gh, org, repo, &dir) {
                ContentsListing::Found(entries) => {
                    let contracts: Vec<_> = entries
                        .iter()
                        .filter(|(t, _, name)| t == "file" && name.ends_with(".pointers.sol"))
                        .map(|(_, path, name)| {
                            let src = gh_file(org, repo, path);
                            let cname = name.strip_suffix(".pointers.sol").unwrap_or(name);
                            let addr = owners::parse_address_constant(&src, "DEPLOYED_ADDRESS");
                            let runtime = deployhealth::parse_hex_constant(&src, "RUNTIME_CODE");
                            let hash = deployhealth::parse_bytes32_constant(&src, "BYTECODE_HASH");
                            // One RPC session per contract, so getCode + both ERC-165
                            // probes hit the same endpoint and can't disagree.
                            let s = rpc_session(Chain::Base);
                            let onchain = addr.as_deref().and_then(|a| eth_get_code(s, a));
                            // ERC-165 conformance: supportsInterface(0x01ffc9a7) must
                            // be true and supportsInterface(0xffffffff) false, both on
                            // Base. Absent (both revert) is fine for e.g. a beacon.
                            let erc165 = match addr.as_deref() {
                                Some(a) => deployhealth::erc165_status(
                                    supports_interface(s, a, [0x01, 0xff, 0xc9, 0xa7]),
                                    supports_interface(s, a, [0xff, 0xff, 0xff, 0xff]),
                                ),
                                None => "unknown",
                            };
                            deployhealth::contract_health(
                                cname, addr, runtime, hash, onchain, erc165,
                            )
                        })
                        .collect();
                    deployhealth::build_health(
                        org,
                        repo,
                        version,
                        "base",
                        "mainnet.base.org",
                        contracts,
                    )
                    .unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            }
        };

        // The 3 production beacons on Base (#84): each should be owned by the
        // token-owner Safe and point at its pinned implementation. owner() +
        // implementation() are read live and checked against the constants.
        let deployment_beacons = {
            let (org, repo) = ("S01-Issuer", "st0x.deploy");
            let v1 = gh_file(org, repo, "src/lib/LibProdDeployV1.sol");
            let safe_lib = gh_file(org, repo, "src/lib/LibSafeInvariants.sol");
            let safe_owner = owners::parse_address_constant(&safe_lib, "STOX_TOKEN_OWNER_SAFE");
            // The pre-migration deploy EOA — a beacon still owned by this hasn't
            // been handed to the Safe.
            let legacy_owner = owners::parse_address_constant(&v1, "BEACON_INITIAL_OWNER");
            // (label, beacon addr const, V1-impl const, 0.1.1-target pointer file).
            // The 0.1.1 target impl is that contract's DEPLOYED_ADDRESS in the
            // generated 0_1_1 dir; the V1 impl is the pre-Zoltu one in LibProdDeployV1.
            let spec = [
                (
                    "Receipt beacon",
                    "STOX_RECEIPT_BEACON_V1",
                    "STOX_RECEIPT_IMPLEMENTATION",
                    "src/generated/0_1_1/StoxReceipt.pointers.sol",
                ),
                (
                    "Receipt-vault beacon",
                    "STOX_RECEIPT_VAULT_BEACON_V1",
                    "STOX_RECEIPT_VAULT_IMPLEMENTATION",
                    "src/generated/0_1_1/StoxReceiptVault.pointers.sol",
                ),
                (
                    "Wrapped-token-vault beacon",
                    "STOX_WRAPPED_TOKEN_VAULT_BEACON_V1",
                    "STOX_WRAPPED_TOKEN_VAULT_IMPLEMENTATION",
                    "src/generated/0_1_1/StoxWrappedTokenVault.pointers.sol",
                ),
            ];
            match (safe_owner, legacy_owner) {
                (Some(safe), Some(legacy)) => {
                    let beacons: Vec<_> = spec
                        .iter()
                        .map(|(label, beacon_const, v1_impl_const, target_file)| {
                            let addr = owners::parse_address_constant(&v1, beacon_const);
                            let v1_impl = owners::parse_address_constant(&v1, v1_impl_const);
                            let target_impl = owners::parse_address_constant(
                                &gh_file(org, repo, target_file),
                                "DEPLOYED_ADDRESS",
                            );
                            // One session per beacon so owner() + implementation()
                            // hit the same RPC and can't disagree.
                            let s = rpc_session(Chain::Base);
                            let live_owner = addr
                                .as_deref()
                                .and_then(|a| eth_call(s, a, &rpc::owner_calldata()))
                                .and_then(|hex| rpc::decode_address(&hex));
                            let live_impl = addr
                                .as_deref()
                                .and_then(|a| eth_call(s, a, &rpc::implementation_calldata()))
                                .and_then(|hex| rpc::decode_address(&hex));
                            deployhealth::beacon_health(
                                label,
                                addr,
                                &safe,
                                &legacy,
                                target_impl.as_deref(),
                                v1_impl.as_deref(),
                                "0.1.1",
                                live_owner,
                                live_impl,
                            )
                        })
                        .collect();
                    deployhealth::build_beacons(
                        org,
                        repo,
                        "base",
                        "mainnet.base.org",
                        &safe,
                        "0.1.1",
                        beacons,
                    )
                    .unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            }
        };

        // Ethereum's IN-USE beacons: the chain bootstrapped at 0.1.1, so its
        // production tokens run on the 0.1.1 generation — a different address
        // set from Base's V1-generation beacons, held by a different Safe.
        // Reading one chain's addresses against the other's endpoints would
        // report live contracts as missing, so the session carries the chain.
        let deployment_beacons_ethereum = {
            let (org, repo) = ("S01-Issuer", "st0x.deploy");
            let safe_lib = gh_file(org, repo, "src/lib/LibSafeInvariants.sol");
            let safe_owner =
                owners::parse_address_constant(&safe_lib, "STOX_TOKEN_OWNER_SAFE_ETHEREUM");
            let v1 = gh_file(org, repo, "src/lib/LibProdDeployV1.sol");
            let legacy_owner = owners::parse_address_constant(&v1, "BEACON_INITIAL_OWNER");
            // Only the wrapped-token-vault beacon has a generated address pin.
            // The receipt and receipt-vault beacons are created inside the
            // 0.1.1 beacon-set deployer's constructor and exist nowhere as a
            // constant, so the in-use pair is resolved live from its getters.
            let deployer = owners::parse_address_constant(
                &gh_file(
                    org,
                    repo,
                    "src/generated/0_1_1/StoxOffchainAssetReceiptVaultBeaconSetDeployer.pointers.sol",
                ),
                "DEPLOYED_ADDRESS",
            );
            let ds = rpc_session(Chain::Ethereum);
            let resolve = |calldata: String| {
                deployer
                    .as_deref()
                    .and_then(|d| eth_call(ds, d, &calldata))
                    .and_then(|hex| rpc::decode_address(&hex))
            };
            let spec = [
                (
                    "Receipt beacon",
                    resolve(rpc::receipt_beacon_calldata()),
                    "src/generated/0_1_1/StoxReceipt.pointers.sol",
                ),
                (
                    "Receipt-vault beacon",
                    resolve(rpc::receipt_vault_beacon_calldata()),
                    "src/generated/0_1_1/StoxReceiptVault.pointers.sol",
                ),
                (
                    "Wrapped-token-vault beacon",
                    owners::parse_address_constant(
                        &gh_file(
                            org,
                            repo,
                            "src/generated/0_1_1/StoxWrappedTokenVaultBeacon.pointers.sol",
                        ),
                        "DEPLOYED_ADDRESS",
                    ),
                    "src/generated/0_1_1/StoxWrappedTokenVault.pointers.sol",
                ),
            ];
            match (safe_owner, legacy_owner) {
                (Some(safe), Some(legacy)) => {
                    let beacons: Vec<_> = spec
                        .into_iter()
                        .map(|(label, addr, target_file)| {
                            let target_impl = owners::parse_address_constant(
                                &gh_file(org, repo, target_file),
                                "DEPLOYED_ADDRESS",
                            );
                            let s = rpc_session(Chain::Ethereum);
                            let live_owner = addr
                                .as_deref()
                                .and_then(|a| eth_call(s, a, &rpc::owner_calldata()))
                                .and_then(|hex| rpc::decode_address(&hex));
                            let live_impl = addr
                                .as_deref()
                                .and_then(|a| eth_call(s, a, &rpc::implementation_calldata()))
                                .and_then(|hex| rpc::decode_address(&hex));
                            // No previous generation to fall back to: Ethereum
                            // has no V1 deploy, so an impl that is not the
                            // 0.1.1 target is simply unrecognised.
                            deployhealth::beacon_health(
                                label,
                                addr,
                                &safe,
                                &legacy,
                                target_impl.as_deref(),
                                None,
                                "0.1.1",
                                live_owner,
                                live_impl,
                            )
                        })
                        .collect();
                    deployhealth::build_beacons(
                        org,
                        repo,
                        "ethereum",
                        Chain::Ethereum.rpc_host(),
                        &safe,
                        "0.1.1",
                        beacons,
                    )
                    .unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            }
        };

        // One block per chain. Base and Ethereum run on different beacon
        // generations owned by different Safes, so a single block could only
        // ever describe one of them. A chain the scan could not read is
        // reported as unavailable rather than dropped: a dropped chain renders
        // as no section at all, which reads as "this chain has no production
        // beacons" instead of "this broke".
        let beacon_sets: Vec<serde_json::Value> = [
            (deployment_beacons, "base", Chain::Base),
            (deployment_beacons_ethereum, "ethereum", Chain::Ethereum),
        ]
        .into_iter()
        .map(|(set, network, chain)| {
            if set.is_null() {
                deployhealth::beacons_unavailable(
                    network,
                    chain.rpc_host(),
                    "the scan could not read the st0x.deploy beacon constants",
                )
            } else {
                set
            }
        })
        .collect();

        // Registry token wiring on Base (#90): for each token in the
        // st0x.registry Base list, confirm the deployed wrapper's
        // name()/symbol()/decimals() match the registry VERBATIM, its asset()
        // points at the registry unwrappedAddress, and the linked
        // unwrapped/legacy/receipt addresses are deployed. Also resolve each
        // receipt vault's live authorizer() against the current prod authoriser
        // and the V4-clone target, so a proposed setAuthorizer migration can be
        // reviewed. Best-effort — a failed read marks that token `unknown`.
        let deployment_tokens = {
            let (org, repo) = ("ST0x-Technology", "st0x.registry");
            // Authoriser targets live in st0x.deploy: the current prod authoriser
            // every receipt vault points at today, and the V4 clone the pending
            // setAuthorizer migration rewires them to. Read once for the list.
            let (dorg, drepo) = ("S01-Issuer", "st0x.deploy");
            let auth_lib = gh_file(dorg, drepo, "src/lib/LibAuthoriserInvariants.sol");
            let v4_lib = gh_file(dorg, drepo, "src/generated/LibProdDeployV4.sol");
            let auth_current = owners::parse_address_constant(&auth_lib, "STOX_PROD_AUTHORISER");
            let auth_target =
                owners::parse_address_constant(&v4_lib, "STOX_PROD_AUTHORISER_V4_CLONE");
            let auth_target_deployed = auth_target
                .as_deref()
                .and_then(|a| code_deployed(rpc_session(Chain::Base), a));
            let auth_summary = json!({
                "current": auth_current,
                "target": auth_target,
                "targetDeployed": auth_target_deployed,
            });
            // The migration's AUTHORITATIVE governed set —
            // LibTokenInvariants.productionReceiptVaults(), the exact list the
            // setAuthorizer bundle operates on. Read up front so each token can be
            // cross-checked BOTH ways: registry→migration (is this token governed?)
            // and migration→registry (is this governed vault in the registry?).
            let tok_lib = gh_file(dorg, drepo, "src/lib/LibTokenInvariants.sol");
            let governed_parse = deployhealth::parse_receipt_vault_list(&tok_lib);
            let governed = governed_parse.addresses;
            let raw = gh_file(org, repo, "token-lists/base.json");
            let parsed: Option<serde_json::Value> = serde_json::from_str(&raw).ok();
            let tokens: Vec<serde_json::Value> = parsed
                .as_ref()
                .and_then(|v| v["tokens"].as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| {
                            let address = t["address"].as_str()?;
                            let name = t["name"].as_str()?;
                            let symbol = t["symbol"].as_str()?;
                            let decimals = t["decimals"].as_u64()?;
                            // Registry extension addresses; a token may legitimately
                            // omit one (plain collateral has no unwrapped; newer
                            // wrapped tokens carry an empty legacy) — treat "" as
                            // absent so it isn't probed or counted as a wiring gap.
                            let ext = &t["extensions"];
                            let nonempty = |k: &str| ext[k].as_str().filter(|s| !s.is_empty());
                            let unwrapped = nonempty("unwrappedAddress");
                            let legacy = nonempty("legacyAddress");
                            let receipt = nonempty("receiptAddress");
                            // The main list is the INTERSECTION: registry tokens the
                            // migration actually governs. A registry token with no
                            // governed receipt vault (plain collateral like USDC, or a
                            // wrapped token the migration misses) is a reconciliation
                            // discrepancy — it belongs in reconcile.missingFromMigration,
                            // not here — so skip it (and its probes) entirely.
                            let in_migration =
                                unwrapped.map(|u| governed.contains(&u.to_lowercase()));
                            if in_migration != Some(true) {
                                return None;
                            }
                            // One session per token so all its reads hit the same RPC.
                            let s = rpc_session(Chain::Base);
                            let live = deployhealth::TokenLive {
                                name: eth_call(s, address, &rpc::name_calldata())
                                    .and_then(|h| rpc::decode_string(&h)),
                                symbol: eth_call(s, address, &rpc::symbol_calldata())
                                    .and_then(|h| rpc::decode_string(&h)),
                                decimals: eth_call(s, address, &rpc::decimals_calldata())
                                    .and_then(|h| rpc::decode_u8(&h)),
                                asset: eth_call(s, address, &rpc::asset_calldata())
                                    .and_then(|h| rpc::decode_address(&h)),
                                unwrapped_deployed: unwrapped.and_then(|a| code_deployed(s, a)),
                                legacy_deployed: legacy.and_then(|a| code_deployed(s, a)),
                                receipt_deployed: receipt.and_then(|a| code_deployed(s, a)),
                                // authorizer() lives on the receipt vault (the unwrapped).
                                authoriser: unwrapped
                                    .and_then(|rv| eth_call(s, rv, &rpc::authorizer_calldata()))
                                    .and_then(|h| rpc::decode_address(&h)),
                            };
                            let spec = deployhealth::TokenSpec {
                                symbol,
                                name,
                                decimals,
                                address,
                                unwrapped,
                                legacy,
                                receipt,
                                auth_current: auth_current.as_deref(),
                                auth_target: auth_target.as_deref(),
                                // registry→migration: is this token's receipt vault
                                // in the governed set the bundle will setAuthorizer?
                                in_migration: unwrapped
                                    .map(|u| governed.contains(&u.to_lowercase())),
                            };
                            Some(deployhealth::token_health(&spec, &live))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Cross-check both directions. `governed` (parsed above) is the exact
            // migration set; the registry receipt-vault set is each token's
            // unwrappedAddress.
            let registry_vaults: std::collections::HashSet<String> = parsed
                .as_ref()
                .and_then(|v| v["tokens"].as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| {
                            t["extensions"]["unwrappedAddress"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .map(str::to_lowercase)
                        })
                        .collect()
                })
                .unwrap_or_default();
            // migration→registry: governed vaults with no registry token → probe
            // identity + authoriser so the whole bundle is reviewable.
            let extra_vaults: Vec<serde_json::Value> = governed
                .iter()
                .filter(|a| !registry_vaults.contains(*a))
                .map(|addr| {
                    let s = rpc_session(Chain::Base);
                    let name = eth_call(s, addr, &rpc::name_calldata())
                        .and_then(|h| rpc::decode_string(&h));
                    let symbol = eth_call(s, addr, &rpc::symbol_calldata())
                        .and_then(|h| rpc::decode_string(&h));
                    let deployed = code_deployed(s, addr);
                    let auth = eth_call(s, addr, &rpc::authorizer_calldata())
                        .and_then(|h| rpc::decode_address(&h));
                    deployhealth::extra_vault(
                        addr,
                        name,
                        symbol,
                        deployed,
                        auth.as_deref(),
                        auth_current.as_deref(),
                        auth_target.as_deref(),
                    )
                })
                .collect();
            // registry→migration: EVERY registry token that has no governed receipt
            // vault — carried WITH identity, symmetric to extraVaults. Two reasons:
            // plain collateral with no receipt vault at all (e.g. USDC — expected,
            // but shown so the reconcile is complete), or a wrapped token whose
            // receipt vault the migration doesn't cover (a real gap).
            let missing_from_migration: Vec<serde_json::Value> = parsed
                .as_ref()
                .and_then(|v| v["tokens"].as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| {
                            let unwrapped = t["extensions"]["unwrappedAddress"]
                                .as_str()
                                .filter(|s| !s.is_empty());
                            let governed_vault =
                                unwrapped.is_some_and(|u| governed.contains(&u.to_lowercase()));
                            if governed_vault {
                                return None;
                            }
                            let reason = if unwrapped.is_none() {
                                "no receipt vault (collateral)"
                            } else {
                                "receipt vault not in migration set"
                            };
                            Some(json!({
                                "symbol": t["symbol"],
                                "name": t["name"],
                                "address": t["address"],
                                "receiptVault": unwrapped,
                                "wrapped": unwrapped.is_some(),
                                "reason": reason,
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let registry_token_count = parsed
                .as_ref()
                .and_then(|v| v["tokens"].as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let reconcile = json!({
                "source": format!("{dorg}/{drepo}"),
                "function": "LibTokenInvariants.productionReceiptVaults()",
                "governedCount": governed.len(),
                // Entries the migration set LISTED. Greater than governedCount means
                // some entry named a constant that did not resolve, so the governed
                // set is short rather than genuinely smaller.
                "governedDeclared": governed_parse.declared,
                "registryTokenCount": registry_token_count,
                "extraVaults": extra_vaults,
                "missingFromMigration": missing_from_migration,
            });
            deployhealth::build_tokens(
                org,
                repo,
                "base",
                "mainnet.base.org",
                auth_summary,
                reconcile,
                tokens,
            )
            .unwrap_or(serde_json::Value::Null)
        };

        let doc = json!({
            "generatedAt": now,
            "auditGraph": audit_graph,
            "deploymentOwners": deployment_owners,
            "deploymentHealth": deployment_health,
            "deploymentBeacons": beacon_sets,
            "deploymentTokens": deployment_tokens,
            // Every org scanned. `org` stays as a joined display string so any
            // reader that has not moved to `orgs` still shows something sensible.
            "orgs": orgs,
            "org": orgs.join(", "),
            "totalRepos": total,
            "reposWithFindings": findings.len(),
            "reposWholeRepoAudited": audited,
            "reposNeverAudited": total - audited,
            "reposExternallyAudited": externally_audited,
            "reposNeverExternallyAudited": protofire_total - externally_audited,
            "summary": summary.iter().map(|(s, n)| (s.to_string(), serde_json::Value::from(*n))).collect::<serde_json::Map<String, serde_json::Value>>(),
            "repos": findings.iter().map(|(r, org, sigs)| json!({"name": r, "org": org, "signals": sigs})).collect::<Vec<_>>(),
            "audits": results.iter().map(|r| {
                // Open findings from the audit skill, alongside the run stamp: a
                // repo can be freshly audited AND still carry open findings.
                let open = open_audit_issues(&audit_issue_counts, &audit_issue_orgs, &r.org, &r.name);
                // The newest adversarial-mutation run: when, and at which commit.
                let mutation = r.last_mutation.as_ref().map_or(serde_json::Value::Null, |m| json!({
                    "timestamp": m.timestamp,
                    "commit": m.commit,
                    "skillVersion": m.skill_version,
                    "scope": m.scope,
                }));
                match &r.last_audit {
                    None => json!({ "name": r.name, "org": r.org, "lastAudit": serde_json::Value::Null, "openAuditIssues": open, "lastMutation": mutation }),
                    Some(a) => json!({ "name": r.name, "org": r.org, "openAuditIssues": open, "lastMutation": mutation, "lastAudit": {
                        "auditedAt": a.audited_at,
                        "auditedCommit": a.audited_commit,
                        "skillVersion": a.skill_version,
                        "stale": a.stale,
                    }}),
                }
            }).collect::<Vec<_>>(),
            "protofireAudits": pf_view.iter().map(|r| {
                let p = &r.protofire;
                let days = if p.audited_date.is_empty() {
                    serde_json::Value::Null
                } else {
                    days_between(&p.audited_date, &now).map_or(serde_json::Value::Null, serde_json::Value::from)
                };
                json!({
                    "name": r.name,
                    "org": r.org,
                    "hasProtofireAudit": p.has_pdf,
                    "externalAudit": p.external_audit.as_str(),
                    "auditPdfs": p.pdfs.iter().map(|pdf| json!({
                        "filename": pdf.filename,
                        "path": pdf.path,
                        "lastCommitIso": pdf.last_commit_iso,
                        "auditDateIso": pdf.audit_date_iso,
                    })).collect::<Vec<_>>(),
                    "referencePdfIndex": p.reference_pdf_index,
                    "auditedRef": p.audited_ref,
                    "anchorKind": p.anchor_kind,
                    "tagConventionAbsent": p.tag_convention_absent,
                    "auditedDate": if p.audited_date.is_empty() { serde_json::Value::Null } else { serde_json::Value::from(p.audited_date.clone()) },
                    "latestTag": p.latest_tag,
                    "latestTagIso": p.latest_tag_iso,
                    "isStale": match (p.has_pdf, p.is_stale) { (true, Some(b)) => serde_json::Value::from(b), _ => serde_json::Value::Null },
                    "sourceLocChangedSinceAudit": p.source_loc,
                    "fullSourceLoc": r.full_source_loc,
                    "sourceLocAddedSinceAudit": p.source_loc_added,
                    "sourceLocRemovedSinceAudit": p.source_loc_removed,
                    "filesChangedSinceAudit": p.files_changed,
                    "commitsSinceAudit": p.commits_since,
                    "sourceDriftTruncated": p.source_drift_truncated,
                    // Comment churn counted apart from code: a NatSpec-only edit
                    // shows here and leaves the audit CURRENT.
                    "commentLocAddedSinceAudit": p.comment_loc_added,
                    "commentLocRemovedSinceAudit": p.comment_loc_removed,
                    "codeLocAddedSinceAudit": p.code_loc_added,
                    "codeLocRemovedSinceAudit": p.code_loc_removed,
                    "driftFullyClassified": p.drift_fully_classified,
                    "compareUrl": p.compare_url,
                    "daysSinceAudit": days,
                })
            }).collect::<Vec<_>>(),
        });
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).is_ok() {
            eprintln!("wrote {path} ({} repos with findings)", findings.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The graph's staleness ceiling is the newest PUBLISHED revision, so the parse
    /// must distinguish published / unpublished / unreadable. Collapsing the last
    /// two would let a registry hiccup read as "no releases" and silently clear
    /// every consumer beneath that package (#86).
    #[test]
    fn latest_revision_reads_published_unpublished_and_unreadable_apart() {
        // published: the newest revision, which `limit=1` puts first
        assert_eq!(
            latest_revision_from_response(br#"{"data":[{"version":"0.1.3"}]}"#),
            Some(Some("0.1.3".to_string()))
        );
        // answered, but the package has never been published
        assert_eq!(
            latest_revision_from_response(br#"{"data":[]}"#),
            Some(None),
            "an empty data array is a real answer: no revisions"
        );
        // unreadable answers are unknown, NOT "no revisions"
        assert_eq!(
            latest_revision_from_response(b"not json at all"),
            None,
            "malformed json is unknown, never an empty registry"
        );
        assert_eq!(
            latest_revision_from_response(br#"{"error":"boom"}"#),
            None,
            "a response with no data array is unknown"
        );
        // a revision row without a version is not a version we can judge against
        assert_eq!(
            latest_revision_from_response(br#"{"data":[{"nope":1}]}"#),
            Some(None)
        );
    }

    /// A scripted `GhApi` for network-free tests. The first route whose needle is
    /// a substring of the joined args decides the returned `FetchOutcome`; an
    /// unmatched request defaults to `NotFound`. Per-needle call counts prove
    /// which endpoints were hit (and that no phantom retry happened at this seam —
    /// retry lives in `GhCli`, which this double bypasses). Never sleeps or spawns.
    struct FakeGh {
        routes: Vec<(&'static str, FetchOutcome)>,
        calls: Mutex<HashMap<String, usize>>,
    }

    impl FakeGh {
        fn new(routes: Vec<(&'static str, FetchOutcome)>) -> Self {
            FakeGh {
                routes,
                calls: Mutex::new(HashMap::new()),
            }
        }
        fn count(&self, needle: &str) -> usize {
            *self.calls.lock().unwrap().get(needle).unwrap_or(&0)
        }
    }

    impl GhApi for FakeGh {
        fn api_jq(&self, args: &[&str]) -> FetchOutcome {
            let joined = args.join(" ");
            for (needle, outcome) in &self.routes {
                if joined.contains(needle) {
                    *self
                        .calls
                        .lock()
                        .unwrap()
                        .entry((*needle).to_string())
                        .or_insert(0) += 1;
                    return outcome.clone();
                }
            }
            FetchOutcome::NotFound
        }
    }

    /// A `contents` listing row (`type\tpath\tname`) for one flat `.pdf`.
    fn pdf_row(name: &str) -> FetchOutcome {
        FetchOutcome::Found(format!("file\taudit/protofire/{name}\t{name}"))
    }

    /// A recursive `git/trees` response carrying the given `(path, sha)` blobs and
    /// truncation flag, as the scanner's tree fetch receives it.
    fn tree_json(blobs: &[(&str, &str)], truncated: bool) -> FetchOutcome {
        let tree: Vec<serde_json::Value> = blobs
            .iter()
            .map(|(p, s)| json!({"path": p, "type": "blob", "sha": s}))
            .collect();
        FetchOutcome::Found(json!({"truncated": truncated, "tree": tree}).to_string())
    }

    // ---- tree-diff drift fallback: changed_sol_count / fetch_tree_blobs ----

    #[test]
    fn changed_sol_count_diffs_trees_accurately() {
        // Mod.sol modified, New.sol added, Gone.sol removed → 3 changed non-test
        // .sol. Keep.sol is unchanged; the test-file and .rs churn are excluded.
        let base = tree_json(
            &[
                ("src/Keep.sol", "k"),
                ("src/Mod.sol", "m1"),
                ("src/Gone.sol", "g"),
                ("test/T.t.sol", "t1"),
                ("crates/x/src/lib.rs", "r1"),
            ],
            false,
        );
        let head = tree_json(
            &[
                ("src/Keep.sol", "k"),
                ("src/Mod.sol", "m2"),
                ("src/New.sol", "n"),
                ("test/T.t.sol", "t9"),
                ("crates/x/src/lib.rs", "r9"),
            ],
            false,
        );
        let gh = FakeGh::new(vec![("git/trees/base", base), ("git/trees/head", head)]);
        assert_eq!(changed_sol_count(&gh, "o", "r", "base", "head"), Some(3));
    }

    #[test]
    fn changed_sol_count_is_none_when_a_tree_is_truncated_or_failed() {
        // A truncated tree can't give a reliable diff → None (never a false 0).
        let gh = FakeGh::new(vec![
            ("git/trees/base", tree_json(&[("src/A.sol", "a")], true)),
            ("git/trees/head", tree_json(&[("src/A.sol", "b")], false)),
        ]);
        assert_eq!(changed_sol_count(&gh, "o", "r", "base", "head"), None);
        // A failed fetch is likewise indeterminate, not zero.
        let gh2 = FakeGh::new(vec![("git/trees/base", FetchOutcome::Failed)]);
        assert_eq!(changed_sol_count(&gh2, "o", "r", "base", "head"), None);
    }

    // ---- Protofire-report membership gate (#54) ----
    #[test]
    fn protofire_report_includes_solidity_projects_and_audited_repos_only() {
        // An audited repo (has a PDF) is always in the report, even without a foundry.toml.
        assert!(in_protofire_report(true, false));
        // A Foundry/Solidity project with no audit is the coverage gap → included.
        assert!(in_protofire_report(false, true));
        assert!(in_protofire_report(true, true));
        // No PDF and no foundry.toml (docs/subgraph/tooling/.github) → excluded, not a gap.
        assert!(!in_protofire_report(false, false));
    }

    // ---- unaudited source LOC counting (#37) ----
    #[test]
    fn sum_sol_loc_counts_non_test_solidity_lines_only() {
        let dir = std::env::temp_dir().join(format!("rohloc-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("src/A.sol"), "line1\nline2\nline3\n").unwrap(); // 3 non-test .sol
        std::fs::write(dir.join("deploy/D.sol"), "").ok(); // ensure parent exists next
        std::fs::create_dir_all(dir.join("deploy")).unwrap();
        std::fs::write(dir.join("deploy/D.sol"), "a\nb\n").unwrap(); // 2 non-test .sol (deploy/)
        std::fs::write(dir.join("test/B.t.sol"), "x\ny\nz\nw\n").unwrap(); // .t.sol → excluded
        std::fs::write(dir.join("test/Helper.sol"), "p\nq\n").unwrap(); // under test/ → excluded
        std::fs::write(dir.join("README.md"), "not\nsol\n").unwrap(); // non-source → excluded
        std::fs::write(dir.join(".git/config"), "junk\njunk\n").unwrap(); // .git → skipped
                                                                          // A symlink to a real .sol must NOT be followed (else A.sol is counted twice);
                                                                          // the total staying 5 proves symlinks are skipped, not traversed.
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("src/A.sol"), dir.join("src/link.sol")).unwrap();
        let loc = sum_sol_loc(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            loc, 5,
            "3 (src/A.sol) + 2 (deploy/D.sol); tests, README, .git, and the symlink excluded"
        );
    }

    // ---- external-call coverage: fetch_protofire_audit / collect_audit_pdfs ----

    #[test]
    fn found_pdf_classifies_as_audited_not_never_or_unknown() {
        // A audit/protofire listing carrying a .pdf → the repo is audited. No tags
        // are scripted, so the taxonomy lands on `na` (has PDF, nothing to compare)
        // — the point is it is NEITHER `never` NOR `unknown`.
        let gh = FakeGh::new(vec![(
            "contents/audit/protofire",
            pdf_row("rain.example.v1.2.3.jun-2026.pdf"),
        )]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        assert!(
            r.has_pdf,
            "a listed .pdf means the repo has a Protofire audit"
        );
        assert_eq!(r.external_audit, protofire::ExternalAudit::Na);
        assert_ne!(r.external_audit, protofire::ExternalAudit::Never);
        assert_ne!(r.external_audit, protofire::ExternalAudit::Unknown);
    }

    #[test]
    fn not_found_listing_is_never_audited() {
        // A genuine 404 on the top-level listing → genuinely absent → `never`.
        let gh = FakeGh::new(vec![("contents/audit/protofire", FetchOutcome::NotFound)]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        assert!(!r.has_pdf);
        assert_eq!(r.external_audit, protofire::ExternalAudit::Never);
    }

    #[test]
    fn failed_listing_is_unknown_never_a_false_never() {
        // THE #52 FIX: a FAILED top-level listing (rate-limit/network after retries)
        // must classify as `unknown`, never the false coverage claim `never`. This
        // assertion fails against the pre-fix code, which returned `never` here.
        let gh = FakeGh::new(vec![("contents/audit/protofire", FetchOutcome::Failed)]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        assert!(!r.has_pdf);
        assert_eq!(r.external_audit, protofire::ExternalAudit::Unknown);
        assert_ne!(
            r.external_audit,
            protofire::ExternalAudit::Never,
            "a failed fetch must NEVER be reported as never-audited"
        );
        // The listing was fetched exactly once at this seam (retry lives in GhCli).
        assert_eq!(gh.count("contents/audit/protofire"), 1);
    }

    // ---- audit-date reference selection (commit_date + newest-by-audit) ----

    #[test]
    fn commit_date_resolves_ref_else_none() {
        let gh = FakeGh::new(vec![(
            "commits/deadbee",
            FetchOutcome::Found("2026-02-07T15:29:43Z".into()),
        )]);
        // A resolvable ref → its committer date.
        assert_eq!(
            commit_date(&gh, "o", "r", "deadbee").as_deref(),
            Some("2026-02-07T15:29:43Z")
        );
        // An unresolvable ref (NotFound at this seam) → None, so the caller falls
        // back to the PDF's own file-commit date.
        assert_eq!(commit_date(&gh, "o", "r", "0000000"), None);
    }

    #[test]
    fn reference_pdf_is_newest_audit_not_newest_file_commit() {
        // Two audited PDFs. The JAN one has the NEWER file-commit date but the
        // OLDER audited-commit date; the FEB one is the reverse. The reference must
        // be the newest AUDIT (feb), proving selection keys on the audited commit
        // date — not the PDF's own file-commit date, which a batch move collapses.
        // (Pre-fix, this picks the jan PDF by its newer file date and FAILS.)
        let listing = FetchOutcome::Found(format!(
            "file\taudit/protofire/{a}\t{a}\nfile\taudit/protofire/{b}\t{b}",
            a = "repo.aaaaaa1.jan-2026.pdf",
            b = "repo.bbbbbb2.feb-2026.pdf",
        ));
        let gh = FakeGh::new(vec![
            ("contents/audit/protofire", listing),
            // pdf_commit (file dates): jan file is NEWER than feb file.
            (
                "commits?path=audit/protofire/repo.aaaaaa1",
                FetchOutcome::Found("2026-08-01T00:00:00Z\tfileshaA".into()),
            ),
            (
                "commits?path=audit/protofire/repo.bbbbbb2",
                FetchOutcome::Found("2026-07-01T00:00:00Z\tfileshaB".into()),
            ),
            // Anchor (audited-commit) dates: feb is NEWER than jan. This route also
            // answers commit_exists for the chosen anchor (non-empty ⇒ exists).
            (
                "commits/aaaaaa1",
                FetchOutcome::Found("2026-01-27T00:00:00Z".into()),
            ),
            (
                "commits/bbbbbb2",
                FetchOutcome::Found("2026-02-07T00:00:00Z".into()),
            ),
            ("graphql", FetchOutcome::Found("main\t\t".into())),
        ]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        // Sorted by path, jan is index 0 and feb index 1; the reference is feb.
        assert_eq!(
            r.reference_pdf_index,
            Some(1),
            "reference is the newest AUDIT"
        );
        assert_eq!(
            r.audited_ref.as_deref(),
            Some("bbbbbb2"),
            "drift anchors to the feb audit's commit"
        );
        assert!(r.pdfs[1].filename.contains("feb-2026"));
    }

    // ---- retry: pure classifier + bounded-retry driver (no sleeping) ----

    #[test]
    fn gh_failure_classifies_404_as_notfound_else_retryable() {
        assert_eq!(
            classify_gh_failure("gh: Not Found (HTTP 404)"),
            GhFailure::NotFound
        );
        assert_eq!(
            classify_gh_failure("HTTP 403: API rate limit exceeded (secondary)"),
            GhFailure::Retryable
        );
        assert_eq!(classify_gh_failure(""), GhFailure::Retryable);
    }

    #[test]
    fn retry_fetch_retries_then_succeeds_with_exact_count() {
        // Retryable for the first 2 attempts, then Found → the driver returns Found
        // and the attempt count proves it retried exactly twice (3 attempts total).
        let mut calls = 0u32;
        let out = retry_fetch(4, |_| {
            calls += 1;
            if calls < 3 {
                Attempt::Retryable
            } else {
                Attempt::Found("ok".into())
            }
        });
        assert_eq!(out, FetchOutcome::Found("ok".into()));
        assert_eq!(calls, 3, "two retries then a success");
    }

    #[test]
    fn retry_fetch_exhausts_to_failed() {
        // Always Retryable → after `max_attempts` the outcome is `Failed`, and it
        // tried exactly `max_attempts` times (no more, no fewer).
        let mut calls = 0u32;
        let out = retry_fetch(4, |_| {
            calls += 1;
            Attempt::Retryable
        });
        assert_eq!(out, FetchOutcome::Failed);
        assert_eq!(calls, 4);
    }

    #[test]
    fn retry_fetch_notfound_short_circuits() {
        // A 404 is authoritative — the driver returns immediately without retrying.
        let mut calls = 0u32;
        let out = retry_fetch(4, |_| {
            calls += 1;
            Attempt::NotFound
        });
        assert_eq!(out, FetchOutcome::NotFound);
        assert_eq!(calls, 1, "NotFound must not be retried");
    }

    // ---- concurrency: scan_repos ----

    fn repo_set(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn sorted(mut v: Vec<(String, &'static str)>) -> Vec<(String, &'static str)> {
        v.sort();
        v
    }

    #[test]
    fn scan_repos_returns_every_repo_at_any_parallelism() {
        let repos = repo_set(&["a", "b", "c", "d", "e"]);
        let expected = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        // par=1, par==len, and par>len must all cover every repo exactly once.
        for par in [1usize, repos.len(), repos.len() + 3] {
            let out = scan_repos(repos.clone(), par, |r| (r.to_string(), "ok"));
            let mut names: Vec<String> = out.into_iter().map(|(n, _)| n).collect();
            names.sort();
            assert_eq!(names, expected, "par={par} must cover every repo");
        }
    }

    #[test]
    fn scan_repos_result_set_is_par_independent() {
        let repos = repo_set(&["alpha", "beta", "gamma", "delta"]);
        let baseline = sorted(scan_repos(repos.clone(), 1, |r| (r.to_string(), "state")));
        for par in [2usize, 4, 50] {
            let got = sorted(scan_repos(repos.clone(), par, |r| (r.to_string(), "state")));
            assert_eq!(got, baseline, "par={par} must yield the same result set");
        }
    }

    #[test]
    fn scan_repos_isolates_per_repo_results_across_threads() {
        // One repo yields `unknown`, the rest `current`. No thread may cross-
        // contaminate: exactly the one repo is `unknown`, every other is `current`.
        let repos = repo_set(&["r0", "r1", "r2", "r3", "r4", "r5"]);
        let out = scan_repos(repos.clone(), 4, |r| {
            let state = if r == "r3" {
                protofire::ExternalAudit::Unknown
            } else {
                protofire::ExternalAudit::Current
            };
            (r.to_string(), state)
        });
        assert_eq!(out.len(), repos.len(), "every repo produced a result");
        for (name, state) in &out {
            if name == "r3" {
                assert_eq!(
                    *state,
                    protofire::ExternalAudit::Unknown,
                    "only r3 is unknown"
                );
            } else {
                assert_eq!(
                    *state,
                    protofire::ExternalAudit::Current,
                    "{name} must stay current"
                );
            }
        }
        let unknowns = out
            .iter()
            .filter(|(_, s)| *s == protofire::ExternalAudit::Unknown)
            .count();
        assert_eq!(unknowns, 1, "exactly one repo is unknown — no leakage");
    }

    #[test]
    fn scan_repos_empty_input_is_empty() {
        let out: Vec<(String, &'static str)> =
            scan_repos(Vec::<String>::new(), 4, |r: &String| (r.to_string(), "x"));
        assert!(out.is_empty());
    }

    #[test]
    fn default_populates_site_health_json() {
        // A bare run (no JSON_OUT, no --json) POPULATES site/health.json — never print-and-discard.
        assert_eq!(resolve_json_out(None, None), "site/health.json");
    }

    #[test]
    fn json_out_env_overrides_default() {
        assert_eq!(resolve_json_out(Some("env.json".into()), None), "env.json");
    }

    #[test]
    fn json_flag_overrides_env_and_default() {
        assert_eq!(
            resolve_json_out(Some("env.json".into()), Some("flag.json".into())),
            "flag.json"
        );
        assert_eq!(
            resolve_json_out(None, Some("flag.json".into())),
            "flag.json"
        );
    }
}
