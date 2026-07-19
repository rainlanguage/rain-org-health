//! Reads the adversarial-mutation-test skill's per-run record so the scan can
//! report when each repo was last mutation-tested, and at which commit. Pure
//! parsing lives here and is unit-tested; the network fetch is in main.rs.
//!
//! The record is `audit/mutation-test-scans.json` — a JSON ARRAY of run objects,
//! the convention already carried by rainlanguage/raindex,
//! S01-Issuer/st0x.atomic-bridge and rainlanguage/rain.math.binary. Each entry
//! holds at least `timestamp`, `commit`, `scope`, `tool` and `skillVersion`; the
//! `summary` sub-shape varies between repos and is deliberately not parsed here.
//!
//! The array is NOT chronologically ordered — raindex's entries interleave, and
//! its LAST element is a full day older than its newest — so the latest run is
//! chosen by maximum timestamp, never by position. Reading the last element
//! would silently under-report recency on exactly the repo with the most runs.

/// The newest adversarial-mutation run recorded for a repo.
#[derive(Debug, PartialEq, Eq)]
pub struct LastMutation {
    pub timestamp: String,
    /// The commit the run was performed against. `None` when the entry omits it
    /// — reported as unknown rather than guessed from a neighbouring entry.
    pub commit: Option<String>,
    pub skill_version: String,
    pub scope: String,
}

/// Parse `audit/mutation-test-scans.json` → the newest run by timestamp.
/// `None` when the file is absent, malformed, not an array, or holds no entry
/// carrying a timestamp — never a fabricated "never ran".
pub fn parse_mutation_scans(src: &str) -> Option<LastMutation> {
    let parsed: serde_json::Value = serde_json::from_str(src).ok()?;
    let entries = parsed.as_array()?;
    entries
        .iter()
        .filter_map(|entry| {
            // A timestamp is the only field required to rank a run; everything
            // else degrades to unknown rather than dropping the entry, so a
            // newest-but-sparse record still wins.
            let timestamp = entry["timestamp"].as_str()?.to_string();
            Some(LastMutation {
                timestamp,
                commit: entry["commit"].as_str().map(str::to_string),
                skill_version: entry["skillVersion"].as_str().unwrap_or("").to_string(),
                scope: entry["scope"].as_str().unwrap_or("").to_string(),
            })
        })
        // RFC3339 UTC (`…Z`) timestamps of one width sort lexicographically.
        .max_by(|a, b| a.timestamp.cmp(&b.timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real raindex shape: entries are NOT in chronological order, and the
    /// LAST element is older than the newest. Position must not decide.
    const UNORDERED: &str = r#"[
        {"timestamp":"2026-06-06T10:05:38Z","commit":"08d547fa","skillVersion":"0.24.0","scope":"whole repo"},
        {"timestamp":"2026-06-11T07:00:00Z","commit":"44590fcf","skillVersion":"0.25.0","scope":"newest"},
        {"timestamp":"2026-06-10T00:00:00Z","commit":"137ba349","skillVersion":"0.24.0","scope":"last but older"}
    ]"#;

    #[test]
    fn picks_the_newest_by_timestamp_not_by_position() {
        let got = parse_mutation_scans(UNORDERED).unwrap();
        assert_eq!(got.timestamp, "2026-06-11T07:00:00Z");
        assert_eq!(got.commit.as_deref(), Some("44590fcf"));
        assert_eq!(got.scope, "newest");
        assert_eq!(got.skill_version, "0.25.0");
    }

    #[test]
    fn single_entry_is_that_entry() {
        let src = r#"[{"timestamp":"2026-07-18T01:50:46Z","commit":"208336a2","skillVersion":"0.27.0","scope":"change-only"}]"#;
        let got = parse_mutation_scans(src).unwrap();
        assert_eq!(got.commit.as_deref(), Some("208336a2"));
        assert_eq!(got.timestamp, "2026-07-18T01:50:46Z");
    }

    #[test]
    fn a_missing_commit_is_unknown_not_borrowed_from_another_entry() {
        let src = r#"[
            {"timestamp":"2026-01-01T00:00:00Z","commit":"aaaaaaa"},
            {"timestamp":"2026-02-01T00:00:00Z"}
        ]"#;
        let got = parse_mutation_scans(src).unwrap();
        assert_eq!(got.timestamp, "2026-02-01T00:00:00Z");
        assert_eq!(got.commit, None);
    }

    #[test]
    fn absent_malformed_or_empty_yields_none() {
        assert_eq!(parse_mutation_scans(""), None);
        assert_eq!(parse_mutation_scans("not json"), None);
        assert_eq!(parse_mutation_scans("[]"), None);
        // an object, not the documented array
        assert_eq!(
            parse_mutation_scans(r#"{"timestamp":"2026-01-01T00:00:00Z"}"#),
            None
        );
        // entries with no timestamp cannot be ranked
        assert_eq!(parse_mutation_scans(r#"[{"commit":"abc"}]"#), None);
    }
}
