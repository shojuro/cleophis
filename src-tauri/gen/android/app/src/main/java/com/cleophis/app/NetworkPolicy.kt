package com.cleophis.app

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.os.BatteryManager

/**
 * The facts Android knows about the connection and the battery, for the
 * download policy (spec §2.2: unmetered-only by default, per-download
 * override, charge-recommended notice).
 *
 * Reached from Rust through the bridge (`src-tauri/src/android_bridge.rs`).
 * Like `SecureStore`, this reports facts and decides nothing — the policy
 * itself is `src/download-policy.js`, where it is tested.
 *
 * ## Why one string instead of three booleans
 *
 * Every JNI method is a signature that can be wrong, and a wrong one fails
 * with a `NoSuchMethodError` that names the signature rather than the cause.
 * Three accessors would be three chances at that, three R8 keep-rule
 * surfaces, and three round trips for facts that must describe the *same
 * instant* — a connection that changes between two calls yields a report that
 * was never simultaneously true. One call returns one consistent snapshot,
 * and the parse becomes a pure Rust function with tests, which is where this
 * project puts anything that can be silently wrong.
 *
 * Format is deliberately dull and greppable — `key=value` pairs, space
 * separated, unknown keys ignored by the reader, so a field can be added
 * without breaking an older parser:
 *
 *     metered=1 charging=0 battery=57
 *
 * `metered=1` means the active connection is metered. Unknown or unavailable
 * values are omitted rather than guessed; the Rust side then falls back to the
 * SAFE assumption (metered), because guessing unmetered spends the user's
 * money and guessing metered costs one prompt.
 *
 * ## R8
 *
 * Reached only from Rust across JNI, so it is dead code to the shrinker.
 * `app/proguard-cleophis.pro` keeps it; release-only failure otherwise.
 */
object NetworkPolicy {
    /**
     * `isActiveNetworkMetered` is the ONLY correct source for this question.
     *
     * The tempting alternatives are both wrong in the case that matters. A
     * transport check (`TRANSPORT_WIFI`) reports Wi-Fi-vs-cellular, and a
     * **metered Wi-Fi hotspot** — a phone tethering another phone, a hotel
     * plan, a capped home connection the user has flagged — is exactly the
     * case that distinction gets backwards. `navigator.connection` in the
     * WebView has the same flaw and is why no part of this policy is built on
     * it. The whole point of the feature is respecting someone's data plan,
     * and a false negative there costs them money.
     *
     * Requires `ACCESS_NETWORK_STATE`, which lands in the manifest with this
     * caller and not before.
     */
    @JvmStatic
    fun describe(context: Context): String {
        val parts = mutableListOf<String>()

        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        if (cm != null) {
            parts += "metered=" + if (cm.isActiveNetworkMetered) "1" else "0"
        }

        // Battery via the sticky ACTION_BATTERY_CHANGED broadcast: no
        // permission, no receiver to unregister, and a null return (no sticky
        // intent yet) simply omits the fields rather than inventing them.
        val status = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        if (status != null) {
            val plugged = status.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
            if (plugged != -1) {
                val charging = plugged == BatteryManager.BATTERY_STATUS_CHARGING ||
                    plugged == BatteryManager.BATTERY_STATUS_FULL
                parts += "charging=" + if (charging) "1" else "0"
            }
            val level = status.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
            val scale = status.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
            if (level >= 0 && scale > 0) {
                parts += "battery=" + (level * 100 / scale)
            }
        }

        return parts.joinToString(" ")
    }
}
