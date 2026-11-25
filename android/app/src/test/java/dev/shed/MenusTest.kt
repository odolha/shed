package dev.shed

import dev.shed.api.BenchRef
import dev.shed.api.Call
import dev.shed.api.CommandTab
import dev.shed.api.LogEntry
import dev.shed.api.NewTab
import dev.shed.api.NewTabKind
import dev.shed.ui.logbook.LogMenu
import dev.shed.ui.logbook.logMenu
import dev.shed.ui.logbook.runAgain
import dev.shed.ui.logbook.runBench
import dev.shed.ui.project.FileMenu
import dev.shed.ui.project.fileMenu
import dev.shed.ui.project.fullPath
import dev.shed.ui.workspaces.BenchMenu
import dev.shed.ui.workspaces.benchMenu
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** What the long-press menus offer, and what their actions send. */
class MenusTest {
    private val entry = LogEntry("cargo test -p shed-save", "/home/you/shed", 0, 1_763_500_000u, 4.2, bench = "b1", tab = "t1")

    @Test
    fun logbookOffersRunningOnlyWithABench() {
        assertEquals(listOf(LogMenu.CopyCommand, LogMenu.CopyFolder, LogMenu.Run), logMenu(canRun = true))
        assertEquals(listOf(LogMenu.CopyCommand, LogMenu.CopyFolder), logMenu(canRun = false))
    }

    @Test
    fun runsInItsOwnBenchElseTheOneItWasOpenedFrom() {
        assertEquals("b1", runBench(entry, "b2", listOf("b1", "b2")))
        // Its bench is closed now: the one the logbook came from.
        assertEquals("b2", runBench(entry, "b2", listOf("b2")))
        assertNull(runBench(entry, null, listOf("b3")))
        assertNull(runBench(entry.copy(bench = null), "b9", listOf("b1")))
    }

    @Test
    fun runningAgainOpensACommandTabInItsFolder() {
        assertEquals(
            Call.NewTab(NewTab(BenchRef.Running("b1"), NewTabKind.Command(CommandTab("/home/you/shed", "cargo test -p shed-save")))),
            runAgain(entry, "b1"),
        )
    }

    @Test
    fun filesAndFolders() {
        assertEquals(listOf(FileMenu.Open, FileMenu.CopyPath, FileMenu.CopyRelative), fileMenu(dir = false, expanded = false))
        assertEquals(listOf(FileMenu.Open, FileMenu.CopyPath, FileMenu.CopyRelative), fileMenu(dir = true, expanded = false))
        assertEquals(listOf(FileMenu.Fold, FileMenu.CopyPath, FileMenu.CopyRelative), fileMenu(dir = true, expanded = true))
        assertEquals("/home/you/shed/src/main.rs", fullPath("/home/you/shed", "src/main.rs"))
        assertEquals("/home/you/shed/src", fullPath("/home/you/shed/", "/src"))
        assertEquals("/home/you/shed", fullPath("/home/you/shed", ""))
    }

    @Test
    fun benches() {
        assertEquals(listOf(BenchMenu.Open, BenchMenu.Rename, BenchMenu.Close), benchMenu(running = true, canChange = true))
        assertEquals(listOf(BenchMenu.Open, BenchMenu.Load, BenchMenu.LoadStart, BenchMenu.Rename), benchMenu(running = false, canChange = true))
        assertEquals(listOf(BenchMenu.Open), benchMenu(running = true, canChange = false))
        assertEquals(listOf(BenchMenu.Open), benchMenu(running = false, canChange = false))
        // Closing asks first, in red; nothing else does.
        assertEquals(listOf(BenchMenu.Close), BenchMenu.entries.filter { it.danger })
    }
}
