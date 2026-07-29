#!/usr/bin/env bash
# presenter — the Cleophis on-brand narrator, built on the HeyGen CLI.
#
# A recurring presenter is a brand asset, not a per-video choice: the avatar and
# voice are pinned once in videos/_brand/presenter.json so the narrator never
# drifts between videos. Everything here reads that file.
#
#   presenter.sh doctor                       # auth + version preflight
#   presenter.sh shortlist [n]                # sample candidates for a human pick
#   presenter.sh pin <avatar_id> <voice_id> ["why"]
#   presenter.sh say <script.txt> -o <out.mp4> [--yes]
#   presenter.sh narrate <script.txt> -o <narration.wav>   # + word timestamps
#
# Auth is HeyGen's (~/.heygen/credentials, mode 0600); this script never reads,
# prints, or copies the credential. Video generation is METERED — `say` shows what
# it is about to spend on and asks, unless --yes is passed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BRAND_FILE="${PRESENTER_BRAND_FILE:-$REPO_ROOT/videos/_brand/presenter.json}"
MEDIA_USE="${MEDIA_USE_DIR:-$HOME/.claude/skills/media-use}"
export PATH="$HOME/.local/bin:$PATH"

die() { echo "presenter: $*" >&2; exit 1; }
say_hdr() { echo "== $* =="; }

need_cli() {
  command -v heygen >/dev/null 2>&1 || die "heygen CLI not found. Install: curl -fsSL https://static.heygen.ai/cli/install.sh | bash"
  local v; v=$(heygen --version 2>/dev/null | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' || true)
  [ -n "$v" ] || die "could not read heygen version"
}

need_auth() {
  need_cli
  heygen auth status >/dev/null 2>&1 || die "not authenticated. Run this yourself in an interactive terminal (OAuth needs a TTY):
    heygen auth login --oauth
Use --oauth, not --api-key: OAuth draws on the free subscription allowance."
}

brand_get() { # brand_get <key>
  [ -f "$BRAND_FILE" ] || die "no pinned presenter at $BRAND_FILE — run: presenter.sh shortlist, then presenter.sh pin <avatar_id> <voice_id>"
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$BRAND_FILE" "$1"
}

cmd_doctor() {
  need_cli
  echo "heygen: $(heygen --version)"
  if heygen auth status 2>&1 | head -20; then :; else
    echo "presenter: NOT AUTHENTICATED — run in an interactive terminal: heygen auth login --oauth"
  fi
  [ -f "$BRAND_FILE" ] && echo "pinned presenter: $BRAND_FILE" && cat "$BRAND_FILE" \
    || echo "pinned presenter: none yet ($BRAND_FILE)"
}

cmd_shortlist() {
  need_auth
  local n="${1:-6}" outdir="$REPO_ROOT/videos/_brand/shortlist"
  mkdir -p "$outdir"
  say_hdr "public avatars (first $n)"
  heygen avatar list --ownership public --limit "$n" | tee "$outdir/avatars.json"
  say_hdr "starfish voices (first $n)"
  heygen voice list --engine starfish --limit "$n" | tee "$outdir/voices.json"
  echo
  echo "Pick one avatar_id + one voice_id, then:"
  echo "  presenter.sh pin <avatar_id> <voice_id> \"why this one\""
  echo "To hear/see a candidate before pinning (metered, one short clip):"
  echo "  presenter.sh sample <avatar_id> <voice_id> $outdir/<name>.mp4"
}

cmd_sample() { # sample <avatar_id> <voice_id> <out.mp4>
  need_auth
  [ $# -ge 3 ] || die "usage: presenter.sh sample <avatar_id> <voice_id> <out.mp4>"
  _generate "$1" "$2" "Cleophis. Intelligence, close to home." "$3"
}

cmd_pin() {
  [ $# -ge 2 ] || die "usage: presenter.sh pin <avatar_id> <voice_id> [\"rationale\"]"
  mkdir -p "$(dirname "$BRAND_FILE")"
  python3 -c '
import json,sys
json.dump({"avatar_id":sys.argv[1],"voice_id":sys.argv[2],
           "rationale":sys.argv[3] if len(sys.argv)>3 else "",
           "note":"Pinned so the Cleophis narrator is identical across videos. Change deliberately, never per-video."},
          open(sys.argv[4],"w"), indent=2)' "$1" "$2" "${3:-}" "$BRAND_FILE"
  echo "presenter: pinned -> $BRAND_FILE"; cat "$BRAND_FILE"
}

_generate() { # _generate <avatar_id> <voice_id> <script-text> <out.mp4>
  local avatar="$1" voice="$2" text="$3" out="$4" payload
  payload=$(python3 -c '
import json,sys
print(json.dumps({"type":"avatar","avatar_id":sys.argv[1],
                  "script":sys.argv[2],"voice_id":sys.argv[3]}))' "$avatar" "$text" "$voice")
  mkdir -p "$(dirname "$out")"
  say_hdr "generating (metered)"
  heygen video create --headers "X-HeyGen-Client-Source: media-use" --wait -d "$payload" \
    | tee /tmp/heygen-video-$$.json
  local url
  url=$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
def find(o):
    if isinstance(o,dict):
        for k,v in o.items():
            if k in ("video_url","url") and isinstance(v,str) and v.startswith("http"): return v
            r=find(v)
            if r: return r
    if isinstance(o,list):
        for v in o:
            r=find(v)
            if r: return r
    return None
print(find(d) or "")' /tmp/heygen-video-$$.json)
  rm -f /tmp/heygen-video-$$.json
  [ -n "$url" ] || die "no video URL in the response — inspect the JSON above"
  curl -fsSL "$url" -o "$out"
  ffprobe -v error -show_entries format=duration,size \
    -show_entries stream=codec_name,codec_type,width,height -of default=noprint_wrappers=1 "$out"
  echo "presenter: wrote $out"
}

cmd_say() {
  need_auth
  local script="" out="" yes=0
  script="$1"; shift
  while [ $# -gt 0 ]; do
    case "$1" in
      -o) out="$2"; shift 2;;
      --yes) yes=1; shift;;
      *) die "unknown flag: $1";;
    esac
  done
  [ -f "$script" ] || die "script not found: $script"
  [ -n "$out" ] || die "-o <out.mp4> is required"

  local avatar voice text words
  avatar=$(brand_get avatar_id); voice=$(brand_get voice_id)
  text=$(tr '\n' ' ' < "$script" | sed 's/  */ /g')
  words=$(wc -w < "$script")
  echo "presenter: avatar=$avatar voice=$voice  (~$words words, ~$((words * 60 / 150))s)"
  if [ "$yes" -ne 1 ]; then
    echo "Video generation is metered. Re-run with --yes to proceed."
    exit 3
  fi
  _generate "$avatar" "$voice" "$text" "$out"
}

cmd_narrate() {
  need_auth
  local script="$1" out=""; shift
  while [ $# -gt 0 ]; do
    case "$1" in -o) out="$2"; shift 2;; *) die "unknown flag: $1";; esac
  done
  [ -f "$script" ] || die "script not found: $script"
  [ -n "$out" ] || die "-o <narration.wav> is required"
  local tts="$MEDIA_USE/audio/scripts/heygen-tts.mjs"
  [ -f "$tts" ] || die "media-use TTS script not found at $tts"
  # --words gives EXACT word timestamps, which beats transcribing the result:
  # they drive emphasis-panel timing downstream.
  node "$tts" "$(cat "$script")" -o "$out" --words "${out%.wav}.words.json"
  echo "presenter: wrote $out and ${out%.wav}.words.json"
}

[ $# -ge 1 ] || die "usage: presenter.sh {doctor|shortlist|sample|pin|say|narrate}"
sub="$1"; shift
case "$sub" in
  doctor)    cmd_doctor;;
  shortlist) cmd_shortlist "$@";;
  sample)    cmd_sample "$@";;
  pin)       cmd_pin "$@";;
  say)       cmd_say "$@";;
  narrate)   cmd_narrate "$@";;
  *)         die "unknown subcommand: $sub";;
esac
