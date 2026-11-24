package io.github.stratayun.paranoic.platform

import android.content.Context
import io.github.stratayun.paranoic.ui.state.*

/** UI preferences only; no passwords, Core journal or SAF grants. */
class AndroidAppearanceStore(context: Context) : AppearanceStore {
    private val preferences = context.applicationContext.getSharedPreferences("appearance-v2", Context.MODE_PRIVATE)
    override fun read() = AppearanceCodec.decode { preferences.getString(it, null) }
    override fun write(settings: AppearanceSettings): Boolean {
        val edit = preferences.edit()
        AppearanceCodec.encode(settings).forEach { (key, value) -> edit.putString(key, value) }
        return edit.commit()
    }
}

