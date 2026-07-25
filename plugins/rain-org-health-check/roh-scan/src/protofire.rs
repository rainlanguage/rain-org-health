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
//! filename encodes one of four anchor kinds (`classify_anchor`):
//! - **tag-anchored** — a `vMAJOR.MINOR.PATCH` token (e.g.
//!   `rain.factory.v0.1.1-r2.0.may-2026.pdf`) that RESOLVES to a real ref;
//!   `parse_audited_tag` extracts it, resolution confirms it.
//! - **commit-anchored** — a 7–40 hex-char token that RESOLVES to a real commit
//!   (e.g. `raindex.e686b4d.apr-2026.pdf`, whose `e686b4d` predates any `sol-v`
//!   contract tag, so the commit is the honest anchor). `parse_commit_candidate`
//!   extracts the candidate; resolution (the I/O half, in `main.rs`) confirms it.
//! - **unresolved-tag** — the name states a `vX.Y.Z` tag that does NOT resolve.
//!   That is a BROKEN record, not an unanchored one: the name asserts a ref, the
//!   assertion is false, and nothing about the audit's date is known. Dating it by
//!   the PDF's own file commit is what published a May-2026 audit as one day old
//!   after a repo split moved the file (#128).
//! - **unanchored** — neither, or a hex-looking token that doesn't resolve; drift
//!   is still computed, but from the PDF's own commit rather than the anchor.
//!
//! ## The inherited-audit convention (`inherited.` + `inherited.json`)
//! An audit performed on a PREDECESSOR repo, whose audited ref does not resolve
//! HERE but whose reviewed source produced artefacts this repo carries (a repo
//! SPLIT — a rename keeps its history, so its anchor still resolves locally and it
//! stays an ordinary local record). Two halves, checked against each other:
//! - the mandatory `inherited.` basename prefix is the DISCRIMINANT — cheap,
//!   visible in an `ls`, and it gates the manifest fetch so a repo with no
//!   inherited PDF costs no extra API call;
//! - `audit/<vendor>/inherited.json` is the PAYLOAD — which predecessor, which
//!   ref, and (the part no filename can hold) WHICH snapshots the report covers.
//!
//! Coverage is partial by construction, so the complement is COMPUTED from the
//! live tree (`coverage`) and never read from the manifest: a snapshot added
//! later shows up as uncovered with no manifest edit. A prefixed PDF whose record
//! is missing, unparseable or unresolvable is `unknown` — never a fallback to
//! resolving the ref locally, and never the PDF's own file-commit date.

use regex::Regex;
use std::sync::OnceLock;

/// Where an audit's date came from — carried WITH the date so a reader can never
/// mistake one provenance for another. The date and its source are one value
/// because they are one fact: a bare `String` with a silent
/// `unwrap_or(file_commit_date)` is exactly what published a genuine May-2026
/// audit as `daysSinceAudit: 1` when a split moved the PDF (#128).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditDate {
    /// The committer date of the ref the FILENAME anchors to, resolved in THIS
    /// repo. The audit's real date.
    Anchor(String),
    /// The committer date of the ref an INHERITED record anchors to, resolved in
    /// the PREDECESSOR repo that owns it. Equally real, just not local.
    InheritedAnchor(String),
    /// No anchor in the name: the PDF's own file-commit date stands in. Weaker —
    /// a `git mv` re-dates it — so it is labelled rather than silently mixed in.
    FileCommit(String),
    /// Nothing could be established: the name anchors to a ref that does not
    /// resolve, or an inherited claim that could not be validated. Absence is a
    /// value here; there is no date to fall back to that would not be invented.
    Unknown,
}

impl AuditDate {
    /// The ISO date, or `None` when it is genuinely unknown.
    pub fn iso(&self) -> Option<&str> {
        match self {
            Self::Anchor(d) | Self::InheritedAnchor(d) | Self::FileCommit(d) => Some(d),
            Self::Unknown => None,
        }
    }

    /// Machine label for where the date came from, mirrored into `health.json`.
    pub fn source(&self) -> &'static str {
        match self {
            Self::Anchor(_) => "anchor",
            Self::InheritedAnchor(_) => "inherited-anchor",
            Self::FileCommit(_) => "file-commit",
            Self::Unknown => "unknown",
        }
    }
}

/// One Protofire audit PDF found under `audit/protofire/` (e.g.
/// `audit/protofire/rain.factory.v0.1.1-r2.0.may-2026.pdf`). `commit_sha` is the
/// commit that last touched the file — the drift base only when the filename
/// encodes no anchor at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPdf {
    pub filename: String,
    pub path: String,
    pub last_commit_iso: String,
    pub commit_sha: String,
    /// Recency key for choosing the reference audit: the committer date of the
    /// commit/tag the record anchors to — the audit's real date — with its
    /// provenance. Unlike `last_commit_iso` this does NOT collapse when several
    /// PDFs are moved in one commit (which ties their file-commit dates): the
    /// audited commit's own timestamp still orders one audit against another.
    pub audit_date: AuditDate,
    /// How this PDF's audited state is anchored: locally, inherited from a
    /// predecessor repo, or declared inherited without a validatable record.
    pub provenance: AuditProvenance,
    /// The snapshot directories this PDF's inherited record CLAIMS to cover, as
    /// written in the manifest. Empty for a local record, and legitimately empty
    /// for an inherited one that covers nothing current (a superseded report) —
    /// which is how real provenance is kept without over-claiming.
    pub covers: Vec<String>,
}

/// One changed file from the `compare` API — the inputs the drift sum needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareFile {
    pub filename: String,
    pub additions: u64,
    pub deletions: u64,
    /// GitHub's change status (`added` / `removed` / `modified` / …). Needed so a
    /// file that genuinely has no base (or head) version is not mistaken for one
    /// whose content could not be fetched.
    pub status: String,
}

/// External-audit coverage status for a repo (the issue's taxonomy):
/// - `never`   — no PDF under `audit/protofire/` (the primary coverage gap)
/// - `na`      — has a PDF but the repo has no tags to compare against
/// - `stale`   — has a PDF and the audited Solidity changed since it
/// - `current` — has a PDF and the audited Solidity is unchanged since it (a
///   newer tag with no Solidity in it does NOT make an audit stale)
/// - `unknown` — the scan could not establish the verdict, so it is
///   indeterminate. Three causes: the `audit/protofire/` listing fetch FAILED (is
///   there a PDF at all?), the repo has a PDF but neither its drift nor its
///   tag recency could be read (is it still current?), or a PDF DECLARES
///   inherited provenance that could not be validated. Distinct from both
///   `never` and `current`: a failed lookup is neither a confirmed coverage gap
///   nor a clean bill of health.
/// - `inherited` — the audit was performed on a PREDECESSOR repo (a split), and
///   this repo's record says so honestly: which report, which source ref, and
///   which of this repo's snapshots it covers. Deliberately NOT `current`:
///   inherited coverage is partial by construction, so it names what is covered
///   and, more importantly, what is not.
///
/// A closed set, as a type rather than free strings: every consumer must decide
/// what each verdict means to it, and a seventh verdict added here should fail to
/// compile at each site that has not decided, rather than silently falling into
/// whatever the `else` arm happened to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExternalAudit {
    Never,
    Na,
    Stale,
    Current,
    Unknown,
    Inherited,
}

impl ExternalAudit {
    /// The wire form: what `health.json` carries and the dashboard renders. The
    /// only place these strings exist.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Na => "na",
            Self::Stale => "stale",
            Self::Current => "current",
            Self::Unknown => "unknown",
            Self::Inherited => "inherited",
        }
    }
}

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
/// state, and both carry the committer date resolution returned — a resolved
/// anchor cannot exist without its date, so the scanner cannot resolve a ref twice
/// and disagree with itself. `UnresolvedTag` is a broken record and `Unanchored`
/// the legacy fallback (drift dated by the PDF's own file commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAnchor {
    /// A `vMAJOR.MINOR.PATCH` version tag encoded in the filename, RESOLVED, with
    /// the committer date of the commit it points at.
    Tag { tag: String, date: String },
    /// A 7–40 hex-char token that resolved to a real commit, with its date.
    Commit { sha: String, date: String },
    /// The filename states a `vX.Y.Z` tag that does NOT resolve in the repo it was
    /// resolved against. Kept apart from `Unanchored` because the two are opposite
    /// facts: an unanchored name claims nothing, while this name makes a claim that
    /// is false. Falling back to the PDF's file-commit date here invents a date for
    /// an audit nobody can locate — the #128 fabrication. Nothing is known, so
    /// nothing is reported.
    UnresolvedTag(String),
    /// Neither a tag nor a resolvable commit — a legacy date-only name. Its hex
    /// candidate, if any, was a false positive rather than a stated ref, so the
    /// PDF's own file commit is a fallback rather than a contradiction.
    Unanchored,
}

impl AuditAnchor {
    /// Stable machine label for the anchor kind, mirrored into `health.json`.
    pub fn kind(&self) -> &'static str {
        match self {
            AuditAnchor::Tag { .. } => "tag",
            AuditAnchor::Commit { .. } => "commit",
            AuditAnchor::UnresolvedTag(_) => "unresolved-tag",
            AuditAnchor::Unanchored => "unanchored",
        }
    }

    /// The ref the drift base compares from (`compare/{ref}...HEAD`): the tag or
    /// the resolved commit SHA. `None` when the anchor names no ref that resolves,
    /// so a compare is never built against a ref the scan could not find.
    pub fn drift_base_ref(&self) -> Option<&str> {
        match self {
            AuditAnchor::Tag { tag, .. } => Some(tag),
            AuditAnchor::Commit { sha, .. } => Some(sha),
            AuditAnchor::UnresolvedTag(_) | AuditAnchor::Unanchored => None,
        }
    }

    /// The audited state's committer date, when the anchor resolved.
    pub fn resolved_date(&self) -> Option<&str> {
        match self {
            AuditAnchor::Tag { date, .. } | AuditAnchor::Commit { date, .. } => Some(date),
            AuditAnchor::UnresolvedTag(_) | AuditAnchor::Unanchored => None,
        }
    }

    /// Does the FILENAME follow the tag convention? True whether or not the tag
    /// resolves: the convention is about the name, and an unresolved tag is a
    /// broken anchor, not an absent one.
    pub fn names_tag(&self) -> bool {
        matches!(
            self,
            AuditAnchor::Tag { .. } | AuditAnchor::UnresolvedTag(_)
        )
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
/// `resolve` is the I/O seam (in `main.rs`, `gh api repos/{o}/{r}/commits/{ref}`)
/// and it returns the ref's COMMITTER DATE: resolution and dating are the same
/// API call, so a ref cannot be confirmed to exist without also pinning when it
/// was. Keeping it injected leaves this classification pure and network-free to
/// test. `resolve` is called at most once.
///
/// A TAG is resolved too. It used to be trusted on sight, which meant
/// `anchorKind: "tag"` asserted a ref nobody had checked — and after a repo split
/// carried a PDF into a repo where its tag does not exist, that produced a
/// fabricated date and a 404 compare link (#128). A tag that does not resolve is
/// `UnresolvedTag`, which reports nothing rather than something invented. A HEX
/// token that does not resolve stays `Unanchored`: an all-hex word in a date-only
/// name is a false positive, not a stated ref.
pub fn classify_anchor<F: FnOnce(&str) -> Option<String>>(
    filename: &str,
    resolve: F,
) -> AuditAnchor {
    if let Some(tag) = parse_audited_tag(filename) {
        return match resolve(&tag) {
            Some(date) => AuditAnchor::Tag { tag, date },
            None => AuditAnchor::UnresolvedTag(tag),
        };
    }
    if let Some(sha) = parse_commit_candidate(filename) {
        if let Some(date) = resolve(&sha) {
            return AuditAnchor::Commit { sha, date };
        }
    }
    AuditAnchor::Unanchored
}

/// The ref a repo's drift compares FROM, given its reference PDF's anchor and
/// that PDF's own file commit. Both inputs are resolution-confirmed by
/// construction: the anchor variants that yield a ref were confirmed by
/// `classify_anchor`, and `pdf_commit_sha` came back from the commits API.
///
/// `UnresolvedTag` yields `None` — NOT the file commit. That distinction is the
/// whole point: a name that states a ref the repo does not have is a broken
/// record, and dating its drift from the commit that last MOVED the file reports
/// the move as if it were the audit.
pub fn drift_base(anchor: &AuditAnchor, pdf_commit_sha: &str) -> Option<String> {
    match anchor {
        AuditAnchor::Tag { .. } | AuditAnchor::Commit { .. } => {
            anchor.drift_base_ref().map(str::to_string)
        }
        AuditAnchor::Unanchored => (!pdf_commit_sha.is_empty()).then(|| pdf_commit_sha.to_string()),
        AuditAnchor::UnresolvedTag(_) => None,
    }
}

/// Build the GitHub compare-view URL for a repo's audit drift:
/// `https://github.com/{owner}/{repo}/compare/{base}...{head}`. `base` is the
/// RESOLVED audited anchor — `None` when nothing resolved, which is why the
/// parameter is an `Option` rather than a string that may or may not be empty: a
/// non-empty string is not evidence that a ref exists, and building a link from
/// one is how a `compare/v0.1.1...main` that 404s got rendered as a live link.
/// `None` (either side) omits the link rather than pointing at a broken view.
pub fn compare_url(owner: &str, repo: &str, base: Option<&str>, head: &str) -> Option<String> {
    let base = base?;
    if base.is_empty() || head.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{repo}/compare/{base}...{head}"
    ))
}

// ---------------------------------------------------------------------------
// The inherited-audit convention
// ---------------------------------------------------------------------------

/// The mandatory basename prefix that DECLARES an inherited audit record. Exact
/// lowercase, on the basename, and the machine discriminant: inheritance is
/// declared, never inferred from a failed local lookup. Inferring it would
/// classify by symptom — a deleted tag and an undeclared inheritance produce the
/// same failed lookup and must not be handled the same way.
pub const INHERITED_PREFIX: &str = "inherited.";

/// The one non-PDF file permitted in a vendor audit directory: the manifest that
/// says where each `inherited.` PDF came from and what it covers.
pub const INHERITED_MANIFEST: &str = "inherited.json";

/// The manifest schema this scanner understands. A manifest declaring anything
/// else is treated as UNREADABLE in full rather than parsed field-by-field —
/// a future schema's records are not this schema's records.
pub const INHERITED_SCHEMA: u64 = 1;

/// Does this PDF basename declare inherited provenance? The prefix is checked on
/// the basename only, and it also gates the manifest fetch, so a repo with no
/// inherited PDF costs no extra API call.
pub fn is_inherited_pdf(filename: &str) -> bool {
    filename.starts_with(INHERITED_PREFIX)
}

/// The predecessor repo + ref an inherited record points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritedSource {
    pub org: String,
    pub repo: String,
    /// Tag or full commit sha, exactly as printed in the report.
    pub git_ref: String,
}

impl InheritedSource {
    /// A link that actually resolves, showing exactly what was reviewed. The one
    /// URL worth rendering for an inherited record: a cross-repo
    /// `compare/{ref}...main` is not a diff of anything.
    pub fn ref_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/tree/{}",
            self.org, self.repo, self.git_ref
        )
    }
}

/// One snapshot an inherited report covers, with the bytecode hash that pins WHICH
/// bytes the claim is about. The scanner does not verify the hash against the
/// snapshot (that is the CI gate's job — it can read the pointers file); carrying
/// it makes the claim specific enough to be checkable at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveredSnapshot {
    pub snapshot: String,
    pub bytecode_hash: String,
}

/// One record in `audit/<vendor>/inherited.json`: the provenance of ONE inherited
/// PDF. The vendor is carried by the DIRECTORY, not a field — `audit/protofire/`
/// already states these are Protofire reports, and a second copy of that fact
/// could disagree with the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritedRecord {
    /// Basename of the PDF in the same directory. A basename, not a path: the
    /// manifest is colocated with what it describes, and a path invites traversal.
    pub pdf: String,
    pub sha256: String,
    pub report: Option<String>,
    pub report_date: String,
    pub source: InheritedSource,
    /// Possibly EMPTY, and that is load-bearing: it is how a superseded report is
    /// retained honestly — real provenance, zero current coverage. A convention
    /// that forced every PDF to claim a snapshot would manufacture over-claiming.
    pub covers: Vec<CoveredSnapshot>,
    pub superseded_by: Option<String>,
    pub basis: String,
}

/// A parsed `inherited.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritedManifest {
    pub records: Vec<InheritedRecord>,
}

impl InheritedManifest {
    /// The record for a PDF basename, if the manifest carries one.
    pub fn record_for(&self, pdf: &str) -> Option<&InheritedRecord> {
        self.records.iter().find(|r| r.pdf == pdf)
    }
}

/// Parse `audit/<vendor>/inherited.json`. Pure — the caller fetches.
///
/// All-or-nothing: an unknown `schema`, a malformed record, a `pdf` that is not a
/// prefixed basename, a path-shaped field, or a DUPLICATE `pdf` yields `None` for
/// the whole manifest. A provenance claim that is half-readable is not a claim
/// the scanner may act on, and a duplicated `pdf` is ambiguous about which record
/// governs — both must land in `unknown`, not in a best guess.
pub fn parse_inherited_manifest(src: &str) -> Option<InheritedManifest> {
    let v: serde_json::Value = serde_json::from_str(src).ok()?;
    if v.get("schema").and_then(serde_json::Value::as_u64) != Some(INHERITED_SCHEMA) {
        return None;
    }
    let rows = v.get("records")?.as_array()?;
    let mut records: Vec<InheritedRecord> = Vec::with_capacity(rows.len());
    for row in rows {
        let rec = parse_inherited_record(row)?;
        if records.iter().any(|r| r.pdf == rec.pdf) {
            return None;
        }
        records.push(rec);
    }
    Some(InheritedManifest { records })
}

/// A required non-empty string field.
fn req_str(v: &serde_json::Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// True if a field is safe to interpolate into a GitHub API path: no separators
/// and no traversal. Every manifest string reaching a URL passes through here —
/// the manifest is repo content, so it is attacker-influenceable input.
fn is_path_safe(s: &str) -> bool {
    !s.contains('/') && !s.contains('\\') && !s.contains("..") && !s.starts_with('.')
}

fn parse_inherited_record(v: &serde_json::Value) -> Option<InheritedRecord> {
    let pdf = req_str(v, "pdf")?;
    if !is_inherited_pdf(&pdf) || pdf.contains('/') || pdf.contains("..") {
        return None;
    }
    let sha256 = req_str(v, "sha256")?;
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let report_date = req_str(v, "reportDate")?;
    if report_date.len() != 10 || parse_ymd(&report_date).is_none() {
        return None;
    }
    let src = v.get("source")?;
    let source = InheritedSource {
        org: req_str(src, "org")?,
        repo: req_str(src, "repo")?,
        git_ref: req_str(src, "ref")?,
    };
    if !is_path_safe(&source.org) || !is_path_safe(&source.repo) || !is_path_safe(&source.git_ref) {
        return None;
    }
    let covers = v
        .get("covers")?
        .as_array()?
        .iter()
        .map(parse_covered_snapshot)
        .collect::<Option<Vec<_>>>()?;
    let basis = req_str(v, "basis")?;
    Some(InheritedRecord {
        pdf,
        sha256,
        report: req_str(v, "report"),
        report_date,
        source,
        covers,
        superseded_by: req_str(v, "supersededBy"),
        basis,
    })
}

fn parse_covered_snapshot(v: &serde_json::Value) -> Option<CoveredSnapshot> {
    let snapshot = req_str(v, "snapshot")?;
    // A repo-relative directory: never absolute, never traversing out, and never
    // reaching into `.audit/` (the audit SKILL's stamp — a different signal that
    // an external report cannot cover).
    if snapshot.starts_with('/') || snapshot.contains("..") || snapshot.starts_with(".audit") {
        return None;
    }
    let bytecode_hash = req_str(v, "bytecodeHash")?;
    let hex = bytecode_hash.strip_prefix("0x")?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(CoveredSnapshot {
        snapshot,
        bytecode_hash,
    })
}

/// How one PDF's audited state is anchored — the three states the inherited
/// convention defines, disjoint by DECLARATION rather than by symptom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditProvenance {
    /// No `inherited.` prefix: an ordinary local record whose anchor must resolve
    /// in THIS repo.
    Local(AuditAnchor),
    /// Prefixed, with a record whose `source.ref` resolved in the predecessor.
    Inherited(InheritedSource),
    /// Prefixed, but the claim could not be validated: no manifest, no record for
    /// this PDF, an unreadable manifest, or a `source.ref` that does not resolve
    /// there. NEVER falls back to resolving the ref locally or to the PDF's own
    /// file-commit date; the reason is carried so the failure can be reported.
    UnverifiedInherited(&'static str),
}

impl AuditProvenance {
    /// Machine label mirrored into `health.json`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Inherited(_) => "inherited",
            Self::UnverifiedInherited(_) => "unverified-inherited",
        }
    }
}

/// The audit's date and where it came from, from the PDF's provenance and its own
/// file commit. ONE rule, so every PDF is dated the same way:
/// - a resolved anchor (local or inherited) dates the audit;
/// - an unanchored name falls back to the file commit, labelled as such;
/// - anything else is `Unknown` — an unresolved tag and an unvalidated inherited
///   claim both know nothing, and nothing is what they report.
pub fn audit_date_for(
    provenance: &AuditProvenance,
    resolved_date: Option<&str>,
    file_commit_iso: &str,
) -> AuditDate {
    match provenance {
        AuditProvenance::Local(anchor) => match anchor {
            AuditAnchor::Tag { date, .. } | AuditAnchor::Commit { date, .. } => {
                AuditDate::Anchor(date.clone())
            }
            AuditAnchor::Unanchored if !file_commit_iso.is_empty() => {
                AuditDate::FileCommit(file_commit_iso.to_string())
            }
            AuditAnchor::Unanchored | AuditAnchor::UnresolvedTag(_) => AuditDate::Unknown,
        },
        AuditProvenance::Inherited(_) => match resolved_date {
            Some(d) => AuditDate::InheritedAnchor(d.to_string()),
            None => AuditDate::Unknown,
        },
        AuditProvenance::UnverifiedInherited(_) => AuditDate::Unknown,
    }
}

/// The anchor kind of an inherited record's `source.ref`, for `health.json`. A
/// 7–40 hex token is a commit; anything else is the tag the report named.
pub fn inherited_anchor_kind(git_ref: &str) -> &'static str {
    let hex =
        git_ref.len() >= 7 && git_ref.len() <= 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit());
    if hex {
        "commit-inherited"
    } else {
        "tag-inherited"
    }
}

/// Sort key for a snapshot directory: the numeric components of its `M_m_p` name.
/// `None` for a name that is not all-numeric components, which then sorts below
/// every numeric one. Numeric comparison is required, not decoration —
/// lexicographically `0_1_10` sorts BEFORE `0_1_9`, which would report the wrong
/// snapshot as live and so the wrong answer to "is the deployed pin audited?".
pub fn snapshot_order_key(dir_path: &str) -> Option<Vec<u64>> {
    let name = dir_path.rsplit('/').next()?;
    if name.is_empty() {
        return None;
    }
    name.split('_')
        .map(|c| c.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()
}

/// The LIVE snapshot: the highest-versioned directory in the listing. This is the
/// pin production actually runs, so whether IT is covered is the question an
/// inherited audit has to answer.
pub fn live_snapshot(listing: &[String]) -> Option<String> {
    listing
        .iter()
        .max_by(|a, b| {
            snapshot_order_key(a)
                .cmp(&snapshot_order_key(b))
                .then_with(|| a.cmp(b))
        })
        .cloned()
}

/// What an inherited audit covers, and — the part that matters — what it does not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Coverage {
    pub covered: Vec<String>,
    pub uncovered: Vec<String>,
    pub live: Option<String>,
    /// `None` only when there is no snapshot at all to be live.
    pub live_covered: Option<bool>,
}

/// Intersect the manifest's CLAIMS with the snapshot directories that actually
/// exist, and compute the complement from the live tree.
///
/// The uncovered set is computed, never declared. A manifest that listed its own
/// complement would be stale the day a snapshot is added — and the honest state
/// (a new, unaudited pin) is exactly the one that must not be able to go unnoticed.
/// A claim naming a directory that does not exist is dropped rather than trusted:
/// the tree is the authority on what this repo carries.
pub fn coverage(claimed: &[String], listing: &[String]) -> Coverage {
    let mut sorted: Vec<String> = listing.to_vec();
    sorted.sort_by(|a, b| {
        snapshot_order_key(a)
            .cmp(&snapshot_order_key(b))
            .then_with(|| a.cmp(b))
    });
    sorted.dedup();
    let (covered, uncovered): (Vec<String>, Vec<String>) =
        sorted.iter().cloned().partition(|s| claimed.contains(s));
    let live = live_snapshot(&sorted);
    let live_covered = live.as_ref().map(|l| covered.contains(l));
    Coverage {
        covered,
        uncovered,
        live,
        live_covered,
    }
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

/// Both versions of one changed file. `None` on a side means the content could
/// NOT be retrieved; a genuinely added/removed file is passed as `Some("")` by
/// the caller, so absence here always means "unknown", never "empty".
#[derive(Debug, Clone)]
pub struct SourceVersions {
    pub path: String,
    pub base: Option<String>,
    pub head: Option<String>,
    /// GitHub's own counts, used only as the fallback when classification fails.
    pub additions: u64,
    pub deletions: u64,
}

/// Split non-test source drift into CODE and COMMENT churn by lexing both
/// versions of each file, so a NatSpec-only change is visible as comment churn
/// and does NOT read as source change.
///
/// Returns `(drift, fully_classified)`. A file whose content could not be fetched
/// or lexed has its whole churn charged to CODE — the safe direction, since the
/// alternative would show a drifted repo as current — and `fully_classified` goes
/// false so the caller can report the split as a lower bound rather than a fact.
pub fn source_drift_split(files: &[SourceVersions]) -> (crate::commentloc::LineDrift, bool) {
    let mut total = crate::commentloc::LineDrift::default();
    let mut fully_classified = true;
    for f in files {
        if !counts_as_source_drift(&f.path) {
            continue;
        }
        let classified = match (&f.base, &f.head) {
            (Some(b), Some(h)) => crate::commentloc::file_drift(b, h),
            _ => None,
        };
        match classified {
            Some(d) => total.add(&d),
            None => {
                if f.additions > 0 || f.deletions > 0 {
                    fully_classified = false;
                }
                total.code_added += f.additions;
                total.code_removed += f.deletions;
            }
        }
    }
    (total, fully_classified)
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

/// Index of the newest audit PDF by the AUDITED commit/tag date (ISO-8601 UTC —
/// lexicographic order == chronological). The audit date is the audited commit's
/// own timestamp, NOT the PDF file's commit date, so several PDFs moved together
/// in a single commit (which ties their file dates) still order by which audit is
/// genuinely newer.
///
/// Two explicit policies, because both were silently decided before:
/// - a PDF with an UNKNOWN date never wins over one with a known date. An empty
///   string sorted below every real date only by accident of `String` ordering.
/// - ties break to the FIRST index, not the last. `max_by` returns the last
///   maximum, so once a bulk move collapsed every date onto one value, the
///   "newest audit" was decided by path sort order — which is how the wrong PDF
///   became the reference for the split repo (#128). First-wins is no more
///   meaningful, but it is stated, and it is stable under a later insertion.
pub fn newest_pdf_index(pdfs: &[AuditPdf]) -> Option<usize> {
    let best_known = pdfs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.audit_date.iso().map(|d| (i, d)))
        .fold(None::<(usize, &str)>, |acc, (i, d)| match acc {
            Some((_, bd)) if bd >= d => acc,
            _ => Some((i, d)),
        });
    match best_known {
        Some((i, _)) => Some(i),
        // Nothing is dated: nothing distinguishes them, so take the first rather
        // than inventing an order.
        None => (!pdfs.is_empty()).then_some(0),
    }
}

/// Index of the REFERENCE audit PDF — the one the repo's verdict, anchor and
/// coverage are computed against.
///
/// When any PDF's record claims coverage, the reference is the newest among THOSE.
/// A superseded report with `covers: []` is real provenance and is kept, but it
/// describes nothing this repo currently carries, so it must not be the record the
/// repo is judged by. Picking by meaning beats picking by date or path order:
/// nothing guarantees the covering report is also the newest file.
pub fn reference_pdf_index(pdfs: &[AuditPdf]) -> Option<usize> {
    let covering: Vec<AuditPdf> = pdfs
        .iter()
        .filter(|p| !p.covers.is_empty())
        .cloned()
        .collect();
    if covering.is_empty() {
        return newest_pdf_index(pdfs);
    }
    let chosen = newest_pdf_index(&covering)?;
    pdfs.iter().position(|p| p.path == covering[chosen].path)
}

/// Strictly-newer comparison for two ISO-8601 UTC timestamps. Same fixed format
/// (`YYYY-MM-DDTHH:MM:SSZ`) ⇒ lexicographic order == chronological order.
pub fn newer_than(a_iso: &str, b_iso: &str) -> bool {
    a_iso > b_iso
}

/// Is the audit stale? Stale means the audited SOURCE changed since the audit —
/// NOT merely that a newer tag exists. Protofire audits the Solidity, so the
/// non-test `.sol` drift is the whole question.
///
/// A newer tag alone proves nothing: the release machinery manufactures tags
/// with no Solidity in them. `rainix-autopublish` bumps `[package].version` and
/// tags every release, so a version bump, a CI change or a docs edit yields a
/// newer tag whose Solidity diff is zero — the audit still covers exactly the
/// contracts that are there, so it is CURRENT. Flagging those stale trains
/// readers to ignore the column, which buries the genuinely stale ones.
///
/// Both inputs are `None` when that fact could not be established, and the
/// result is `None` when neither could — a scan that learned nothing about a
/// repo must not answer "current". Reporting no evidence as clean is the one
/// failure that empties the stale list precisely when the scan is broken.
///
/// Measured drift decides on its own. Only when drift is unmeasurable does tag
/// recency stand in for it, and only when the tag is genuinely known: a failed
/// tag lookup is not "no newer tag".
pub fn is_stale(newer_tag_exists: Option<bool>, source_changed: Option<bool>) -> Option<bool> {
    source_changed.or(newer_tag_exists)
}

/// Whether a tag newer than the audit anchor exists, `None` when EITHER date
/// could not be read.
///
/// The `Option`s are the whole point, so this is not inlined at the call site: a
/// failed tag lookup is not evidence that no newer tag exists, and collapsing
/// it to `false` is what let a scan which learned nothing report every audited
/// repo `current`. An unknown AUDIT date is the same failure from the other side —
/// every tag is "newer" than a date that does not exist.
pub fn newer_tag_exists(latest_tag_iso: Option<&str>, audited_date: Option<&str>) -> Option<bool> {
    match (latest_tag_iso, audited_date) {
        (Some(t), Some(a)) => Some(newer_than(t, a)),
        _ => None,
    }
}

/// Sort rank for the audited-repos list: stale first, then unknown, then
/// current. Unknown outranks current because a verdict the scan could not
/// establish still needs a human look, and ranks below stale because confirmed
/// drift needs one more.
pub fn staleness_rank(is_stale: Option<bool>) -> u8 {
    match is_stale {
        Some(true) => 2,
        None => 1,
        Some(false) => 0,
    }
}

/// Coverage taxonomy from the facts the classification turns on. Staleness is
/// delegated to `is_stale` so the rendered verdict and the `is_stale` flag cannot
/// disagree about what stale means.
///
/// This is the LOCAL taxonomy only. `Inherited` and the `Unknown` an unvalidated
/// inherited claim earns are decided upstream by PROVENANCE, before drift is even
/// a question: whether an audit performed elsewhere covers this repo's snapshots
/// is not something a diff against a local ref can answer.
pub fn classify_external_audit(
    has_pdf: bool,
    has_tags: bool,
    newer_tag_exists: Option<bool>,
    source_changed: Option<bool>,
) -> ExternalAudit {
    if !has_pdf {
        ExternalAudit::Never
    } else if !has_tags {
        ExternalAudit::Na
    } else {
        match is_stale(newer_tag_exists, source_changed) {
            Some(true) => ExternalAudit::Stale,
            Some(false) => ExternalAudit::Current,
            None => ExternalAudit::Unknown,
        }
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

    fn sv(
        path: &str,
        base: Option<&str>,
        head: Option<&str>,
        add: u64,
        del: u64,
    ) -> SourceVersions {
        SourceVersions {
            path: path.to_string(),
            base: base.map(str::to_string),
            head: head.map(str::to_string),
            additions: add,
            deletions: del,
        }
    }

    /// The whole point: a NatSpec-only edit is comment churn, so the audited
    /// source did NOT change and the repo must not read stale.
    #[test]
    fn a_comment_only_change_is_not_source_change() {
        let base = "contract C {\n    /// Old wording.\n    uint256 x;\n}";
        let head = "contract C {\n    /// New wording.\n    uint256 x;\n}";
        let (d, classified) = source_drift_split(&[sv("src/A.sol", Some(base), Some(head), 1, 1)]);
        assert!(d.comment_added > 0 && d.comment_removed > 0);
        assert!(
            !d.code_changed(),
            "comment churn must not read as source change"
        );
        assert!(classified);
        // and therefore the audit stays CURRENT
        assert_eq!(is_stale(Some(true), Some(d.code_changed())), Some(false));
        assert_eq!(
            classify_external_audit(true, true, Some(true), Some(d.code_changed())),
            ExternalAudit::Current
        );
    }

    /// A real code edit still marks it stale, with comments counted apart.
    #[test]
    fn code_change_still_marks_stale_with_comments_counted_apart() {
        let base = "contract C {\n    /// Doc.\n    uint256 x = 1;\n}";
        let head = "contract C {\n    /// Doc reworded.\n    uint256 x = 2;\n}";
        let (d, _) = source_drift_split(&[sv("src/A.sol", Some(base), Some(head), 2, 2)]);
        assert!(d.comment_added > 0, "comment churn counted");
        assert!(d.code_changed(), "code churn still detected");
        assert_eq!(is_stale(Some(false), Some(d.code_changed())), Some(true));
    }

    /// Test files never contribute — same predicate the LOC totals use.
    #[test]
    fn test_files_are_excluded_from_the_split() {
        let base = "contract T {\n    uint256 x = 1;\n}";
        let head = "contract T {\n    uint256 x = 2;\n}";
        let (d, _) = source_drift_split(&[sv("test/src/A.t.sol", Some(base), Some(head), 1, 1)]);
        assert_eq!(d, crate::commentloc::LineDrift::default());
        assert!(!d.code_changed());
    }

    /// Content that could not be fetched cannot be classified, so its churn is
    /// charged to CODE (keeping the repo stale) and the split is flagged — never
    /// silently reported as comment-only.
    #[test]
    fn unclassifiable_churn_counts_as_code_and_is_flagged() {
        let (d, classified) =
            source_drift_split(&[sv("src/A.sol", None, Some("contract C {}"), 40, 3)]);
        assert_eq!(d.code_added, 40);
        assert_eq!(d.code_removed, 3);
        assert_eq!(d.comment_added, 0);
        assert!(d.code_changed(), "unmeasured must not read as unchanged");
        assert!(
            !classified,
            "caller must be told the split is not authoritative"
        );
    }

    /// A newly ADDED file is classifiable: the caller passes an empty base, so its
    /// comments and code are counted properly rather than lumped into code.
    #[test]
    fn an_added_file_is_still_split() {
        let head = "contract C {\n    /// Doc.\n    uint256 x = 1;\n}";
        let (d, classified) = source_drift_split(&[sv("src/A.sol", Some(""), Some(head), 3, 0)]);
        assert!(classified);
        assert!(d.code_added > 0);
        assert!(d.comment_added > 0);
    }

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
    fn classify_resolves_the_tag_and_carries_its_date() {
        // A vX.Y.Z name is tag-anchored ONLY once the tag resolves, and the same
        // call that resolves it dates it — the anchor cannot exist undated.
        let anchor = classify_anchor("rain.factory.v0.1.1-r2.0.may-2026.pdf", |r| {
            assert_eq!(r, "v0.1.1"); // exactly the filename token is resolved
            Some("2026-05-12T15:15:26Z".to_string())
        });
        assert_eq!(
            anchor,
            AuditAnchor::Tag {
                tag: "v0.1.1".into(),
                date: "2026-05-12T15:15:26Z".into()
            }
        );
        assert_eq!(anchor.kind(), "tag");
        assert_eq!(anchor.drift_base_ref(), Some("v0.1.1"));
        assert_eq!(anchor.resolved_date(), Some("2026-05-12T15:15:26Z"));
        assert!(anchor.names_tag());
    }

    /// THE #128 FIX. A tag used to be trusted on sight, so a PDF carried into a
    /// repo where its tag does not exist still reported `anchorKind: "tag"`, a
    /// date, and a compare link — all invented. An unresolvable tag now yields a
    /// distinct state that offers NO ref, NO date and NO compare base. Against the
    /// pre-fix code this assertion fails: it returned `Tag("v0.1.1")`.
    #[test]
    fn an_unresolvable_tag_is_broken_not_anchored_and_yields_no_date_or_base() {
        let anchor = classify_anchor("rain.factory.v0.1.1-r2.0.may-2026.pdf", |_| None);
        assert_eq!(anchor, AuditAnchor::UnresolvedTag("v0.1.1".into()));
        assert_eq!(anchor.kind(), "unresolved-tag");
        assert_eq!(
            anchor.drift_base_ref(),
            None,
            "a ref that does not resolve is not a drift base"
        );
        assert_eq!(anchor.resolved_date(), None);
        assert_ne!(
            anchor.kind(),
            "tag",
            "must not claim a tag anchor nobody could find"
        );
        // The naming convention IS followed — the tag is present, just broken.
        assert!(anchor.names_tag());
        // And no compare URL can be built from it (the 404 link the split published).
        assert_eq!(
            compare_url("rainlanguage", "rain.factory.deploy", None, "main"),
            None
        );
        // Nor is its date faked from the commit that last touched the file.
        assert_eq!(
            audit_date_for(
                &AuditProvenance::Local(anchor),
                None,
                "2026-07-24T08:42:11Z"
            ),
            AuditDate::Unknown
        );
    }

    #[test]
    fn classify_commit_anchored_when_hex_resolves() {
        // raindex's PDF: a hex token that resolves → commit-anchored at e686b4d.
        let anchor = classify_anchor("raindex.e686b4d.apr-2026.pdf", |sha| {
            assert_eq!(sha, "e686b4d"); // exactly the filename token is resolved
            Some("2026-04-01T00:00:00Z".to_string())
        });
        assert_eq!(
            anchor,
            AuditAnchor::Commit {
                sha: "e686b4d".into(),
                date: "2026-04-01T00:00:00Z".into()
            }
        );
        assert_eq!(anchor.kind(), "commit");
        assert_eq!(anchor.drift_base_ref(), Some("e686b4d"));
        assert!(!anchor.names_tag());
    }

    #[test]
    fn classify_unanchored_when_hex_does_not_resolve() {
        // a hex-looking token that resolution rejects falls back to unanchored — a
        // false positive must never error or masquerade as a commit anchor. Unlike
        // an unresolved TAG this is not a broken claim: an all-hex word in a
        // date-only name never asserted a ref in the first place.
        let anchor = classify_anchor("raindex.deadbeef.jan-2026.pdf", |_| None);
        assert_eq!(anchor, AuditAnchor::Unanchored);
        assert_eq!(anchor.kind(), "unanchored");
        assert_eq!(anchor.drift_base_ref(), None);
        // So it KEEPS the file-commit fallback, labelled as such.
        assert_eq!(
            audit_date_for(
                &AuditProvenance::Local(anchor),
                None,
                "2026-01-09T00:00:00Z"
            ),
            AuditDate::FileCommit("2026-01-09T00:00:00Z".into())
        );
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

    // ---- drift_base ----
    #[test]
    fn drift_base_uses_the_resolved_anchor_and_never_a_broken_one() {
        let dated = |d: &str| d.to_string();
        assert_eq!(
            drift_base(
                &AuditAnchor::Tag {
                    tag: "v0.1.1".into(),
                    date: dated("2026-05-12T00:00:00Z")
                },
                "filesha"
            )
            .as_deref(),
            Some("v0.1.1")
        );
        assert_eq!(
            drift_base(
                &AuditAnchor::Commit {
                    sha: "e686b4d".into(),
                    date: dated("2026-04-01T00:00:00Z")
                },
                "filesha"
            )
            .as_deref(),
            Some("e686b4d")
        );
        // Unanchored keeps the legacy fallback to the PDF's own commit …
        assert_eq!(
            drift_base(&AuditAnchor::Unanchored, "filesha").as_deref(),
            Some("filesha")
        );
        assert_eq!(drift_base(&AuditAnchor::Unanchored, "").as_deref(), None);
        // … but a STATED tag that does not resolve gets no base at all. Falling
        // back here measures drift from the commit that last moved the PDF.
        assert_eq!(
            drift_base(&AuditAnchor::UnresolvedTag("v0.1.1".into()), "filesha"),
            None
        );
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
                status: String::new(),
            }, // +10 / −5  counts (src/ Solidity)
            CompareFile {
                filename: "deploy/D.sol".into(),
                additions: 2,
                deletions: 1,
                status: String::new(),
            }, // +2 / −1   counts (deploy/ Solidity is audited surface)
            CompareFile {
                filename: "test/A.t.sol".into(),
                additions: 99,
                deletions: 99,
                status: String::new(),
            }, // excluded (test) — symmetric so a leak inflates BOTH counts
            CompareFile {
                filename: "src/B.rs".into(),
                additions: 3,
                deletions: 2,
                status: String::new(),
            }, // excluded (.rs — not Solidity, not audited surface)
            CompareFile {
                filename: "README.md".into(),
                additions: 40,
                deletions: 40,
                status: String::new(),
            }, // excluded (non-source)
            CompareFile {
                filename: "src/C.ts".into(),
                additions: 1,
                deletions: 0,
                status: String::new(),
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

    // ---- newest_pdf_index / reference_pdf_index ----

    /// Build a local-record PDF with a known audit date.
    fn pdf(f: &str, audit: AuditDate) -> AuditPdf {
        AuditPdf {
            filename: f.into(),
            path: format!("audit/protofire/{f}"),
            last_commit_iso: "2026-07-14T00:00:00Z".into(),
            commit_sha: "sha".into(),
            audit_date: audit,
            provenance: AuditProvenance::Local(AuditAnchor::Unanchored),
            covers: Vec::new(),
        }
    }
    fn dated(iso: &str) -> AuditDate {
        AuditDate::Anchor(iso.into())
    }

    #[test]
    fn newest_pdf_is_by_audit_date_not_file_commit_date() {
        // Every PDF shares ONE file-commit date (as when a batch of PDFs is moved
        // into audit/protofire/ in a single commit) — so file-commit date can't
        // distinguish them. The AUDITED commit/tag date must.
        let pdfs = vec![
            pdf("old.pdf", dated("2025-01-01T00:00:00Z")),
            pdf("new.pdf", dated("2026-05-12T00:00:00Z")),
            pdf("mid.pdf", dated("2026-02-01T00:00:00Z")),
        ];
        // Newest is the newest AUDIT (index 1), even though all file dates tie.
        assert_eq!(newest_pdf_index(&pdfs), Some(1));
        assert_eq!(newest_pdf_index(&[]), None);
    }

    /// An undated PDF must never outrank a dated one, and ties must not be decided
    /// by position. `max_by` returned the LAST maximum, so once a bulk move
    /// collapsed every date the reference was picked by path sort order (#128).
    #[test]
    fn unknown_dates_never_win_and_ties_break_to_the_first() {
        let pdfs = vec![
            pdf("a.pdf", AuditDate::Unknown),
            pdf("b.pdf", dated("2026-02-01T00:00:00Z")),
            pdf("c.pdf", AuditDate::Unknown),
        ];
        assert_eq!(
            newest_pdf_index(&pdfs),
            Some(1),
            "the only DATED audit is the newest one"
        );
        let tied = vec![
            pdf("a.pdf", dated("2026-02-01T00:00:00Z")),
            pdf("b.pdf", dated("2026-02-01T00:00:00Z")),
        ];
        assert_eq!(newest_pdf_index(&tied), Some(0), "ties break to the first");
        // Nothing dated at all: still a deterministic answer, not a panic.
        let blind = vec![
            pdf("a.pdf", AuditDate::Unknown),
            pdf("b.pdf", AuditDate::Unknown),
        ];
        assert_eq!(newest_pdf_index(&blind), Some(0));
    }

    /// The reference is the record that covers something. A superseded report is
    /// real provenance and is kept, but it describes nothing this repo carries, so
    /// judging the repo by it would report coverage that does not exist. Picking by
    /// MEANING also survives the case the date rule gets wrong: here the covering
    /// report is neither the newest nor the first.
    #[test]
    fn reference_prefers_a_record_that_covers_something() {
        let mut covering = pdf("inherited.r2.pdf", dated("2026-05-12T00:00:00Z"));
        covering.covers = vec!["src/generated/0_1_3".into()];
        let pdfs = vec![
            pdf("inherited.r1.pdf", dated("2026-02-11T00:00:00Z")),
            covering,
            pdf("inherited.r3.pdf", dated("2026-06-01T00:00:00Z")),
        ];
        assert_eq!(
            reference_pdf_index(&pdfs),
            Some(1),
            "the covering record is the reference even though a newer one exists"
        );
        // With nothing covering anything, it falls back to plain recency.
        let none_cover = vec![
            pdf("a.pdf", dated("2026-02-11T00:00:00Z")),
            pdf("b.pdf", dated("2026-06-01T00:00:00Z")),
        ];
        assert_eq!(reference_pdf_index(&none_cover), Some(1));
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
    fn stale_iff_audited_source_changed() {
        // The bug this pins: a newer tag with NO Solidity change is NOT stale.
        // rainix-autopublish tags every release, so a version bump alone yields a
        // newer tag the audit still covers exactly.
        assert_eq!(is_stale(Some(true), Some(false)), Some(false));
        assert_eq!(is_stale(Some(false), Some(false)), Some(false));
        // Source actually changed -> stale, tag or no tag.
        assert_eq!(is_stale(Some(true), Some(true)), Some(true));
        assert_eq!(is_stale(Some(false), Some(true)), Some(true));
        // Unknown is not zero: with no measurement, fall back to tag recency
        // rather than claiming current on no evidence.
        assert_eq!(is_stale(Some(true), None), Some(true));
        assert_eq!(is_stale(Some(false), None), Some(false));
    }

    /// A scan that established neither fact answers UNKNOWN. Collapsing that to
    /// "not stale" empties the stale list exactly when the scan is broken — the
    /// failure mode a rate-limited run actually produced.
    #[test]
    fn no_evidence_at_all_is_unknown_never_current() {
        assert_eq!(
            is_stale(None, None),
            None,
            "an unknown tag plus unmeasurable drift is not evidence of currency"
        );
        // A known fact on either side is still decisive.
        assert_eq!(is_stale(None, Some(true)), Some(true));
        assert_eq!(is_stale(None, Some(false)), Some(false));
        assert_eq!(is_stale(Some(true), None), Some(true));
    }

    /// An unreadable latest tag is unknown, NOT "no newer tag". Pinned here
    /// because collapsing it at the call site is invisible to every other test.
    #[test]
    fn an_unreadable_latest_tag_is_unknown_not_absent() {
        assert_eq!(
            newer_tag_exists(None, Some("2026-01-01T00:00:00Z")),
            None,
            "a failed tag lookup must not read as 'no newer tag exists'"
        );
        assert_eq!(
            newer_tag_exists(Some("2026-06-01T00:00:00Z"), Some("2026-01-01T00:00:00Z")),
            Some(true)
        );
        assert_eq!(
            newer_tag_exists(Some("2025-06-01T00:00:00Z"), Some("2026-01-01T00:00:00Z")),
            Some(false)
        );
        // The same failure from the other side: with no AUDIT date, every tag
        // trivially looks newer. Unknown, not stale.
        assert_eq!(
            newer_tag_exists(Some("2026-06-01T00:00:00Z"), None),
            None,
            "an unknown audit date is not evidence that a tag is newer than it"
        );
    }

    /// The verdict a rate-limited scan actually produced: every fact unknown,
    /// rendered `current`. It must classify UNKNOWN.
    #[test]
    fn a_scan_that_established_nothing_classifies_unknown_not_current() {
        assert_eq!(
            classify_external_audit(true, true, None, None),
            ExternalAudit::Unknown
        );
    }

    /// Unknown sorts between stale and current: it needs a look, but less than
    /// confirmed drift. Ranking it with `current` would bury a failed lookup at
    /// the bottom of the list.
    #[test]
    fn unknown_outranks_current_but_not_stale() {
        assert!(staleness_rank(Some(true)) > staleness_rank(None));
        assert!(staleness_rank(None) > staleness_rank(Some(false)));
    }

    // ---- classify_external_audit ----
    #[test]
    fn classification_taxonomy() {
        assert_eq!(
            classify_external_audit(false, false, Some(false), None),
            ExternalAudit::Never
        );
        assert_eq!(
            classify_external_audit(false, true, Some(true), Some(true)),
            ExternalAudit::Never
        ); // no PDF dominates
        assert_eq!(
            classify_external_audit(true, false, Some(false), None),
            ExternalAudit::Na
        ); // PDF but no tags
        assert_eq!(
            classify_external_audit(true, true, Some(true), Some(true)),
            ExternalAudit::Stale
        );
        assert_eq!(
            classify_external_audit(true, true, Some(false), Some(false)),
            ExternalAudit::Current
        );
        // The reported bug: a newer tag with zero Solidity drift renders CURRENT,
        // not STALE — the verdict follows the source, not the tag.
        assert_eq!(
            classify_external_audit(true, true, Some(true), Some(false)),
            ExternalAudit::Current
        );
        // Unmeasured drift still falls back to tag recency.
        assert_eq!(
            classify_external_audit(true, true, Some(true), None),
            ExternalAudit::Stale
        );
    }

    // ---- the inherited-audit convention ----

    /// The real manifest shape, as `rain.factory.deploy` carries it: r2.0 covers
    /// the two byte-identical snapshots built from the audited source, and r1.0 is
    /// retained with ZERO coverage.
    fn manifest_json() -> String {
        r#"{
          "schema": 1,
          "records": [
            {
              "pdf": "inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf",
              "sha256": "6c4ecf9f47ebf96cda58a856901ccd25dd83b3a8b0e8524a00adb60f149894ca",
              "report": "r2.0",
              "reportDate": "2026-05-27",
              "source": { "org": "rainlanguage", "repo": "rain.factory", "ref": "v0.1.1" },
              "covers": [
                { "snapshot": "src/generated/0_1_3", "bytecodeHash": "0xf21b813c7075a1621285df3a8369d0652c31ea80cb807be1aaadafeecd134475" },
                { "snapshot": "src/generated/0_1_4", "bytecodeHash": "0xf21b813c7075a1621285df3a8369d0652c31ea80cb807be1aaadafeecd134475" }
              ],
              "basis": "rebuilt from the audited source; reproduces both snapshots byte-for-byte"
            },
            {
              "pdf": "inherited.rain.factory.1a92a86.feb-2026.pdf",
              "sha256": "ffb52e9f5d84eaa66c35b760aaf4b8d1ebcd6ac10b2da1ab601cc1f3f272ec61",
              "report": "r1.0",
              "reportDate": "2026-02-11",
              "source": { "org": "rainlanguage", "repo": "rain.factory", "ref": "1a92a8688249aa5a8f4e82d5ed584604515f1ea0" },
              "covers": [],
              "supersededBy": "inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf",
              "basis": "an earlier state of the same source; no snapshot here builds from it"
            }
          ]
        }"#
        .to_string()
    }

    #[test]
    fn the_prefix_is_the_discriminant_and_survives_anchor_parsing() {
        assert!(is_inherited_pdf(
            "inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf"
        ));
        assert!(!is_inherited_pdf("rain.factory.v0.1.1-r2.0.may-2026.pdf"));
        // Case-exact: `Inherited.` is not the marker.
        assert!(!is_inherited_pdf("Inherited.rain.factory.v0.1.1.pdf"));
        // The prefix must not break the anchor parsers it sits in front of, and
        // must not itself look like a commit candidate.
        assert_eq!(
            parse_audited_tag("inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf").as_deref(),
            Some("v0.1.1")
        );
        assert_eq!(
            parse_commit_candidate("inherited.rain.factory.1a92a86.feb-2026.pdf").as_deref(),
            Some("1a92a86")
        );
        assert_eq!(parse_commit_candidate("inherited.x.pdf"), None);
    }

    #[test]
    fn parses_the_manifest_including_an_empty_covers() {
        let m = parse_inherited_manifest(&manifest_json()).expect("valid manifest");
        assert_eq!(m.records.len(), 2);
        let r2 = m
            .record_for("inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf")
            .expect("r2 record");
        assert_eq!(r2.source.org, "rainlanguage");
        assert_eq!(r2.source.repo, "rain.factory");
        assert_eq!(r2.source.git_ref, "v0.1.1");
        assert_eq!(r2.report_date, "2026-05-27");
        assert_eq!(r2.covers.len(), 2);
        assert_eq!(r2.covers[0].snapshot, "src/generated/0_1_3");
        assert_eq!(
            r2.source.ref_url(),
            "https://github.com/rainlanguage/rain.factory/tree/v0.1.1"
        );
        // An EMPTY covers is legal and load-bearing: real provenance, no current
        // coverage. Forcing every report to claim a snapshot would manufacture the
        // over-claim the convention exists to prevent.
        let r1 = m
            .record_for("inherited.rain.factory.1a92a86.feb-2026.pdf")
            .expect("r1 record");
        assert!(r1.covers.is_empty());
        assert_eq!(
            r1.superseded_by.as_deref(),
            Some("inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf")
        );
        assert_eq!(inherited_anchor_kind(&r2.source.git_ref), "tag-inherited");
        assert_eq!(
            inherited_anchor_kind(&r1.source.git_ref),
            "commit-inherited"
        );
    }

    /// Half-readable provenance is not provenance. Every one of these yields
    /// `None` for the WHOLE manifest, which the scanner reports as `unknown` —
    /// never a partially-trusted record, and never a fabricated date.
    #[test]
    fn an_unreadable_manifest_is_rejected_whole_never_partly() {
        let base = manifest_json();
        assert!(parse_inherited_manifest("not json").is_none());
        // A future schema's records are not this schema's records.
        assert!(
            parse_inherited_manifest(&base.replace("\"schema\": 1", "\"schema\": 2")).is_none()
        );
        assert!(
            parse_inherited_manifest(&base.replace("\"schema\": 1", "\"schema\": \"1\"")).is_none()
        );
        // A record for a PDF that does not declare the prefix.
        assert!(parse_inherited_manifest(&base.replace(
            "\"pdf\": \"inherited.rain.factory.v0.1.1",
            "\"pdf\": \"rain.factory.v0.1.1"
        ))
        .is_none());
        // A malformed date, a short hash, a missing basis: each kills the manifest.
        assert!(parse_inherited_manifest(&base.replace("2026-05-27", "may 2026")).is_none());
        assert!(parse_inherited_manifest(&base.replace("2026-05-27", "2026-13-01")).is_none());
        assert!(parse_inherited_manifest(&base.replace(
            "6c4ecf9f47ebf96cda58a856901ccd25dd83b3a8b0e8524a00adb60f149894ca",
            "deadbeef"
        ))
        .is_none());
        assert!(parse_inherited_manifest(&base.replace(
            "\"basis\": \"rebuilt from the audited source; reproduces both snapshots byte-for-byte\"",
            "\"basis\": \"\""
        ))
        .is_none());
        // A DUPLICATE pdf is ambiguous about which record governs.
        let dup = base.replace(
            "inherited.rain.factory.1a92a86.feb-2026.pdf",
            "inherited.rain.factory.v0.1.1-r2.0.may-2026.pdf",
        );
        assert!(
            parse_inherited_manifest(&dup).is_none(),
            "two records for one PDF must not silently pick one"
        );
    }

    /// The manifest is repo content, so it is attacker-influenceable input that
    /// reaches an API path and a rendered link. Anything path-shaped is refused
    /// rather than sanitised at the point of use.
    #[test]
    fn path_shaped_manifest_fields_are_refused() {
        let base = manifest_json();
        for (from, to) in [
            ("\"repo\": \"rain.factory\"", "\"repo\": \"../../evil\""),
            ("\"org\": \"rainlanguage\"", "\"org\": \"a/b\""),
            ("\"ref\": \"v0.1.1\"", "\"ref\": \"../heads/main\""),
        ] {
            assert!(
                parse_inherited_manifest(&base.replace(from, to)).is_none(),
                "{to} must be refused"
            );
        }
        // Snapshot paths: no traversal, no absolute, and never into `.audit/` —
        // the audit SKILL's stamp is a different signal an external report cannot
        // cover.
        for to in [
            "\"snapshot\": \"../../etc\"",
            "\"snapshot\": \"/etc\"",
            "\"snapshot\": \".audit\"",
        ] {
            assert!(
                parse_inherited_manifest(
                    &base.replace("\"snapshot\": \"src/generated/0_1_3\"", to)
                )
                .is_none(),
                "{to} must be refused"
            );
        }
    }

    /// The complement is computed from the LIVE TREE, never read from the
    /// manifest. That is what makes the record self-correcting: a snapshot added
    /// later shows up uncovered, and the live pin's coverage flips, with no
    /// manifest edit — the honest state cannot go unnoticed.
    #[test]
    fn coverage_computes_the_uncovered_set_from_the_tree() {
        let claimed = vec![
            "src/generated/0_1_3".to_string(),
            "src/generated/0_1_4".to_string(),
        ];
        let listing = vec![
            "src/generated/0_1_5".to_string(),
            "src/generated/0_1_3".to_string(),
            "src/generated/0_1_4".to_string(),
        ];
        let c = coverage(&claimed, &listing);
        assert_eq!(
            c.covered,
            vec!["src/generated/0_1_3", "src/generated/0_1_4"]
        );
        assert_eq!(
            c.uncovered,
            vec!["src/generated/0_1_5"],
            "the live pin is uncovered and must be named"
        );
        assert_eq!(c.live.as_deref(), Some("src/generated/0_1_5"));
        assert_eq!(
            c.live_covered,
            Some(false),
            "the pin production actually runs is NOT audited"
        );

        // A later snapshot lands and nobody touches the manifest: it is uncovered
        // and it becomes the live pin, automatically.
        let mut grown = listing.clone();
        grown.push("src/generated/0_1_6".to_string());
        let c2 = coverage(&claimed, &grown);
        assert_eq!(c2.live.as_deref(), Some("src/generated/0_1_6"));
        assert_eq!(c2.uncovered.len(), 2);
        assert_eq!(c2.live_covered, Some(false));

        // A claim naming a directory that does not exist is dropped: the tree is
        // the authority on what this repo carries.
        let c3 = coverage(&["src/generated/9_9_9".to_string()], &listing);
        assert!(c3.covered.is_empty());
        assert_eq!(c3.uncovered.len(), 3);
    }

    /// Every snapshot covered ⇒ the live pin is covered. The flag must be able to
    /// say YES, or `false` carries no information.
    #[test]
    fn full_coverage_reports_the_live_pin_covered() {
        let dirs = vec![
            "src/generated/0_1_3".to_string(),
            "src/generated/0_1_4".to_string(),
        ];
        let c = coverage(&dirs, &dirs);
        assert!(c.uncovered.is_empty());
        assert_eq!(c.live_covered, Some(true));
        // No snapshots at all: nothing is live, so the flag is absent rather than
        // a `false` about a pin that does not exist.
        let empty = coverage(&[], &[]);
        assert_eq!(empty.live, None);
        assert_eq!(empty.live_covered, None);
    }

    /// Snapshot versions compare NUMERICALLY. Lexicographically `0_1_10` sorts
    /// before `0_1_9`, which would name the wrong directory as the live pin — and
    /// so answer the only question that matters ("is the deployed pin audited?")
    /// about the wrong deployment.
    #[test]
    fn the_live_snapshot_is_the_highest_version_not_the_last_string() {
        let dirs = vec![
            "src/generated/0_1_9".to_string(),
            "src/generated/0_1_10".to_string(),
            "src/generated/0_1_2".to_string(),
        ];
        assert_eq!(
            live_snapshot(&dirs).as_deref(),
            Some("src/generated/0_1_10")
        );
        assert_eq!(
            snapshot_order_key("src/generated/0_1_10"),
            Some(vec![0, 1, 10])
        );
        // A non-numeric name has no version to compare, so it never outranks one
        // that does — but it is still listed rather than dropped.
        let mixed = vec![
            "src/generated/legacy".to_string(),
            "src/generated/0_1_3".to_string(),
        ];
        assert_eq!(snapshot_order_key("src/generated/legacy"), None);
        assert_eq!(
            live_snapshot(&mixed).as_deref(),
            Some("src/generated/0_1_3")
        );
        assert_eq!(coverage(&[], &mixed).uncovered.len(), 2);
    }

    /// An inherited date is dated by the SOURCE ref, and labelled so it can never
    /// be confused with a local anchor or a file-move date.
    #[test]
    fn an_inherited_record_dates_from_the_source_ref() {
        let src = InheritedSource {
            org: "rainlanguage".into(),
            repo: "rain.factory".into(),
            git_ref: "v0.1.1".into(),
        };
        let d = audit_date_for(
            &AuditProvenance::Inherited(src.clone()),
            Some("2026-05-12T15:15:26Z"),
            "2026-07-24T08:42:11Z",
        );
        assert_eq!(d.iso(), Some("2026-05-12T15:15:26Z"));
        assert_eq!(d.source(), "inherited-anchor");
        assert_ne!(
            d.iso(),
            Some("2026-07-24T08:42:11Z"),
            "the split commit is not the audit date"
        );
        // A DECLARED inherited claim that could not be validated knows nothing —
        // and must not quietly borrow the file-commit date.
        let bad = audit_date_for(
            &AuditProvenance::UnverifiedInherited("no record for this PDF"),
            None,
            "2026-07-24T08:42:11Z",
        );
        assert_eq!(bad, AuditDate::Unknown);
        assert_eq!(bad.iso(), None);
        assert_eq!(bad.source(), "unknown");
        assert_eq!(AuditProvenance::Inherited(src).kind(), "inherited");
    }

    #[test]
    fn inherited_is_its_own_verdict_string() {
        assert_eq!(ExternalAudit::Inherited.as_str(), "inherited");
        // Distinct from every other verdict — it is neither a clean bill of health
        // nor a confirmed gap.
        for other in [
            ExternalAudit::Never,
            ExternalAudit::Na,
            ExternalAudit::Stale,
            ExternalAudit::Current,
            ExternalAudit::Unknown,
        ] {
            assert_ne!(ExternalAudit::Inherited.as_str(), other.as_str());
        }
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
