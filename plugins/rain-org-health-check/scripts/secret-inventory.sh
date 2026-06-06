#!/usr/bin/env bash
# rain-org-health: inventory of GitHub Actions secret NAMES referenced across the org.
#
# Reports which secret *names* (identifiers) are referenced by each repo's
# workflows, to support secret consolidation / dead-secret cleanup. It reads
# only the names that already appear in workflow YAML (visible to anyone with
# repo read) — it NEVER reads, requests, or emits secret VALUES.
#
# Diff the referenced set this prints against the org's SET secrets
# (`gh api orgs/<org>/actions/secrets --jq '.secrets[].name'`, needs admin) to
# find dead secrets; group the near-duplicate names to plan consolidation.
#
# Usage:
#   secret-inventory.sh                 # all active non-fork org repos
#   secret-inventory.sh <repo> [repo..] # specific repos
#   ORG=rainlanguage secret-inventory.sh
#
# Requires: gh (authenticated), python3. Read-only.
set -uo pipefail

ORG="${ORG:-rainlanguage}"
PAR="${PAR:-12}"
command -v gh >/dev/null || { echo "error: gh CLI not found / not authenticated" >&2; exit 1; }

if [ "$#" -gt 0 ]; then
  printf '%s\n' "$@" > /tmp/secinv_repos.txt
else
  gh repo list "$ORG" --no-archived --limit 300 --json name,isFork \
    -q '.[]|select(.isFork==false)|.name' 2>/dev/null | sort > /tmp/secinv_repos.txt
fi
TOTAL=$(wc -l < /tmp/secinv_repos.txt | tr -d ' ')
echo "Inventorying secret names across $TOTAL $ORG repos..." >&2

scan_repo() {
  repo="$1"; org="$2"
  for wf in $(gh api "repos/$org/$repo/contents/.github/workflows" --jq '.[].name' 2>/dev/null); do
    case "$wf" in *.yaml|*.yml) ;; *) continue ;; esac
    body=$(gh api "repos/$org/$repo/contents/.github/workflows/$wf" 2>/dev/null \
      | python3 -c 'import sys,json,base64
try: print(base64.b64decode(json.load(sys.stdin).get("content","")).decode("utf-8","replace"))
except Exception: pass' 2>/dev/null)
    [ -z "$body" ] && continue
    # static: secrets.NAME  and  secrets['NAME'] / secrets["NAME"]
    printf '%s' "$body" | grep -oE 'secrets\.[A-Za-z_][A-Za-z0-9_]*' | sed 's/^secrets\.//'
    printf '%s' "$body" | grep -oE "secrets\[[\"'][A-Za-z_][A-Za-z0-9_]*[\"']\]" | grep -oE '[A-Za-z_][A-Za-z0-9_]*' | grep -vx 'secrets'
    # dynamic: secrets[<expr>] where the name is built at runtime (not statically resolvable)
    printf '%s' "$body" | grep -qE "secrets\[[^]\"']" && echo "__DYNAMIC__ $repo/$wf" >&2
  done | grep -vx 'inherit' | sort -u | sed "s|^|$repo|;s|^$repo|$repo\t|"
}
export -f scan_repo; export ORG

# collect: lines of "<repo>\t<SECRET_NAME>"
xargs -P "$PAR" -I{} bash -c 'scan_repo "$@"' _ {} "$ORG" < /tmp/secinv_repos.txt 2>/tmp/secinv_dynamic.txt \
  | sort -u > /tmp/secinv_pairs.txt

echo
echo "================= secret NAME inventory (referenced across org) ================="
printf '%-44s %5s   %s\n' "SECRET NAME" "REPOS" "(referencing repos)"
awk -F'\t' '{cnt[$2]++; if(repos[$2])repos[$2]=repos[$2]","$1; else repos[$2]=$1}
  END{for(n in repos) printf "%s\t%d\t%s\n", n, cnt[n], repos[n]}' /tmp/secinv_pairs.txt \
  | sort -t$'\t' -k2,2nr -k1,1 \
  | awk -F'\t' '{printf "%-44s %5d   %s\n", $1, $2, $3}'

echo
echo "================= distinct secret names: $(awk -F'\t' '{print $2}' /tmp/secinv_pairs.txt | sort -u | wc -l | tr -d ' ') ================="
echo "All referenced names (sorted) — diff against your SET org secrets to find dead ones:"
awk -F'\t' '{print $2}' /tmp/secinv_pairs.txt | sort -u | sed 's/^/  /'

if [ -s /tmp/secinv_dynamic.txt ]; then
  echo
  echo "================= dynamic secret access (name built at runtime — not in inventory) ================="
  echo "These workflows index secrets[<expr>]; the actual names can't be read statically — check them by hand:"
  sed 's/^__DYNAMIC__ /  /' /tmp/secinv_dynamic.txt | sort -u
fi
echo
echo "(names only — no secret values are ever read. pairs: /tmp/secinv_pairs.txt)"
