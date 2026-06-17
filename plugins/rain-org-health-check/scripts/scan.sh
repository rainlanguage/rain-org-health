#!/usr/bin/env bash
# rain-org-health-check: scan rainlanguage org repos for modernization debt.
#
# Encodes the health signals from the rainix/soldeer modernization effort:
#   submodules, dead magic-nix-cache, bespoke (non-reusable) CI, removed rainix
#   tasks, PRIVATE_KEY_DEV, per-chain etherscan keys, telegram secret drift,
#   deprecated publish-soldeer, old action versions, soldeer publish gaps, and
#   foundry fuzz-config gaps / drift / pinned seeds.
#
# Usage:
#   scan.sh                 # scan all active non-fork org repos
#   scan.sh <repo> [repo..] # scan only the named repos
#   ORG=rainlanguage scan.sh
#
# Requires: gh (authenticated), curl, python3. Read-only (no writes/pushes).
set -uo pipefail

ORG="${ORG:-rainlanguage}"
PAR="${PAR:-12}"

command -v gh >/dev/null || { echo "error: gh CLI not found / not authenticated" >&2; exit 1; }

# ---- repo list -------------------------------------------------------------
if [ "$#" -gt 0 ]; then
  printf '%s\n' "$@" > /tmp/roh_repos.txt
else
  gh repo list "$ORG" --no-archived --limit 300 --json name,isFork \
    -q '.[]|select(.isFork==false)|.name' 2>/dev/null | sort > /tmp/roh_repos.txt
fi
TOTAL=$(wc -l < /tmp/roh_repos.txt | tr -d ' ')
echo "Scanning $TOTAL $ORG repos (parallel=$PAR)..." >&2

# ---- per-repo check --------------------------------------------------------
check_repo() {
  repo="$1"; org="$2"
  # contents fetch: emit decoded file only on HTTP 200 (empty on 404/error).
  api() {
    out=$(gh api "repos/$org/$repo/contents/$1" 2>/dev/null) || return 0
    printf '%s' "$out" | python3 -c 'import sys,json,base64
try: print(base64.b64decode(json.load(sys.stdin).get("content","")).decode("utf-8","replace"))
except Exception: pass' 2>/dev/null
  }
  exists() { gh api "repos/$org/$repo/contents/$1" >/dev/null 2>&1; }
  flags=""
  add() { flags="$flags $1"; }

  # workflows (fetch once, concatenated)
  wfnames=$(gh api "repos/$org/$repo/contents/.github/workflows" --jq '.[].name' 2>/dev/null) || wfnames=""
  wfblob=""
  while IFS= read -r wf; do
    [ -z "$wf" ] && continue
    case "$wf" in *.yaml|*.yml) ;; *) continue ;; esac
    wfblob="$wfblob"$'\n'"$(api ".github/workflows/$wf")"
  done <<< "$wfnames"

  foundry=$(api "foundry.toml")

  # ---- signals ----
  exists ".gitmodules" && add "submodules"
  printf '%s' "$wfblob" | grep -q 'magic-nix-cache' && add "dead-magic-nix-cache"
  printf '%s' "$wfblob" | grep -qE 'DeterminateSystems/nix-installer-action' && add "old-nix-installer"
  printf '%s' "$wfblob" | grep -qE '(-c|command|nix run[^ ]*) +rainix-(rs|sol)-artifacts|rainix-rs-prelude' && add "removed-rainix-task"
  printf '%s' "$wfblob" | grep -qE '\-c +rainix-(sol|rs)-(test|static|legal)|command +rainix-(sol|rs)-(test|static|legal)' \
    && ! printf '%s' "$wfblob" | grep -q 'rainlanguage/rainix/.github/workflows/' && add "bespoke-ci"
  printf '%s' "$wfblob" | grep -q 'PRIVATE_KEY_DEV' && add "private-key-dev"
  printf '%s' "$wfblob" | grep -q 'publish-soldeer' && add "deprecated-publish-soldeer"
  printf '%s' "$wfblob" | grep -qE 'TG_TOKEN|TG_CHAT_ID' && add "telegram-secret-drift"
  printf '%s' "$wfblob" | grep -qE 'actions/checkout@v[12]([^0-9]|$)' && add "old-actions-checkout"
  { printf '%s' "$wfblob"; printf '%s' "$foundry"; } | grep -qE 'CI_DEPLOY_[A-Z_]*ETHERSCAN_API_KEY' && add "per-chain-etherscan-key"
  printf '%s' "$wfblob" | grep -qE 'soldeer push' && printf '%s' "$wfblob" | grep -qE 'skip[-_]warnings' && add "soldeer-skip-warnings"

  # soldeer publish gap: has a [package] in foundry.toml but no version on the registry
  pkgname=$(printf '%s' "$foundry" | awk '/^\[package\]/{f=1;next} /^\[/{f=0} f&&/^name/{gsub(/name *= *|"/,"");print;exit}')
  if [ -n "$pkgname" ]; then
    reg=$(curl -fsSL "https://api.soldeer.xyz/api/v1/revision?project_name=${pkgname}&offset=0&limit=1" 2>/dev/null \
      | python3 -c "import sys,json;d=json.load(sys.stdin);print('yes' if d.get('data') else 'no')" 2>/dev/null)
    [ "$reg" = "no" ] && add "soldeer-unpublished"
  fi
  # sol lib that COULD publish but has no [package] at all (and not a deploy/app repo)
  if [ -n "$foundry" ] && [ -z "$pkgname" ] && printf '%s' "$foundry" | grep -q 'src ='; then
    : # heuristic only; skip to avoid noise
  fi

  # foundry fuzz config: the org canonical is the default profile's [fuzz] runs =
  # 5096. A foundry project with no [fuzz] runs leaves fuzz tests at foundry's
  # default 256 (shallow); one whose runs differs has drifted. (foundry.toml
  # usually omits `src`, relying on the default, so gate on the manifest itself.)
  # Parse the TOML for a `runs` under a [fuzz] / [profile.*.fuzz] section (NOT
  # optimizer_runs, which lives in the profile table).
  if [ -n "$foundry" ]; then
    fuzzruns=$(printf '%s' "$foundry" | awk '
      /^[[:space:]]*\[/ { infuzz = ($0 ~ /\[fuzz\]/ || $0 ~ /\.fuzz\][[:space:]]*$/) }
      infuzz && /^[[:space:]]*runs[[:space:]]*=/ { gsub(/[^0-9]/,""); print; exit }')
    if [ -z "$fuzzruns" ]; then add "no-fuzz-runs"
    elif [ "$fuzzruns" != "5096" ]; then add "fuzz-runs-drift"; fi
    # a pinned fuzz seed makes fuzzing deterministic (same inputs every CI run),
    # defeating cross-run input exploration — no org repo pins one, so flag it.
    printf '%s' "$foundry" | awk '
      /^[[:space:]]*\[/ { infuzz = ($0 ~ /\[fuzz\]/ || $0 ~ /\.fuzz\][[:space:]]*$/) }
      infuzz && /^[[:space:]]*seed[[:space:]]*=/ { f=1 } END { exit !f }' && add "fuzz-seed-pinned"
  fi

  [ -n "$flags" ] && printf '%s|%s\n' "$repo" "${flags# }"
}
export -f check_repo
export ORG

# ---- run -------------------------------------------------------------------
xargs -P "$PAR" -I{} bash -c 'check_repo "$@"' _ {} "$ORG" < /tmp/roh_repos.txt > /tmp/roh_findings.txt 2>/dev/null
sort -o /tmp/roh_findings.txt /tmp/roh_findings.txt

# ---- report ----------------------------------------------------------------
echo
echo "================ rain org health: per-repo findings ================"
if [ -s /tmp/roh_findings.txt ]; then
  awk -F'|' '{printf "  %-30s %s\n", $1, $2}' /tmp/roh_findings.txt
else
  echo "  (no findings — all clean)"
fi
echo
echo "================ org-wide summary (repos affected) ================="
awk -F'|' '{print $2}' /tmp/roh_findings.txt | tr ' ' '\n' | grep -v '^$' | sort | uniq -c | sort -rn \
  | awk '{printf "  %3d  %s\n", $1, $2}'
echo
echo "repos with findings: $(wc -l < /tmp/roh_findings.txt | tr -d ' ') / $TOTAL"
echo "raw findings: /tmp/roh_findings.txt"
