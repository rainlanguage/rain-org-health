# Health dashboard

Static, dependency-free dashboard for the org-health scan.

**Live: <https://rainlanguage.github.io/rain-org-health/>**

- `index.html` — self-contained page (inline CSS/JS, no build, no external requests) that fetches `health.json` and renders per-repo modernization-debt signals: stat tiles, a per-signal magnitude summary, and a filterable repo list. Theme follows the OS with a manual toggle.
- `health.json` — the data source, produced by the scan:

  ```
  FORMAT=json JSON_OUT=site/health.json \
    plugins/rain-org-health-check/scripts/scan.sh
  ```

  Omit repo args to scan the whole org. The committed copy is a real snapshot; re-run to refresh.

## Deploy

`.github/workflows/pages.yml` publishes `site/` to GitHub Pages on push to `master`. To refresh the data, run the scan locally (or in a scheduled job with an org-read token) and commit the regenerated `health.json`.

## Design

Every signal means one thing — modernization debt to clear — so signals use a single **status** color (debt amber), not per-signal categorical hues; a repo's identity of each signal is its text label + ▲ icon, never color alone (clean repos read ✓ green). Palette validated against the dataviz six-checks (CVD ΔE 15.4, contrast pass) in both light and dark.
