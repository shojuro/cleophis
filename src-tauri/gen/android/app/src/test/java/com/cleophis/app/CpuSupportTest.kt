package com.cleophis.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pre-load CPU gate's arithmetic, on the JVM.
 *
 * Only the pure half is exercised — [CpuSupport.missingFeatures] reads
 * `/proc/self/auxv` and `android.os.Build`, neither of which exists here. That
 * half is verified on hardware instead, which is the stronger evidence anyway:
 * a device either shows the screen or it does not.
 *
 * The cases below are chosen so that the gate is demonstrated **capable of both
 * verdicts**. A gate that only ever passes and a gate that only ever blocks are
 * both useless, and the first is exactly the defect that produced this work —
 * a `DOTPROD = 1` line that could not print `0`.
 *
 * `junit:junit:4.13.2` was already a `testImplementation` dependency with no
 * test source set to use it. Run: `./gradlew :app:testUniversalDebugUnitTest`.
 */
class CpuSupportTest {

    /**
     * Galaxy A51, SM-A515F, Exynos 9611 (Cortex-A73 + A53). `AT_HWCAP` read off
     * the device on 2026-07-31: `0x8ff` — fp, asimd, evtstrm, aes, pmull, sha1,
     * sha2, crc32, cpuid, and nothing above bit 11.
     */
    private val a51Hwcap = 0x8ffL

    /** A constructed ARMv8.2 CPU: LSE atomics, fp16 and dotprod, no i8mm. */
    private val v82Hwcap = (1L shl 8) or (1L shl 10) or (1L shl 20) or 0x3L

    // ── the arch string ──────────────────────────────────────────────────────

    @Test
    fun archLevelsAreReadOffTheMarchString() {
        assertEquals(0, CpuSupport.v8MinorLevel("armv8-a"))
        assertEquals(2, CpuSupport.v8MinorLevel("armv8.2-a+dotprod"))
        assertEquals(6, CpuSupport.v8MinorLevel("armv8.6-a+i8mm"))
        // ARMv9.0-A is defined on top of ARMv8.5-A, so it is at least as strict.
        assertEquals(5, CpuSupport.v8MinorLevel("armv9-a"))
        assertEquals(7, CpuSupport.v8MinorLevel("armv9.2-a"))
    }

    @Test
    fun anUnparseableArchYieldsNoRequirements() {
        // The gate's default must be non-interference: wrongly blocking a
        // working phone is its own outage. `check-cpu-floor.py` is what stops
        // an unparseable floor from ever reaching a release, and it is tested
        // against that case separately.
        assertEquals(-1, CpuSupport.v8MinorLevel("nonsense"))
        assertTrue(CpuSupport.requirements("nonsense").isEmpty())
        assertTrue(CpuSupport.missing("nonsense", 0L, 0L).isEmpty())
    }

    // ── what an arch floor demands ───────────────────────────────────────────

    @Test
    fun theBaselineFloorDemandsNothing() {
        // The whole appeal of a universal binary: on `armv8-a` the gate is
        // inert, so it can ship without excluding anybody.
        assertTrue(CpuSupport.requirements("armv8-a").isEmpty())
    }

    @Test
    fun theDotprodFloorDemandsLseAtomicsAsWellAsDotprod() {
        // The one that is easy to miss. `+dotprod` is visible in the string;
        // ARMv8.1 LSE atomics are implied by the LEVEL and llama.cpp reports no
        // flag for them at all — so a check built only around DOTPROD would
        // have called the A51 supported for the second-worst reason.
        val names = CpuSupport.requirements("armv8.2-a+dotprod").map { it.name }
        assertEquals(listOf("lse-atomics", "asimddp"), names)
    }

    @Test
    fun i8mmIsReadFromTheSecondCapabilityWord() {
        // A wrong word here is silent: hwcap bit 13 is JSCVT, which a modern
        // CPU has, so reading i8mm out of the wrong word would report it
        // present on hardware that lacks it — a false pass, the bad direction.
        val i8mm = CpuSupport.requirements("armv8.6-a+i8mm").single { it.name == "i8mm" }
        assertTrue(i8mm.inHwcap2)
        assertEquals(1L shl 13, i8mm.mask)
    }

    // ── the verdict, both directions ─────────────────────────────────────────

    @Test
    fun theA51IsBlockedByTheDotprodBuildForBothReasons() {
        assertEquals(
            listOf("lse-atomics", "asimddp"),
            CpuSupport.missing("armv8.2-a+dotprod", a51Hwcap, 0L),
        )
    }

    @Test
    fun theA51RunsTheBaselineBuild() {
        // The positive control on the failing device: same CPU, different
        // floor, gate silent. Without this, "blocked" could be an instrument
        // that blocks everything.
        assertTrue(CpuSupport.missing("armv8-a", a51Hwcap, 0L).isEmpty())
    }

    @Test
    fun aCapableCpuIsNotBlockedByTheDotprodBuild() {
        assertTrue(CpuSupport.missing("armv8.2-a+dotprod", v82Hwcap, 0L).isEmpty())
        // ...but it IS blocked by a floor it genuinely cannot meet.
        assertEquals(
            listOf("i8mm"),
            CpuSupport.missing("armv8.6-a+i8mm", v82Hwcap, 0L),
        )
    }

    // ── the auxv parse ───────────────────────────────────────────────────────

    @Test
    fun hwcapsAreReadOutOfAnAuxvImage() {
        val caps = CpuSupport.hwcapsFromAuxv(
            auxv(6L to 4096L, 16L to a51Hwcap, 26L to 7L, 0L to 0L)
        )
        assertEquals(a51Hwcap, caps[0])
        assertEquals(7L, caps[1])
    }

    @Test
    fun auxvParsingStopsAtTheTerminatorAndToleratesAShortRead() {
        // Past AT_NULL is not data.
        val stopped = CpuSupport.hwcapsFromAuxv(auxv(0L to 0L, 16L to a51Hwcap))
        assertEquals(0L, stopped[0])

        // A truncated final pair is dropped, never half-read: losing a
        // capability blocks a phone that would have worked, inventing one
        // crashes a phone that would not.
        val short = auxv(16L to a51Hwcap).copyOfRange(0, 12)
        assertEquals(0L, CpuSupport.hwcapsFromAuxv(short)[0])
        assertEquals(0L, CpuSupport.hwcapsFromAuxv(ByteArray(0))[0])
    }

    @Test
    fun theKotlinAndRustReadersAgreeOnTheMeasuredDeviceWord() {
        // `kpack_engine::cpu` parses the same file and its own test pins the
        // same 0x8ff. Two readers of one file in two languages is a real cost;
        // this is the assertion that they cannot quietly diverge on the one
        // device where the answer has actually been measured.
        val caps = CpuSupport.hwcapsFromAuxv(auxv(16L to 0x8ffL, 0L to 0L))
        assertEquals(0x8ffL, caps[0])
        assertTrue(CpuSupport.missing("armv8.2-a+dotprod", caps[0], caps[1]).isNotEmpty())
        assertTrue(CpuSupport.missing("armv8-a", caps[0], caps[1]).isEmpty())
    }

    /** Little-endian `(key, value)` pairs, the shape of `/proc/self/auxv`. */
    private fun auxv(vararg pairs: Pair<Long, Long>): ByteArray {
        val buf = java.nio.ByteBuffer.allocate(pairs.size * 16)
            .order(java.nio.ByteOrder.LITTLE_ENDIAN)
        pairs.forEach { (k, v) -> buf.putLong(k).putLong(v) }
        return buf.array()
    }
}
