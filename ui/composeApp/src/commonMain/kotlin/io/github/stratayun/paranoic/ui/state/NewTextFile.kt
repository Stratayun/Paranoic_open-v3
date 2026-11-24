package io.github.stratayun.paranoic.ui.state

fun validNewTextName(name: String): Boolean = plausibleOperationName(name) &&
    name.substringBeforeLast('.', "").isNotBlank() && name.substringAfterLast('.', "").lowercase() in setOf("txt", "md")
