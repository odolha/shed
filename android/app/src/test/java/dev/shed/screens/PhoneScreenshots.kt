package dev.shed.screens

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.navigation.compose.rememberNavController
import com.github.takahirom.roborazzi.captureScreenRoboImage
import dev.shed.api.Bench
import dev.shed.api.Call
import dev.shed.api.ClosedBench
import dev.shed.api.Cursor
import dev.shed.api.Event
import dev.shed.api.FolderEntry
import dev.shed.api.LogEntry
import dev.shed.api.Pane
import dev.shed.api.Policy
import dev.shed.api.ProjectTab
import dev.shed.api.Reply
import dev.shed.api.Screen
import dev.shed.api.ScreenLine
import dev.shed.api.ScreenRow
import dev.shed.api.Span
import dev.shed.api.Tab
import dev.shed.api.TabKind
import dev.shed.api.Topic
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import dev.shed.data.ThemeChoice
import dev.shed.ui.LocalSession
import dev.shed.ui.common.nowSeconds
import dev.shed.ui.logbook.LogbookScreen
import dev.shed.ui.project.ProjectScreen
import dev.shed.ui.terminal.TerminalScreen
import dev.shed.ui.theme.ShedTheme
import dev.shed.ui.workspaces.WorkspacesScreen
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Whole screens against a pretend nexus, at a phone's size: the long-press
 * menus (benches, the logbook, project files) and selecting in a terminal.
 * Pictures to look at, not compared with anything.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xxhdpi")
class PhoneScreenshots {
    @get:Rule val compose = createComposeRule()

    private val now = nowSeconds()
    private val home = "/home/you"
    private val shell = Tab("t1", title = "shell", kind = TabKind.Terminal, panes = listOf(Pane("p1", "$home/shed", 80u, 24u)), activePane = "p1")
    private val project = ProjectTab("$home/shed")
    private val projectTab = Tab("t2", title = "shed", kind = TabKind.Project(project))
    private val bench = Bench("b1", "shed", "blue", cwd = "$home/shed", tabs = listOf(shell, projectTab), activeTab = "t1")
    private val state = WorkspaceState(
        host = "laptop",
        benches = listOf(bench, Bench("b2", "notes", "green", cwd = "$home/notes", tabs = listOf(shell.copy(id = "t3")), activeTab = "t3")),
        closed = listOf(ClosedBench("website", "coral", cwd = "$home/site", tabs = emptyList(), saved = (now - 86_400).toUInt())),
        home = home,
    )

    private val log = listOf(
        LogEntry("cargo test -p shed-save closed_benches", "$home/shed", 0, (now - 300).toUInt(), 12.4, "b1", "t1"),
        LogEntry("git log --oneline -5", "$home/shed", 0, (now - 900).toUInt(), 0.1, "b1", "t1"),
        LogEntry("./gradlew assembleDebug", "$home/shed/android", 1, (now - 3600).toUInt(), 48.0, "b1", "t1"),
        LogEntry("rg -n \"fn rewrap\" crates", "$home/shed", 0, (now - 7200).toUInt(), 0.3, "b1", "t1"),
    )

    private val files = listOf(
        FolderEntry("android", true), FolderEntry("crates", true), FolderEntry("docs", true), FolderEntry("target", true, ignored = true),
        FolderEntry("Cargo.toml", false), FolderEntry("CLAUDE.md", false), FolderEntry("README.md", false),
    )

    private fun line(text: String, wraps: Boolean = false) = ScreenLine(listOf(Span(text)), wraps)

    private val normal = Screen(
        full = true, cols = 80u, rows = 8u,
        lines = listOf(
            line("you@shed:~/shed$ cargo test -p shed-save closed_benches"),
            line("    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.21s"),
            line("     Running unittests src/lib.rs (target/debug/deps/shed_save-6f1c2a)"),
            line("running 2 tests"),
            line("test bench::tests::closed_benches_change_tab_by_tab ... ok"),
            line("test result: ok. 2 passed; 0 failed; 0 ignored; finished in 0.01s"),
            line("you@shed:~/shed$ "),
        ).mapIndexed { i, l -> ScreenRow(i.toUInt(), l) },
        cursor = Cursor(6u, 17u, true),
    )

    private val alt = Screen(
        full = true, cols = 60u, rows = 10u, alt = true,
        lines = listOf(
            line("  PID USER      PRI  NI  VIRT   RES  CPU% MEM%  Command"),
            line(" 4211 you        20   0 1.2G  210M  12.0  2.6  shedd --foreground"),
            line(" 4302 you        20   0 3.4G  640M   8.5  8.0  shed"),
            line(" 5120 you        20   0  20M  6.1M   0.7  0.1  bash"),
            line(" 5188 you        20   0 890M  120M   0.3  1.5  claude"),
        ).mapIndexed { i, l -> ScreenRow(i.toUInt(), l) },
        cursor = Cursor(0u, 0u, false),
    )

    private var nexus: FakeNexus? = null

    @After fun tearDown() { nexus?.close() }

    private fun shoot(name: String, screen: Screen = normal, content: @Composable () -> Unit, then: () -> Unit) {
        val fake = FakeNexus(
            WorkspaceInfo("w1", "laptop", true, now.toUInt(), Policy.Full, "0.9.0"),
            state,
            answer = { call ->
                when (call) {
                    is Call.LogSearch -> Reply.Log(log)
                    is Call.ListFolder -> Reply.Folder(if (call.content.path.isEmpty()) files else emptyList())
                    else -> Reply.Done
                }
            },
            events = { topic -> if (topic is Topic.Screen) listOf(Event.Screen(screen)) else emptyList() },
        )
        nexus = fake
        compose.setContent {
            CompositionLocalProvider(LocalSession provides fake.session) {
                ShedTheme(ThemeChoice.Dark) { content() }
            }
        }
        then()
        compose.waitForIdle()
        captureScreenRoboImage(screenshot(name))
    }

    private fun waitFor(text: String) {
        compose.waitUntil(10_000) { compose.onAllNodes(hasText(text, substring = true)).fetchSemanticsNodes().isNotEmpty() }
    }

    private fun longPress(text: String) {
        waitFor(text)
        compose.onNodeWithText(text, substring = true).performTouchInput { longClick() }
    }

    @Test fun benchMenu() = shoot("menu-bench-dark", content = { WorkspacesScreen(rememberNavController()) }) { longPress("notes") }

    @Test fun closedBenchMenu() = shoot("menu-closed-bench-dark", content = { WorkspacesScreen(rememberNavController()) }) {
        waitFor("closed bench")
        compose.onNodeWithText("closed bench", substring = true).performClick()
        longPress("website")
    }

    @Test fun logbookMenu() = shoot("menu-logbook-dark", content = { LogbookScreen(rememberNavController(), "w1", "t1") }) { longPress("git log") }

    @Test fun fileMenu() = shoot("menu-file-dark", content = { ProjectScreen(rememberNavController(), "w1", bench, projectTab, project) }) { longPress("Cargo.toml") }

    private fun pressTerminal(at: Offset) {
        waitFor("closed_benches_change")
        compose.onNode(hasScrollToIndexAction()).performTouchInput { longClick(at) }
    }

    /** A long press on a word: it is selected, with handles and Copy. */
    @Test fun terminalSelect() = shoot("terminal-select-dark", content = { TerminalScreen(rememberNavController(), "w1", bench, shell) }) {
        // Row 4 of the re-wrapped screen, a few cells in: "bench::tests::…" (16 px rows at xxhdpi = 48 px).
        pressTerminal(Offset(150f, 48f * 5.5f))
    }

    /** The same grown to whole lines: a wrapped line comes whole. */
    @Test fun terminalSelectLines() = shoot("terminal-select-lines-dark", content = { TerminalScreen(rememberNavController(), "w1", bench, shell) }) {
        pressTerminal(Offset(150f, 48f * 5.5f))
        compose.onNodeWithText("Lines").performClick()
    }

    @Test fun terminalSelectAlt() = shoot("terminal-select-alt-dark", alt, content = { TerminalScreen(rememberNavController(), "w1", bench, shell) }) {
        waitFor("shedd --foreground")
        compose.onNodeWithText("shedd --foreground", substring = true).performTouchInput { longClick(Offset(centerX, centerY)) }
    }
}
