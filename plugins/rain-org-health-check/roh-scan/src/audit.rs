//! Reads the audit skill's per-run stamp (`.audit/last-run.json`) so the scan can
//! report when each repo was last *fully* audited. Pure parsing lives here and is
//! unit-tested; the network fetch is in main.rs.
//!
//! Accuracy hinges on the `scope` discriminator: the audit skill is also invoked
//! PR-scoped (the vetter/producer run it against a PR's changed files), and those
//! runs must NOT count as a whole-repo audit. So a stamp is honoured ONLY when
//! `scope == "whole-repo"`; every other scope (or a missing/malformed stamp) means
//! "not fully audited".

/// The canonical scope string a whole-repo audit stamp must carry. Any other value
/// (e.g. `pr:123`, `paths:src/foo`) is deliberately not a whole-repo audit.
pub const WHOLE_REPO_SCOPE: &str = "whole-repo";

#[derive(Debug, PartialEq, Eq)]
pub struct LastAudit {
    pub audited_at: String,
    pub audited_commit: String,
    pub skill_version: String,
    /// `Some(true)` if the audited commit is no longer the branch HEAD (audit is
    /// stale); `Some(false)` if it still is; `None` if HEAD couldn't be resolved.
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
        let p = r#"{"scope":"paths:src/lib","auditedAt":"2026-07-07T22:00:00Z","auditedCommit":"abc"}"#;
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
}
