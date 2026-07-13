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
    changed_source_file_count, classify_anchor, classify_external_audit, counts_as_source_drift,
    days_between, is_stale, newer_than, newest_pdf_index, source_drift, AuditAnchor, AuditPdf,
    CompareFile,
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
    anchor_kind: Option<&'static str>,
    tag_convention_absent: bool,
    audited_date: String,
    latest_tag: Option<String>,
    latest_tag_iso: Option<String>,
    is_stale: bool,
    source_loc: Option<u64>,
    source_loc_added: Option<u64>,
    source_loc_removed: Option<u64>,
    files_changed: Option<u64>,
    commits_since: Option<u64>,
    source_drift_truncated: bool,
    compare_url: Option<String>,
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
                })
                .collect()
        })
        .unwrap_or_default();
    let truncated = files.len() >= 300;
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
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some(".git") {
                    walk(&path, root, acc);
                }
            } else if let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) {
                if counts_as_source_drift(rel) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        *acc += content.lines().count() as u64;
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
    let ok = Command::new("git")
        .args([
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
fn empty_protofire(state: &'static str) -> ProtofireResult {
    ProtofireResult {
        has_pdf: false,
        external_audit: state,
        pdfs: Vec::new(),
        audited_ref: None,
        anchor_kind: None,
        tag_convention_absent: false,
        audited_date: String::new(),
        latest_tag: None,
        latest_tag_iso: None,
        is_stale: false,
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
            protofire::UNKNOWN
        } else {
            protofire::NEVER
        });
    }
    // Resolve each PDF's commit date + sha, then order by path for stable output.
    let mut pdfs: Vec<AuditPdf> = pdf_paths
        .into_iter()
        .map(|(filename, path)| {
            let (iso, sha) = pdf_commit(gh, org, repo, &path);
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
        ),
        Some((base_date, files, total, _)) => {
            let (added, removed, n) = source_drift(files);
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
        ),
    };

    let has_tags = latest_tag.is_some();
    let newer_tag_exists = latest_tag_iso
        .as_deref()
        .map(|t| newer_than(t, &audited_date))
        .unwrap_or(false);
    let external_audit = classify_external_audit(true, has_tags, newer_tag_exists);
    let stale = is_stale(newer_tag_exists, source_loc.unwrap_or(0));

    // The GitHub compare-view URL for the audited drift (base…default branch);
    // None when either ref is empty, so the panel links only when it resolves.
    let compare_url = protofire::compare_url(org, repo, &base, &default_branch);

    ProtofireResult {
        has_pdf: true,
        external_audit,
        pdfs,
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
        compare_url,
    }
}

/// One repo's scan result: modernization signals, last whole-repo audit (if any),
/// and the external (Protofire) audit situation.
struct RepoResult {
    name: String,
    signals: Vec<&'static str>,
    last_audit: Option<LastAudit>,
    protofire: ProtofireResult,
    has_foundry: bool,
    /// FULL non-test `.sol` LOC (#37), counted for never-audited Solidity repos;
    /// `None` for audited/non-Solidity repos (audited repos use drift instead).
    full_source_loc: Option<u64>,
}

/// Whether a repo belongs in the Protofire external-audit report. A Protofire
/// audit is a Solidity audit, so the report only concerns Foundry/Solidity
/// projects (proxied by a `foundry.toml`) plus anything already carrying a PDF; a
/// repo with neither (docs, subgraph, tooling, `.github`) is not a coverage gap
/// and must not inflate the never-audited count (issue #54).
fn in_protofire_report(has_pdf: bool, has_foundry: bool) -> bool {
    has_pdf || has_foundry
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

/// Fan `work` out over `repos` across up to `par` worker threads (work-stealing
/// via a shared cursor) and return a result for EVERY repo. Each result is
/// produced by `work` and carries its own repo identity, so completeness and the
/// result SET are independent of `par` — `par=1` and `par=N` yield the same set.
fn scan_repos<T, F>(repos: Vec<String>, par: usize, work: F) -> Vec<T>
where
    F: Fn(&str) -> T + Sync,
    T: Send,
{
    let total = repos.len();
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
                if idx >= repos.len() {
                    break;
                }
                let out = work(&repos[idx]);
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

    // One shared `gh` fetcher borrowed by every worker (`GhApi: Sync`). It retries
    // the secondary-rate-limit failure that `par` concurrent subprocesses provoke,
    // so a transient error surfaces as `unknown`, never a false `never`.
    let gh = GhCli::new();
    let mut results: Vec<RepoResult> = scan_repos(repos, par, |repo| {
        let inputs = fetch_inputs(&org, repo);
        // A repo counts toward the Protofire (Solidity-audit) report only if it is a
        // Foundry/Solidity project — proxied by a foundry.toml, which fetch_inputs
        // already retrieves, so this gate adds no extra request (#54).
        let has_foundry = !inputs.foundry.trim().is_empty();
        let signals = detect_signals(&inputs);
        let last_audit = fetch_last_audit(&org, repo);
        let protofire = fetch_protofire_audit(&gh, &org, repo);
        // #37: a never-audited Solidity repo's FULL non-test .sol LOC (via a shallow
        // clone) quantifies the coverage gap; audited repos use their drift instead.
        let full_source_loc = if has_foundry && !protofire.has_pdf {
            count_source_loc(&org, repo)
        } else {
            None
        };
        RepoResult {
            name: repo.to_string(),
            signals,
            last_audit,
            protofire,
            has_foundry,
            full_source_loc,
        }
    });
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
            "reposNeverExternallyAudited": protofire_total - externally_audited,
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
                    "anchorKind": p.anchor_kind,
                    "tagConventionAbsent": p.tag_convention_absent,
                    "auditedDate": if p.audited_date.is_empty() { serde_json::Value::Null } else { serde_json::Value::from(p.audited_date.clone()) },
                    "latestTag": p.latest_tag,
                    "latestTagIso": p.latest_tag_iso,
                    "isStale": if p.has_pdf { serde_json::Value::from(p.is_stale) } else { serde_json::Value::Null },
                    "sourceLocChangedSinceAudit": p.source_loc,
                    "fullSourceLoc": r.full_source_loc,
                    "sourceLocAddedSinceAudit": p.source_loc_added,
                    "sourceLocRemovedSinceAudit": p.source_loc_removed,
                    "filesChangedSinceAudit": p.files_changed,
                    "commitsSinceAudit": p.commits_since,
                    "sourceDriftTruncated": p.source_drift_truncated,
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
        let loc = sum_sol_loc(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            loc, 5,
            "3 (src/A.sol) + 2 (deploy/D.sol); tests, README, .git excluded"
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
        assert_eq!(r.external_audit, protofire::NA);
        assert_ne!(r.external_audit, protofire::NEVER);
        assert_ne!(r.external_audit, protofire::UNKNOWN);
    }

    #[test]
    fn not_found_listing_is_never_audited() {
        // A genuine 404 on the top-level listing → genuinely absent → `never`.
        let gh = FakeGh::new(vec![("contents/audit/protofire", FetchOutcome::NotFound)]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        assert!(!r.has_pdf);
        assert_eq!(r.external_audit, protofire::NEVER);
    }

    #[test]
    fn failed_listing_is_unknown_never_a_false_never() {
        // THE #52 FIX: a FAILED top-level listing (rate-limit/network after retries)
        // must classify as `unknown`, never the false coverage claim `never`. This
        // assertion fails against the pre-fix code, which returned `never` here.
        let gh = FakeGh::new(vec![("contents/audit/protofire", FetchOutcome::Failed)]);
        let r = fetch_protofire_audit(&gh, "rainlanguage", "example");
        assert!(!r.has_pdf);
        assert_eq!(r.external_audit, protofire::UNKNOWN);
        assert_ne!(
            r.external_audit,
            protofire::NEVER,
            "a failed fetch must NEVER be reported as never-audited"
        );
        // The listing was fetched exactly once at this seam (retry lives in GhCli).
        assert_eq!(gh.count("contents/audit/protofire"), 1);
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
                protofire::UNKNOWN
            } else {
                protofire::CURRENT
            };
            (r.to_string(), state)
        });
        assert_eq!(out.len(), repos.len(), "every repo produced a result");
        for (name, state) in &out {
            if name == "r3" {
                assert_eq!(*state, protofire::UNKNOWN, "only r3 is unknown");
            } else {
                assert_eq!(*state, protofire::CURRENT, "{name} must stay current");
            }
        }
        let unknowns = out.iter().filter(|(_, s)| *s == protofire::UNKNOWN).count();
        assert_eq!(unknowns, 1, "exactly one repo is unknown — no leakage");
    }

    #[test]
    fn scan_repos_empty_input_is_empty() {
        let out: Vec<(String, &'static str)> = scan_repos(Vec::new(), 4, |r| (r.to_string(), "x"));
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
