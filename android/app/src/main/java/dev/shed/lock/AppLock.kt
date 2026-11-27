package dev.shed.lock

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first

/** How long the app may be away before it locks; [Off] never locks. */
enum class LockAfter(val minutes: Int?) {
    Immediately(0),
    OneMinute(1),
    FiveMinutes(5),
    FifteenMinutes(15),
    Off(null),
    ;

    val millis: Long? get() = minutes?.let { it * 60_000L }

    companion object {
        /** The default: five minutes. */
        val DEFAULT = FiveMinutes

        fun of(name: String?): LockAfter = entries.firstOrNull { it.name == name } ?: DEFAULT
    }
}

/**
 * When the app is locked: at a cold start and after it was in the
 * background at least [LockAfter] (measured on [clock], a clock that keeps
 * going while the phone sleeps: `SystemClock.elapsedRealtime`). Leaving for
 * the unlock itself (the PIN screen is another app's) does not count.
 *
 * [canLock] says whether the phone has a screen lock to ask for; without
 * one the app cannot lock, and does not.
 */
class AppLock(
    after: LockAfter,
    private val clock: () -> Long,
    private val canLock: () -> Boolean,
) {
    var after: LockAfter = after
        private set

    private val _locked = MutableStateFlow(false)
    val locked: StateFlow<Boolean> = _locked.asStateFlow()

    /** When the app went to the background; null while in front. */
    private var awaySince: Long? = null
    private var unlocking = false

    /** Whether the lock is on and can be had on this phone. */
    val active: Boolean get() = after != LockAfter.Off && canLock()

    /** The app starts: locked if the lock is active and there is something to keep from view. */
    fun coldStart(signedIn: Boolean) {
        awaySince = null
        _locked.value = signedIn && active
    }

    fun background() {
        if (!unlocking && awaySince == null) awaySince = clock()
    }

    /** Back in front: locks if away long enough. True when locked. */
    fun foreground(): Boolean {
        val since = awaySince
        awaySince = null
        // The screen lock was removed meanwhile: there is nothing to ask for.
        if (!canLock()) _locked.value = false
        val limit = after.millis
        if (since != null && limit != null && canLock() && clock() - since >= limit) _locked.value = true
        return _locked.value
    }

    /** The unlock prompt is up (its PIN screen may take the app to the background). */
    fun unlocking(on: Boolean) {
        unlocking = on
        if (on) awaySince = null
    }

    fun unlocked() {
        unlocking = false
        awaySince = null
        _locked.value = false
    }

    /** Changes when it locks (turning it off is for after an unlock). */
    fun set(after: LockAfter) {
        this.after = after
        if (after == LockAfter.Off) _locked.value = false
    }

    /** Waits while locked: nothing reaches the nexus until the unlock. */
    suspend fun awaitUnlocked() {
        locked.first { !it }
    }
}
