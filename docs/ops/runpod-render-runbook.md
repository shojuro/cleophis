# RunPod render runbook

**Standing policy: all heavy video work runs on a RunPod CPU pod.** The dev laptop
cannot do it — this is measured, not a preference.

## Why

Same jobs, same week:

| Stage | Dev laptop (12 cores, load 12–17) | RunPod `cpu5c` 32 vCPU |
| --- | --- | --- |
| Transcribe 21m47s audio | 2h46m / 10.2 CPU-hours, **never finished** | **5m50s** incl. whisper build + model download |
| Render 39,199 frames (1080p) | ~45 frames/min → ~14h projected | **~35 min** at ~1,100 frames/min |
| Dense-keyframe re-encode (21m) | not attempted | **62s** |
| 1fps thumbnails for zone map | **40 min** | seconds |
| Delivery compress | 20+ min (4-min clip) | **85s** (21-min clip) |

Cost for the full 21-minute video was **~$2.30**. The end-to-end driver test on a
30-second slice cost **$0.17**.

The local whisper failure was not merely slow: whisper.cpp stalled in a decode loop
and produced nothing, and a chunked parallel retry timed out because 12 whisper
threads on top of an existing load of 14 starved every chunk.

## CPU, not GPU

Deliberate. The cost is headless-Chrome frame capture and x264 encoding, and
HyperFrames caps capture at `min(16, cores-2)` workers — so vCPU count is the lever.
A GPU adds cost and would only help the encode tail. Revisit only if a job needs
GPU-bound generation (image/video models) rather than compositing.

## Credentials

The real key lives in `tools/pipeline/.env` as `RUNPOD_API_KEY=rpa_…` (gitignored).

**Trap:** a `RUNPOD_API_KEY` is usually present in the shell environment set to the
placeholder literal `your-runpod-api-key-here`; it 401s on every call. `podctl.sh`
ignores any value that does not start with `rpa_`, and resolves the repo's `.env`
through `git rev-parse --git-common-dir` so it works from a worktree. The key is
never echoed — keep it out of logs, `context.log`, and shell history.

SSH identity is `~/.ssh/id_ed25519`, which matches the public key RunPod injects.

## Usage

```bash
tools/runpod/podctl.sh status                       # ALWAYS run first — catches orphans
tools/runpod/video-job.sh prep   <project> <src.mp4>
#   -> project/{transcript.json,zonemap.json,metadata.json}; pod destroyed
#   (local, free: pick beats, run the project generator, review with the user)
tools/runpod/video-job.sh render <project> <src.mp4> --at 5,14,25
#   -> project/renders/output.mp4 + snapshots/; pod destroyed
```

Two phases exist so the pod is **down** during human review. That is the whole cost
model: a job is $1–2, an idle pod is ~$27/day.

`podctl.sh` alone is a general driver — `up`, `provision`, `exec`, `exec-bg`, `logs`,
`push`, `pull`, `down`, `status` — usable for any remote compute, not just video.

## Cost discipline

1. `podctl.sh status` at the start of any session. Anything `RUNNING` you did not
   just create is burning money.
2. Both `video-job.sh` phases install `trap cleanup EXIT INT TERM`, so the pod dies
   on failure and on Ctrl-C, not only on success.
3. Never leave a pod up across a review gap. Re-provisioning costs ~4 minutes;
   idling overnight costs ~$27.
4. If teardown ever fails the script says so explicitly — follow up with
   `podctl.sh status` and delete by id.

## What runs where

**Pod:** transcription, thumbnail extraction + zone analysis, dense-keyframe
re-encode, `lint`, `check`, `snapshot`, `render`, delivery compression.

**Local:** reading transcripts, choosing beats, running the project generators
(pure text transforms, milliseconds), git, docs.

## Pod spec

`runpod/base:0.6.2-cpu` (Ubuntu 20.04, ffmpeg preinstalled), `cpu5c` with `cpu3c`
fallback, 32 vCPU, 100 GB container disk, `22/tcp` exposed, SECURE cloud.
`bootstrap.sh` adds Node 22 (NodeSource), `google-chrome-stable`, `fonts-liberation`,
and numpy/pillow, and is idempotent.

Gotchas already handled in the scripts:

- HyperFrames' own Chrome download fails in minimal containers (no zip archiver) —
  always set `HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome`.
- Long renders need `PRODUCER_ENABLE_CHUNKED_ENCODE=true` and a raised
  `FFMPEG_ENCODE_TIMEOUT_MS`; the default 999,200 ms killed a 21-minute render.
- Source video must be re-encoded with `-g 30 -keyint_min 30` or frame capture
  freezes on seek.
- Long-running steps are launched detached with a done-marker and polled, so an SSH
  drop cannot kill the job.

## Media and git

Third-party footage and its derivatives are never committed — the repo is public.
Each video project's `.gitignore` excludes the source, `transcript.json`, renders,
and frames. Media travels by `scp` through `podctl push/pull`; a 302 MB source
uploads in ~93s and a 459 MB render comes back in a few minutes.
