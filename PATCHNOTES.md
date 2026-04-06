# Patch Notes

## 2026-04-06

### Added

- New `[sddm]` config section in `bootfx-core`:
  - `video_background_enabled`
  - `theme`
  - `theme_root`
  - `video_path`
  - `launch_external_player`
- New helper script `packaging/patch-sddm-theme-video.sh` for one-time SDDM theme patching (injects video block into `Main.qml`, creates backup).
- Arch installer support for SDDM patch workflow:
  - `--patch-sddm-theme`
  - `--sddm-theme`
  - `--sddm-theme-root`
  - installs helper to `/usr/bin/bootfx-patch-sddm-theme`

### Changed

- `boot-video-player` now supports SDDM continuation mode:
  - updates `<theme>/theme.conf.user` with BootFX keys (`BootFXVideoEnabled`, `BootFXVideoPath`, `BootFXStartMs`, `BootFXUseVideoBackground`)
  - can skip external player launch when `sddm.launch_external_player=false`
  - supports `--dry-run` reporting for planned SDDM update
- `boot-video-player.service` ordering changed to run before `display-manager.service` (after `boot-ui.service`) so SDDM config updates can happen earlier in boot.
- `README.md` updated with SDDM setup, rollback, and troubleshooting steps.
- `StructureAndLogic.md` updated with SDDM continuation architecture notes.

## 2026-04-04

### Added

- Runtime debug logging subsystem in `boot-ui`.
- Full in-memory history buffer with periodic snapshot flush to file.
- Panic hook logging for easier post-mortem analysis.
- Configurable debug section in config (`[debug]`).
- Systemd log directory creation via `LogsDirectory=boot-ui`.
- Arch installer creates `/var/log/boot-ui`.
- Automatic debug artifact export into project-local directory (`/var/lib/boot-ui/debug`).
- Per-run debug bundle folders (`run-<timestamp>`) with copied files:
  - `config.toml`, `manifest.json`, `state.json`, `boot-ui.log`, `boot-ui-history.log`
  - combined `debug-summary.txt`
- Global combined latest file: `/var/lib/boot-ui/debug/debug-latest.txt`.
- Retention and cleanup options in config (age/count limits + log size rotation).
- Session environment auto-detection in `boot-video-player` using `loginctl` and `/proc/<leader>/environ`.
- New optional session override file: `/etc/boot-ui/video-session.env` (template in `packaging/video-session.env`).
- New manual console rendering flag: `boot-ui --force-console` (ignores `graphical.target` stop condition).
- New built-in fallback render mode: `boot-ui --donut` (spinning ASCII 3D donut).
- New fullscreen output test mode: `boot-ui --hash-test` (fills screen with `#`).

### Changed

- `boot-ui` now records startup, manifest load, frame events, overlay events, watcher checks, and handoff write events.
- `README.md` includes runtime debug files and commands to collect a debug bundle.
- `StructureAndLogic.md` updated to include `debug` config abstraction.
- `boot-ui.service` now preserves `/run/boot-ui` after service stop (`RuntimeDirectoryPreserve=yes`) so `state.json` survives handoff.
- `RequiresMountsFor=/var/lib/boot-ui` moved to correct section `[Unit]`.
- `boot-ui` install target switched to `basic.target` to start earlier in boot sequence.
- `README.md` and `StructureAndLogic.md` expanded with debug bundle and cleanup workflow.
- `boot-video-player.service` now reads optional overrides from `/etc/boot-ui/video-session.env` and waits for `display-manager.service`.
- Arch installer now installs `/etc/boot-ui/video-session.env` if missing.
- Fixed Rust ownership bug in `boot-ui` log rotation path construction (`E0382` compile error on Arch).
- `boot-video-player` now consumes/removes `/run/boot-ui/state.json` after reading it to prevent repeated `.path` retriggers on player failure.
- Wayland sessions without `XAUTHORITY` now avoid forced `DISPLAY=:0` fallback to prevent mpv X11 assertion crashes in some VM/display stacks.
- `boot-video-player` now waits briefly for a usable graphical session env before launching the player, reducing early-boot race conditions.
- Added SDDM xauth fallback detection from `/run/sddm/xauth_*` for systems where `loginctl` session data is not ready yet.
- `boot-ui` now auto-falls back to donut mode when manifest file is missing (useful when no precomputed video assets are installed).
- `boot-ui` now skips handoff state writes in test/forced modes (`--force-console`, `--donut`, `--hash-test`) to avoid accidental `boot-video-player.path` triggers during manual checks.

### Debug Artifacts

- Main log: `/var/log/boot-ui/boot-ui.log`
- History buffer snapshot: `/var/log/boot-ui/boot-ui-history.log`
- Handoff state: `/run/boot-ui/state.json`
