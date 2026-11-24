package io.github.stratayun.paranoic.ui.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import io.github.stratayun.paranoic.ui.state.ThemePack
import kotlin.math.max
import kotlin.math.min
import kotlin.test.Test
import kotlin.test.assertTrue

/** Adapted v1.5 palette cases; not proof of rendered UI accessibility. */
class ThemeContrastTest {
    @Test fun primaryAndBodyTextHaveReadablePaletteContrast() {
        for (pack in ThemePack.entries) for (dark in listOf(false, true)) {
            val p = paletteFor(pack, dark)
            val pairs = mapOf("primary" to (p.onPrimary to p.primary),
                "surface" to (p.onSurface to p.surface), "background" to (p.onBackground to p.background),
                "variant" to (p.onSurfaceVariant to p.surfaceVariant))
            pairs.forEach { (role, colors) ->
                val ratio = contrast(colors.first, colors.second)
                assertTrue(ratio >= 4.5, "$pack dark=$dark $role contrast=$ratio")
            }
        }
    }
    private fun contrast(a: Color, b: Color): Double =
        (max(a.luminance(), b.luminance()) + 0.05) / (min(a.luminance(), b.luminance()) + 0.05)
}
