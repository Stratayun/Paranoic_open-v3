package io.github.stratayun.paranoic.ui.state

// Adapted from v1.5 AppearanceSettings. Only non-secret UI preferences.
enum class ThemePack(val key: String) { Base("base"), Mono("mono"), Paranoic("paranoic") }
enum class Brightness(val key: String) { System("system"), Light("light"), Dark("dark"), Amoled("amoled") }
enum class ContentDensity(val key: String) { Comfortable("comfortable"), Compact("compact") }
enum class FontChoice(val key: String) { System("system"), Theme("theme") }
enum class UiLanguage(val key: String) { Russian("ru"), English("en") }
enum class FileView(val key: String) { List("list"), Detailed("details"), Grid("grid") }
data class AppearanceSettings(
    val theme: ThemePack = ThemePack.Paranoic,
    val brightness: Brightness = Brightness.Dark,
    val density: ContentDensity = ContentDensity.Comfortable,
    val font: FontChoice = FontChoice.System,
    val language: UiLanguage = UiLanguage.Russian,
    val fileView: FileView = FileView.List,
    val welcomeCompleted: Boolean = false,
)
/** No keys, paths, grants, passwords or document content may enter this store. */
interface AppearanceStore {
    fun read(): AppearanceSettings
    /** false means preferences were NOT confirmed saved. */
    fun write(settings: AppearanceSettings): Boolean
}
object AppearanceCodec {
    fun decode(get: (String) -> String?): AppearanceSettings = AppearanceSettings(
        theme = ThemePack.entries.firstOrNull { it.key == get("theme") } ?: ThemePack.Paranoic,
        brightness = Brightness.entries.firstOrNull { it.key == get("brightness") } ?: Brightness.Dark,
        density = ContentDensity.entries.firstOrNull { it.key == get("density") } ?: ContentDensity.Comfortable,
        font = FontChoice.entries.firstOrNull { it.key == get("font") } ?: FontChoice.System,
        language = UiLanguage.entries.firstOrNull { it.key == get("language") } ?: UiLanguage.Russian,
        fileView = FileView.entries.firstOrNull { it.key == get("view") } ?: FileView.List,
        welcomeCompleted = get("welcome_completed") == "true",
    )
    fun encode(s: AppearanceSettings) = mapOf(
        "theme" to s.theme.key, "brightness" to s.brightness.key, "density" to s.density.key,
        "font" to s.font.key, "language" to s.language.key, "view" to s.fileView.key,
        "welcome_completed" to s.welcomeCompleted.toString(),
    )
}
