// Generates 11 abstract "book cover" webps — seeded per model id, palette per category.
// Run from tools/covers with WSL node: node generate.mjs
import sharp from 'sharp';
import { readFileSync, mkdirSync } from 'node:fs';

const OUT = '../../src-tauri/resources/covers';
const W = 640, H = 800;
const PAL = {
  education: { accent: '#35D0BA', deep: '#06332C' },
  medical:   { accent: '#E8A33D', deep: '#3A2A05' },
};

function seedFrom(str) {
  let h = 2166136261 >>> 0;
  for (const c of str) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619); }
  return h >>> 0;
}
function mulberry32(a) {
  return () => {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function cover(id, category) {
  const rnd = mulberry32(seedFrom(id));
  const { accent, deep } = PAL[category];
  const motif = Math.floor(rnd() * 3); // 0 rings, 1 bands, 2 blocks
  let art = '';
  if (motif === 0) {
    const cx = 120 + rnd() * 400, cy = 160 + rnd() * 420;
    for (let i = 9; i >= 0; i--) {
      const r = 50 + i * (34 + rnd() * 14);
      art += `<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${i % 3 === 0 ? accent : '#2A3340'}" stroke-width="${i % 3 === 0 ? 5 : 2}" opacity="${0.25 + 0.07 * (9 - i)}"/>`;
    }
    art += `<circle cx="${cx}" cy="${cy}" r="${26 + rnd() * 20}" fill="${accent}"/>`;
  } else if (motif === 1) {
    const angle = -18 - rnd() * 20;
    for (let i = 0; i < 12; i++) {
      const y = -120 + i * (70 + rnd() * 24);
      const hgt = 12 + rnd() * 44;
      const col = i % 4 === 0 ? accent : i % 4 === 2 ? deep : '#1E242E';
      art += `<rect x="-200" y="${y}" width="1100" height="${hgt}" fill="${col}" opacity="${0.5 + rnd() * 0.5}" transform="rotate(${angle} 320 400)"/>`;
    }
  } else {
    for (let i = 0; i < 26; i++) {
      const x = rnd() * W, y = rnd() * H, s = 22 + rnd() * 120;
      const col = i % 5 === 0 ? accent : i % 5 === 3 ? deep : '#1E242E';
      const shape = rnd() > 0.5
        ? `<rect x="${x}" y="${y}" width="${s}" height="${s}" rx="${s / 8}" fill="${col}" opacity="${0.35 + rnd() * 0.5}"/>`
        : `<circle cx="${x}" cy="${y}" r="${s / 2}" fill="${col}" opacity="${0.35 + rnd() * 0.5}"/>`;
      art += shape;
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
    <defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#12161d"/><stop offset="1" stop-color="#0E1218"/>
    </linearGradient></defs>
    <rect width="${W}" height="${H}" fill="url(#bg)"/>
    ${art}
    <rect x="0" y="${H - 120}" width="${W}" height="120" fill="#0E1218" opacity="0.55"/>
    <rect x="34" y="${H - 78}" width="110" height="6" rx="3" fill="${accent}"/>
  </svg>`;
}

mkdirSync(OUT, { recursive: true });
const catalog = JSON.parse(readFileSync('../../src-tauri/resources/catalog.json', 'utf8'));
for (const m of catalog) {
  const svg = cover(m.id, m.category);
  await sharp(Buffer.from(svg)).webp({ quality: 82 }).toFile(`${OUT}/${m.id}.webp`);
  console.log('wrote', `${m.id}.webp`);
}
