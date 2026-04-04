# Patch Notes

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

### Debug Artifacts

- Main log: `/var/log/boot-ui/boot-ui.log`
- History buffer snapshot: `/var/log/boot-ui/boot-ui-history.log`
- Handoff state: `/run/boot-ui/state.json`
