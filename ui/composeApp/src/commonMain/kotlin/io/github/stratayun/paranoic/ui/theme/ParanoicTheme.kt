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

private fun expressiveScheme(dark: Boolean): ColorScheme = if (dark) darkColorScheme(
    primary = Color(0xFFB8C4FF), onPrimary = Color(0xFF1F2D61), primaryContainer = Color(0xFF374479), onPrimaryContainer = Color(0xFFDDE1FF),
    secondary = Color(0xFFC3C5DD), onSecondary = Color(0xFF2C2F42), secondaryContainer = Color(0xFF3E4257), onSecondaryContainer = Color(0xFFDFE1F9),
    tertiary = Color(0xFFE5BAD8), onTertiary = Color(0xFF44263E), tertiaryContainer = Color(0xFFE9DDFF), onTertiaryContainer = Color(0xFF3B2A56),
    error = Color(0xFFFFB4AB), onError = Color(0xFF690005), errorContainer = Color(0xFF93000A), onErrorContainer = Color(0xFFFFDAD6),
    background = Color(0xFF121318), onBackground = Color(0xFFE3E1E9), surface = Color(0xFF121318), onSurface = Color(0xFFE3E1E9),
    surfaceVariant = Color(0xFF45464F), onSurfaceVariant = Color(0xFFC6C5D0), surfaceTint = Color(0xFFB8C4FF),
    inverseSurface = Color(0xFFE3E1E9), inverseOnSurface = Color(0xFF2F3036), inversePrimary = Color(0xFF4F5C92),
    outline = Color(0xFF90909A), outlineVariant = Color(0xFF45464F), scrim = Color.Black,
    surfaceDim = Color(0xFF121318), surfaceBright = Color(0xFF38393F), surfaceContainerLowest = Color(0xFF0D0E13),
    surfaceContainerLow = Color(0xFF1A1B21), surfaceContainer = Color(0xFF1E1F25), surfaceContainerHigh = Color(0xFF292A2F),
    surfaceContainerHighest = Color(0xFF34343A),
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

