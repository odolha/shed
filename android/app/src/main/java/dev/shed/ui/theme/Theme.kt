package dev.shed.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.text.font.FontFamily
import dev.shed.data.ThemeChoice

/**
 * Shed's colours beyond Material's: from the theme files in `crates/shed-model/themes`
 * (`[ui]`, the terminal palette), plus Git and limit colours.
 */
@Immutable
data class ShedColors(
    val dark: Boolean,
    val background: Color,
    val surface: Color,
    val hover: Color,
    val text: Color,
    val textDim: Color,
    val divider: Color,
    val accent: Color,
    val danger: Color,
    val terminalBackground: Color,
    val terminalForeground: Color,
    val cursor: Color,
    /** The 16 ANSI colours. */
    val ansi: List<Color>,
    val added: Color,
    val removed: Color,
    val addedBackground: Color,
    val removedBackground: Color,
    val good: Color,
    val warn: Color,
) {
    /** A bench or tag colour as text or an icon: darkened on a light theme so it stays readable. */
    fun ink(color: Color): Color = if (dark) color else lerp(color, Color.Black, 0.35f)
}

private fun hex(rgb: Long) = Color(0xFF000000 or rgb)

val DarkShed = ShedColors(
    dark = true,
    background = hex(0x0E1116),
    surface = hex(0x14171D),
    hover = hex(0x1A1D24),
    text = hex(0xDDE1E8),
    textDim = hex(0x8A93A3),
    divider = hex(0x2A2F3A),
    accent = hex(0xFF8A57),
    danger = hex(0xF0566A),
    terminalBackground = hex(0x14171D),
    terminalForeground = hex(0xDDE1E8),
    cursor = hex(0xFF9A5A),
    ansi = listOf(
        0x20242D, 0xF0566A, 0x5FD38D, 0xF2C14E, 0x5C9DFF, 0xB48BFF, 0x3CCFC0, 0xC9CED8,
        0x5F6878, 0xFF7A8A, 0x8CF0BE, 0xFFD97A, 0x8CC4FF, 0xD0B6FF, 0x7FEBDD, 0xF4F6FA,
    ).map { hex(it.toLong()) },
    added = hex(0x5FD38D),
    removed = hex(0xF0566A),
    addedBackground = Color(0x2A5FD38D),
    removedBackground = Color(0x2AF0566A),
    good = hex(0x5FD38D),
    warn = hex(0xF2C14E),
)

val LightShed = ShedColors(
    dark = false,
    background = hex(0xECEEF2),
    surface = hex(0xFBFBFC),
    hover = hex(0xE2E5EA),
    text = hex(0x24292F),
    textDim = hex(0x6E7781),
    divider = hex(0xD0D7DE),
    accent = hex(0xD45A28),
    danger = hex(0xCF222E),
    terminalBackground = hex(0xFBFBFC),
    terminalForeground = hex(0x24292F),
    cursor = hex(0xE0602F),
    ansi = listOf(
        0x24292F, 0xCF222E, 0x1A7F37, 0x9A6700, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781,
        0x57606A, 0xE5534B, 0x2DA44E, 0xBF8700, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F,
    ).map { hex(it.toLong()) },
    added = hex(0x1A7F37),
    removed = hex(0xCF222E),
    addedBackground = Color(0x221A7F37),
    removedBackground = Color(0x22CF222E),
    good = hex(0x1A7F37),
    warn = hex(0x9A6700),
)

val LocalShed = staticCompositionLocalOf { DarkShed }

val Mono = FontFamily.Monospace

@Composable
fun ShedTheme(choice: ThemeChoice, content: @Composable () -> Unit) {
    val dark = when (choice) {
        ThemeChoice.System -> isSystemInDarkTheme()
        ThemeChoice.Dark -> true
        ThemeChoice.Light -> false
    }
    val shed = if (dark) DarkShed else LightShed
    val scheme = if (dark) {
        darkColorScheme(
            primary = shed.accent,
            onPrimary = shed.background,
            primaryContainer = lerp(shed.surface, shed.accent, 0.22f),
            onPrimaryContainer = shed.text,
            secondary = shed.accent,
            onSecondary = shed.background,
            secondaryContainer = shed.hover,
            onSecondaryContainer = shed.text,
            tertiary = shed.ansi[6],
            background = shed.background,
            onBackground = shed.text,
            surface = shed.background,
            onSurface = shed.text,
            surfaceVariant = shed.hover,
            onSurfaceVariant = shed.textDim,
            surfaceContainerLowest = shed.background,
            surfaceContainerLow = shed.surface,
            surfaceContainer = shed.surface,
            surfaceContainerHigh = shed.hover,
            surfaceContainerHighest = lerp(shed.hover, shed.divider, 0.5f),
            outline = shed.divider,
            outlineVariant = shed.divider,
            error = shed.danger,
            onError = shed.background,
        )
    } else {
        lightColorScheme(
            primary = shed.accent,
            onPrimary = Color.White,
            primaryContainer = lerp(shed.surface, shed.accent, 0.16f),
            onPrimaryContainer = shed.text,
            secondary = shed.accent,
            onSecondary = Color.White,
            secondaryContainer = shed.hover,
            onSecondaryContainer = shed.text,
            tertiary = shed.ansi[6],
            background = shed.background,
            onBackground = shed.text,
            surface = shed.background,
            onSurface = shed.text,
            surfaceVariant = shed.hover,
            onSurfaceVariant = shed.textDim,
            surfaceContainerLowest = Color.White,
            surfaceContainerLow = shed.surface,
            surfaceContainer = shed.surface,
            surfaceContainerHigh = shed.hover,
            surfaceContainerHighest = lerp(shed.hover, shed.divider, 0.5f),
            outline = shed.divider,
            outlineVariant = shed.divider,
            error = shed.danger,
            onError = Color.White,
        )
    }
    CompositionLocalProvider(LocalShed provides shed) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}

/** `#rrggbb` (or `#rgb`) as a colour; `null` when it is not one. */
fun parseColor(text: String?): Color? {
    if (text == null) return null
    val t = text.trim().removePrefix("#")
    val full = when (t.length) {
        3 -> t.map { "$it$it" }.joinToString("")
        6 -> t
        else -> return null
    }
    return full.toLongOrNull(16)?.let { Color(0xFF000000 or it) }
}

/** A colour of the 256-colour palette: the theme's 16, the 6×6×6 cube, the greys. */
fun paletteColor(index: Int, shed: ShedColors): Color = when {
    index < 16 -> shed.ansi[index.coerceAtLeast(0)]
    index < 232 -> {
        val i = index - 16
        val steps = intArrayOf(0, 95, 135, 175, 215, 255)
        Color(steps[i / 36], steps[(i / 6) % 6], steps[i % 6])
    }
    else -> {
        val v = 8 + (index.coerceAtMost(255) - 232) * 10
        Color(v, v, v)
    }
}

/** A terminal span's colour (`#rrggbb` or a palette index), `null` for the default. */
fun terminalColor(text: String?, shed: ShedColors): Color? {
    if (text.isNullOrEmpty()) return null
    if (text.startsWith("#")) return parseColor(text)
    return text.toIntOrNull()?.let { paletteColor(it, shed) }
}
