#!/usr/bin/env bash
# podctl — thin RunPod pod lifecycle driver.
#
# Heavy media work (whisper, headless-Chrome frame capture, ffmpeg) does not run
# usefully on the dev laptop; see docs/ops/runpod-render-runbook.md for measured
# numbers. This script owns create/provision/transfer/exec/destroy so job scripts
# stay simple.
#
#   podctl.sh up [--vcpu 32] [--flavor cpu5c] [--name hf-render]
#   podctl.sh provision
#   podctl.sh exec "<command>"
#   podctl.sh exec-bg "<command>" <remote-logfile>
#   podctl.sh logs <remote-logfile> [bytes]
#   podctl.sh push <local> <remote>
#   podctl.sh pull <remote> <local>
#   podctl.sh down
#   podctl.sh status
#
# The API key is read from tools/pipeline/.env and is never printed. Note the
# RUNPOD_API_KEY commonly present in the shell environment is a placeholder
# literal ("your-runpod-api-key-here") that 401s — this script ignores it unless
# it looks like a real key.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_FILE="$SCRIPT_DIR/.state.json"
SSH_KEY="${RUNPOD_SSH_KEY:-$HOME/.ssh/id_ed25519}"
API="https://rest.runpod.io/v1"

SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
          -o ServerAliveInterval=20 -o ServerAliveCountMax=6 -o ConnectTimeout=25)

die() { echo "podctl: $*" >&2; exit 1; }

# ---------------------------------------------------------------- credentials
load_key() {
  # 1. explicit override, 2. repo's tools/pipeline/.env (works from a worktree),
  # 3. ~/.runpod.env. A value that does not start with rpa_ is treated as absent
  # so the well-known placeholder cannot silently cause 401s.
  local candidates=() common main
  [ -n "${RUNPOD_ENV_FILE:-}" ] && candidates+=("$RUNPOD_ENV_FILE")
  if common=$(git -C "$SCRIPT_DIR" rev-parse --git-common-dir 2>/dev/null); then
    main=$(cd "$(dirname "$common")" && pwd)
    candidates+=("$main/tools/pipeline/.env")
  fi
  candidates+=("$HOME/.runpod.env")

  if [ -n "${RUNPOD_API_KEY:-}" ] && [[ "$RUNPOD_API_KEY" == rpa_* ]]; then
    KEY="$RUNPOD_API_KEY"; return
  fi
  local f v
  for f in "${candidates[@]}"; do
    [ -f "$f" ] || continue
    v=$(grep -m1 -E '^[[:space:]]*RUNPOD_API_KEY=' "$f" 2>/dev/null | cut -d= -f2- | tr -d '"'"'"' \r\n' || true)
    if [ -n "$v" ] && [[ "$v" == rpa_* ]]; then KEY="$v"; return; fi
  done
  die "no usable RUNPOD_API_KEY found (looked in: ${candidates[*]}). A key must start with rpa_."
}

api() { # api <METHOD> <path> [json-body]
  local method="$1" path="$2" body="${3:-}" out code
  out=$(mktemp)
  if [ -n "$body" ]; then
    code=$(curl -sS -X "$method" -H "Authorization: Bearer $KEY" \
      -H "Content-Type: application/json" -d "$body" "$API$path" -o "$out" -w '%{http_code}')
  else
    code=$(curl -sS -X "$method" -H "Authorization: Bearer $KEY" "$API$path" -o "$out" -w '%{http_code}')
  fi
  if [ "$code" -ge 400 ]; then
    echo "podctl: API $method $path -> HTTP $code" >&2
    head -c 400 "$out" >&2; echo >&2
    rm -f "$out"; return 1
  fi
  cat "$out"; rm -f "$out"
}

state_get() { # state_get <key>
  [ -f "$STATE_FILE" ] || die "no active pod (missing $STATE_FILE). Run: podctl.sh up"
  python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2],''))" "$STATE_FILE" "$1"
}

ssh_pod() {
  local ip port; ip=$(state_get ip); port=$(state_get port)
  ssh "${SSH_OPTS[@]}" -i "$SSH_KEY" -p "$port" "root@$ip" "$@"
}

# ------------------------------------------------------------------- commands
cmd_up() {
  local vcpu=32 flavor=cpu5c name="hf-render" disk=100
  while [ $# -gt 0 ]; do
    case "$1" in
      --vcpu) vcpu="$2"; shift 2;;
      --flavor) flavor="$2"; shift 2;;
      --name) name="$2"; shift 2;;
      --disk) disk="$2"; shift 2;;
      *) die "unknown flag for up: $1";;
    esac
  done
  [ -f "$STATE_FILE" ] && die "a pod is already tracked ($(state_get id)). Run: podctl.sh down"
  [ -f "$SSH_KEY" ] || die "ssh key not found at $SSH_KEY"

  echo "podctl: creating $flavor pod, ${vcpu} vCPU, ${disk}GB disk..."
  local body res id
  body=$(python3 -c '
import json,sys
print(json.dumps({
 "name": sys.argv[1], "imageName": "runpod/base:0.6.2-cpu",
 "computeType": "CPU", "cpuFlavorIds": [sys.argv[2], "cpu3c"],
 "vcpuCount": int(sys.argv[3]), "containerDiskInGb": int(sys.argv[4]),
 "volumeInGb": 0, "ports": ["22/tcp"], "cloudType": "SECURE"}))' "$name" "$flavor" "$vcpu" "$disk")
  res=$(api POST /pods "$body") || die "pod creation failed"
  id=$(echo "$res" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  echo "podctl: pod $id created; waiting for SSH endpoint..."

  local i ip port cost
  for i in $(seq 1 40); do
    res=$(api GET "/pods/$id") || true
    ip=$(echo "$res" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("publicIp") or "")' 2>/dev/null || echo "")
    port=$(echo "$res" | python3 -c 'import json,sys; d=json.load(sys.stdin); m=d.get("portMappings") or {}; print(m.get("22") or "")' 2>/dev/null || echo "")
    cost=$(echo "$res" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("costPerHr") or 0)' 2>/dev/null || echo 0)
    if [ -n "$ip" ] && [ -n "$port" ]; then break; fi
    sleep 10
  done
  [ -n "${ip:-}" ] && [ -n "${port:-}" ] || { api DELETE "/pods/$id" >/dev/null 2>&1 || true; die "pod $id never exposed SSH; terminated it"; }

  python3 -c '
import json,sys,time
json.dump({"id":sys.argv[1],"ip":sys.argv[2],"port":int(sys.argv[3]),
           "costPerHr":float(sys.argv[4]),"createdAt":time.time()},
          open(sys.argv[5],"w"), indent=1)' "$id" "$ip" "$port" "$cost" "$STATE_FILE"

  echo "podctl: waiting for sshd..."
  for i in $(seq 1 30); do
    if ssh_pod "true" >/dev/null 2>&1; then break; fi
    sleep 10
  done
  ssh_pod "true" >/dev/null 2>&1 || die "pod $id up but SSH not answering"
  echo "podctl: ready — $id at $ip:$port (\$$cost/hr)"
}

cmd_provision() {
  echo "podctl: provisioning (idempotent)..."
  ssh_pod "bash -s" < "$SCRIPT_DIR/bootstrap.sh"
}

cmd_down() {
  [ -f "$STATE_FILE" ] || { echo "podctl: no tracked pod; nothing to do"; return 0; }
  local id elapsed cost
  id=$(state_get id); cost=$(state_get costPerHr)
  elapsed=$(python3 -c '
import json,time,sys
d=json.load(open(sys.argv[1]))
m=(time.time()-d["createdAt"])/60
spend=m/60*d.get("costPerHr",0)
print("%.1f min, ~$%.2f" % (m, spend))' "$STATE_FILE")
  api DELETE "/pods/$id" >/dev/null && echo "podctl: terminated $id (up $elapsed)"
  rm -f "$STATE_FILE"
}

cmd_status() {
  local res
  res=$(api GET /pods) || die "could not list pods"
  echo "$res" | python3 -c '
import json,sys
d=json.load(sys.stdin); pods = d if isinstance(d,list) else d.get("data",[])
running=[p for p in pods if p.get("desiredStatus")=="RUNNING"]
print("pods: %d total, %d RUNNING" % (len(pods), len(running)))
for p in pods:
    flag = "  <-- BILLING" if p.get("desiredStatus")=="RUNNING" else ""
    print("  %s  %s  %s  $%s/hr%s" % (p.get("id"), p.get("name"),
          p.get("desiredStatus"), p.get("costPerHr"), flag))
if running:
    print("")
    print("Terminate anything unexpected: podctl.sh down (tracked) or DELETE /v1/pods/<id>")'
  [ -f "$STATE_FILE" ] && echo "tracked: $(state_get id) at $(state_get ip):$(state_get port)"
  return 0
}

main() {
  [ $# -ge 1 ] || die "usage: podctl.sh {up|provision|exec|exec-bg|logs|push|pull|down|status}"
  local sub="$1"; shift
  load_key
  case "$sub" in
    up)        cmd_up "$@";;
    provision) cmd_provision;;
    exec)      ssh_pod "$@";;
    exec-bg)   ssh_pod "nohup bash -lc $(printf '%q' "$1") > $2 2>&1 & echo started";;
    logs)      ssh_pod "tail -c ${2:-2000} $1";;
    push)      scp "${SSH_OPTS[@]}" -i "$SSH_KEY" -P "$(state_get port)" -r "$1" "root@$(state_get ip):$2";;
    pull)      scp "${SSH_OPTS[@]}" -i "$SSH_KEY" -P "$(state_get port)" -r "root@$(state_get ip):$1" "$2";;
    down)      cmd_down;;
    status)    cmd_status;;
    *)         die "unknown subcommand: $sub";;
  esac
}

main "$@"
