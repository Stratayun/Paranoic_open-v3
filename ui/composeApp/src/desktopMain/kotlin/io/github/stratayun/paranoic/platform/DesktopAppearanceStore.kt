package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import java.util.prefs.Preferences

/** User preferences (HKCU on Windows), never a secret or Core-data store. */
object DesktopAppearanceStore : AppearanceStore {
    private fun preferences() = Preferences.userRoot().node("io/github/stratayun/paranoic/appearance-v2")
    override fun read(): AppearanceSettings {
        val preferences = preferences()
        return AppearanceCodec.decode { preferences.get(it, null) }
    }
    override fun write(settings: AppearanceSettings): Boolean = try {
        val preferences = preferences()
        AppearanceCodec.encode(settings).forEach { (key, value) -> preferences.put(key, value) }
        preferences.flush()
        true
    } catch (_: Exception) { false }
}

