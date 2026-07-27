/* Deterministic generator for the House of El recut.
 *
 * Placement: each beat is assigned to one of three zones — left third, right
 * third, or bottom band. The choice is "random" in feel but deterministic in
 * fact: a seeded PRNG picks among only the zones that are visually FREE at that
 * beat, using the per-second busyness map built from 1fps thumbnails
 * (zonemap.json). This is what stops a panel landing on the video's own article
 * screenshots, which occupy the left third for ~25% of the runtime.
 *
 * Style: "liquid glass" — thick backdrop blur, specular rim, lit top bevel,
 * shaded base, soft refractive edge, rounded slab.
 */
import { writeFileSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const WORK = process.argv[2];
const ZONEMAP = process.argv[3];
const BEATS_FILE = process.argv[4];

const FPS = 30;
const W = 1920;
const H = 1080;
const DUR = Number(process.env.HF_DUR || 1306.61); // override for slices/tests
const HOLD = 3.6;
const q = (t) => Math.round(t * FPS) / FPS;

// busyness thresholds above which a zone counts as occupied by video content
const BLOCK = { left: 20.0, right: 16.0, bottom: 40.0 };

// panel geometry per zone (full-res px)
const ZONES = {
  left: { x: 70, y: 330, w: 570, h: 430, align: "flex-start" },
  right: { x: 1280, y: 330, w: 570, h: 430, align: "flex-start" },
  bottom: { x: 500, y: 800, w: 920, h: 210, align: "center" },
};

const zonemap = JSON.parse(readFileSync(ZONEMAP, "utf8"));
const BEATS = JSON.parse(readFileSync(BEATS_FILE, "utf8")); // [{t, words}]

// deterministic PRNG (mulberry32) — seeded, so the scatter reproduces exactly
function mulberry32(a) {
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rand = mulberry32(20260727);

function zoneFreeAt(zone, startSec, endSec) {
  for (let s = Math.floor(startSec); s <= Math.ceil(endSec); s++) {
    const row = zonemap[String(s)];
    if (!row) continue;
    if (row[zone] > BLOCK[zone]) return false;
  }
  return true;
}

const sizeFor = (s, zone) => {
  const base = zone === "bottom" ? 76 : 64;
  if (s.length <= 14) return base + 16;
  if (s.length <= 26) return base;
  return base - 12;
};

let lastZone = null;
const cards = BEATS.map((b, i) => {
  const id = `card-${String(i + 1).padStart(2, "0")}`;
  const start = q(Math.max(0, b.t - 0.25));
  const next = BEATS[i + 1] ? BEATS[i + 1].t - 0.25 : DUR;
  const dur = q(Math.min(HOLD, Math.max(2.0, next - start - 0.4)));
  const end = q(start + dur);

  // candidate zones that are actually free for this card's whole window
  let free = Object.keys(ZONES).filter((z) => zoneFreeAt(z, start, end));
  if (free.length === 0) free = ["right"]; // right is free ~100% of the runtime
  // avoid repeating the same zone back-to-back when an alternative exists
  const varied = free.filter((z) => z !== lastZone);
  const pool = varied.length ? varied : free;
  const zone = pool[Math.floor(rand() * pool.length)];
  lastZone = zone;

  return { id, start, dur, end, zone, words: b.words, accent: i % 5, i };
});

// ---------- storyboard.json ----------
const storyboard = {
  schemaVersion: 3,
  composition: {
    fps: FPS,
    width: W,
    height: H,
    durationSeconds: DUR,
    layout: "landscape",
    themeId: "noir",
    seed: 20260727,
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
    intent: `Liquid-glass emphasis panel "${c.words}" in the ${c.zone} zone`,
    startSec: c.start,
    endSec: c.end,
    accentIndex: c.accent,
    zone: c.zone === "bottom" ? "lower-third" : "video-overlay",
    contentHints: { emphasis: c.words, placement: c.zone },
  })),
};
writeFileSync(join(WORK, "storyboard.json"), JSON.stringify(storyboard, null, 2));

// ---------- card fragments ----------
mkdirSync(join(WORK, "public/cards"), { recursive: true });
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function cardHtml(c) {
  const z = ZONES[c.zone];
  const size = sizeFor(c.words, c.zone);
  const spans = c.words
    .split(" ")
    .map((w) => `<span class="char">${esc(w)}</span>`)
    .join("\n            ");
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
    /* ---- liquid glass slab ---- */
    .card[data-card-id="${c.id}"] .panel {
      position: absolute;
      left: ${z.x}px;
      top: ${z.y}px;
      width: ${z.w}px;
      height: ${z.h}px;
      border-radius: 34px;
      overflow: hidden;
      display: flex;
      flex-direction: column;
      justify-content: center;
      align-items: ${z.align};
      gap: 22px;
      padding: 40px 44px;
      box-sizing: border-box;
      /* refraction: thick blur + saturation lift, like glass over the scene */
      -webkit-backdrop-filter: blur(30px) saturate(2.1) brightness(1.02) contrast(1.06);
      backdrop-filter: blur(30px) saturate(2.1) brightness(1.02) contrast(1.06);
      /* SMOKED glass, not clear: the clip cuts to bright article screenshots and
         a bright warm-lit wall, and clear glass dropped white type to 2.6:1
         there. A dark tint keeps the refractive character while holding
         contrast over both bright and dark footage. */
      background: linear-gradient(
        135deg,
        rgba(20, 26, 42, 0.54) 0%,
        rgba(10, 14, 24, 0.42) 42%,
        rgba(16, 22, 42, 0.48) 100%
      );
      border: 1px solid rgba(255, 255, 255, 0.34);
      box-shadow:
        0 30px 70px rgba(4, 6, 14, 0.5),
        0 8px 20px rgba(4, 6, 14, 0.34),
        0 0 0 1px rgba(255, 255, 255, 0.1),
        inset 0 1px 0 rgba(255, 255, 255, 0.78),
        inset 0 3px 14px rgba(255, 255, 255, 0.1),
        inset 0 -1px 0 rgba(255, 255, 255, 0.14),
        inset 0 0 52px rgba(160, 190, 255, 0.1);
    }
    /* specular sweep — the highlight that sells thickness */
    .card[data-card-id="${c.id}"] .sheen {
      position: absolute;
      left: -30%;
      top: -60%;
      width: 90%;
      height: 220%;
      transform: rotate(18deg);
      background: linear-gradient(
        to right,
        rgba(255, 255, 255, 0) 0%,
        rgba(255, 255, 255, 0.18) 45%,
        rgba(255, 255, 255, 0.26) 55%,
        rgba(255, 255, 255, 0) 100%
      );
      pointer-events: none;
    }
    /* refractive inner rim with a faint cool fringe */
    .card[data-card-id="${c.id}"] .rim {
      position: absolute;
      inset: 3px;
      border-radius: 31px;
      border: 1px solid rgba(190, 214, 255, 0.28);
      box-shadow: inset 0 0 22px rgba(140, 180, 255, 0.14);
      pointer-events: none;
    }
    .card[data-card-id="${c.id}"] .accent {
      position: relative;
      height: 8px;
      width: 0;
      border-radius: 4px;
      background: var(--accent-${c.accent});
      box-shadow: 0 0 22px var(--accent-${c.accent});
    }
    .card[data-card-id="${c.id}"] .emph {
      position: relative;
      margin: 0;
      font: 800 ${size}px/1.08 'Inter', ui-sans-serif, system-ui, sans-serif;
      letter-spacing: -0.012em;
      text-transform: uppercase;
      text-align: ${c.zone === "bottom" ? "center" : "left"};
      color: #ffffff;
      text-shadow:
        0 2px 18px rgba(8, 12, 24, 0.75),
        0 1px 2px rgba(8, 12, 24, 0.6);
    }
    .card[data-card-id="${c.id}"] .emph .char {
      display: inline-block;
      margin-right: 0.22em;
    }
  </style>

  <div class="root">
    <div class="panel" id="${c.id}-panel">
      <div class="sheen" data-layout-allow-overflow="true"></div>
      <div class="rim"></div>
      <div
        class="accent"
        id="${c.id}-accent"
        data-anim="grow-x"
        data-anim-at="0.16"
        data-anim-duration="0.6"
        data-anim-target-w="${c.zone === "bottom" ? 300 : 210}"
      ></div>
      <h2
        class="emph"
        id="${c.id}-emph"
        data-anim="kinetic-chars"
        data-anim-at="0.26"
        data-anim-duration="0.52"
        data-anim-stagger="0.055"
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
${html.split("\n").map((l) => "        " + l).join("\n")}
      </div>`,
  )
  .join("\n\n");

const tweens = cards
  .map((c) => {
    const host = `'.card-host[data-card-id="${c.id}"]'`;
    const panel = `'.card[data-card-id="${c.id}"] #${c.id}-panel'`;
    const accentAt = q(c.start + 0.16);
    const textAt = q(c.start + 0.26);
    const outAt = q(c.end - 0.42);
    return `          // ── ${c.id} · ${c.zone} · "${c.words}" [${c.start.toFixed(2)}–${c.end.toFixed(2)}] ──
          tl.set(${host}, { visibility: 'visible' }, ${c.start.toFixed(4)});
          tl.fromTo(${host}, { opacity: 0 }, { opacity: 1, duration: 0.4, ease: 'power2.out' }, ${c.start.toFixed(4)});
          tl.fromTo(${panel}, { scale: 0.9, y: 20 }, { scale: 1, y: 0, duration: 0.72, ease: 'back.out(1.5)' }, ${c.start.toFixed(4)});
          tl.fromTo('.card[data-card-id="${c.id}"] #${c.id}-accent', { width: 0 }, { width: ${c.zone === "bottom" ? 300 : 210}, duration: 0.6, ease: 'power2.out' }, ${accentAt.toFixed(4)});
          tl.from('.card[data-card-id="${c.id}"] #${c.id}-emph .char', { opacity: 0, y: 22, scale: 0.88, duration: 0.52, ease: 'power2.out', stagger: 0.055 }, ${textAt.toFixed(4)});
          tl.to(${panel}, { scale: 0.97, duration: 0.4, ease: 'power2.in' }, ${outAt.toFixed(4)});
          tl.to(${host}, { opacity: 0, duration: 0.38, ease: 'power2.in' }, ${outAt.toFixed(4)});
          tl.set(${host}, { visibility: 'hidden' }, ${c.end.toFixed(4)});`;
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
      #stage { position: relative; width: 100%; height: 100%; overflow: hidden; }
      .video-wrapper {
        position: absolute;
        left: 0;
        top: 0;
        width: ${W}px;
        height: ${H}px;
        overflow: hidden;
      }
      .video-wrapper video { width: 100%; height: 100%; object-fit: cover; }
      .card-host { position: absolute; pointer-events: none; overflow: hidden; }
      .card-host .card { position: relative; width: 100%; height: 100%; overflow: hidden; }
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

const counts = cards.reduce((m, c) => ((m[c.zone] = (m[c.zone] || 0) + 1), m), {});
console.log(`cards: ${cards.length}`);
console.log(`zone distribution: ${JSON.stringify(counts)}`);
console.log(`last card ends: ${cards[cards.length - 1].end.toFixed(2)}s of ${DUR}s`);
