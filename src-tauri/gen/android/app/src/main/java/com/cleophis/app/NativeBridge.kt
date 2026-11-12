package com.cleophis.app

import android.os.Build

/**
 * The Kotlin end of the JNI bridge (see `src-tauri/src/android_bridge.rs`).
 *
 * This exists to be called from Rust and to prove that path works before any
 * feature rides on it. `ConnectivityManager` (download network policy),
 * `ACTION_SEND` (share-sheet export) and, if the pack-upload feature lands,
 * SAF/content-URIs all need Rust to reach our own Kotlin; each of them would
 * otherwise be debugging the bridge and the feature at the same time.
 *
 * `object` + `@JvmStatic` is load-bearing rather than idiomatic preference: it
 * emits a real static method on the class `com.cleophis.app.NativeBridge`, so
 * the Rust side can use `CallStaticObjectMethod` without first constructing an
 * instance. A plain Kotlin `object` member without `@JvmStatic` would compile
 * to an instance method on the `INSTANCE` singleton, and the JNI lookup would
 * fail with a NoSuchMethodError that says nothing about why.
 *
 * Keep this class free of app state. It is a shim surface reached from a
 * JNI-attached thread, and anything it touches inherits that threading
 * context.
 */
object NativeBridge {
    /**
     * Returns a compact device identity string, e.g. `samsung/SM-A226B/api33`.
     *
     * Reads `android.os.Build` rather than returning a constant on purpose: a
     * constant would prove the call mechanism and nothing more, while this
     * proves the Kotlin ran with genuine framework access — which is the thing
     * every queued shim actually depends on. It is also predictable in advance
     * for a known device, so the smoke test has an expected value to be
     * checked against rather than merely a non-empty string.
     *
     * Device/OS identifiers only: no user content, nothing personal beyond the
     * hardware model, so it is safe to print in a release log (security
     * review M3).
     */
    @JvmStatic
    fun describeDevice(): String =
        "${Build.MANUFACTURER}/${Build.MODEL}/api${Build.VERSION.SDK_INT}"
}
