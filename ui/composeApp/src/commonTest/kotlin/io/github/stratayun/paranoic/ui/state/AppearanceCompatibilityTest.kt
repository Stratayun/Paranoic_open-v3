package io.github.stratayun.paranoic.ui.state

import kotlin.test.*

class AppearanceCompatibilityTest {
    @Test fun persistedV15KeysRemainReadable() {
        val saved = mapOf("theme" to "mono", "brightness" to "amoled", "density" to "compact",
            "font" to "theme", "language" to "en", "view" to "grid")
        val settings = AppearanceCodec.decode(saved::get)
        assertEquals(AppearanceSettings(ThemePack.Mono, Brightness.Amoled, ContentDensity.Compact,
            FontChoice.Theme, UiLanguage.English, FileView.Grid), settings)
        // v1.5 had no welcome flag; it decodes as not completed and is written from then on.
        assertEquals(saved + ("welcome_completed" to "false"), AppearanceCodec.encode(settings))
    }

    @Test fun futureOrCorruptValuesFallBackIndependently() {
        val saved = mapOf("theme" to "future-pack", "brightness" to "LIGHT", "language" to "en", "view" to "grid")
        val settings = AppearanceCodec.decode(saved::get)
        assertEquals(ThemePack.Expressive, settings.theme)
        assertEquals(Brightness.Dark, settings.brightness)
        assertEquals(UiLanguage.English, settings.language)
        assertEquals(FileView.Grid, settings.fileView)
    }

    @Test fun appearanceStoreReadsOnlyPublicPreferenceKeys() {
        val requested = mutableSetOf<String>()
        AppearanceCodec.decode { requested += it; null }
        val allowed = setOf("theme", "brightness", "density", "font", "language", "view", "welcome_completed")
        assertEquals(allowed, requested)
        assertEquals(allowed, AppearanceCodec.encode(AppearanceSettings()).keys)
    }
}
