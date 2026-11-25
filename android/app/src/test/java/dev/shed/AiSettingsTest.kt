package dev.shed

import dev.shed.api.AiActivity
import dev.shed.api.AiInfo
import dev.shed.api.AiModel
import dev.shed.api.AiSettings
import dev.shed.api.AiTab
import dev.shed.ui.ai.FULL_AUTO
import dev.shed.ui.ai.ModePick
import dev.shed.ui.ai.pickMode
import dev.shed.ui.ai.pickModel
import dev.shed.ui.ai.settingsChoices
import dev.shed.ui.ai.withFolder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The AI tab's settings sheet: what it offers and what each choice changes. */
class AiSettingsTest {
    private val info = AiInfo(
        commands = emptyList(),
        models = listOf(
            AiModel("default", "Default", "Opus 5.5 · Best for everyday work", listOf("low", "medium", "high")),
            AiModel("sonnet", "Sonnet", "For quick work"),
        ),
    )

    private fun tab(model: String? = null, mode: String = "auto", effort: String? = null, follow: Boolean? = null, dirs: List<String>? = null) =
        AiTab(folder = "/home/you/shed", addDirs = dirs, model = model, mode = mode, effort = effort, activity = AiActivity.Idle, follow = follow)

    @Test
    fun noModelIsTheDefaultOne() {
        val c = settingsChoices(tab(), info)
        assertEquals(listOf(true, false), c.models.map { it.selected })
        assertEquals("Opus 5.5 · Best for everyday work", c.models[0].detail)
        assertNull(c.models[1].detail?.takeIf { it.isBlank() })
    }

    @Test
    fun effortsAreTheModelsWithItsOwnFirst() {
        val c = settingsChoices(tab(effort = "high"), info)
        assertEquals(listOf("", "low", "medium", "high"), c.efforts.map { it.value })
        assertEquals(listOf(false, false, false, true), c.efforts.map { it.selected })
        assertTrue(settingsChoices(tab(), info).efforts.first().selected)
        // A model without efforts: none to choose.
        assertTrue(settingsChoices(tab(model = "sonnet"), info).efforts.isEmpty())
    }

    @Test
    fun modes() {
        val c = settingsChoices(tab(mode = "plan"), info)
        assertEquals(listOf("default", "acceptEdits", "auto", "plan", FULL_AUTO), c.modes.map { it.value })
        assertEquals("plan", c.modes.single { it.selected }.value)
        // A mode this version does not know shows as the workspace says it.
        val newer = settingsChoices(tab(mode = "dontAsk"), info)
        assertEquals("dontAsk", newer.modes.last().name)
        assertTrue(newer.modes.last().selected)
    }

    @Test
    fun fullAutoAsksFirst() {
        assertEquals(ModePick.AskFirst, pickMode("t", "auto", FULL_AUTO))
        assertEquals(ModePick.Same, pickMode("t", "auto", "auto"))
        assertEquals(ModePick.Change(AiSettings("t", mode = "plan")), pickMode("t", "auto", "plan"))
        // Leaving Full Auto does not ask.
        assertEquals(ModePick.Change(AiSettings("t", mode = "default")), pickMode("t", FULL_AUTO, "default"))
    }

    @Test
    fun theDefaultModelIsSentAsEmpty() {
        assertEquals(AiSettings("t", model = ""), pickModel("t", "default"))
        assertEquals(AiSettings("t", model = "sonnet"), pickModel("t", "sonnet"))
    }

    @Test
    fun switchesAndFolders() {
        val c = settingsChoices(tab(follow = true, dirs = listOf("/srv/data")), info)
        assertTrue(c.follow)
        assertFalse(c.browser)
        assertEquals(listOf("/srv/data"), c.folders)
        assertEquals(listOf("/srv/data", "/srv/more"), withFolder(c.folders, "/srv/more"))
        // Once only, with or without a trailing slash.
        assertEquals(listOf("/srv/data"), withFolder(c.folders, "/srv/data/"))
        assertEquals(listOf("/srv/data"), withFolder(c.folders, " "))
    }
}
