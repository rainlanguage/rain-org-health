//! roh-scan — scan a GitHub org's repos for rainix/soldeer modernization-debt signals.
//! Signal detection lives in signals.rs (pure, tested); this file is the gh/network
//! orchestration and output rendering (text report + optional JSON).
//!
//! Usage:
//!   roh-scan [--json <path>] [repo ...]
//! Env: ORG (default rainlanguage), PAR (default 12), JSON_OUT (default site/health.json).

mod audit;
mod protofire;
mod signals;
use audit::{audit_sort_key, parse_last_audit, LastAudit};
use protofire::{
    classify_external_audit, count_lines, counts_as_source_drift, days_between, is_stale,
    newer_than, newest_pdf_index, parse_audited_tag, source_drift, source_loc_total,
    total_unaudited_source_loc, AuditPdf, CompareFile,
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

    // soldeer registry lookup, only when a package name exists
    let soldeer_published =
        foundry_package_name(&foundry).and_then(|pkg| soldeer_has_revision(&pkg));

    RepoInputs {
        workflows,
        foundry,
        soldeer_published,
    }
}

/// Read `.audit/last-run.json` and return the whole-repo audit stamp if present.
/// `None` when the repo has never had a whole-repo audit (no stamp, or only a
/// PR-/path-scoped one — see the `scope` gate in `audit::parse_last_audit`).
fn fetch_last_audit(org: &str, repo: &str) -> Option<LastAudit> {
    let body = gh_file(org, repo, ".audit/last-run.json");
    if body.trim().is_empty() {
        return None;
    }
    // Parse first with no HEAD: a PR-/path-scoped or malformed stamp returns None
    // here, so we skip the extra `commits/HEAD` API call in those cases (org scale).
    let mut audit = parse_last_audit(&body, None)?;
    // Confirmed a whole-repo stamp — now resolve HEAD to flag staleness.
    let head = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/commits/HEAD"),
        "--jq",
        ".sha",
    ]);
    audit.stale = head.as_deref().map(|h| h.trim() != audit.audited_commit);
    Some(audit)
}

/// One repo's assembled EXTERNAL (Protofire) audit situation. See `protofire.rs`
/// for the pure logic; this is the orchestrated result folded into `health.json`.
struct ProtofireResult {
    has_pdf: bool,
    external_audit: &'static str,
    pdfs: Vec<AuditPdf>,
    audited_ref: Option<String>,
    tag_convention_absent: bool,
    audited_date: String,
    latest_tag: Option<String>,
    latest_tag_iso: Option<String>,
    is_stale: bool,
    source_loc: Option<u64>,
    files_changed: Option<u64>,
    commits_since: Option<u64>,
    source_drift_truncated: bool,
    /// FULL non-test source LOC of a NEVER-externally-audited repo (the dominant
    /// term of the unaudited-LOC headline). `None` for audited repos, whose
    /// out-of-coverage code is the `source_loc` drift instead.
    total_source_loc: Option<u64>,
    /// The git-tree API truncated on a very large repo → `total_source_loc` is a
    /// lower bound (mirrors `source_drift_truncated`).
    total_source_loc_truncated: bool,
}

/// List a repo directory via the contents API → (type, path, name) rows. `None`
/// on 404 / error (e.g. the directory doesn't exist).
fn gh_contents_entries(org: &str, repo: &str, path: &str) -> Option<Vec<(String, String, String)>> {
    let out = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/contents/{path}"),
        "--jq",
        ".[]|[.type,.path,.name]|@tsv",
    ])?;
    Some(
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
fn collect_audit_pdfs(
    org: &str,
    repo: &str,
    dir: &str,
    depth: u8,
    acc: &mut Vec<(String, String)>,
) {
    if depth == 0 {
        return;
    }
    let Some(entries) = gh_contents_entries(org, repo, dir) else {
        return;
    };
    for (ty, path, name) in entries {
        if ty == "file" {
            if name.to_ascii_lowercase().ends_with(".pdf") {
                acc.push((name, path));
            }
        } else if ty == "dir" {
            collect_audit_pdfs(org, repo, &path, depth - 1, acc);
        }
    }
}

/// Commit date + sha of the commit that last touched (added) a path.
fn pdf_commit(org: &str, repo: &str, path: &str) -> (String, String) {
    let out = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/commits?path={path}&per_page=1"),
        "--jq",
        "[.[0].commit.committer.date // \"\", .[0].sha // \"\"]|@tsv",
    ])
    .unwrap_or_default();
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
fn repo_default_and_latest_tag(org: &str, repo: &str) -> (String, Option<String>, Option<String>) {
    const Q: &str = "query($o:String!,$n:String!){repository(owner:$o,name:$n){defaultBranchRef{name} refs(refPrefix:\"refs/tags/\",orderBy:{field:TAG_COMMIT_DATE,direction:DESC},first:1){nodes{name target{__typename ... on Commit{committedDate} ... on Tag{target{... on Commit{committedDate}}}}}}}}";
    const JQ: &str = ".data.repository | [(.defaultBranchRef.name // \"\"), (.refs.nodes[0].name // \"\"), (.refs.nodes[0].target.committedDate // .refs.nodes[0].target.target.committedDate // \"\")] | @tsv";
    let out = gh_stdout(&[
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
    ])
    .unwrap_or_default();
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
fn fetch_compare(
    org: &str,
    repo: &str,
    base: &str,
    head: &str,
) -> Option<(String, Vec<CompareFile>, u64, bool)> {
    let raw = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/compare/{base}...{head}"),
    ])?;
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
                })
                .collect()
        })
        .unwrap_or_default();
    let truncated = files.len() >= 300;
    Some((base_date, files, total, truncated))
}

/// The repo's default branch — the tree-ish for a full-repo source count. Falls
/// back to "main" when the field is missing/unreadable, mirroring the branch
/// fallback in `repo_default_and_latest_tag`.
fn default_branch(org: &str, repo: &str) -> String {
    gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}"),
        "--jq",
        ".default_branch",
    ])
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "main".to_string())
}

/// Line count of one blob via the raw media type (no base64 round-trip). `None` on
/// any failure (404, network). Binary blobs are never asked for — the caller only
/// passes source-file SHAs.
fn gh_blob_line_count(org: &str, repo: &str, sha: &str) -> Option<u64> {
    let out = Command::new("gh")
        .args([
            "api",
            &format!("repos/{org}/{repo}/git/blobs/{sha}"),
            "-H",
            "Accept: application/vnd.github.raw",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(count_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// A repo's FULL non-test source LOC, clone-free — the count the scan does NOT
/// otherwise compute (drift only exists for audited repos). Enumerate the default
/// branch's git tree in one recursive call, keep the blobs the shared
/// `counts_as_source_drift` predicate accepts (source AND not a test — the SAME
/// definition as the drift columns), and count each one's lines via the raw blob
/// API. Filtering BEFORE fetching means only source blobs cost a request.
///
/// Returns `(loc, source_files, truncated)`. Like the compare API's 300-file cap,
/// the git-tree API silently truncates on very large repos (its own `truncated`
/// flag) → the LOC is then a lower bound, surfaced like `sourceDriftTruncated`.
fn fetch_source_loc_total(org: &str, repo: &str) -> (u64, u64, bool) {
    let branch = default_branch(org, repo);
    let Some(raw) = gh_stdout(&[
        "api",
        &format!("repos/{org}/{repo}/git/trees/{branch}?recursive=1"),
    ]) else {
        return (0, 0, false);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (0, 0, false);
    };
    let truncated = v["truncated"].as_bool().unwrap_or(false);
    let mut collected: Vec<(String, u64)> = Vec::new();
    if let Some(entries) = v["tree"].as_array() {
        for e in entries {
            if e["type"].as_str() != Some("blob") {
                continue;
            }
            let path = e["path"].as_str().unwrap_or("");
            if !counts_as_source_drift(path) {
                continue;
            }
            let sha = e["sha"].as_str().unwrap_or("");
            if sha.is_empty() {
                continue;
            }
            if let Some(n) = gh_blob_line_count(org, repo, sha) {
                collected.push((path.to_string(), n));
            }
        }
    }
    let (loc, files) = source_loc_total(&collected);
    (loc, files, truncated)
}

/// Assemble a repo's EXTERNAL (Protofire) audit situation: enumerate
/// `audit/protofire/` PDFs, pick the newest as the reference, parse its tag (or
/// fall back to its commit), find the newest tag, and quantify source-LOC drift
/// base…HEAD — all clone-free. No PDF ⇒ `never` (the coverage gap), returned cheaply.
fn fetch_protofire_audit(org: &str, repo: &str) -> ProtofireResult {
    let mut pdf_paths: Vec<(String, String)> = Vec::new();
    collect_audit_pdfs(org, repo, "audit/protofire", 2, &mut pdf_paths);
    if pdf_paths.is_empty() {
        // Never externally audited: its FULL non-test source LOC is the coverage
        // gap (the dominant term of the unaudited-LOC headline), so count it here.
        let (total_source_loc, _files, trunc) = fetch_source_loc_total(org, repo);
        return ProtofireResult {
            has_pdf: false,
            external_audit: protofire::NEVER,
            pdfs: Vec::new(),
            audited_ref: None,
            tag_convention_absent: false,
            audited_date: String::new(),
            latest_tag: None,
            latest_tag_iso: None,
            is_stale: false,
            source_loc: None,
            files_changed: None,
            commits_since: None,
            source_drift_truncated: false,
            total_source_loc: Some(total_source_loc),
            total_source_loc_truncated: trunc,
        };
    }
    // Resolve each PDF's commit date + sha, then order by path for stable output.
    let mut pdfs: Vec<AuditPdf> = pdf_paths
        .into_iter()
        .map(|(filename, path)| {
            let (iso, sha) = pdf_commit(org, repo, &path);
            AuditPdf {
                filename,
                path,
                last_commit_iso: iso,
                commit_sha: sha,
            }
        })
        .collect();
    pdfs.sort_by(|a, b| a.path.cmp(&b.path));
    let newest = newest_pdf_index(&pdfs).expect("non-empty");
    let audited_ref = parse_audited_tag(&pdfs[newest].filename);
    let tag_convention_absent = audited_ref.is_none();

    let (default_branch, latest_tag, latest_tag_iso) = repo_default_and_latest_tag(org, repo);

    // Drift base: the audited tag when the filename encodes one, else the newest
    // PDF's own commit (the task's fallback when the tag convention is absent).
    let base = audited_ref
        .clone()
        .unwrap_or_else(|| pdfs[newest].commit_sha.clone());
    let cmp = if base.is_empty() {
        None
    } else {
        fetch_compare(org, repo, &base, &default_branch)
    };

    let (audited_date, source_loc, files_changed, commits_since, source_drift_truncated) =
        match &cmp {
            Some((base_date, files, total, trunc)) => {
                let (loc, n) = source_drift(files);
                (base_date.clone(), Some(loc), Some(n), Some(*total), *trunc)
            }
            // Compare unavailable → date the audit by the PDF's own commit, drift unknown.
            None => (
                pdfs[newest].last_commit_iso.clone(),
                None,
                None,
                None,
                false,
            ),
        };

    let has_tags = latest_tag.is_some();
    let newer_tag_exists = latest_tag_iso
        .as_deref()
        .map(|t| newer_than(t, &audited_date))
        .unwrap_or(false);
    let external_audit = classify_external_audit(true, has_tags, newer_tag_exists);
    let stale = is_stale(newer_tag_exists, source_loc.unwrap_or(0));

    ProtofireResult {
        has_pdf: true,
        external_audit,
        pdfs,
        audited_ref,
        tag_convention_absent,
        audited_date,
        latest_tag,
        latest_tag_iso,
        is_stale: stale,
        source_loc,
        files_changed,
        commits_since,
        source_drift_truncated,
        // Audited repos' out-of-coverage code is the `source_loc` drift, not a
        // full-repo count — the full-source LOC pass is reserved for never-audited.
        total_source_loc: None,
        total_source_loc_truncated: false,
    }
}

/// One repo's scan result: modernization signals, last whole-repo audit (if any),
/// and the external (Protofire) audit situation.
struct RepoResult {
    name: String,
    signals: Vec<&'static str>,
    last_audit: Option<LastAudit>,
    protofire: ProtofireResult,
}

/// Query the soldeer registry for a published revision. Some(true/false), None on error.
fn soldeer_has_revision(pkg: &str) -> Option<bool> {
    let url =
        format!("https://api.soldeer.xyz/api/v1/revision?project_name={pkg}&offset=0&limit=1");
    let out = Command::new("curl").args(["-fsSL", &url]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let data = v.get("data")?;
    Some(data.as_array().map(|a| !a.is_empty()).unwrap_or(false))
}

/// Resolve where the dashboard JSON is written. A bare run POPULATES `site/health.json` (the scan
/// never print-and-discards by default); `JSON_OUT` overrides the default; `--json <path>` overrides both.
fn resolve_json_out(json_out_env: Option<String>, json_flag: Option<String>) -> String {
    json_flag
        .or(json_out_env)
        .unwrap_or_else(|| "site/health.json".into())
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
    let org = std::env::var("ORG").unwrap_or_else(|_| "rainlanguage".into());
    let par: usize = std::env::var("PAR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);

    let repos: Vec<String> = if !repos_arg.is_empty() {
        repos_arg
    } else {
        let mut v: Vec<String> = gh_stdout(&[
            "repo",
            "list",
            &org,
            "--no-archived",
            "--limit",
            "300",
            "--json",
            "name,isFork",
            "-q",
            ".[]|select(.isFork==false)|.name",
        ])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
        v.sort();
        v
    };
    let total = repos.len();
    eprintln!("Scanning {total} {org} repos (parallel={par})...");

    // bounded-concurrency fan-out over repos
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<RepoResult>> = Mutex::new(Vec::new());
    let nworkers = par.clamp(1, total.max(1));
    std::thread::scope(|s| {
        for _ in 0..nworkers {
            s.spawn(|| loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= repos.len() {
                    break;
                }
                let repo = &repos[idx];
                let signals = detect_signals(&fetch_inputs(&org, repo));
                let last_audit = fetch_last_audit(&org, repo);
                let protofire = fetch_protofire_audit(&org, repo);
                results.lock().unwrap().push(RepoResult {
                    name: repo.clone(),
                    signals,
                    last_audit,
                    protofire,
                });
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    // findings view (owned) so we can re-sort `results` for audit recency afterwards
    let mut findings: Vec<(String, Vec<&'static str>)> = results
        .iter()
        .filter(|r| !r.signals.is_empty())
        .map(|r| (r.name.clone(), r.signals.clone()))
        .collect();
    findings.sort_by(|a, b| (b.1.len(), &a.0).cmp(&(a.1.len(), &b.0)));

    // text report
    println!("\n================ rain org health: per-repo findings ================");
    if findings.is_empty() {
        println!("  (no findings — all clean)");
    } else {
        for (repo, sigs) in &findings {
            println!("  {:<30} {}", repo, sigs.join(" "));
        }
    }
    println!("\n================ org-wide summary (repos affected) =================");
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, sigs) in &findings {
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
    let externally_audited = results.iter().filter(|r| r.protofire.has_pdf).count();

    // Unaudited source LOC (the headline): never-audited repos' FULL source LOC
    // (the bulk) + audited repos' drift since the audited tag (already out of
    // coverage). Both use the same non-test source predicate as the drift columns.
    let never_audited_bulk: u64 = results
        .iter()
        .filter(|r| !r.protofire.has_pdf)
        .map(|r| r.protofire.total_source_loc.unwrap_or(0))
        .sum();
    let drifted_since_audit: u64 = results
        .iter()
        .filter(|r| r.protofire.has_pdf)
        .map(|r| r.protofire.source_loc.unwrap_or(0))
        .sum();
    let total_unaudited_loc = total_unaudited_source_loc(never_audited_bulk, drifted_since_audit);
    let never_audited_loc_truncated = results
        .iter()
        .filter(|r| !r.protofire.has_pdf)
        .any(|r| r.protofire.total_source_loc_truncated);
    let drift_loc_truncated = results
        .iter()
        .filter(|r| r.protofire.has_pdf)
        .any(|r| r.protofire.source_drift_truncated);
    let unaudited_loc_lower_bound = never_audited_loc_truncated || drift_loc_truncated;

    let mut pf_view: Vec<&RepoResult> = results.iter().collect();
    pf_view.sort_by(|a, b| {
        let (pa, pb) = (&a.protofire, &b.protofire);
        (
            pa.has_pdf,
            std::cmp::Reverse(pa.is_stale),
            std::cmp::Reverse(pa.source_loc.unwrap_or(0)),
            &a.name,
        )
            .cmp(&(
                pb.has_pdf,
                std::cmp::Reverse(pb.is_stale),
                std::cmp::Reverse(pb.source_loc.unwrap_or(0)),
                &b.name,
            ))
    });
    println!("\n============ external audit coverage (Protofire PDFs under audit/protofire/) ====");
    println!("  externally audited:       {externally_audited} / {total}");
    println!(
        "  NEVER externally audited: {} / {total}  (the coverage gap)",
        total - externally_audited
    );
    println!(
        "  total unaudited source LOC: {total_unaudited_loc}{lb}  ({never_audited_bulk} never-audited bulk + {drifted_since_audit} drifted since audit)",
        lb = if unaudited_loc_lower_bound { "+" } else { "" }
    );
    for r in &pf_view {
        let p = &r.protofire;
        if !p.has_pdf {
            continue;
        }
        let refd = p.audited_ref.as_deref().unwrap_or("(no tag in PDF name)");
        let latest = p.latest_tag.as_deref().unwrap_or("(no tags)");
        let drift = match (p.source_loc, p.files_changed, p.commits_since) {
            (Some(loc), Some(files), Some(commits)) => {
                let days = days_between(&p.audited_date, &now).unwrap_or(-1);
                let trunc = if p.source_drift_truncated { "+" } else { "" };
                format!("{loc}{trunc} src LOC / {files} files / {commits} commits · {days}d")
            }
            _ => "drift unavailable".to_string(),
        };
        let flag = if p.tag_convention_absent {
            "  [tag convention absent]"
        } else {
            ""
        };
        println!(
            "  {:<28} {:<8} audited {refd} → latest {latest} · {drift}{flag}",
            r.name, p.external_audit
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
        let doc = json!({
            "generatedAt": now,
            "org": org,
            "totalRepos": total,
            "reposWithFindings": findings.len(),
            "reposWholeRepoAudited": audited,
            "reposNeverAudited": total - audited,
            "reposExternallyAudited": externally_audited,
            "reposNeverExternallyAudited": total - externally_audited,
            // Headline (#37): total non-test source LOC not covered by a current audit,
            // with the never-audited-bulk vs. drifted-since-audit split. `lowerBound` is
            // true when any git-tree or compare page truncated on a very large repo.
            "unauditedSourceLoc": {
                "total": total_unaudited_loc,
                "neverAuditedBulk": never_audited_bulk,
                "driftedSinceAudit": drifted_since_audit,
                "neverAuditedTruncated": never_audited_loc_truncated,
                "driftTruncated": drift_loc_truncated,
                "lowerBound": unaudited_loc_lower_bound,
            },
            "summary": summary.iter().map(|(s, n)| (s.to_string(), serde_json::Value::from(*n))).collect::<serde_json::Map<String, serde_json::Value>>(),
            "repos": findings.iter().map(|(r, sigs)| json!({"name": r, "signals": sigs})).collect::<Vec<_>>(),
            "audits": results.iter().map(|r| match &r.last_audit {
                None => json!({ "name": r.name, "lastAudit": serde_json::Value::Null }),
                Some(a) => json!({ "name": r.name, "lastAudit": {
                    "auditedAt": a.audited_at,
                    "auditedCommit": a.audited_commit,
                    "skillVersion": a.skill_version,
                    "stale": a.stale,
                }}),
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
                    "hasProtofireAudit": p.has_pdf,
                    "externalAudit": p.external_audit,
                    "auditPdfs": p.pdfs.iter().map(|pdf| json!({
                        "filename": pdf.filename,
                        "path": pdf.path,
                        "lastCommitIso": pdf.last_commit_iso,
                    })).collect::<Vec<_>>(),
                    "auditedRef": p.audited_ref,
                    "tagConventionAbsent": p.tag_convention_absent,
                    "auditedDate": if p.audited_date.is_empty() { serde_json::Value::Null } else { serde_json::Value::from(p.audited_date.clone()) },
                    "latestTag": p.latest_tag,
                    "latestTagIso": p.latest_tag_iso,
                    "isStale": if p.has_pdf { serde_json::Value::from(p.is_stale) } else { serde_json::Value::Null },
                    "sourceLocChangedSinceAudit": p.source_loc,
                    "filesChangedSinceAudit": p.files_changed,
                    "commitsSinceAudit": p.commits_since,
                    "sourceDriftTruncated": p.source_drift_truncated,
                    "daysSinceAudit": days,
                    // Never-audited repos only: FULL non-test source LOC (the bulk term);
                    // null for audited repos, which report `sourceLocChangedSinceAudit` drift.
                    "totalSourceLoc": p.total_source_loc,
                    "totalSourceLocTruncated": p.total_source_loc_truncated,
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
    use super::resolve_json_out;

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
