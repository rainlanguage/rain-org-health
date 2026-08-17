---
paths:
  - "site/**"
  - "plugins/rain-org-health-check/roh-scan/**"
  - ".github/workflows/pages.yml"
---

# The dashboard is a CONSUMER of data, never a PRODUCER

`site/` is a pure presentation layer: it **fetches JSON artifacts at runtime and
renders them** — it never generates data, shells out to tools, or reaches into
another repo's tooling. Every data source is owned + emitted by its
**producer**:

| data                             | produced by                                                                  | how the dashboard gets it    |
| -------------------------------- | ---------------------------------------------------------------------------- | ---------------------------- |
| repo modernization signals       | `roh-scan` (this repo) → `health.json`                                       | same-origin fetch            |
| producer-run metrics             | `issue-pr-cron` → `metrics/runs.jsonl`                                       | runtime fetch of its raw URL |
| pipeline / FSM-conformance state | `issue-pr-cron`'s `pr-review-report human-queue --json` → `human-queue.json` | runtime fetch of its raw URL |

Do not regress this:

- **The dashboard must not compute pipeline state.** It does NOT call
  `pr-review-report`, and `roh-scan` does NOT call it either — the pipeline repo
  emits `human-queue.json` on its own cron and the dashboard `fetch()`es it. A
  stale FSM panel is fixed in **issue-pr-cron's refresh**, not here.
- **Data changes must never require a Pages redeploy.** Keep frequently-changing
  data out of the `pages.yml` deploy path (`site/**`) — fetch it at runtime
  (`raw.githubusercontent.com` serves `access-control-allow-origin: *`). The
  site redeploys only when the **presentation** changes.
- **New data source ⇒ new fetch, not new baking.** Have the producing repo
  commit an artifact; fetch it here. Never embed/generate the data in this repo.
