#!/usr/bin/env bash
# video-job — run the heavy stages of a HyperFrames video job on a RunPod CPU pod.
#
# Two phases, so the pod is DOWN while a human reviews the beat list (the main
# cost lever — an idle pod is ~$27/day, a job is ~$1-2):
#
#   video-job.sh prep   <project-dir> <source.mp4>
#       -> transcript.json + zonemap.json, pod destroyed
#      (local, free: pick beats, run the project generator, review with the user)
#   video-job.sh render <project-dir> <source.mp4> [--at 3,208,613]
#       -> renders/output.mp4 + snapshots/, pod destroyed
#
# The pod is destroyed by an EXIT trap, so it dies even on failure or Ctrl-C.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PODCTL="$SCRIPT_DIR/podctl.sh"
HF_VER="${HYPERFRAMES_VERSION:-0.7.72}"
REMOTE=/root/job

die() { echo "video-job: $*" >&2; exit 1; }
say() { echo "== $* =="; }

cleanup() {
  local rc=$?
  echo
  say "tearing down pod (exit $rc)"
  "$PODCTL" down || echo "video-job: WARNING pod teardown failed — run tools/runpod/podctl.sh status"
  exit $rc
}

start_pod() {
  "$PODCTL" status | head -1
  "$PODCTL" up --name "hf-$1"
  trap cleanup EXIT INT TERM
  "$PODCTL" provision
}

# Long jobs run detached so an SSH hiccup cannot kill them; poll until the marker
# file appears.
run_detached() { # run_detached <remote-cmd> <logfile> <done-marker> <poll-secs> <label>
  local cmd="$1" log="$2" marker="$3" every="$4" label="$5" i
  "$PODCTL" exec "rm -f $marker; nohup bash -lc $(printf '%q' "$cmd; touch $marker") > $log 2>&1 & echo started" >/dev/null
  for i in $(seq 1 200); do
    if "$PODCTL" exec "test -f $marker && echo yes" 2>/dev/null | grep -q yes; then
      say "$label complete"; return 0
    fi
    sleep "$every"
    echo "   [$label] $("$PODCTL" exec "tail -c 120 $log | tr -d '\r' | tail -1" 2>/dev/null || echo waiting)"
  done
  die "$label did not finish in time; log: $log"
}

cmd_prep() {
  local proj="$1" src="$2"
  [ -d "$proj" ] || die "project dir not found: $proj"
  [ -f "$src" ] || die "source video not found: $src"

  start_pod prep
  say "uploading source ($(du -h "$src" | cut -f1))"
  "$PODCTL" push "$src" "$REMOTE/source.mp4"
  "$PODCTL" push "$SCRIPT_DIR/lib/zone-analyze.py" "$REMOTE/zone-analyze.py"

  say "probing + extracting audio"
  "$PODCTL" exec "cd $REMOTE && ffprobe -v error -select_streams v:0 \
      -show_entries stream=width,height,r_frame_rate -show_entries format=duration \
      -of json source.mp4 > metadata.json && \
      ffmpeg -y -v error -i source.mp4 -vn -acodec libmp3lame -q:a 2 audio.mp3 && \
      cat metadata.json"

  run_detached "cd $REMOTE && npx -y hyperframes@$HF_VER transcribe audio.mp3 -d $REMOTE --json --model small.en" \
               "$REMOTE/transcribe.log" "$REMOTE/.transcribe.done" 30 "transcribe"

  say "extracting 1fps thumbnails + scoring zones"
  local W H
  W=$("$PODCTL" exec "python3 -c \"import json;print(json.load(open('$REMOTE/metadata.json'))['streams'][0]['width'])\"" | tr -d '\r')
  H=$("$PODCTL" exec "python3 -c \"import json;print(json.load(open('$REMOTE/metadata.json'))['streams'][0]['height'])\"" | tr -d '\r')
  "$PODCTL" exec "cd $REMOTE && rm -rf thumbs && mkdir -p thumbs && \
      ffmpeg -y -v error -i source.mp4 -vf 'fps=1,scale=384:-2' -q:v 4 thumbs/f_%05d.jpg && \
      python3 zone-analyze.py thumbs zonemap.json --width $W --height $H"

  say "downloading transcript + zone map"
  "$PODCTL" pull "$REMOTE/transcript.json" "$proj/transcript.json"
  "$PODCTL" pull "$REMOTE/zonemap.json" "$proj/zonemap.json"
  "$PODCTL" pull "$REMOTE/metadata.json" "$proj/metadata.json"
  echo
  say "prep done: $proj/{transcript.json,zonemap.json,metadata.json}"
  echo "Next (local, no pod): choose beats, run the project generator, review with the user,"
  echo "then: tools/runpod/video-job.sh render $proj <source.mp4>"
}

cmd_render() {
  local proj="$1" src="$2"; shift 2
  local at=""
  while [ $# -gt 0 ]; do
    case "$1" in --at) at="$2"; shift 2;; *) die "unknown flag: $1";; esac
  done
  [ -f "$proj/public/index.html" ] || die "no composition at $proj/public/index.html — generate it first"
  [ -f "$src" ] || die "source video not found: $src"

  start_pod render
  say "uploading source + composition"
  "$PODCTL" push "$src" "$REMOTE/source.mp4"
  "$PODCTL" exec "mkdir -p $REMOTE/public"
  "$PODCTL" push "$proj/public/index.html" "$REMOTE/public/index.html"
  for d in cards fonts vendor; do
    [ -d "$proj/public/$d" ] && "$PODCTL" push "$proj/public/$d" "$REMOTE/public/"
  done

  say "re-encoding with dense keyframes (seekability for frame capture)"
  "$PODCTL" exec "cd $REMOTE && ffmpeg -y -v error -i source.mp4 -c:v libx264 -crf 18 \
      -preset veryfast -g 30 -keyint_min 30 -pix_fmt yuv420p -movflags +faststart \
      -c:a aac public/input-video.mp4 && ls -la public/input-video.mp4"

  say "lint + check"
  "$PODCTL" exec "cd $REMOTE && export HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome && \
      npx -y hyperframes@$HF_VER lint public 2>&1 | tail -3 && \
      npx -y hyperframes@$HF_VER check public 2>&1 | tail -12"

  if [ -n "$at" ]; then
    say "proof snapshots at $at"
    "$PODCTL" exec "cd $REMOTE && export HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome && \
        npx -y hyperframes@$HF_VER snapshot public --at $at 2>&1 | tail -4"
    mkdir -p "$proj/snapshots"
    "$PODCTL" pull "$REMOTE/public/snapshots/contact-sheet*.jpg" "$proj/snapshots/" || true
    echo "   contact sheet(s) in $proj/snapshots — INSPECT BEFORE TRUSTING THE RENDER"
  fi

  run_detached "cd $REMOTE && export HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome PRODUCER_ENABLE_CHUNKED_ENCODE=true FFMPEG_ENCODE_TIMEOUT_MS=7200000 && rm -rf public/snapshots && npx -y hyperframes@$HF_VER render public --skill=talking-head-recut -o output.mp4 --fps 30" \
               "$REMOTE/render.log" "$REMOTE/.render.done" 60 "render"

  say "verifying + compressing for delivery"
  "$PODCTL" exec "cd $REMOTE && ffprobe -v error -show_entries format=duration,size \
      -show_entries stream=codec_name,codec_type,width,height,channels \
      -of default=noprint_wrappers=1 output.mp4 && \
      ffmpeg -y -v error -i output.mp4 -c:v libx264 -crf 21 -preset fast -pix_fmt yuv420p \
      -c:a aac -b:a 160k -movflags +faststart output-web.mp4 && ls -la output-web.mp4"

  say "downloading"
  mkdir -p "$proj/renders"
  "$PODCTL" pull "$REMOTE/output-web.mp4" "$proj/renders/output.mp4"
  ffprobe -v error -show_entries format=duration,size \
    -show_entries stream=codec_name,codec_type,width,height,channels \
    -of default=noprint_wrappers=1 "$proj/renders/output.mp4"
  say "render done: $proj/renders/output.mp4"
}

[ $# -ge 3 ] || die "usage: video-job.sh {prep|render} <project-dir> <source.mp4> [--at t1,t2]"
sub="$1"; shift
case "$sub" in
  prep)   cmd_prep "$@";;
  render) cmd_render "$@";;
  *)      die "unknown phase: $sub (expected prep|render)";;
esac
