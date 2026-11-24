package io.github.stratayun.paranoic.ui.state

@OptIn(kotlin.time.ExperimentalTime::class)
fun fileModifiedLabel(milliseconds: ULong?): String? {
    if (milliseconds == null || milliseconds > Long.MAX_VALUE.toULong()) return null
    return try { kotlin.time.Instant.fromEpochMilliseconds(milliseconds.toLong()).toString().replace('T', ' ').removeSuffix("Z") + " UTC" }
    catch (_: IllegalArgumentException) { null }
}
