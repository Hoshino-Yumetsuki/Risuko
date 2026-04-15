#!/usr/bin/env bash
# Test script for Risuko aria2-compatible JSON-RPC 2.0 endpoint
# Usage: ./scripts/test-rpc.sh [secret]
#   If your RPC secret is set, pass it as the first argument.

set -euo pipefail

HOST="http://127.0.0.1:16800/jsonrpc"
SECRET="${1:-}"
DL_URL="https://cdn.hotelnearmedanta.com/testfile.org/testfile.org-5GB.dat"

# Build token param if secret is provided
TOKEN_PARAM=""
if [[ -n "$SECRET" ]]; then
  TOKEN_PARAM="\"token:${SECRET}\","
fi

rpc() {
  local method="$1"
  local params="$2"
  local id="${3:-1}"
  local payload="{\"jsonrpc\":\"2.0\",\"method\":\"${method}\",\"params\":[${TOKEN_PARAM}${params}],\"id\":${id}}"

  echo ">>> ${method}"
  echo "    payload: ${payload}"
  curl -s -X POST "$HOST" \
    -H "Content-Type: application/json" \
    -d "$payload" | python3 -m json.tool
  echo ""
}

echo "============================================"
echo " Risuko JSON-RPC 2.0 Test"
echo " Endpoint: ${HOST}"
echo " Secret:   ${SECRET:-<none>}"
echo "============================================"
echo ""

# 1. getVersion
echo "--- 1. Get Version ---"
rpc "risuko.getVersion" ""

# 2. listMethods
echo "--- 2. List Methods ---"
rpc "system.listMethods" "" 2

# 3. getGlobalStat
echo "--- 3. Global Stats ---"
rpc "risuko.getGlobalStat" "" 3

# 4. getGlobalOption
echo "--- 4. Global Options ---"
rpc "risuko.getGlobalOption" "" 4

# 5. addUri - start a download
echo "--- 5. Add Download ---"
RESPONSE=$(curl -s -X POST "$HOST" \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"risuko.addUri\",\"params\":[${TOKEN_PARAM}[\"${DL_URL}\"],{}],\"id\":5}")
echo ">>> risuko.addUri"
echo "$RESPONSE" | python3 -m json.tool

GID=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',''))" 2>/dev/null || true)
echo "    GID: ${GID}"
echo ""

if [[ -z "$GID" ]]; then
  echo "ERROR: addUri did not return a GID. Check that Risuko engine is running."
  exit 1
fi

# 6. tellStatus
echo "--- 6. Tell Status ---"
sleep 1
rpc "risuko.tellStatus" "\"${GID}\"" 6

# 7. tellActive
echo "--- 7. Tell Active ---"
rpc "risuko.tellActive" "" 7

# 8. changeOption (limit download speed to 1MB/s)
echo "--- 8. Change Option (limit to 1MB/s) ---"
rpc "risuko.changeOption" "\"${GID}\",{\"max-download-limit\":\"1048576\"}" 8

sleep 1

# 9. tellStatus again to see speed limit applied
echo "--- 9. Tell Status (after speed limit) ---"
rpc "risuko.tellStatus" "\"${GID}\",[\"gid\",\"status\",\"totalLength\",\"completedLength\",\"downloadSpeed\"]" 9

# 10. pause
echo "--- 10. Pause Download ---"
rpc "risuko.pause" "\"${GID}\"" 10

sleep 1

# 11. tellStatus (should be paused)
echo "--- 11. Tell Status (paused) ---"
rpc "risuko.tellStatus" "\"${GID}\",[\"gid\",\"status\"]" 11

# 12. unpause
echo "--- 12. Unpause Download ---"
rpc "risuko.unpause" "\"${GID}\"" 12

sleep 1

# 13. tellStatus (should be active again)
echo "--- 13. Tell Status (resumed) ---"
rpc "risuko.tellStatus" "\"${GID}\",[\"gid\",\"status\",\"downloadSpeed\",\"completedLength\"]" 13

# 14. remove
echo "--- 14. Remove Download ---"
rpc "risuko.forceRemove" "\"${GID}\"" 14

sleep 1

# 15. tellStopped
echo "--- 15. Tell Stopped ---"
rpc "risuko.tellStopped" "0,10" 15

# 16. purgeDownloadResult
echo "--- 16. Purge Download Results ---"
rpc "risuko.purgeDownloadResult" "" 16

# 17. system.multicall
echo "--- 17. Multicall (getVersion + getGlobalStat) ---"
MULTI_PARAMS="[{\"methodName\":\"risuko.getVersion\",\"params\":[${TOKEN_PARAM%,}]},{\"methodName\":\"risuko.getGlobalStat\",\"params\":[${TOKEN_PARAM%,}]}]"
rpc "system.multicall" "${MULTI_PARAMS}" 17

echo "============================================"
echo " All tests complete"
echo "============================================"
