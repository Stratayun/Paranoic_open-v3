# Пройденные тесты

Только результаты, которые ещё относятся к текущему коду. Устаревшую строку удалять, когда код переписан или есть новый прогон.

| Дата | Что | Результат | SHA |
| --- | --- | --- | --- |
| 2026-11-26 | Безопасность изображений и Windows local, целевая проверка | Kotlin 2.2.21/JDK 21: PlatformImageFallbackTest 3/3; AndroidImageFallback и реальные PngBounds/PlatformImageFallback скомпилированы против Android API 36. Rust 1.89: cargo check paranoic-local + paranoic-windows со всеми тестовыми целями для x86_64-pc-windows-gnu успешен; только проверка типов, SQLite через host pkg-config, без Windows-линковки и выполнения. Новые Windows-регрессии подготовлены, не выполнялись. Gateway/полный APK/устройства/CI не проверялись. | `ce638e8` |
| 2026-11-25 | Очистка и исправления проверки пароля и OAuth, Linux, Rust 1.89 / JDK 17 | cargo check workspace со всеми тестовыми целями успешен. Пять целевых Rust-тестов прошли. Kotlin Desktop и Android скомпилированы с текущими UniFFI bindings. Desktop 122/122, Android JVM 112/112, пропусков нет. Включены настоящий Kotlin↔Rust round-trip и Compose-регрессия сброса оценки при смене пароля. | `4b2427d` |
| 2026-11-15 | Sol, EncryptedFolderPolicyTest + EncryptedFolderBridgeTest, JVM 21/Kotlin 2.2.21/JUnit4/coroutines; classes-vault-connect | 8/8, exit 0: все мутации read-only, Move false без второго удаления, opaque token/Changed без автокопии, маршрутизация архивов в обоих форматах хранилищ, привязка PDF/Office/media к одному handle/path/name, отказ чужим UI identities и snapshot пути. Фактические Core/DTO и два адаптера скомпилированы; Fake Core, без Rust/Compose/Android | `3e53830` |
| 2026-11-15 | Sol, AndroidManifest.xml, стандартный XML parser | XML корректен; manifest merger, Android Intent/grants/task/lifecycle и APK не выполнялись | `a99c8b6` |
| 2026-11-15 | Sol, ProtectedWriteStateTest; Kotlin 2.2.21/JUnit4, `classes-protected-write` | 7/7, exit 0: запрет двойной отправки, поздний reply после прерывания/сверки, Unknown только с явным подтверждением, ticket exhaustion и классификация неопределённых исходов. Фактические CoreGateway/TextDocument/OfficeDocument/FileOperationPolicy скомпилированы как зависимости. Только чистая модель; AppSession, native mutex/IO и UI не выполнялись | `11e23c7` |
| 2026-11-15 | Sol, FileProjectionRegressionTest 4; неизменённая часть `classes-seams` | 4/4 из прогона 7/7, exit 0: неизвестный размер остаётся видимым и сортируется после известных в обоих направлениях. EncryptedFolderPolicyTest после новых методов обновлён и выполнен новой строкой. Без native/Android/Compose | `b567d97` |
| 2026-11-15 | Sol, DiagnosticEnvironmentTest 8; неизменённая часть прежнего `classes-final-kotlin` | 8/8 из прогона 11/11, exit 0: ограниченные отчёт/запись сбоя без exception messages и файлов, циклы/повреждения, сохранение/очистка и независимая очистка журналов при отказе. Политика read-only после изменения проверена новой строкой (3); native/Keystore/DPAPI/UI/устройства не выполнялись | `6ab534d` |
| 2026-11-15 | Sol, VaultCatalogModelTest 13, VaultCatalogStoreTest 4; неизменённые сценарии прежнего `classes-vault-settings` | 17/17 из прогона 19/19, exit 0: таймауты/настройки, чтение v1 без записи и сохранение v2; fake envelope. Политика доступа после изменения проверена новой строкой (3). Keystore/DPAPI, таймер и UI не выполнялись | `ab1763d` |
| 2026-11-15 | Sol, FileWorkspaceTest, Kotlin 2.2.21/JUnit4, `classes-workspace` | 7/7, exit 0: независимые вкладки/панели, закрытие/лимит/уникальные ID, отзыв доступа, отсутствие старых выделений в дубликате. Модели файлов/видов/имени/метаданных скомпилированы отдельно; AppSession/UI не выполнялись | `4f7dcfb` |
| 2026-11-15 | Sol, FileBatchTest; Kotlin 2.2.21/JUnit4/coroutines, `/tmp/paranoic-catalog-test/classes-batch` | 12/12, exit 0: фиксированные ID до отправки, unknown/исключение без повтора, отмена/stop, частичный отказ, сверка журнала, раздельные исходы, ограничения замены/SAF/вложенности, recursive только для папок, снимок и лимиты. Модель и диспетчер с fake CoreGateway; native/Compose/AppSession не выполнялись | `987aa06` |
| 2026-11-15 | Sol, PlaceCatalogModelTest (6), PlaceCatalogStoreTest (3); неизменённая часть прежнего прогона | 9/9: имена/скрытие/отмена, повреждения и отказ защиты; VaultCatalogStore после этого изменён и проверен новой строкой. Без Android/Windows | `939e2ef` |
| 2026-11-15 | Sol, VaultFolderPickerFlowTest, Kotlin 2.2.21/JUnit4, отдельный `/tmp/paranoic-catalog-test/classes-flow` | 6/6, exit 0: новый корень/формат, отмена и ошибки, исходная карточка, повторные/устаревшие ответы, снимок пути. Только модель и DTO-срез; без платформенного picker, AppSession и Compose | `a0e60e8` |
| 2026-11-14 | Телефон, первый ручной тест | работает: заметки (пароль, биометрия, сохранение), PDF на arm64 с изоляцией, JPG, TXT, новая папка; ошибки — У-61–У-67 | `9064b10` |
| 2026-11-14 | Аудит формата 1.8 (первичный) | независимый декодер: заголовки, каталоги, 10 файлов, 30 повреждений отклонены; находки У-57–У-60 | `9c39832` |
| 2026-11-14 | Внешний аудит кода | 15 находок; приоритетные — У-51–У-56 | `9c39832` |
| 2026-11-14 | B11, B12 (мост) | 1.8: перенос, замена, смена пароля, ключ восстановления; Cryptomator: смена пароля с обрывом | `f7ac11e` |
| 2026-11-13 | B8 (заметки) | вложения: хранение, уборка, перешифровка, Clear | `67227c5` |
| 2026-11-13 | B7 (манифест 1.8) | откат файла, воскрешение, обрыв записи, откат хранилища с якорем | `396963b` |
| 2026-11-13 | B6 (ключ заметок) | старый ключ не открывает; обрыв переключения восстанавливается | `3a07a61` |
| 2026-11-13 | Блок 1, Rust 1.87 и stable | 89/89 (без удалённого VeraCrypt); Cryptomator сверен с cryptofs | `42dd08a` |
| 2026-11-13 | Блок 1, мутации | 8 из 8 поломок пойманы | `42dd08a` |
| 2026-11-13 | Windows-раннер, блок 1 | сборка, clippy, ACL, FileId, запись без перезаписи | `5f342e0` |
| 2026-11-13 | Блок 3, малые тесты | block3 14/14, block1_fixes 12/12, сервер собран под Windows | `b556fcf` |
| 2026-11-13 | У-5, У-22, У-23 | тесты и мутации | `fc3980a` |

**Отдельные проверки C** включают фаззинг формата 1.8, внешние эталоны форматов, интеграцию через WebDAV и поведение приложений на устройствах.
