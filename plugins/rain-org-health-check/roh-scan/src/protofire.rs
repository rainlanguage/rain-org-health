//! Pure logic for the EXTERNAL (Protofire) audit-coverage + drift report.
//!
//! This is a DIFFERENT signal from `audit.rs`: that reads each repo's
//! `.audit/last-run.json` INTERNAL audit-skill stamp. Here we classify every repo
//! by whether it carries a formal Protofire audit PDF committed under
//! `audit/protofire/`, and for the ones that do, quantify how much source has
//! drifted since the audit. The scope is that dir specifically — a non-Protofire
//! report elsewhere under `audit/` is NOT a Protofire audit and must not count.
//!
//! All parsing/predicate/arithmetic logic lives here as pure functions (unit +
//! mutation tested, no I/O); the `gh`/GraphQL orchestration is in `main.rs`.
//!
//! ## The tag-in-filename convention (the drift enabler)
//! A Protofire PDF named with the audited git tag — e.g.
//! `rain.factory.v0.1.1-r2.0.may-2026.pdf` — makes the audited version
//! machine-readable, so drift is measured against the exact tag rather than
//! inferred from a commit date. `parse_audited_tag` extracts that `vX.Y.Z` token.
//! PDFs that don't encode a tag (`raindex.e686b4d.apr-2026.pdf`,
//! `protofire.rain.metadata.feb-2026.pdf`) are flagged `tag_convention_absent` —
//! drift is still computed, but from the PDF's own commit rather than a tag.

use regex::Regex;
use std::sync::OnceLock;

/// One Protofire audit PDF found under `audit/protofire/` (e.g.
/// `audit/protofire/rain.factory.v0.1.1-r2.0.may-2026.pdf`). `commit_sha` is the
/// commit that ADDED the file — the drift base when the filename encodes no tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPdf {
    pub filename: String,
    pub path: String,
    pub last_commit_iso: String,
    pub commit_sha: String,
}

/// One changed file from the `compare` API — the inputs the drift sum needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareFile {
    pub filename: String,
    pub additions: u64,
    pub deletions: u64,
}

/// External-audit coverage status for a repo (the issue's taxonomy):
/// - `never`   — no PDF under `audit/protofire/` (the primary coverage gap)
/// - `na`      — has a PDF but the repo has no tags to compare against
/// - `stale`   — has a PDF and a tag is newer than the audit
/// - `current` — has a PDF and no tag is newer than the audit
pub const NEVER: &str = "never";
pub const NA: &str = "na";
pub const STALE: &str = "stale";
pub const CURRENT: &str = "current";

/// Extract the audited git tag (`vMAJOR.MINOR.PATCH`) encoded in a PDF filename
/// per the naming convention. Returns `None` when the filename encodes no such
/// tag (older date-only or short-commit-sha names) — the caller then flags
/// `tag_convention_absent` and falls back to the PDF's own commit.
///
/// The `v` must sit on a non-alphanumeric boundary so `rev1.2.3` never matches,
/// and greedy digit runs mean `v0.1.10` reads as `v0.1.10`, not `v0.1.1`. Any
/// trailing audit-revision suffix (`-r2.0`) is deliberately NOT part of the tag.
pub fn parse_audited_tag(filename: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:^|[^A-Za-z0-9])v(\d+\.\d+\.\d+)").expect("static tag regex")
    });
    re.captures(filename).map(|c| format!("v{}", &c[1]))
}

/// True if `path` is a TEST file (excluded from source-LOC drift). Pins the
/// exclusion set for the org's languages (sol / rs / ts): any `test`/`tests`/
/// `spec`/`specs`/`__tests__` path segment, or a per-language test basename
/// suffix (`*.t.sol`, `*.test.ts`, `*.spec.ts`, and the js/jsx/tsx/mjs/cjs kin).
pub fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    const TEST_DIRS: [&str; 5] = ["test", "tests", "spec", "specs", "__tests__"];
    if p.split('/').any(|seg| TEST_DIRS.contains(&seg)) {
        return true;
    }
    const TEST_SUFFIXES: [&str; 13] = [
        ".t.sol",
        ".test.ts",
        ".spec.ts",
        ".test.tsx",
        ".spec.tsx",
        ".test.js",
        ".spec.js",
        ".test.jsx",
        ".spec.jsx",
        ".test.mjs",
        ".spec.mjs",
        ".test.cjs",
        ".spec.cjs",
    ];
    TEST_SUFFIXES.iter().any(|s| p.ends_with(s))
}

/// True if `path` is a SOURCE-CODE file whose LOC drift is meaningful. Scoped to
/// the org's languages (sol / rs / ts + js family). TypeScript declaration files
/// (`*.d.ts`) are generated type surface, not hand-written source, so excluded.
pub fn is_source_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".d.ts") {
        return false;
    }
    const SRC_EXT: [&str; 8] = [".sol", ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"];
    SRC_EXT.iter().any(|e| p.ends_with(e))
}

/// The drift predicate: a file counts toward source-LOC drift iff it is source
/// AND not a test. A "source LOC" number that silently counts tests is misleading.
pub fn counts_as_source_drift(path: &str) -> bool {
    is_source_file(path) && !is_test_path(path)
}

/// Sum non-test source LOC drift (`additions + deletions`) and count the files
/// that contributed, over a `compare` file list.
pub fn source_drift(files: &[CompareFile]) -> (u64, u64) {
    let mut loc = 0u64;
    let mut n = 0u64;
    for f in files {
        if counts_as_source_drift(&f.filename) {
            loc += f.additions + f.deletions;
            n += 1;
        }
    }
    (loc, n)
}

/// Build the GitHub compare-view URL for a repo's audit drift:
/// `https://github.com/{owner}/{repo}/compare/{base}...{head}`. `base` is the
/// resolved audited anchor (tag / commit / PDF-file-commit fallback — the SAME
/// base the drift count is computed against) and `head` is the current head.
/// Returns `None` when either ref is empty, so the panel never renders a broken
/// link to `compare/...head` (or `compare/base...`).
pub fn compare_url(owner: &str, repo: &str, base: &str, head: &str) -> Option<String> {
    if base.is_empty() || head.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{repo}/compare/{base}...{head}"
    ))
}

/// Index of the newest PDF by commit date (ISO-8601 UTC sorts lexicographically).
/// The newest PDF is the reference audit — its filename is parsed for the tag and
/// its commit is the drift base when no tag is present.
pub fn newest_pdf_index(pdfs: &[AuditPdf]) -> Option<usize> {
    pdfs.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.last_commit_iso.cmp(&b.last_commit_iso))
        .map(|(i, _)| i)
}

/// Strictly-newer comparison for two ISO-8601 UTC timestamps. Same fixed format
/// (`YYYY-MM-DDTHH:MM:SSZ`) ⇒ lexicographic order == chronological order.
pub fn newer_than(a_iso: &str, b_iso: &str) -> bool {
    a_iso > b_iso
}

/// Is the audit stale? True when a newer tag exists OR non-test source changed
/// since the audit — either means the audited artifact no longer matches HEAD.
pub fn is_stale(newer_tag_exists: bool, source_loc_drift: u64) -> bool {
    newer_tag_exists || source_loc_drift > 0
}

/// Coverage taxonomy from the three facts the classification turns on.
pub fn classify_external_audit(
    has_pdf: bool,
    has_tags: bool,
    newer_tag_exists: bool,
) -> &'static str {
    if !has_pdf {
        NEVER
    } else if !has_tags {
        NA
    } else if newer_tag_exists {
        STALE
    } else {
        CURRENT
    }
}

/// Days between two ISO-8601 dates (`from` → `to`), using a days-from-civil
/// conversion (no chrono). Reads only the leading `YYYY-MM-DD`; `None` if either
/// is malformed.
pub fn days_between(from_iso: &str, to_iso: &str) -> Option<i64> {
    let (fy, fm, fd) = parse_ymd(from_iso)?;
    let (ty, tm, td) = parse_ymd(to_iso)?;
    Some(days_from_civil(ty, tm, td) - days_from_civil(fy, fm, fd))
}

/// Parse the leading `YYYY-MM-DD` of an ISO timestamp into (y, m, d).
fn parse_ymd(iso: &str) -> Option<(i64, i64, i64)> {
    let s = iso.get(0..10)?;
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Howard Hinnant's days-from-civil: serial day number for a proleptic-Gregorian
/// date, epoch 1970-01-01 = 0. Exact integer arithmetic, no floating point.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_audited_tag ----
    #[test]
    fn parses_tag_from_convention_names() {
        // The real may-2026 PDFs that adopted the tag-in-filename convention.
        assert_eq!(
            parse_audited_tag("rain.factory.v0.1.1-r2.0.may-2026.pdf"),
            Some("v0.1.1".into())
        );
        assert_eq!(
            parse_audited_tag("rain.vats.v0.1.5-r2.0.may-2026.pdf"),
            Some("v0.1.5".into())
        );
        assert_eq!(
            parse_audited_tag("rain.extrospection.v0.1.1-r3.0.may-2026.pdf"),
            Some("v0.1.1".into())
        );
        // bare `audit/protofire/<tag>.pdf` form from the issue.
        assert_eq!(parse_audited_tag("v1.2.3.pdf"), Some("v1.2.3".into()));
    }

    #[test]
    fn no_tag_in_older_or_sha_names() {
        // date-only names (no tag): current majority of PDFs.
        assert_eq!(
            parse_audited_tag("protofire.rain.metadata.feb-2026.pdf"),
            None
        );
        assert_eq!(parse_audited_tag("2023-08-29-Payant_Report.pdf"), None);
        // short-commit-sha names encode a commit, not a tag.
        assert_eq!(parse_audited_tag("raindex.e686b4d.apr-2026.pdf"), None);
        assert_eq!(parse_audited_tag("rain.factory.1a92a86.feb-2026.pdf"), None);
        // a 2-part `2.0` version is NOT a vX.Y.Z tag.
        assert_eq!(
            parse_audited_tag("Report_rain.interpreter.interface_2.0_feb_2026.pdf"),
            None
        );
    }

    #[test]
    fn tag_boundary_and_greedy_patch() {
        // greedy: v0.1.10 must not truncate to v0.1.1.
        assert_eq!(
            parse_audited_tag("rain.x.v0.1.10-r2.0.jun-2026.pdf"),
            Some("v0.1.10".into())
        );
        // the `v` must be on a non-alnum boundary: `rev1.2.3` is not a tag.
        assert_eq!(parse_audited_tag("rev1.2.3.pdf"), None);
    }

    // ---- is_test_path ----
    #[test]
    fn test_dirs_are_tests() {
        assert!(is_test_path("test/Foo.sol"));
        assert!(is_test_path("src/lib/tests/mod.rs"));
        assert!(is_test_path("packages/ui/__tests__/a.ts"));
        assert!(is_test_path("spec/thing.js"));
    }

    #[test]
    fn test_suffixes_are_tests() {
        assert!(is_test_path("src/Vault.t.sol"));
        assert!(is_test_path("app/foo.test.ts"));
        assert!(is_test_path("app/foo.spec.tsx"));
    }

    #[test]
    fn non_test_source_is_not_a_test() {
        assert!(!is_test_path("src/Vault.sol"));
        assert!(!is_test_path("src/lib.rs"));
        assert!(!is_test_path("app/latest.ts")); // "latest" contains "test" but is not a segment/suffix
        assert!(!is_test_path("contracts/Contest.sol")); // "Contest" is not a test segment
    }

    // ---- is_source_file ----
    #[test]
    fn recognizes_source_extensions() {
        for p in [
            "src/A.sol",
            "src/b.rs",
            "x.ts",
            "x.tsx",
            "x.js",
            "x.jsx",
            "x.mjs",
            "x.cjs",
        ] {
            assert!(is_source_file(p), "{p} should be source");
        }
    }

    #[test]
    fn non_source_and_decls_excluded() {
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("audit/report.pdf"));
        assert!(!is_source_file("foundry.toml"));
        assert!(!is_source_file("types/index.d.ts")); // generated declarations, not source
    }

    // ---- counts_as_source_drift ----
    #[test]
    fn drift_counts_source_but_not_tests() {
        assert!(counts_as_source_drift("src/Vault.sol"));
        assert!(!counts_as_source_drift("src/Vault.t.sol")); // source ext but a test
        assert!(!counts_as_source_drift("test/helpers.sol")); // in a test dir
        assert!(!counts_as_source_drift("README.md")); // not source at all
    }

    #[test]
    fn source_drift_sums_only_non_test_source() {
        let files = vec![
            CompareFile {
                filename: "src/A.sol".into(),
                additions: 10,
                deletions: 5,
            }, // +15
            CompareFile {
                filename: "test/A.t.sol".into(),
                additions: 99,
                deletions: 99,
            }, // excluded (test)
            CompareFile {
                filename: "src/B.rs".into(),
                additions: 3,
                deletions: 2,
            }, // +5
            CompareFile {
                filename: "README.md".into(),
                additions: 40,
                deletions: 40,
            }, // excluded (non-source)
            CompareFile {
                filename: "src/C.ts".into(),
                additions: 1,
                deletions: 0,
            }, // +1
        ];
        assert_eq!(source_drift(&files), (21, 3));
    }

    // ---- compare_url ----
    #[test]
    fn compare_url_from_base_and_head() {
        // The audited anchor is a tag: base...head under the repo's compare view.
        assert_eq!(
            compare_url("rainlanguage", "rain.factory", "v0.1.1", "main"),
            Some("https://github.com/rainlanguage/rain.factory/compare/v0.1.1...main".into())
        );
        // The fallback anchor is a commit sha (unanchored PDF); still base...head.
        assert_eq!(
            compare_url("rainlanguage", "raindex", "e686b4d", "main"),
            Some("https://github.com/rainlanguage/raindex/compare/e686b4d...main".into())
        );
    }

    #[test]
    fn compare_url_none_when_base_missing() {
        // No resolvable base (empty tag AND empty fallback commit) ⇒ no URL, so
        // the panel omits the link rather than pointing at compare/...head.
        assert_eq!(
            compare_url("rainlanguage", "rain.factory", "", "main"),
            None
        );
        // A missing head is equally broken (compare/base...) ⇒ also no URL.
        assert_eq!(
            compare_url("rainlanguage", "rain.factory", "v0.1.1", ""),
            None
        );
    }

    // ---- newest_pdf_index ----
    #[test]
    fn newest_pdf_is_by_commit_date() {
        let mk = |f: &str, iso: &str| AuditPdf {
            filename: f.into(),
            path: format!("audit/protofire/{f}"),
            last_commit_iso: iso.into(),
            commit_sha: "sha".into(),
        };
        let pdfs = vec![
            mk("old.pdf", "2025-01-01T00:00:00Z"),
            mk("new.pdf", "2026-05-12T00:00:00Z"),
            mk("mid.pdf", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(newest_pdf_index(&pdfs), Some(1));
        assert_eq!(newest_pdf_index(&[]), None);
    }

    // ---- newer_than ----
    #[test]
    fn newer_than_is_strict_chronological() {
        assert!(newer_than("2026-07-12T14:01:53Z", "2026-05-12T15:15:26Z"));
        assert!(!newer_than("2026-05-12T15:15:26Z", "2026-07-12T14:01:53Z"));
        assert!(!newer_than("2026-05-12T15:15:26Z", "2026-05-12T15:15:26Z")); // equal is not newer
    }

    // ---- is_stale ----
    #[test]
    fn stale_on_newer_tag_or_source_drift() {
        assert!(is_stale(true, 0)); // newer tag alone
        assert!(is_stale(false, 1)); // source drift alone
        assert!(is_stale(true, 42)); // both
        assert!(!is_stale(false, 0)); // neither
    }

    // ---- classify_external_audit ----
    #[test]
    fn classification_taxonomy() {
        assert_eq!(classify_external_audit(false, false, false), NEVER);
        assert_eq!(classify_external_audit(false, true, true), NEVER); // no PDF dominates
        assert_eq!(classify_external_audit(true, false, false), NA); // PDF but no tags
        assert_eq!(classify_external_audit(true, true, true), STALE);
        assert_eq!(classify_external_audit(true, true, false), CURRENT);
    }

    // ---- days_between ----
    #[test]
    fn days_between_spans_months_and_years() {
        // May 12 → Jul 12 2026 = 61 days.
        assert_eq!(
            days_between("2026-05-12T15:15:26Z", "2026-07-12T14:01:53Z"),
            Some(61)
        );
        assert_eq!(
            days_between("2026-01-01T00:00:00Z", "2026-01-01T23:59:59Z"),
            Some(0)
        );
        assert_eq!(
            days_between("2025-12-31T00:00:00Z", "2026-01-01T00:00:00Z"),
            Some(1)
        );
        // leap year: 2024-02-28 → 2024-03-01 = 2 days (Feb 29 exists).
        assert_eq!(days_between("2024-02-28", "2024-03-01"), Some(2));
    }

    #[test]
    fn days_between_rejects_malformed() {
        assert_eq!(days_between("not-a-date", "2026-01-01"), None);
        assert_eq!(days_between("2026-13-01", "2026-01-01"), None); // month 13
        assert_eq!(days_between("2026-01-00", "2026-01-01"), None); // day 0
        assert_eq!(days_between("", ""), None);
    }
}
