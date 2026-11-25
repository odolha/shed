package dev.shed.ui.ai

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.CreateNewFolder
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shed.R
import dev.shed.api.AiInfo
import dev.shed.api.AiSettings
import dev.shed.api.AiTab
import dev.shed.ui.bench.FolderPicker
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.tildePath
import dev.shed.ui.theme.LocalShed

/** Claude Code's mode that lets it do everything without asking. */
const val FULL_AUTO = "bypassPermissions"

/** One choice of the sheet: its value, its name (`null`: the sheet's default name) and whether it is the tab's. */
data class SettingChoice(val value: String, val name: String?, val detail: String?, val selected: Boolean)

/** What the AI tab's settings sheet offers, from the tab and what Claude Code says it has. */
data class AiSettingsChoices(
    val models: List<SettingChoice>,
    /** Empty when the model has no efforts to choose; the first is the model's own (`""`). */
    val efforts: List<SettingChoice>,
    val modes: List<SettingChoice>,
    val follow: Boolean,
    val browser: Boolean,
    val folders: List<String>,
)

/** The sheet's choices for [ai]: its model ("default" when none), effort, mode, switches and folders. */
fun settingsChoices(ai: AiTab, info: AiInfo): AiSettingsChoices {
    val current = ai.model ?: "default"
    val model = info.models.firstOrNull { it.value == current }
    val models = info.models.map { SettingChoice(it.value, it.name, it.description?.takeIf { d -> d.isNotBlank() }, it.value == current) }
        .ifEmpty { listOf(SettingChoice(current, ai.model, null, true)) }
    val effort = ai.effort.orEmpty()
    val efforts = model?.efforts.orEmpty().takeIf { it.isNotEmpty() }?.let { list ->
        listOf(SettingChoice("", null, null, effort.isEmpty())) + list.map { SettingChoice(it, it, null, it == effort) }
    }.orEmpty()
    val known = MODES.map { it.first }
    val modes = known.map { SettingChoice(it, null, null, it == ai.mode) } +
        // A mode this version does not know, as the workspace says it.
        listOfNotNull(ai.mode.takeIf { it !in known }?.let { SettingChoice(it, it, null, true) })
    return AiSettingsChoices(models, efforts, modes, ai.follow == true, ai.browser == true, ai.addDirs.orEmpty())
}

/** The change picking model [value] makes: "default" goes back to the default (`""`). */
fun pickModel(tab: String, value: String): AiSettings = AiSettings(tab, model = if (value == "default") "" else value)

/** What picking a mode does: nothing (it is the tab's), asks first (Full Auto), or the change. */
sealed interface ModePick {
    data object Same : ModePick
    data object AskFirst : ModePick
    data class Change(val settings: AiSettings) : ModePick
}

fun pickMode(tab: String, current: String, value: String): ModePick = when (value) {
    current -> ModePick.Same
    FULL_AUTO -> ModePick.AskFirst
    else -> ModePick.Change(AiSettings(tab, mode = value))
}

/** The folders with [path] added (once). */
fun withFolder(folders: List<String>, path: String): List<String> =
    if (path.isBlank() || folders.any { it.trimEnd('/') == path.trimEnd('/') }) folders else folders + path

/**
 * The AI tab's settings, from the icon beside Send: the model, its effort,
 * the mode (Full Auto asks first), following edits, the browser and the
 * folders Claude works in besides the tab's.
 */
@Composable
fun AiSettingsSheet(
    ws: String,
    tab: String,
    ai: AiTab,
    info: AiInfo,
    home: String,
    set: (AiSettings) -> Unit,
    onDismiss: () -> Unit,
) {
    val shed = LocalShed.current
    val choices = settingsChoices(ai, info)
    var fullAuto by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = 12.dp)) {
            SectionTitle(stringResource(R.string.ai_model))
            for (m in choices.models) {
                ChoiceRow(m.name ?: stringResource(R.string.ai_default_model), m.detail, m.selected) {
                    if (!m.selected) set(pickModel(tab, m.value))
                }
            }
            if (choices.efforts.isNotEmpty()) {
                SectionTitle(stringResource(R.string.ai_effort_title))
                FlowRow(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (e in choices.efforts) {
                        FilterChip(
                            selected = e.selected,
                            onClick = { if (!e.selected) set(AiSettings(tab, effort = e.value)) },
                            label = { Text(e.name ?: stringResource(R.string.ai_effort_model)) },
                            leadingIcon = if (e.selected) ({ Icon(Icons.Default.Check, null, Modifier.size(18.dp)) }) else null,
                            colors = FilterChipDefaults.filterChipColors(
                                selectedContainerColor = shed.accent.copy(alpha = 0.22f),
                                selectedLabelColor = shed.text,
                                selectedLeadingIconColor = shed.ink(shed.accent),
                            ),
                        )
                    }
                }
            }
            SectionTitle(stringResource(R.string.ai_mode_title))
            for (m in choices.modes) {
                val danger = m.value == FULL_AUTO
                val name = MODES.firstOrNull { it.first == m.value }?.second?.let { stringResource(it) } ?: m.name.orEmpty()
                ChoiceRow(
                    name + if (danger && !m.selected) "…" else "",
                    null,
                    m.selected,
                    color = if (danger) shed.danger else null,
                ) {
                    when (val pick = pickMode(tab, ai.mode, m.value)) {
                        ModePick.Same -> {}
                        ModePick.AskFirst -> fullAuto = true
                        is ModePick.Change -> set(pick.settings)
                    }
                }
            }
            SectionTitle(stringResource(R.string.ai_while_it_works))
            SwitchRow(stringResource(R.string.ai_follow), stringResource(R.string.ai_follow_detail), choices.follow) {
                set(AiSettings(tab, follow = it))
            }
            SwitchRow(stringResource(R.string.ai_browser), stringResource(R.string.ai_browser_detail), choices.browser) {
                set(AiSettings(tab, browser = it))
            }
            SectionTitle(stringResource(R.string.ai_folders))
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Default.Folder, null, tint = shed.textDim, modifier = Modifier.size(20.dp))
                HSpace(12)
                Text(tildePath(ai.folder, home), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text(stringResource(R.string.ai_folder_main), color = shed.textDim, fontSize = 12.sp)
            }
            for (dir in choices.folders) {
                Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Default.Folder, null, tint = shed.textDim, modifier = Modifier.size(20.dp))
                    HSpace(12)
                    Text(tildePath(dir, home), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    IconButton(onClick = { set(AiSettings(tab, addDirs = choices.folders - dir)) }) {
                        Icon(Icons.Default.Close, stringResource(R.string.ai_folder_remove, dir))
                    }
                }
            }
            TextButton(onClick = { picking = true }, modifier = Modifier.padding(horizontal = 8.dp)) {
                Icon(Icons.Default.CreateNewFolder, null)
                HSpace(8)
                Text(stringResource(R.string.ai_folder_add))
            }
            Text(
                stringResource(R.string.ai_folders_note),
                color = shed.textDim,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
        }
    }
    if (fullAuto) {
        ConfirmDialog(
            title = stringResource(R.string.mode_full_auto_title),
            text = stringResource(R.string.mode_full_auto_text),
            confirm = stringResource(R.string.mode_full_auto),
            onConfirm = { set(AiSettings(tab, mode = FULL_AUTO)) },
            onDismiss = { fullAuto = false },
        )
    }
    if (picking) {
        FolderPicker(
            ws = ws,
            start = ai.folder,
            home = home,
            title = stringResource(R.string.ai_folder_add_title),
            onPick = { path -> withFolder(choices.folders, path).takeIf { it != choices.folders }?.let { set(AiSettings(tab, addDirs = it)) } },
            onDismiss = { picking = false },
        )
    }
}

@Composable
private fun ChoiceRow(name: String, detail: String?, selected: Boolean, color: Color? = null, onClick: () -> Unit) {
    val shed = LocalShed.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(
            selected = selected,
            onClick = null,
            colors = if (color != null) RadioButtonDefaults.colors(selectedColor = color, unselectedColor = color) else RadioButtonDefaults.colors(),
        )
        HSpace(12)
        Column(Modifier.weight(1f)) {
            Text(name, color = color ?: shed.text, style = MaterialTheme.typography.bodyLarge)
            if (detail != null) Text(detail, color = shed.textDim, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun SwitchRow(name: String, detail: String, on: Boolean, onChange: (Boolean) -> Unit) {
    val shed = LocalShed.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .toggleable(value = on, role = Role.Switch, onValueChange = onChange)
            .padding(horizontal = 16.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge)
            Text(detail, color = shed.textDim, style = MaterialTheme.typography.bodySmall)
        }
        HSpace(12)
        Switch(checked = on, onCheckedChange = null)
    }
}
