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

/** Dark graphite palette (owner's mockup 27.11): calm neutral surfaces, soft periwinkle accent, mint for progress. */
private fun expressiveScheme(dark: Boolean): ColorScheme = if (dark) darkColorScheme(
    primary = Color(0xFFAEB6F5), onPrimary = Color(0xFF1B2050), primaryContainer = Color(0xFF353B63), onPrimaryContainer = Color(0xFFDEE1FF),
    secondary = Color(0xFFA9B4CC), onSecondary = Color(0xFF1F2738), secondaryContainer = Color(0xFF363C4C), onSecondaryContainer = Color(0xFFDCE2F2),
    tertiary = Color(0xFF7FD3A4), onTertiary = Color(0xFF00391E), tertiaryContainer = Color(0xFF1E4A35), onTertiaryContainer = Color(0xFFA9F0C7),
    error = Color(0xFFFF9AA8), onError = Color(0xFF5C0017), errorContainer = Color(0xFF6E1D2C), onErrorContainer = Color(0xFFFFD9DF),
    background = Color(0xFF16181E), onBackground = Color(0xFFE8E9EE), surface = Color(0xFF16181E), onSurface = Color(0xFFE8E9EE),
    surfaceVariant = Color(0xFF2A2D37), onSurfaceVariant = Color(0xFFA4A8B5), surfaceTint = Color(0xFFAEB6F5),
    inverseSurface = Color(0xFFE8E9EE), inverseOnSurface = Color(0xFF1E2027), inversePrimary = Color(0xFF4F579A),
    outline = Color(0xFF4B4F5C), outlineVariant = Color(0xFF2D303A), scrim = Color.Black,
    surfaceDim = Color(0xFF16181E), surfaceBright = Color(0xFF353844), surfaceContainerLowest = Color(0xFF101217),
    surfaceContainerLow = Color(0xFF1B1D24), surfaceContainer = Color(0xFF20232B), surfaceContainerHigh = Color(0xFF282B34),
    surfaceContainerHighest = Color(0xFF30333D),
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

