package io.github.stratayun.paranoic.ui.navigation

enum class Destination { Files, Encryption, Notes, Sync, Operations, Settings, Diagnostics }
val mainDestinations = listOf(Destination.Encryption, Destination.Files, Destination.Notes)
