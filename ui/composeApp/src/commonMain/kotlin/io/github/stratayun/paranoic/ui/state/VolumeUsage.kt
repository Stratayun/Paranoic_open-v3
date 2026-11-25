package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.staticCompositionLocalOf

/** Free/total bytes of a device volume, by its place name. Platform-provided; null when unknown. */
data class VolumeUsage(val freeBytes: Long, val totalBytes: Long)
val LocalVolumeUsage = staticCompositionLocalOf<(String) -> VolumeUsage?> { { null } }
