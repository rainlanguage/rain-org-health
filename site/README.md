# Health dashboard

Static, dependency-free dashboard for the org-health scan.

**Live: <https://rainlanguage.github.io/rain-org-health/>**

- `repositories.html` — the landing page: self-contained (inline CSS/JS, no
  build, no external requests), fetches `health.json` and renders per-repo
  modernization-debt signals — a per-signal magnitude summary and a filterable
  repo list. Theme follows the OS with a manual toggle.
- `audit.html`, `pipeline.html`, `metrics.html`, `deployments.html` — the other
  pages, same shape; each fetches the artifact it reports on.
- `index.html` — a redirect to `repositories.html`. GitHub Pages serves it as
  the site root, so it stays even though the overview page it used to hold is
  gone.
- `health.json` — the data source, produced by the scan:

  ```
  nix run .#roh-scan -- --json site/health.json
  ```

  Omit repo args to scan the whole org. The committed copy is a real snapshot;
  re-run to refresh.

## Deploy

`.github/workflows/pages.yml` publishes `site/` to GitHub Pages on push to
`master`. To refresh the data, run the scan locally (or in a scheduled job with
an org-read token) and commit the regenerated `health.json`.

## Design

Every signal means one thing — modernization debt to clear — so signals use a
single **status** color (debt amber), not per-signal categorical hues; a repo's
identity of each signal is its text label + ▲ icon, never color alone (clean
repos read ✓ green). Palette validated against the dataviz six-checks (CVD ΔE
15.4, contrast pass) in both light and dark.
