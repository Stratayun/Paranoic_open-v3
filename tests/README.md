# Ручной запуск подготовленных проверок

Эта инструкция описывает исходники и команды, а не результаты. Фактические
прогоны и платформы отмечаются только в корневом ROADMAP.md. Команды ниже
выполняются после отдельной команды владельца; workflow, pre-commit hook,
автозапуск приложения и CI-триггеры здесь не добавляются.

## Что подготовлено

| Исходники | Что проверяют | Чего не подтверждают |
| --- | --- | --- |
| ui/composeApp/src/commonTest | Сортировка/фильтры без изменения identity; совместимость настроек; контрастность палитр; устаревшие ответы, pause и отсутствие повтора неопределённой записи | Отрисовку, TalkBack, реальную FFI |
| ui/composeApp/src/jvmTest | Независимый бинарный fixture grants v1; повреждённые, усечённые, будущие и чрезмерные payload | Android Keystore или Windows DPAPI |
| ui/composeApp/src/desktopTest | Реальный loopback HTTP с тестовым MediaSource: ranges, HEAD, Host/Origin, неподходящие запросы, закрытие reader | JavaFX-кодеки и воспроизведение на Windows |
| ui/composeApp/src/androidInstrumentedTest | Реальный Keystore/AES-GCM и AtomicFile на временных данных; fixture ContentProvider с pipe, кешем имени, отказами и лимитами | Системный picker, реальные URI grants, Google Drive и OEM-провайдеры |
| ui/composeApp/src/androidInstrumentedTest/.../ui/CreateDirectoryDialogTest.kt | Настоящие Compose-диалоги на Android: запрещённое имя не отправляется, Unicode-имя передаётся без изменений, отмена не создаёт папку, RU/EN | Выполнение файловой операции в Rust; проверку всех экранов и TalkBack |
| crates/paranoic-bridge/src/platform_contract_tests.rs | NativeReader, пределы чтения, закрытие, неверные запросы/grants, SAF create-new/conflict/cleanup и lenient listing | Сериализацию UniFFI/JNA, sandbox PDF или Android-приложение |
| tools/validation/android_apk.py | Состав ABI и native-библиотек готового APK, границы ELF, 16-КиБ LOAD alignment, при direct-загрузке также ZIP offset | Подпись APK, зависимости DT_NEEDED, успешную загрузку, manifest, UID/изоляцию PDF |
| tools/validation/test_android_apk.py | Враждебные синтетические ELF/ZIP и корректные fixtures самого инспектора | Настоящий APK или работоспособность приложения |

Первоначально подготовлено 45 тестовых методов/функций (включая параметризуемые
внутри них сценарии); затем добавлены 2 Compose-теста и SAF-тест границы дерева.
Число исходников не означает число успешных прогонов. Один Unix-only Rust-тест проверяет symlink и
непереносимое имя. Проверка изменения открытого файла в NativeReader ограничена
Unix: Windows может запрещать такую запись средствами ОС. SAF fixture resolver
использует ContentResolver.wrap и выполняется с API 29; Keystore cases — с minSdk 26.
Это не замена прогонам на Android 26 и на актуальной версии Android.

В commonTest используется backgroundScope тестового scheduler: pause создаёт
новый SupervisorJob; обычный родительский runTest scope мог бы ждать его бесконечно.
Некооперативные отложенные ответы моделируются без задержек и реальных файлов.

## Происхождение сценариев

- Paranoic_1.5v: android/app/src/test/kotlin/io/github/stratayun/paranoic/files/FileListProjectionTest.kt
  и ui/theme/ThemeContrastTest.kt — адаптированы применимые сценарии для новых моделей.
- Там же AppearancePolicyAuditTest и Android FolderBookmarksStorageTest — идеи
  совместимости public preferences, повреждённого store и отсутствующего ключа.
  Старые bookmarks, Android ViewModel и JNI не переносятся.
- Paranoic-crossplatform, snapshot 9ceb9215247b86437e58da96d0a2140f1f17cf50:
  transport/gobridge/hardening_test.go — перенесены идеи ограниченного ввода/чтения
  и закрытого handle в Rust-контракт, без Go/rclone-зависимостей.
- Старые checkpoint-скрипты с go test/vet, загрузкой SDK и сборками не копировались.

## Команды для будущего разрешённого запуска

Из корня репозитория, Python 3.10+, стандартная библиотека:

~~~sh
python3 -m unittest discover -s tools/validation -p test_android_apk.py
python3 tools/validation/android_apk.py /absolute/path/app.apk --abis arm64-v8a,x86_64 --profile core --packaging extracted
~~~

Список --abis должен точно совпадать со всеми упакованными native ABI:
дополнительная ABI только с JNA, без Rust bridge, считается неполной поставкой.
core требует libparanoic_bridge.so и libjnidispatch.so для каждой ABI.
pdf дополнительно требует libparanoic_pdf_worker.so и libpdfium.so.
Инспектор не скачивает, не распаковывает и не запускает эти файлы. Лимиты:
10 000 ZIP entries, суммарно 1 ГиБ объявленного содержимого и 64 МиБ на native ELF.

extracted соответствует нынешнему useLegacyPackaging=true; для direct нужны
несжатые библиотеки и 16-КиБ ZIP offset. Проверка ELF alignment остаётся в обоих
режимах. Выбор режима должен соответствовать реальному manifest, который этот
инспектор не разбирает. Размер страницы памяти на устройстве проверяется отдельно.
Основание: https://developer.android.com/guide/practices/page-sizes.

Rust, после отдельного разрешения компиляции/проверки:

~~~sh
cargo test -p paranoic-bridge platform_contract_tests --locked
~~~

Kotlin из ui/; Windows использует gradlew.bat вместо ./gradlew:

~~~sh
./gradlew :composeApp:desktopTest :composeApp:testDebugUnitTest
./gradlew :composeApp:connectedDebugAndroidTest
~~~

Эти Gradle-команды компилируют исходники и вызывают уже существующую генерацию
bindings/native build. Они не являются способом «проверить без сборки».
Нужны JDK 17, Rust/toolchain из проекта, для Android — SDK/NDK/cargo-ndk.
connectedDebugAndroidTest устанавливает test/app APK: использовать чистый эмулятор
или отдельное тестовое устройство/профиль без пользовательских Vault и аккаунтов.
Команды показаны как инструкция, на этапе подготовки не выполнялись.

Android fixture-тесты создают только UUID-каталоги и alias paranoic-test-*;
боевой alias paranoic-grants-v1 не читают и не удаляют. Если процесс теста убит,
остатки с тестовым UUID могут сохраниться; очистка допустима только для точно
идентифицированных тестовых данных. Отзыв настоящих grants, смена биометрии,
системный backup и очистка данных приложения сюда не автоматизированы.

## Отдельное Android-пространство в GitHub Actions

После ревью 2026-11-13 добавлены `NativeFeaturesTest` (настоящая packaged
UniFFI/JNA/Rust-библиотека, заметки с отдельным тестовым alias Keystore,
Clear против старого key record, Android KDF gate, vault import/export),
`SecretsLifecycleTest` (секретные диалоги исчезают до позднего status-ответа)
и две SAF-регрессии на перемещение в потомка и появление дубликата.
Они скомпилированы; выполнение на устройстве пока не подтверждено.
Ручной workflow проверяет состав APK до запуска эмулятора: успешная сборка
или fixture-тесты сами по себе не доказывают присутствие Rust Core.

`desktopTest/NativeCoreIntegrationTest` также вызывает настоящую библиотеку:
notes save/lock/unlock/Clear/recreate, vault add/export/no-replace/Quick Lock.
Это выполнение Kotlin ↔ Rust на хосте, не Windows DPAPI или Android Keystore.
Результаты и SHA — в ROADMAP и [отчёте ревью](../docs/audit/2026-11-13-formats-ui.md).

`Actions → Android tests (manual) → Run workflow` запускается только вручную.
Выбрать ветку `main` и API 26 либо 35. Создаётся одноразовый эмулятор x86_64
без личных файлов и аккаунтов; собираются настоящие app/test APK, запускаются
Keystore/AtomicFile, SAF fixture и Compose-тесты. На API 26 SAF fixture помечен
как пропущенный: `ContentResolver.wrap` требует API 29. API 35 покрывает и SAF.
Отчёты сохраняются в artifact `android-api-<API>-reports`, включая результаты
пропусков и ошибок. Успех этого workflow не подтверждает ARM64, OEM-провайдеры,
16-КиБ устройство, PDF sandbox или облачную авторизацию.

Для локального чистого эмулятора x86_64 из `ui/`:

~~~sh
./gradlew -Pparanoic.androidAbis=x86_64 :composeApp:connectedDebugAndroidTest
~~~

`paranoic.androidAbis` задаёт и Rust ABI, и фильтр библиотек APK: JNA не должен
добавлять ABI, для которой отсутствует Rust bridge. По умолчанию остаются
`arm64-v8a,x86_64`. Compose-тесты используют пустую Activity из debug-only
`ui-test-manifest`; реальный MainActivity/Core и пользовательские хранилища
не запускаются. Тестовая Activity не попадает в release-вариант.

Ручной `verify (manual)` с `target=kotlin` отдельно компилирует desktop,
Android и instrumented test sources, выполняет desktop/Android JVM unit tests.
Это проверка компиляции и JVM, без запуска Android-устройства.

## Обязательные отдельные проверки платформы

Android: [сценарии реального устройства](e2e/android.md). Проверка manifest или
наличие worker executable не доказывают песочницу PDF. Обычный дочерний процесс
с UID приложения не обеспечивает требуемую изоляцию; это блокер до релиза.

Windows: отдельно проверить DPAPI после перезапуска/из другого пользователя;
замену корня grant и symlink/junction за его пределы; отказ записи при отсутствии прав ОС; конфликты копирования в app-private Files; JavaFX seek/stop и
закрытие loopback после ухода с экрана. Только временные пользовательские данные.
