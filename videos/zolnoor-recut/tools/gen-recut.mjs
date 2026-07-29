/* Deterministic generator for the Zolnoor recut composition.
   Writes storyboard.json, public/cards/card-XX.html, and public/index.html. */
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const WORK = process.argv[2];
const FPS = 30;
const W = 1080;
const H = 1920;
const DUR = 249.8;
const HOLD = 3.2; // default card hold
const q = (t) => Math.round(t * FPS) / FPS;

// beat = [spokenTime, words, accentIndex]
const BEATS = [
  [5.15, "COMPLETELY ARBITRARY", 4],
  [17.65, "THEY JUST DON'T APPLY", 0],
  [35.61, "IGNORE REPERCUSSIONS", 3],
  [40.87, "THE COURT BECOMES ILLEGITIMATE", 1],
  [56.51, "TAKE CONTROL = IN CONTROL", 4],
  [63.74, "NO ONE ASKS HOW YOU GOT THERE", 0],
  [75.48, "GO ALONG AND GET ALONG", 2],
  [79.75, "STRONGLY WORDED THINGS", 3],
  [99.28, "DELEGITIMIZE THE COURTS", 1],
  [122.07, "THAT WASN'T ALLOWED", 4],
  [133.57, "WHO'S GOING TO ENFORCE IT?", 0],
  [139.36, "WHERE ARE THOSE GUYS?", 3],
  [150.35, "IN THE WOODS. COSPLAYING.", 2],
  [161.49, "TYRANNY LOOKS EXACTLY LIKE THIS", 1],
  [172.72, "CO-EQUAL BRANCH", 4],
  [192.66, "I ANSWER TO A KING", 0],
  [213.6, "LEGITIMACY ONLY EXISTS BECAUSE WE GIVE IT", 3],
  [228.0, "THE LAWS ARE JUST AN AGREEMENT", 2],
  [239.07, "ARBITRARINESS DELEGITIMIZES IT", 1],
];

const sizeFor = (s) => (s.length <= 16 ? 128 : s.length <= 28 ? 108 : 88);
const ruleFor = (s) => (s.length <= 16 ? 260 : s.length <= 28 ? 360 : 440);

const cards = BEATS.map(([t, words, accent], i) => {
  const id = `card-${String(i + 1).padStart(2, "0")}`;
  const start = q(Math.max(0, t - 0.25));
  const next = BEATS[i + 1] ? BEATS[i + 1][0] - 0.25 : DUR;
  const dur = q(Math.min(HOLD, Math.max(1.8, next - start - 0.35)));
  return { id, start, dur, end: q(start + dur), words, accent, i };
});

// ---------- storyboard.json ----------
const storyboard = {
  schemaVersion: 3,
  composition: {
    fps: FPS,
    width: W,
    height: H,
    durationSeconds: DUR,
    layout: "portrait",
    themeId: "noir",
    seed: 42,
  },
  videoTrack: {
    sourcePath: "input-video.mp4",
    startSec: 0,
    endSec: DUR,
    bounds: { x: 0, y: 0, width: W, height: H },
  },
  subtitles: { enabled: false },
  cards: cards.map((c) => ({
    id: c.id,
    intent: `Emphasize the spoken line "${c.words}" as kinetic type over the full-bleed clip`,
    startSec: c.start,
    endSec: c.end,
    accentIndex: c.accent,
    zone: "video-overlay",
    contentHints: { emphasis: c.words },
  })),
};
writeFileSync(join(WORK, "storyboard.json"), JSON.stringify(storyboard, null, 2));

// ---------- card fragments ----------
mkdirSync(join(WORK, "public/cards"), { recursive: true });
const esc = (s) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function cardHtml(c) {
  const size = sizeFor(c.words);
  const rw = ruleFor(c.words);
  const spans = c.words
    .split(" ")
    .map((w) => `<span class="char">${esc(w)}</span>`)
    .join("\n          ");
  return `<div class="card" data-card-id="${c.id}">
  <style>
    .card[data-card-id="${c.id}"] .root {
      width: 100%;
      height: 100%;
      position: relative;
      overflow: hidden;
      background: transparent;
      font-family: 'Inter', ui-sans-serif, system-ui, sans-serif;
    }
    /* The source footage carries a permanent burned-in disclaimer at
       y≈1230–1590. The emphasis band sits ABOVE it and the scrim fades out
       before it, so the speaker's own caption is never occluded. */
    .card[data-card-id="${c.id}"] .scrim {
      position: absolute;
      left: 0;
      right: 0;
      top: 600px;
      height: 620px;
      background: linear-gradient(
        to bottom,
        rgba(6, 5, 16, 0) 0%,
        rgba(6, 5, 16, 0.82) 26%,
        rgba(6, 5, 16, 0.86) 82%,
        rgba(6, 5, 16, 0) 100%
      );
    }
    .card[data-card-id="${c.id}"] .block {
      position: absolute;
      left: 64px;
      right: 64px;
      bottom: 760px;
      display: flex;
      flex-direction: column;
      gap: 26px;
    }
    .card[data-card-id="${c.id}"] .rule {
      height: 10px;
      width: 0;
      border-radius: 5px;
      background: var(--accent-${c.accent});
      box-shadow: 0 0 26px var(--accent-${c.accent});
    }
    .card[data-card-id="${c.id}"] .emph {
      margin: 0;
      font: 800 ${size}px/1.06 'Inter', ui-sans-serif, system-ui, sans-serif;
      letter-spacing: -0.01em;
      text-transform: uppercase;
      color: #f7f5ff;
      text-shadow:
        0 6px 44px rgba(6, 5, 16, 0.95),
        0 0 70px rgba(6, 5, 16, 0.8);
    }
    .card[data-card-id="${c.id}"] .emph .char {
      display: inline-block;
      margin-right: 0.24em;
    }
  </style>

  <div class="root">
    <div class="scrim"></div>
    <div class="block">
      <div
        class="rule"
        id="${c.id}-rule"
        data-anim="grow-x"
        data-anim-at="0.10"
        data-anim-duration="0.55"
        data-anim-target-w="${rw}"
      ></div>
      <h2
        class="emph"
        id="${c.id}-emph"
        data-anim="kinetic-chars"
        data-anim-at="0.22"
        data-anim-duration="0.5"
        data-anim-stagger="0.06"
        data-anim-pattern="pop"
      >
          ${spans}
      </h2>
    </div>
  </div>
</div>`;
}

const fragments = cards.map((c) => {
  const html = cardHtml(c);
  writeFileSync(join(WORK, `public/cards/${c.id}.html`), html + "\n");
  return { c, html };
});

// ---------- composition ----------
const hosts = fragments
  .map(
    ({ c, html }) => `      <div
        class="card-host clip"
        id="host-${c.id}"
        data-card-id="${c.id}"
        data-start="${c.start.toFixed(4)}"
        data-duration="${c.dur.toFixed(4)}"
        data-track-index="2"
        style="left:0;top:0;width:${W}px;height:${H}px;visibility:hidden;opacity:0;"
      >
${html
  .split("\n")
  .map((l) => "        " + l)
  .join("\n")}
      </div>`,
  )
  .join("\n\n");

const tweens = cards
  .map((c) => {
    const sel = `'.card-host[data-card-id="${c.id}"]'`;
    const inAt = q(c.start + 0.22);
    const ruleAt = q(c.start + 0.1);
    const outAt = q(c.end - 0.4);
    return `          // ── ${c.id} · "${c.words}" [${c.start.toFixed(2)}–${c.end.toFixed(2)}] ──
          tl.set(${sel}, { visibility: 'visible' }, ${c.start.toFixed(4)});
          tl.fromTo(${sel}, { opacity: 0 }, { opacity: 1, duration: 0.38, ease: 'power2.out' }, ${c.start.toFixed(4)});
          tl.fromTo('.card[data-card-id="${c.id}"] #${c.id}-rule', { width: 0 }, { width: ${ruleFor(c.words)}, duration: 0.55, ease: 'power2.out' }, ${ruleAt.toFixed(4)});
          tl.from('.card[data-card-id="${c.id}"] #${c.id}-emph .char', { opacity: 0, y: 26, scale: 0.86, duration: 0.5, ease: 'power2.out', stagger: 0.06 }, ${inAt.toFixed(4)});
          tl.to(${sel}, { opacity: 0, duration: 0.35, ease: 'power2.in' }, ${outAt.toFixed(4)});
          tl.set(${sel}, { visibility: 'hidden' }, ${c.end.toFixed(4)});`;
  })
  .join("\n\n");

const index = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <style>
      @font-face {
        font-family: 'Inter';
        src: url('fonts/Inter-400-latin.woff2') format('woff2');
        font-weight: 400;
        font-display: block;
      }
      @font-face {
        font-family: 'Inter';
        src: url('fonts/Inter-700-latin.woff2') format('woff2');
        font-weight: 700;
        font-display: block;
      }

      :root {
        /* noir palette */
        --bg: #0a0a0a;
        --text: #f1f1f1;
        --accent-0: #4cc9f0;
        --accent-1: #f72585;
        --accent-2: #4ade80;
        --accent-3: #fb923c;
        --accent-4: #a78bfa;
      }
      * { box-sizing: border-box; }
      html, body {
        margin: 0;
        padding: 0;
        width: 100%;
        height: 100%;
        overflow: hidden;
        background: #000;
        font-family: 'Inter', ui-sans-serif, system-ui, sans-serif;
      }
      #stage {
        position: relative;
        width: 100%;
        height: 100%;
        overflow: hidden;
      }
      .video-wrapper {
        position: absolute;
        left: 0;
        top: 0;
        width: ${W}px;
        height: ${H}px;
        overflow: hidden;
      }
      .video-wrapper video {
        width: 100%;
        height: 100%;
        object-fit: cover;
      }
      .card-host {
        position: absolute;
        pointer-events: none;
        overflow: hidden;
      }
      .card-host .card {
        position: relative;
        width: 100%;
        height: 100%;
        overflow: hidden;
      }
      .card-host .char { display: inline-block; visibility: visible; }
    </style>
  </head>
  <body>
    <div
      id="stage"
      data-composition-id="talking-head-recut"
      data-start="0"
      data-duration="${DUR}"
      data-fps="${FPS}"
      data-width="${W}"
      data-height="${H}"
    >
      <div class="video-wrapper" id="video-wrap">
        <video
          id="bg-video"
          src="input-video.mp4"
          muted
          playsinline
          data-start="0"
          data-duration="${DUR}"
          data-track-index="1"
        ></video>
      </div>
      <audio
        id="source-audio"
        src="input-video.mp4"
        data-start="0"
        data-duration="${DUR}"
        data-track-index="10"
        data-volume="1"
      ></audio>

${hosts}

      <script src="vendor/gsap.min.js"></script>
      <script>
        (function () {
          const tl = window.gsap.timeline({ paused: true });

${tweens}

          window.__timelines = window.__timelines || {};
          window.__timelines['talking-head-recut'] = tl;
          tl.seek(0);
        })();
      </script>
    </div>
  </body>
</html>
`;
writeFileSync(join(WORK, "public/index.html"), index);
console.log(`cards: ${cards.length}`);
console.log(`last card ends: ${cards[cards.length - 1].end.toFixed(2)}s of ${DUR}s`);
console.log(
  cards.map((c) => `${c.start.toFixed(2)}-${c.end.toFixed(2)} ${c.words}`).join("\n"),
);
