package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.sectionHeading

/** Startup result: a failure keeps its category and can be retried; it is never a silent "not connected". */
sealed interface Startup {
    data object Loading : Startup
    data class Ready(val core: NativeCoreGateway) : Startup
    data class Failed(val fault: StartupFault?) : Startup
}

fun startupFault(error: Throwable): StartupFault? = when (error) {
    is StartupException -> error.fault
    is LinkageError -> StartupFault.NativeLibrary
    else -> null
}

@Composable
@OptIn(ExperimentalLayoutApi::class)
fun StartupFailure(fault: StartupFault?, onRetry: () -> Unit, onContinue: () -> Unit) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(tr("Rust Core не запустился", "Rust Core did not start"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
            Text(when (fault) {
                StartupFault.NativeLibrary -> tr("Не загружена нативная библиотека. Переустановите приложение.",
                    "The native library could not be loaded. Reinstall the app.")
                StartupFault.GrantStore -> tr("Хранилище подключённых папок не читается. Данные папок не изменялись.",
                    "The connected-folder store cannot be read. Folder contents were not changed.")
                StartupFault.Core -> tr("Не открылся журнал операций или подключения. Файлы не изменялись.",
                    "The operation journal or connections could not be opened. Files were not changed.")
                null -> tr("Неизвестная ошибка запуска.", "Unknown startup failure.")
            })
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = onRetry) { Text(tr("Повторить", "Retry")) }
                TextButton(onClick = onContinue) { Text(tr("Продолжить без Core", "Continue without Core")) }
            }
        }
    }
}
