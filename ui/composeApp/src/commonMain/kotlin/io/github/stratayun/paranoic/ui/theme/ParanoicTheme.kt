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

fun tokensFor(pack: ThemePack, density: ContentDensity): ThemeTokens {
    val base = when (pack) {
        ThemePack.Base -> ThemeTokens()
        ThemePack.Mono -> ThemeTokens(pagePadding = 28.dp, sectionGap = 32.dp, corner = 2.dp, iconStroke = 1.5.dp)
        ThemePack.Paranoic -> ThemeTokens(pagePadding = 18.dp, itemPadding = 16.dp, sectionGap = 18.dp, corner = 20.dp, iconStroke = 1.65.dp)
    }
    return if (density == ContentDensity.Compact) base.copy(itemPadding = 12.dp, sectionGap = 20.dp) else base
}

fun paletteFor(pack: ThemePack, dark: Boolean): ColorScheme {
    val palette = when (pack) {
    ThemePack.Base -> if (dark) darkColorScheme(
        primary = Color(0xFFACD2BE), onPrimary = Color(0xFF14382A),
        primaryContainer = Color(0xFF294E3E), onPrimaryContainer = Color(0xFFC8EFDA),
        secondary = Color(0xFFBDCBBF), background = Color(0xFF101713), surface = Color(0xFF151E18),
        onBackground = Color(0xFFE2E8DE), onSurface = Color(0xFFE2E8DE),
        surfaceVariant = Color(0xFF354139), onSurfaceVariant = Color(0xFFC4CEC4),
    ) else lightColorScheme(
        primary = Color(0xFF315E49), onPrimary = Color.White,
        primaryContainer = Color(0xFFD6ECDC), onPrimaryContainer = Color(0xFF1B3F2D),
        secondary = Color(0xFF4F6355), background = Color(0xFFF8FAF5), surface = Color(0xFFFFFFFF),
        onBackground = Color(0xFF17231A), onSurface = Color(0xFF17231A),
        surfaceVariant = Color(0xFFE3EAE0), onSurfaceVariant = Color(0xFF495449),
    )
    ThemePack.Mono -> if (dark) darkColorScheme(
        primary = Color(0xFFE8E8E8), onPrimary = Color(0xFF191919),
        primaryContainer = Color(0xFF353535), onPrimaryContainer = Color(0xFFF2F2F2),
        secondary = Color(0xFFCECECE), background = Color(0xFF111111), surface = Color(0xFF191919),
        onBackground = Color(0xFFECECEC), onSurface = Color(0xFFECECEC),
        surfaceVariant = Color(0xFF333333), onSurfaceVariant = Color(0xFFC5C5C5),
    ) else lightColorScheme(
        primary = Color(0xFF222222), onPrimary = Color.White,
        primaryContainer = Color(0xFFE5E5E5), onPrimaryContainer = Color(0xFF181818),
        secondary = Color(0xFF525252), background = Color(0xFFF7F7F7), surface = Color.White,
        onBackground = Color(0xFF1B1B1B), onSurface = Color(0xFF1B1B1B),
        surfaceVariant = Color(0xFFE5E5E5), onSurfaceVariant = Color(0xFF515151),
    )
    ThemePack.Paranoic -> if (dark) darkColorScheme(
        primary = Color(0xFFFF8733), onPrimary = Color(0xFF160B07),
        primaryContainer = Color(0xFF4B2517), onPrimaryContainer = Color(0xFFFFE4D1),
        secondary = Color(0xFFE0DAD5), background = Color(0xFF0B0C0E), surface = Color(0xFF141619),
        onBackground = Color(0xFFF5F3F0), onSurface = Color(0xFFF5F3F0),
        surfaceVariant = Color(0xFF222429), onSurfaceVariant = Color(0xFFB6B5B9),
    ) else lightColorScheme(
        primary = Color(0xFFA63C00), onPrimary = Color.White,
        primaryContainer = Color(0xFFFFDBC2), onPrimaryContainer = Color(0xFF542000),
        secondary = Color(0xFF6D5044), background = Color(0xFFFBF9F7), surface = Color.White,
        onBackground = Color(0xFF211B19), onSurface = Color(0xFF211B19),
        surfaceVariant = Color(0xFFF1EAE6), onSurfaceVariant = Color(0xFF5E534D),
    )
    }
    // Every neutral/tonal surface belongs to the pack, including Material components
    // such as the navigation indicator and dialogs. Do not inherit default purple.
    fun surface(alpha: Float) = palette.onSurface.copy(alpha = alpha).compositeOver(palette.surface)
    return palette.copy(
        onSecondary = palette.onPrimary,
        secondaryContainer = palette.primaryContainer,
        onSecondaryContainer = palette.onPrimaryContainer,
        tertiary = palette.secondary,
        onTertiary = palette.onPrimary,
        tertiaryContainer = palette.primaryContainer,
        onTertiaryContainer = palette.onPrimaryContainer,
        surfaceTint = palette.primary,
        surfaceDim = if (dark) palette.background else surface(0.08f),
        surfaceBright = if (dark) surface(0.12f) else palette.surface,
        surfaceContainerLowest = palette.background,
        surfaceContainerLow = surface(0.02f),
        surfaceContainer = surface(0.04f),
        surfaceContainerHigh = surface(0.06f),
        surfaceContainerHighest = surface(0.08f),
        outline = palette.onSurfaceVariant.copy(alpha = 0.65f).compositeOver(palette.surface),
        outlineVariant = palette.onSurfaceVariant.copy(alpha = 0.25f).compositeOver(palette.surface),
        inverseSurface = palette.onSurface,
        inverseOnSurface = palette.surface,
        inversePrimary = palette.onPrimary,
    )
}

@Composable
fun ParanoicTheme(settings: AppearanceSettings, content: @Composable () -> Unit) {
    val dark = usesDarkPalette(settings.brightness, isSystemInDarkTheme())
    var colors = paletteFor(settings.theme, dark)
    if (settings.brightness == Brightness.Amoled) {
        colors = colors.copy(background = Color.Black, surface = Color.Black, surfaceContainer = Color.Black)
    }
    val tokens = tokensFor(settings.theme, settings.density)
    val font = if (settings.font == FontChoice.System) FontFamily.Default else when (settings.theme) {
        ThemePack.Base -> FontFamily.SansSerif
        ThemePack.Mono -> FontFamily.Monospace
        ThemePack.Paranoic -> FontFamily.SansSerif
    }
    val weight = if (settings.theme == ThemePack.Mono) FontWeight.Normal else FontWeight.SemiBold
    val tracking = if (settings.theme == ThemePack.Mono) 0.5.sp else 0.sp
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
        LocalBrandArtwork provides (settings.theme == ThemePack.Paranoic),
        LocalBrandDark provides dark,
    ) {
        MaterialTheme(
            colorScheme = colors,
            typography = typography,
            shapes = Shapes(
                small = RoundedCornerShape(tokens.corner / 2),
                medium = RoundedCornerShape(tokens.corner),
                large = RoundedCornerShape(tokens.corner),
                extraLarge = RoundedCornerShape(tokens.corner + 4.dp),
            ),
            content = content,
        )
    }
}

