# HyperFrames local video pipeline — validated 2026-07-26

The "Doom → Relief" campaign's production pipeline, proven end-to-end on WSL2: plain-English/HTML compositions rendered to MP4 **entirely on our own machine** (on-brand: "made privately, on my own machine"). Two videos produced on day one — an 8s toolchain smoke test and campaign **Bit 3 "The Ominous Voice" v1** (9:16, 14.5s, caption-driven, no VO).

## What HyperFrames is

Open-source framework (HeyGen, MIT-ish, `heygen-com/hyperframes`) that renders video from HTML: the DOM declares clip timing via `data-*` attributes, one paused GSAP timeline drives animation, headless Chrome captures frames deterministically, FFmpeg encodes. Docs index: `https://hyperframes.heygen.com/llms.txt`. It ships agent skills (installed into `~/.claude/skills/`) that Claude Code picks up automatically after a project scaffold.

## One-time setup (WSL2, no sudo needed)

```bash
# Node 22+ required (CLI refuses less). Via existing nvm, default alias left at 20:
source ~/.nvm/nvm.sh && nvm install 22        # then per-shell: PATH prefix (below)

# FFmpeg (no apt access needed) — static build:
cd ~/dev/cleophis-media/tools
curl -sL -o f.tar.xz https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz
tar xf f.tar.xz && mkdir -p bin && ln -sf "$PWD"/ffmpeg-*-amd64-static/{ffmpeg,ffprobe} bin/

# Per shell:
export PATH="$HOME/.nvm/versions/node/v22.23.1/bin:$HOME/dev/cleophis-media/tools/bin:$PATH"
export HYPERFRAMES_BROWSER_PATH="/usr/bin/google-chrome"   # render needs a browser; system Chrome works
```

Workspace is **WSL-native** (`~/dev/cleophis-media/`), not `/mnt/c` — node + Chrome I/O on the Windows mount is painfully slow. Finished MP4s get copied to `/mnt/c/...` for viewing.

## Project scaffold + dev loop

```bash
cd ~/dev/cleophis-media
npx -y hyperframes init <name> --non-interactive --example blank   # ~5 min first time (3GB npx cache)
cd <name>
# author index.html (see composition contract below), then:
npx --yes hyperframes@0.7.71 check --snapshots   # REQUIRED gate: lint+runtime+layout+motion+WCAG contrast + PNG frames
npx --yes hyperframes@0.7.71 render --quality high --output renders/<name>.mp4
ffprobe -v error -show_entries format=duration -show_entries stream=width,height <file>
```

The scaffold pins `hyperframes@0.7.71` in package.json scripts; use the same pin for reproducible renders. `check` writes 5 snapshot PNGs — **eyeball them before rendering** (Claude can Read them directly). `npx hyperframes preview` serves the interactive Studio for human iteration.

## Composition contract (the rules that matter)

- Root `<div data-composition-id="main" data-start="0" data-width data-height data-duration>` — explicit px size; render length comes from root `data-duration`.
- Every timed element: `class="clip"` + `data-start` + `data-duration` + `data-track-index` (seconds, not frames). Sequential scenes share track 1; the framework owns clip visibility — never tween `.clip` elements themselves.
- Exactly one `gsap.timeline({ paused: true })` registered synchronously at `window.__timelines["main"]`.
- Deterministic only: no `Date.now`/`Math.random`/network at render; no `repeat: -1` (finite counts); animate only opacity/x/y/scale/rotation/color/backgroundColor.
- Full-screen fills go on an absolute `inset:0` **child** div, never the root's own background (root background can render black).
- Fonts: the compiler auto-fetches/caches Google Fonts (Inter) and bundles JetBrains Mono / EB Garamond — name bundled fonts directly instead of Georgia/Courier fallbacks.

## Traps hit on day one (so we don't hit them twice)

1. **Node 20 fails silently-ish** — `npm error could not determine executable to run` from bare `npx hyperframes`; the pin (`npx --yes hyperframes@0.7.71`) plus Node 22 PATH fixes it.
2. **Render needs a browser** — `check` works without config but `render` errors until `HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome` (or `npx hyperframes browser ensure` to fetch headless-shell).
3. **Tween-start vs clip-start flash** — a tween starting even 0.05s after its clip's `data-start` flashes the resting state for a frame. Align `fromTo` position exactly with the clip's `data-start` (or give the element `opacity: 0` in CSS when the tween ends visible).
4. **Overlapping opacity tweens lint** — fade-out must start at/after the fade-in tween's end time.
5. **Contrast is a gating audit (WCAG AA)** — amber-on-amber (a symbol drifting over the same-colored orb) flagged at 1.07:1; fixed by repositioning. Design captions to ≥4.5:1 (≥3:1 for 24px+).
6. **`sweep_static`** — a 3s+ stretch with zero animated change fails `check`; keep something subtly moving.

## Day-one results

| Video | Spec | Output | Render time |
| --- | --- | --- | --- |
| Smoke test "sunlight" (tutorial Level-1 equivalent) | 8s, 1920×1080, 30fps | 1.7 MB MP4, clean gate (12/12 contrast) | 3m05s |
| **Bit 3 "The Ominous Voice" v1** | 14.5s, 1080×1920, 30fps, caption-driven | 1.7 MB MP4, gate 9/9 contrast, 0 layout issues | 5m16s |

Bit 3 v1 beat sheet as built: doom (red HAL-trope eye + scanline, mono captions, cut mid-word at 4.5s) → relief (cream, CSS laptop + amber orb tutor, algebra symbols, 3 caption beats) → fold (small dim eye, "…that does sound kind of nice.") → tag (CLEOPHIS serif wordmark, amber rule, "Josiah's Library — expert AI, on your device."). All guardrails hold: pure trope parody, no real person/company, claims all true, Cleophis closes with dignity.

## Next steps (deferred by design)

- v2: voice-over (local TTS — Piper/Kokoro — to stay on-brand; the CLI also has `hyperframes tts`), music/SFX via the `media-use` skill.
- The multi-character "Terminator High" bits (need dialogue layout + more scenes).
- `/website-to-video` teaser from the landing page; Blotato scheduling/distribution.
- Optional: `render --docker` for bit-identical reproducible renders.
