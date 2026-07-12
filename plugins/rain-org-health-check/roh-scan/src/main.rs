//! roh-scan — scan a GitHub org's repos for rainix/soldeer modernization-debt signals.
//! Signal detection lives in signals.rs (pure, tested); this file is the gh/network
//! orchestration and output rendering (text report + optional JSON).
//!
//! Usage:
//!   roh-scan [--json <path>] [repo ...]
//! Env: ORG (default rainlanguage), PAR (default 12), JSON_OUT (default site/health.json).

mod audit;
mod issues;
mod signals;
use audit::{audit_sort_key, parse_last_audit, LastAudit};
use issues::{
    age_days, classify_issues, issue_sort_key, parse_issue_node, parse_pr_closing_refs, IssueNode,
    OpenIssue,
};
use signals::{detect_signals, foundry_package_name, RepoInputs};

use serde_json::json;
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

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

/// GraphQL for the issue queue: every open issue with the fields the backlog
/// view shows. Paginated via gh's `--paginate` ($endCursor + pageInfo).
const OPEN_ISSUES_QUERY: &str = "query($owner:String!,$name:String!,$endCursor:String){repository(owner:$owner,name:$name){issues(states:OPEN,first:100,after:$endCursor){pageInfo{hasNextPage endCursor}nodes{number title createdAt labels(first:20){nodes{name}}assignees(first:5){nodes{login}}}}}}";

/// GraphQL for coverage: every open PR's `closingIssuesReferences`, with the
/// referenced issue's repo + owner (a PR can close an issue in another repo).
const OPEN_PRS_QUERY: &str = "query($owner:String!,$name:String!,$endCursor:String){repository(owner:$owner,name:$name){pullRequests(states:OPEN,first:100,after:$endCursor){pageInfo{hasNextPage endCursor}nodes{closingIssuesReferences(first:25){nodes{number repository{name owner{login}}}}}}}}";

/// Run a paginated GraphQL query and return `--jq`'s one-node-per-line output
/// (compact JSON objects) as lines for the pure parsers in issues.rs.
fn gh_graphql_nodes(org: &str, repo: &str, query: &str, jq: &str) -> Vec<String> {
    gh_stdout(&[
        "api",
        "graphql",
        "--paginate",
        "-f",
        &format!("owner={org}"),
        "-f",
        &format!("name={repo}"),
        "-f",
        &format!("query={query}"),
        "--jq",
        jq,
    ])
    .unwrap_or_default()
    .lines()
    .map(str::to_string)
    .collect()
}

/// Fetch one repo's open issues plus the org-wide (repo, issue number) pairs
/// its open PRs cover. Coverage is classified only after every repo is in,
/// because a PR here can close an issue elsewhere in the org.
fn fetch_issue_inputs(org: &str, repo: &str) -> (Vec<IssueNode>, Vec<(String, u64)>) {
    let issue_nodes = gh_graphql_nodes(
        org,
        repo,
        OPEN_ISSUES_QUERY,
        ".data.repository.issues.nodes[]",
    )
    .iter()
    .filter_map(|line| parse_issue_node(line))
    .collect();
    let closing_refs = gh_graphql_nodes(
        org,
        repo,
        OPEN_PRS_QUERY,
        ".data.repository.pullRequests.nodes[]",
    )
    .iter()
    .flat_map(|line| parse_pr_closing_refs(line, org))
    .collect();
    (issue_nodes, closing_refs)
}

/// One repo's scan result: modernization signals + last whole-repo audit (if
/// any) + its open issues and the issue coverage its open PRs provide.
struct RepoResult {
    name: String,
    signals: Vec<&'static str>,
    last_audit: Option<LastAudit>,
    issue_nodes: Vec<IssueNode>,
    closing_refs: Vec<(String, u64)>,
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
                let (issue_nodes, closing_refs) = fetch_issue_inputs(&org, repo);
                results.lock().unwrap().push(RepoResult {
                    name: repo.clone(),
                    signals,
                    last_audit,
                    issue_nodes,
                    closing_refs,
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

    let now = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // issue queue: coverage is org-wide (a PR may close an issue in another
    // repo), so pool every repo's closing refs before classifying any issue.
    let covered: HashSet<(String, u64)> = results
        .iter()
        .flat_map(|r| r.closing_refs.iter().cloned())
        .collect();
    let mut queue: Vec<OpenIssue> = results
        .iter()
        .flat_map(|r| classify_issues(&r.name, &r.issue_nodes, &covered))
        .collect();
    queue.sort_by_key(issue_sort_key);
    let uncovered = queue.iter().filter(|i| !i.covered).count();
    println!("\n================ open-issue queue (uncovered first) ===============");
    if queue.is_empty() {
        println!("  (no open issues)");
    }
    for i in &queue {
        let cov = if i.covered { "covered  " } else { "UNCOVERED" };
        let age = match age_days(&i.created_at, &now) {
            Some(d) => format!("{d:>4}d"),
            None => "   ?d".to_string(),
        };
        let id = format!("{}#{}", i.repo, i.number);
        println!("  {cov} {age}  {id:<34} {}", i.title);
    }
    println!("\nopen issues: {} ({uncovered} uncovered)", queue.len());

    // JSON output — always written (populate by default)
    {
        let path = json_out;
        // roh-scan is the producer of SCAN data only. It does NOT compute pipeline/FSM state and
        // does NOT call pr-review-report: the dashboard's FSM panel fetches issue-pr-cron's own
        // `human-queue.json` artifact at runtime (see CLAUDE.md — the dashboard is a consumer, not
        // a producer, of data). Do not re-add a `humanQueue` block to health.json here.
        let doc = json!({
            "generatedAt": now,
            "org": org,
            "totalRepos": total,
            "reposWithFindings": findings.len(),
            "reposWholeRepoAudited": audited,
            "reposNeverAudited": total - audited,
            "openIssues": queue.len(),
            "uncoveredIssues": uncovered,
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
            // pre-sorted uncovered-first, oldest-first — the canonical queue order
            "issueQueue": queue.iter().map(|i| json!({
                "repo": i.repo,
                "number": i.number,
                "title": i.title,
                "createdAt": i.created_at,
                "ageDays": age_days(&i.created_at, &now),
                "labels": i.labels,
                "assignee": i.assignee,
                "covered": i.covered,
            })).collect::<Vec<_>>(),
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
