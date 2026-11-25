package dev.shed.ui.common

import androidx.annotation.StringRes
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.DpOffset
import dev.shed.ui.theme.LocalShed

/**
 * What a long-press menu offers: an icon, a label, and whether it is
 * destructive (in red; its label ends in "…" and it asks first).
 * Menus are lists of these, made by plain functions (tested on the JVM).
 */
interface MenuChoice {
    val icon: ImageVector
    @get:StringRes val label: Int
    val danger: Boolean get() = false
}

/** A long-press menu at its row: [choices] in order; picking one closes it. */
@Composable
fun <T : MenuChoice> ChoiceMenu(
    expanded: Boolean,
    choices: List<T>,
    onDismiss: () -> Unit,
    offset: DpOffset = DpOffset.Zero,
    onPick: (T) -> Unit,
) {
    val shed = LocalShed.current
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss, offset = offset) {
        for (choice in choices) {
            val color = if (choice.danger) shed.danger else null
            DropdownMenuItem(
                text = { Text(stringResource(choice.label), color = color ?: Color.Unspecified) },
                leadingIcon = { Icon(choice.icon, null, tint = color ?: shed.textDim) },
                onClick = {
                    onDismiss()
                    onPick(choice)
                },
            )
        }
    }
}
