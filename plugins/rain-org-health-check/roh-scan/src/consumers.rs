//! Reverse dependency lookup: given a soldeer package, who depends on it, at
//! what version, and who actually uses a symbol from it.
//!
//! The recurring question — "if I change `unsafeList` in rain.solmem, what
//! breaks?" — has two layers, and answering only one answers wrongly:
//!
//! 1. the MANIFEST layer: which repos declare the package as a dependency, and
//!    at which version. A consumer list is the blast radius of a republish.
//! 2. the SOURCE layer: which of those consumers name the symbol in their own
//!    Solidity. A repo that depends on the package but never touches the symbol
//!    is not in the blast radius of that symbol's change.
//!
//! ## Why every manifest shape, and not just one
//! Rain repos declare Solidity dependencies in at least five places, and no repo
//! uses all of them: `foundry.toml` `[dependencies]` (plus `remappings` inside a
//! `[profile.*]`), `soldeer.lock`, `foundry.lock`, `remappings.txt` and
//! `.gitmodules`. A sweep of ONE shape silently under-returns — grepping
//! `foundry.lock` misses `rainlanguage/rainlang`, which has no `foundry.lock` at
//! all, and reports a wrong answer with no error. So every shape is parsed and
//! the results are UNIONED, and a manifest that will not parse is an ERROR
//! carried to the report, never an empty dependency list.
//!
//! GitHub code search is deliberately not a source here: it indexes default
//! branches only and under-returns without saying so (it returned 7 of 16 known
//! consumers of `rain-solmem`, missing `raindex`).
//!
//! ## Names
//! The shapes disagree about spelling: soldeer writes `rain-solmem`, a git
//! submodule writes `lib/rain.solmem`, a remapping writes
//! `dependencies/rain-solmem-0.1.3/`. [`normalize_name`] folds all of them onto
//! one key so a query in any spelling finds all of them.
//!
//! Pure: the caller fetches, this parses and relates.

use crate::untested;
use std::collections::BTreeSet;

/// The manifest shapes a rain Solidity repo declares dependencies in.
///
/// Adding a shape is adding a variant plus its parser: every match here is
/// exhaustive, so a new shape cannot be silently skipped by the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestKind {
    /// `[dependencies]`, and `remappings` inside a `[profile.*]`. Also the only
    /// shape that says what the repo itself PUBLISHES (`[package].name`).
    FoundryToml,
    /// Soldeer's resolved lock: `[[dependencies]]` name/version tables.
    SoldeerLock,
    /// Foundry's lock: a JSON map of dependency path to `{rev|tag|branch}`.
    FoundryLock,
    /// `prefix/=dependencies/<name>-<version>/` (or `lib/<repo>/…`) lines.
    Remappings,
    /// Git submodule dependencies: `path` + `url` per `[submodule "…"]`.
    GitModules,
}

impl ManifestKind {
    /// The file name this shape is written in.
    pub fn file_name(self) -> &'static str {
        match self {
            ManifestKind::FoundryToml => "foundry.toml",
            ManifestKind::SoldeerLock => "soldeer.lock",
            ManifestKind::FoundryLock => "foundry.lock",
            ManifestKind::Remappings => "remappings.txt",
            ManifestKind::GitModules => ".gitmodules",
        }
    }

    /// Every shape, so the driver and the docs enumerate one list.
    pub const ALL: [ManifestKind; 5] = [
        ManifestKind::FoundryToml,
        ManifestKind::SoldeerLock,
        ManifestKind::FoundryLock,
        ManifestKind::Remappings,
        ManifestKind::GitModules,
    ];

    /// Which shape a repo-relative path is, by file name. `None` for anything
    /// that is not a dependency manifest.
    pub fn from_path(path: &str) -> Option<ManifestKind> {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        ManifestKind::ALL.into_iter().find(|k| k.file_name() == name)
    }
}

/// Whether a manifest path is a VENDORED copy rather than the repo's own
/// declaration.
///
/// `dependencies/rain-solmem-0.1.3/foundry.toml` is rain.solmem's manifest
/// sitting inside its consumer; reading it would invent a second publisher of
/// the package and a phantom dependency set.
///
/// The dir NAMES are [`untested::VENDOR_DIRS`], shared so the two checks cannot
/// drift. The DEPTH rule differs by design: `lib` is judged top-level only (for
/// the same reason `untested` does — `src/lib/` is the org's own layout), while
/// the unambiguous vendor roots are matched at any depth so a monorepo package's
/// own `dependencies/` tree is excluded too.
pub fn is_vendored_manifest(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let segs: Vec<&str> = lower.split('/').collect();
    if segs
        .first()
        .is_some_and(|s| untested::VENDOR_DIRS.contains(s))
    {
        return true;
    }
    // Skip the last segment: that is the file name, not a directory.
    segs.iter()
        .rev()
        .skip(1)
        .any(|s| *s != "lib" && untested::VENDOR_DIRS.contains(s))
}

/// Canonical key for comparing a dependency name across manifest shapes.
///
/// Case-folded, with every non-alphanumeric run collapsed to a single `-` and
/// trimmed. `rain.solmem`, `rain-solmem`, `lib/rain.solmem`'s basename and
/// `RAIN_SOLMEM` all land on `rain-solmem`; `@openzeppelin-contracts` lands on
/// `openzeppelin-contracts`, matching how soldeer, git and foundry each spell
/// the same package differently.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// One dependency declaration read out of one manifest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Declared {
    /// The name exactly as the manifest writes it — reported verbatim so a
    /// reader sees which spelling the repo actually uses.
    pub name: String,
    /// The pinned version, when the shape carries one. `None` is "this shape
    /// does not pin a version" (a git submodule, a bare remapping), NEVER "no
    /// version pinned anywhere".
    pub version: Option<String>,
    /// What the shape pins INSTEAD of a version: a git rev/tag, or a submodule
    /// url. Carried so a submodule-pinned consumer is still reportable.
    pub rev: Option<String>,
}

impl Declared {
    fn new(name: impl Into<String>) -> Declared {
        Declared {
            name: name.into(),
            version: None,
            rev: None,
        }
    }
}

/// What one manifest says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestFacts {
    /// The package this repo PUBLISHES, if the shape declares one (foundry.toml
    /// `[package].name` only).
    pub package: Option<String>,
    /// Every dependency the manifest declares.
    pub deps: Vec<Declared>,
}

/// A manifest that would not parse. Its dependencies are UNKNOWN — distinct
/// from having none, and never collapsed into one, because a broken manifest
/// reading as "declares nothing" is exactly how a consumer goes missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestError(pub String);

/// Parse one manifest of a known shape.
pub fn parse_manifest(kind: ManifestKind, content: &str) -> Result<ManifestFacts, ManifestError> {
    match kind {
        ManifestKind::FoundryToml => parse_foundry_toml(content),
        ManifestKind::SoldeerLock => parse_soldeer_lock(content),
        ManifestKind::FoundryLock => parse_foundry_lock(content),
        ManifestKind::Remappings => Ok(ManifestFacts {
            package: None,
            deps: content.lines().filter_map(parse_remapping).collect(),
        }),
        ManifestKind::GitModules => Ok(ManifestFacts {
            package: None,
            deps: parse_gitmodules(content),
        }),
    }
}

/// `foundry.toml`: `[package].name`, `[dependencies]`, and any `remappings`
/// array inside a `[profile.*]` (raindex pins `@openzeppelin` that way, so a
/// `[dependencies]`-only read misses a real declaration).
fn parse_foundry_toml(content: &str) -> Result<ManifestFacts, ManifestError> {
    // `[dependencies]` goes through the graph's parser rather than a second
    // copy of it: the dependency graph and this lookup must never disagree
    // about what a manifest declares.
    let mut deps: Vec<Declared> = crate::graph::foundry_dependencies(content)
        .map_err(|e| ManifestError(e.0))?
        .into_iter()
        .map(|d| Declared {
            name: d.package,
            version: (!d.version_req.is_empty()).then_some(d.version_req),
            rev: None,
        })
        .collect();
    let value = content
        .parse::<toml::Value>()
        .map_err(|e| ManifestError(e.to_string()))?;
    if let Some(profiles) = value.get("profile").and_then(|p| p.as_table()) {
        for profile in profiles.values() {
            let Some(arr) = profile.get("remappings").and_then(|r| r.as_array()) else {
                continue;
            };
            deps.extend(arr.iter().filter_map(|e| e.as_str()).filter_map(parse_remapping));
        }
    }
    Ok(ManifestFacts {
        package: crate::signals::foundry_package_name(content),
        deps,
    })
}

/// `soldeer.lock`: an array of `[[dependencies]]` tables carrying name/version.
///
/// The table form (`[dependencies]` with `name = "version"`) is accepted too.
/// Soldeer has written both over its life and this lookup must not lose a
/// consumer to a lockfile written by an older toolchain.
fn parse_soldeer_lock(content: &str) -> Result<ManifestFacts, ManifestError> {
    let value = content
        .parse::<toml::Value>()
        .map_err(|e| ManifestError(e.to_string()))?;
    // No `[[dependencies]]` is a real answer: the lock pins nothing.
    let Some(deps) = value.get("dependencies") else {
        return Ok(ManifestFacts::default());
    };
    let out = match deps {
        toml::Value::Array(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                let name = entry.get("name").and_then(|n| n.as_str()).ok_or_else(|| {
                    ManifestError("a [[dependencies]] entry has no `name`".into())
                })?;
                out.push(Declared {
                    name: name.to_string(),
                    version: entry
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    rev: entry.get("rev").and_then(|r| r.as_str()).map(str::to_string),
                });
            }
            out
        }
        toml::Value::Table(t) => t
            .iter()
            .map(|(name, val)| Declared {
                name: name.clone(),
                version: val.as_str().map(str::to_string).or_else(|| {
                    val.get("version")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                }),
                rev: val.get("rev").and_then(|r| r.as_str()).map(str::to_string),
            })
            .collect(),
        other => {
            return Err(ManifestError(format!(
                "`dependencies` is a {}, not an array or table",
                other.type_str()
            )))
        }
    };
    Ok(ManifestFacts {
        package: None,
        deps: out,
    })
}

/// `foundry.lock`: JSON mapping a dependency PATH to its pin. Both worlds show
/// up here — `"lib/rain.solmem": {"rev": …}` for a git submodule and
/// `"dependencies/rain-solmem-0.1.3": {…}` for a soldeer package — so the key is
/// read as a path and split into name + version.
fn parse_foundry_lock(content: &str) -> Result<ManifestFacts, ManifestError> {
    // An empty file is an empty lock, not a parse failure.
    if content.trim().is_empty() {
        return Ok(ManifestFacts::default());
    }
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|e| ManifestError(e.to_string()))?;
    let Some(obj) = value.as_object() else {
        return Err(ManifestError("foundry.lock is not a JSON object".into()));
    };
    Ok(ManifestFacts {
        package: None,
        deps: obj
            .iter()
            .map(|(path, pin)| {
                let (name, version) = dependency_path_name(path);
                Declared {
                    name,
                    version,
                    rev: match pin {
                        serde_json::Value::String(s) => Some(s.clone()),
                        _ => ["rev", "tag", "branch"]
                            .iter()
                            .find_map(|k| pin.get(k).and_then(|v| v.as_str()))
                            .map(str::to_string),
                    },
                }
            })
            .collect(),
    })
}

/// `.gitmodules`: one dependency per `[submodule "…"]`, identified by BOTH its
/// checkout path and its url's repo name. Both are emitted when they disagree —
/// a submodule checked out under a renamed directory would otherwise be missed
/// by whichever half the query happens to spell.
fn parse_gitmodules(content: &str) -> Vec<Declared> {
    let mut out: Vec<Declared> = Vec::new();
    let mut path: Option<String> = None;
    let mut url: Option<String> = None;
    let mut flush = |path: &mut Option<String>, url: &mut Option<String>, out: &mut Vec<Declared>| {
        let from_path = path.take().map(|p| basename(&p).to_string());
        let from_url = url.take().map(|u| {
            let u = u.trim_end_matches('/').trim_end_matches(".git");
            basename(u).to_string()
        });
        for name in [from_path.clone(), from_url] {
            let Some(name) = name.filter(|n| !n.is_empty()) else {
                continue;
            };
            if !out.iter().any(|d| normalize_name(&d.name) == normalize_name(&name)) {
                out.push(Declared::new(name));
            }
        }
    };
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            flush(&mut path, &mut url, &mut out);
            continue;
        }
        if let Some(v) = t.strip_prefix("path").and_then(strip_eq) {
            path = Some(v.to_string());
        } else if let Some(v) = t.strip_prefix("url").and_then(strip_eq) {
            url = Some(v.to_string());
        }
    }
    flush(&mut path, &mut url, &mut out);
    out
}

fn strip_eq(rest: &str) -> Option<&str> {
    let rest = rest.trim_start();
    let v = rest.strip_prefix('=')?.trim();
    (!v.is_empty()).then_some(v)
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// One remapping (`prefix/=target/`), from `remappings.txt` or a `[profile.*]`
/// `remappings` array. The TARGET is authoritative — it names the vendored
/// directory, which is where the version lives — and the prefix is the fallback
/// for a remapping that points somewhere else.
///
/// Comments (`#`) and blank lines yield nothing.
pub fn parse_remapping(line: &str) -> Option<Declared> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return None;
    }
    let (prefix, target) = line.split_once('=')?;
    // A context remapping is `context:prefix/=target/`; only the prefix matters.
    let prefix = prefix.rsplit(':').next().unwrap_or(prefix).trim();
    let target = target.trim().trim_end_matches('/');
    let segs: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();
    // `dependencies/<dir>/…` or `lib/<dir>/…` — the dir names the package.
    let dir = match segs.first() {
        Some(root) if untested::VENDOR_DIRS.contains(root) => segs.get(1).copied(),
        _ => None,
    }
    .unwrap_or_else(|| prefix.trim_end_matches('/'));
    if dir.is_empty() {
        return None;
    }
    let (name, version) = split_name_version(dir);
    Some(Declared {
        name,
        version,
        rev: None,
    })
}

/// A vendored dependency PATH (`dependencies/rain-solmem-0.1.3`,
/// `lib/rain.solmem`) → its package name and pinned version.
fn dependency_path_name(path: &str) -> (String, Option<String>) {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let dir = match segs.first() {
        Some(root) if untested::VENDOR_DIRS.contains(root) => segs.get(1).copied(),
        _ => None,
    }
    .or_else(|| segs.last().copied())
    .unwrap_or(path);
    split_name_version(dir)
}

/// Split a vendored directory name into `(package, version)`.
///
/// The version is the first `-`-delimited suffix that looks like one, so
/// `rain-solmem-0.1.3` splits at the version and `rain-datacontract` does not
/// split at all. A version must carry a dot (`0.1.3`, `1.0.0-rc1`): without
/// that, a package whose name contains a number — `erc-4626-words` — would be
/// truncated to `erc` and never match anything.
fn split_name_version(dir: &str) -> (String, Option<String>) {
    for (i, _) in dir.match_indices('-') {
        let suffix = &dir[i + 1..];
        if looks_like_version(suffix) {
            return (dir[..i].to_string(), Some(suffix.to_string()));
        }
    }
    (dir.to_string(), None)
}

/// Whether `s` is a version: a dotted numeric core (at least `major.minor`),
/// optionally followed by a pre-release/build suffix.
fn looks_like_version(s: &str) -> bool {
    let core = s.split(['-', '+']).next().unwrap_or("");
    let mut parts = core.split('.');
    let numeric = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
    parts.next().is_some_and(numeric) && parts.clone().count() >= 1 && parts.all(numeric)
}

/// A repo's relationship to the queried package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// This repo PUBLISHES the package (its `foundry.toml` `[package].name`).
    /// Reported separately: it is the thing being changed, not a consumer of
    /// it, and it necessarily contains every symbol the query asks about.
    Producer,
    /// This repo declares the package as a dependency.
    Consumer,
}

/// One declaration of the queried package, with the manifest it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Repo-relative manifest path.
    pub manifest: String,
    pub kind: ManifestKind,
    /// The name as that manifest spells it.
    pub declared_as: String,
    pub version: Option<String>,
    pub rev: Option<String>,
}

/// What one repo's manifests say about the queried package.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoMatch {
    /// `None` when the repo neither publishes nor consumes the package.
    pub role: Option<Role>,
    /// Every declaration found, one per (manifest, spelling).
    pub hits: Vec<Hit>,
    /// Distinct versions declared across the manifests. More than one is real
    /// drift: a `remappings.txt` left pointing at a version `foundry.toml` no
    /// longer pins.
    pub versions: Vec<String>,
    /// `(manifest path, reason)` for every manifest that would not parse. A
    /// repo with errors may be a consumer this lookup cannot see, so the driver
    /// reports the run as INCOMPLETE rather than answering as if it were clean.
    pub errors: Vec<(String, String)>,
}

/// Match one repo's parsed manifests against `query`.
///
/// `manifests` is `(repo-relative path, shape, parse result)` for every
/// non-vendored manifest the repo has.
pub fn match_repo(
    query: &str,
    manifests: &[(String, ManifestKind, Result<ManifestFacts, ManifestError>)],
) -> RepoMatch {
    let want = normalize_name(query);
    let mut out = RepoMatch::default();
    let mut versions: BTreeSet<String> = BTreeSet::new();
    let mut publishes = false;
    for (path, kind, parsed) in manifests {
        let facts = match parsed {
            Ok(f) => f,
            Err(e) => {
                out.errors.push((path.clone(), e.0.clone()));
                continue;
            }
        };
        if facts
            .package
            .as_deref()
            .is_some_and(|p| normalize_name(p) == want)
        {
            publishes = true;
        }
        for dep in &facts.deps {
            if normalize_name(&dep.name) != want {
                continue;
            }
            if let Some(v) = &dep.version {
                versions.insert(v.clone());
            }
            out.hits.push(Hit {
                manifest: path.clone(),
                kind: *kind,
                declared_as: dep.name.clone(),
                version: dep.version.clone(),
                rev: dep.rev.clone(),
            });
        }
    }
    out.hits.sort_by(|a, b| (a.kind, &a.manifest).cmp(&(b.kind, &b.manifest)));
    out.versions = versions.into_iter().collect();
    // A repo that publishes the package is its PRODUCER even if it also names
    // itself in a self-remapping — it is not a consumer of itself.
    out.role = if publishes {
        Some(Role::Producer)
    } else if !out.hits.is_empty() {
        Some(Role::Consumer)
    } else {
        None
    };
    out
}

/// Where a symbol is referenced inside one repo's checkout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SymbolUsage {
    pub symbol: String,
    /// Whole-identifier occurrences in the repo's OWN Solidity.
    pub own_refs: usize,
    /// The repo's own `.sol` files containing it, sorted.
    pub own_files: Vec<String>,
    /// How many of `own_files` are test sources. Equal to `own_files.len()`
    /// means the symbol is only ever named by tests — a weaker claim on the
    /// production surface than a src reference, and worth seeing.
    pub own_test_files: usize,
    /// Vendored `.sol` files containing it, EXCLUDED from the counts above. A
    /// vendored copy of the library holds the symbol's own definition, so
    /// counting it would make every consumer look like a caller.
    pub vendored_files: usize,
}

/// Count references to `symbol` across one checkout's `.sol` files, given as
/// `(repo-relative path, content)`.
///
/// Matching is whole-identifier (`unsafeList` is not satisfied by
/// `unsafeListOf`), and ANY mention counts — a call, an import, a comment. The
/// question is "does this repo touch the symbol at all", and a mention is
/// evidence it does; the restraint is the same one `untested` applies.
pub fn symbol_usage(files: &[(String, String)], symbol: &str) -> SymbolUsage {
    let mut out = SymbolUsage {
        symbol: symbol.to_string(),
        ..Default::default()
    };
    for (path, content) in files {
        if !path.to_ascii_lowercase().ends_with(".sol") {
            continue;
        }
        let n = untested::identifier_occurrences(content, symbol);
        if n == 0 {
            continue;
        }
        if untested::is_vendored(path) {
            out.vendored_files += 1;
            continue;
        }
        out.own_refs += n;
        if crate::protofire::is_test_path(path) {
            out.own_test_files += 1;
        }
        out.own_files.push(path.clone());
    }
    out.own_files.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(content: &str, kind: ManifestKind) -> ManifestFacts {
        parse_manifest(kind, content).expect("manifest parses")
    }

    fn dep_names(f: &ManifestFacts) -> Vec<&str> {
        f.deps.iter().map(|d| d.name.as_str()).collect()
    }

    fn found(f: &ManifestFacts, name: &str) -> Option<Declared> {
        let want = normalize_name(name);
        f.deps.iter().find(|d| normalize_name(&d.name) == want).cloned()
    }

    // ---- name normalization: the shapes spell the same package differently ----

    #[test]
    fn every_spelling_of_one_package_normalizes_together() {
        let key = normalize_name("rain-solmem");
        for spelling in [
            "rain.solmem",
            "rain-solmem",
            "RAIN_SOLMEM",
            "  rain.solmem  ",
            "rain--solmem",
        ] {
            assert_eq!(normalize_name(spelling), key, "{spelling} did not fold");
        }
        // …and distinct packages must NOT collide.
        assert_ne!(normalize_name("rain.string"), normalize_name("rain.intorastring"));
        assert_ne!(normalize_name("rain.math.float"), normalize_name("rain.math.fixedpoint"));
        // a leading @ is a scope marker, not part of the identity
        assert_eq!(
            normalize_name("@openzeppelin-contracts"),
            "openzeppelin-contracts"
        );
    }

    // ---- foundry.toml ----

    /// Verbatim from rainlanguage/raindex: quoted and unquoted keys, a comment
    /// inside the section, and a `remappings` array in `[profile.default]` that
    /// declares a dependency `[dependencies]` alone does not carry.
    #[test]
    fn foundry_toml_reads_dependencies_package_name_and_profile_remappings() {
        let toml = r#"
[package]
name = "raindex"
version = "0.1.13"

[profile.default]
src = 'src'
remappings = [
  "@openzeppelin/contracts/=dependencies/@openzeppelin-contracts-5.6.1/",
]

[dependencies]
forge-std = "1.16.1"
# renamed upstream; the registry name follows
rainlang = "0.1.5"
"rain-solmem" = "0.1.3"
inline-form = { version = "0.1.0", url = "https://example.invalid/x.zip" }
"#;
        let f = facts(toml, ManifestKind::FoundryToml);
        assert_eq!(f.package.as_deref(), Some("raindex"));
        assert_eq!(
            found(&f, "rain-solmem").and_then(|d| d.version),
            Some("0.1.3".into())
        );
        assert_eq!(
            found(&f, "inline-form").and_then(|d| d.version),
            Some("0.1.0".into()),
            "the inline-table form still yields its version"
        );
        assert_eq!(
            found(&f, "@openzeppelin-contracts").and_then(|d| d.version),
            Some("5.6.1".into()),
            "a [profile.*] remapping is a declaration too — reading [dependencies] alone misses it"
        );
    }

    /// A malformed manifest is UNKNOWN, never "declares nothing". Collapsing the
    /// two is how a consumer disappears from the answer without an error.
    #[test]
    fn a_manifest_that_will_not_parse_is_an_error_not_an_empty_list() {
        assert!(parse_manifest(ManifestKind::FoundryToml, "not toml {{{").is_err());
        assert!(parse_manifest(ManifestKind::SoldeerLock, "not toml {{{").is_err());
        assert!(parse_manifest(ManifestKind::FoundryLock, "not json [[[").is_err());
        // …while a genuinely empty declaration is a real, readable answer.
        assert_eq!(
            parse_manifest(ManifestKind::FoundryToml, "[profile.default]\nsrc = 'src'\n"),
            Ok(ManifestFacts::default())
        );
        assert_eq!(
            parse_manifest(ManifestKind::FoundryLock, "  "),
            Ok(ManifestFacts::default())
        );
    }

    // ---- soldeer.lock ----

    /// Verbatim from rainlanguage/rainlang — the repo a foundry.lock-only sweep
    /// misses entirely, because it has no foundry.lock at all.
    #[test]
    fn soldeer_lock_reads_the_array_of_dependency_tables() {
        let lock = r#"
[[dependencies]]
name = "forge-std"
version = "1.16.1"
url = "https://soldeer-revisions.s3.amazonaws.com/forge-std/1_16_1.zip"
checksum = "839b"

[[dependencies]]
name = "rain-solmem"
version = "0.1.3"
url = "https://soldeer-revisions.s3.amazonaws.com/rain-solmem/0_1_3.zip"
integrity = "aa"
"#;
        let f = facts(lock, ManifestKind::SoldeerLock);
        assert_eq!(dep_names(&f), vec!["forge-std", "rain-solmem"]);
        assert_eq!(
            found(&f, "rain-solmem").and_then(|d| d.version),
            Some("0.1.3".into())
        );
    }

    /// A lock entry with no name is a shape this parser does not understand.
    /// Skipping it silently would drop a dependency; it is an error.
    #[test]
    fn a_nameless_soldeer_lock_entry_is_an_error() {
        assert!(parse_manifest(
            ManifestKind::SoldeerLock,
            "[[dependencies]]\nversion = \"0.1.3\"\n"
        )
        .is_err());
    }

    /// Older soldeer wrote a table instead of an array. Losing those consumers
    /// to a toolchain-version difference is the same silent under-return.
    #[test]
    fn soldeer_lock_also_reads_the_older_table_form() {
        let f = facts(
            "[dependencies]\n\"rain-solmem\" = \"0.1.3\"\n",
            ManifestKind::SoldeerLock,
        );
        assert_eq!(
            found(&f, "rain-solmem").and_then(|d| d.version),
            Some("0.1.3".into())
        );
    }

    // ---- foundry.lock ----

    /// Verbatim from rainlanguage/rain.math.float: submodule pins keyed by path.
    /// The key names the REPO (`rain.solmem`), not the soldeer package
    /// (`rain-solmem`) — which is why names are normalized before comparison.
    #[test]
    fn foundry_lock_reads_submodule_paths_and_revs() {
        let lock = r#"{
  "lib/forge-std": { "rev": "1801b054" },
  "lib/rain.solmem": { "rev": "3bff3078" }
}"#;
        let f = facts(lock, ManifestKind::FoundryLock);
        let d = found(&f, "rain-solmem").expect("lib/rain.solmem is rain-solmem");
        assert_eq!(d.name, "rain.solmem", "the name is reported as written");
        assert_eq!(d.rev.as_deref(), Some("3bff3078"));
        assert_eq!(d.version, None, "a git rev is not a version");
    }

    /// Foundry also locks soldeer packages here, keyed by the vendored dir —
    /// where the version lives in the directory NAME.
    #[test]
    fn foundry_lock_reads_soldeer_paths_with_their_versions() {
        let f = facts(
            r#"{"dependencies/rain-solmem-0.1.3": {"tag": "v0.1.3"}}"#,
            ManifestKind::FoundryLock,
        );
        let d = found(&f, "rain-solmem").unwrap();
        assert_eq!(d.version.as_deref(), Some("0.1.3"));
        assert_eq!(d.rev.as_deref(), Some("v0.1.3"));
    }

    // ---- remappings ----

    /// Verbatim from rainlanguage/raindex's remappings.txt, plus the submodule
    /// form. Two versions of one package (a leftover line) both survive: that
    /// disagreement is the finding, not something to collapse.
    #[test]
    fn remappings_read_the_vendored_dir_for_name_and_version() {
        let content = "\
@openzeppelin-contracts-5.6.1/=dependencies/@openzeppelin-contracts-5.6.1/
rain-solmem-0.1.3/=dependencies/rain-solmem-0.1.3/
rain-string-0.1.0/=dependencies/rain-string-0.1.0/
rain-string-0.2.0/=dependencies/rain-string-0.2.0/
rain.solmem/=lib/rain.solmem/src/

# a comment
";
        let f = facts(content, ManifestKind::Remappings);
        let solmem: Vec<Option<String>> = f
            .deps
            .iter()
            .filter(|d| normalize_name(&d.name) == "rain-solmem")
            .map(|d| d.version.clone())
            .collect();
        assert_eq!(
            solmem,
            vec![Some("0.1.3".to_string()), None],
            "both the soldeer and submodule remappings name rain-solmem"
        );
        let strings: Vec<Option<String>> = f
            .deps
            .iter()
            .filter(|d| normalize_name(&d.name) == "rain-string")
            .map(|d| d.version.clone())
            .collect();
        assert_eq!(
            strings,
            vec![Some("0.1.0".into()), Some("0.2.0".into())],
            "a stale leftover remapping is kept, not collapsed"
        );
        assert!(
            !f.deps.iter().any(|d| d.name.starts_with('#')),
            "comments declare nothing"
        );
    }

    /// A package whose name ends in a number must not be truncated at it.
    #[test]
    fn a_numeric_name_segment_is_not_mistaken_for_a_version() {
        assert_eq!(
            split_name_version("erc-4626-words"),
            ("erc-4626-words".to_string(), None)
        );
        assert_eq!(
            split_name_version("rain-solmem-0.1.3"),
            ("rain-solmem".to_string(), Some("0.1.3".to_string()))
        );
        assert_eq!(
            split_name_version("rain-x-1.0.0-rc1"),
            ("rain-x".to_string(), Some("1.0.0-rc1".to_string())),
            "a pre-release suffix travels with the version"
        );
        assert_eq!(
            split_name_version("rain.solmem"),
            ("rain.solmem".to_string(), None)
        );
    }

    // ---- .gitmodules ----

    /// Verbatim from h20liquidity/h20.test-std.
    #[test]
    fn gitmodules_names_the_dependency_by_path_and_url() {
        let content = "\
[submodule \"lib/rain.solmem\"]
\tpath = lib/rain.solmem
\turl = https://github.com/rainlanguage/rain.solmem.git
[submodule \"lib/sushi\"]
\tpath = lib/sushi
\turl = https://github.com/rainlanguage/sushiswap.git
";
        let f = facts(content, ManifestKind::GitModules);
        assert!(
            found(&f, "rain-solmem").is_some(),
            "a submodule dependency is a dependency"
        );
        // path and url disagree for the second: BOTH names are kept, so a query
        // in either spelling finds it.
        assert!(found(&f, "sushi").is_some() && found(&f, "sushiswap").is_some());
    }

    // ---- vendored manifests ----

    /// A vendored copy of the library carries the library's OWN manifest.
    /// Reading it would invent a second publisher and a phantom dependency set.
    #[test]
    fn vendored_manifests_are_excluded_but_first_party_ones_are_not() {
        for vendored in [
            "dependencies/rain-solmem-0.1.3/foundry.toml",
            "lib/forge-std/foundry.toml",
            "node_modules/x/remappings.txt",
            "packages/app/dependencies/rain-solmem-0.1.3/soldeer.lock",
            "out/foundry.lock",
        ] {
            assert!(is_vendored_manifest(vendored), "{vendored} read as first-party");
        }
        for own in [
            "foundry.toml",
            "soldeer.lock",
            "contracts/foundry.toml",
            "packages/core/soldeer.lock",
            // `src/lib/` is the org's own layout — `lib` is vendored only at the
            // repo root, exactly as untested.rs judges it.
            "src/lib/foundry.toml",
        ] {
            assert!(!is_vendored_manifest(own), "{own} read as vendored");
        }
    }

    #[test]
    fn manifest_kinds_are_recognized_by_file_name_at_any_depth() {
        assert_eq!(
            ManifestKind::from_path("foundry.toml"),
            Some(ManifestKind::FoundryToml)
        );
        assert_eq!(
            ManifestKind::from_path("contracts/soldeer.lock"),
            Some(ManifestKind::SoldeerLock)
        );
        assert_eq!(
            ManifestKind::from_path("a/b/.gitmodules"),
            Some(ManifestKind::GitModules)
        );
        assert_eq!(ManifestKind::from_path("Cargo.toml"), None);
        assert_eq!(ManifestKind::from_path("README.md"), None);
        // every shape must be reachable from a path, or the driver never fetches it
        for kind in ManifestKind::ALL {
            assert_eq!(ManifestKind::from_path(kind.file_name()), Some(kind));
        }
    }

    // ---- matching a repo against the query ----

    fn m(
        path: &str,
        kind: ManifestKind,
        content: &str,
    ) -> (String, ManifestKind, Result<ManifestFacts, ManifestError>) {
        (path.to_string(), kind, parse_manifest(kind, content))
    }

    /// The whole point: the same consumer found through whichever shape it
    /// happens to use, and the union across shapes.
    #[test]
    fn a_consumer_is_found_through_any_single_manifest_shape() {
        let cases = [
            m(
                "foundry.toml",
                ManifestKind::FoundryToml,
                "[dependencies]\n\"rain-solmem\" = \"0.1.3\"\n",
            ),
            m(
                "soldeer.lock",
                ManifestKind::SoldeerLock,
                "[[dependencies]]\nname = \"rain-solmem\"\nversion = \"0.1.3\"\n",
            ),
            m(
                "foundry.lock",
                ManifestKind::FoundryLock,
                r#"{"lib/rain.solmem": {"rev": "abc"}}"#,
            ),
            m(
                "remappings.txt",
                ManifestKind::Remappings,
                "rain-solmem-0.1.3/=dependencies/rain-solmem-0.1.3/\n",
            ),
            m(
                ".gitmodules",
                ManifestKind::GitModules,
                "[submodule \"lib/rain.solmem\"]\npath = lib/rain.solmem\n",
            ),
        ];
        for case in cases {
            let kind = case.1;
            let got = match_repo("rain-solmem", std::slice::from_ref(&case));
            assert_eq!(
                got.role,
                Some(Role::Consumer),
                "{} alone did not identify the consumer",
                kind.file_name()
            );
        }
    }

    /// The exact failure that motivated this mode: a repo whose ONLY lockfile is
    /// soldeer.lock. A foundry.lock-shaped sweep sees no foundry.lock, reads
    /// that as "no dependencies", and drops a real consumer with no error.
    #[test]
    fn a_repo_with_no_foundry_lock_is_still_found() {
        let manifests = [
            m(
                "foundry.toml",
                ManifestKind::FoundryToml,
                "[package]\nname = \"rainlang\"\n\n[dependencies]\n\"rain-solmem\" = \"0.1.3\"\n",
            ),
            m(
                "soldeer.lock",
                ManifestKind::SoldeerLock,
                "[[dependencies]]\nname = \"rain-solmem\"\nversion = \"0.1.3\"\n",
            ),
        ];
        let got = match_repo("rain-solmem", &manifests);
        assert_eq!(got.role, Some(Role::Consumer));
        assert_eq!(got.versions, vec!["0.1.3".to_string()]);
        assert_eq!(got.hits.len(), 2, "both shapes are reported: {:?}", got.hits);
    }

    /// A query in either spelling must find the same consumer — the manifests
    /// disagree about spelling, so the query cannot be required to guess.
    #[test]
    fn the_query_may_be_spelled_as_the_package_or_as_the_repo() {
        let manifests = [m(
            "foundry.lock",
            ManifestKind::FoundryLock,
            r#"{"lib/rain.solmem": {"rev": "abc"}}"#,
        )];
        for query in ["rain-solmem", "rain.solmem"] {
            assert_eq!(
                match_repo(query, &manifests).role,
                Some(Role::Consumer),
                "query {query} missed the consumer"
            );
        }
    }

    /// The publisher is not a consumer of itself, even when it self-remaps.
    #[test]
    fn the_publishing_repo_is_the_producer_not_a_consumer() {
        let manifests = [
            m(
                "foundry.toml",
                ManifestKind::FoundryToml,
                "[package]\nname = \"rain-solmem\"\nversion = \"0.1.4\"\n",
            ),
            m(
                "remappings.txt",
                ManifestKind::Remappings,
                "rain-solmem/=src/\n",
            ),
        ];
        assert_eq!(
            match_repo("rain-solmem", &manifests).role,
            Some(Role::Producer)
        );
    }

    #[test]
    fn an_unrelated_repo_matches_nothing() {
        let manifests = [m(
            "foundry.toml",
            ManifestKind::FoundryToml,
            "[package]\nname = \"rain-string\"\n[dependencies]\nforge-std = \"1.16.1\"\n",
        )];
        let got = match_repo("rain-solmem", &manifests);
        assert_eq!(got.role, None);
        assert!(got.hits.is_empty() && got.errors.is_empty());
    }

    /// A manifest that will not parse leaves this repo's answer INCOMPLETE. The
    /// error is carried out so the driver can say so rather than reporting the
    /// repo as a clean non-consumer.
    #[test]
    fn an_unparseable_manifest_is_reported_not_swallowed() {
        let manifests = [
            m("foundry.toml", ManifestKind::FoundryToml, "not toml {{{"),
            m(
                "soldeer.lock",
                ManifestKind::SoldeerLock,
                "[[dependencies]]\nname = \"forge-std\"\nversion = \"1\"\n",
            ),
        ];
        let got = match_repo("rain-solmem", &manifests);
        assert_eq!(got.role, None, "no evidence of a dependency was found…");
        assert_eq!(got.errors.len(), 1, "…but the repo was not fully readable");
        assert_eq!(got.errors[0].0, "foundry.toml");
    }

    /// Manifests disagreeing about the version is real drift and must survive
    /// into the report — collapsing to one version hides a stale remapping.
    #[test]
    fn disagreeing_versions_are_all_reported() {
        let manifests = [
            m(
                "foundry.toml",
                ManifestKind::FoundryToml,
                "[dependencies]\n\"rain-solmem\" = \"0.1.3\"\n",
            ),
            m(
                "remappings.txt",
                ManifestKind::Remappings,
                "rain-solmem-0.1.2/=dependencies/rain-solmem-0.1.2/\n",
            ),
        ];
        assert_eq!(
            match_repo("rain-solmem", &manifests).versions,
            vec!["0.1.2".to_string(), "0.1.3".to_string()]
        );
    }

    // ---- the source layer ----

    fn sol(path: &str, src: &str) -> (String, String) {
        (path.to_string(), src.to_string())
    }

    /// A vendored copy of the library holds the symbol's DEFINITION. Counting it
    /// as the repo's own use would make every consumer look like a caller —
    /// which is the whole reason the source layer exists.
    #[test]
    fn a_vendored_copy_of_the_definition_is_never_the_repos_own_use() {
        let files = vec![
            sol(
                "dependencies/rain-solmem-0.1.3/src/lib/LibUint256Array.sol",
                "function unsafeList(uint256 a) internal pure {}",
            ),
            sol("src/Consumer.sol", "// nothing here"),
        ];
        let u = symbol_usage(&files, "unsafeList");
        assert_eq!(u.own_refs, 0, "the vendored definition is not a call");
        assert!(u.own_files.is_empty());
        assert_eq!(u.vendored_files, 1, "…but it is reported, not hidden");
    }

    #[test]
    fn own_sources_and_tests_are_counted_separately() {
        let files = vec![
            sol("src/A.sol", "a.unsafeList(1); a.unsafeList(2);"),
            sol("test/A.t.sol", "a.unsafeList(3);"),
            sol("lib/forge-std/src/Vm.sol", "unsafeList"),
            sol("README.md", "unsafeList"),
        ];
        let u = symbol_usage(&files, "unsafeList");
        assert_eq!(u.own_refs, 3, "every occurrence in the repo's own .sol");
        assert_eq!(
            u.own_files,
            vec!["src/A.sol".to_string(), "test/A.t.sol".to_string()]
        );
        assert_eq!(u.own_test_files, 1);
        assert_eq!(u.vendored_files, 1);
    }

    /// Whole-identifier matching: a longer name that merely contains the symbol
    /// is not a reference to it.
    #[test]
    fn a_longer_identifier_is_not_a_reference() {
        let files = vec![sol("src/A.sol", "unsafeListOf(); myUnsafeList(); x.unsafeList();")];
        assert_eq!(symbol_usage(&files, "unsafeList").own_refs, 1);
    }

    #[test]
    fn a_symbol_nothing_references_reports_zero_not_absence_of_data() {
        let files = vec![sol("src/A.sol", "contract A {}")];
        let u = symbol_usage(&files, "unsafeList");
        assert_eq!(u.own_refs, 0);
        assert_eq!(u.symbol, "unsafeList");
        assert!(u.own_files.is_empty() && u.vendored_files == 0);
    }
}
