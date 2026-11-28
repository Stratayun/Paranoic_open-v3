package io.github.stratayun.paranoic.ui.navigation

enum class Destination { Home, Files, Modules, Encryption, Notes, Sync, Operations, Settings, Diagnostics }
/** Top-level screens: no back arrow (owner's mockup 27.11: bottom tabs plus the sections opened from Home). */
val mainDestinations = listOf(Destination.Home, Destination.Files, Destination.Modules, Destination.Sync, Destination.Settings,
    Destination.Encryption, Destination.Notes)
/** Bottom navigation tabs on phones. */
val bottomTabs = listOf(Destination.Home, Destination.Files, Destination.Modules, Destination.Sync, Destination.Settings)
