package com.cleophis.app

import android.content.Intent
import android.os.Bundle
import android.util.Log
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity

/**
 * The launcher entry point, and the only activity that runs before
 * `libcleophis_lib.so` is loaded.
 *
 * ## Why a separate activity rather than a check inside MainActivity
 *
 * Not a style choice — the alternative does not compile into working code.
 * `MainActivity.onCreate` must call `super.onCreate`, and that chain
 * (`TauriActivity` → `WryActivity` → `Rust.create()`) is what triggers
 * `System.loadLibrary("cleophis_lib")` in `Rust`'s static initialiser.
 * Returning early instead throws `SuperNotCalledException`, and calling super
 * first has already loaded the library. There is no point inside MainActivity
 * that is both legal and early enough, so the check has to live in front of it.
 *
 * `Rust.kt` is auto-generated and cannot be edited, which rules out the other
 * obvious hook.
 *
 * ## What it costs when the device is fine
 *
 * One activity that reads a small file, starts MainActivity and finishes, with
 * transitions suppressed so there is nothing to see. MainActivity keeps every
 * attribute it had, including `singleTask`; only the LAUNCHER filter moved.
 */
class LaunchGateActivity : AppCompatActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Logged on every launch, supported or not. A verdict that is only
        // printed when it fires cannot be shown to be capable of not firing,
        // which is the exact property the old `[kernels]` line lacked.
        CpuSupport.describe().forEach { Log.i("Cleophis", it) }

        val missing = CpuSupport.missingFeatures()
        if (missing.isEmpty()) {
            startActivity(Intent(this, MainActivity::class.java))
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
            finish()
            return
        }

        // From here the native library is never loaded in this process.
        setContentView(R.layout.activity_unsupported)
        findViewById<TextView>(R.id.unsupportedWhy)?.text =
            getString(R.string.unsupported_why, missing.joinToString(", "))
    }
}
