package com.cleophis.app

import android.os.Build
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Does this CPU implement the instructions the shipped native library was
 * compiled to use?
 *
 * ## Why this is in Kotlin and not in Rust
 *
 * Rust already answers this question far better (`kpack_engine::cpu`, which
 * adjudicates the compile-time macros against `AT_HWCAP` and prints both). But
 * every line of Rust in this app lives inside `libcleophis_lib.so`, and the
 * failure this guards against is a **SIGILL**. If the illegal instruction is in
 * a static initialiser, the process dies inside `System.loadLibrary` and there
 * is no Rust alive to render anything, no webview, no error — the launcher icon
 * simply does nothing. Only a check that runs *before* the library is loaded
 * can turn that into a screen, and on Android that means Kotlin.
 *
 * It is a **safety net, not the fix.** The fix is the arch floor the native
 * library is compiled at. This exists so that getting that wrong costs a
 * message instead of a crash.
 *
 * ## Measured, not hypothetical
 *
 * A Galaxy A51 (SM-A515F, Exynos 9611, Cortex-A73 + A53, ARMv8.0) runs the
 * `armv8.2-a+dotprod` build up to the first inference and then exits 132
 * (128 + 4 = SIGILL). Its `/proc/cpuinfo` Features line has no `asimddp`, no
 * `i8mm` and no `atomics`. Spec §3 names that class of phone — "a 4–6 GB budget
 * phone, Samsung A-series, 2–4 years old" — as the Android floor device, so
 * this is the target market, not an outlier.
 *
 * ## Same source as Rust, deliberately
 *
 * Reads `/proc/self/auxv`, exactly like `kpack_engine::cpu::runtime_cpu`, not
 * `/proc/cpuinfo`. Two readers of one file can be wrong together but cannot
 * quietly disagree, and a disagreement between a gate and a diagnostic is the
 * same defect this whole change exists to remove.
 *
 * R8: reached from the manifest via [LaunchGateActivity], not across JNI, so
 * the shrinker can see it. No keep rule needed.
 */
object CpuSupport {

    /**
     * ⚠ **MUST MATCH `GGML_CPU_ARM_ARCH` in
     * `third_party/llama-cpp-sys-2/build.rs`.**
     *
     * This is the one fact that lives in two languages and cannot be shared:
     * Kotlin runs before any Rust and cannot ask it. Two homes for one fact is
     * exactly the shape that produced the defect being fixed here, so the drift
     * is closed by a mechanism rather than by a comment asking for care —
     * `docs/superpowers/mobile-tools/check-cpu-floor.py` fails the build if
     * these two strings differ. Change one, change the other, or CI stops you.
     */
    const val NATIVE_ARM_ARCH: String = "armv8-a"

    // AArch64 auxv keys and HWCAP bits, from the kernel's
    // `arch/arm64/include/uapi/asm/hwcap.h`. Identical values to
    // `kpack_engine::cpu`'s table.
    private const val AT_HWCAP = 16L
    private const val AT_HWCAP2 = 26L
    private const val HWCAP_ATOMICS = 1L shl 8
    private const val HWCAP_ASIMDHP = 1L shl 10
    private const val HWCAP_ASIMDDP = 1L shl 20
    private const val HWCAP2_I8MM = 1L shl 13

    /** One instruction group the build needs and the CPU may or may not have. */
    data class Requirement(val name: String, val inHwcap2: Boolean, val mask: Long)

    /**
     * The ARMv8 minor level an `-march` string asks for: `armv8-a` → 0,
     * `armv8.2-a+dotprod` → 2, `armv9-a` → 5 (ARMv9.0-A is defined on top of
     * ARMv8.5-A). `-1` means the string could not be parsed.
     */
    fun v8MinorLevel(arch: String): Int {
        val m = Regex("""^armv(\d+)(?:\.(\d+))?-a""").find(arch.trim().lowercase())
            ?: return -1
        val major = m.groupValues[1].toIntOrNull() ?: return -1
        val minor = m.groupValues[2].toIntOrNull() ?: 0
        return when {
            major == 8 -> minor
            major >= 9 -> 5 + minor
            else -> -1
        }
    }

    /**
     * What an `-march` string licenses the C compiler to emit, expressed as
     * HWCAP bits.
     *
     * The `+dotprod` entry is the obvious one. **`lse-atomics` is the one that
     * is easy to miss and would have bitten independently**: the arch *level*
     * alone (anything at or above `armv8.1-a`) makes LSE atomics mandatory, so
     * the compiler may emit `casal`/`ldadd` anywhere in llama.cpp's C sources —
     * not only in the SIMD kernels — and llama.cpp reports no feature flag for
     * it at all. The A51 has neither `asimddp` nor `atomics`, so the current
     * build has two independent ways to kill it.
     *
     * An unparseable string yields **no** requirements rather than all of them.
     * This is a safety net whose default must be non-interference — wrongly
     * blocking a working phone is its own outage — and an unparseable string
     * cannot reach a release anyway, because the drift check refuses to parse
     * it in CI.
     */
    fun requirements(arch: String): List<Requirement> {
        val level = v8MinorLevel(arch)
        if (level < 0) return emptyList()
        val a = arch.lowercase()
        val req = mutableListOf<Requirement>()
        if (level >= 1) req += Requirement("lse-atomics", false, HWCAP_ATOMICS)
        if (a.contains("+fp16")) req += Requirement("asimdhp", false, HWCAP_ASIMDHP)
        if (a.contains("+dotprod")) req += Requirement("asimddp", false, HWCAP_ASIMDDP)
        if (a.contains("+i8mm")) req += Requirement("i8mm", true, HWCAP2_I8MM)
        return req
    }

    /** Requirement names this CPU does not satisfy. Empty means "will run". */
    fun missing(arch: String, hwcap: Long, hwcap2: Long): List<String> =
        requirements(arch)
            .filter { r -> (if (r.inHwcap2) hwcap2 else hwcap) and r.mask == 0L }
            .map { it.name }

    /**
     * `AT_HWCAP` and `AT_HWCAP2` out of a raw `/proc/self/auxv` image, as
     * `[hwcap, hwcap2]`.
     *
     * A flat array of 8-byte `(key, value)` pairs terminated by an `AT_NULL`
     * key. A trailing partial pair is dropped rather than half-read: losing a
     * capability blocks a phone that would have worked, inventing one crashes a
     * phone that would not — and only the first is recoverable by the user
     * reading a screen.
     */
    fun hwcapsFromAuxv(bytes: ByteArray): LongArray {
        val buf = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        var hwcap = 0L
        var hwcap2 = 0L
        while (buf.remaining() >= 16) {
            val key = buf.long
            val value = buf.long
            when (key) {
                0L -> return longArrayOf(hwcap, hwcap2) // AT_NULL terminates
                AT_HWCAP -> hwcap = value
                AT_HWCAP2 -> hwcap2 = value
            }
        }
        return longArrayOf(hwcap, hwcap2)
    }

    /** `null` when the vector cannot be read at all. */
    private fun readHwcaps(): LongArray? = try {
        hwcapsFromAuxv(File("/proc/self/auxv").readBytes())
    } catch (_: Throwable) {
        null
    }

    /** Is this process running the 64-bit ARM library at all? */
    private fun isArm64(): Boolean =
        Build.SUPPORTED_ABIS.firstOrNull()?.startsWith("arm64") == true

    /**
     * What is missing, for the screen and for logcat. Empty means the app may
     * load the native library.
     *
     * Every branch that cannot answer returns "nothing missing". A gate that
     * fails closed on its own uncertainty would block every non-ARM emulator
     * and every device whose `/proc` is unusual, to prevent a crash that has
     * only ever been observed with a specific, detectable capability absent.
     */
    fun missingFeatures(): List<String> {
        if (!isArm64()) return emptyList()
        val caps = readHwcaps() ?: return emptyList()
        return missing(NATIVE_ARM_ARCH, caps[0], caps[1])
    }

    /**
     * One greppable line for logcat, in the mould of the `[kernels]` line and
     * for the same reason: a device report should carry its own evidence rather
     * than a verdict somebody has to trust.
     */
    fun describe(): List<String> {
        val caps = readHwcaps()
        val words = if (caps == null) "unreadable" else
            "hwcap=0x%016x hwcap2=0x%016x".format(caps[0], caps[1])
        val gaps = missingFeatures()
        val verdict = if (gaps.isEmpty()) "OK" else "UNSUPPORTED missing=${gaps.joinToString(",")}"
        return listOf(
            "[cpu-gate] abi=${Build.SUPPORTED_ABIS.firstOrNull()} $words",
            "[cpu-gate] native_arch=$NATIVE_ARM_ARCH " +
                "requires=[${requirements(NATIVE_ARM_ARCH).joinToString(" ") { it.name }}] $verdict",
        )
    }
}
