package com.cleophis.app

import android.content.Context
import android.os.Environment

/** The WebView's camera capture writes `JPEG_<stamp>_*.jpg` into the app's
 *  external pictures dir (wry's RustWebChromeClient.createImageFile). That is
 *  app-scoped EXTERNAL storage, not private storage: on Android 9 and lower any
 *  app holding READ_EXTERNAL_STORAGE can read it, and on Android 10 so can an
 *  app running with legacy storage; from 11 only USB/MTP or root reaches it.
 *  Nothing reads a capture after the page has decoded it, so they are swept on
 *  activity start, after every transcription, and (the `sweep_captures`
 *  command) as soon as the page has decoded the capture or abandons the photo
 *  flow (Retake, close, a decode failure). Backups exclude the folder
 *  (allowBackup is false). Reached from Rust over JNI; kept by
 *  proguard-cleophis.pro. */
object CaptureHygiene {
    @JvmStatic
    fun sweep(context: Context): Int {
        val dir = context.getExternalFilesDir(Environment.DIRECTORY_PICTURES) ?: return 0
        var n = 0
        dir.listFiles()?.forEach { f ->
            if (f.isFile && f.name.startsWith("JPEG_") && f.name.endsWith(".jpg") && f.delete()) n++
        }
        return n
    }
}
