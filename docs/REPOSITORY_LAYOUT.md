# Paranoic — размещение исходников

**Утверждённая организационная схема (12 ноября 2026).** Этот документ отвечает на вопрос **«куда записывать новый код»**. Он **не** заменяет [ROADMAP.md](../ROADMAP.md), не назначает новые задачи и не меняет архитектуру существующих компонентов. Пустые папки отмечены пустым файлом `.gitkeep` только потому, что Git не хранит каталоги без файлов.

## Уже реализованная структура — сохраняется

| Путь | Ответственность | Реализация |
| --- | --- | --- |
| `crates/paranoic-core/` | Storage API, безопасные файловые операции, очередь, журнал и Recovery | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-local/` | Адаптер нативного локального хранилища | Нативный адаптер Android/Linux и Windows |
| `crates/paranoic-crypto/` | Криптографические примитивы, сессии и управление ключами | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-formats/` | Собственный формат 1.8, Cryptomator и age | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-documents/` | ZIP/7z, текст, Office и подготовка медиа | Rust-исходники; частично проверено 2.1–2.4, остальные части не подтверждены |
| `crates/paranoic-bridge/` | Единый новый UniFFI Runtime, records/callbacks, Core/SAF/read stream/preview wiring | UniFFI API, генерация bindings и платформенные callbacks |
| `crates/paranoic-cloud/` | WebDAV-клиент, облачные папки в очереди Core, синхронизация | Rust-исходники; UI/live accounts не подтверждены, см. ROADMAP |
| `crates/paranoic-notes/` | Заметки, вложения, жизненный цикл ключей и стирание | Реализация защищённого сейфа |
| `crates/paranoic-server/` | Собственный WebDAV-сервер | Исполняемый сервер |
| `crates/paranoic-windows/` | Win32 ACL, FileId и изоляция процессов | Платформенные вызовы Windows |
| `tools/interop/` | Существующие Java/Maven инструменты эталонной совместимости Cryptomator | Не перемещать без отдельного решения |

Заявления о тестировании относятся только к точному SHA и окружению, указанным в [ROADMAP.md](../ROADMAP.md). Наличие каталога или `.gitkeep` **не означает наличия реализации или пройденных проверок**.

## Размещение UI, проверок и документации

### `ui/composeApp/` — Compose Multiplatform

| Каталог | Ответственность |
| --- | --- |
| `src/commonMain/kotlin/io/github/stratayun/paranoic/ui/components/` | Общие кнопки, диалоги, файловые списки и повторно используемые UI-элементы |
| `src/commonMain/kotlin/io/github/stratayun/paranoic/ui/screens/{home,files,operations,archives,documents,analysis,encryption,vault,cloud,settings}/` | Общие экранные компоненты, независимые от Android API |
| `src/commonMain/kotlin/io/github/stratayun/paranoic/ui/{theme,state,navigation}/` | Дизайн-токены, UI state и маршруты |
| `src/androidMain/kotlin/io/github/stratayun/paranoic/platform/` | Application/Activity, SAF read-only backend/picker, Keystore, system-auth/биометрия, lifecycle, Media3/bitmap adapter |
| `src/desktopMain/kotlin/io/github/stratayun/paranoic/platform/` | Desktop окно, picker, DPAPI, JavaFX player/loopback stream и bitmap adapter; безопасные FS операции остаются в Rust |
| `src/jvmMain/kotlin/io/github/stratayun/paranoic/platform/` | Общая папка JVM gateway/store codec, компилируемая каждым leaf; не отдельный target |
| `src/commonMain/kotlin/io/github/stratayun/paranoic/bridge/` | Presentation-контракт CoreGateway и медиа; native symbols находятся только в JVM/platform-коде |
| `src/commonTest/kotlin/io/github/stratayun/paranoic/` | Тесты переносимых моделей и UI-состояния |

Имя пакета `io.github.stratayun.paranoic` сохранено при адаптации UI v1.5.
Текущие точки входа открывают NativeCoreGateway; общие экраны находятся в
commonMain, платформенные адаптеры — в androidMain и desktopMain, общая JVM-часть
подключается в оба leaf target из jvmMain. Происхождение компонентов описано
в [ui/README.md](../ui/README.md). Сборки и результаты проверки ведутся в ROADMAP
и docs/roadmap/tests.md с указанием SHA.

**Перенос v1.5:** выборочно адаптировать UI, не копировать Android ViewModel/JNI/OperationEngine и не вносить второй файловый движок. `commonMain` не импортирует Android классы; Android- и desktop-части подключаются через узкие интерфейсы. Хранение ключей, файловые операции и Recovery остаются за Rust Core. Миграцию UI планировать в пределах блока 4 уже существующего ROADMAP.

### `tests/` — общая проверка проекта

- `integration/` — межкрейтовые, межплатформенные и сквозные сценарии нескольких модулей.
- `interoperability/` — материалы для сравнения внешних форматов/клиентов; существующие executable Java/Maven harnesses остаются в `tools/interop/`.
- `fixtures/` — эталоны и воспроизводимые тестовые данные без реальных пользовательских секретов.
- `e2e/` — сценарии проверки Android/Windows на устройствах.

**Rust unit/integration tests остаются в `crates/<name>/src` и `crates/<name>/tests` согласно Cargo.** Тесты Compose также располагаются в соответствующих source sets. Не переносить работающие тесты ради формального единства каталогов.

### `tools/` — обслуживание разработки

- `validation/` — валидаторы поставки, статические проверки и вспомогательные скрипты.
- `build/` — воспроизводимые сборочные и упаковочные команды.
- `interop/` — **уже существует**; сохраняется по текущему адресу.

Python может использоваться для подготовки фикстур и проверок, но не подменяет основной Rust-код проекта. Наличие папки не запускает скрипты и не разрешает CI/сборку.

### `docs/` — обоснования и результаты

- `architecture/` — границы API, интерфейсы платформ, решения о компонентах.
- `security/` — модель угроз, спецификация формата 1.8 (`docs/format/`), гарантии и ограничения.
- `audit/` — фактические отчёты ревью, выполненные проверки и их evidence.
- `roadmap/` — приложения к карте: открытые проблемы и актуальные тесты. Статусы — только в `ROADMAP.md`.

**Один источник задач — корневой `ROADMAP.md`; один действующий процесс разработки — `AGENTS.md`.** Не создавать новые параллельные карты готовности.

Cargo workspace явно перечисляет десять пакетов. Собственное защищённое
хранилище реализовано в paranoic-formats, его UI находится в Compose.
Сборка native-библиотек, генерация bindings и поставка PDFium описаны в
[tools/build/README.md](../tools/build/README.md).

## Техническое правило размещения

**По ответственности, а не по одному языку:** Rust runtime/политики — `crates/`, экраны — `ui/`, межмодульные проверки — `tests/`, скрипты — `tools/`, документация — `docs/`. Если код работает только на Android или Windows, не переносить его в переносимый UI. При появлении функции заменять соответствующий `.gitkeep` реальными файлами, по желанию удалять маркер. Без force push; текущий `main` перепроверять перед коммитом.

**Что выполнено при утверждении схемы:** добавлены лишь этот справочник, ссылка из README/AGENTS и пустые `.gitkeep`. Функциональные Rust/Kotlin файлы, `ROADMAP.md`, Cargo/Gradle и тесты не менялись; сборки и проверки не запускались.

