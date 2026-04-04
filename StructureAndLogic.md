# BootFX: Structure And Logic

Этот документ описывает текущую архитектуру BootFX (MVP), назначение файлов и логику ключевых абстракций/функций.

## 1. Общая идея

BootFX состоит из двух фаз:

1. Текстовая boot-фаза (`boot-ui`): вывод ASCII-анимации в `tty1` + overlay из boot-логов.
2. Графическая фаза (`boot-video-player`): продолжение видео с точки, где закончилась текстовая фаза.

Подготовка ASCII-данных выполняется офлайн через `boot-ui-precompute`.

## 2. Логический pipeline

1. `boot-ui-precompute` читает source-видео и генерирует:
   - бинарные кадры `frames/*.frame`
   - `manifest.json` (индекс кадров, `pts_ms`, размер, fps)
2. `boot-ui` во время загрузки:
   - читает `config.toml`
   - читает `manifest.json`
   - рендерит кадры в терминал
   - поверх рисует строки из `journalctl`
   - при завершении пишет `/run/boot-ui/state.json`
3. `boot-video-player`:
   - читает `state.json`
   - выбирает видео и плеер
   - запускает плеер с `--start`/`-ss`/`--start-time` по `pts_ms`

## 3. Структура workspace и задачи файлов

### Корень

- `Cargo.toml`  
  Workspace на 4 crate: `bootfx-core`, `boot-ui`, `boot-ui-precompute`, `boot-video-player`.
- `README.md`  
  Инструкции установки/запуска (включая Arch Linux).
- `StructureAndLogic.md`  
  Этот файл.

### `bootfx-core/`

- `bootfx-core/Cargo.toml`  
  Общие зависимости сериализации/конфига.
- `bootfx-core/src/lib.rs`  
  Единые модели данных и утилиты чтения/записи:
  - `Config` и секции конфига
  - `Manifest`, `FrameMeta`
  - `State`
  - дефолтные пути (`DEFAULT_CONFIG_PATH`, `DEFAULT_STATE_PATH`)

### `boot-ui-precompute/`

- `boot-ui-precompute/Cargo.toml`  
  Зависимости `image`, `bootfx-core`.
- `boot-ui-precompute/src/main.rs`  
  Офлайн-конвертер видео -> ASCII frames + manifest.

### `boot-ui/`

- `boot-ui/Cargo.toml`  
  Зависимость на `bootfx-core`.
- `boot-ui/src/main.rs`  
  Основной runtime-рендерер для boot-фазы.

### `boot-video-player/`

- `boot-video-player/Cargo.toml`  
  Зависимость на `bootfx-core`.
- `boot-video-player/src/main.rs`  
  Пост-boot запуск графического плеера с resume.

### `packaging/`

- `packaging/example-config.toml`  
  Базовый конфиг (`/etc/boot-ui/config.toml`).
- `packaging/boot-ui.service`  
  systemd unit для текстовой boot-фазы.
- `packaging/boot-video-player.service`  
  systemd unit для графического продолжения.
- `packaging/boot-video-player.path`  
  Триггер запуска видео-плеера по появлению `state.json`.
- `packaging/video-session.env`
  Optional override file for graphical session variables (`DISPLAY`, `XDG_RUNTIME_DIR`, `XAUTHORITY`, `WAYLAND_DISPLAY`).
- `packaging/install-arch.sh`  
  Установочный скрипт под Arch Linux.

## 4. Ключевые абстракции (`bootfx-core`)

### `Config`

Содержит секции:

- `screen` (`width`, `height`, `fps`)
- `layering` (`order`, например `["animation", "systemd"]`)
- `overlay` (`region_y`, `region_h`)
- `animation` (`manifest`)
- `handoff` (`write_state`)
- `video` (`source`, `player`, `args`)
- `debug`:
  - `log_file`, `history_file`
  - `export_enabled`, `export_dir`
  - `flush_every`, флаги детализации логов
  - cleanup/retention параметры (`cleanup_enabled`, `max_artifact_age_days`, `max_artifacts`, `max_log_size_mb`, `max_history_size_mb`)

Методы:

- `Config::load_from_path(...)` — чтение TOML + валидация.
- `Config::validate()` — проверка базовых инвариантов (размеры, fps, непустые пути).

### `Manifest` / `FrameMeta`

- `Manifest` задает метаданные ASCII-анимации и список кадров.
- `FrameMeta` содержит:
  - `index`
  - `pts_ms`
  - `file` (относительный путь до `.frame`)

Методы:

- `Manifest::load_from_path(...)`
- `Manifest::write_to_path(...)`
- `Manifest::validate()` (в т.ч. `frame_count == frames.len()`).

### `State`

Файл handoff-состояния:

- `frame_index`
- `pts_ms`

Методы:

- `State::load_from_path(...)`
- `State::write_to_path(...)`

## 5. Логика `boot-ui-precompute`

`boot-ui-precompute/src/main.rs`:

1. `parse_args()` — разбор CLI-аргументов.
2. `ensure_ffmpeg()` — проверка наличия `ffmpeg`.
3. `extract_frames_with_ffmpeg(...)` — извлечение PNG-кадров фиксированного размера/частоты.
4. `collect_png_frames(...)` — сбор и сортировка кадров.
5. Для каждого PNG:
   - загрузка grayscale-изображения
   - преобразование в ASCII:
     - `grayscale_to_ascii(...)` или
     - `edges_to_ascii(...)` (Sobel-подобный градиент)
   - запись `NNNNNN.frame`
   - формирование `FrameMeta` (`pts_ms = index * 1000 / fps`)
6. Сборка и запись `manifest.json`.
7. Очистка временной директории `.tmp-png`.

Вспомогательное:

- `map_luma_to_ascii(...)` — маппинг яркости к символу из charset.

## 6. Логика `boot-ui` (runtime)

`boot-ui/src/main.rs`:

1. `parse_args()` — опции `--config`, `--max-frames`.
2. `run()`:
   - загрузка `Config` и `Manifest`
   - проверка соответствия размеров экрана и манифеста
   - запуск фоновых задач:
     - `spawn_journal_reader(...)`
     - `spawn_graphical_target_watcher(...)`
   - вход в raw-like терминальный режим через `TerminalGuard::enter()`
   - цикл кадров:
     - чтение `.frame`
     - снимок overlay (`snapshot_overlay_lines(...)`)
     - композиция слоев (`compose_layers(...)`)
     - вывод в терминал (`render_frame(...)`)
     - обновление `last_state`
   - запись `state.json` через `write_handoff_state(...)`

### Overlay-подсистема

- `spawn_journal_reader(...)` читает `journalctl -b -f -n 0 -o cat`.
- `classify_journal_line(...)` грубо размечает строки:
  - `[FAILED]` при `failed`
  - `[  OK  ]` при `started`
  - `[    ]` при `starting`
  - иначе `[INFO ]`
- `push_overlay_line(...)` поддерживает кольцевой буфер (`VecDeque`) ограниченного размера.
- `sanitize_ascii_line(...)` оставляет только печатный ASCII.

### Композиция

- `compose_layers(...)` применяет порядок из `config.layering.order`.
- `build_overlay_layer(...)` превращает список строк в прозрачный слой.
- `blit(...)` копирует непрозрачные байты (`byte != 0`).

### Управление lifecycle

- `spawn_graphical_target_watcher(...)` раз в секунду проверяет `systemctl is-active graphical.target`.
- При достижении графической цели цикл кадров завершается, затем пишется handoff-state.

### `TerminalGuard`

RAII-обертка:

- при входе скрывает курсор и очищает экран
- в `Drop` возвращает курсор и сбрасывает состояние терминала

### Debug artifacts и retention

- При завершении `boot-ui` формирует bundle в `debug`-директории проекта (`/var/lib/boot-ui/debug` по умолчанию):
  - копии `config.toml`, `manifest.json`, `state.json`, `boot-ui.log`, `boot-ui-history.log`
  - общий файл `debug-summary.txt`
  - глобальный свежий агрегат `debug-latest.txt` в корне `debug/`
- Автоочистка удаляет слишком старые debug-бандлы и ограничивает их количество.
- При переполнении основных логов выполняется ротация (`*.old-<timestamp>`).

## 7. Логика `boot-video-player`

`boot-video-player/src/main.rs`:

1. `parse_args()` — `--config`, `--state`, `--video`, `--dry-run`.
2. `run()`:
   - загрузка `Config`
   - чтение `State` (или `0ms`, если файла нет)
   - удаление (consume) `state.json` после чтения, чтобы избежать повторных trigger-циклов `.path` при падении плеера
   - выбор пути видео (`select_video_path(...)`)
   - выбор плеера (`choose_player(...)`)
   - автоопределение параметров графической сессии через `loginctl` и `/proc/<leader>/environ` (DISPLAY/XDG_RUNTIME_DIR/XAUTHORITY/WAYLAND_DISPLAY)
   - ожидание короткого окна до появления "usable" графической среды перед запуском плеера
   - fallback для SDDM по `/run/sddm/xauth_*`, если `loginctl` еще не дал готовую пользовательскую сессию
   - для Wayland-сессий без `XAUTHORITY` не устанавливается fallback `DISPLAY=:0`, чтобы не провоцировать X11 crash-path в `mpv`
   - сбор команды (`build_player_command(...)`)
   - запуск плеера (кроме `--dry-run`)

`build_player_command(...)` знает про разные флаги старта:

- `mpv`: `--start=<sec>`
- `ffplay`: `-ss <sec>`
- `vlc`: `--start-time <sec>`

## 8. Systemd-слой и handoff

### `boot-ui.service`

- Конфликтует с `getty@tty1.service`
- Запускает `boot-ui --config /etc/boot-ui/config.toml`
- Работает с `TTYPath=/dev/tty1`
- Готовит runtime dir `/run/boot-ui` через `RuntimeDirectory=boot-ui`
- Сохраняет runtime dir после остановки через `RuntimeDirectoryPreserve=yes` (важно для handoff `state.json`)
- Включается в `basic.target`, чтобы стартовать раньше `graphical.target`

### `boot-video-player.path`

- Следит за `PathExists=/run/boot-ui/state.json`
- При появлении файла стартует `boot-video-player.service`

### `boot-video-player.service`

- Запускает `boot-video-player` с путями к config и state
- Содержит `ConditionPathExists=/run/boot-ui/state.json`
- Читает опциональный override-файл `/etc/boot-ui/video-session.env`
- Упорядочен после `display-manager.service` для более надежной графической сессии перед запуском плеера

## 9. Форматы данных

### `.frame`

- Сырым массивом `width * height` байт
- 1 байт = 1 символ ASCII-клетки

### `manifest.json`

Содержит:

- `fps`, `width`, `height`
- `frame_count`
- `frames[]` с `index`, `pts_ms`, `file`

### `state.json`

Содержит:

- `frame_index`
- `pts_ms`

## 10. Точки расширения

1. Улучшить overlay:
   - заменить простую классификацию journald на полноценный D-Bus/systemd parser.
2. Улучшить renderer:
   - diff rendering (не перерисовывать целый экран).
3. Улучшить handoff:
   - синхронизация с графической сессией более надежным способом.
4. Добавить тесты:
   - unit-тесты для парсинга/валидации config/manifest/state
   - smoke-тесты pipeline precompute -> boot-ui -> state.
