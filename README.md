# BootFX - Animated ASCII Boot for Linux (MVP)

BootFX is a Linux boot UX experiment (Arch-first): it plays a fullscreen ASCII animation on `tty1`, overlays real boot log lines, and then hands off to a graphical video continuation.

> [!WARNING]
> This project is early-stage and partially AI-generated.
> Use it on your own risk, preferably on a test machine or VM first.
> It can contain bugs or unsafe assumptions that may break your boot workflow.

## Current Status

What works in this MVP:

- `boot-ui-precompute`: converts a source video into ASCII `.frame` files + `manifest.json`
- `boot-ui`: plays precomputed frames in TTY and overlays live journald lines
- `boot-video-player`: reads `/run/boot-ui/state.json` and starts video from `pts_ms`
- Systemd packaging for boot stage and handoff trigger (`.service` + `.path`)

What is not complete yet:

- No full D-Bus unit-state pipeline (overlay is currently journald-based)
- Limited compositor features (binary transparency, no alpha blending)

## Components

- `bootfx-core`: shared config/manifest/state models
- `boot-ui-precompute`: offline video -> ASCII assets
- `boot-ui`: boot-time ASCII renderer
- `boot-video-player`: post-boot resume player

## Architecture And Logic

For a detailed explanation of architecture, file responsibilities, abstractions, and core functions, see:

- [StructureAndLogic.md](./StructureAndLogic.md)
- [PATCHNOTES.md](./PATCHNOTES.md)

## Arch Linux: Quick Start

### 1. Install dependencies

```bash
sudo pacman -Syu --needed base-devel git rustup ffmpeg mpv
rustup default stable
```

### 2. Clone repository

```bash
git clone https://github.com/BaranBard/BootFX.git
cd BootFX
```

### 3. One-command install (recommended)

Use installer script (it builds, installs binaries, installs units/config, optionally precomputes assets):

```bash
bash packaging/install-arch.sh --video /absolute/path/to/intro.mp4 --mode grayscale --fps 15 --width 120 --height 40 --enable
```

If you skip `--video`, script will install everything except frame assets.

### 4. Reboot and test

```bash
sudo reboot
```

## Arch Linux: Manual Install (step by step)

### 1. Build binaries

```bash
cargo build --release --workspace
```

### 2. Install binaries

```bash
sudo install -Dm755 target/release/boot-ui /usr/bin/boot-ui
sudo install -Dm755 target/release/boot-ui-precompute /usr/bin/boot-ui-precompute
sudo install -Dm755 target/release/boot-video-player /usr/bin/boot-video-player
```

### 3. Install config and units

```bash
sudo install -d -m755 /etc/boot-ui
sudo install -Dm644 packaging/example-config.toml /etc/boot-ui/config.toml

sudo install -Dm644 packaging/boot-ui.service /etc/systemd/system/boot-ui.service
sudo install -Dm644 packaging/boot-video-player.service /etc/systemd/system/boot-video-player.service
sudo install -Dm644 packaging/boot-video-player.path /etc/systemd/system/boot-video-player.path

sudo systemctl daemon-reload
```

### 4. Prepare assets

By default, config expects assets in `/var/lib/boot-ui/intro`.

```bash
sudo install -d -m755 /var/lib/boot-ui/intro
sudo install -d -m755 /var/log/boot-ui
sudo install -Dm644 /absolute/path/to/intro.mp4 /var/lib/boot-ui/intro/video.mp4

sudo /usr/bin/boot-ui-precompute \
  --input /var/lib/boot-ui/intro/video.mp4 \
  --output-dir /var/lib/boot-ui/intro \
  --mode grayscale \
  --fps 15 \
  --width 120 \
  --height 40
```

### 5. Validate manually before enabling at boot

```bash
sudo /usr/bin/boot-ui --config /etc/boot-ui/config.toml --max-frames 120
sudo /usr/bin/boot-video-player --config /etc/boot-ui/config.toml --dry-run
```

### 6. Enable units

```bash
sudo systemctl enable boot-ui.service
sudo systemctl enable boot-video-player.path
```

Optional: if you do not want graphical continuation yet, skip `boot-video-player.path`.

### 7. Optional kernel cmdline cleanup

To reduce default boot noise, add `quiet splash` to kernel parameters.

## Runtime Files

- Config: `/etc/boot-ui/config.toml`
- Assets: `/var/lib/boot-ui/intro/`
- Handoff state: `/run/boot-ui/state.json`
- Debug log: `/var/log/boot-ui/boot-ui.log`
- Debug history buffer snapshot: `/var/log/boot-ui/boot-ui-history.log`

Default config example:

```toml
[screen]
width = 120
height = 40
fps = 15

[layering]
order = ["animation", "systemd"]

[overlay]
region_y = 24
region_h = 16

[animation]
manifest = "/var/lib/boot-ui/intro/manifest.json"

[handoff]
write_state = "/run/boot-ui/state.json"

[video]
source = "/var/lib/boot-ui/intro/video.mp4"
player = "mpv"
args = ["--fullscreen"]

[debug]
log_file = "/var/log/boot-ui/boot-ui.log"
history_file = "/var/log/boot-ui/boot-ui-history.log"
flush_every = 64
log_frame_events = true
log_overlay_events = true
```

## Troubleshooting

### Check service status

```bash
systemctl status boot-ui.service
systemctl status boot-video-player.path
systemctl status boot-video-player.service
```

### Check logs

```bash
journalctl -u boot-ui.service -b
journalctl -u boot-video-player.service -b
sudo tail -n 200 /var/log/boot-ui/boot-ui.log
sudo tail -n 200 /var/log/boot-ui/boot-ui-history.log
```

### Files To Share For Debug Review

If animation did not play correctly, please send these files after one full boot attempt:

```text
/etc/boot-ui/config.toml
/var/log/boot-ui/boot-ui.log
/var/log/boot-ui/boot-ui-history.log
/run/boot-ui/state.json
/var/lib/boot-ui/intro/manifest.json
```

Also send command outputs:

```bash
systemctl status boot-ui.service --no-pager
systemctl status boot-video-player.service --no-pager
systemctl status boot-video-player.path --no-pager
journalctl -u boot-ui.service -b --no-pager
journalctl -u boot-video-player.service -b --no-pager
```

Optional single bundle command:

```bash
sudo tar -czf /tmp/bootfx-debug-$(date +%F-%H%M%S).tar.gz \
  /etc/boot-ui/config.toml \
  /var/log/boot-ui/boot-ui.log \
  /var/log/boot-ui/boot-ui-history.log \
  /run/boot-ui/state.json \
  /var/lib/boot-ui/intro/manifest.json
```

### Common issues

- `boot-ui` exits immediately:
  - Verify `manifest.json` exists and frame files are readable.
- `boot-video-player` does not start:
  - Verify `/run/boot-ui/state.json` exists after `boot-ui` run.
  - Check `boot-video-player.path` is enabled and active.
- Player window not visible in graphical session:
  - Your display stack may need custom `DISPLAY`/session setup; adjust `boot-video-player.service` accordingly.

## Safe Rollback

```bash
sudo systemctl disable --now boot-ui.service boot-video-player.path boot-video-player.service
sudo rm -f /etc/systemd/system/boot-ui.service
sudo rm -f /etc/systemd/system/boot-video-player.service
sudo rm -f /etc/systemd/system/boot-video-player.path
sudo systemctl daemon-reload
```

## Project Layout

```text
bootfx/
|- bootfx-core/
|- boot-ui/
|- boot-ui-precompute/
|- boot-video-player/
|- packaging/
|  |- boot-ui.service
|  |- boot-video-player.service
|  |- boot-video-player.path
|  |- example-config.toml
|  '- install-arch.sh
|- assets/
|- docs/
|- Cargo.toml
'- README.md
```

## License

TBD
