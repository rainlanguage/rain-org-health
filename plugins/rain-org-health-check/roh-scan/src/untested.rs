//! Flags EXTERNAL/PUBLIC functions on concrete Solidity contracts that no test
//! references — the "untested external surface" check
//! (rainlanguage/claude-audit-skills#54). External/public functions are a
//! contract's API and attack surface; one with zero test coverage is a latent
//! risk (rain.math.float#156: three external `format()` overloads sat uncovered
//! for months until someone noticed by hand).
//!
//! Pure logic only — the caller (main.rs) shallow-clones the repo and hands the
//! `.sol` files here. Parsing is [`solang_parser`]'s real AST, the same parser
//! family `forge fmt` uses, so a `contract` keyword in a comment or string can
//! never masquerade as a declaration.
//!
//! ## What "untested" claims, and the restraint behind it
//! The check ENUMERATES every external/public function declared on every
//! concrete `contract` (never `abstract contract`, `interface` or `library` —
//! those are not deployed API), then greps the repo's own test sources for the
//! function's name as a whole identifier. Only a function whose name appears
//! NOWHERE in any test source is flagged. That is deliberately generous toward
//! coverage — the audit skill's own test-coverage dimension warns that a
//! function can be exercised indirectly, so a mere mention (a helper call, a
//! selector table, even a comment) counts as referenced and suppresses the
//! flag. A flagged function is therefore a strong claim: no test file so much
//! as names it.
//!
//! Known floors (documented, not silent):
//! - Overloads collapse into one `(contract, function)` entry — a name grep
//!   cannot tell overloads apart, so per-overload verdicts would overstate
//!   precision. #156's three `format()` overloads still flag: no test named
//!   `format` at all.
//! - Functions a concrete contract INHERITS from an abstract base are declared
//!   at the base and enumerated there only if the base is itself concrete;
//!   cross-file inheritance resolution is a compiler's job. The report is a
//!   floor on the untested surface, never an exhaustive ceiling.
//! - `constructor`/`receive`/`fallback` are excluded: they are unnameable by a
//!   test (called via deployment / raw calls), so a name grep cannot judge them.
//! - Public state variables' auto-generated getters are excluded — only
//!   declared functions are enumerated.
//!
//! ## Failure semantics (the #52 rule: no error may become a coverage claim)
//! A file that does not parse contributes NOTHING silently: it is counted in
//! `sources_unparsed`, so a repo full of unparsable source reports an
//! unqualified-looking zero nowhere. Whole-repo unknowns (a failed clone) are
//! the caller's to represent — `analyze` is only ever called on files that
//! were actually read.

use solang_parser::pt::{
    ContractPart, ContractTy, FunctionAttribute, FunctionTy, SourceUnitPart, Visibility,
};

/// One external/public function on a concrete contract. Overloads share one
/// entry (see module docs).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalFn {
    /// Repo-relative path of the file declaring the contract.
    pub file: String,
    pub contract: String,
    pub function: String,
}

/// The per-repo result of the untested-external-surface check.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepoUntested {
    /// Distinct `(contract, function)` external/public entries enumerated.
    pub external_count: usize,
    /// Test source files whose content formed the reference corpus.
    pub test_files: usize,
    /// Source files that did not parse — their functions are UNKNOWN, not
    /// absent, and a nonzero count qualifies every other number here.
    pub sources_unparsed: usize,
    /// The finding: enumerated functions whose name no test source mentions.
    pub untested: Vec<ExternalFn>,
}

/// Whether `path` names a Solidity TEST file of the repo itself: `.t.sol`, or
/// any `test`/`tests` path segment — mirroring `protofire::is_test_path` — and
/// not vendored. These files form the reference corpus.
pub fn is_test_corpus(path: &str) -> bool {
    is_sol(path) && !is_vendored(path) && crate::protofire::is_test_path(path)
}

/// Whether `path` names first-party DEPLOYED-SURFACE Solidity the check should
/// enumerate contracts from. Excludes tests (not surface), Foundry scripts
/// (`.s.sol` / `script(s)/` — tooling that runs off-chain, whose `run()` would
/// be pure noise), and vendored dependencies (third-party surface is not this
/// repo's coverage gap).
pub fn is_enumerable_source(path: &str) -> bool {
    is_sol(path) && !is_vendored(path) && !crate::protofire::is_test_path(path) && !is_script(path)
}

fn is_sol(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".sol")
}

fn is_script(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".s.sol") {
        return true;
    }
    // Foundry's script dir lives at the repo root; judged top-level only, like
    // `is_vendored`, so a deeper first-party dir that happens to share the name
    // is not silently dropped. `.s.sol` still catches scripts anywhere.
    const SCRIPT_DIRS: [&str; 2] = ["script", "scripts"];
    p.split('/')
        .next()
        .is_some_and(|seg| SCRIPT_DIRS.contains(&seg))
}

/// Vendored/generated trees that must count neither as surface nor as tests,
/// nor as the repo's own imports (`graph::imported_prefixes` shares this
/// predicate):
/// `lib/` (git submodule deps — forge-std's OWN test dir would otherwise fake
/// coverage), `dependencies/` (soldeer), `node_modules/`, and build output.
///
/// Judged by the TOP-LEVEL path segment ONLY. Foundry vendors at the repo root,
/// while first-party code legitimately nests the same names deeper —
/// `src/lib/LibFoo.sol` and `test/src/lib/…` are the org's own layout, and an
/// any-depth `lib` match silently dropped rain.math.binary's entire
/// `test/src/lib/` corpus in the first live run of this check.
pub fn is_vendored(path: &str) -> bool {
    const VENDOR_DIRS: [&str; 5] = ["lib", "dependencies", "node_modules", "out", "cache"];
    path.to_ascii_lowercase()
        .split('/')
        .next()
        .is_some_and(|seg| VENDOR_DIRS.contains(&seg))
}

/// Enumerate the external/public functions of every CONCRETE contract in one
/// source file. `None` when the file does not parse — the caller must count it
/// unknown, never as declaring nothing (#52).
pub fn external_functions(file: &str, src: &str) -> Option<Vec<ExternalFn>> {
    let (unit, _comments) = solang_parser::parse(src, 0).ok()?;
    let mut out = Vec::new();
    for part in &unit.0 {
        let SourceUnitPart::ContractDefinition(def) = part else {
            continue;
        };
        // Concrete contracts only: an interface/library declares no deployed
        // surface, and an abstract contract's surface is judged where a
        // concrete descendant ships it.
        if !matches!(def.ty, ContractTy::Contract(_)) {
            continue;
        }
        let Some(contract) = def.name.as_ref().map(|n| n.name.clone()) else {
            continue;
        };
        for cpart in &def.parts {
            let ContractPart::FunctionDefinition(f) = cpart else {
                continue;
            };
            // Named functions only — constructor/receive/fallback/modifier are
            // not name-greppable surface (see module docs).
            if !matches!(f.ty, FunctionTy::Function) {
                continue;
            }
            let Some(name) = f.name.as_ref().map(|n| n.name.clone()) else {
                continue;
            };
            let visible = f.attributes.iter().any(|a| {
                matches!(
                    a,
                    FunctionAttribute::Visibility(Visibility::External(_))
                        | FunctionAttribute::Visibility(Visibility::Public(_))
                )
            });
            if visible {
                out.push(ExternalFn {
                    file: file.to_string(),
                    contract: contract.clone(),
                    function: name,
                });
            }
        }
    }
    // Overloads collapse to one entry per (file, contract, name).
    out.sort();
    out.dedup();
    Some(out)
}

/// Whether `name` occurs in `corpus` as a whole identifier — not as a fragment
/// of a longer one (`format` must not be satisfied by `formatted` or
/// `reformat`). This is the grep-the-test-dir restraint: ANY whole-identifier
/// mention, in code or comment, counts as a reference.
pub fn referenced(corpus: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let bytes = corpus.as_bytes();
    let mut from = 0;
    while let Some(pos) = corpus[from..].find(name) {
        let start = from + pos;
        let end = start + name.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1] as char);
        let after_ok = end == bytes.len() || !is_ident(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Analyze one repo's `.sol` files (repo-relative path, content): enumerate the
/// external/public surface of concrete contracts and flag every function whose
/// name no test source mentions. Files are routed by the predicates above, so
/// the caller's walk stays a dumb reader.
pub fn analyze(files: &[(String, String)]) -> RepoUntested {
    let mut fns: Vec<ExternalFn> = Vec::new();
    let mut sources_unparsed = 0usize;
    for (path, content) in files {
        if !is_enumerable_source(path) {
            continue;
        }
        match external_functions(path, content) {
            Some(mut v) => fns.append(&mut v),
            None => sources_unparsed += 1,
        }
    }
    fns.sort();
    fns.dedup();

    // One corpus string: membership is per-name over ALL tests, so per-file
    // structure adds nothing. Joined with a non-identifier separator so a name
    // can never straddle two files into a false boundary.
    let tests: Vec<&str> = files
        .iter()
        .filter(|(p, _)| is_test_corpus(p))
        .map(|(_, c)| c.as_str())
        .collect();
    let corpus = tests.join("\n");

    let external_count = fns.len();
    let untested: Vec<ExternalFn> = fns
        .into_iter()
        .filter(|f| !referenced(&corpus, &f.function))
        .collect();
    RepoUntested {
        external_count,
        test_files: tests.len(),
        sources_unparsed,
        untested,
    }
}

/// The findings-table signal for a repo: present iff the check RAN and flagged
/// at least one function. An unknown repo (`None` — e.g. its clone failed)
/// yields no signal: absence of the flag is never a coverage claim there,
/// because the JSON carries the repo as `unknown` alongside.
pub fn signal(report: Option<&RepoUntested>) -> Option<&'static str> {
    match report {
        Some(r) if !r.untested.is_empty() => Some("untested-externals"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(file: &str, contract: &str, function: &str) -> ExternalFn {
        ExternalFn {
            file: file.into(),
            contract: contract.into(),
            function: function.into(),
        }
    }

    // ---- enumeration: external_functions ----

    #[test]
    fn enumerates_external_and_public_but_not_internal_or_private() {
        let src = "contract C {\n\
            function a() external pure returns (uint256) { return 1; }\n\
            function b() public pure returns (uint256) { return 2; }\n\
            function c() internal pure returns (uint256) { return 3; }\n\
            function d() private pure returns (uint256) { return 4; }\n\
        }";
        let got = external_functions("src/C.sol", src).unwrap();
        assert_eq!(
            got,
            vec![f("src/C.sol", "C", "a"), f("src/C.sol", "C", "b")],
            "external + public are surface; internal + private are not"
        );
    }

    #[test]
    fn interfaces_libraries_and_abstract_contracts_are_not_surface() {
        let src = "interface I { function i() external; }\n\
            library L { function l() public pure returns (uint256) { return 1; } }\n\
            abstract contract A { function a() public virtual returns (uint256); }\n\
            contract C { function c() external pure returns (uint256) { return 1; } }";
        let got = external_functions("src/M.sol", src).unwrap();
        assert_eq!(
            got,
            vec![f("src/M.sol", "C", "c")],
            "only the concrete contract's function is deployed API"
        );
    }

    #[test]
    fn special_functions_are_excluded() {
        let src = "contract C {\n\
            constructor() {}\n\
            receive() external payable {}\n\
            fallback() external {}\n\
            modifier m() { _; }\n\
            function real() external {}\n\
        }";
        let got = external_functions("src/C.sol", src).unwrap();
        assert_eq!(got, vec![f("src/C.sol", "C", "real")]);
    }

    #[test]
    fn a_modifier_with_bogus_visibility_is_still_not_surface() {
        // solc rejects visibility on a modifier, but the scan reads arbitrary
        // repo contents and solang-parser accepts this permissively — so the
        // FunctionTy gate must exclude it even when a visibility attribute is
        // present. (Constructor/receive/fallback are already unnamed in the
        // AST; this is the one function-like item where the gate is load-bearing.)
        let got = external_functions(
            "src/P.sol",
            "contract C { modifier m() external { _; } function real() external {} }",
        )
        .unwrap();
        assert_eq!(got, vec![f("src/P.sol", "C", "real")]);
    }

    #[test]
    fn public_state_variable_getters_are_not_enumerated() {
        let src = "contract C { uint256 public counter; function bump() external { counter++; } }";
        let got = external_functions("src/C.sol", src).unwrap();
        assert_eq!(got, vec![f("src/C.sol", "C", "bump")]);
    }

    #[test]
    fn overloads_collapse_to_one_entry() {
        // The #156 shape: three external overloads of one name. A name grep
        // cannot tell them apart, so they are one enumerable claim, not three.
        let src = "contract DecimalFloat {\n\
            function format(int256 x) external pure returns (string memory) {}\n\
            function format(int256 x, uint8 p) external pure returns (string memory) {}\n\
            function format(bytes32 x) external pure returns (string memory) {}\n\
        }";
        let got = external_functions("src/D.sol", src).unwrap();
        assert_eq!(got, vec![f("src/D.sol", "DecimalFloat", "format")]);
    }

    #[test]
    fn a_contract_keyword_in_a_comment_or_string_is_not_a_declaration() {
        let src = "// contract Fake { function ghost() external {} }\n\
            contract Real {\n\
                string constant NOTE = \"contract Str { function ghost2() external {} }\";\n\
                function real() public {}\n\
            }";
        let got = external_functions("src/R.sol", src).unwrap();
        assert_eq!(
            got,
            vec![f("src/R.sol", "Real", "real")],
            "the AST, not a text scan, decides what a declaration is"
        );
    }

    #[test]
    fn unparsable_source_is_unknown_not_empty() {
        assert_eq!(
            external_functions("src/Broken.sol", "contract C { function ("),
            None,
            "a parse failure must never read as a file declaring nothing"
        );
    }

    // ---- reference grep: referenced ----

    #[test]
    fn referenced_matches_whole_identifiers_only() {
        assert!(referenced("x.format(1);", "format"));
        assert!(referenced("format(1)", "format"));
        assert!(referenced("selector == C.format.selector", "format"));
        assert!(
            !referenced("formatted(1); reformat(2); format_(3);", "format"),
            "fragments of longer identifiers are not references"
        );
        assert!(!referenced("", "format"));
        assert!(!referenced("form at", "format"));
    }

    #[test]
    fn a_comment_mention_counts_as_a_reference() {
        // Deliberate generosity: the flag is only for functions NO test so much
        // as names — indirect/indirectly-documented coverage suppresses it.
        assert!(referenced(
            "// exercised via format in the fuzz harness",
            "format"
        ));
    }

    #[test]
    fn a_later_whole_word_match_is_found_after_a_fragment() {
        // First occurrence is a fragment (reformat); the scan must keep going
        // and find the standalone one.
        assert!(referenced("reformat then format(", "format"));
    }

    // ---- file routing predicates ----

    #[test]
    fn enumerable_source_excludes_tests_scripts_and_vendored() {
        assert!(is_enumerable_source("src/DecimalFloat.sol"));
        assert!(is_enumerable_source("contracts/deep/Thing.sol"));
        assert!(!is_enumerable_source("test/DecimalFloat.t.sol"));
        assert!(!is_enumerable_source("src/Foo.t.sol"));
        assert!(!is_enumerable_source("script/Deploy.sol"));
        assert!(!is_enumerable_source("src/Deploy.s.sol"));
        assert!(!is_enumerable_source("lib/forge-std/src/Test.sol"));
        assert!(!is_enumerable_source("dependencies/rain.math/src/M.sol"));
        assert!(!is_enumerable_source("src/README.md"));
    }

    #[test]
    fn vendored_and_script_dirs_are_top_level_facts_not_any_segment() {
        // The org's own layout nests `lib` under `src/` and `test/` — an
        // any-depth segment match dropped rain.math.binary's whole
        // `test/src/lib/` corpus in the first live run. Foundry vendors at the
        // repo ROOT, so only the first segment may exclude.
        assert!(is_enumerable_source("src/lib/LibParse.sol"));
        assert!(is_enumerable_source("src/scripts/Runner.sol"));
        assert!(is_test_corpus("test/src/lib/LibCtPop.ctpop.t.sol"));
        assert!(is_test_corpus("test/lib/LibDataContract.t.sol"));
        // Root-level vendor/script dirs still excluded on both sides.
        assert!(!is_enumerable_source("lib/forge-std/src/Test.sol"));
        assert!(!is_test_corpus("lib/forge-std/test/StdAssertions.t.sol"));
        assert!(!is_enumerable_source("script/util/Helper.sol"));
    }

    #[test]
    fn test_corpus_is_own_tests_only_never_vendored_ones() {
        assert!(is_test_corpus("test/DecimalFloat.t.sol"));
        assert!(is_test_corpus("test/util/Helper.sol"));
        assert!(is_test_corpus("src/Foo.t.sol"));
        assert!(
            !is_test_corpus("lib/forge-std/test/StdAssertions.t.sol"),
            "a dependency's own tests must not fake first-party coverage"
        );
        assert!(!is_test_corpus("src/DecimalFloat.sol"));
    }

    // ---- whole-repo analysis: analyze ----

    fn repo(files: &[(&str, &str)]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn analyze_flags_only_functions_no_test_mentions() {
        let files = repo(&[
            (
                "src/C.sol",
                "contract C { function covered() external {} function orphan() external {} }",
            ),
            (
                "test/C.t.sol",
                "contract CTest { function testCovered() external { C(address(0)).covered(); } }",
            ),
        ]);
        let got = analyze(&files);
        assert_eq!(got.external_count, 2);
        assert_eq!(got.test_files, 1);
        assert_eq!(got.sources_unparsed, 0);
        assert_eq!(got.untested, vec![f("src/C.sol", "C", "orphan")]);
    }

    #[test]
    fn analyze_with_no_tests_flags_the_whole_surface() {
        let files = repo(&[("src/C.sol", "contract C { function a() external {} }")]);
        let got = analyze(&files);
        assert_eq!(got.test_files, 0);
        assert_eq!(got.untested, vec![f("src/C.sol", "C", "a")]);
    }

    #[test]
    fn analyze_counts_unparsable_sources_instead_of_dropping_them() {
        let files = repo(&[
            ("src/Broken.sol", "contract C { function ("),
            ("src/Ok.sol", "contract D { function d() external {} }"),
        ]);
        let got = analyze(&files);
        assert_eq!(
            got.sources_unparsed, 1,
            "the broken file is UNKNOWN, on the record"
        );
        assert_eq!(got.untested, vec![f("src/Ok.sol", "D", "d")]);
    }

    #[test]
    fn analyze_ignores_vendored_sources_and_vendored_tests() {
        let files = repo(&[
            ("src/C.sol", "contract C { function mine() external {} }"),
            // forge-std's surface is not ours; its tests are not our coverage.
            (
                "lib/forge-std/src/StdCheats.sol",
                "contract StdCheats { function skip() external {} }",
            ),
            (
                "lib/forge-std/test/StdCheats.t.sol",
                "contract T { function testMine() external { mine(); } }",
            ),
        ]);
        let got = analyze(&files);
        assert_eq!(got.external_count, 1, "only first-party surface enumerated");
        assert_eq!(
            got.untested,
            vec![f("src/C.sol", "C", "mine")],
            "a vendored test's mention must not count as first-party coverage"
        );
    }

    #[test]
    fn analyze_corpus_join_cannot_bridge_two_files_into_a_name() {
        // One test file's content ends with the LITERAL characters `for` and the
        // next begins with `mat(`: joined with no separator they read `format(`,
        // a false reference that would suppress the flag. The separator must
        // keep the two files apart. (First written with a `//` between the
        // halves — a fixture the mutant survived; this one kills it.)
        let files = repo(&[
            ("src/C.sol", "contract C { function format() external {} }"),
            (
                "test/A.t.sol",
                "contract A { uint256 xfor; } // trailing for",
            ),
            ("test/B.t.sol", "mat(); // fragment continues"),
        ]);
        let got = analyze(&files);
        assert_eq!(got.untested, vec![f("src/C.sol", "C", "format")]);
    }

    #[test]
    fn analyze_dedupes_the_same_name_across_overloads() {
        let files = repo(&[(
            "src/D.sol",
            "contract D { function format(int256 x) external {} function format(bytes32 x) external {} }",
        )]);
        let got = analyze(&files);
        assert_eq!(got.external_count, 1);
        assert_eq!(got.untested, vec![f("src/D.sol", "D", "format")]);
    }

    // ---- signal ----

    #[test]
    fn signal_fires_only_on_a_confirmed_nonempty_finding() {
        let clean = RepoUntested::default();
        assert_eq!(signal(Some(&clean)), None, "a clean repo has no finding");
        let dirty = RepoUntested {
            external_count: 1,
            untested: vec![f("src/C.sol", "C", "a")],
            ..Default::default()
        };
        assert_eq!(signal(Some(&dirty)), Some("untested-externals"));
        assert_eq!(
            signal(None),
            None,
            "unknown is not a finding — and not clean"
        );
    }
}
