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
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag { command, flag } => {
                write!(f, "unknown flag `{flag}` for `{command}`")
            }
            CliError::MissingValue(flag) => write!(f, "`{flag}` needs a value"),
            CliError::EmptyValue(flag) => write!(f, "`{flag}` was given an empty value"),
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
        // `scan` may be named explicitly; omitting it keeps the historical
        // `roh-scan [repo …]` form working.
        Some("scan") => parse_scan(&args[1..]),
        _ => parse_scan(args),
    }
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
pub fn usage() -> String {
    "roh-scan — rain org health scanner.

USAGE
  roh-scan [scan] [--json <path>] [<repo> ...]
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

ENVIRONMENT
  ORGS      space-separated orgs to scan. Default: $ORG, else rainlanguage.
  ORG       single-org fallback when ORGS is unset.
  PAR       parallel workers (default 12).
  JSON_OUT  where the scan writes its document (default site/health.json);
            --json overrides it.

Exit status: 2 for a usage error or a named repo that does not exist, else 0.
GitHub access is read-only and uses the caller's authenticated `gh`.
"
    .to_string()
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

    /// Every unrecognised flag is an error. The old parser silently accepted all
    /// of them as repo names.
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

    /// `--help` is where this tool's reference material lives, so it must
    /// actually name every mode, flag and default. A help text that omits one is
    /// why someone hand-rolls a shell loop instead of using the tool.
    #[test]
    fn usage_documents_every_mode_flag_and_env_var() {
        let u = usage();
        for token in [
            "scan", "--json", "--help", "ORGS", "ORG", "PAR", "JSON_OUT", "Exit status",
        ] {
            assert!(u.contains(token), "--help never mentions {token}");
        }
    }
}
