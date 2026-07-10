//! The org-wide open-issue queue: parse the GraphQL nodes the scan fetches
//! (open issues + open PRs' `closingIssuesReferences`), classify each issue as
//! covered (an open PR closes it) or uncovered, and order the queue with the
//! uncovered backlog first. Pure — no I/O here — so classification and ordering
//! are unit- and mutation-testable without gh/network; the fetch is in main.rs.
//!
//! Coverage is resolved org-wide, not per repo: a PR may close an issue in
//! another repo, so the covered set is keyed on (repo, issue number) pairs
//! collected across every scanned repo before any issue is classified.

use std::collections::HashSet;

/// One open issue as fetched from GraphQL, before coverage classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueNode {
    pub number: u64,
    pub title: String,
    /// ISO-8601 UTC timestamp (`createdAt`); lexicographic order is age order.
    pub created_at: String,
    pub labels: Vec<String>,
    /// First assignee's login ("" when unassigned).
    pub assignee: String,
}

/// One queue entry: an open issue plus the repo it lives in and whether an
/// open PR's `closingIssuesReferences` covers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenIssue {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub created_at: String,
    pub labels: Vec<String>,
    pub assignee: String,
    /// true when an open PR (anywhere in the org) closes this issue.
    pub covered: bool,
}

/// Parse one GraphQL open-issue node (a single `--jq '…nodes[]'` output line).
/// `None` on malformed JSON or a missing number/title/createdAt; absent labels
/// or assignees default to empty.
pub fn parse_issue_node(line: &str) -> Option<IssueNode> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let number = v.get("number")?.as_u64()?;
    let title = v.get("title")?.as_str()?.to_string();
    let created_at = v.get("createdAt")?.as_str()?.to_string();
    let labels = v
        .pointer("/labels/nodes")
        .and_then(|n| n.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|l| l.get("name").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let assignee = v
        .pointer("/assignees/nodes/0/login")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Some(IssueNode {
        number,
        title,
        created_at,
        labels,
        assignee,
    })
}

/// Extract the (repo, issue number) pairs one open-PR node's
/// `closingIssuesReferences` covers. Only issues owned by `org` count —
/// closing references can point across orgs, and those are outside the scan.
/// Malformed JSON or missing refs yield no pairs.
pub fn parse_pr_closing_refs(line: &str, org: &str) -> Vec<(String, u64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return Vec::new();
    };
    let Some(nodes) = v
        .pointer("/closingIssuesReferences/nodes")
        .and_then(|n| n.as_array())
    else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|n| {
            let owner = n.pointer("/repository/owner/login")?.as_str()?;
            // GitHub logins are case-insensitive.
            if !owner.eq_ignore_ascii_case(org) {
                return None;
            }
            let repo = n.pointer("/repository/name")?.as_str()?.to_string();
            let number = n.get("number")?.as_u64()?;
            Some((repo, number))
        })
        .collect()
}

/// Classify one repo's open issues against the org-wide covered set: an issue
/// is covered exactly when (this repo, its number) appears in `covered`.
pub fn classify_issues(
    repo: &str,
    nodes: &[IssueNode],
    covered: &HashSet<(String, u64)>,
) -> Vec<OpenIssue> {
    nodes
        .iter()
        .map(|n| OpenIssue {
            repo: repo.to_string(),
            number: n.number,
            title: n.title.clone(),
            created_at: n.created_at.clone(),
            labels: n.labels.clone(),
            assignee: n.assignee.clone(),
            covered: covered.contains(&(repo.to_string(), n.number)),
        })
        .collect()
}

/// Sort key for the queue: uncovered issues first (the real backlog), then
/// oldest first (ISO timestamps compare lexicographically), repo and number
/// as final tiebreaks.
pub fn issue_sort_key(issue: &OpenIssue) -> (u8, String, String, u64) {
    (
        issue.covered as u8,
        issue.created_at.clone(),
        issue.repo.clone(),
        issue.number,
    )
}

/// Whole days elapsed from `created_at` to `now` (both ISO-8601 dates or
/// timestamps; only the YYYY-MM-DD prefix counts). `None` when either fails
/// to parse. Negative if `created_at` is in the future.
pub fn age_days(created_at: &str, now: &str) -> Option<i64> {
    Some(days_from_civil(parse_date(now)?) - days_from_civil(parse_date(created_at)?))
}

/// Parse the YYYY-MM-DD prefix of an ISO-8601 string into (y, m, d).
fn parse_date(s: &str) -> Option<(i64, i64, i64)> {
    let date = s.get(..10)?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's days_from_civil).
fn days_from_civil((y, m, d): (i64, i64, i64)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape mirrors a real `gh api graphql --jq '.data.repository.issues.nodes[]'` line.
    const ISSUE_LINE: &str = r#"{"assignees":{"nodes":[{"login":"thedavidmeister"}]},"createdAt":"2026-06-13T08:32:14Z","labels":{"nodes":[{"name":"bug"},{"name":"site"}]},"number":1,"title":"Extend health check"}"#;

    fn issue(number: u64, created_at: &str) -> IssueNode {
        IssueNode {
            number,
            title: format!("issue {number}"),
            created_at: created_at.into(),
            labels: Vec::new(),
            assignee: String::new(),
        }
    }

    fn covered_set(pairs: &[(&str, u64)]) -> HashSet<(String, u64)> {
        pairs.iter().map(|(r, n)| (r.to_string(), *n)).collect()
    }

    #[test]
    fn issue_node_parses_all_fields() {
        let n = parse_issue_node(ISSUE_LINE).expect("should parse");
        assert_eq!(n.number, 1);
        assert_eq!(n.title, "Extend health check");
        assert_eq!(n.created_at, "2026-06-13T08:32:14Z");
        assert_eq!(n.labels, vec!["bug".to_string(), "site".to_string()]);
        assert_eq!(n.assignee, "thedavidmeister");
    }

    #[test]
    fn issue_node_defaults_labels_and_assignee() {
        let bare = r#"{"number":7,"title":"t","createdAt":"2026-01-01T00:00:00Z"}"#;
        let n = parse_issue_node(bare).unwrap();
        assert!(n.labels.is_empty());
        assert_eq!(n.assignee, "");
        // empty connections behave the same as absent ones
        let empty = r#"{"number":7,"title":"t","createdAt":"2026-01-01T00:00:00Z","labels":{"nodes":[]},"assignees":{"nodes":[]}}"#;
        let n = parse_issue_node(empty).unwrap();
        assert!(n.labels.is_empty());
        assert_eq!(n.assignee, "");
    }

    #[test]
    fn issue_node_missing_required_field_is_none() {
        assert_eq!(
            parse_issue_node(r#"{"title":"t","createdAt":"2026-01-01T00:00:00Z"}"#),
            None
        );
        assert_eq!(
            parse_issue_node(r#"{"number":7,"createdAt":"2026-01-01T00:00:00Z"}"#),
            None
        );
        assert_eq!(parse_issue_node(r#"{"number":7,"title":"t"}"#), None);
    }

    #[test]
    fn issue_node_malformed_is_none() {
        assert_eq!(parse_issue_node(""), None);
        assert_eq!(parse_issue_node("not json"), None);
    }

    #[test]
    fn pr_refs_same_org_kept() {
        let line = r#"{"closingIssuesReferences":{"nodes":[{"number":17,"repository":{"name":"rain-org-health","owner":{"login":"rainlanguage"}}}]}}"#;
        assert_eq!(
            parse_pr_closing_refs(line, "rainlanguage"),
            vec![("rain-org-health".to_string(), 17)]
        );
        // logins compare case-insensitively
        assert_eq!(
            parse_pr_closing_refs(line, "RainLanguage"),
            vec![("rain-org-health".to_string(), 17)]
        );
    }

    #[test]
    fn pr_refs_cross_org_dropped() {
        let line = r#"{"closingIssuesReferences":{"nodes":[{"number":9,"repository":{"name":"other","owner":{"login":"someone-else"}}},{"number":3,"repository":{"name":"flow","owner":{"login":"rainlanguage"}}}]}}"#;
        assert_eq!(
            parse_pr_closing_refs(line, "rainlanguage"),
            vec![("flow".to_string(), 3)]
        );
    }

    #[test]
    fn pr_refs_empty_or_malformed_is_empty() {
        assert!(
            parse_pr_closing_refs(r#"{"closingIssuesReferences":{"nodes":[]}}"#, "o").is_empty()
        );
        assert!(parse_pr_closing_refs("{}", "o").is_empty());
        assert!(parse_pr_closing_refs("not json", "o").is_empty());
    }

    #[test]
    fn classify_covers_exact_repo_and_number() {
        let covered = covered_set(&[("flow", 3)]);
        let out = classify_issues(
            "flow",
            &[
                issue(3, "2026-01-01T00:00:00Z"),
                issue(4, "2026-01-02T00:00:00Z"),
            ],
            &covered,
        );
        assert!(out[0].covered, "issue with a closing open PR is covered");
        assert!(
            !out[1].covered,
            "issue with no closing open PR is uncovered"
        );
    }

    #[test]
    fn classify_same_number_other_repo_is_not_covered() {
        // the covered pair is (flow, 3); issue 3 of ANOTHER repo must stay uncovered
        let covered = covered_set(&[("flow", 3)]);
        let out = classify_issues("raindex", &[issue(3, "2026-01-01T00:00:00Z")], &covered);
        assert!(!out[0].covered);
    }

    #[test]
    fn classify_carries_issue_fields_through() {
        let node = IssueNode {
            number: 5,
            title: "the title".into(),
            created_at: "2026-02-03T04:05:06Z".into(),
            labels: vec!["bug".into()],
            assignee: "someone".into(),
        };
        let out = classify_issues("flow", &[node], &HashSet::new());
        assert_eq!(out[0].repo, "flow");
        assert_eq!(out[0].number, 5);
        assert_eq!(out[0].title, "the title");
        assert_eq!(out[0].created_at, "2026-02-03T04:05:06Z");
        assert_eq!(out[0].labels, vec!["bug".to_string()]);
        assert_eq!(out[0].assignee, "someone");
    }

    #[test]
    fn sort_uncovered_first_even_when_covered_is_older() {
        let mk = |repo: &str, number, created_at: &str, covered| OpenIssue {
            repo: repo.into(),
            number,
            title: String::new(),
            created_at: created_at.into(),
            labels: Vec::new(),
            assignee: String::new(),
            covered,
        };
        let mut queue = [
            mk("a", 1, "2025-01-01T00:00:00Z", true), // oldest, but covered
            mk("b", 2, "2026-06-01T00:00:00Z", false),
            mk("b", 3, "2026-01-01T00:00:00Z", false),
        ];
        queue.sort_by_key(issue_sort_key);
        assert!(
            !queue[0].covered && !queue[1].covered,
            "the uncovered backlog sorts ahead of covered issues"
        );
        assert_eq!(queue[0].number, 3, "oldest uncovered first");
        assert_eq!(queue[1].number, 2);
        assert_eq!(
            queue[2].number, 1,
            "covered issue last despite being oldest"
        );
    }

    #[test]
    fn sort_ties_break_on_repo_then_number() {
        let mk = |repo: &str, number| OpenIssue {
            repo: repo.into(),
            number,
            title: String::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            labels: Vec::new(),
            assignee: String::new(),
            covered: false,
        };
        let mut queue = [mk("z", 1), mk("a", 9), mk("a", 2)];
        queue.sort_by_key(issue_sort_key);
        assert_eq!(
            queue
                .iter()
                .map(|i| (i.repo.as_str(), i.number))
                .collect::<Vec<_>>(),
            vec![("a", 2), ("a", 9), ("z", 1)]
        );
    }

    #[test]
    fn age_days_counts_whole_days() {
        assert_eq!(
            age_days("2026-07-10T01:00:00Z", "2026-07-10T23:00:00Z"),
            Some(0)
        );
        assert_eq!(
            age_days("2026-06-10T12:00:00Z", "2026-07-10T00:00:00Z"),
            Some(30)
        );
        // across a leap day: 2024 is a leap year
        assert_eq!(
            age_days("2024-02-28T00:00:00Z", "2024-03-01T00:00:00Z"),
            Some(2)
        );
        // across a year boundary
        assert_eq!(
            age_days("2025-12-31T00:00:00Z", "2026-01-01T00:00:00Z"),
            Some(1)
        );
        // future createdAt goes negative rather than clamping
        assert_eq!(
            age_days("2026-07-11T00:00:00Z", "2026-07-10T00:00:00Z"),
            Some(-1)
        );
    }

    #[test]
    fn age_days_malformed_is_none() {
        assert_eq!(age_days("", "2026-07-10T00:00:00Z"), None);
        assert_eq!(age_days("2026-07-10T00:00:00Z", "not a date"), None);
        assert_eq!(
            age_days("2026-13-01T00:00:00Z", "2026-07-10T00:00:00Z"),
            None
        );
    }
}
