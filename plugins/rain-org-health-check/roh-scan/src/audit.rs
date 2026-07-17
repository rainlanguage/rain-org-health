//! Reads the audit skill's per-run stamp so the scan can report when each repo was
//! last *fully* audited. Pure parsing lives here and is unit-tested; the network
//! fetch is in main.rs.
//!
//! The stamp is an append-only `.audit/runs.jsonl` (JSON Lines) — one object per
//! audit run, newest last — so the repo keeps its whole audit history rather than
//! only the latest run. The recency is the LAST line whose scope is whole-repo.
//! `.audit/last-run.json` (a single object) is the earlier format and is still
//! read as a fallback during the transition.
//!
//! Accuracy hinges on the `scope` discriminator: the audit skill is also invoked
//! PR-scoped (the vetter/producer run it against a PR's changed files), and those
//! runs must NOT count as a whole-repo audit. So a line is honoured ONLY when
//! `scope == "whole-repo"`; every other scope (or a missing/malformed line) means
//! "not fully audited".

/// The canonical scope string a whole-repo audit stamp must carry. Any other value
/// (e.g. `pr:123`, `paths:src/foo`) is deliberately not a whole-repo audit.
pub const WHOLE_REPO_SCOPE: &str = "whole-repo";

#[derive(Debug, PartialEq, Eq)]
pub struct LastAudit {
    pub audited_at: String,
    pub audited_commit: String,
    pub skill_version: String,
    /// Whether first-party source has changed since the audit. `Some(true)` stale,
    /// `Some(false)` current, `None` if it couldn't be determined. In production
    /// this is set by `fetch_last_audit` from a `.audit/`-excluding tree compare
    /// (see [`source_changed_outside_audit`]); the parse functions only fill a
    /// naive SHA-equality preliminary when handed a `head_sha` (used by tests).
    pub stale: Option<bool>,
}

/// Parse `.audit/last-run.json`. Returns `Some` ONLY for a whole-repo stamp
/// (`scope == "whole-repo"`) that also carries `auditedAt` + `auditedCommit`;
/// any other scope, a missing required field, or malformed/empty JSON → `None`
/// (i.e. "not fully audited"). `head_sha` is the current branch HEAD, used only
/// to compute staleness.
pub fn parse_last_audit(body: &str, head_sha: Option<&str>) -> Option<LastAudit> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    // The scope gate: nothing but the exact whole-repo string counts.
    if v.get("scope").and_then(|s| s.as_str()) != Some(WHOLE_REPO_SCOPE) {
        return None;
    }
    let audited_at = v.get("auditedAt")?.as_str()?.to_string();
    let audited_commit = v.get("auditedCommit")?.as_str()?.to_string();
    if audited_at.is_empty() || audited_commit.is_empty() {
        return None;
    }
    let skill_version = v
        .get("skillVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let stale = head_sha.map(|h| h.trim() != audited_commit);
    Some(LastAudit {
        audited_at,
        audited_commit,
        skill_version,
        stale,
    })
}

/// Parse `.audit/runs.jsonl` (append-only JSON Lines, one run per line, newest
/// last) and return the LAST line that is a whole-repo stamp. Each line is parsed
/// with [`parse_last_audit`], so the scope gate and required fields are identical
/// to the single-object format; non-whole-repo lines (PR/paths scopes), blank
/// lines, and malformed lines are skipped rather than aborting the read — a bad
/// line must not erase a valid prior whole-repo audit. `None` when no line is a
/// whole-repo stamp (empty/absent file, or only scoped runs).
pub fn parse_runs_jsonl(body: &str, head_sha: Option<&str>) -> Option<LastAudit> {
    // Scan from the back: the last whole-repo line is the current recency, and
    // `next_back` stops at the first match rather than walking the whole history.
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| parse_last_audit(line, head_sha))
        .next_back()
}

/// Whether the audit is stale given the files changed between `auditedCommit` and
/// HEAD. Only changes **outside** `.audit/` count: the run's own stamp commit
/// advances HEAD while touching only `.audit/runs.jsonl` / `.audit/scope.json`, so
/// counting it would report every *fresh* audit as immediately stale. `true` iff
/// any changed path is first-party source (not under `.audit/`).
pub fn source_changed_outside_audit<'a>(changed_files: impl IntoIterator<Item = &'a str>) -> bool {
    changed_files
        .into_iter()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .any(|f| !f.starts_with(".audit/"))
}

/// Sort key for audit recency: never-audited repos first, then oldest audit
/// first, name as the final tiebreak — so the most overdue repos sort to the top.
pub fn audit_sort_key(last_audit: Option<&LastAudit>, name: &str) -> (u8, String, String) {
    match last_audit {
        None => (0, String::new(), name.to_string()),
        Some(a) => (1, a.audited_at.clone(), name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE: &str = r#"{
        "scope": "whole-repo",
        "auditedAt": "2026-07-07T22:00:00Z",
        "auditedCommit": "abc123def456",
        "skillVersion": "0.10.0",
        "fileCount": 42
    }"#;

    #[test]
    fn audit_sort_key_never_audited_first_then_oldest() {
        let mk = |at: &str| LastAudit {
            audited_at: at.into(),
            audited_commit: "x".into(),
            skill_version: String::new(),
            stale: None,
        };
        let older = mk("2026-01-01T00:00:00Z");
        let newer = mk("2026-06-01T00:00:00Z");
        let mut keys = [
            audit_sort_key(Some(&newer), "z-newer"),
            audit_sort_key(None, "a-never"),
            audit_sort_key(Some(&older), "m-older"),
        ];
        keys.sort();
        assert_eq!(keys[0].0, 0, "never-audited sorts first");
        assert_eq!(keys[1].1, "2026-01-01T00:00:00Z", "then the oldest audit");
        assert_eq!(keys[2].1, "2026-06-01T00:00:00Z", "then the newer audit");
    }

    #[test]
    fn whole_repo_stamp_parses() {
        let a = parse_last_audit(WHOLE, Some("abc123def456")).expect("should parse");
        assert_eq!(a.audited_at, "2026-07-07T22:00:00Z");
        assert_eq!(a.audited_commit, "abc123def456");
        assert_eq!(a.skill_version, "0.10.0");
        assert_eq!(a.stale, Some(false)); // audited commit == HEAD
    }

    #[test]
    fn stale_when_head_moved() {
        let a = parse_last_audit(WHOLE, Some("zzz999")).unwrap();
        assert_eq!(a.stale, Some(true));
    }

    #[test]
    fn stale_unknown_when_no_head() {
        let a = parse_last_audit(WHOLE, None).unwrap();
        assert_eq!(a.stale, None);
    }

    #[test]
    fn pr_scoped_stamp_is_not_a_whole_repo_audit() {
        let pr = r#"{"scope":"pr:123","auditedAt":"2026-07-07T22:00:00Z","auditedCommit":"abc"}"#;
        assert_eq!(parse_last_audit(pr, None), None);
    }

    #[test]
    fn path_scoped_stamp_is_not_a_whole_repo_audit() {
        let p =
            r#"{"scope":"paths:src/lib","auditedAt":"2026-07-07T22:00:00Z","auditedCommit":"abc"}"#;
        assert_eq!(parse_last_audit(p, None), None);
    }

    #[test]
    fn missing_scope_is_none() {
        let s = r#"{"auditedAt":"2026-07-07T22:00:00Z","auditedCommit":"abc"}"#;
        assert_eq!(parse_last_audit(s, None), None);
    }

    #[test]
    fn missing_required_field_is_none() {
        let s = r#"{"scope":"whole-repo","auditedAt":"2026-07-07T22:00:00Z"}"#; // no auditedCommit
        assert_eq!(parse_last_audit(s, None), None);
    }

    #[test]
    fn empty_or_malformed_is_none() {
        assert_eq!(parse_last_audit("", None), None);
        assert_eq!(parse_last_audit("not json", None), None);
        assert_eq!(parse_last_audit("{}", None), None);
    }

    // ---- runs.jsonl (append-only history) ----

    fn line(commit: &str, at: &str) -> String {
        format!(
            r#"{{"scope":"whole-repo","auditedAt":"{at}","auditedCommit":"{commit}","skillVersion":"0.15.0"}}"#
        )
    }

    #[test]
    fn jsonl_takes_the_last_whole_repo_line() {
        // Newest last: the second whole-repo run is the current recency.
        let body = format!(
            "{}\n{}\n",
            line("old111", "2026-01-01T00:00:00Z"),
            line("new222", "2026-06-01T00:00:00Z"),
        );
        let a = parse_runs_jsonl(&body, Some("new222")).expect("last whole-repo line");
        assert_eq!(a.audited_commit, "new222");
        assert_eq!(a.audited_at, "2026-06-01T00:00:00Z");
        assert_eq!(
            a.stale,
            Some(false),
            "staleness is vs the last line's commit"
        );
    }

    #[test]
    fn jsonl_skips_interleaved_scoped_and_blank_lines() {
        // A PR-scoped run and a blank line between two whole-repo runs must not be
        // mistaken for the latest whole-repo audit.
        let body = format!(
            "{}\n{}\n\n{}\n",
            line("whole1", "2026-01-01T00:00:00Z"),
            r#"{"scope":"pr:9","auditedAt":"2026-05-01T00:00:00Z","auditedCommit":"prc"}"#,
            line("whole2", "2026-03-01T00:00:00Z"),
        );
        let a = parse_runs_jsonl(&body, None).unwrap();
        assert_eq!(
            a.audited_commit, "whole2",
            "last WHOLE-REPO line, not the pr line"
        );
    }

    #[test]
    fn jsonl_malformed_trailing_line_does_not_erase_a_valid_audit() {
        // A corrupt final line must fall back to the last good whole-repo line, not
        // read as "never audited".
        let body = format!(
            "{}\n{{ this is not json\n",
            line("good333", "2026-04-01T00:00:00Z")
        );
        let a = parse_runs_jsonl(&body, None).expect("the good line still counts");
        assert_eq!(a.audited_commit, "good333");
    }

    #[test]
    fn jsonl_only_scoped_or_empty_is_none() {
        assert_eq!(parse_runs_jsonl("", None), None);
        assert_eq!(parse_runs_jsonl("\n\n", None), None);
        let scoped = r#"{"scope":"pr:1","auditedAt":"2026-01-01T00:00:00Z","auditedCommit":"a"}"#;
        assert_eq!(parse_runs_jsonl(scoped, None), None);
    }

    #[test]
    fn staleness_ignores_the_audit_stamp_commit() {
        // The fresh audit's stamp commit touches only .audit/ — NOT stale (this is
        // the case a bare auditedCommit != HEAD check gets wrong).
        assert!(!source_changed_outside_audit([
            ".audit/runs.jsonl",
            ".audit/scope.json"
        ]));
        // A first-party source change alongside the stamp — stale.
        assert!(source_changed_outside_audit([
            ".audit/runs.jsonl",
            "src/lib/Foo.sol"
        ]));
        // A non-.audit change on its own — stale.
        assert!(source_changed_outside_audit(["README.md"]));
        // Blank entries are skipped, not counted as source.
        assert!(!source_changed_outside_audit([
            "",
            "  ",
            ".audit/scope.json"
        ]));
        // No changes at all — not stale.
        assert!(!source_changed_outside_audit(Vec::<&str>::new()));
    }
}
