# Paranoic v2.1

Новая реализация защищённого файлового менеджера с нуля: Android + Windows.

**Кто что делает сейчас:** [WORK.md](WORK.md).

**Единственная карта задач:** [ROADMAP.md](ROADMAP.md) — очередь задач с отдельными отметками кода, UI, тестов и телефона; проблемы и тесты — в [docs/roadmap](docs/roadmap/).

**Правила для исполнителей:** [AGENTS.md](AGENTS.md).

**Структура исходников:** [docs/REPOSITORY_LAYOUT.md](docs/REPOSITORY_LAYOUT.md).

## Рабочий принцип

- **Rust-first:** ядро, файловые операции, шифрование, архивы, документы и облачные адаптеры — Rust.
- **Интерфейс:** Kotlin Multiplatform + Compose; связь с Rust через UniFFI (`crates/paranoic-bridge`). Интерфейс сначала минимальный, дизайн позже.
- **Облака:** Google Drive, OneDrive, pCloud, собственный сервер; без Go в новой версии.
- Все завершённые изменения сразу коммитятся в **main**. Скачать весь текущий код: GitHub → **Code → Download ZIP**.
- **main = актуальный код, а не гарантированная стабильная версия.** Проверки только по отдельной команде владельца после окончания крупного блока. Непроверенный код отмечается как непроверенный.
- Не копировать старые реализации и ограничения без необходимости; новую структуру развивать модульно.

## Текущее ядро

В Cargo workspace написаны `paranoic-core` (API, очередь, журнал SQLite,
восстановление), `paranoic-local` (доступ к native-каталогам через capability handles)
и `paranoic-crypto` (аутентифицированное шифрование, ключи, сессии).
Описание контрактов, подключения и ограничений: [Core API](crates/paranoic-core/README.md).
Кодовая часть 1.6 и границы её ответственности: [Crypto Engine](crates/paranoic-crypto/README.md).
В `paranoic-formats` (1.7) написаны [адаптеры age и Cryptomator (форматы 7/8)](crates/paranoic-formats/README.md) (VeraCrypt и AES Crypt удалены 2026-11-14).

**Состояние, план и отметки готовности** — только в [ROADMAP.md](ROADMAP.md) (разделы «Очередь» и «Покрытие функциональности»); проблемы и тесты — [docs/roadmap](docs/roadmap/).

Проверка вручную (Linux): сначала `cargo build -p paranoic-server -p paranoic-documents --features paranoic-documents/pdf`, затем `cargo test --workspace --locked --features paranoic-documents/pdf -- --test-threads=1`.
Совместимость с Cryptomator: `tools/interop/` (Java, Maven) создаёт хранилища и
передаётся тестам через `PARANOIC_CRYPTOMATOR_FIXTURES` / `PARANOIC_CRYPTOMATOR_V7_FIXTURES`.

