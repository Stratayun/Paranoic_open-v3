package io.github.stratayun.paranoic.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.ui.state.AppearanceSettings
import io.github.stratayun.paranoic.ui.state.Brightness
import io.github.stratayun.paranoic.ui.state.ContentDensity
import io.github.stratayun.paranoic.ui.state.FontChoice
import io.github.stratayun.paranoic.ui.state.ThemePack

@Immutable
data class ThemeTokens(
    val pagePadding: Dp = 24.dp,
    val itemPadding: Dp = 20.dp,
    val sectionGap: Dp = 28.dp,
    val corner: Dp = 20.dp,
    val iconStroke: Dp = 2.dp,
)

val LocalThemeTokens = staticCompositionLocalOf { ThemeTokens() }
val LocalBrandArtwork = staticCompositionLocalOf { false }
val LocalBrandDark = staticCompositionLocalOf { true }

fun usesDarkPalette(brightness: Brightness, systemDark: Boolean): Boolean = when (brightness) {
    Brightness.System -> systemDark
    Brightness.Light -> false
    Brightness.Dark, Brightness.Amoled -> true
}

fun tokensFor(@Suppress("UNUSED_PARAMETER") pack: ThemePack, density: ContentDensity): ThemeTokens {
    val base = ThemeTokens(pagePadding = 16.dp, itemPadding = 16.dp, sectionGap = 20.dp, corner = 28.dp, iconStroke = 2.dp)
    return if (density == ContentDensity.Compact) base.copy(itemPadding = 12.dp, sectionGap = 16.dp) else base
}

fun paletteFor(@Suppress("UNUSED_PARAMETER") pack: ThemePack, dark: Boolean): ColorScheme = expressiveScheme(dark)

/** Dark palette tuned to the owner's neon glass icon set: deep navy glass, periwinkle and cyan glow. */
private fun expressiveScheme(dark: Boolean): ColorScheme = if (dark) darkColorScheme(
    primary = Color(0xFF9DA8FF), onPrimary = Color(0xFF14185A), primaryContainer = Color(0xFF2C338A), onPrimaryContainer = Color(0xFFDFE2FF),
    secondary = Color(0xFF6FD8FF), onSecondary = Color(0xFF00344A), secondaryContainer = Color(0xFF183B73), onSecondaryContainer = Color(0xFFD3EEFF),
    tertiary = Color(0xFFD29BFF), onTertiary = Color(0xFF3B0E66), tertiaryContainer = Color(0xFF3F2380), onTertiaryContainer = Color(0xFFF1DBFF),
    error = Color(0xFFFF8FA3), onError = Color(0xFF5C0017), errorContainer = Color(0xFF7A1430), onErrorContainer = Color(0xFFFFD9DF),
    background = Color(0xFF070B24), onBackground = Color(0xFFE6E8FF), surface = Color(0xFF070B24), onSurface = Color(0xFFE6E8FF),
    surfaceVariant = Color(0xFF242C5C), onSurfaceVariant = Color(0xFFB4BAE6), surfaceTint = Color(0xFF9DA8FF),
    inverseSurface = Color(0xFFE6E8FF), inverseOnSurface = Color(0xFF1A1F45), inversePrimary = Color(0xFF4A52B8),
    outline = Color(0xFF5A63A8), outlineVariant = Color(0xFF2A3266), scrim = Color.Black,
    surfaceDim = Color(0xFF070B24), surfaceBright = Color(0xFF2A3160), surfaceContainerLowest = Color(0xFF04071A),
    surfaceContainerLow = Color(0xFF0C1232), surfaceContainer = Color(0xFF10173D), surfaceContainerHigh = Color(0xFF151D4A),
    surfaceContainerHighest = Color(0xFF1C2558),
) else lightColorScheme(
    primary = Color(0xFF4F5C92), onPrimary = Color.White, primaryContainer = Color(0xFFDDE1FF), onPrimaryContainer = Color(0xFF374479),
    secondary = Color(0xFF5A5D72), onSecondary = Color.White, secondaryContainer = Color(0xFFDFE1F9), onSecondaryContainer = Color(0xFF424659),
    tertiary = Color(0xFF75546F), onTertiary = Color.White, tertiaryContainer = Color(0xFFE9DDFF), onTertiaryContainer = Color(0xFF3B2A56),
    error = Color(0xFFBA1A1A), onError = Color.White, errorContainer = Color(0xFFFFDAD6), onErrorContainer = Color(0xFF93000A),
    background = Color(0xFFFBF8FF), onBackground = Color(0xFF1B1B21), surface = Color(0xFFFBF8FF), onSurface = Color(0xFF1B1B21),
    surfaceVariant = Color(0xFFE2E1EC), onSurfaceVariant = Color(0xFF45464F), surfaceTint = Color(0xFF4F5C92),
    inverseSurface = Color(0xFF303036), inverseOnSurface = Color(0xFFF2EFF7), inversePrimary = Color(0xFFB8C4FF),
    outline = Color(0xFF767680), outlineVariant = Color(0xFFC6C5D0), scrim = Color.Black,
    surfaceDim = Color(0xFFDBD9E0), surfaceBright = Color(0xFFFBF8FF), surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF5F2FA), surfaceContainer = Color(0xFFEFEDF4), surfaceContainerHigh = Color(0xFFE9E7EF),
    surfaceContainerHighest = Color(0xFFE3E1E9),
)

@Composable
fun ParanoicTheme(settings: AppearanceSettings, content: @Composable () -> Unit) {
    val dark = usesDarkPalette(settings.brightness, isSystemInDarkTheme())
    var colors = paletteFor(settings.theme, dark)
    if (settings.brightness == Brightness.Amoled) {
        colors = colors.copy(background = Color.Black, surface = Color.Black, surfaceContainer = Color.Black)
    }
    val expressive = true
    val tokens = tokensFor(settings.theme, settings.density)
    val font = if (settings.font == FontChoice.System) FontFamily.Default else FontFamily.SansSerif
    val weight = FontWeight.Medium
    val tracking = 0.sp
    val base = Typography()
    val typography = base.copy(
        displaySmall = base.displaySmall.copy(fontFamily = font, fontWeight = weight, letterSpacing = tracking),
        headlineLarge = base.headlineLarge.copy(fontFamily = font, fontWeight = weight),
        headlineMedium = base.headlineMedium.copy(fontFamily = font, fontWeight = weight),
        titleLarge = base.titleLarge.copy(fontFamily = font, fontWeight = weight),
        titleMedium = base.titleMedium.copy(fontFamily = font, fontWeight = weight),
        bodyLarge = base.bodyLarge.copy(fontFamily = if (settings.font == FontChoice.System) font else FontFamily.SansSerif),
        bodyMedium = base.bodyMedium.copy(fontFamily = if (settings.font == FontChoice.System) font else FontFamily.SansSerif),
        labelLarge = base.labelLarge.copy(fontFamily = font, letterSpacing = tracking),
    )
    CompositionLocalProvider(
        LocalThemeTokens provides tokens,
        LocalBrandArtwork provides false,
        LocalBrandDark provides dark,
    ) {
        MaterialTheme(
            colorScheme = colors,
            typography = typography,
            shapes = if (expressive) Shapes(
                extraSmall = RoundedCornerShape(8.dp), small = RoundedCornerShape(12.dp), medium = RoundedCornerShape(20.dp),
                large = RoundedCornerShape(28.dp), extraLarge = RoundedCornerShape(32.dp),
            ) else Shapes(
                small = RoundedCornerShape(tokens.corner / 2),
                medium = RoundedCornerShape(tokens.corner),
                large = RoundedCornerShape(tokens.corner),
                extraLarge = RoundedCornerShape(tokens.corner + 4.dp),
            ),
            content = content,
        )
    }
}

