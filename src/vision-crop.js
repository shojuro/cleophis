// Crop geometry for the photo -> crop -> read flow (Stage 2).
//
// Pure (no DOM, no invoke) so `node --test` covers it; app.js feeds it pointer
// deltas and the canvas box. Everything below the `cropToImagePixels` line is in
// DISPLAY pixels (the drawn image inside the box); that function is the one
// place display space becomes SOURCE-image space.

// The transcriber was measured at a 512px longest edge. Feeding it larger
// crops costs decode time without improving the read, and smaller-than-source
// crops are never upscaled (that only invents pixels).
export const MAX_SIDE = 512;
// A crop edge below this is too small to hold a legible line on a phone and too
// small for a thumb to grab, so the handles stop here instead of collapsing.
export const MIN_CROP_PX = 48;

/** The image drawn to fit (contain) a box, centred. */
export function fitContain(imgW, imgH, boxW, boxH) {
  const scale = Math.min(boxW / imgW, boxH / imgH);
  const w = imgW * scale;
  const h = imgH * scale;
  // imgW/imgH ride along so cropToImagePixels can clamp to the real image.
  return { scale, x: (boxW - w) / 2, y: (boxH - h) / 2, w, h, imgW, imgH };
}

// Wide and short: a worked problem is typically one or two lines across the
// page, so this lands on the common case and the user mostly nudges.
export function defaultCrop(fit) {
  const w = fit.w * 0.88;
  const h = fit.h * 0.34;
  return clampCrop({ x: fit.x + (fit.w - w) / 2, y: fit.y + (fit.h - h) / 2, w, h }, fit);
}

export function clampCrop(rect, fit) {
  // When the drawn image is itself smaller than MIN_CROP_PX the image is the floor,
  // otherwise the rect could never fit and translation would push it off-image.
  const w = Math.max(Math.min(rect.w, fit.w), Math.min(MIN_CROP_PX, fit.w));
  const h = Math.max(Math.min(rect.h, fit.h), Math.min(MIN_CROP_PX, fit.h));
  const x = Math.min(Math.max(rect.x, fit.x), fit.x + fit.w - w);
  const y = Math.min(Math.max(rect.y, fit.y), fit.y + fit.h - h);
  return { x, y, w, h };
}

export function moveCrop(rect, dx, dy, fit) {
  return clampCrop({ ...rect, x: rect.x + dx, y: rect.y + dy }, fit);
}

export function resizeCrop(rect, handle, dx, dy, fit) {
  let left = rect.x;
  let top = rect.y;
  let right = rect.x + rect.w;
  let bottom = rect.y + rect.h;
  // Each moving edge is limited by the image and by the fixed opposite edge,
  // so a drag past the image or the minimum stops that edge rather than
  // sliding the whole rect (which would move the edge that must stay fixed).
  if (handle.includes('w')) left = Math.max(fit.x, Math.min(left + dx, right - MIN_CROP_PX));
  if (handle.includes('e')) right = Math.min(fit.x + fit.w, Math.max(right + dx, left + MIN_CROP_PX));
  if (handle.includes('n')) top = Math.max(fit.y, Math.min(top + dy, bottom - MIN_CROP_PX));
  if (handle.includes('s')) bottom = Math.min(fit.y + fit.h, Math.max(bottom + dy, top + MIN_CROP_PX));
  return clampCrop({ x: left, y: top, w: right - left, h: bottom - top }, fit);
}

/** Display-space rect -> integer source-image pixels, always a non-empty in-image box. */
export function cropToImagePixels(rect, fit) {
  const sx = Math.min(Math.max(Math.round((rect.x - fit.x) / fit.scale), 0), fit.imgW - 1);
  const sy = Math.min(Math.max(Math.round((rect.y - fit.y) / fit.scale), 0), fit.imgH - 1);
  const sw = Math.min(Math.max(Math.round(rect.w / fit.scale), 1), fit.imgW - sx);
  const sh = Math.min(Math.max(Math.round(rect.h / fit.scale), 1), fit.imgH - sy);
  return { sx, sy, sw, sh };
}

export function targetSize(sw, sh, maxSide = MAX_SIDE) {
  const scale = Math.min(1, maxSide / Math.max(sw, sh));
  return { w: Math.max(1, Math.round(sw * scale)), h: Math.max(1, Math.round(sh * scale)) };
}
