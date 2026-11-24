# Compose UI — перенос v1.5 и интеграция нового Core

Gradle-проект `ui/` отделён от Cargo workspace. Android и Windows — целевые
платформы. 2026-11-12 на Linux проверены генерация UniFFI, компиляция desktop,
Android и instrumented-тестов, 19 desktop и 16 Android JVM-тестов (0 ошибок).
Запуск приложения на Windows/Android и instrumented-тесты на устройстве
этим не подтверждены. Полный статус и SHA — только в
[ROADMAP.md](../ROADMAP.md); процесс — [AGENTS.md](../AGENTS.md).

## Границы исходников

| Путь в composeApp/src | Содержимое |
| --- | --- |
| commonMain | Навигация, UI state, темы, список/плитки, фильтр загруженного списка, выбор, диалоги, PDF/image viewer и контракты CoreGateway/MediaSource. Без Android/Java imports. |
| jvmMain/kotlin | NativeCoreGateway и бинарный codec ссылок на grants. Это общая папка JVM-исходников, компилируемая каждым leaf target; отдельный KMP target не создаётся. |
| androidMain | Activity/Application, SAF, дескрипторы, системный вход/биометрия, Keystore grant store, Media3, bitmap decoding. |
| desktopMain | Окно, системный picker, DPAPI grant store, PNG decoding, JavaFX player и ограниченный loopback range endpoint. |
| build/generated/uniffi | Генерируемые Kotlin bindings. Генератор/ABI — crates/paranoic-bridge. |
| build/native | Собираемые native binaries и ресурсы. Факт проверки конкретной платформы — в ROADMAP. |

Обе точки входа создают настоящий `NativeCoreGateway` поверх единого
`NativeRuntime`. `DisconnectedCoreGateway` используется только при неуспешном
старте (например, native library не собрана/не упакована или недоступен защищённый
store). UI не подставляет demo-файлы, фиктивные подключения и успешные операции.

## Происхождение компонентов

Выборочный источник: `gitcortex-Ai/Paranoic_1.5v`,
SHA `3fd6ac3fdf967078c9aba03096c1cf8188354c65`.
Пути ниже относительно `android/app/src/main/kotlin/io/github/stratayun/paranoic/ui/`.

| Источник v1.5 | Адаптация |
| --- | --- |
| theme/ParanoicTheme.kt | Палитры Base/Mono/Paranoic, токены, плотность, шрифты, AMOLED. Android dynamic colors исключены. |
| components/AppIcon.kt | Canvas-векторы без R/drawable. |
| ParanoicApp.kt | Общий Scaffold, enum-маршруты, адаптация по ширине. Старые ViewModel и Android NavController исключены. |
| FilesScreen.kt, FilesNavigation.kt, FileListControls.kt | Список/плитки, breadcrumbs, фильтр только загруженных строк, выбор. Данные приходят из Rust. |
| CreateDirectoryDialog.kt, CopyDestinationField.kt | Callback-диалоги создания папки и выбора конкретной папки назначения. |
| AppearanceScreen.kt | Темы, настройки, RU/EN. |
| OperationsScreen.kt | Снимки нового Core journal, прогресс и запрос отмены. Старый контроллер не перенесён. |

Модель оформления адаптирована из `android/platform/.../settings/AppearanceSettings.kt`.
Старые Rust Engine/JNI/platform-модули, Rclone и secret EditText не переносились.

## Файлы и платформы

Реальные вызовы списка, создания папки, копирования и журнала идут:
Compose → CoreGateway → сгенерированный UniFFI → NativeRuntime → существующий Core.
Типы Location и storage UUID остаются за Rust; UI не собирает OS-пути для команд.

Core допускает добавление grant после запуска: UUID только добавляются,
binding фиксируется в журнале до экспонирования, заменить/перепривязать ID нельзя.
Ссылки на подключения сохраняются с UUID до регистрации, чтобы потерянный ответ
не приводил к автоматическому созданию нового ID. Неподтверждённые записи не
повторяются. Принятое задание не показывается завершённым.

В пространстве Paranoic находится реальная папка приложения, созданная host.
Это обычные Files, **без автоматического шифрования**; зашифрованные папки формата 1.8 создаются явно на экране «Шифрование».
Доступ к директориям Core state не предоставляется как пользовательский grant.

- Android SAF: системный picker, только выданные ОС persistable flags. При праве
  записи — создание папок, копирование в SAF и удаление: запись во временный
  скрытый документ и переименование с проверкой фактического имени (занятое имя
  → наш документ удаляется, ничего не перезаписывается). Перенос внутри SAF не
  предлагается. Потоковые (pipe) документы облачных провайдеров читаются
  последовательно. Файлы с недопустимыми/повторяющимися именами скрываются при
  показе с указанием количества, а не ломают всю папку.
- Windows: системный выбор папки; выбранные внешние каталоги read-only,
  capability confinement и стабильная привязка к открытому корню.
  В папке приложения доступны создание/копирование с create-new публикацией
  hard link. Нужна private ACL пользовательского профиля. Запись в произвольный
  выбранный каталог, замена, directory move, ReFS/network shares и гарантии
  directory fsync не объявлены реализованными.
- Операции обновляются при открытии страницы и по кнопке журнала. Постоянного
  фонового сервиса, уведомлений и recovery UI пока нет. Список последних
  заданий ограничен 128; полноценная навигация по всему журналу не добавлена.

## Вход и хранение настроек

Appearance store сохраняет только оформление. Grants имеют отдельный bounded
бинарный формат: Android — AES-GCM/Keystore + AtomicFile в noBackupFilesDir;
Windows — DPAPI текущего пользователя + атомарная замена шифротекста.
Повреждение/потеря ключа не скрываются сбросом store и выдачей новых UUID.

Android optional system-auth защищает вход в UI, по умолчанию выключен.
API 30+: BIOMETRIC_STRONG или DEVICE_CREDENTIAL; API 26–29: системный запрос
кода устройства. Модальный gate сохраняет композицию и регистрацию SAF picker.
При ON_STOP сбрасывается AppSession и блокируется вход; ожидающие UI-запросы
отменяются, старые ответы отбрасываются. Media3 освобождается при ON_STOP.
Отмена ожидания не равна отмене durable Core job.

Keystore/DPAPI здесь защищают ссылки на grants; это не система ключей Vault.
FLAG_SECURE включён. Не гарантируется уничтожение всех JVM/decoder/GPU-копий
метаданных и изображений в RAM. Gate не заявляет шифрование обычных Files.

## PDF и медиа

- Тип файла определяет Rust по magic. Открытие PDF/фото/аудио/видео происходит
  в приложении; неподдержанный тип/кодек показывается как недоступный.
- Фото: Rust image decoder; миниатюры в списке и preview первого кадра до 1024px.
  Вход до 32 MiB, RGBA до 64 MiB, декодирование сериализовано. PNG передаётся
  платформенному bitmap renderer. App не создаёт plaintext disk cache.
- PDF: Pdfium работает только в отдельном процессе `paranoic-pdf-worker`
  (документ разбирается один раз на сеанс просмотра, тайм-аут 30 с, аварийное
  завершение при сбое); pdfium-render 0.9.5 / Pdfium ABI 8125 по явному пути.
  Страницы, масштаб, текстовый поиск порциями по 16 страниц с continuation.
  Вход до 64 MiB, bitmap до 4096x4096, до 20000 страниц. Без OCR, паролей,
  форм, выполнения JS/XFA и перехода по ссылкам. Pdfium кладётся в сборку
  (Android: каждый ABI; Windows x64) из закреплённого выпуска с проверкой SHA-256
  — см. tools/build/README.md; `-Pparanoic.pdfium=none` — сборка без PDF, тогда
  отсутствие библиотеки — реальная ошибка capability.
- Android audio/video: Media3 1.8.0, собственный DataSource читает ограниченными
  порциями через NativeReader; OS-путь/URI файла decoder не получает.
- Windows audio/video: OpenJFX 21.0.13 (JDK17). JavaFX требует URI, поэтому
  один Core reader выдаётся через одноразовый 256-bit URL на 127.0.0.1.
  Только GET/HEAD и одиночный range, проверка Host/Origin, no-store,
  ограниченные workers/очередь. URL не содержит имени файла и не логируется.
  Сервер и поток закрываются при выходе из viewer, включая отмену передачи
  ресурса между coroutine contexts. Это транспорт обычных Files, не Vault.
  Набор кодеков ограничен возможностями JavaFX/Windows; универсальная поддержка
  всех контейнеров не обещается. VLC/GPL библиотека не добавлялась.

PDF разбирается только в отдельном процессе `paranoic-pdf-worker` (seccomp и
лимиты на Linux/Android, job object и Low integrity на Windows); Android
isolatedProcess — открытая задача (ROADMAP, У-4). Расшифрованное содержимое
формата 1.8 к этим адаптерам не передаётся.

## Генерация и упаковка

Настройка: Kotlin/Compose compiler 2.2.21, CMP 1.9.1, AGP 8.11.1,
Gradle 8.13, JDK17, Android SDK36/minSdk26. UniFFI 0.29.4/JNA5.17.0.
Подробности native/Pdfium — [tools/build/README.md](../tools/build/README.md).

Gradle compilation/package tasks вызывают tools/build/native.py для генерации
bindings и сборки нужных Rust libraries. Для генерации используется отдельный
Cargo-профиль `bindings` с сохранённой таблицей символов; `release` остаётся
с удалением символов. UniFFI требует компонент `paranoic_bridge` и ожидаемый
Kotlin-файл; пустой результат останавливает сборку. `NativeReader.releaseStream()`
закрывает поток отдельно от сгенерированного `AutoCloseable.close()/destroy()`.
Что подключено к экранам, что проверено и что осталось — только в корневом
[ROADMAP](../ROADMAP.md).
