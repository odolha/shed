package dev.shed.data

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import android.util.Base64
import androidx.core.content.edit
import dev.shed.lock.LockAfter
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Who this phone is signed in as, at which nexus. */
data class Account(
    val nexus: String,
    val username: String,
    val device: String,
    val token: String,
)

/** Encrypts and decrypts the device token with a key of its own. */
interface Sealer {
    fun seal(text: String): String
    fun open(sealed: String): String
    /** Forgets the key. */
    fun delete()
}

/** What loading the account found. */
sealed interface Loaded {
    data class Found(val account: Account) : Loaded

    /** No account here. */
    data object None : Loaded

    /** The token is under the locked key, and the phone was not unlocked just now. */
    data object NotUnlocked : Loaded

    /** The key the token was sealed with is gone (the screen lock was removed): sign in again. */
    data object KeyLost : Loaded
}

/**
 * Keeps the account. The device token is encrypted with an AES-GCM key in the
 * Android Keystore (it never leaves it): while the app lock is on, a key that
 * works only within a few seconds of the phone's own unlock (fingerprint,
 * face, PIN or pattern), so a copy of the app's files, or the app started
 * without unlocking, cannot read it; with the lock off (or no screen lock on
 * the phone), a key without that condition. A token kept the old way (the
 * plain key) moves to the locked key at the first unlock ([reseal]).
 * Nothing here is backed up (see `data_extraction_rules.xml`).
 */
class AccountStore(
    private val prefs: SharedPreferences,
    private val plain: Sealer,
    private val locked: Sealer,
) {
    constructor(context: Context) : this(
        context.getSharedPreferences("account", Context.MODE_PRIVATE),
        KeystoreSealer(PLAIN_ALIAS, authenticated = false),
        KeystoreSealer(LOCKED_ALIAS, authenticated = true),
    )

    /** Whether an account is kept (the token not read). */
    fun has(): Boolean = prefs.getString(NEXUS, null) != null && prefs.getString(TOKEN, null) != null

    /** Whether the token is under the locked key. */
    fun isLocked(): Boolean = prefs.getString(KEY, PLAIN) == LOCKED

    /** The account; with the locked key, only within a few seconds of an unlock. */
    fun load(): Loaded {
        val nexus = prefs.getString(NEXUS, null) ?: return Loaded.None
        val sealed = prefs.getString(TOKEN, null) ?: return Loaded.None
        val token = try {
            (if (isLocked()) locked else plain).open(sealed)
        } catch (e: KeyPermanentlyInvalidatedException) {
            return Loaded.KeyLost
        } catch (e: java.security.UnrecoverableKeyException) {
            return Loaded.KeyLost
        } catch (e: UserNotAuthenticatedException) {
            return Loaded.NotUnlocked
        } catch (e: java.security.GeneralSecurityException) {
            return Loaded.KeyLost
        } catch (e: IllegalArgumentException) {
            return Loaded.KeyLost
        }
        return Loaded.Found(Account(nexus, prefs.getString(USER, "").orEmpty(), prefs.getString(DEVICE, "").orEmpty(), token))
    }

    /**
     * Keeps [account], its token under the locked key if [lock] (falling back
     * to the plain key when that cannot be used now: no screen lock, or not
     * unlocked lately). True if it went under the key asked for.
     */
    fun save(account: Account, lock: Boolean): Boolean {
        val (sealed, underLock) = sealFor(account.token, lock)
        prefs.edit {
            putString(NEXUS, account.nexus)
            putString(USER, account.username)
            putString(DEVICE, account.device)
            putString(TOKEN, sealed)
            putString(KEY, if (underLock) LOCKED else PLAIN)
            putString(LAST_NEXUS, account.nexus)
            putString(LAST_USER, account.username)
            remove(SIGNED_OUT)
        }
        if (underLock) runCatching { plain.delete() } else if (!lock) runCatching { locked.delete() }
        return underLock == lock
    }

    /**
     * Moves the token under the key [lock] asks for, if it is not there yet
     * (a token kept the old way, the lock turned on or off). Call just
     * after an unlock: the locked key works only then. True when it is
     * under that key now.
     */
    fun reseal(lock: Boolean): Boolean {
        if (isLocked() == lock) return true
        val account = (load() as? Loaded.Found)?.account ?: return false
        return save(account, lock)
    }

    private fun sealFor(token: String, lock: Boolean): Pair<String, Boolean> {
        if (lock) {
            runCatching { locked.seal(token) }.getOrNull()?.let { return it to true }
        }
        return plain.seal(token) to false
    }

    /** The last nexus address, to fill in the sign-in form. */
    fun lastNexus(): String = prefs.getString(LAST_NEXUS, "").orEmpty()

    /** The last user name, to fill in the sign-in form. */
    fun lastUser(): String = prefs.getString(LAST_USER, "").orEmpty()

    fun rememberNexus(nexus: String) = prefs.edit { putString(LAST_NEXUS, nexus) }

    /** Why this phone was signed out (the nexus's words), until it signs in again. */
    fun signedOutWhy(): String? = prefs.getString(SIGNED_OUT, null)

    /**
     * Forgets the account and its keys, keeping the nexus address and user
     * name for the sign-in form, and [why] it ended, if it was not asked for.
     */
    fun wipe(why: String? = null) {
        prefs.edit {
            prefs.getString(NEXUS, null)?.let { putString(LAST_NEXUS, it) }
            prefs.getString(USER, null)?.takeIf { it.isNotEmpty() }?.let { putString(LAST_USER, it) }
            remove(NEXUS)
            remove(USER)
            remove(DEVICE)
            remove(TOKEN)
            remove(KEY)
            if (why != null) putString(SIGNED_OUT, why) else remove(SIGNED_OUT)
        }
        runCatching { plain.delete() }
        runCatching { locked.delete() }
    }

    private companion object {
        const val PLAIN_ALIAS = "shed-device-token"
        const val LOCKED_ALIAS = "shed-device-token-locked"
        const val NEXUS = "nexus"
        const val USER = "username"
        const val DEVICE = "device"
        const val TOKEN = "token"
        /** Which key the token is under: [PLAIN] (also when not said: the old way) or [LOCKED]. */
        const val KEY = "tokenKey"
        const val PLAIN = "plain"
        const val LOCKED = "locked"
        const val LAST_NEXUS = "lastNexus"
        const val LAST_USER = "lastUser"
        const val SIGNED_OUT = "signedOut"
    }
}

/**
 * An AES-GCM key in the Android Keystore. [authenticated]: usable only for a
 * few seconds after the user unlocks (a strong biometric or the screen lock);
 * it cannot be made without a screen lock, and is lost if the screen lock is
 * removed. Adding a fingerprint does not lose it.
 */
class KeystoreSealer(private val alias: String, private val authenticated: Boolean) : Sealer {
    private fun keyStore(): KeyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }

    private fun key(): SecretKey {
        (keyStore().getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
        if (authenticated) {
            spec.setUserAuthenticationRequired(true)
            spec.setInvalidatedByBiometricEnrollment(false)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                spec.setUserAuthenticationParameters(
                    UNLOCKED_FOR_SECONDS,
                    KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL,
                )
            } else {
                @Suppress("DEPRECATION")
                spec.setUserAuthenticationValidityDurationSeconds(UNLOCKED_FOR_SECONDS)
            }
        }
        generator.init(spec.build())
        return generator.generateKey()
    }

    override fun seal(text: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(text.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    override fun open(sealed: String): String {
        val bytes = Base64.decode(sealed, Base64.NO_WRAP)
        val key = keyStore().getKey(alias, null) as? SecretKey ?: throw java.security.UnrecoverableKeyException("no key")
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, bytes, 0, IV_SIZE))
        return String(cipher.doFinal(bytes, IV_SIZE, bytes.size - IV_SIZE), Charsets.UTF_8)
    }

    override fun delete() {
        keyStore().deleteEntry(alias)
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_SIZE = 12
        /** How long after an unlock the locked key works: the token is read just then. */
        const val UNLOCKED_FOR_SECONDS = 30
    }
}

enum class ThemeChoice { System, Dark, Light }

/** The app's own settings. */
class Settings(private val prefs: SharedPreferences) {
    constructor(context: Context) : this(context.getSharedPreferences("settings", Context.MODE_PRIVATE))

    /** When the app locks; on, after 5 minutes away, unless turned off. */
    var lockAfter: LockAfter
        get() = LockAfter.of(prefs.getString("lockAfter", null))
        set(value) = prefs.edit { putString("lockAfter", value.name) }

    var theme: ThemeChoice
        get() = runCatching { ThemeChoice.valueOf(prefs.getString("theme", null) ?: "Dark") }.getOrDefault(ThemeChoice.Dark)
        set(value) = prefs.edit { putString("theme", value.name) }

    /** The push endpoint the distributor gave, and its keys (sent to the nexus). */
    var pushEndpoint: String?
        get() = prefs.getString("pushEndpoint", null)
        set(value) = prefs.edit { putString("pushEndpoint", value) }

    /** An endpoint that came while locked, as `url key auth`: sent to the nexus once unlocked. */
    var pendingEndpoint: String?
        get() = prefs.getString("pendingEndpoint", null)
        set(value) = prefs.edit { putString("pendingEndpoint", value) }

    var pushEnabled: Boolean
        get() = prefs.getBoolean("pushEnabled", false)
        set(value) = prefs.edit { putBoolean("pushEnabled", value) }
}
