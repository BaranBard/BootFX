#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="/etc/boot-ui/config.toml"
ASSET_DIR="/var/lib/boot-ui/intro"

VIDEO_INPUT=""
MODE="grayscale"
WIDTH="120"
HEIGHT="40"
FPS="15"
ENABLE_UNITS="0"

usage() {
  cat <<'EOF'
BootFX Arch installer

Usage:
  ./packaging/install-arch.sh [options]

Options:
  --video <path>      Source video to precompute into ASCII frames.
  --mode <name>       grayscale | edges (default: grayscale)
  --width <n>         Character grid width (default: 120)
  --height <n>        Character grid height (default: 40)
  --fps <n>           Target FPS (default: 15)
  --enable            Enable systemd units after install.
  -h, --help          Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --video)
      VIDEO_INPUT="${2:-}"
      shift 2
      ;;
    --mode)
      MODE="${2:-}"
      shift 2
      ;;
    --width)
      WIDTH="${2:-}"
      shift 2
      ;;
    --height)
      HEIGHT="${2:-}"
      shift 2
      ;;
    --fps)
      FPS="${2:-}"
      shift 2
      ;;
    --enable)
      ENABLE_UNITS="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

for cmd in cargo install systemctl sudo; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "Missing required command: ${cmd}" >&2
    exit 1
  fi
done

if [[ -n "${VIDEO_INPUT}" ]]; then
  if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "Missing required command: ffmpeg (needed for --video)." >&2
    exit 1
  fi
fi

echo "[1/6] Building release binaries"
cargo build --release --workspace --manifest-path "${ROOT_DIR}/Cargo.toml"

echo "[2/6] Installing binaries to /usr/bin"
sudo install -Dm755 "${ROOT_DIR}/target/release/boot-ui" /usr/bin/boot-ui
sudo install -Dm755 "${ROOT_DIR}/target/release/boot-ui-precompute" /usr/bin/boot-ui-precompute
sudo install -Dm755 "${ROOT_DIR}/target/release/boot-video-player" /usr/bin/boot-video-player

echo "[3/6] Installing config and systemd units"
sudo install -d -m755 /etc/boot-ui
if ! sudo test -f "${CONFIG_PATH}"; then
  sudo install -Dm644 "${ROOT_DIR}/packaging/example-config.toml" "${CONFIG_PATH}"
  echo "Installed default config: ${CONFIG_PATH}"
else
  echo "Config already exists, keeping current file: ${CONFIG_PATH}"
fi

sudo install -Dm644 "${ROOT_DIR}/packaging/boot-ui.service" /etc/systemd/system/boot-ui.service
sudo install -Dm644 "${ROOT_DIR}/packaging/boot-video-player.service" /etc/systemd/system/boot-video-player.service
sudo install -Dm644 "${ROOT_DIR}/packaging/boot-video-player.path" /etc/systemd/system/boot-video-player.path

echo "[4/6] Preparing asset directory"
sudo install -d -m755 "${ASSET_DIR}"

if [[ -n "${VIDEO_INPUT}" ]]; then
  echo "[5/6] Copying source video and precomputing ASCII frames"
  sudo install -Dm644 "${VIDEO_INPUT}" "${ASSET_DIR}/video.mp4"
  sudo /usr/bin/boot-ui-precompute \
    --input "${ASSET_DIR}/video.mp4" \
    --output-dir "${ASSET_DIR}" \
    --width "${WIDTH}" \
    --height "${HEIGHT}" \
    --fps "${FPS}" \
    --mode "${MODE}"
else
  echo "[5/6] Skipping precompute (--video not provided)"
  echo "Run boot-ui-precompute manually when a source video is available."
fi

echo "[6/6] Reloading systemd"
sudo systemctl daemon-reload

if [[ "${ENABLE_UNITS}" == "1" ]]; then
  sudo systemctl enable boot-ui.service
  sudo systemctl enable boot-video-player.path
  echo "Enabled: boot-ui.service, boot-video-player.path"
else
  echo "Units not enabled automatically. Use:"
  echo "  sudo systemctl enable boot-ui.service boot-video-player.path"
fi

cat <<'EOF'

Install completed.
Next checks:
  sudo boot-ui --config /etc/boot-ui/config.toml --max-frames 120
  sudo boot-video-player --config /etc/boot-ui/config.toml --dry-run

EOF
