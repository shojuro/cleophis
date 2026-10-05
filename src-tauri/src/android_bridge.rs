//! The JNI bridge: Rust → our own Kotlin → Rust.
//!
//! Android-only. This module exists to be *proven* before anything rides on
//! it. Three separate Kotlin shim surfaces are queued behind it —
//! `ConnectivityManager` (download network policy), `ACTION_SEND` (share-sheet
//! export), and SAF/content-URIs if the founder's pack-upload feature lands —
//! and if the first of them were built on an unproven bridge, a failure would
//! have two candidate causes and one symptom. This track has already paid that
//! bill at full price once: the gradle launcher bug surfaced as a failure in
//! `:app:rustBuildArm64Debug`, a *Rust* task, after the Rust build had
//! genuinely succeeded.
//!
//! # Why this does not use `ndk_context`
//!
//! The P1 brief (§5) and the native handoff both specify `ndk_context` as the
//! source of the `JavaVM` and `Context`. **That is wrong for this app, and
//! adopting it would have produced a bridge that fails at runtime for a reason
//! with nothing to do with JNI.** Checked rather than assumed:
//!
//! - `ndk-context` is absent from `Cargo.lock` entirely — not merely
//!   undeclared by us, but depended on by nothing in the graph. (`Cargo.lock`
//!   is target-independent and does list the Android-only deps: `jni 0.21.1`
//!   and `ndk 0.9.0` are both in it, via `tao`.)
//! - Nothing in `tao 0.35.3`, `wry 0.55.1` or `tauri 2.11.5` calls
//!   `ndk_context::initialize_android_context`. The single occurrence of the
//!   name in any of the three is a commented-out `// TODO: use ndk-context
//!   instead` in tao's Android event loop.
//!
//! `ndk_context`'s accessor reads process-global statics that some *other*
//! crate is expected to have filled in. Adding the dependency would have
//! compiled cleanly, cross-compiled cleanly, and then handed us null pointers
//! on device — the failure presenting as "JNI is broken" rather than as "this
//! global was never initialized". That is precisely the class of silent,
//! plausible-looking defect this milestone keeps cataloguing, and it is the
//! reason the bridge got its own commit and its own checkpoint.
//!
//! What Tauri actually offers is better: `tao` keeps its own `AndroidContext`
//! (the `JavaVM` pointer and the activity's global ref), populated in the
//! activity-create JNI handler, and re-exports an accessor for it. The chain
//! is public the whole way — `tauri::tao` is a re-export
//! (`tauri/src/lib.rs`: `pub use tauri_runtime_wry::{tao, wry}`), and
//! `tao::platform::android::prelude` re-exports the `ndk_glue` module where
//! `main_android_context` lives. No new dependency, and no global for us to
//! initialize.
//!
//! **Ordering is proven, not hoped for:** tao inserts the context into its map
//! and only *then* calls the setup function that reaches our `run()`
//! (`tao .../android/ndk_glue.rs`, the insert precedes the `setup(...)` call in
//! the same function). So `main_android_context()` is populated by the time
//! Tauri's `setup` hook runs, which is where the probe is called from.
//!
//! # Why none of this lives where the tests run (decision D-3)
//!
//! D-3 moves logic out from behind a `cfg` when it is platform-neutral *and*
//! fails silently. Neither half holds here. Every line below is FFI against a
//! live JVM, which is the case D-3 explicitly exempts ("tolerable for FFI,
//! where the alternative is mocking llama.cpp"), and the failure mode is the
//! opposite of silent: each step returns a described `Err` that the startup
//! line prints. There is no decision left in it for a test to check — the
//! class name and the method signature are the only claims being made, and the
//! round trip is what checks them.

use jni::objects::{JClass, JObject, JString};
use jni::{JNIEnv, JavaVM};
use tauri::tao::platform::android::prelude::main_android_context;

/// The Kotlin object we call into. Must match `NativeBridge.kt`'s package and
/// name; a mismatch surfaces as a `ClassNotFoundException` from `getAppClass`,
/// which the error string carries through verbatim.
const BRIDGE_CLASS: &str = "com.cleophis.app.NativeBridge";

/// **The one place the JNI entry discipline lives.** Acquire the JVM and the
/// activity, resolve one of our own Kotlin classes through the app class
/// loader, run `f` against it, and clear any pending Java exception on the way
/// out.
///
/// Generalised from `describe_device` when Phase 3.2's `SecureStore` became
/// the second caller. **This is not tidying.** The exception cleanup below is
/// the code most likely to be wrong and least likely to be exercised — it runs
/// only when something else has already failed, there is no JVM on the build
/// host to test it against, and its first version was wrong in a way that
/// crashed the process instead of reporting. A second hand-written copy of it
/// is the duplicated-fact trap (D-4's etiology) sitting in exactly the code
/// that would never catch the divergence.
///
/// `f` receives the activity as well as the class, because some shims need a
/// `Context` argument (`SecureStore.blobDir`) and re-deriving it would mean a
/// second `unsafe` block over the same pointer.
pub(crate) fn with_app_class<T>(
    class_name: &str,
    f: impl FnOnce(&mut JNIEnv, &JClass, &JObject) -> Result<T, String>,
) -> Result<T, String> {
    let ctx = main_android_context()
        .ok_or_else(|| "main_android_context() is None (no activity yet)".to_string())?;

    // SAFETY: `java_vm` is the pointer tao obtained from `JNIEnv::get_java_vm`
    // in the activity-create handler and holds for the life of the process.
    let vm = unsafe { JavaVM::from_raw(ctx.java_vm.cast()) }
        .map_err(|e| format!("JavaVM::from_raw: {e}"))?;

    // On the UI thread — where the smoke probe runs — the JVM has already
    // attached, so `attach_current_thread` tries `get_env()` first and hands
    // back a *nested* guard with `should_detach: false`, making the drop a
    // no-op. Only a guard for a thread this call actually attached detaches on
    // drop. Worth stating because the failure it rules out — detaching the UI
    // thread from the JVM — would take the whole app down and would look
    // nothing like a bridge bug.
    //
    // `SecureStore`'s callers reach this from a DIFFERENT direction:
    // `Cloud::restore` arrives via `tauri::async_runtime::spawn_blocking`
    // (`cloud/commands.rs`), i.e. a Tokio blocking-pool thread that the JVM has
    // NOT attached, and potentially a different one per call. There the guard
    // genuinely attaches and genuinely detaches, which is correct — and is a
    // code path the bridge's device checkpoint did not exercise, since that
    // probe only ever ran on the UI thread.
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach_current_thread: {e}"))?;

    // SAFETY: `context_jobject` is a *global* ref tao created and keeps alive
    // (`env.new_global_ref(activity)`), so borrowing it here is sound and we
    // must not free it — `JObject::from_raw` does not take ownership.
    let activity = unsafe { JObject::from_raw(ctx.context_jobject.cast()) };

    let result = resolve_and_run(&mut env, &activity, class_name, f);

    // A failed JNI call leaves its Java exception PENDING — `jni` does not
    // clear it (its own docs: the exception "will be thrown in java unless
    // `exception_clear` is called"). That is not cosmetic. On the UI thread,
    // which Tauri and wry drive with constant JNI traffic, ART aborts the
    // process on the next JNI call made with an exception pending. So the
    // naive version of this **crashes the app instead of reporting**, and only
    // on the failure path — the one this code exists to make legible. A
    // diagnostic whose failure mode is a crash tells you strictly less than
    // one that prints a line.
    //
    // `exception_describe` first, because it dumps the Java stack trace to
    // logcat, which is the part that names the missing class or method — or,
    // for `SecureStore`, distinguishes an `AEADBadTagException` (tampered or
    // mismatched blob) from a keystore failure.
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }

    result
}

/// Class resolution plus the caller's work. Split out so `with_app_class` can
/// clear a pending exception on any error without repeating cleanup at each `?`.
fn resolve_and_run<T>(
    env: &mut JNIEnv,
    activity: &JObject,
    class_name: &str,
    f: impl FnOnce(&mut JNIEnv, &JClass, &JObject) -> Result<T, String>,
) -> Result<T, String> {
    let name = env
        .new_string(class_name)
        .map_err(|e| format!("new_string: {e}"))?;

    // Deliberately NOT `env.find_class`. On a thread attached via JNI,
    // `FindClass` resolves against the *system* class loader, which cannot see
    // application classes — the single most common way an Android JNI bridge
    // fails, and it fails with a bare ClassNotFoundException that says nothing
    // about class loaders. `WryActivity.getAppClass` calls `Class.forName` from
    // inside an app class, so it resolves in the app's loader. tao routes its
    // own lookups through the same method for the same reason.
    //
    // Note the consequence for `Desc`: passing a `&str` where `jni` wants a
    // class silently selects the `find_class` path, so the wrong path is not
    // merely avoided here but unreachable — `f` is handed a `&JClass`.
    let class = env
        .call_method(
            activity,
            "getAppClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&name).into()],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("getAppClass({class_name}): {e}"))?;

    f(env, &JClass::from(class), activity)
}

/// Pull a `String` return value back into Rust. Shared for the same reason as
/// the entry discipline: one conversion, one error string.
pub(crate) fn jstring_result(env: &mut JNIEnv, value: JObject, what: &str) -> Result<String, String> {
    if value.is_null() {
        return Err(format!("{what}: returned null"));
    }
    env.get_string(&JString::from(value))
        .map_err(|e| format!("{what}: get_string: {e}"))
        .map(|s| s.to_string_lossy().to_string())
}

/// One honest round trip: acquire the JVM and the activity, call a static
/// method on **our** Kotlin, and bring its `String` back into Rust.
///
/// It returns the device description rather than a constant on purpose. A
/// hard-coded return value would prove the call mechanism and nothing else;
/// reading `android.os.Build` proves the Kotlin ran with real framework access,
/// which is what every queued shim actually needs. It also makes the result
/// *predictable in advance* — see the checkpoint ask — so an unexpected value
/// is a failure signal rather than a curiosity.
pub(crate) fn describe_device() -> Result<String, String> {
    with_app_class(BRIDGE_CLASS, |env, class, _activity| {
        let value = env
            .call_static_method(class, "describeDevice", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
            .map_err(|e| format!("describeDevice: {e}"))?;
        jstring_result(env, value, "describeDevice")
    })
}

/// Run the round trip and print one line, in the mould of `[kernels]`.
///
/// The value of that convention is that a device transcript carries its own
/// proof rather than requiring someone to have believed a build log. Both
/// outcomes print: a bridge that silently does nothing would be
/// indistinguishable from one that was never called, which is the delivery
/// failure this milestone has already recorded once.
///
/// CPU/model identifiers only — no user or token material — so this is safe in
/// a release log (security review M3).
pub(crate) fn log_smoke_probe() {
    match describe_device() {
        Ok(device) => eprintln!("[bridge] ok round-trip via {BRIDGE_CLASS} device={device}"),
        Err(e) => eprintln!("[bridge] FAILED {e}"),
    }
}
