package com.cleophis.app

import android.app.Activity
import android.content.Context
import android.view.WindowManager

/**
 * The "hide app content in the app switcher" toggle (spec §5.3, task 5.1).
 *
 * Sets or clears `WindowManager.LayoutParams.FLAG_SECURE`, which blanks the
 * app's thumbnail in Recents and blocks screenshots and screen recording of
 * this window.
 *
 * ## Default OFF, and that is a product decision, not an oversight
 *
 * §5.3 is explicit: **screenshots are the user's right.** Someone screenshotting
 * their own study notes, a translated letter, or a reply they want to send to a
 * friend is doing something completely legitimate, and an app that silently
 * forbids it has taken something away without asking. So this ships off, and
 * the user turns it on.
 *
 * The user it exists *for* is the one §5.3 names — the person going through
 * immigration paperwork on a phone they hand to other people — and for them it
 * is one tap. That asymmetry is the whole design: the cost of the default being
 * wrong is a tap for the person who needs it, versus a silently confiscated
 * capability for everyone who does not.
 *
 * ## Reached only from Rust, so R8 cannot see the caller
 *
 * Same JNI-entry-point family as [NativeBridge], [SecureStore], [NetworkPolicy],
 * [ShareSheet] and [InferenceService]; kept by `proguard-cleophis.pro`.
 *
 * ## ⚠ Window flags are UI-thread-only
 *
 * `Window.setFlags` must run on the UI thread. Rust reaches this from whichever
 * thread the command happened to land on — for Tauri commands that is a Tokio
 * pool thread, *not* the UI thread — so this hops via [Activity.runOnUiThread]
 * rather than trusting the caller. Getting that wrong does not fail cleanly: it
 * throws from deep inside the view system with a message about the originating
 * thread, at a point that has nothing to do with privacy settings.
 *
 * The hop also makes the call **asynchronous**, so the Rust side's `Ok` means
 * "the request was posted", not "the flag is set". That is honest for a
 * fire-and-forget preference and is why nothing reads state back through here —
 * the frontend owns the setting; this only applies it.
 */
object ScreenPrivacy {
    /**
     * Apply the flag. `secure = false` clears it, so this is the whole API —
     * one idempotent setter rather than a set/clear pair that can disagree.
     */
    @JvmStatic
    fun apply(context: Context, secure: Boolean) {
        // The bridge hands us the activity as a Context. If it is somehow not
        // an Activity there is no window to flag, and inventing one is not
        // possible — so this reports by doing nothing rather than crashing a
        // preference toggle.
        val activity = context as? Activity ?: return
        activity.runOnUiThread {
            if (secure) {
                activity.window.setFlags(
                    WindowManager.LayoutParams.FLAG_SECURE,
                    WindowManager.LayoutParams.FLAG_SECURE,
                )
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
            }
        }
    }
}
