package dev.shed.screens

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeDown
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.captureScreenRoboImage
import dev.shed.api.AiActivity
import dev.shed.api.AiAssistant
import dev.shed.api.AiEntry
import dev.shed.api.AiInfo
import dev.shed.api.AiModel
import dev.shed.api.AiRun
import dev.shed.api.AiRunState
import dev.shed.api.AiStatus
import dev.shed.api.AiTab
import dev.shed.api.AiTimer
import dev.shed.api.AiTimerKind
import dev.shed.api.AiTool
import dev.shed.api.AiUser
import dev.shed.api.LimitWindow
import dev.shed.api.Limits
import dev.shed.data.ThemeChoice
import dev.shed.ui.ai.AiTabActions
import dev.shed.ui.ai.AiTabData
import dev.shed.ui.ai.AiTabView
import dev.shed.ui.common.nowSeconds
import dev.shed.ui.theme.ShedTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/** Where the pictures go: `-Pshed.screenshots=/tmp/shed-android` (build/screenshots by default). */
fun screenshot(name: String): String {
    val dir = File(System.getProperty("shed.screenshots") ?: "build/screenshots")
    dir.mkdirs()
    return File(dir, "$name.png").path
}

/**
 * The AI tab drawn at a typical phone's size (412 × 915 dp), dark and light:
 * a conversation opened at its end, the prompt focused, the limits opened,
 * the settings sheet. Pictures to look at, not compared with anything.
 */
@OptIn(ExperimentalRoborazziApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xxhdpi")
class AiScreenshots {
    @get:Rule val compose = createComposeRule()

    private val now = nowSeconds()

    private fun user(text: String) = AiEntry.User(AiUser(text, at = (now - 600).toUInt()))
    private fun answer(text: String) = AiEntry.Assistant(AiAssistant(text))
    private fun tool(name: String, summary: String, result: String? = "ok", run: AiRun? = null, detail: String? = null) =
        AiEntry.Tool(AiTool("t-$summary", name, summary, detail = detail, result = result, run = run))

    private val entries = listOf(
        user("The sign-in page shows a blank screen on Android 10. Can you look?"),
        tool("Grep", "Grep \"SignIn\" in app/src"),
        tool("Read", "Read app/src/main/java/dev/shed/ui/signin/SignInScreen.kt"),
        answer("The screen reads the **window insets** before the first layout, and on Android 10 they are still zero, so the form is laid out with no height.\n\nI'll make it wait for the first layout."),
        tool("Edit", "Edit SignInScreen.kt"),
        tool("Bash", "$ ./gradlew testDebugUnitTest"),
        answer("Fixed: the form now lays out once the insets are known.\n\n- `SignInScreen.kt`: waits for the first layout\n- a test for a zero-height start\n\nAll 84 tests pass."),
        user("Thanks. Now make the logbook search keep its query when I rotate the phone."),
        tool(
            "Agent",
            "Agent: find where the logbook keeps its query",
            result = null,
            run = AiRun(
                AiRunState.Running,
                background = true,
                agent = "Explore",
                started = (now - 95).toUInt(),
                last = "Read app/src/main/java/dev/shed/ui/logbook/LogbookScreen.kt",
                tools = 6u,
                tokens = 18_400.0,
                steps = listOf(
                    AiEntry.Tool(AiTool("s1", "Grep", "Grep \"query\" in app/src/main/java/dev/shed/ui", result = "LogbookScreen.kt:72\nLogbookScreen.kt:80")),
                    AiEntry.Tool(AiTool("s2", "Glob", "Glob **/logbook/*.kt", result = "LogbookScreen.kt")),
                    AiEntry.Assistant(AiAssistant("The query is kept in `LogbookScreen`; reading it now.")),
                    AiEntry.Tool(AiTool("s3", "Read", "Read app/src/main/java/dev/shed/ui/logbook/LogbookScreen.kt")),
                ),
            ),
            detail = "Find where the logbook screen keeps its search query and whether it survives a configuration change. Report the file and line.",
        ),
        tool("Bash", "$ ./gradlew lintDebug", result = null, run = AiRun(AiRunState.Running, background = true, started = (now - 40).toUInt(), last = "> Task :app:lintAnalyzeDebug")),
        answer("An agent looks for where the query lives; meanwhile, the search field is a plain `remember`, so a rotation drops it. Switching it to `rememberSaveable`…"),
    )

    private val ai = AiTab(
        folder = "/home/you/shed",
        addDirs = listOf("/home/you/notes"),
        mode = "auto",
        effort = "high",
        activity = AiActivity.Working,
        queued = listOf("Then run lint too"),
        follow = true,
    )

    private val status = AiStatus(
        state = AiActivity.Working,
        cost = 1.42,
        context = 84_000.0,
        contextMax = 200_000.0,
        since = (now - 75).toUInt(),
        timers = listOf(
            AiTimer(AiTimerKind.Wakeup, "Check the CI run", at = (now + 240).toUInt()),
            AiTimer(AiTimerKind.Watch, "Logs of the dev server", at = (now + 1800).toUInt()),
        ),
    )

    private val limits = Limits(
        fiveHour = LimitWindow(0.42f, (now + 2 * 3600 + 600).toUInt()),
        week = LimitWindow(0.76f, (now + 3 * 86400).toUInt()),
    )

    private val info = AiInfo(
        commands = emptyList(),
        models = listOf(
            AiModel("default", "Default (recommended)", "Opus 5.5 · Best for everyday work", listOf("low", "medium", "high", "max")),
            AiModel("sonnet", "Sonnet", "Sonnet 5 · Quick and capable", listOf("low", "medium", "high")),
            AiModel("haiku", "Haiku", "Haiku 4.5 · Fastest for quick answers"),
        ),
    )

    private val actions = AiTabActions(
        send = {},
        upload = { name, _ -> "/tmp/$name" },
        loadPicture = {},
        back = {},
        sessions = {},
        review = {},
        drafts = {},
        git = {},
        copyAll = {},
        userAction = { _, _, _ -> },
        picture = {},
        timer = {},
    )

    @Composable
    private fun Tab(theme: ThemeChoice) {
        ShedTheme(theme) {
            AiTabView(
                AiTabData(
                    ws = "w",
                    tab = "t",
                    title = "Fix the sign-in page",
                    subtitle = "shed · AI",
                    accent = Color(0xFF5C9DFF),
                    ai = ai,
                    entries = entries,
                    status = status,
                    limits = limits,
                    info = info,
                    pictures = emptyMap(),
                    ended = false,
                    canChange = true,
                    home = "/home/you",
                    drafts = 1,
                ),
                actions,
                { nowSeconds() },
            )
        }
    }

    private fun shoot(theme: ThemeChoice, name: String, then: () -> Unit = {}) {
        compose.setContent { Tab(theme) }
        compose.waitForIdle()
        then()
        compose.waitForIdle()
        captureScreenRoboImage(screenshot("ai-$name-${theme.name.lowercase()}"))
    }

    @Test fun openDark() = shoot(ThemeChoice.Dark, "open")

    @Test fun openLight() = shoot(ThemeChoice.Light, "open")

    @Test fun scrolledUpDark() = shoot(ThemeChoice.Dark, "scrolled-up") {
        compose.onNode(hasScrollToIndexAction()).performTouchInput { swipeDown(startY = top + 100f, endY = bottom - 100f) }
    }

    /** Scrolled by hand: the scrollbar shows (caught before it fades). */
    @Test fun scrollbarDark() {
        compose.setContent { Tab(ThemeChoice.Dark) }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        compose.onNode(hasScrollToIndexAction()).performTouchInput { swipeDown(startY = top + 100f, endY = centerY) }
        compose.mainClock.advanceTimeBy(250)
        captureScreenRoboImage(screenshot("ai-scrollbar-dark"))
    }

    @Test fun agentDark() = shoot(ThemeChoice.Dark, "agent") { scrollTo("find where the logbook") }

    @Test fun agentLight() = shoot(ThemeChoice.Light, "agent") { scrollTo("find where the logbook") }

    @Test fun agentSheetDark() = shoot(ThemeChoice.Dark, "agent-sheet") {
        scrollTo("find where the logbook")
        compose.onNodeWithText("find where the logbook", substring = true).performClick()
        compose.waitUntil(5_000) { compose.onAllNodes(hasText("Steps")).fetchSemanticsNodes().isNotEmpty() }
    }

    @Test fun backgroundStripDark() = shoot(ThemeChoice.Dark, "background-open") {
        compose.onNodeWithText("agent running", substring = true).performClick()
    }

    @Test fun messageMenuDark() = shoot(ThemeChoice.Dark, "message-menu") {
        scrollTo("Thanks. Now make")
        compose.onNodeWithText("Thanks. Now make", substring = true).performTouchInput { longClick() }
    }

    @Test fun messageMenuLight() = shoot(ThemeChoice.Light, "message-menu") {
        scrollTo("Thanks. Now make")
        compose.onNodeWithText("Thanks. Now make", substring = true).performTouchInput { longClick() }
    }

    @Test fun selectTextDark() = shoot(ThemeChoice.Dark, "select-text") {
        scrollTo("Thanks. Now make")
        compose.onNodeWithText("Thanks. Now make", substring = true).performTouchInput { longClick() }
        compose.onNodeWithText("Select Text…").performClick()
    }

    private fun scrollTo(text: String) {
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(text, substring = true))
        compose.waitForIdle()
    }

    @Test fun typingDark() = shoot(ThemeChoice.Dark, "typing") { type() }

    @Test fun typingLight() = shoot(ThemeChoice.Light, "typing") { type() }

    /** A long prompt: the whole screen is the prompt, its end in sight. */
    @Test fun typingLongDark() = shoot(ThemeChoice.Dark, "typing-long") {
        val field = compose.onNode(hasSetTextAction())
        field.performClick()
        field.performTextInput((1..40).joinToString("\n") { "Line $it of a long message, so it wraps on a phone's narrow screen" })
    }

    @Test fun limitsDark() = shoot(ThemeChoice.Dark, "limits") { compose.onNodeWithText("Working", substring = true).performClick() }

    @Test fun limitsLight() = shoot(ThemeChoice.Light, "limits") { compose.onNodeWithText("Working", substring = true).performClick() }

    @Test fun settingsDark() = shoot(ThemeChoice.Dark, "settings") { settings() }

    @Test fun settingsLight() = shoot(ThemeChoice.Light, "settings") { settings() }

    private fun type() {
        val field = compose.onNode(hasSetTextAction())
        field.performClick()
        field.performTextInput("Also keep the scroll position of the results, and the filter chips")
    }

    private fun settings() {
        compose.onNode(hasContentDescription("Model, mode and effort")).performClick()
        compose.waitUntil(5_000) { compose.onAllNodes(hasText("Follow edits")).fetchSemanticsNodes().isNotEmpty() }
    }
}
