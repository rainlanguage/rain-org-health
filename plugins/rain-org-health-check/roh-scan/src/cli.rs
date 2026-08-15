//! The command line: a PURE parse of `argv` into a typed command, plus the
//! `--help` text.
//!
//! Pure and typed deliberately. The parser this replaces folded every argument
//! it did not recognise into the repo list, so `roh-scan --help` scanned a repo
//! literally named `--help`, found nothing, and exited 0 — "no findings, 0/1
//! repos", a clean bill of health for a repo that does not exist. A typo'd repo
//! name did the same. Both are the same defect: an unrecognised input became a
//! successful empty answer instead of an error.
//!
//! So: every argument is either recognised or an error, and the whole surface is
//! unit-testable without argv, env, network or a process exit.

/// The orgs a "rain org" query spans by default. The registry
/// (`issue-pr-cron/ORG-REGISTRY.md`) lists these as the in-scope orgs; the
/// dormant/out-of-scope ones alongside them are deliberately absent.
///
/// Sweeping only `rainlanguage` is what a reverse-dependency query must NOT do:
/// `S01-Issuer/st0x.deploy` consumes org packages, so a single-org sweep answers
/// the question wrongly and says nothing about having done so.
pub const RAIN_ORGS: &[&str] = &[
    "rainlanguage",
    "cyclofinance",
    "S01-Issuer",
    "ST0x-Technology",
    "h20liquidity",
    "gildlab",
    "raincommercial",
    "rain-archive",
];

/// A resolved command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `--help`/`-h`: print usage and exit 0.
    Help,
    /// The org modernization-debt scan (the default mode).
    Scan {
        /// `--json <path>`, overriding `JSON_OUT` and the `site/health.json` default.
        json: Option<String>,
        /// Repo names to scan instead of the whole org listing. Empty means "the
        /// whole org".
        repos: Vec<String>,
    },
    /// The reverse-dependency lookup: who consumes a package, at what version,
    /// and which of them reference each `--symbol`.
    Consumers(ConsumersArgs),
}

/// `roh-scan consumers <package> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumersArgs {
    /// The soldeer package — or the repo name; the two are matched modulo
    /// spelling — whose consumers are wanted.
    pub package: String,
    /// Solidity identifiers to look for in each consumer's OWN sources. Empty
    /// means the manifest layer only, and nothing is cloned.
    pub symbols: Vec<String>,
    /// `--orgs`, overriding both the `ORGS`/`ORG` env vars and [`RAIN_ORGS`].
    pub orgs: Option<Vec<String>>,
    /// `--json <path>`. Opt-in here: unlike the scan this mode has no default
    /// output file, because writing `site/health.json` would clobber the
    /// dashboard's data with an unrelated document.
    pub json: Option<String>,
}

/// Why a command line was rejected. Typed rather than a formatted string so the
/// tests pin the DECISION, not the wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// An argument starting with `-` that no mode defines.
    UnknownFlag { command: &'static str, flag: String },
    /// A flag whose value is missing (`--json` at the end of the line).
    MissingValue(String),
    /// A flag given an empty value, which would otherwise be carried silently.
    EmptyValue(String),
    /// `consumers` with no package argument.
    MissingPackage,
    /// A positional argument the mode has no slot for.
    UnexpectedArgument { command: &'static str, arg: String },
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag { command, flag } => {
                write!(f, "unknown flag `{flag}` for `{command}`")
            }
            CliError::MissingValue(flag) => write!(f, "`{flag}` needs a value"),
            CliError::EmptyValue(flag) => write!(f, "`{flag}` was given an empty value"),
            CliError::MissingPackage => {
                write!(f, "`consumers` needs a package name, e.g. `rain-solmem`")
            }
            CliError::UnexpectedArgument { command, arg } => {
                write!(f, "unexpected argument `{arg}` for `{command}`")
            }
        }
    }
}

/// Parse `args` (argv WITHOUT the program name) into a command.
///
/// `--help` anywhere on the line wins, so help is reachable even when the rest
/// of the line is wrong — that is the one case where an unrecognised argument
/// must not shadow the answer the caller wants.
pub fn parse_args(args: &[String]) -> Result<Command, CliError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Command::Help);
    }
    match args.first().map(String::as_str) {
        Some("consumers") => parse_consumers(&args[1..]),
        // `scan` may be named explicitly; omitting it keeps the historical
        // `roh-scan [repo …]` form working.
        Some("scan") => parse_scan(&args[1..]),
        _ => parse_scan(args),
    }
}

fn parse_consumers(args: &[String]) -> Result<Command, CliError> {
    let mut package: Option<String> = None;
    let mut symbols: Vec<String> = Vec::new();
    let mut orgs: Option<Vec<String>> = None;
    let mut json: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--symbol" => {
                symbols.push(value_of("--symbol", args.get(i + 1))?);
                i += 2;
            }
            "--orgs" => {
                let raw = value_of("--orgs", args.get(i + 1))?;
                orgs = Some(
                    raw.split([' ', ',', '\t', '\n'])
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
                i += 2;
            }
            "--json" => {
                json = Some(value_of("--json", args.get(i + 1))?);
                i += 2;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::UnknownFlag {
                    command: "consumers",
                    flag: arg.to_string(),
                })
            }
            _ if package.is_none() => {
                package = Some(arg.to_string());
                i += 1;
            }
            // A second positional is a forgotten `--symbol`, not a second
            // package. Ignoring it would answer a question nobody asked.
            _ => {
                return Err(CliError::UnexpectedArgument {
                    command: "consumers",
                    arg: arg.to_string(),
                })
            }
        }
    }
    Ok(Command::Consumers(ConsumersArgs {
        package: package.ok_or(CliError::MissingPackage)?,
        symbols,
        orgs,
        json,
    }))
}

fn parse_scan(args: &[String]) -> Result<Command, CliError> {
    let mut json = None;
    let mut repos = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--json" => {
                json = Some(value_of("--json", args.get(i + 1))?);
                i += 2;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::UnknownFlag {
                    command: "scan",
                    flag: arg.to_string(),
                })
            }
            _ => {
                repos.push(arg.to_string());
                i += 1;
            }
        }
    }
    Ok(Command::Scan { json, repos })
}

/// A flag's value: present and non-empty, or a typed error.
fn value_of(flag: &str, next: Option<&String>) -> Result<String, CliError> {
    match next {
        None => Err(CliError::MissingValue(flag.to_string())),
        Some(v) if v.trim().is_empty() => Err(CliError::EmptyValue(flag.to_string())),
        Some(v) => Ok(v.clone()),
    }
}

/// The `--help` text — this tool's reference material, which is why it is
/// exhaustive about modes, flags and defaults rather than a reminder.
///
/// Built at runtime so the default org list is the ONE [`RAIN_ORGS`] constant
/// rather than a prose copy of it that drifts away from the code.
pub fn usage() -> String {
    format!(
        "roh-scan — rain org health scanner.

USAGE
  roh-scan [scan] [--json <path>] [<repo> ...]
  roh-scan consumers <package> [--symbol <name>]... [--orgs \"<org> ...\"] [--json <path>]
  roh-scan --help

MODES

  scan (the default; `scan` may be written explicitly)
      Sweep every repo in the org for rainix/soldeer modernization debt and
      write the dashboard document. With no <repo> the whole org listing is
      scanned; naming repos scans only those. A named repo that does not exist
      is an ERROR, not an empty clean result.
      Also reports: audit recency, external (Protofire) audit coverage + source
      drift, the first-party dependency graph, untested external surface, and
      the st0x deploy-health reads.

      --json <path>     Write the dashboard document here, overriding both
                        JSON_OUT and the site/health.json default.

  consumers <package>
      Reverse dependency lookup, in two layers: which repos across the orgs
      declare <package> as a dependency and at what version, and — with
      --symbol — which of those actually reference a symbol in their own
      Solidity. A consumer list alone does not answer \"does anything call
      unsafeList\", which is the question that gets asked.

      <package> is matched modulo spelling: the soldeer name (rain-solmem), the
      repo name (rain.solmem) and a submodule path basename all fold onto one
      key, because the manifest shapes disagree about which to write.

      Dependencies are read from EVERY manifest shape a repo may use, at any
      depth outside vendored trees, and unioned:
        foundry.toml    [dependencies], and `remappings` inside a [profile.*]
        soldeer.lock    [[dependencies]] name/version (array or table form)
        foundry.lock    JSON dependency-path -> {{rev|tag|branch}} pins
        remappings.txt  prefix/=dependencies/<name>-<version>/ targets
        .gitmodules     submodule path + url
      Reading one shape and calling it done is the defect this mode exists to
      prevent: rainlanguage/rainlang has no foundry.lock at all, so a
      foundry.lock-only sweep drops it silently. A manifest that will not parse
      is reported as UNKNOWN, never as \"declares nothing\". GitHub code search
      is not used: it indexes default branches only and under-returns without
      saying so.

      --symbol <name>   A Solidity identifier to look for in each consumer's own
                        sources (repeatable). Matched as a WHOLE identifier, so
                        `unsafeList` is not satisfied by `unsafeListOf`.
                        Vendored copies (lib/, dependencies/, node_modules/,
                        out/, cache/ at the repo root) are counted separately
                        and never as the repo's own use — a vendored copy of the
                        library holds the symbol's DEFINITION and would
                        otherwise read as a caller. Needs a shallow clone of
                        each consumer; with no --symbol nothing is cloned.
      --orgs \"a b c\"     Orgs to sweep, space- or comma-separated.
      --json <path>     Also write the full result as JSON. Opt-in: this mode
                        has no default output file.

ENVIRONMENT
  ORGS      space-separated orgs to scan. Default for `scan`: $ORG, else
            rainlanguage. Default for `consumers`:
              {orgs}
  ORG       single-org fallback when ORGS is unset.
  PAR       parallel workers (default 12).
  JSON_OUT  where `scan` writes its document (default site/health.json);
            --json overrides it. Not read by `consumers`.

Exit status: 2 for a usage error or a named repo that does not exist; 1 when a
repo could not be read, so the answer is INCOMPLETE and must not be taken as
complete; else 0.
GitHub access is read-only and uses the caller's authenticated `gh`.
",
        orgs = RAIN_ORGS.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn parse(v: &[&str]) -> Result<Command, CliError> {
        parse_args(&args(v))
    }

    /// The bug verbatim: `--help` used to be pushed onto the repo list, so the
    /// scan reported "0/1 repos, no findings" and exited 0.
    #[test]
    fn help_is_a_command_not_a_repo_name() {
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        // reachable even when the rest of the line is wrong
        assert_eq!(parse(&["--nonsense", "--help"]), Ok(Command::Help));
    }

    /// Every unrecognised flag is an error, in EITHER mode. The old parser
    /// silently accepted all of them as repo names.
    #[test]
    fn unknown_flags_are_errors() {
        for flag in ["--jsonn", "--verbose", "-x", "--json=foo"] {
            assert_eq!(
                parse(&[flag]),
                Err(CliError::UnknownFlag {
                    command: "scan",
                    flag: flag.to_string()
                }),
                "scan accepted {flag}"
            );
            assert_eq!(
                parse(&["consumers", "rain-solmem", flag]),
                Err(CliError::UnknownFlag {
                    command: "consumers",
                    flag: flag.to_string()
                }),
                "consumers accepted {flag}"
            );
        }
    }

    /// A flag at the end of the line has no value. Silently reading `None` is
    /// how `--json` used to become "write nowhere".
    #[test]
    fn a_flag_without_a_value_is_an_error() {
        assert_eq!(
            parse(&["--json"]),
            Err(CliError::MissingValue("--json".into()))
        );
    }

    #[test]
    fn an_empty_flag_value_is_an_error() {
        assert_eq!(
            parse(&["--json", ""]),
            Err(CliError::EmptyValue("--json".into()))
        );
        assert_eq!(
            parse(&["--json", "   "]),
            Err(CliError::EmptyValue("--json".into()))
        );
    }

    #[test]
    fn bare_scan_is_the_whole_org_with_no_json_override() {
        assert_eq!(
            parse(&[]),
            Ok(Command::Scan {
                json: None,
                repos: vec![]
            })
        );
    }

    #[test]
    fn scan_keeps_the_historical_positional_repo_form() {
        assert_eq!(
            parse(&["rain.dia", "rain.flare"]),
            Ok(Command::Scan {
                json: None,
                repos: vec!["rain.dia".into(), "rain.flare".into()]
            })
        );
        // …and the same with the mode named explicitly, and --json on either side
        assert_eq!(
            parse(&["scan", "--json", "/tmp/h.json", "rain.dia"]),
            Ok(Command::Scan {
                json: Some("/tmp/h.json".into()),
                repos: vec!["rain.dia".into()]
            })
        );
        assert_eq!(
            parse(&["rain.dia", "--json", "/tmp/h.json"]),
            Ok(Command::Scan {
                json: Some("/tmp/h.json".into()),
                repos: vec!["rain.dia".into()]
            })
        );
    }

    /// `scan` names the mode; it must not also become a repo name.
    #[test]
    fn the_scan_keyword_is_not_a_repo() {
        assert_eq!(
            parse(&["scan"]),
            Ok(Command::Scan {
                json: None,
                repos: vec![]
            })
        );
    }

    // ---- consumers mode ----

    #[test]
    fn consumers_takes_a_package_and_repeatable_symbols() {
        assert_eq!(
            parse(&[
                "consumers",
                "rain-solmem",
                "--symbol",
                "unsafeList",
                "--symbol",
                "unsafeCopyWordsTo",
            ]),
            Ok(Command::Consumers(ConsumersArgs {
                package: "rain-solmem".into(),
                symbols: vec!["unsafeList".into(), "unsafeCopyWordsTo".into()],
                orgs: None,
                json: None,
            }))
        );
    }

    #[test]
    fn consumers_needs_a_package() {
        assert_eq!(parse(&["consumers"]), Err(CliError::MissingPackage));
        assert_eq!(
            parse(&["consumers", "--symbol", "unsafeList"]),
            Err(CliError::MissingPackage)
        );
    }

    /// A second positional is a forgotten `--symbol`, not a second package.
    #[test]
    fn consumers_rejects_a_second_positional() {
        assert_eq!(
            parse(&["consumers", "rain-solmem", "unsafeList"]),
            Err(CliError::UnexpectedArgument {
                command: "consumers",
                arg: "unsafeList".into()
            })
        );
    }

    /// An empty symbol would match nothing and report every consumer as not
    /// referencing it — a wrong answer with no error, the shape of failure this
    /// whole parser exists to stop.
    #[test]
    fn an_empty_symbol_is_an_error_not_a_search_for_nothing() {
        assert_eq!(
            parse(&["consumers", "rain-solmem", "--symbol", ""]),
            Err(CliError::EmptyValue("--symbol".into()))
        );
        assert_eq!(
            parse(&["consumers", "rain-solmem", "--orgs", "  "]),
            Err(CliError::EmptyValue("--orgs".into()))
        );
        assert_eq!(
            parse(&["consumers", "rain-solmem", "--symbol"]),
            Err(CliError::MissingValue("--symbol".into()))
        );
    }

    #[test]
    fn orgs_split_on_spaces_and_commas() {
        let got = parse(&[
            "consumers",
            "rain-solmem",
            "--orgs",
            "rainlanguage, S01-Issuer gildlab",
        ]);
        let Ok(Command::Consumers(a)) = got else {
            panic!("{got:?}")
        };
        assert_eq!(
            a.orgs,
            Some(vec![
                "rainlanguage".into(),
                "S01-Issuer".into(),
                "gildlab".into()
            ])
        );
    }

    /// A single-org default would answer the reverse-dependency question
    /// wrongly and silently: S01-Issuer/st0x.deploy is a real consumer of
    /// rainlanguage packages.
    #[test]
    fn the_default_org_set_spans_every_rain_org() {
        for org in [
            "rainlanguage",
            "cyclofinance",
            "S01-Issuer",
            "ST0x-Technology",
            "h20liquidity",
            "gildlab",
            "raincommercial",
            "rain-archive",
        ] {
            assert!(RAIN_ORGS.contains(&org), "{org} is not swept by default");
        }
    }

    /// `--help` is where this tool's reference material lives, so it must
    /// actually name every mode, flag, manifest shape and default. A help text
    /// that omits one is why someone hand-rolls a shell loop instead.
    #[test]
    fn usage_documents_every_mode_flag_and_manifest_shape() {
        let u = usage();
        for token in [
            "scan",
            "consumers",
            "--symbol",
            "--orgs",
            "--json",
            "--help",
            "ORGS",
            "ORG",
            "PAR",
            "JSON_OUT",
            "Exit status",
            "foundry.toml",
            "soldeer.lock",
            "foundry.lock",
            "remappings.txt",
            ".gitmodules",
        ] {
            assert!(u.contains(token), "--help never mentions {token}");
        }
        for org in RAIN_ORGS {
            assert!(u.contains(org), "--help omits the default org {org}");
        }
    }
}
