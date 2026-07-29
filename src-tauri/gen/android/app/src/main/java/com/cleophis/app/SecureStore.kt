package com.cleophis.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * AES-256-GCM under a non-exportable AndroidKeyStore key — the Kotlin half of
 * Phase 3.2. Reached from Rust through the bridge proven in the native chunk
 * (`src-tauri/src/android_bridge.rs`), and called by
 * `src-tauri/src/cloud/secure_store/android.rs`.
 *
 * **This class does crypto and nothing else.** It holds no app state, owns no
 * files beyond telling Rust where they go, and knows nothing about refresh
 * tokens or verifiers. Rust owns the file layout, the slot naming, the
 * traversal rejection and the on-disk envelope — all of which are
 * platform-neutral, all of which fail *silently* when wrong, and all of which
 * therefore live where the test suite actually runs (decision D-3). The common
 * Android pattern of an EncryptedSharedPreferences-style store that owns its
 * own persistence would have put every one of those rules somewhere no test on
 * this project can reach.
 *
 * `object` + `@JvmStatic` is load-bearing, not preference — see `NativeBridge`
 * for the full reason: without it these compile to instance methods on
 * `INSTANCE` and the JNI lookup fails with a `NoSuchMethodError` that says
 * nothing about why.
 *
 * ## R8
 *
 * Every method here is reachable only from Rust across JNI, which R8 cannot
 * see, so all of it is dead code to the shrinker and `isMinifyEnabled = true`
 * for release. `app/proguard-cleophis.pro` keeps it. The failure would be
 * release-only while the debug device checkpoint passes green.
 *
 * ## ⚠ Do not add `setUserAuthenticationRequired(true)`
 *
 * It looks like a free security upgrade and it is not. Two consequences, and
 * the second is the one that would be discovered by users rather than by
 * review:
 *
 * 1. **It breaks the feature.** Every `decrypt` would demand a device
 *    credential or biometric, and the decrypt that matters happens during
 *    boot-time `Cloud::restore()` — no UI, no user present. "Stays signed in"
 *    becomes "prompt on every launch", which is the exact opposite of what
 *    Phase 3.2 exists to deliver.
 * 2. **It arms `KeyPermanentlyInvalidatedException`.** With user-auth off — as
 *    it is here — enrolling a new fingerprint or changing the lock-screen PIN
 *    does **not** invalidate this key, so that exception essentially cannot
 *    arise, and the residue is already covered by the generic "key missing or
 *    unusable → no credential" path. Turn user-auth on and a routine PIN
 *    change silently destroys the key: every stored blob becomes permanently
 *    undecryptable and the user is signed out with no explanation and no way
 *    back except signing in again.
 *
 * So the absence of that flag is a **decision with a stated reason**, not an
 * oversight to be tidied up later. If a future requirement genuinely needs
 * auth-gated storage, it needs a second key with a different alias for data
 * that can afford to be lost — not this one.
 *
 * StrongBox (`setIsStrongBoxBacked`) is likewise deliberate: it throws
 * `StrongBoxUnavailableException` on devices without a discrete secure
 * element, which is much of our target market, so adopting it means adding a
 * fallback branch to the boot path in exchange for a property we cannot
 * confirm the reference device even offers. TEE-backed AndroidKeyStore is what
 * the threat model asks for.
 */
object SecureStore {
    private const val KEYSTORE = "AndroidKeyStore"

    /**
     * Versioned so a future key rotation is unambiguous. The envelope version
     * lives on the Rust side (`secure_store::blob::MAGIC`); this names the
     * *key*, which is a different fact with a different lifetime.
     */
    private const val ALIAS = "cleophis.securestore.v1"

    private const val TRANSFORM = "AES/GCM/NoPadding"

    /** GCM's standard nonce size. Also what AndroidKeyStore generates. */
    private const val IV_BYTES = 12

    /**
     * Pinned at the maximum. The tag length must be specified on decrypt, and
     * specifying it means a stored blob cannot negotiate its own — a shorter
     * tag is weaker forgery resistance, and "whatever the blob says" is not a
     * property.
     */
    private const val TAG_BITS = 128

    /** Directory name under `getNoBackupFilesDir()`. */
    private const val BLOB_DIR = "secure"

    /**
     * Where Rust writes the ciphertext blobs: `<noBackupFilesDir>/secure`,
     * created if absent.
     *
     * **`getNoBackupFilesDir()` rather than the app-data root, and the reason
     * is not merely defence in depth — it is the only coherent choice.** The
     * key these blobs are encrypted under is non-exportable and device-bound,
     * so it cannot be backed up or transferred by anything. A blob restored
     * onto another device (or onto this one after a wipe) is therefore
     * *permanently undecryptable*: backing the ciphertext up could only ever
     * produce a confusing decrypt failure, never a working session. Excluding
     * it is not a trade-off against convenience; there is no upside to trade
     * against.
     *
     * That it is excluded **by construction** is the second reason.
     * `BackupAgent.getExtraExcludeDirsIfAny` unconditionally adds this
     * directory, and the framework ignores even an app's own explicit
     * `fullBackupFile()` call on a file inside it. Compare the alternative:
     * exclusion via a rule in `backup_rules.xml` — a mechanism this project
     * has just demonstrated can be wrong for four phases without anyone
     * noticing, since those rules excluded four domains our data was not in.
     * A guarantee the framework enforces against our own mistakes beats one we
     * have to keep writing correctly.
     *
     * (It is *also* under the data-dir root, which those rules now exclude —
     * so the blobs are covered twice, once structurally. That is what
     * belt-and-braces is supposed to look like.)
     */
    @JvmStatic
    fun blobDir(context: Context): String =
        File(context.noBackupFilesDir, BLOB_DIR).apply { mkdirs() }.absolutePath

    /**
     * Encrypt `plaintext`, binding the result to `aad`.
     *
     * Returns base64 of `iv ‖ ciphertext‖tag`. The IV is **not** supplied by
     * us: `setRandomizedEncryptionRequired(true)` makes AndroidKeyStore
     * generate it from its own CSPRNG and *refuse* a caller-supplied one, so
     * nonce uniqueness is enforced by the platform rather than by our
     * discipline. That distinction matters more than it sounds — GCM nonce
     * reuse under one key is catastrophic rather than merely weak: two
     * messages sharing a nonce leak their XOR *and* leak the authentication
     * subkey, which lets an attacker forge tags for that key. It is the one
     * mistake in this file that could not be recovered from, so it is the one
     * we arrange to be unable to make.
     *
     * `aad` is the slot label from `secure_store::blob`. Without it every blob
     * under this key is interchangeable, and an attacker who could write to
     * the private directory could copy one account's verifier over another's:
     * it would decrypt perfectly and the second account's offline sign-in
     * would then accept the first account's password. That is a privilege
     * transfer, not corruption.
     */
    @JvmStatic
    fun encrypt(aad: String, plaintext: String): String {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, orCreateKey())
        cipher.updateAAD(aad.toByteArray(Charsets.UTF_8))
        val ciphertext = cipher.doFinal(plaintext.toByteArray(Charsets.UTF_8))
        val iv = cipher.iv
        // Not defensive padding: if a future provider ever returned a
        // different nonce size, `decrypt`'s fixed split would silently cut the
        // blob in the wrong place and every read would fail its tag check —
        // which reads as tampering. Fail here, where the cause is visible.
        require(iv.size == IV_BYTES) { "unexpected GCM IV length: ${iv.size}" }
        val out = ByteArray(iv.size + ciphertext.size)
        System.arraycopy(iv, 0, out, 0, iv.size)
        System.arraycopy(ciphertext, 0, out, iv.size, ciphertext.size)
        return Base64.encodeToString(out, Base64.NO_WRAP)
    }

    /**
     * Decrypt a blob produced by [encrypt] under the same `aad`.
     *
     * Throws on any failure and **never returns a partial result** — that is
     * the whole content of security review L1's "tag verified". `doFinal`
     * yields plaintext only after the GCM tag checks out; an
     * `AEADBadTagException` means the blob was tampered with, truncated, or
     * presented under the wrong slot label, and all three deserve the same
     * answer: no credential.
     *
     * Uses an EXISTING key only. If it generated one on demand, a lost or
     * rotated key would turn every read into a tag failure — indistinguishable
     * from tampering, and pointing an investigator at the blob rather than at
     * the keystore.
     */
    @JvmStatic
    fun decrypt(aad: String, blob: String): String {
        val key = existingKey()
            ?: throw IllegalStateException("no SecureStore key in AndroidKeyStore (alias $ALIAS)")
        val raw = Base64.decode(blob, Base64.NO_WRAP)
        require(raw.size > IV_BYTES) { "blob too short: ${raw.size} bytes" }
        val iv = raw.copyOfRange(0, IV_BYTES)
        val ciphertext = raw.copyOfRange(IV_BYTES, raw.size)

        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, iv))
        cipher.updateAAD(aad.toByteArray(Charsets.UTF_8))
        return String(cipher.doFinal(ciphertext), Charsets.UTF_8)
    }

    private fun existingKey(): SecretKey? {
        val ks = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        return (ks.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.secretKey
    }

    /**
     * `@Synchronized` because generation is not idempotent: `generateKey()`
     * on an existing alias REPLACES the key, which would leave every blob
     * written by the loser of the race undecryptable. Two saves can genuinely
     * overlap here — `Cloud::restore` runs on a `spawn_blocking` thread while
     * a sign-in runs on another — so the check-then-create needs to be atomic
     * rather than merely usually-fine.
     */
    @Synchronized
    private fun orCreateKey(): SecretKey {
        existingKey()?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                // GCM is a stream mode; padding is meaningless and
                // AndroidKeyStore rejects the combination outright.
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                // The default — set explicitly anyway, because a security
                // property that holds only by default is one line away from
                // not holding, and this is the line that makes nonce reuse
                // impossible rather than merely unlikely.
                .setRandomizedEncryptionRequired(true)
                .build()
        )
        return generator.generateKey()
    }
}
