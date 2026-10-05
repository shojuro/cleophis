//! What the CPU **actually implements**, read from the kernel — and the
//! adjudication of that against what the binary was **compiled to require**.
//!
//! ## Why this module exists
//!
//! The `[kernels]` line has printed `DOTPROD = 1` since the P0 spike, and the
//! standing rule was *"DOTPROD = 1 or the numbers are invalid"*. That line
//! comes from `llama_print_system_info()`, which reports the **compile-time**
//! macro `__ARM_FEATURE_DOTPROD`. The vendored `llama-cpp-sys-2` patch hardcodes
//! `GGML_CPU_ARM_ARCH=armv8.2-a+dotprod`, so the macro is defined on **every**
//! aarch64-android build of this project, on every device, unconditionally.
//!
//! It printed `DOTPROD = 1` on a Galaxy A51 (Exynos 9611, Cortex-A73+A53,
//! ARMv8.0) seconds before that device took **SIGILL on a dotprod
//! instruction**. A check that cannot print `0` is not a check — it is the
//! project's most load-bearing diagnostic reporting a build constant back to
//! the person who set it.
//!
//! This module supplies the other half: the runtime capability word the kernel
//! publishes in the auxiliary vector, and a **differential** between the two.
//! Per the ledger's standing form — *a differential result localises the error
//! to the instrument that disagrees with the artifact* — neither number alone
//! is the finding. Their disagreement is.
//!
//! ## Why `/proc/self/auxv` rather than `getauxval(3)`
//!
//! Same values (`getauxval` is a lookup over this vector), no new dependency,
//! and — the reason that decides it — the parse becomes a **pure function the
//! desktop suite can test**, while the detection around it can be tested
//! nowhere but a phone. Decision D-3, applied the same way
//! `src-tauri/src/hardware.rs` applied it for SoC tiering; this module is now
//! that code's single home, so a tiering verdict and a kernel verdict cannot
//! disagree about the same bit.
//!
//! ## What this module is NOT
//!
//! It is not a guarantee that a build will run. It adjudicates the four
//! instruction groups llama.cpp *reports*; a `-march` floor also licenses the C
//! compiler to emit ARMv8.1 atomics and ARMv8.2 half-precision **anywhere**,
//! and llama.cpp reports no flag for either. Those bits are therefore printed
//! as facts and called out in an advisory, deliberately kept out of the pass/
//! fail verdict so that the verdict stays something the printed evidence
//! entails. See [`ARCH_FLOOR_ADVISORY`].

use core::fmt::Write as _;

/// Which auxv word carries a bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    Hwcap,
    Hwcap2,
}

// AArch64 HWCAP bits, from the kernel's `arch/arm64/include/uapi/asm/hwcap.h`,
// and the auxv keys that carry them. Named rather than inlined because a wrong
// bit here is silent in the worst direction: it would report a capability the
// CPU does not have, which is the exact failure this module was written to end.
pub const AT_HWCAP: u64 = 16;
pub const AT_HWCAP2: u64 = 26;

pub const HWCAP_FP: u64 = 1 << 0;
pub const HWCAP_ASIMD: u64 = 1 << 1;
/// ARMv8.1 LSE atomics (`ldadd`, `casal`, …). Absent on Cortex-A73/A53.
pub const HWCAP_ATOMICS: u64 = 1 << 8;
/// ARMv8.2 half-precision SIMD arithmetic.
pub const HWCAP_ASIMDHP: u64 = 1 << 10;
/// ARMv8.2 dot product (`sdot`/`udot`) — llama.cpp's Q4 matmul kernel.
pub const HWCAP_ASIMDDP: u64 = 1 << 20;
pub const HWCAP_SVE: u64 = 1 << 22;
/// ARMv8.6 Int8 matrix multiply (`smmla`).
pub const HWCAP2_I8MM: u64 = 1 << 13;
pub const HWCAP2_BF16: u64 = 1 << 14;

/// The pairs the verdict is computed from: a llama.cpp system-info flag naming
/// a **compile-time** macro, and the HWCAP bit naming the **runtime** fact for
/// the same instruction group.
///
/// `(runtime name, word, mask, llama.cpp flag)`. Adjudicated in exactly one
/// direction — *compiled 1 while runtime 0 is fatal* — because that is the
/// direction in which the program executes an instruction the silicon does not
/// decode.
pub const ADJUDICATED: &[(&str, Word, u64, &str)] = &[
    ("asimddp", Word::Hwcap, HWCAP_ASIMDDP, "DOTPROD"),
    ("i8mm", Word::Hwcap2, HWCAP2_I8MM, "MATMUL_INT8"),
    ("sve", Word::Hwcap, HWCAP_SVE, "SVE"),
    ("asimdhp", Word::Hwcap, HWCAP_ASIMDHP, "FP16_VA"),
];

/// Printed as facts, with no llama.cpp counterpart to adjudicate against.
///
/// `lse-atomics` is the one that matters and the reason this list exists: a
/// `-march=armv8.2-a` floor licenses LSE everywhere in the C sources, llama.cpp
/// reports no flag for it, and the A51 does not implement it. So its absence is
/// a *second*, independent way the ARMv8.2 floor can SIGILL — invisible to the
/// adjudicated pairs above.
pub const REPORTED: &[(&str, Word, u64)] = &[
    ("fp", Word::Hwcap, HWCAP_FP),
    ("asimd", Word::Hwcap, HWCAP_ASIMD),
    ("lse-atomics", Word::Hwcap, HWCAP_ATOMICS),
    ("bf16", Word::Hwcap2, HWCAP2_BF16),
];

/// Emitted when a compiled ARMv8.2 flag is present but the ARMv8.1/8.2 baseline
/// bits are not. Advisory rather than verdict: see the module docs.
pub const ARCH_FLOOR_ADVISORY: &str =
    "an ARMv8.2 kernel flag is compiled in, so the C floor is >= armv8.1-a and \
     the compiler may emit LSE atomics / fp16 ANYWHERE, not only in the kernels \
     llama.cpp reports";

/// The kernel's capability words for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeCpu {
    pub hwcap: u64,
    pub hwcap2: u64,
}

impl RuntimeCpu {
    pub fn has(&self, word: Word, mask: u64) -> bool {
        match word {
            Word::Hwcap => self.hwcap & mask != 0,
            Word::Hwcap2 => self.hwcap2 & mask != 0,
        }
    }
}

/// The adjudication of a build against the silicon it is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not aarch64, or the auxiliary vector could not be read. **Nothing was
    /// checked** — reported under its own name so it is never mistaken for a
    /// pass, which is the mistake this whole module exists to correct.
    Unchecked,
    /// Every compile-time kernel flag is backed by a runtime capability bit.
    Ok,
    /// Will run, but the CPU implements instruction groups the build does not
    /// use. The signal for "this binary is a baseline build on capable
    /// hardware" — i.e. the cost side of shipping one universal binary.
    Underbuilt {
        /// Runtime names present in hardware, absent from the build.
        unused: Vec<&'static str>,
    },
    /// The build requires instructions this CPU does not implement. SIGILL is
    /// the expected outcome, at the first kernel that uses one.
    Incompatible {
        /// `(llama.cpp flag, runtime name)` pairs, compiled 1 and runtime 0.
        missing: Vec<(&'static str, &'static str)>,
    },
}

/// Pull `AT_HWCAP` and `AT_HWCAP2` out of a raw `/proc/self/auxv` image.
///
/// The file is a flat array of `(key, value)` pairs of `unsigned long`,
/// terminated by an `AT_NULL` (0) key.
///
/// Trailing bytes that do not form a whole pair are ignored rather than guessed
/// at: a truncated read should **lose** a capability and never invent one,
/// because losing one produces a false `Incompatible` (loud, and wrong in the
/// direction that refuses to run) while inventing one produces a false `Ok`
/// (silent, and wrong in the direction that crashes).
pub fn hwcaps_from_auxv(bytes: &[u8]) -> RuntimeCpu {
    let mut caps = RuntimeCpu::default();
    for pair in bytes.chunks_exact(16) {
        let key = u64::from_ne_bytes(pair[..8].try_into().unwrap_or_default());
        let val = u64::from_ne_bytes(pair[8..].try_into().unwrap_or_default());
        match key {
            0 => break, // AT_NULL terminates the vector.
            AT_HWCAP => caps.hwcap = val,
            AT_HWCAP2 => caps.hwcap2 = val,
            _ => {}
        }
    }
    caps
}

/// Read this process's capability words. `None` on any target where the
/// question is not asked (not aarch64) or cannot be answered (auxv unreadable)
/// — both of which produce [`Verdict::Unchecked`] rather than a pass.
#[cfg(all(target_arch = "aarch64", any(target_os = "android", target_os = "linux")))]
pub fn runtime_cpu() -> Option<RuntimeCpu> {
    std::fs::read("/proc/self/auxv").ok().map(|b| hwcaps_from_auxv(&b))
}

#[cfg(not(all(target_arch = "aarch64", any(target_os = "android", target_os = "linux"))))]
pub fn runtime_cpu() -> Option<RuntimeCpu> {
    None
}

/// Read one flag out of `llama_print_system_info()`'s output.
///
/// The format is `NAME = VALUE | NAME = VALUE | …`, with the first field
/// carrying a `CPU : ` prefix. Parsed by splitting on the separators rather
/// than by substring search, so `MATMUL_INT8` cannot be found by asking for
/// `INT8` and `DOTPROD = 10` could never read as `DOTPROD = 1`.
///
/// ⚑ **Absent means off, and that is established by reading the producer, not
/// by assuming it.** `ggml_backend_cpu_get_features` pushes an entry only
/// inside `if (ggml_cpu_has_X())` and always with the literal value `"1"`, so
/// a feature that is off is simply missing from the string — `Some(false)` is
/// unreachable from real output. Hence `None` is treated as "compiled off" by
/// [`adjudicate`]. It is still returned distinctly rather than folded into
/// `Some(false)`, because that distinction is what lets a caller tell "this
/// producer does not know the flag" from "this producer says no".
///
/// (Also read at the source, and it is the whole defect: every `ggml_cpu_has_*`
/// on ARM is `#if defined(__ARM_FEATURE_…)`. The string is a build constant.)
pub fn compiled_flag(info: &str, name: &str) -> Option<bool> {
    for field in info.split('|') {
        let mut halves = field.splitn(2, '=');
        let Some(key) = halves.next() else { continue };
        let Some(value) = halves.next() else { continue };
        // `CPU : NEON` -> `NEON`. Anything after the last colon, trimmed.
        let key = key.rsplit(':').next().unwrap_or(key).trim();
        if key != name {
            continue;
        }
        return match value.trim() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
    }
    None
}

/// Compare what the binary was compiled to require against what the silicon
/// implements.
///
/// **Only one direction is fatal.** Compiled-1/runtime-0 means the program will
/// execute an undefined instruction. Compiled-0/runtime-1 means it will merely
/// run slower than the hardware allows, which is [`Verdict::Underbuilt`].
pub fn adjudicate(info: &str, runtime: Option<RuntimeCpu>) -> Verdict {
    let Some(cpu) = runtime else {
        return Verdict::Unchecked;
    };
    let mut missing = Vec::new();
    let mut unused = Vec::new();
    for &(rt_name, word, mask, flag) in ADJUDICATED {
        let present = cpu.has(word, mask);
        match compiled_flag(info, flag) {
            Some(true) if !present => missing.push((flag, rt_name)),
            Some(false) | None if present => unused.push(rt_name),
            _ => {}
        }
    }
    if !missing.is_empty() {
        Verdict::Incompatible { missing }
    } else if !unused.is_empty() {
        Verdict::Underbuilt { unused }
    } else {
        Verdict::Ok
    }
}

/// Does the build claim an ARMv8.2 kernel while the ARMv8.1/8.2 baseline bits
/// are absent? The second, independent way this build can SIGILL — see
/// [`ARCH_FLOOR_ADVISORY`].
fn arch_floor_suspect(info: &str, cpu: RuntimeCpu) -> bool {
    let claims_v82 = ADJUDICATED
        .iter()
        .any(|&(_, _, _, flag)| compiled_flag(info, flag) == Some(true));
    claims_v82 && !cpu.has(Word::Hwcap, HWCAP_ATOMICS)
}

/// The runtime half: what the kernel says this CPU implements.
///
/// Split from the verdict deliberately. **Ordering is load-bearing** — a caller
/// prints this BEFORE touching llama.cpp, so a device that dies inside backend
/// initialisation still leaves the capability word in the transcript, which is
/// the one fact that explains such a death. `kernel_report` glues the halves
/// back together and a test pins the two forms to each other so they cannot
/// drift.
///
/// Every line is CPU feature flags. No user text, no tokens, no paths: safe in
/// a release log (security review M3), which is the point of putting it in the
/// app rather than in a build log.
pub fn runtime_line(runtime: Option<RuntimeCpu>) -> String {
    let mut out = String::new();

    match runtime {
        Some(cpu) => {
            let _ = writeln!(
                out,
                "[cpu-runtime] aarch64 hwcap=0x{:016x} hwcap2=0x{:016x}  (AT_HWCAP/AT_HWCAP2, /proc/self/auxv)",
                cpu.hwcap, cpu.hwcap2
            );
            let mut names = String::new();
            for &(rt_name, word, mask, _) in ADJUDICATED {
                let _ = write!(names, " {rt_name}={}", u8::from(cpu.has(word, mask)));
            }
            for &(rt_name, word, mask) in REPORTED {
                let _ = write!(names, " {rt_name}={}", u8::from(cpu.has(word, mask)));
            }
            let _ = writeln!(out, "[cpu-runtime]{names}");
        }
        None => {
            let _ = writeln!(
                out,
                "[cpu-runtime] UNCHECKED — not aarch64, or /proc/self/auxv unreadable"
            );
        }
    }

    out.trim_end().to_string()
}

/// The compile-time half and the adjudication between them.
pub fn verdict_block(info: &str, runtime: Option<RuntimeCpu>) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "[kernels] (compile-time macros) {}", info.trim());

    // The verdict, and — per the ledger's standing rule that a passing check
    // owes an attribution — the pair that carried it, named, in every branch.
    match adjudicate(info, runtime) {
        Verdict::Unchecked => {
            let _ = writeln!(
                out,
                "[cpu-verdict] UNCHECKED — no runtime capability word; the [kernels] line above is a BUILD CONSTANT and says nothing about this machine"
            );
        }
        // `Ok` and `Underbuilt` are only reachable when `runtime` is `Some`
        // (`adjudicate` returns `Unchecked` otherwise), so the default here is
        // unreachable rather than a fallback with an opinion.
        Verdict::Ok => {
            let _ = writeln!(
                out,
                "[cpu-verdict] OK — every compiled kernel flag is implemented here; checked (compiled/runtime){}",
                pairs_checked(info, runtime.unwrap_or_default())
            );
        }
        Verdict::Underbuilt { unused } => {
            let _ = writeln!(
                out,
                "[cpu-verdict] OK (UNDERBUILT) — will run; this CPU implements [{}] which the build does not use; checked (compiled/runtime){}",
                unused.join(" "),
                pairs_checked(info, runtime.unwrap_or_default())
            );
        }
        Verdict::Incompatible { missing } => {
            for (flag, rt_name) in &missing {
                let _ = writeln!(
                    out,
                    "[cpu-verdict] INCOMPATIBLE — compiled {flag}=1 but runtime {rt_name}=0; SIGILL expected at the first kernel that uses it"
                );
            }
        }
    }

    if let Some(cpu) = runtime {
        if arch_floor_suspect(info, cpu) {
            let _ = writeln!(out, "[cpu-verdict] ADVISORY — {ARCH_FLOOR_ADVISORY}");
        }
    }

    out.trim_end().to_string()
}

/// Both halves, for a caller that can afford to print them together (tests, and
/// any context where llama.cpp is already initialised).
pub fn kernel_report(info: &str, runtime: Option<RuntimeCpu>) -> String {
    format!("{}\n{}", runtime_line(runtime), verdict_block(info, runtime))
}

/// `" DOTPROD/asimddp=1/1 MATMUL_INT8/i8mm=0/0 …"` — compiled/runtime for every
/// adjudicated pair. The attribution a green owes: without it "OK" is a word,
/// and with it a reader can see which clause carried the verdict and whether it
/// could ever have said otherwise.
fn pairs_checked(info: &str, cpu: RuntimeCpu) -> String {
    let mut s = String::new();
    for &(rt_name, word, mask, flag) in ADJUDICATED {
        let c = match compiled_flag(info, flag) {
            Some(true) => "1",
            Some(false) => "0",
            None => "-",
        };
        let r = u8::from(cpu.has(word, mask));
        let _ = write!(s, " {flag}/{rt_name}={c}/{r}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚑ **MEASURED, not composed.** This is `llama_print_system_info()`'s
    /// output byte-for-byte from the shipped `armv8.2-a+dotprod` build running
    /// on a Galaxy A51 on 2026-07-31, captured off the device before the run
    /// SIGILL'd.
    ///
    /// The first draft of this constant was invented from memory and carried
    /// `FP16_VA = 1 | MATMUL_INT8 = 0 | LLAMAFILE = 1 | OPENMP = 1`. **None of
    /// those fields exist.** Reading the producer explained why: features that
    /// are off are omitted entirely, never printed as `= 0`. A fixture that
    /// looked completely plausible would have pinned the parser to a format
    /// llama.cpp does not emit — the exact shape of "verify against the
    /// artifact, not the config".
    const DOTPROD_BUILD: &str = "CPU : NEON = 1 | ARM_FMA = 1 | DOTPROD = 1 | REPACK = 1 | ";

    /// The same source at a plain `armv8-a` floor.
    ///
    /// ⚠ **PREDICTED, not yet measured** — `__ARM_FEATURE_DOTPROD` goes
    /// undefined so `DOTPROD` drops out, while `__ARM_NEON` and
    /// `__ARM_FEATURE_FMA` hold at ARMv8.0. To be replaced with the measured
    /// string once the baseline build runs on a device; if the prediction is
    /// wrong that is a finding, and it is labelled here so the difference is
    /// visible rather than quietly overwritten.
    const BASELINE_BUILD: &str = "CPU : NEON = 1 | ARM_FMA = 1 | REPACK = 1 | ";

    /// Galaxy A51, SM-A515F, Exynos 9611 (Cortex-A73 + A53), Android 13.
    /// `/proc/cpuinfo` Features, read off the device: `fp asimd evtstrm aes
    /// pmull sha1 sha2 crc32 cpuid` — no `asimddp`, no `i8mm`, no `atomics`.
    fn a51() -> RuntimeCpu {
        RuntimeCpu { hwcap: HWCAP_FP | HWCAP_ASIMD, hwcap2: 0 }
    }

    /// A **constructed** ARMv8.2 CPU with dotprod and fp16, no i8mm.
    ///
    /// Deliberately not named for a device. The obvious name here is `a22()`,
    /// and the A22's capability word has never been read — writing its name on
    /// an assumed bit pattern would smuggle an unmeasured device claim into the
    /// suite, which is a smaller version of the defect this module fixes. The
    /// A22's real positive control is the on-device run, reported separately.
    fn v82_dotprod_cpu() -> RuntimeCpu {
        RuntimeCpu {
            hwcap: HWCAP_FP | HWCAP_ASIMD | HWCAP_ATOMICS | HWCAP_ASIMDHP | HWCAP_ASIMDDP,
            hwcap2: 0,
        }
    }

    // ── the parse ────────────────────────────────────────────────────────────

    #[test]
    fn a_flag_is_read_through_the_cpu_prefix_and_the_separators() {
        // `NEON` proves the `CPU : ` prefix on the first field is handled.
        assert_eq!(compiled_flag(DOTPROD_BUILD, "NEON"), Some(true));
        assert_eq!(compiled_flag(DOTPROD_BUILD, "DOTPROD"), Some(true));
        // Off is expressed by omission, per the producer.
        assert_eq!(compiled_flag(DOTPROD_BUILD, "MATMUL_INT8"), None);
    }

    #[test]
    fn a_substring_is_not_a_flag() {
        // `info.contains("DOT")` is true of this string; asking for the flag
        // `DOT` must not be.
        assert_eq!(compiled_flag(DOTPROD_BUILD, "DOT"), None);
        assert_eq!(compiled_flag("CPU : MATMUL_INT8 = 1 | ", "INT8"), None);
        assert_eq!(compiled_flag("CPU : MATMUL_INT8 = 1 | ", "MATMUL_INT8"), Some(true));
    }

    #[test]
    fn an_absent_flag_is_none_not_false() {
        assert_eq!(compiled_flag(DOTPROD_BUILD, "AVX2"), None);
        assert_eq!(compiled_flag("", "DOTPROD"), None);
    }

    #[test]
    fn a_value_that_is_not_a_bit_is_not_a_bit() {
        assert_eq!(compiled_flag("CPU : DOTPROD = 10 | ", "DOTPROD"), None);
        assert_eq!(compiled_flag("CPU : DOTPROD | ", "DOTPROD"), None);
    }

    // ── the auxv parse ───────────────────────────────────────────────────────

    #[test]
    fn hwcaps_are_read_out_of_an_auxv_image() {
        let mut buf = Vec::new();
        let mut pair = |k: u64, v: u64| {
            buf.extend_from_slice(&k.to_ne_bytes());
            buf.extend_from_slice(&v.to_ne_bytes());
        };
        pair(6, 4096); // AT_PAGESZ — must be ignored
        pair(AT_HWCAP, HWCAP_ASIMDDP);
        pair(AT_HWCAP2, HWCAP2_I8MM);
        pair(0, 0); // AT_NULL

        let caps = hwcaps_from_auxv(&buf);
        assert!(caps.has(Word::Hwcap, HWCAP_ASIMDDP));
        assert!(caps.has(Word::Hwcap2, HWCAP2_I8MM));
        assert!(!caps.has(Word::Hwcap, HWCAP_ATOMICS));
    }

    #[test]
    fn auxv_parsing_stops_at_the_terminator_and_tolerates_a_short_read() {
        let mut buf = Vec::new();
        let mut pair = |k: u64, v: u64| {
            buf.extend_from_slice(&k.to_ne_bytes());
            buf.extend_from_slice(&v.to_ne_bytes());
        };
        pair(0, 0); // AT_NULL first
        pair(AT_HWCAP, HWCAP_ASIMDDP); // past the terminator — must not be read
        assert_eq!(hwcaps_from_auxv(&buf), RuntimeCpu::default());

        let mut short = AT_HWCAP.to_ne_bytes().to_vec();
        short.extend_from_slice(&[0xff, 0xff, 0xff]);
        assert_eq!(hwcaps_from_auxv(&short), RuntimeCpu::default());
        assert_eq!(hwcaps_from_auxv(&[]), RuntimeCpu::default());
    }

    // ── the verdict ──────────────────────────────────────────────────────────

    #[test]
    fn the_shipped_build_on_the_a51_is_incompatible() {
        // The defect, as a test. This is the arrangement that produced EXIT=132
        // (128+4, SIGILL) on real hardware.
        assert_eq!(
            adjudicate(DOTPROD_BUILD, Some(a51())),
            Verdict::Incompatible { missing: vec![("DOTPROD", "asimddp")] }
        );
    }

    #[test]
    fn the_shipped_build_on_a_v82_cpu_does_not_fire() {
        // The positive control, and the reason it matters: without a case where
        // the same instrument declines to fail, INCOMPATIBLE could be a check
        // that always fires — the mirror image of the check that never could.
        //
        // The verdict is UNDERBUILT rather than OK, and that is not a
        // near-miss: `armv8.2-a+dotprod` does not turn on `+fp16`, so a CPU
        // with `asimdhp` really does have a capability this build ignores. What
        // the control asserts is the fatal list being EMPTY.
        let v = adjudicate(DOTPROD_BUILD, Some(v82_dotprod_cpu()));
        assert!(!matches!(v, Verdict::Incompatible { .. }), "{v:?}");
        assert_eq!(v, Verdict::Underbuilt { unused: vec!["asimdhp"] });
    }

    #[test]
    fn a_baseline_build_runs_everywhere_and_says_what_it_gives_up() {
        // The whole point of the cheap fix: one binary, correct on both classes
        // of device, and the instrument distinguishes them.
        assert_eq!(adjudicate(BASELINE_BUILD, Some(a51())), Verdict::Ok);
        assert_eq!(
            adjudicate(BASELINE_BUILD, Some(v82_dotprod_cpu())),
            Verdict::Underbuilt { unused: vec!["asimddp", "asimdhp"] }
        );
    }

    #[test]
    fn no_runtime_word_is_unchecked_never_a_pass() {
        // The whole failure being corrected, asserted directly: absence of
        // evidence must not render as OK.
        assert_eq!(adjudicate(DOTPROD_BUILD, None), Verdict::Unchecked);
        assert!(kernel_report(DOTPROD_BUILD, None).contains("UNCHECKED"));
        assert!(!kernel_report(DOTPROD_BUILD, None).contains("[cpu-verdict] OK"));
    }

    // ── the printed block ────────────────────────────────────────────────────

    #[test]
    fn the_report_prints_runtime_before_it_touches_the_compile_time_string() {
        // Ordering is the property, not decoration: a device that dies inside
        // llama.cpp's backend init must still leave the capability word behind.
        let r = kernel_report(DOTPROD_BUILD, Some(a51()));
        let rt = r.find("[cpu-runtime]").expect("runtime line");
        let kernels = r.find("[kernels]").expect("kernels line");
        assert!(rt < kernels, "runtime line must precede the kernels line:\n{r}");
    }

    #[test]
    fn the_a51_report_names_the_instruction_and_predicts_the_signal() {
        let r = kernel_report(DOTPROD_BUILD, Some(a51()));
        assert!(r.contains("INCOMPATIBLE"), "{r}");
        assert!(r.contains("compiled DOTPROD=1 but runtime asimddp=0"), "{r}");
        assert!(r.contains("SIGILL"), "{r}");
        // The second, independent way this build dies on this CPU.
        assert!(r.contains("ADVISORY"), "{r}");
        assert!(r.contains("lse-atomics=0"), "{r}");
    }

    #[test]
    fn a_green_names_the_clause_that_carried_it() {
        // A passing check owes an attribution (Conventions). Without the pair
        // list, "OK" is indistinguishable from a check that cannot fail — which
        // is precisely the defect this module replaces.
        let r = kernel_report(DOTPROD_BUILD, Some(v82_dotprod_cpu()));
        assert!(r.contains("[cpu-verdict] OK"), "{r}");
        assert!(r.contains("DOTPROD/asimddp=1/1"), "{r}");
        // `-` is "the producer did not report this flag", distinct from `0`.
        assert!(r.contains("MATMUL_INT8/i8mm=-/0"), "{r}");
        assert!(!r.contains("ADVISORY"), "this CPU has LSE; no advisory is due:\n{r}");
    }

    #[test]
    fn the_split_halves_are_exactly_the_whole_report() {
        // The shipped path prints `runtime_line` first, then calls into
        // llama.cpp, then prints `verdict_block`. Every other test here
        // exercises `kernel_report`. This is the only thing stopping the tested
        // form and the shipped form from drifting apart — which is this repo's
        // characteristic failure, a check that is green about something other
        // than what runs.
        for (info, rt) in [
            (DOTPROD_BUILD, Some(a51())),
            (DOTPROD_BUILD, Some(v82_dotprod_cpu())),
            (BASELINE_BUILD, Some(v82_dotprod_cpu())),
            (DOTPROD_BUILD, None),
        ] {
            assert_eq!(
                kernel_report(info, rt),
                format!("{}\n{}", runtime_line(rt), verdict_block(info, rt))
            );
        }
    }

    #[test]
    fn the_hex_words_are_printed_so_a_transcript_can_be_re_adjudicated_later() {
        // Named bits are an interpretation; the raw words are the evidence. A
        // transcript that carries only the interpretation cannot be re-checked
        // when the bit table turns out to be wrong.
        let r = kernel_report(DOTPROD_BUILD, Some(v82_dotprod_cpu()));
        assert!(r.contains(&format!("hwcap=0x{:016x}", v82_dotprod_cpu().hwcap)), "{r}");
        assert!(r.contains("hwcap2=0x0000000000000000"), "{r}");
    }
}
