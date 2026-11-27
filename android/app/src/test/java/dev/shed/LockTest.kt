package dev.shed

import dev.shed.lock.AppLock
import dev.shed.lock.LockAfter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the app lock locks: cold starts, time away, the unlock's own trip away. */
class LockTest {
    private var now = 1_000_000L
    private var screenLock = true

    private fun lock(after: LockAfter = LockAfter.FiveMinutes) = AppLock(after, { now }, { screenLock })

    private fun away(lock: AppLock, millis: Long): Boolean {
        lock.background()
        now += millis
        return lock.foreground()
    }

    @Test
    fun aColdStartLocksWhenSignedIn() {
        val lock = lock()
        lock.coldStart(signedIn = true)
        assertTrue(lock.locked.value)
        lock.unlocked()
        assertFalse(lock.locked.value)
        // Nothing to keep from view yet: the sign-in shows.
        val fresh = lock()
        fresh.coldStart(signedIn = false)
        assertFalse(fresh.locked.value)
    }

    @Test
    fun itLocksAfterTheTimeAway() {
        val lock = lock()
        lock.coldStart(signedIn = true)
        lock.unlocked()
        assertFalse(away(lock, 4 * 60_000L))
        assertFalse(away(lock, 4 * 60_000L), "the time away does not add up across trips")
        assertTrue(away(lock, 5 * 60_000L))
        // Stays locked until unlocked, however soon it comes back.
        assertTrue(away(lock, 1000L))
        lock.unlocked()
        assertFalse(lock.locked.value)
    }

    @Test
    fun immediatelyLocksOnAnyTripAway() {
        val lock = lock(LockAfter.Immediately)
        lock.coldStart(signedIn = true)
        lock.unlocked()
        assertTrue(away(lock, 0L))
    }

    @Test
    fun theUnlockItselfIsNotATripAway() {
        val lock = lock(LockAfter.Immediately)
        lock.coldStart(signedIn = true)
        // The PIN screen is another app's: the app goes to the background.
        lock.unlocking(true)
        lock.background()
        now += 20_000L
        lock.foreground()
        lock.unlocked()
        assertFalse(lock.locked.value)
        // A prompt given up (cancelled) counts no time away either.
        lock.unlocking(true)
        assertFalse(away(lock, 60_000L))
        lock.unlocking(false)
        assertTrue(away(lock, 0L))
    }

    @Test
    fun offNeverLocks() {
        val lock = lock(LockAfter.Off)
        lock.coldStart(signedIn = true)
        assertFalse(lock.locked.value)
        assertFalse(away(lock, 3_600_000L))
        assertFalse(lock.active)
        // Turned on later: from then on.
        lock.set(LockAfter.OneMinute)
        assertTrue(lock.active)
        assertTrue(away(lock, 60_000L))
        // Turning it off (after an unlock) unlocks.
        lock.set(LockAfter.Off)
        assertFalse(lock.locked.value)
    }

    @Test
    fun withoutAScreenLockItCannotLock() {
        screenLock = false
        val lock = lock()
        lock.coldStart(signedIn = true)
        assertFalse(lock.locked.value)
        assertFalse(lock.active)
        assertFalse(away(lock, 3_600_000L))
        // Locked, then the screen lock removed while away: nothing to ask for.
        screenLock = true
        assertTrue(away(lock, 10 * 60_000L))
        screenLock = false
        assertFalse(away(lock, 1000L))
    }

    @Test
    fun settingsKeepTheChoice() {
        assertEquals(LockAfter.FiveMinutes, LockAfter.of(null))
        assertEquals(LockAfter.FiveMinutes, LockAfter.of("nonsense"))
        assertEquals(LockAfter.Off, LockAfter.of("Off"))
        assertEquals(15 * 60_000L, LockAfter.FifteenMinutes.millis)
        assertEquals(null, LockAfter.Off.millis)
    }

    private fun assertFalse(value: Boolean, message: String) = org.junit.Assert.assertFalse(message, value)
}
