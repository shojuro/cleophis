package com.cleophis.app

import android.app.Service
import android.content.Intent
import android.os.IBinder

/**
 * On-device inference foreground service (spec §8, hazards H4/H5).
 *
 * DECLARED in Phase 0.3 (manifest hardening) as an Android 14+ `specialUse`
 * foreground service so the OS does not kill token generation mid-turn under
 * memory pressure. This is a minimal stub: the real lifecycle — `startForeground`
 * with a notification, started/stopped around `chat_stream`, and the
 * thermal-throttle notice — is implemented in Phase 5.1. The service performs
 * NO network I/O (see the justification string in AndroidManifest.xml).
 */
class InferenceService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null
}
