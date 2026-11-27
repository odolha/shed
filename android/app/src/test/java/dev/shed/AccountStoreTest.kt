package dev.shed

import android.content.Context
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.UserNotAuthenticatedException
import org.robolectric.RuntimeEnvironment
import dev.shed.data.Account
import dev.shed.data.AccountStore
import dev.shed.data.Loaded
import dev.shed.data.Sealer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Keeping the token under the right key: a token kept the old way moves
 * under the locked key at the first unlock, back when the lock is turned
 * off; a lost key signs out, keeping the address and user name.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AccountStoreTest {
    /** A key in a pretend keystore: [unlocked] while the phone was unlocked just now. */
    private class FakeSealer(private val tag: String, private val needsUnlock: Boolean) : Sealer {
        var unlocked = false
        var invalidated = false
        var exists = false

        private fun check() {
            if (invalidated) throw KeyPermanentlyInvalidatedException()
            if (needsUnlock && !unlocked) throw UserNotAuthenticatedException()
        }

        override fun seal(text: String): String {
            check()
            exists = true
            return "$tag:$text"
        }

        override fun open(sealed: String): String {
            check()
            require(sealed.startsWith("$tag:")) { "not sealed with $tag" }
            return sealed.removePrefix("$tag:")
        }

        override fun delete() {
            exists = false
        }
    }

    private val account = Account("https://nexus.example.com", "you", "phone-1", "secret-token")
    private lateinit var prefs: android.content.SharedPreferences
    private val plain = FakeSealer("plain", needsUnlock = false)
    private val locked = FakeSealer("locked", needsUnlock = true)
    private lateinit var store: AccountStore

    @Before
    fun setUp() {
        prefs = RuntimeEnvironment.getApplication().getSharedPreferences("account-test", Context.MODE_PRIVATE)
        prefs.edit().clear().commit()
        store = AccountStore(prefs, plain, locked)
    }

    @Test
    fun aTokenKeptTheOldWayMovesUnderTheLockAtTheFirstUnlock() {
        // As an older version kept it: the plain key, nothing said of which.
        prefs.edit()
            .putString("nexus", account.nexus).putString("username", account.username)
            .putString("device", account.device).putString("token", plain.seal(account.token))
            .commit()
        assertTrue(store.has())
        assertFalse(store.isLocked())
        assertEquals(Loaded.Found(account), store.load())
        // Not unlocked: it stays where it is (no sign-out).
        assertFalse(store.reseal(lock = true))
        assertEquals(Loaded.Found(account), store.load())
        // Unlocked: moved, and the old key forgotten.
        locked.unlocked = true
        assertTrue(store.reseal(lock = true))
        assertTrue(store.isLocked())
        assertFalse(plain.exists)
        assertTrue(prefs.getString("token", "")!!.startsWith("locked:"))
        assertEquals(Loaded.Found(account), store.load())
        // Later, without an unlock, it cannot be read.
        locked.unlocked = false
        assertEquals(Loaded.NotUnlocked, store.load())
    }

    @Test
    fun signingInWithTheLockOnUsesTheLockedKeyWhenItCan() {
        // Not unlocked just now: kept under the plain key until the next unlock.
        assertFalse(store.save(account, lock = true))
        assertFalse(store.isLocked())
        locked.unlocked = true
        assertTrue(store.reseal(lock = true))
        assertTrue(store.isLocked())
        // Turning the lock off (after an unlock) moves it back.
        assertTrue(store.reseal(lock = false))
        assertFalse(store.isLocked())
        assertFalse(locked.exists)
        locked.unlocked = false
        assertEquals(Loaded.Found(account), store.load())
    }

    @Test
    fun aLostKeySignsOutButKeepsWhoAndWhere() {
        locked.unlocked = true
        assertTrue(store.save(account, lock = true))
        // The screen lock removed: the key is gone for good.
        locked.invalidated = true
        assertEquals(Loaded.KeyLost, store.load())
        store.wipe("the phone’s screen lock was removed")
        assertFalse(store.has())
        assertEquals(Loaded.None, store.load())
        assertEquals("https://nexus.example.com", store.lastNexus())
        assertEquals("you", store.lastUser())
        assertEquals("the phone’s screen lock was removed", store.signedOutWhy())
        // Signing in again forgets why.
        locked.invalidated = false
        store.save(account, lock = false)
        assertNull(store.signedOutWhy())
    }

    @Test
    fun securityAlertsOpenTheDevices() {
        assertTrue(dev.shed.ui.isDevicesLink(dev.shed.push.DEVICES_LINK))
        assertFalse(dev.shed.ui.isDevicesLink(dev.shed.push.shedLink("w1")))
        assertEquals(null, dev.shed.ui.parseLink(dev.shed.push.DEVICES_LINK))
    }

    @Test
    fun signedOutByTheNexusSaysWhy() {
        assertEquals("not used for 7 days", signedOutReason("signed out: not used for 7 days"))
        assertEquals("the sign-in is older than 60 days", signedOutReason("signed out: the sign-in is older than 60 days"))
        assertNull(signedOutReason("signed out"))
        assertEquals("not signed in", signedOutReason("not signed in"))
    }
}
