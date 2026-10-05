package com.cleophis.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * On-device inference foreground service (spec §8, hazards H4/H5).
 *
 * Declared in Phase 0.3, given its real lifecycle here in 5.1: started when a
 * turn begins, stopped when it ends, so Android does not kill token generation
 * mid-reply under memory pressure. Without this the failure is H5 in its ugly
 * form -- the process dies partway through a turn and the transcript is left
 * holding half an answer.
 *
 * ## Why `specialUse`, stated once so Play review and the next reader get the
 * same answer
 *
 * Android 14+ requires a declared foreground-service *type*, and none of the
 * catalogued types describes local text generation: it is not `dataSync` (no
 * network -- see below), not `mediaPlayback`, not `location`. `specialUse` is
 * the residual category and the justification string lives in the manifest
 * (`PROPERTY_SPECIAL_USE_FGS_SUBTYPE`): *"local AI text generation at explicit
 * user request; no network"*. Both halves of that are load-bearing and both are
 * true -- generation is always user-initiated, and this service performs no
 * network I/O whatsoever. Downloads are a separate concern with a separate
 * policy (§2.2), and conflating them is exactly what would make the
 * justification false.
 *
 * ## Reached only from Rust, so R8 cannot see the callers
 *
 * Like [NativeBridge], [SecureStore], [NetworkPolicy] and [ShareSheet], the
 * companion's statics are invoked across JNI from
 * `src-tauri/src/android_bridge.rs`. R8 shrinks on reachability from
 * Java/Kotlin and sees dead code, so `proguard-cleophis.pro` keeps this class.
 * Debug builds do not minify, which means an on-device debug checkpoint proves
 * nothing about release -- the standing trap on this branch.
 *
 * ## The two Android rules that bite here
 *
 * 1. **`startForeground` must be called within ~5 seconds** of
 *    `startForegroundService`, or the system throws
 *    `ForegroundServiceDidNotStartInTimeException`. So [onStartCommand] builds
 *    the notification and promotes immediately, with no work in between.
 * 2. **A foreground service cannot be started from the background** on Android
 *    12+. Safe here by construction: the only caller is a chat turn, which is
 *    user-initiated with the app visible. Worth stating because a future
 *    caller -- a scheduled or background-triggered generation -- would violate
 *    it, and the exception names the restriction rather than the design
 *    mistake.
 *
 * ## POST_NOTIFICATIONS may be denied, and that is survivable
 *
 * On Android 13+ the user can refuse notifications. The service still runs and
 * still protects the turn; only the notification is not displayed. That is the
 * right trade -- the point is keeping generation alive, and the notification is
 * how the platform makes that visible, not why it works. So nothing here treats
 * a denied permission as a failure.
 */
class InferenceService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        ensureChannel(this)
        val notification = buildNotification(this)

        // The type argument is required from API 34 and must match the
        // manifest's `foregroundServiceType`, or the promotion throws. Below 34
        // the manifest attribute is ignored and the two-argument form is the
        // correct call, not a fallback.
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        // START_NOT_STICKY: if the process dies mid-turn there is nothing to
        // resume -- the engine state died with it, and the partial turn is
        // recovered from the conversation store on next launch (1.4's flush),
        // not by restarting a service into an empty engine. START_STICKY would
        // relaunch this with a null intent and a foreground notification for
        // generation that is not happening.
        return START_NOT_STICKY
    }

    companion object {
        private const val CHANNEL_ID = "cleophis.inference"

        /** Stable, so a second `start` updates the existing notification. */
        private const val NOTIFICATION_ID = 1001

        /**
         * Begin protecting a turn. Idempotent -- a second call re-promotes with
         * the same id and simply refreshes the notification.
         */
        @JvmStatic
        fun start(context: Context) {
            val intent = Intent(context, InferenceService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        /**
         * The turn ended. Stopping the service removes the notification, so
         * there is no separate `stopForeground` call to get wrong.
         *
         * Harmless if the service is not running.
         */
        @JvmStatic
        fun stop(context: Context) {
            context.stopService(Intent(context, InferenceService::class.java))
        }

        private fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            // IMPORTANCE_LOW: visible in the shade, silent, never a heads-up
            // banner. A sound or a pop-up for every single reply would make the
            // feature that keeps generation alive the most irritating thing in
            // the app.
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Writing replies",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description =
                    "Shown while Cleophis is generating a reply on this device, " +
                    "so Android does not stop it partway through."
                setShowBadge(false)
            }
            manager.createNotificationChannel(channel)
        }

        private fun buildNotification(context: Context): Notification {
            // Tapping returns to the conversation rather than launching a
            // second task. FLAG_IMMUTABLE is mandatory from API 31 and correct
            // everywhere: nothing needs to rewrite this intent.
            val open = context.packageManager
                .getLaunchIntentForPackage(context.packageName)
                ?.let {
                    PendingIntent.getActivity(
                        context,
                        0,
                        it,
                        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    )
                }

            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, CHANNEL_ID)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(context)
            }

            return builder
                .setContentTitle("Writing a reply")
                // Says the true and reassuring thing in one line. "On this
                // device" is the product's whole claim, and the status bar is
                // one of the few places the user sees it unprompted.
                .setContentText("Running on this device. No network.")
                .setSmallIcon(R.drawable.ic_stat_cleophis)
                .setOngoing(true)
                .setContentIntent(open)
                .build()
        }
    }
}
