package dev.shed.data

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
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

/**
 * Keeps the account. The device token is encrypted with an AES-GCM key that
 * lives in the Android Keystore (it never leaves it); the rest is plain.
 * Nothing here is backed up (see `data_extraction_rules.xml`).
 */
class AccountStore(context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences("account", Context.MODE_PRIVATE)

    fun load(): Account? {
        val nexus = prefs.getString(NEXUS, null) ?: return null
        val sealed = prefs.getString(TOKEN, null) ?: return null
        val token = runCatching { open(sealed) }.getOrNull() ?: return null
        return Account(nexus, prefs.getString(USER, "").orEmpty(), prefs.getString(DEVICE, "").orEmpty(), token)
    }

    fun save(account: Account) {
        prefs.edit {
            putString(NEXUS, account.nexus)
            putString(USER, account.username)
            putString(DEVICE, account.device)
            putString(TOKEN, seal(account.token))
        }
    }

    /** The last nexus address typed, to fill in the sign-in form. */
    fun lastNexus(): String = prefs.getString(LAST_NEXUS, "").orEmpty()

    fun rememberNexus(nexus: String) = prefs.edit { putString(LAST_NEXUS, nexus) }

    /** Forgets the account and its key. */
    fun wipe() {
        prefs.edit {
            remove(NEXUS)
            remove(USER)
            remove(DEVICE)
            remove(TOKEN)
        }
        runCatching { keyStore().deleteEntry(KEY_ALIAS) }
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }

    private fun key(): SecretKey {
        (keyStore().getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun seal(text: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(text.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    private fun open(sealed: String): String {
        val bytes = Base64.decode(sealed, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, IV_SIZE))
        return String(cipher.doFinal(bytes, IV_SIZE, bytes.size - IV_SIZE), Charsets.UTF_8)
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "shed-device-token"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_SIZE = 12
        const val NEXUS = "nexus"
        const val USER = "username"
        const val DEVICE = "device"
        const val TOKEN = "token"
        const val LAST_NEXUS = "lastNexus"
    }
}

enum class ThemeChoice { System, Dark, Light }

/** The app's own settings. */
class Settings(context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var biometricLock: Boolean
        get() = prefs.getBoolean("biometricLock", false)
        set(value) = prefs.edit { putBoolean("biometricLock", value) }

    var theme: ThemeChoice
        get() = runCatching { ThemeChoice.valueOf(prefs.getString("theme", null) ?: "Dark") }.getOrDefault(ThemeChoice.Dark)
        set(value) = prefs.edit { putString("theme", value.name) }

    /** The push endpoint the distributor gave, and its keys (sent to the nexus). */
    var pushEndpoint: String?
        get() = prefs.getString("pushEndpoint", null)
        set(value) = prefs.edit { putString("pushEndpoint", value) }

    var pushEnabled: Boolean
        get() = prefs.getBoolean("pushEnabled", false)
        set(value) = prefs.edit { putBoolean("pushEnabled", value) }
}
