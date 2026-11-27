package dev.shed.screens

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.navigation.compose.rememberNavController
import com.github.takahirom.roborazzi.captureScreenRoboImage
import dev.shed.R
import dev.shed.api.Policy
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import dev.shed.data.ThemeChoice
import dev.shed.lock.LockAfter
import dev.shed.ui.LocalSession
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.nowSeconds
import dev.shed.ui.devices.DevicesScreen
import dev.shed.ui.lock.LockScreen
import dev.shed.ui.settings.LockSettings
import dev.shed.ui.settings.SecurityAlertsRow
import dev.shed.ui.theme.ShedTheme
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The app lock and sign-ins: the lock screen, the lock's settings (and
 * without a screen lock), security alerts in the notifications' settings,
 * and the Devices saying when each sign-in ends. Pictures to look at.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xxhdpi")
class SecurityScreenshots {
    @get:Rule val compose = createComposeRule()

    private val now = nowSeconds()
    private var nexus: FakeNexus? = null

    @After fun tearDown() { nexus?.close() }

    private fun shoot(name: String, theme: ThemeChoice = ThemeChoice.Dark, content: @Composable () -> Unit) {
        compose.setContent { ShedTheme(theme) { content() } }
        compose.waitForIdle()
        captureScreenRoboImage(screenshot(name))
    }

    @Test fun lockScreen() = shoot("lock-dark") { LockScreen(onUnlock = {}) }

    @Test fun lockScreenLight() = shoot("lock-light", ThemeChoice.Light) { LockScreen(onUnlock = {}) }

    @Composable
    private fun SettingsPart(canLock: Boolean, chosen: LockAfter) {
        ShedScaffold(title = stringResource(R.string.settings_title), onBack = {}) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                SectionTitle(stringResource(R.string.settings_notifications))
                SecurityAlertsRow()
                SectionTitle(stringResource(R.string.settings_security))
                LockSettings(canLock = canLock, chosen = chosen, onChoose = {})
            }
        }
    }

    @Test fun lockSettings() = shoot("settings-lock-dark") { SettingsPart(canLock = true, chosen = LockAfter.FiveMinutes) }

    @Test fun lockSettingsOff() = shoot("settings-lock-off-dark") { SettingsPart(canLock = true, chosen = LockAfter.Off) }

    @Test fun lockSettingsNoScreenLock() = shoot("settings-lock-none-dark") { SettingsPart(canLock = false, chosen = LockAfter.FiveMinutes) }

    @Test fun devicesSayWhenSignInsEnd() {
        val day = 86_400L
        val devices = """[
            {"id":"d1","kind":"phone","name":"Pixel 8","created":${now - 3 * day},"seen":${now - 60},"address":"203.0.113.24","country":"NL","current":true,"online":true,"expires":${now - 60 + 7 * day}},
            {"id":"d2","kind":"phone","name":"Old phone","created":${now - 50 * day},"seen":${now - 5 * day},"address":"198.51.100.4","country":"DE","current":false,"online":false,"expires":${now + 2 * day}},
            {"id":"d3","kind":"desktop","name":"Shed on laptop","created":${now - 10 * day},"seen":${now - 3600},"address":"198.51.100.9","current":false,"online":false,"expires":${now + 20 * day}},
            {"id":"w1","kind":"workspace","name":"laptop","created":${now - 90 * day},"seen":$now,"address":"198.51.100.9","current":false,"online":true}
        ]"""
        val fake = FakeNexus(
            WorkspaceInfo("w1", "laptop", true, now.toUInt(), Policy.Full, "0.9.0"),
            WorkspaceState(host = "laptop", benches = emptyList(), closed = emptyList(), home = "/home/you"),
            http = { path -> when (path) { "/devices" -> devices; "/pair/pending" -> "[]"; else -> null } },
        )
        nexus = fake
        compose.setContent {
            CompositionLocalProvider(LocalSession provides fake.session) {
                ShedTheme(ThemeChoice.Dark) { DevicesScreen(rememberNavController()) }
            }
        }
        compose.waitUntil(10_000) { compose.onAllNodes(hasText("Old phone")).fetchSemanticsNodes().isNotEmpty() }
        compose.waitForIdle()
        captureScreenRoboImage(screenshot("devices-dark"))
    }
}
