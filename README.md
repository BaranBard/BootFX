# BootFX — Animated ASCII Boot for Linux

BootFX is a custom boot interface system for Linux (Arch-first) that renders **fullscreen ASCII animation** synchronized with the **real systemd boot process**, with optional **graphical video continuation** after boot completes.

It doesn't replace or simulate the boot — it acts as a **live renderer of the actual boot process**, displaying systemd events on top of text-based animation.

## Features

- Fullscreen ASCII animation (video → text)
- Real systemd events via D-Bus + journald
- systemd-style status output: `[ OK ]`, `[FAILED]`, `[INFO ]`
- Layer-based compositing (animation + systemd overlay)
- Full configuration via `config.toml`
- Frame-accurate handoff to graphical video player via `pts_ms`
- Runs in pure TTY (no X11/Wayland required)
- Extensible architecture

## How It Works

During boot:

1. systemd runs **as usual**
2. Default console output is **hidden**
3. BootFX takes over `tty1`
4. BootFX reads systemd events (D-Bus), reads the journal (journald), plays ASCII animation, and inserts real boot messages into the overlay
5. When `graphical.target` is reached, BootFX saves its state (frame index + timestamp) and hands off to the graphical video player

```
UEFI → systemd-boot → kernel → systemd → boot-ui → graphical player
```

## Architecture

### `boot-ui-precompute`

Offline tool that prepares animation data:

- Converts `mp4` → ASCII frames
- Generates `manifest.json`
- Extracts audio (optional)

### `boot-ui`

Runtime component:

- Launches during boot, captures `tty1`
- Plays ASCII animation from precomputed frames
- Listens to systemd via D-Bus
- Composites layers and draws each frame
- Performs handoff when boot completes

### `boot-video-player`

Post-boot graphical stage:

- Reads `state.json` from `boot-ui`
- Launches the original video
- Continues playback from the exact handoff point

## Rendering Model

The screen is a grid of `120×40` characters (configurable). Each frame is composited from two layers: **animation** (background) and **systemd** (overlay). For each cell, layers are drawn in order — non-transparent characters overwrite transparent ones (`0x00` = transparent).

### ASCII Animation Modes

| Mode | Description |
|------|-------------|
| `grayscale` | Brightness mapped to ASCII density |
| `edges` | Edge-detected outlines |

Pipeline: `mp4 → frames → resize → grayscale/edges → ASCII → .frame`

Each `.frame` file is binary: `width × height` bytes, 1 byte per cell.

### systemd Overlay

Events sourced from D-Bus (`org.freedesktop.systemd1`) and journald:

```
[    ] Starting Network Manager...
[  OK  ] Started Network Manager.
[FAILED] Failed to start Docker.
```

## Configuration

`/etc/boot-ui/config.toml`:

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
manifest = "/boot/boot-ui/intro/manifest.json"

[handoff]
write_state = "/run/boot-ui/state.json"
```

## Manifest Format

`manifest.json` describes the precomputed animation:

```json
{
  "fps": 15,
  "width": 120,
  "height": 40,
  "frame_count": 742,
  "frames": [
    { "index": 0, "pts_ms": 0, "file": "000000.frame" }
  ]
}
```

## Handoff

When boot completes, `boot-ui` writes `state.json`:

```json
{
  "frame_index": 239,
  "pts_ms": 15933
}
```

The graphical player reads this to resume the original video seamlessly.

## Audio

Audio is **disabled** during the ASCII boot phase (ALSA may not be ready, hardware-dependent instability). Audio playback is supported in the graphical continuation phase.

## Project Structure

```
bootfx/
├── boot-ui/                  # Runtime ASCII renderer
│   ├── Cargo.toml
│   └── src/main.rs
├── boot-ui-precompute/       # Offline video → ASCII converter
│   ├── Cargo.toml
│   └── src/main.rs
├── boot-video-player/        # Post-boot graphical player
│   ├── Cargo.toml
│   └── src/main.rs
├── packaging/
│   ├── boot-ui.service
│   ├── boot-video-player.service
│   └── example-config.toml
├── assets/
├── docs/
├── Cargo.toml                # Workspace root
├── .gitignore
└── README.md
```

## Requirements

### Runtime

- Linux kernel ≥ 5.x
- systemd ≥ 250
- D-Bus
- journald
- Access to `/dev/tty1`
- Root privileges

### Precompute

- ffmpeg
- Sufficient RAM/CPU for video processing

## Supported Platforms

**Primary target:** Arch Linux

Also compatible with other systemd-based distributions: Fedora, Debian, Ubuntu.

## Installation (Planned)

```bash
# 1. Build the project
cargo build --release

# 2. Install binaries
sudo install -m 755 target/release/boot-ui /usr/bin/
sudo install -m 755 target/release/boot-video-player /usr/bin/

# 3. Configure
sudo mkdir -p /etc/boot-ui
sudo cp packaging/example-config.toml /etc/boot-ui/config.toml

# 4. Enable systemd services
sudo systemctl enable boot-ui.service
sudo systemctl enable boot-video-player.service

# 5. Update kernel cmdline (hide default console output)
# Add: quiet splash
```

## Roadmap

**MVP** — ASCII video playback, D-Bus integration, systemd-style overlay, frame compositor, manifest loader, `state.json` handoff

**Phase 2** — Edge rendering, layer ordering config, graphical continuation, performance optimization

**Phase 3** — Color ASCII, diff rendering, audio sync, multiple overlays

## Limitations

- No true graphical effects in TTY phase — ASCII characters only
- No alpha channel or blending — binary transparency
- No GPU acceleration
- TTY output is character-cell based

## Philosophy

BootFX is not a "pretty splash screen." It is a **live boot interface** that renders the real systemd startup process in an artistic ASCII form — bridging function and aesthetics.

## Contributing

PRs are welcome. Areas of interest: render optimization, new ASCII filters, terminal compatibility, packaging for different distros.

## License

TBD
