package io.malac.app.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * The malac palette from `web/style.css`: neutral black and white, a true
 * near-black dark theme, green only for focus, links and status, amber and
 * red for status.
 */
@Immutable
data class MalacColors(
    val green: Color,
    val greenSoft: Color,
    val amber: Color,
    val amberSoft: Color,
    val red: Color,
    val redSoft: Color,
    val text2: Color,
    val text3: Color,
    val dotIdle: Color,
    val border: Color,
    val diffAdd: Color,
    val diffDel: Color,
)

private val LightMalac = MalacColors(
    green = Color(0xFF15803D),
    greenSoft = Color(0x1215803D),
    amber = Color(0xFFA15C07),
    amberSoft = Color(0x12B45309),
    red = Color(0xFFB91C1C),
    redSoft = Color(0x0FB91C1C),
    text2 = Color(0xFF525252),
    text3 = Color(0xFF6B6B6B),
    dotIdle = Color(0xFFA3A3A3),
    border = Color(0xFFE5E5E5),
    diffAdd = Color(0xFFEBF7EF),
    diffDel = Color(0xFFFCECEC),
)

private val DarkMalac = MalacColors(
    green = Color(0xFF4ADE80),
    greenSoft = Color(0x174ADE80),
    amber = Color(0xFFFBBF24),
    amberSoft = Color(0x14FBBF24),
    red = Color(0xFFF87171),
    redSoft = Color(0x17F87171),
    text2 = Color(0xFFA3A3A3),
    text3 = Color(0xFF8A8A8A),
    dotIdle = Color(0xFF5C5C5C),
    border = Color(0xFF262626),
    diffAdd = Color(0x1A4ADE80),
    diffDel = Color(0x1AF87171),
)

private val LightScheme: ColorScheme = lightColorScheme(
    primary = Color(0xFF0A0A0A),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFEBEBEB),
    onPrimaryContainer = Color(0xFF0A0A0A),
    inversePrimary = Color(0xFFEDEDED),
    secondary = Color(0xFF525252),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFEBEBEB),
    onSecondaryContainer = Color(0xFF0A0A0A),
    tertiary = Color(0xFF15803D),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0x1215803D),
    onTertiaryContainer = Color(0xFF15803D),
    background = Color(0xFFFAFAFA),
    onBackground = Color(0xFF0A0A0A),
    surface = Color(0xFFFAFAFA),
    onSurface = Color(0xFF0A0A0A),
    surfaceVariant = Color(0xFFF5F5F5),
    onSurfaceVariant = Color(0xFF525252),
    surfaceTint = Color(0xFFFAFAFA),
    inverseSurface = Color(0xFF0A0A0A),
    inverseOnSurface = Color(0xFFEDEDED),
    error = Color(0xFFB91C1C),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFCECEC),
    onErrorContainer = Color(0xFFB91C1C),
    outline = Color(0xFFD4D4D4),
    outlineVariant = Color(0xFFE5E5E5),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFFFFFFFF),
    surfaceDim = Color(0xFFEBEBEB),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFFFFFF),
    surfaceContainer = Color(0xFFF5F5F5),
    surfaceContainerHigh = Color(0xFFF5F5F5),
    surfaceContainerHighest = Color(0xFFEBEBEB),
)

private val DarkScheme: ColorScheme = darkColorScheme(
    primary = Color(0xFFEDEDED),
    onPrimary = Color(0xFF0A0A0A),
    primaryContainer = Color(0xFF222222),
    onPrimaryContainer = Color(0xFFEDEDED),
    inversePrimary = Color(0xFF0A0A0A),
    secondary = Color(0xFFA3A3A3),
    onSecondary = Color(0xFF0A0A0A),
    secondaryContainer = Color(0xFF2A2A2A),
    onSecondaryContainer = Color(0xFFEDEDED),
    tertiary = Color(0xFF4ADE80),
    onTertiary = Color(0xFF0A0A0A),
    tertiaryContainer = Color(0x174ADE80),
    onTertiaryContainer = Color(0xFF4ADE80),
    background = Color(0xFF0A0A0A),
    onBackground = Color(0xFFEDEDED),
    surface = Color(0xFF0A0A0A),
    onSurface = Color(0xFFEDEDED),
    surfaceVariant = Color(0xFF181818),
    onSurfaceVariant = Color(0xFFA3A3A3),
    surfaceTint = Color(0xFF0A0A0A),
    inverseSurface = Color(0xFFEDEDED),
    inverseOnSurface = Color(0xFF0A0A0A),
    error = Color(0xFFF87171),
    onError = Color(0xFF0A0A0A),
    errorContainer = Color(0x17F87171),
    onErrorContainer = Color(0xFFF87171),
    outline = Color(0xFF363636),
    outlineVariant = Color(0xFF262626),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFF222222),
    surfaceDim = Color(0xFF0A0A0A),
    surfaceContainerLowest = Color(0xFF0A0A0A),
    surfaceContainerLow = Color(0xFF111111),
    surfaceContainer = Color(0xFF181818),
    surfaceContainerHigh = Color(0xFF181818),
    surfaceContainerHighest = Color(0xFF222222),
)

private val LocalMalacColors = staticCompositionLocalOf { LightMalac }

/** The status and accent colours that Material's scheme has no role for. */
val MaterialTheme.malac: MalacColors
    @Composable get() = LocalMalacColors.current

@Composable
fun MalacTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalMalacColors provides if (dark) DarkMalac else LightMalac) {
        MaterialTheme(colorScheme = if (dark) DarkScheme else LightScheme, content = content)
    }
}
