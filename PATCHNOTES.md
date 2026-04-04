# Patch Notes

## 2026-04-04

### Added

- Runtime debug logging subsystem in `boot-ui`.
- Full in-memory history buffer with periodic snapshot flush to file.
- Panic hook logging for easier post-mortem analysis.
- Configurable debug section in config (`[debug]`).
- Systemd log directory creation via `LogsDirectory=boot-ui`.
- Arch installer creates `/var/log/boot-ui`.

### Changed

- `boot-ui` now records startup, manifest load, frame events, overlay events, watcher checks, and handoff write events.
- `README.md` includes runtime debug files and commands to collect a debug bundle.
- `StructureAndLogic.md` updated to include `debug` config abstraction.

### Debug Artifacts

- Main log: `/var/log/boot-ui/boot-ui.log`
- History buffer snapshot: `/var/log/boot-ui/boot-ui-history.log`
- Handoff state: `/run/boot-ui/state.json`
