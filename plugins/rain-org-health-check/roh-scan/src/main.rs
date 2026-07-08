//! roh-scan — scan a GitHub org's repos for rainix/soldeer modernization-debt signals.
//! Signal detection lives in signals.rs (pure, tested); this file is the gh/network
//! orchestration and output rendering (text report + optional JSON).
//!
//! Usage:
//!   roh-scan [--json <path>] [repo ...]
//! Env: ORG (default rainlanguage), PAR (default 12), JSON_OUT (default site/health.json).

mod signals;
use signals::{detect_signals, foundry_package_name, RepoInputs};

use serde_json::json;
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut json_out: Option<String> = std::env::var("JSON_OUT").ok();
    let mut repos_arg: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json_out = args.get(i + 1).cloned().or(json_out);
                i += 2;
            }
            r => {
                repos_arg.push(r.to_string());
                i += 1;
            }
        }
    }
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
    let findings: Mutex<Vec<(String, Vec<&'static str>)>> = Mutex::new(Vec::new());
    let nworkers = par.clamp(1, total.max(1));
    std::thread::scope(|s| {
        for _ in 0..nworkers {
            s.spawn(|| loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= repos.len() {
                    break;
                }
                let repo = &repos[idx];
                let sigs = detect_signals(&fetch_inputs(&org, repo));
                if !sigs.is_empty() {
                    findings.lock().unwrap().push((repo.clone(), sigs));
                }
            });
        }
    });

    let mut findings = findings.into_inner().unwrap();
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

    // JSON output
    if let Some(path) = json_out {
        let now = Command::new("date")
            .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let doc = json!({
            "generatedAt": now,
            "org": org,
            "totalRepos": total,
            "reposWithFindings": findings.len(),
            "summary": summary.iter().map(|(s, n)| (s.to_string(), serde_json::Value::from(*n))).collect::<serde_json::Map<String, serde_json::Value>>(),
            "repos": findings.iter().map(|(r, sigs)| json!({"name": r, "signals": sigs})).collect::<Vec<_>>(),
        });
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).is_ok() {
            eprintln!("wrote {path} ({} repos with findings)", findings.len());
        }
    }
}
