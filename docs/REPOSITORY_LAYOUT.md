# Paranoic v2.1 — размещение исходников и заготовленные каталоги

**Утверждённая организационная схема (12 ноября 2026).** Этот документ отвечает на вопрос **«куда записывать новый код»**. Он **не** заменяет [ROADMAP.md](../ROADMAP.md), не назначает новые задачи и не меняет архитектуру существующих компонентов. Пустые папки отмечены пустым файлом `.gitkeep` только потому, что Git не хранит каталоги без файлов.

## Уже реализованная структура — сохраняется

| Путь | Ответственность | Статус на момент утверждения |
| --- | --- | --- |
| `crates/paranoic-core/` | Storage API, безопасные файловые операции, очередь, журнал и Recovery | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-local/` | Адаптер нативного локального хранилища | Rust-исходники; Windows ещё требует адаптера |
| `crates/paranoic-crypto/` | Криптографические примитивы, сессии и управление ключами | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-formats/` | Собственный формат 1.8, Cryptomator и age | Rust-исходники; частичная проверка блока 1 |
| `crates/paranoic-documents/` | ZIP/7z, текст, Office и подготовка медиа | Rust-исходники; частично проверено 2.1–2.4, остальные части не подтверждены |
| `crates/paranoic-bridge/` | Единый новый UniFFI Runtime, records/callbacks, Core/SAF/read stream/preview wiring | Исходники; bindings/native binaries и запуск не проверены |
| `crates/paranoic-cloud/` | API облачных провайдеров, OAuth, сессии и transfer adapters | Rust-исходники; UI/live accounts не подтверждены, см. ROADMAP |
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
| `src/commonTest/kotlin/io/github/stratayun/paranoic/` | Тесты переносимого Compose-кода, когда он появится |

Имя пакета `io.github.stratayun.paranoic` взято из v1.5. В основе переноса добавлены
Gradle-проект `ui/`, targets `androidTarget` и `jvm("desktop")`, platform entry points,
общие Compose-компоненты и настройки. Подробности адаптации исходников и граница
с Rust — [ui/README.md](../ui/README.md). Точки входа открывают NativeCoreGateway;
отключённый gateway остаётся явным fallback ошибки старта. Генерация bindings,
упаковка native libraries, работа на устройствах и Windows не подтверждены.
Сборки и тесты не выполнялись. Оставшиеся `.gitkeep` не означают реализацию
соответствующих разделов; шифрование/заметки показывают только недоступность.

**Перенос v1.5:** выборочно адаптировать UI, не копировать Android ViewModel/JNI/OperationEngine и не вносить второй файловый движок. `commonMain` не импортирует Android классы; Android- и desktop-части подключаются через узкие интерфейсы. Хранение ключей, файловые операции и Recovery остаются за Rust Core. Миграцию UI планировать в пределах блока 4 уже существующего ROADMAP.

### `tests/` — общая проверка проекта

- `integration/` — межкрейтовые, межплатформенные и сквозные сценарии нескольких модулей.
- `interoperability/` — материалы для сравнения внешних форматов/клиентов; существующие executable Java/Maven harnesses остаются в `tools/interop/`.
- `fixtures/` — эталоны и воспроизводимые тестовые данные без реальных пользовательских секретов.
- `e2e/` — сценарии полноценного Android/Windows поведения, когда появятся приложения.

**Rust unit/integration tests остаются в `crates/<name>/src` и `crates/<name>/tests` согласно Cargo.** Тесты Compose также располагаются в соответствующих source sets. Не переносить работающие тесты ради формального единства каталогов.

### `tools/` — обслуживание разработки

- `validation/` — будущие Python-валидации, статические проверки, вспомогательные скрипты.
- `build/` — воспроизводимые сборочные и упаковочные команды.
- `interop/` — **уже существует**; сохраняется по текущему адресу.

Python может использоваться для подготовки фикстур и проверок, но не подменяет основной Rust-код проекта. Наличие папки не запускает скрипты и не разрешает CI/сборку.

### `docs/` — обоснования и результаты

- `architecture/` — границы API, интерфейсы платформ, решения о компонентах.
- `security/` — модель угроз, спецификация формата 1.8 (`docs/format/`), гарантии и ограничения.
- `audit/` — фактические отчёты ревью, выполненные проверки и их evidence.
- `roadmap/` — приложения к карте: открытые проблемы и актуальные тесты. Статусы — только в `ROADMAP.md`.

**Один источник задач — корневой `ROADMAP.md`; один действующий процесс разработки — `AGENTS.md`.** Не создавать новые параллельные карты готовности.

## Будущие Rust-крейты (пока не создавать)

- `crates/paranoic-vault/` — возможный домен собственного защищённого хранилища (1.8), только после согласования настоящего API и реализации.

Сейчас `Cargo.toml` перечисляет семь существующих крейтов явно, включая реальные
`paranoic-bridge` и `paranoic-cloud`. Не добавлять пустые `Cargo.toml`, фиктивные модули или заглушки;
не менять workspace до появления настоящего кода. Native build/generation и
поставка Pdfium описаны в [tools/build/README.md](../tools/build/README.md).

## Техническое правило размещения

**По ответственности, а не по одному языку:** Rust runtime/политики — `crates/`, экраны — `ui/`, межмодульные проверки — `tests/`, скрипты — `tools/`, документация — `docs/`. Если код работает только на Android или Windows, не переносить его в переносимый UI. При появлении функции заменять соответствующий `.gitkeep` реальными файлами, по желанию удалять маркер. Без force push; текущий `main` перепроверять перед коммитом.

**Что выполнено при утверждении схемы:** добавлены лишь этот справочник, ссылка из README/AGENTS и пустые `.gitkeep`. Функциональные Rust/Kotlin файлы, `ROADMAP.md`, Cargo/Gradle и тесты не менялись; сборки и проверки не запускались.

