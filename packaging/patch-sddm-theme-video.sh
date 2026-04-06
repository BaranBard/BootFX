#!/usr/bin/env bash
set -euo pipefail

THEME="breeze"
THEME_ROOT="/usr/share/sddm/themes"
MODE="enable"

usage() {
  cat <<'EOF'
BootFX SDDM video background patcher

Usage:
  ./packaging/patch-sddm-theme-video.sh [options]

Options:
  --theme <name>       SDDM theme name (default: breeze)
  --theme-root <path>  SDDM themes root (default: /usr/share/sddm/themes)
  --enable             Patch theme Main.qml to add BootFX video background block (default)
  --disable            Restore Main.qml from backup
  --status             Show patch status for selected theme
  -h, --help           Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --theme)
      THEME="${2:-}"
      shift 2
      ;;
    --theme-root)
      THEME_ROOT="${2:-}"
      shift 2
      ;;
    --enable)
      MODE="enable"
      shift
      ;;
    --disable)
      MODE="disable"
      shift
      ;;
    --status)
      MODE="status"
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

THEME_DIR="${THEME_ROOT}/${THEME}"
MAIN_QML="${THEME_DIR}/Main.qml"
BACKUP_QML="${THEME_DIR}/Main.qml.bootfx.bak"

if [[ ! -f "${MAIN_QML}" ]]; then
  echo "Main.qml not found: ${MAIN_QML}" >&2
  exit 1
fi

if [[ "${MODE}" == "status" ]]; then
  echo "Theme: ${THEME}"
  echo "Theme dir: ${THEME_DIR}"
  if grep -q "BOOTFX_VIDEO_BACKGROUND_BEGIN" "${MAIN_QML}"; then
    echo "Patch status: enabled"
  else
    echo "Patch status: disabled"
  fi
  if [[ -f "${BACKUP_QML}" ]]; then
    echo "Backup: ${BACKUP_QML}"
  else
    echo "Backup: not found"
  fi
  exit 0
fi

if [[ "${MODE}" == "disable" ]]; then
  if [[ ! -f "${BACKUP_QML}" ]]; then
    echo "Backup not found, cannot restore: ${BACKUP_QML}" >&2
    exit 1
  fi
  cp -a "${BACKUP_QML}" "${MAIN_QML}"
  echo "Restored theme from backup: ${MAIN_QML}"
  exit 0
fi

if grep -q "BOOTFX_VIDEO_BACKGROUND_BEGIN" "${MAIN_QML}"; then
  echo "Theme already patched: ${MAIN_QML}"
  exit 0
fi

if [[ ! -f "${BACKUP_QML}" ]]; then
  cp -a "${MAIN_QML}" "${BACKUP_QML}"
  echo "Created backup: ${BACKUP_QML}"
fi

tmp1="$(mktemp)"
tmp2="$(mktemp)"
block_file="$(mktemp)"
trap 'rm -f "${tmp1}" "${tmp2}" "${block_file}"' EXIT

if grep -Eq '^[[:space:]]*import[[:space:]]+QtMultimedia' "${MAIN_QML}"; then
  cp -a "${MAIN_QML}" "${tmp1}"
else
  awk '
    BEGIN { last_import = 0 }
    /^[[:space:]]*import[[:space:]]+/ { last_import = NR }
    { lines[NR] = $0 }
    END {
      if (last_import == 0) {
        print "import QtMultimedia"
      }
      for (i = 1; i <= NR; i++) {
        print lines[i]
        if (i == last_import) {
          print "import QtMultimedia"
        }
      }
    }
  ' "${MAIN_QML}" > "${tmp1}"
fi

cat > "${block_file}" <<'EOF'
    /* BOOTFX_VIDEO_BACKGROUND_BEGIN */
    Video {
        id: bootfxBackgroundVideo
        anchors.fill: parent
        z: -1000
        muted: true
        loops: MediaPlayer.Infinite
        fillMode: VideoOutput.PreserveAspectCrop
        source: (typeof config !== "undefined" && config.BootFXVideoPath) ? String(config.BootFXVideoPath) : ""
        visible: (typeof config !== "undefined" && String(config.BootFXVideoEnabled).toLowerCase() === "true" && source.length > 0)
        autoPlay: visible
        Component.onCompleted: {
            var bootfxStartMs = 0
            if (typeof config !== "undefined" && config.BootFXStartMs) {
                bootfxStartMs = parseInt(config.BootFXStartMs)
            }
            if (!isNaN(bootfxStartMs) && bootfxStartMs > 0) {
                position = bootfxStartMs
            }
        }
    }
    /* BOOTFX_VIDEO_BACKGROUND_END */
EOF

awk -v block_file="${block_file}" '
  { lines[NR] = $0 }
  END {
    target = 0
    for (i = NR; i >= 1; i--) {
      line = lines[i]
      gsub(/[[:space:]]+/, "", line)
      if (line == "}") {
        target = i
        break
      }
    }

    if (target == 0) {
      for (i = 1; i <= NR; i++) {
        print lines[i]
      }
      while ((getline l < block_file) > 0) {
        print l
      }
      close(block_file)
      exit 0
    }

    for (i = 1; i <= NR; i++) {
      if (i == target) {
        while ((getline l < block_file) > 0) {
          print l
        }
        close(block_file)
      }
      print lines[i]
    }
  }
' "${tmp1}" > "${tmp2}"

cp -a "${tmp2}" "${MAIN_QML}"
echo "Patched theme with BootFX video background block: ${MAIN_QML}"
echo "Use boot-video-player with [sddm].video_background_enabled=true to update start position per boot."
