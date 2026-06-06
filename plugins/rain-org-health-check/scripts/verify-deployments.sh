#!/usr/bin/env bash
# rain-org-health: confirm deployed contracts are source-verified on explorers.
#
# For each deploy repo (one with src/generated/*.pointers.sol), reads the
# deterministic DEPLOYED_ADDRESS of every contract and checks whether that
# address is source-verified on each network the repo targets. Catches the
# silent failure where a contract is published/deployed but its `--verify` step
# never landed (e.g. a bytecode-metadata mismatch on one chain).
#
# Deterministic (Zoltu) deploys land at the SAME address on every chain, so each
# contract has one address checked across all of its networks.
#
# Usage:
#   verify-deployments.sh                  # discover + check all deploy repos
#   verify-deployments.sh rain.verify dvin.deploy
#
# Requires: gh, curl, python3, and EXPLORER_VERIFICATION_KEY (Etherscan V2 key).
# Flare/Songbird use Routescan (no key needed).
set -uo pipefail
ORG="${ORG:-rainlanguage}"
PAR="${PAR:-8}"
KEY="${EXPLORER_VERIFICATION_KEY:-}"
[ -z "$KEY" ] && echo "warn: EXPLORER_VERIFICATION_KEY unset — Etherscan-chain checks return '?'." >&2

# network -> Etherscan V2 chainid
es_chainid() { case "$1" in
  ethereum|mainnet) echo 1;; arbitrum) echo 42161;; base) echo 8453;;
  base_sepolia) echo 84532;; polygon) echo 137;; sepolia) echo 11155111;; *) echo "";; esac; }
# network -> Routescan chainid (non-etherscan explorers)
rs_chainid() { case "$1" in flare) echo 14;; songbird) echo 19;; *) echo "";; esac; }

# verification status of an address on a network -> verified | UNVERIFIED | ?
verif() {
  net="$1"; addr="$2"; cid=$(es_chainid "$net")
  if [ -n "$cid" ]; then
    [ -z "$KEY" ] && { echo "?"; return; }
    url="https://api.etherscan.io/v2/api?chainid=$cid&module=contract&action=getsourcecode&address=$addr&apikey=$KEY"
  else
    cid=$(rs_chainid "$net"); [ -z "$cid" ] && { echo "?"; return; }
    url="https://api.routescan.io/v2/network/mainnet/evm/$cid/etherscan/api?module=contract&action=getsourcecode&address=$addr"
  fi
  curl -fsSL --max-time 25 "$url" 2>/dev/null | python3 -c 'import sys,json
try:
  d=json.load(sys.stdin); r=d.get("result")
  r=r[0] if isinstance(r,list) and r else (r if isinstance(r,dict) else {})
  print("verified" if r.get("SourceCode") else "UNVERIFIED")
except Exception: print("?")'
}

# ---- repo list -------------------------------------------------------------
if [ "$#" -gt 0 ]; then printf '%s\n' "$@" > /tmp/vd_repos.txt
else gh repo list "$ORG" --no-archived --limit 300 --json name,isFork \
  -q '.[]|select(.isFork==false)|.name' 2>/dev/null | sort > /tmp/vd_repos.txt; fi

is_deploy_repo() { gh api "repos/$ORG/$1/contents/src/generated" \
  --jq '.[].name | select(endswith(".pointers.sol"))' 2>/dev/null | grep -q . ; }
export -f is_deploy_repo; export ORG
echo "Finding deploy repos (with src/generated/*.pointers.sol)..." >&2
deploys=$(xargs -P "$PAR" -I{} bash -c 'is_deploy_repo "$1" && echo "$1"' _ {} < /tmp/vd_repos.txt 2>/dev/null | sort)
[ -z "$deploys" ] && { echo "no deploy repos found"; exit 0; }

# ---- check each ------------------------------------------------------------
echo
for repo in $deploys; do
  ft=$(gh api "repos/$ORG/$repo/contents/foundry.toml" --jq '.content' 2>/dev/null | base64 -d 2>/dev/null)
  nets=$(printf '%s' "$ft" | sed -n '/\[etherscan\]/,/^\[/p' | grep -oE '^[a-z_]+ *=' | grep -oE '^[a-z_]+')
  [ -z "$nets" ] && nets="arbitrum base base_sepolia flare polygon"
  echo "### $repo  [networks: $(echo $nets | tr '\n' ' ')]"
  for pf in $(gh api "repos/$ORG/$repo/contents/src/generated" --jq '.[].name | select(endswith(".pointers.sol"))' 2>/dev/null); do
    name=${pf%.pointers.sol}
    addr=$(gh api "repos/$ORG/$repo/contents/src/generated/$pf" --jq '.content' 2>/dev/null | base64 -d 2>/dev/null \
      | grep -oE 'DEPLOYED_ADDRESS = address\(0x[0-9a-fA-F]+' | grep -oE '0x[0-9a-fA-F]+' | head -1)
    [ -z "$addr" ] && { echo "  $name: (no DEPLOYED_ADDRESS)"; continue; }
    line="  $name $addr:"
    for net in $nets; do line="$line $net=$(verif "$net" "$addr")"; done
    printf '%s\n' "$line"
  done
done
echo
echo "Legend: verified | UNVERIFIED (deployed-but-unverified, or not deployed) | ? (no key / unknown network)."
echo "UNVERIFIED on a live network = re-run the deploy's verify step for that chain."
