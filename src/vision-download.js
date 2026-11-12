// Is photo input usable on this device, and what would enabling it download?
// Pure: app.js passes in the bundled-catalog variant and `vision_state()`.

import { pickVisionArtifacts } from './dist-pick.js';
import { formatBytes } from './download-policy.js';

export const VISION_DOWNLOAD_PROMPT = (bytes) => `Photo input needs a one-time ${formatBytes(bytes)} download.`;

/**
 * 'unavailable'    no pinned transcriber for this tier, or `vision_state()` is
 *                  null (desktop, or neither declared nor installed): the
 *                  photo controls stay hidden rather than promise a feature.
 * 'needs-download' declared by the build but not on disk yet.
 * 'ready'          installed.
 */
export function visionAvailability({ variant, visionState }) {
  if (!variant || !visionState) return 'unavailable';
  if (visionState.installed) return 'ready';
  if (visionState.declared) return 'needs-download';
  return 'unavailable';
}

/** Both files the download needs, plus the combined size for the consent prompt. */
export function visionDownloadPlan(artifacts, variant) {
  // Throws pickVisionArtifacts's readable reasons; a partial plan would
  // download a base the projector can't pair with.
  const { base, mmproj } = pickVisionArtifacts(artifacts, variant);
  return { base, mmproj, totalBytes: (Number(base.size) || 0) + (Number(mmproj.size) || 0) };
}
