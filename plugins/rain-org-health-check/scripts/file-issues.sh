#!/usr/bin/env bash
# rain-org-health: file/track GitHub issues for findings (idempotent).
#
# Turns scan findings into one tracked issue per (repo, finding) in that repo,
# labeled `rain-health`, so results persist instead of scrolling away. Idempotent
# by a hidden marker in the issue body: re-running never duplicates, and a finding
# that has cleared gets its open issue auto-closed.
#
# DRY-RUN by default (prints what it would do). Pass --apply to actually write.
#
# Usage:
#   scan.sh > /dev/null                       # produces /tmp/roh_findings.txt
#   file-issues.sh                            # dry-run preview
#   file-issues.sh --apply                    # actually file/close
#   file-issues.sh --apply /path/findings.txt # custom findings file
#
# Findings format (one per line): "<repo>|<flag> <flag> ...". Requires gh (write).
set -uo pipefail
ORG="${ORG:-rainlanguage}"
LABEL="rain-health"
APPLY=0; [ "${1:-}" = "--apply" ] && { APPLY=1; shift; }
FINDINGS="${1:-/tmp/roh_findings.txt}"
[ -f "$FINDINGS" ] || { echo "no findings file: $FINDINGS (run scan.sh first)" >&2; exit 1; }
[ "$APPLY" = 1 ] && echo "MODE: APPLY (writing issues)" || echo "MODE: dry-run (no writes; pass --apply to file)"

# flag -> "title | remediation" (mirrors the SKILL.md table)
meta() { case "$1" in
  submodules)               echo "Migrate git submodules to soldeer|Replace lib/ submodules with [dependencies] + [soldeer] recursive_deps=false; rewrite imports to versioned soldeer paths; drop .gitmodules.";;
  dead-magic-nix-cache)     echo "Replace dead magic-nix-cache action|magic-nix-cache was sunset (HTTP 418, fails builds). Swap for nix-quick-install + cachix(rainlanguage) + cache-nix-action, or move CI to a rainix reusable.";;
  old-nix-installer)        echo "Bump nix installer|Replace DeterminateSystems/nix-installer-action with nixbuild/nix-quick-install-action@v30.";;
  removed-rainix-task)      echo "Remove deleted rainix task|Workflow runs a task removed from rainix (rainix-rs-prelude / *-artifacts). Convert CI to the reusable workflows; move deploy to manual-sol-artifacts.";;
  bespoke-ci)               echo "Adopt rainix reusable CI|Runs rainix sol/rs tasks inline. Replace with rainlanguage/rainix/.github/workflows/rainix-sol.yaml or rainix-rs.yaml (secrets: inherit).";;
  private-key-dev)          echo "Drop PRIVATE_KEY_DEV|Always sign with secrets.PRIVATE_KEY; remove the PRIVATE_KEY_DEV fallback.";;
  deprecated-publish-soldeer) echo "Migrate off publish-soldeer|publish-soldeer.yaml is removed. Use rainix-autopublish + [package].version in foundry.toml.";;
  telegram-secret-drift)    echo "Standardize telegram secrets|Use TELEGRAM_BOT_TOKEN / TELEGRAM_CHAT_ID, not TG_TOKEN / TG_CHAT_ID.";;
  old-actions-checkout)     echo "Bump actions/checkout|Pinned to a deprecated checkout (v1/v2). Bump to v4+.";;
  per-chain-etherscan-key)  echo "Consolidate etherscan key|Use EXPLORER_VERIFICATION_KEY (Etherscan V2 multichain) instead of per-chain CI_DEPLOY_<CHAIN>_ETHERSCAN_API_KEY. Keep flare/songbird (Routescan).";;
  soldeer-unpublished)      echo "Publish soldeer package|foundry.toml has [package] but no registry revision. Wire rainix-autopublish + .soldeerignore.";;
  *)                        echo "$1|See the rain-org-health-check playbook.";;
esac; }

ensure_label() { gh label create "$LABEL" --repo "$ORG/$1" --color 1d76db \
  --description "rain-org-health-check finding" >/dev/null 2>&1 || true; }

created=0 skipped=0 closed=0
# track which (repo,flag) are still active so we can close cleared ones
: > /tmp/roh_active.txt

while IFS='|' read -r repo flags; do
  [ -z "$repo" ] && continue
  for flag in $flags; do
    echo "$repo|$flag" >> /tmp/roh_active.txt
    IFS='|' read -r title rem <<< "$(meta "$flag")"
    marker="<!-- rain-health:$flag -->"
    # existing open issue for this (repo, flag)?
    existing=$(gh issue list --repo "$ORG/$repo" --label "$LABEL" --state open --limit 100 \
      --json number,body --jq ".[] | select(.body | contains(\"$marker\")) | .number" 2>/dev/null | head -1)
    if [ -n "$existing" ]; then
      echo "  skip   $repo #$existing  ($flag)"; skipped=$((skipped+1)); continue
    fi
    full="[rain-health] $title"
    body=$(printf '%s\n\n**Remediation:** %s\n\nFiled by the rain-org-health-check skill. Closes automatically when the finding clears.\n\n%s' \
      "Detected by an org health scan." "$rem" "$marker")
    if [ "$APPLY" = 1 ]; then
      ensure_label "$repo"
      n=$(gh issue create --repo "$ORG/$repo" --title "$full" --body "$body" --label "$LABEL" --jq '.number' 2>/dev/null \
          || gh issue create --repo "$ORG/$repo" --title "$full" --body "$body" --label "$LABEL" 2>/dev/null | grep -oE '[0-9]+$')
      echo "  create $repo #${n:-?}  ($flag)"
    else
      echo "  would-create $repo  ($flag): $full"
    fi
    created=$((created+1))
  done
done < "$FINDINGS"

# auto-close issues whose finding has cleared (labeled rain-health, marker no longer active)
while IFS= read -r repo; do
  gh issue list --repo "$ORG/$repo" --label "$LABEL" --state open --limit 100 --json number,body 2>/dev/null \
    | python3 -c 'import sys,json,re
for i in json.load(sys.stdin):
  m=re.search(r"rain-health:([a-z-]+)",i["body"] or "")
  if m: print(i["number"],m.group(1))' 2>/dev/null \
  | while read -r num flag; do
      grep -qx "$repo|$flag" /tmp/roh_active.txt && continue
      if [ "$APPLY" = 1 ]; then
        gh issue close "$num" --repo "$ORG/$repo" --comment "Finding cleared on the latest rain-org-health scan." >/dev/null 2>&1 && echo "  close  $repo #$num ($flag)"
      else echo "  would-close $repo #$num ($flag) — finding cleared"; fi
      closed=$((closed+1))
    done
done < <(cut -d'|' -f1 "$FINDINGS" | sort -u)

echo
echo "summary: $([ "$APPLY" = 1 ] && echo filed || echo would-file)=$created  skipped(existing)=$skipped  $([ "$APPLY" = 1 ] && echo closed || echo would-close)=$closed"
