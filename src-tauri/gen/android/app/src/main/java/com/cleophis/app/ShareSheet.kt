package com.cleophis.app

import android.content.Context
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File

/**
 * Android's share sheet (`ACTION_SEND`) for chat export.
 *
 * Desktop exports through `dialog.save` — an OS file picker, which is the
 * wrong shape on a phone, where "put this somewhere" is a share target
 * (Drive, Keep, mail, another chat app) rather than a filesystem path. Rust
 * has already written the bytes; this only supplies the destination.
 *
 * Reached from Rust through the bridge (`src-tauri/src/mobile_native.rs`).
 *
 * ## R8
 *
 * Reached only from Rust across JNI, so it is dead code to the shrinker.
 * `app/proguard-cleophis.pro` keeps it. Release-only failure otherwise, and
 * for this class the symptom would be a share button that does nothing.
 */
object ShareSheet {
    /**
     * Share `absPath` as `mime`, titled `title` in the chooser.
     *
     * Three details are load-bearing and each is a way this fails silently or
     * with a misleading error:
     *
     * 1. **`FileProvider.getUriForFile`, never `Uri.fromFile`.** A `file://`
     *    URI thrown at another app raises `FileUriExposedException` on every
     *    Android we target (N+), and our `minSdk` is 24. The authority must
     *    match the manifest's `${applicationId}.fileprovider`, and the path
     *    must fall under a root declared in `res/xml/file_paths.xml` — the
     *    cache-path root already covers the `exports/` directory Rust writes
     *    to, which is why this needed no new provider wiring.
     * 2. **`FLAG_GRANT_READ_URI_PERMISSION` on the intent that is actually
     *    launched.** The grant rides on the Intent the system delivers, so it
     *    goes on the chooser too — setting it only on the inner send intent
     *    is a classic way to hand a target a URI it cannot open, and the
     *    failure surfaces inside the *other* app as a permission denial with
     *    nothing pointing back here.
     * 3. **`FLAG_ACTIVITY_NEW_TASK`.** This is called from a JNI-attached
     *    thread rather than from an Activity's own call stack, and starting an
     *    activity without a task raises `AndroidRuntimeException`.
     *
     * Returns `Unit`; any failure throws, and the Rust side reports it after
     * `exception_describe` has put the Java stack trace in logcat.
     */
    @JvmStatic
    fun shareFile(context: Context, absPath: String, mime: String, title: String) {
        val file = File(absPath)
        val uri = FileProvider.getUriForFile(
            context,
            "${context.packageName}.fileprovider",
            file
        )

        val send = Intent(Intent.ACTION_SEND).apply {
            type = mime
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_TITLE, title)
            // Subject is what mail targets use; without it the export arrives
            // titled "(no subject)".
            putExtra(Intent.EXTRA_SUBJECT, title)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }

        val chooser = Intent.createChooser(send, title).apply {
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }

        context.startActivity(chooser)
    }
}
