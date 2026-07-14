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
//! ## The anchor-in-filename convention (the drift enabler)
//! A Protofire PDF names the audited git state so drift is measured against the
//! exact audited ref rather than inferred from the PDF's own commit date. A
//! filename encodes one of three anchor kinds (`classify_anchor`):
//! - **tag-anchored** — a `vMAJOR.MINOR.PATCH` token (e.g.
//!   `rain.factory.v0.1.1-r2.0.may-2026.pdf`); `parse_audited_tag` extracts it.
//! - **commit-anchored** — a 7–40 hex-char token that RESOLVES to a real commit
//!   (e.g. `raindex.e686b4d.apr-2026.pdf`, whose `e686b4d` predates any `sol-v`
//!   contract tag, so the commit is the honest anchor). `parse_commit_candidate`
//!   extracts the candidate; resolution (the I/O half, in `main.rs`) confirms it.
//! - **unanchored** — neither, or a hex-looking token that doesn't resolve; drift
//!   is still computed, but from the PDF's own commit rather than the anchor.

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
    /// Recency key for choosing the reference audit: the committer date of the
    /// commit/tag the filename anchors to — the audit's real date. Falls back to
    /// the PDF's own file-commit date (`last_commit_iso`) when the name encodes no
    /// resolvable anchor. Unlike `last_commit_iso` this does NOT collapse when
    /// several PDFs are moved in one commit (which ties their file-commit dates):
    /// the audited commit's own timestamp still orders one audit against another.
    pub audit_date_iso: String,
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
/// - `unknown` — the `audit/protofire/` listing fetch FAILED, so coverage is
///   indeterminate. This is distinct from `never`: a failed fetch must never be
///   read as a confirmed coverage gap.
pub const NEVER: &str = "never";
pub const NA: &str = "na";
pub const STALE: &str = "stale";
pub const CURRENT: &str = "current";
pub const UNKNOWN: &str = "unknown";

/// Extract the audited git tag (`[sol-]vMAJOR.MINOR.PATCH`) encoded in a PDF
/// filename per the naming convention. Returns the FULL tag so it resolves as a
/// real git ref: a bare `v0.1.1` (as `rain.factory.v0.1.1-r2.0.…`) OR the org's
/// Solidity-release form `sol-v0.1.12` (as `raindex.sol-v0.1.12.…`). Returns
/// `None` when the filename encodes no such tag (older date-only or commit-sha
/// names) — the caller then flags `tag_convention_absent` and falls back to the
/// PDF's own commit.
///
/// The tag must sit on a non-alphanumeric boundary so `rev1.2.3` never matches,
/// and greedy digit runs mean `v0.1.10` reads as `v0.1.10`, not `v0.1.1`. The
/// `sol-` prefix is captured so the returned tag matches the repo's actual tag
/// (dropping it would yield a `v0.1.12` that does not resolve). Any trailing
/// audit-revision suffix (`-r2.0`) is deliberately NOT part of the tag.
pub fn parse_audited_tag(filename: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:^|[^A-Za-z0-9])((?:sol-)?v\d+\.\d+\.\d+)").expect("static tag regex")
    });
    re.captures(filename).map(|c| c[1].to_string())
}

/// The audited anchor a Protofire PDF filename encodes: the ref the drift base is
/// measured from. Tag and commit are BOTH honest anchors on the audited contract
/// state; `Unanchored` is the fallback (drift dated by the PDF's own file commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAnchor {
    /// A `vMAJOR.MINOR.PATCH` version tag encoded in the filename.
    Tag(String),
    /// A 7–40 hex-char token that resolved to a real commit in the repo.
    Commit(String),
    /// Neither a tag nor a resolvable commit.
    Unanchored,
}

impl AuditAnchor {
    /// Stable machine label for the anchor kind, mirrored into `health.json`.
    pub fn kind(&self) -> &'static str {
        match self {
            AuditAnchor::Tag(_) => "tag",
            AuditAnchor::Commit(_) => "commit",
            AuditAnchor::Unanchored => "unanchored",
        }
    }

    /// The ref the drift base compares from (`compare/{ref}...HEAD`): the tag or
    /// the resolved commit SHA. `None` when unanchored — the caller then falls back
    /// to the PDF's own commit.
    pub fn drift_base_ref(&self) -> Option<&str> {
        match self {
            AuditAnchor::Tag(t) => Some(t),
            AuditAnchor::Commit(sha) => Some(sha),
            AuditAnchor::Unanchored => None,
        }
    }
}

/// Extract a commit-SHA CANDIDATE (a 7–40 hex-char token on non-alphanumeric
/// boundaries) from a PDF filename. Only a candidate: whether it names a real
/// commit is decided by resolution (the I/O half). The both-sides boundary means
/// only a whole dot/dash-delimited token that is entirely hex matches — so
/// `raindex`, `metadata`, `interface` never yield a spurious sub-run, while
/// `raindex.e686b4d.apr-2026.pdf` yields `e686b4d`. Returns the first such token.
pub fn parse_commit_candidate(filename: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:^|[^0-9A-Za-z])([0-9a-fA-F]{7,40})(?:[^0-9A-Za-z]|$)")
            .expect("static commit regex")
    });
    re.captures(filename).map(|c| c[1].to_string())
}

/// Classify the audited anchor a PDF filename encodes. Precedence: a `vX.Y.Z` tag
/// wins (a tag is never mistaken for a commit); else a 7–40 hex token that
/// `resolve` confirms names a real commit is commit-anchored; else unanchored.
///
/// `resolve` is the I/O seam (in `main.rs`, `gh api repos/{o}/{r}/commits/{sha}`):
/// keeping it injected leaves this classification pure and network-free to test. A
/// hex-looking token that does NOT resolve — or no hex token at all — falls back to
/// `Unanchored` rather than erroring, guarding false positives. `resolve` is called
/// at most once, only when a tag is absent but a hex candidate is present.
pub fn classify_anchor<F: FnOnce(&str) -> bool>(filename: &str, resolve: F) -> AuditAnchor {
    if let Some(tag) = parse_audited_tag(filename) {
        return AuditAnchor::Tag(tag);
    }
    if let Some(sha) = parse_commit_candidate(filename) {
        if resolve(&sha) {
            return AuditAnchor::Commit(sha);
        }
    }
    AuditAnchor::Unanchored
}

/// The anchor REF a PDF filename encodes — a `vX.Y.Z` tag (preferred) or a 7–40
/// hex commit candidate — as a bare git ref string, or `None` when the name
/// encodes neither. Unlike `classify_anchor` this performs NO resolution (no
/// I/O): the caller resolves the ref (e.g. `gh api commits/{ref}` for the audit's
/// date). Same precedence as `classify_anchor` so both agree on which token is
/// the anchor. A hex candidate that is not a real commit is still returned here;
/// resolving it simply fails and the caller falls back.
pub fn anchor_ref(filename: &str) -> Option<String> {
    parse_audited_tag(filename).or_else(|| parse_commit_candidate(filename))
}

/// Build the GitHub compare-view URL for a repo's audit drift:
/// `https://github.com/{owner}/{repo}/compare/{base}...{head}`. `base` is the
/// audited anchor (tag or resolved commit); `head` the repo default branch.
/// `None` when either side is empty (a `compare/...head` or `compare/base...`
/// link is broken), so the panel omits the link rather than pointing at a
/// malformed compare view.
pub fn compare_url(owner: &str, repo: &str, base: &str, head: &str) -> Option<String> {
    if base.is_empty() || head.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{repo}/compare/{base}...{head}"
    ))
}

/// True if a Solidity path is a TEST file (excluded from source-LOC drift): it
/// ends with the Foundry test suffix `.t.sol`, OR it lies under a `test/` or
/// `tests/` directory (any such path segment, including a leading one — so
/// `test/util/Foo.sol` is a test even without a `.t.sol` suffix). Solidity
/// scripts (`.s.sol`) are NOT tests and are deliberately not matched here.
pub fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".t.sol") {
        return true;
    }
    const TEST_DIRS: [&str; 2] = ["test", "tests"];
    p.split('/').any(|seg| TEST_DIRS.contains(&seg))
}

/// True if `path` is a Solidity SOURCE file whose LOC drift is meaningful.
/// Protofire audits are Solidity audits, so drift is measured over `.sol` files
/// ONLY — every other language is out of scope for this metric.
pub fn is_source_file(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".sol")
}

/// The drift predicate: a file counts toward source-LOC drift iff it is a
/// non-test Solidity source file. Every `.sol` outside a test — src/, deploy/,
/// script(s)/ (`.s.sol` scripts included), config, dependencies/ — counts; only
/// the test exclusion removes files. A "source LOC" number that silently counts
/// tests is misleading.
pub fn counts_as_source_drift(path: &str) -> bool {
    is_source_file(path) && !is_test_path(path)
}

/// Sum non-test source LOC drift over a `compare` file list, keeping additions
/// and deletions SEPARATE — `(added, removed, files)`. A `+X / −Y` diffstat tells
/// a reader whether the churn is net growth, net shrinkage, or an in-place rewrite;
/// collapsing to one `added + deletions` total erases that distinction. The
/// combined total is derivable as `added + removed`. Only non-test source files
/// contribute to any of the three figures (the `counts_as_source_drift` predicate).
pub fn source_drift(files: &[CompareFile]) -> (u64, u64, u64) {
    let mut added = 0u64;
    let mut removed = 0u64;
    let mut n = 0u64;
    for f in files {
        if counts_as_source_drift(&f.filename) {
            added += f.additions;
            removed += f.deletions;
            n += 1;
        }
    }
    (added, removed, n)
}

/// Count non-test Solidity files that differ between two git trees — the accurate
/// drift-file count when a `compare` diff is TRUNCATED at GitHub's 300-file cap.
/// A large repo whose `.sol` sorts past the cap otherwise reads as a false zero
/// drift; the tree blob-sha diff sidesteps the cap and distinguishes a real zero
/// (no `.sol` changed) from "the `.sol` files weren't in the truncated page".
/// Each input is `(path, blob_sha)` for every blob in a recursive tree. A non-test
/// `.sol` path counts when it is added, removed, or its blob sha changed. Line
/// counts are NOT recoverable from trees, so this reports the file count only.
pub fn changed_source_file_count(base: &[(String, String)], head: &[(String, String)]) -> u64 {
    use std::collections::HashMap;
    let src = |files: &[(String, String)]| -> HashMap<String, String> {
        files
            .iter()
            .filter(|(p, _)| counts_as_source_drift(p))
            .map(|(p, s)| (p.clone(), s.clone()))
            .collect()
    };
    let base_map = src(base);
    let head_map = src(head);
    let mut changed = 0u64;
    for (path, sha) in &head_map {
        // Added (absent in base) or content-modified (blob sha differs).
        if base_map.get(path) != Some(sha) {
            changed += 1;
        }
    }
    for path in base_map.keys() {
        if !head_map.contains_key(path) {
            changed += 1; // removed
        }
    }
    changed
}

/// Index of the reference (newest) audit PDF, by the AUDITED commit/tag date
/// (`audit_date_iso`, ISO-8601 UTC — lexicographic order == chronological). The
/// audit date is the audited commit's own timestamp, NOT the PDF file's commit
/// date, so several PDFs moved together in a single commit (which ties their file
/// dates) still order by which audit is genuinely newer. The reference PDF's
/// filename then supplies the drift-base anchor (tag or commit).
pub fn newest_pdf_index(pdfs: &[AuditPdf]) -> Option<usize> {
    pdfs.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.audit_date_iso.cmp(&b.audit_date_iso))
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
        // The org's Solidity-release tag scheme `sol-v<X.Y.Z>` — the WHOLE tag is
        // returned (dropping `sol-` would yield a `v0.1.12` that resolves to no
        // real ref, since raindex's tag is `sol-v0.1.12`).
        assert_eq!(
            parse_audited_tag("raindex.sol-v0.1.12.jun-2026.pdf"),
            Some("sol-v0.1.12".into())
        );
        // greedy patch digits under the prefix: `sol-v0.1.10`, not `sol-v0.1.1`.
        assert_eq!(
            parse_audited_tag("raindex.sol-v0.1.10.jun-2026.pdf"),
            Some("sol-v0.1.10".into())
        );
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

    // ---- parse_commit_candidate ----
    #[test]
    fn extracts_hex_commit_token() {
        // the motivating case: a 7-hex short SHA delimited by dots.
        assert_eq!(
            parse_commit_candidate("raindex.e686b4d.apr-2026.pdf"),
            Some("e686b4d".into())
        );
        assert_eq!(
            parse_commit_candidate("rain.factory.1a92a86.feb-2026.pdf"),
            Some("1a92a86".into())
        );
        // a full 40-char SHA is in range.
        let full = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            parse_commit_candidate(&format!("raindex.{full}.apr-2026.pdf")),
            Some(full.into())
        );
    }

    #[test]
    fn no_commit_candidate_when_no_whole_hex_token() {
        // names with no all-hex token: the boundary rule means a hex sub-run inside
        // a mixed word ("raindex", "metadata") is NOT a candidate.
        assert_eq!(
            parse_commit_candidate("protofire.rain.metadata.feb-2026.pdf"),
            None
        );
        assert_eq!(parse_commit_candidate("2023-08-29-Payant_Report.pdf"), None);
        // too short: a 6-hex token is below the 7-char floor.
        assert_eq!(parse_commit_candidate("raindex.abc123.apr-2026.pdf"), None);
        // too long: 41 hex chars exceeds the 40-char ceiling (git object id width).
        let over = "0".repeat(41);
        assert_eq!(parse_commit_candidate(&format!("x.{over}.pdf")), None);
    }

    // ---- classify_anchor ----
    #[test]
    fn classify_prefers_tag_and_skips_resolution() {
        // a vX.Y.Z name is tag-anchored — the commit resolver must never run.
        assert_eq!(
            classify_anchor("rain.factory.v0.1.1-r2.0.may-2026.pdf", |_| panic!(
                "resolver must not run for a tag-anchored name"
            )),
            AuditAnchor::Tag("v0.1.1".into())
        );
    }

    #[test]
    fn classify_commit_anchored_when_hex_resolves() {
        // raindex's PDF: a hex token that resolves → commit-anchored at e686b4d.
        let anchor = classify_anchor("raindex.e686b4d.apr-2026.pdf", |sha| {
            assert_eq!(sha, "e686b4d"); // exactly the filename token is resolved
            true
        });
        assert_eq!(anchor, AuditAnchor::Commit("e686b4d".into()));
        assert_eq!(anchor.kind(), "commit");
        assert_eq!(anchor.drift_base_ref(), Some("e686b4d"));
    }

    #[test]
    fn classify_unanchored_when_hex_does_not_resolve() {
        // a hex-looking token that resolution rejects falls back to unanchored — a
        // false positive must never error or masquerade as a commit anchor.
        let anchor = classify_anchor("raindex.deadbeef.jan-2026.pdf", |_| false);
        assert_eq!(anchor, AuditAnchor::Unanchored);
        assert_eq!(anchor.kind(), "unanchored");
        assert_eq!(anchor.drift_base_ref(), None);
    }

    #[test]
    fn classify_unanchored_when_no_anchor_token() {
        // neither tag nor hex token: unanchored, and the resolver is never called.
        assert_eq!(
            classify_anchor("protofire.rain.metadata.feb-2026.pdf", |_| panic!(
                "resolver must not run without a hex candidate"
            )),
            AuditAnchor::Unanchored
        );
    }

    // ---- anchor_ref ----
    #[test]
    fn anchor_ref_prefers_tag_then_commit_then_none() {
        // A vX.Y.Z tag is the ref (same precedence as classify_anchor).
        assert_eq!(
            anchor_ref("rain.factory.v0.1.1-r2.0.may-2026.pdf").as_deref(),
            Some("v0.1.1")
        );
        // No tag but a whole-hex token → that commit candidate is the ref (returned
        // WITHOUT resolution — unlike classify_anchor, no I/O here).
        assert_eq!(
            anchor_ref("raindex.e686b4d.apr-2026.pdf").as_deref(),
            Some("e686b4d")
        );
        // Neither → None; the caller then dates the PDF by its own file commit.
        assert_eq!(anchor_ref("protofire.rain.metadata.feb-2026.pdf"), None);
    }

    // ---- is_test_path ----
    #[test]
    fn test_dirs_are_tests() {
        // A `test/` or `tests/` segment (leading or nested) marks a test, even
        // without a `.t.sol` suffix.
        assert!(is_test_path("test/Foo.sol"));
        assert!(is_test_path("test/util/Helper.sol")); // nested under test/, plain .sol
        assert!(is_test_path("src/lib/tests/Mod.sol"));
    }

    #[test]
    fn test_suffix_is_a_test() {
        // The Foundry `.t.sol` suffix marks a test anywhere.
        assert!(is_test_path("src/Vault.t.sol"));
        assert!(is_test_path("test/Foo.t.sol"));
    }

    #[test]
    fn non_test_solidity_is_not_a_test() {
        assert!(!is_test_path("src/Vault.sol"));
        assert!(!is_test_path("deploy/Deploy.sol"));
        assert!(!is_test_path("script/Thing.s.sol")); // a script, NOT a test
        assert!(!is_test_path("contracts/Contest.sol")); // "Contest" is not a test segment
    }

    // ---- is_source_file (Solidity only) ----
    #[test]
    fn recognizes_solidity_source() {
        for p in [
            "src/A.sol",
            "deploy/Deploy.sol",
            "script/Thing.s.sol",
            "dependencies/x/Y.sol",
        ] {
            assert!(is_source_file(p), "{p} should be Solidity source");
        }
    }

    #[test]
    fn non_solidity_excluded() {
        // Protofire audits are Solidity audits — every non-`.sol` extension is out
        // of scope, including the languages the metric used to (wrongly) count.
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("audit/report.pdf"));
        assert!(!is_source_file("foundry.toml"));
        assert!(!is_source_file("src/lib.rs")); // Rust dropped
        assert!(!is_source_file("packages/x/src/a.ts")); // TypeScript dropped
        assert!(!is_source_file("app/ui.tsx")); // TSX dropped
    }

    // ---- counts_as_source_drift ----
    #[test]
    fn drift_counts_non_test_solidity_only() {
        // Every non-test `.sol` counts, regardless of directory.
        assert!(counts_as_source_drift("src/Foo.sol"));
        assert!(counts_as_source_drift("deploy/Deploy.sol"));
        assert!(counts_as_source_drift("script/Thing.s.sol")); // scripts are source, not tests
        assert!(counts_as_source_drift("dependencies/x/Y.sol"));
        // Tests are excluded: the `.t.sol` suffix and the `test/` directory both.
        assert!(!counts_as_source_drift("src/Foo.t.sol")); // Solidity but a test suffix
        assert!(!counts_as_source_drift("test/util/Helper.sol")); // under test/, plain .sol
        assert!(!counts_as_source_drift("test/Foo.t.sol")); // both
                                                            // Non-Solidity never counts — the JS/TS/Rust extensions were dropped.
        assert!(!counts_as_source_drift("src/lib.rs"));
        assert!(!counts_as_source_drift("packages/x/src/a.ts"));
        assert!(!counts_as_source_drift("README.md")); // not source at all
    }

    #[test]
    fn source_drift_keeps_added_and_removed_separate_over_non_test_source() {
        let files = vec![
            CompareFile {
                filename: "src/A.sol".into(),
                additions: 10,
                deletions: 5,
            }, // +10 / −5  counts (src/ Solidity)
            CompareFile {
                filename: "deploy/D.sol".into(),
                additions: 2,
                deletions: 1,
            }, // +2 / −1   counts (deploy/ Solidity is audited surface)
            CompareFile {
                filename: "test/A.t.sol".into(),
                additions: 99,
                deletions: 99,
            }, // excluded (test) — symmetric so a leak inflates BOTH counts
            CompareFile {
                filename: "src/B.rs".into(),
                additions: 3,
                deletions: 2,
            }, // excluded (.rs — not Solidity, not audited surface)
            CompareFile {
                filename: "README.md".into(),
                additions: 40,
                deletions: 40,
            }, // excluded (non-source)
            CompareFile {
                filename: "src/C.ts".into(),
                additions: 1,
                deletions: 0,
            }, // excluded (.ts — not Solidity)
        ];
        let (added, removed, files_n) = source_drift(&files);
        // Only non-test Solidity contributes: 10+2 additions vs 5+1 deletions
        // across src/ and deploy/. The asymmetry (12 ≠ 6) catches an added/removed
        // swap; excluding the symmetric 99/99 test file, the 40/40 README, and the
        // .rs/.ts files proves the non-test-Solidity predicate gates BOTH sides —
        // Rust/TS are not part of a Protofire (Solidity) audit's surface.
        assert_eq!(added, 12, "additions = non-test Solidity additions only");
        assert_eq!(removed, 6, "deletions = non-test Solidity deletions only");
        assert_eq!(files_n, 2, "only the 2 non-test .sol files contribute");
        assert_ne!(
            added, removed,
            "the two figures are distinct, not one total"
        );
        // The combined total the JSON keeps is derivable as the sum.
        assert_eq!(added + removed, 18);
    }

    // ---- changed_source_file_count (tree diff) ----
    #[test]
    fn changed_source_file_count_counts_added_removed_modified_non_test_sol() {
        let b = |p: &str, s: &str| (p.to_string(), s.to_string());
        let base = vec![
            b("src/Keep.sol", "aaa"),       // unchanged
            b("src/Modify.sol", "bbb"),     // modified (sha changes below)
            b("src/Removed.sol", "ccc"),    // removed below
            b("src/Old.sol", "ddd"),        // renamed -> src/New.sol
            b("test/T.t.sol", "t1"),        // test — excluded
            b("test/util/H.sol", "t2"),     // under test/ — excluded
            b("crates/x/src/lib.rs", "r1"), // Rust — excluded
            b("README.md", "m1"),           // non-source — excluded
        ];
        let head = vec![
            b("src/Keep.sol", "aaa"),       // unchanged (same sha)
            b("src/Modify.sol", "b2b"),     // modified
            b("src/New.sol", "eee"),        // renamed target (add)
            b("deploy/D.sol", "fff"),       // added (deploy/ is source)
            b("test/T.t.sol", "t9"),        // test churn — excluded
            b("crates/x/src/lib.rs", "r9"), // Rust churn — excluded
        ];
        // Changed non-test .sol: Modify (mod), New (add), deploy/D (add),
        // Removed (del), Old (del from rename) = 5. Keep is unchanged; the test,
        // Rust, and README churn are all excluded on both sides.
        assert_eq!(changed_source_file_count(&base, &head), 5);
    }

    #[test]
    fn changed_source_file_count_zero_when_only_non_sol_churn() {
        let b = |p: &str, s: &str| (p.to_string(), s.to_string());
        // A repo where only non-Solidity churned: a real, legitimate zero — the
        // count must be 0, NOT conflated with a truncated-compare unknown.
        let base = vec![
            b("src/A.sol", "x"),
            b("docs/readme.md", "d1"),
            b("crates/a/src/l.rs", "r1"),
        ];
        let head = vec![
            b("src/A.sol", "x"),
            b("docs/readme.md", "d2"),
            b("crates/a/src/l.rs", "r2"),
        ];
        assert_eq!(changed_source_file_count(&base, &head), 0);
    }

    // ---- newest_pdf_index ----
    #[test]
    fn newest_pdf_is_by_audit_date_not_file_commit_date() {
        // Every PDF shares ONE file-commit date (as when a batch of PDFs is moved
        // into audit/protofire/ in a single commit) — so file-commit date can't
        // distinguish them. The AUDITED commit/tag date (audit_date_iso) must.
        let mk = |f: &str, audit_iso: &str| AuditPdf {
            filename: f.into(),
            path: format!("audit/protofire/{f}"),
            last_commit_iso: "2026-07-14T00:00:00Z".into(),
            commit_sha: "sha".into(),
            audit_date_iso: audit_iso.into(),
        };
        let pdfs = vec![
            mk("old.pdf", "2025-01-01T00:00:00Z"),
            mk("new.pdf", "2026-05-12T00:00:00Z"),
            mk("mid.pdf", "2026-02-01T00:00:00Z"),
        ];
        // Newest is the newest AUDIT (index 1), even though all file dates tie.
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
