package io.github.stratayun.paranoic.ui.screens.settings

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.shape.RoundedCornerShape
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.components.fileViewLabel
import io.github.stratayun.paranoic.ui.theme.*
import io.github.stratayun.paranoic.resources.*
import org.jetbrains.compose.resources.DrawableResource

private enum class SettingsSection { Appearance, Files, Language, Security, Diagnostics, Setup }
private enum class SettingId { Theme, Brightness, Density, Font, FileView, Language, Security, Diagnostics, Setup }
private data class SettingEntry(val id: SettingId, val section: SettingsSection, val title: String, val search: String)

@Composable
private fun SettingsSection.label() = when (this) {
    SettingsSection.Appearance -> tr("Оформление", "Appearance")
    SettingsSection.Files -> tr("Файлы", "Files")
    SettingsSection.Language -> tr("Язык", "Language")
    SettingsSection.Security -> tr("Защита", "Security")
    SettingsSection.Diagnostics -> tr("Диагностика", "Diagnostics")
    SettingsSection.Setup -> tr("Первый запуск", "Initial setup")
}
@Composable
private fun SettingsSection.summary() = when (this) {
    SettingsSection.Appearance -> tr("Тема, яркость, плотность и шрифт", "Theme, brightness, density and font")
    SettingsSection.Files -> tr("Список или плитки", "List or grid")
    SettingsSection.Language -> "Русский / English"
    SettingsSection.Security -> tr("Вход в приложение и скриншоты на Android", "App entry and screenshots on Android")
    SettingsSection.Diagnostics -> tr("Журнал, сбои и отчёт", "Journal, crashes and report")
    SettingsSection.Setup -> tr("Повторить мастер настройки", "Run the setup wizard again")
}

/** Settings hub over existing preferences. Search contains labels/values only, never user content. */
@Composable
fun AppearanceScreen(settings: AppearanceSettings, saving: Boolean, saved: Boolean?,
    onChange: (AppearanceSettings) -> Unit, securitySettings: @Composable () -> Unit = {},
    backHandler: @Composable (Boolean, () -> Unit) -> Unit = { _, _ -> }, diagnosticsSettings: @Composable () -> Unit = {},
    onRestartWelcome: () -> Unit = {}) {
    var section by remember { mutableStateOf<SettingsSection?>(null) }
    var query by remember { mutableStateOf("") }
    val entries = listOf(
        SettingEntry(SettingId.Brightness, SettingsSection.Appearance, tr("Яркость", "Brightness"), "яркость оформление светлая тёмная темная система amoled brightness appearance light dark system"),
        SettingEntry(SettingId.Density, SettingsSection.Appearance, tr("Плотность", "Density"), "плотность оформление компактная обычная density appearance compact comfortable"),
        SettingEntry(SettingId.Font, SettingsSection.Appearance, tr("Шрифт", "Font"), "шрифт оформление системный по теме font appearance system theme"),
        SettingEntry(SettingId.FileView, SettingsSection.Files, tr("Вид файлов", "File view"), "файлы вид список плитки сетка files view list tiles grid"),
        SettingEntry(SettingId.Language, SettingsSection.Language, tr("Язык", "Language"), "язык русский английский russian english language"),
        SettingEntry(SettingId.Security, SettingsSection.Security, tr("Защита", "Security"), "защита безопасность системный вход биометрия скриншоты снимки запись экрана security authentication biometric screenshots capture recording"),
        SettingEntry(SettingId.Diagnostics, SettingsSection.Diagnostics, tr("Диагностика", "Diagnostics"), "ошибки сбой журнал отчёт отчет диагностика diagnostic diagnostics report crash journal errors"),
        SettingEntry(SettingId.Setup, SettingsSection.Setup, tr("Первый запуск", "Initial setup"), "мастер первый запуск язык защита заметки хранилище welcome setup onboarding first launch"),
    )
    val search = query.trim().lowercase()
    val matches = entries.filter { entry ->
        if (search.isNotEmpty()) search.split(Regex("\\s+")).all { term -> term in (entry.title + " " + entry.search).lowercase() }
        else section == entry.section
    }
    backHandler(section != null || query.isNotEmpty()) {
        if (query.isNotEmpty()) query = "" else section = null
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(LocalThemeTokens.current.pagePadding),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            val current = section
            if (current == null || search.isNotEmpty()) SectionHero(Res.drawable.ic_settings, tr("Настройки", "Settings"),
                tr("Оформление, язык, защита и диагностика", "Appearance, language, security and diagnostics"), Modifier.sectionHeading())
            else SectionHero(current.icon(), current.label(), current.summary(), Modifier.sectionHeading())
        }
        item { PillSearchField(query, { query = it.take(160) }, tr("Поиск по настройкам", "Search settings")) }
        if (saving) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
        if (saved == false) item {
            Text(tr("Не удалось сохранить настройки.", "Preferences could not be saved."),
                Modifier.announceChanges(), color = MaterialTheme.colorScheme.error)
        }
        if (search.isEmpty() && section == null) {
            item {
                ConceptCard {
                    SettingsSection.entries.forEach { destination ->
                        val title = destination.label()
                        ConceptRow(title, destination.summary(), destination.icon(),
                            Modifier.actionDescription(tr("Открыть раздел: ", "Open section: ") + title)) { section = destination }
                    }
                }
            }
        } else {
            item {
                AssistChip(onClick = { query = ""; section = null }, label = { Text(tr("Все разделы", "All sections")) },
                    leadingIcon = { AppIcon(AppSymbol.Back) }, shape = RoundedCornerShape(20.dp))
                if (search.isNotEmpty()) Text(tr("Результаты во всех разделах", "Results from all sections"), Modifier.sectionHeading(),
                    style = MaterialTheme.typography.titleMedium)
            }
            if (matches.isEmpty()) item {
                Text(tr("Ничего не найдено. Попробуйте «язык», «тема» или «скриншоты».",
                    "No results. Try language, theme or screenshots."), Modifier.announceChanges())
            }
            items(matches, key = { it.id.name }) { entry ->
                ConceptCard {
                    if (search.isNotEmpty()) Text(entry.section.label(), style = MaterialTheme.typography.labelMedium)
                    when (entry.id) {
                        SettingId.Theme -> Unit
                        SettingId.Brightness -> ChoiceRow(entry.title, settings.brightness, Brightness.entries, !saving, {
                            when (it) {
                                Brightness.System -> tr("Как в системе", "System")
                                Brightness.Light -> tr("Светлая", "Light")
                                Brightness.Dark -> tr("Тёмная", "Dark")
                                Brightness.Amoled -> "AMOLED"
                            }
                        }) { onChange(settings.copy(brightness = it)) }
                        SettingId.Density -> ChoiceRow(entry.title, settings.density, ContentDensity.entries, !saving, {
                            if (it == ContentDensity.Compact) tr("Компактная", "Compact") else tr("Обычная", "Comfortable")
                        }) { onChange(settings.copy(density = it)) }
                        SettingId.Font -> ChoiceRow(entry.title, settings.font, FontChoice.entries, !saving, {
                            if (it == FontChoice.System) tr("Системный", "System") else tr("По теме", "Theme")
                        }) { onChange(settings.copy(font = it)) }
                        SettingId.FileView -> ChoiceRow(entry.title, settings.fileView, FileView.entries, !saving, {
                            it.fileViewLabel()
                        }) { onChange(settings.copy(fileView = it)) }
                        SettingId.Language -> ChoiceRow(entry.title, settings.language, UiLanguage.entries, !saving, {
                            if (it == UiLanguage.Russian) "Русский" else "English"
                        }) { onChange(settings.copy(language = it)) }
                        SettingId.Security -> {
                            Text(tr("Системный вход и скриншоты настраиваются на Android. Пароли заметок и хранилищ — в соответствующих разделах.",
                                "App entry and screenshots are configured on Android. Notes and storage passwords are managed in their own sections."),
                                style = MaterialTheme.typography.bodySmall)
                            securitySettings()
                        }
                        SettingId.Diagnostics -> diagnosticsSettings()
                        SettingId.Setup -> ConceptRow(tr("Повторить мастер", "Run setup again"),
                            tr("Язык, защита, заметки и хранилище — заново", "Language, security, notes and storage — again"),
                            Res.drawable.ic_settings, enabled = !saving, onClick = onRestartWelcome)
                    }
                }
            }
        }
    }
}


@Composable
private fun <T> ChoiceRow(title: String, selected: T, values: List<T>, enabled: Boolean,
    label: @Composable (T) -> String, onSelect: (T) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Box {
        ConceptRow(title, label(selected), enabled = enabled, onClick = { expanded = true },
            trailing = { Icon(AppSymbol.Down.vector(), null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) })
        DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
            values.forEach { value ->
                DropdownMenuItem(text = { Text(label(value)) }, onClick = { expanded = false; onSelect(value) })
            }
        }
    }
}

private fun SettingsSection.icon(): DrawableResource = when (this) {
    SettingsSection.Appearance -> Res.drawable.ic_appearance; SettingsSection.Files -> Res.drawable.ic_folder
    SettingsSection.Language -> Res.drawable.ic_language; SettingsSection.Security -> Res.drawable.ic_shield
    SettingsSection.Diagnostics -> Res.drawable.ic_diagnostics; SettingsSection.Setup -> Res.drawable.ic_settings
}
