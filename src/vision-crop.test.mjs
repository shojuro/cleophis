import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  MAX_SIDE, MIN_CROP_PX, fitContain, defaultCrop, clampCrop, moveCrop, resizeCrop, cropToImagePixels, targetSize,
} from './vision-crop.js';

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} !~ ${b}`);
const inside = (r, f) => {
  assert.ok(r.x >= f.x - 1e-9 && r.y >= f.y - 1e-9, 'top-left inside');
  assert.ok(r.x + r.w <= f.x + f.w + 1e-9 && r.y + r.h <= f.y + f.h + 1e-9, 'bottom-right inside');
};

const FIT = fitContain(4000, 3000, 360, 640);

test('constants', () => {
  assert.equal(MAX_SIDE, 512);
  assert.equal(MIN_CROP_PX, 48);
});

test('fitContain scales to the limiting axis and centres the other', () => {
  near(FIT.scale, 0.09);
  near(FIT.w, 360);
  near(FIT.h, 270);
  near(FIT.x, 0);
  near(FIT.y, (640 - 270) / 2);
  assert.equal(FIT.imgW, 4000);
  assert.equal(FIT.imgH, 3000);
});

test('fitContain letterboxes a tall image horizontally', () => {
  const f = fitContain(1000, 4000, 400, 400);
  near(f.scale, 0.1);
  near(f.w, 100);
  near(f.x, 150);
  near(f.y, 0);
});

test('defaultCrop is centred, 88% wide, 34% tall and inside the drawn image', () => {
  const r = defaultCrop(FIT);
  near(r.w, FIT.w * 0.88);
  near(r.h, FIT.h * 0.34);
  near(r.x + r.w / 2, FIT.x + FIT.w / 2);
  near(r.y + r.h / 2, FIT.y + FIT.h / 2);
  inside(r, FIT);
  assert.ok(r.w >= MIN_CROP_PX && r.h >= MIN_CROP_PX);
});

test('defaultCrop on a tiny image is still clamped inside', () => {
  const f = fitContain(100, 40, 100, 40);
  const r = defaultCrop(f);
  inside(r, f);
});

test('clampCrop pushes an out-of-bounds rect back inside', () => {
  const r = clampCrop({ x: -50, y: FIT.y - 30, w: 100, h: 100 }, FIT);
  inside(r, FIT);
  near(r.x, FIT.x);
  near(r.y, FIT.y);
  const r2 = clampCrop({ x: 9999, y: 9999, w: 100, h: 100 }, FIT);
  inside(r2, FIT);
  near(r2.x + r2.w, FIT.x + FIT.w);
  near(r2.y + r2.h, FIT.y + FIT.h);
});

test('clampCrop enforces the minimum edge', () => {
  const r = clampCrop({ x: 100, y: 200, w: 5, h: 7 }, FIT);
  assert.equal(r.w, MIN_CROP_PX);
  assert.equal(r.h, MIN_CROP_PX);
  inside(r, FIT);
});

test('clampCrop shrinks a rect larger than the drawn image', () => {
  const r = clampCrop({ x: -10, y: -10, w: 9999, h: 9999 }, FIT);
  near(r.w, FIT.w);
  near(r.h, FIT.h);
  near(r.x, FIT.x);
  near(r.y, FIT.y);
});

test('clampCrop uses the drawn image size as the floor when the image is smaller than the minimum', () => {
  const f = fitContain(30, 20, 30, 20);
  const r = clampCrop({ x: 0, y: 0, w: 10, h: 10 }, f);
  assert.equal(r.w, 30);
  assert.equal(r.h, 20);
  inside(r, f);
});

test('moveCrop translates then clamps', () => {
  const start = { x: 50, y: 250, w: 100, h: 80 };
  const m = moveCrop(start, 20, 10, FIT);
  assert.deepEqual(m, { x: 70, y: 260, w: 100, h: 80 });
  const out = moveCrop(start, 1000, 1000, FIT);
  inside(out, FIT);
  assert.equal(out.w, 100);
  assert.equal(out.h, 80);
});

const R = { x: 100, y: 250, w: 120, h: 100 };

test('resizeCrop se keeps the top-left fixed', () => {
  const r = resizeCrop(R, 'se', 30, 20, FIT);
  assert.deepEqual(r, { x: 100, y: 250, w: 150, h: 120 });
});

test('resizeCrop nw keeps the bottom-right fixed', () => {
  const r = resizeCrop(R, 'nw', -20, -10, FIT);
  assert.equal(r.x + r.w, R.x + R.w);
  assert.equal(r.y + r.h, R.y + R.h);
  assert.equal(r.x, 80);
  assert.equal(r.y, 240);
});

test('resizeCrop ne keeps the bottom-left fixed, sw keeps the top-right fixed', () => {
  const ne = resizeCrop(R, 'ne', 10, -10, FIT);
  assert.equal(ne.x, R.x);
  assert.equal(ne.y + ne.h, R.y + R.h);
  assert.equal(ne.x + ne.w, R.x + R.w + 10);
  const sw = resizeCrop(R, 'sw', -10, 10, FIT);
  assert.equal(sw.x + sw.w, R.x + R.w);
  assert.equal(sw.y, R.y);
  assert.equal(sw.x, R.x - 10);
});

test('edge handles move only their own edge', () => {
  assert.deepEqual(resizeCrop(R, 'e', 20, 99, FIT), { x: 100, y: 250, w: 140, h: 100 });
  assert.deepEqual(resizeCrop(R, 'w', -20, 99, FIT), { x: 80, y: 250, w: 140, h: 100 });
  assert.deepEqual(resizeCrop(R, 's', 99, 20, FIT), { x: 100, y: 250, w: 120, h: 120 });
  assert.deepEqual(resizeCrop(R, 'n', 99, -20, FIT), { x: 100, y: 230, w: 120, h: 120 });
});

test('resizeCrop cannot shrink below the minimum nor leave the image', () => {
  const s = resizeCrop(R, 'se', -500, -500, FIT);
  assert.equal(s.w, MIN_CROP_PX);
  assert.equal(s.h, MIN_CROP_PX);
  assert.equal(s.x, R.x);
  assert.equal(s.y, R.y);
  const g = resizeCrop(R, 'se', 5000, 5000, FIT);
  inside(g, FIT);
  assert.equal(g.x, R.x);
});

test('cropToImagePixels inverts fitContain within one pixel', () => {
  const rect = { x: 36, y: FIT.y + 27, w: 180, h: 90 };
  const p = cropToImagePixels(rect, FIT);
  assert.ok(Number.isInteger(p.sx) && Number.isInteger(p.sy) && Number.isInteger(p.sw) && Number.isInteger(p.sh));
  assert.ok(Math.abs(p.sx * FIT.scale + FIT.x - rect.x) <= 1);
  assert.ok(Math.abs(p.sy * FIT.scale + FIT.y - rect.y) <= 1);
  assert.ok(Math.abs(p.sw * FIT.scale - rect.w) <= 1);
  assert.ok(Math.abs(p.sh * FIT.scale - rect.h) <= 1);
  assert.deepEqual(p, { sx: 400, sy: 300, sw: 2000, sh: 1000 });
});

test('cropToImagePixels clamps to the source image and never returns an empty crop', () => {
  const full = cropToImagePixels({ x: FIT.x, y: FIT.y, w: FIT.w, h: FIT.h }, FIT);
  assert.deepEqual(full, { sx: 0, sy: 0, sw: 4000, sh: 3000 });
  const over = cropToImagePixels({ x: FIT.x - 20, y: FIT.y + FIT.h - 5, w: FIT.w + 40, h: 50 }, FIT);
  assert.ok(over.sx >= 0 && over.sy >= 0);
  assert.ok(over.sx + over.sw <= 4000 && over.sy + over.sh <= 3000);
  const zero = cropToImagePixels({ x: FIT.x, y: FIT.y, w: 0, h: 0 }, FIT);
  assert.ok(zero.sw >= 1 && zero.sh >= 1);
});

test('targetSize downscales the longest edge to the cap', () => {
  assert.deepEqual(targetSize(2000, 700), { w: 512, h: 179 });
  assert.deepEqual(targetSize(700, 2000), { w: 179, h: 512 });
});

test('targetSize never upscales and never returns less than 1', () => {
  assert.deepEqual(targetSize(300, 100), { w: 300, h: 100 });
  assert.deepEqual(targetSize(512, 512), { w: 512, h: 512 });
  assert.deepEqual(targetSize(10000, 1), { w: 512, h: 1 });
  assert.deepEqual(targetSize(2000, 700, 1000), { w: 1000, h: 350 });
});
