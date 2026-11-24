# paranoic-formats — внешние форматы (пункт 1.7)

Документ описывает **текущее состояние**. История реализации — в git
(`git log -- crates/paranoic-formats`).

## Статус (2026-11-12)

Проверка: Linux x86_64, Rust 1.87.0 и stable; тесты `tests/block1.rs`,
`tests/block1_interop.rs`, `tests/block1_fixes.rs`
(исправления по ревью: перенос с проверкой источника, Expected-операции, запечатанный
журнал, отзыв хранилища, PrivateDirectory, задержка пароля, отзыв сессии).

**2026-11-14 — по решению владельца удалены VeraCrypt (вместе с FAT/exFAT/NTFS
внутри томов и `third_party/fatfs`) и AES Crypt.** Хранилища — собственный формат 1.8
и Cryptomator; шифрование отдельных файлов — только age. История — в git.

| Формат / функция | Код | Проверено | Чем проверено |
| --- | :---: | :---: | --- |
| age v1 (X25519, пароль/scrypt) | + | + | роундтрип, неверный ключ, порча, лимиты |
| Cryptomator формат 8, SIV_GCM | + | + | хранилища от cryptofs 2.7.2: чтение и обратная проверка записи |
| Cryptomator формат 8, SIV_CTRMAC | + | + | то же |
| Cryptomator формат 7 (SIV_CTRMAC) | + | + | хранилище от cryptofs 1.9.14: чтение и обратная проверка записи |
| Cryptomator: длинные имена (.c9s), ссылки (чтение) | + | + | фикстуры cryptofs |
| Cryptomator: создание хранилища, импорт/экспорт, папки | + | + | block1, interop |
| Cryptomator: rename/move, все классы имён, глубже по дереву | + | + | block1, interop |
| Cryptomator: удаление файла/папки (план) | + | + | interop (cryptofs подтверждает) |
| Cryptomator: смена пароля на диске, удаление бэкапа ключа | + | + | block1_interop |
| Cryptomator: recovery прерванных rename/reshape | + | частично | rename — block1; reshape repair — без теста |

**Не реализовано (сознательно или позже):** Cryptomator
форматов ≤ 6 и Cryptomator Hub (сетевые ключи); создание ссылок в Cryptomator;
ASCII armor/SSH/плагины age.

**Не проверено:** Android/Windows.

## Общие правила

- Формат выбирается явно; нет угадывания по расширению и нет fallback на другой формат.
- Расшифровка пишет только в приватный `StreamSink`; `commit` — после полной
  проверки, при ошибке/отмене — `abort`. Ошибка `commit` означает, что результат
  мог быть опубликован: автоматического повтора нет.
- Пароли и ключи — `Zeroizing`, без Debug/serde/Clone; тексты ошибок не содержат
  паролей, путей и содержимого.
- Дорогие KDF ограничиваются заранее (scrypt, PBKDF2, Argon2id); одна деривация
  не прерывается, checkpoint вызывается между шагами. Хост ограничивает число
  параллельных разблокировок и повторы пароля.
- Работу вызывать вне UI-потока. Callbacks прогресса не должны обращаться к тому
  же reader/session (сессия держит свой мьютекс во время кодека).

## age

`AgeIdentity` (генерация/импорт/экспорт X25519), `AgeFormat::encrypt_recipients`
(1–64 получателя), `encrypt_password` (scrypt work factor 14–18), `decrypt_identity`,
`decrypt_password`. Парольный файл не открывается как recipient-файл и наоборот.
Заголовок и полный объём ограничены до разбора; work factor проверяется до scrypt.
Только бинарный age v1.

## Cryptomator

**Открытие.** `CryptomatorReader::open` / `CryptomatorManager::open_reader`.
Формат 8: JWT `vault.cryptomator` (HS256/384/512, `kid` только
`masterkeyfile:masterkey.cryptomator`), `cipherCombo` SIV_GCM или SIV_CTRMAC,
порог сокращения 36–255. Формат 7: нет `vault.cryptomator`, masterkey `"version": 7`
с проверенным `versionMac`, SIV_CTRMAC, порог 220; ID сессии выводится HMAC от
MAC-ключа. `versionMac` проверяется везде, где он есть. scrypt ограничен по
памяти и работе; одновременно — одна деривация (иначе `Busy`).

**Содержимое.** SIV_GCM: заголовок 68 байт, блоки 32 КиБ + nonce 12 + тег 16.
SIV_CTRMAC: заголовок 88 байт, блоки 32 КиБ + nonce 16 + HMAC-SHA256 32, MAC
проверяется до расшифровки. В обоих форматах удаление целых последних блоков
не обнаруживается: `CryptomatorLengthPolicy::RequireTrusted(n)` сверяет с внешней
доверенной длиной, `AllowUnauthenticated` — явное согласие на этот риск. Длина из
метаданных самого шифротекста не доверенная. Откат файла на старую версию формат
не обнаруживает.

**Имена и каталоги.** AES-SIV, base64url, привязка к ID родителя; длинные имена —
обёртка `.c9s` с `name.c9s`. Обход ограничен 64 уровнями, циклы и алиасы каталогов
внутри обходимого пути отклоняются. `read_symlink` расшифровывает цель ссылки
(≤ 32 КиБ), ссылка не разыменовывается.

**Изменения (через журнал Core).** Все операции готовятся reader'ом и исполняются
очередью Core без замены существующих объектов:
- импорт: шифрование в приватный spool → `submit_file_import` (`CopyExpected` /
  `CopyExpectedBundle` для `.c9s`);
- папка: `prepare_directory` / `submit_directory` (связанная публикация данных и ссылки);
- rename/move: `prepare_move` / `submit_rename`; для папок — также переход
  c9r↔c9s (`ReshapeDirectory`) и перенос глубже по дереву после ограниченного
  обхода поддерева; для файлов c9r↔c9s — план `prepare_file_reshape`;
- многошаговые планы `CryptomatorPlan` (сериализуемые, детерминированные job ID,
  `advance_plan` подаёт шаг N только после успеха шага N-1):
  - `prepare_delete`: файл `.c9r` — один `DeleteFile`; иначе сначала атомарный
    `Move` в скрытое `.pnc-delete-<plan>`, затем удаление data-каталогов поддерева
    (перед первым удалением поддерево перепроверяется), затем отсоединённой обёртки;
  - `prepare_password_change_plan`: новый masterkey → бэкап старого
    `masterkey.cryptomator.<plan>.bkup` → новый на место; между шагами 2 и 3 файла
    ключа нет, продолжение плана завершает замену; `prepare_remove_masterkey_backup`
    удаляет бэкап только после проверки нового пароля на опубликованном файле.
- recovery: `Core::inspect_mapped_rename` / `recover_mapped_rename`,
  `inspect_directory_reshape` / `recover_directory_reshape` — только явным решением хоста.

**Обязанности хоста.** Сериализовать изменения пространства имён хранилища и
исключать внешних писателей от подготовки до завершения задания: проверки
метаданных обнаруживают изменения, но не дают снимка файловой системы. Алиасы
каталогов извне удаляемого поддерева без обхода всего хранилища не видны.
Нативные rename/reshape/recovery есть только в адаптере Linux/Android.

## Проверка вручную

```sh
cargo +1.87.0 test --workspace --locked -- --test-threads=1
cargo +1.87.0 clippy --workspace --all-targets --locked
```

Совместимость с Cryptomator: собрать `tools/interop/cryptomator` и
`tools/interop/cryptomator-v7` (Maven), выполнить `create <dir>`, запустить тесты с
`PARANOIC_CRYPTOMATOR_FIXTURES=<dir>` / `PARANOIC_CRYPTOMATOR_V7_FIXTURES=<dir>`,
затем `verify <dir>` для обратной проверки записанного. Запускать через
`java -cp <jar>:<classpath из mvn dependency:build-classpath> interop.Main …`:
под `mvn exec:java` провайдер файловой системы `cryptomator` не находится.
