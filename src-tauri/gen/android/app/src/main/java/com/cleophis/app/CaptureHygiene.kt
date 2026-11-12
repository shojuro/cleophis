package com.cleophis.app

import android.content.Context
import android.os.Environment

/** The WebView's camera capture writes `JPEG_<stamp>_*.jpg` into the app's
 *  external pictures dir (wry's RustWebChromeClient.createImageFile). Nothing
 *  else reads them after the page has read the file, so they are swept on
 *  activity start and after every transcription. App-private storage: no
 *  other app sees them, but a stale photo of a worksheet is still a leak into
 *  the next backup. Reached from Rust over JNI; kept by proguard-cleophis.pro. */
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
