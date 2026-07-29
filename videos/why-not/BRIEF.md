---
workflow: motion-graphics
flow: automation
storyboard: no
message: "why not? — a defiant question surfacing calmly over catastrophe"
destination: web
aspect: 1920x1080
language: en
length: 20s
angle: statement-hit
---

# Brief — "why not?" (20s statement piece)

User spec (verbatim requirements):

1. 20-second video.
2. Background: a nuclear explosion, from initial explosion/flash through full
   mushroom-cloud formation, then fades to black.
3. Halfway through (~10s), a **glassmorphic overlay element** gently enters over
   the **middle third** of the screen.
4. Overlay text: **"why not?"** in **Georgia** type, **cobalt blue**.

Inferred decisions (autonomous background run, creative choices delegated in
this session's opening message):

- 1920x1080 @ 30fps, web destination — matches the sibling `cleophis-intro`
  project; no aspect was specified.
- "middle third" read as the middle **horizontal band** of the frame (y from
  360 to 720 in a 1080p canvas); the glass panel spans that band.
- "gently comes" = slow fade + small upward drift (autoAlpha 0→1, ~2.5s ease),
  no bounce/pop.
- No audio: the spec itemizes visuals only, and silence suits the subject.
  A sub-bass rumble or BGM can be added on request.
- Background footage: no stock/gen video provider exists in this environment
  (media-use has no video type; heygen CLI absent; no GEMINI/GOOGLE keys) —
  the explosion is a hand-authored deterministic animated layer (GSAP), the
  same accepted degrade as `cleophis-intro`. Stylized, not photoreal.
- Georgia is a system serif stack (`Georgia, 'Times New Roman', serif`) — no
  webfont fetch needed; deterministic.
- Cobalt blue = #0047AB. On a glass panel over dark/black frames it needs a
  luminance-safe treatment for WCAG contrast — final call in design phase.
- Render approved in advance: the deliverable is the video; the run is
  unattended (same resolution as cleophis-intro's render gate).
