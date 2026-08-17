# rain-org-health

Guidance that only applies to some paths lives in `.claude/rules/*.md` with
`paths:` frontmatter, so it loads when a matching file is read instead of on
every turn of every session. Put a new rule there rather than a section here:
this file is launch context, charged against the org agent-context cap whatever
the session turns out to be about.

Layout, commands and what CI runs are deliberately not restated here — they are
`README.md`, `site/README.md`, `nix flake show` and `.github/workflows/`, where
they cannot go stale against the thing they describe.
