use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/boot-ui/config.toml";
pub const DEFAULT_STATE_PATH: &str = "/run/boot-ui/state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub screen: ScreenConfig,
    pub layering: LayeringConfig,
    pub overlay: OverlayConfig,
    pub animation: AnimationConfig,
    pub handoff: HandoffConfig,
    pub video: VideoConfig,
    pub sddm: SddmConfig,
    pub interaction: InteractionConfig,
    pub debug: DebugConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            screen: ScreenConfig::default(),
            layering: LayeringConfig::default(),
            overlay: OverlayConfig::default(),
            animation: AnimationConfig::default(),
            handoff: HandoffConfig::default(),
            video: VideoConfig::default(),
            sddm: SddmConfig::default(),
            interaction: InteractionConfig::default(),
            debug: DebugConfig::default(),
        }
    }
}

impl Config {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.screen.width == 0 || self.screen.height == 0 {
            bail!("screen width and height must be > 0");
        }
        if self.screen.fps == 0 {
            bail!("screen fps must be > 0");
        }
        if self.overlay.region_h == 0 {
            bail!("overlay.region_h must be > 0");
        }
        if self.animation.manifest.trim().is_empty() {
            bail!("animation.manifest must not be empty");
        }
        if self.handoff.write_state.trim().is_empty() {
            bail!("handoff.write_state must not be empty");
        }
        if self.sddm.theme.trim().is_empty() {
            bail!("sddm.theme must not be empty");
        }
        if self.sddm.theme_root.trim().is_empty() {
            bail!("sddm.theme_root must not be empty");
        }
        if self.sddm.video_background_enabled && self.sddm.video_path.trim().is_empty() {
            bail!("sddm.video_path must not be empty when sddm.video_background_enabled=true");
        }
        if self.interaction.stop_combo.trim().is_empty() {
            bail!("interaction.stop_combo must not be empty");
        }
        if self.debug.log_file.trim().is_empty() {
            bail!("debug.log_file must not be empty");
        }
        if self.debug.history_file.trim().is_empty() {
            bail!("debug.history_file must not be empty");
        }
        if self.debug.export_dir.trim().is_empty() {
            bail!("debug.export_dir must not be empty");
        }
        if self.debug.flush_every == 0 {
            bail!("debug.flush_every must be > 0");
        }
        if self.debug.max_artifact_age_days == 0 {
            bail!("debug.max_artifact_age_days must be > 0");
        }
        if self.debug.max_artifacts == 0 {
            bail!("debug.max_artifacts must be > 0");
        }
        if self.debug.max_log_size_mb == 0 {
            bail!("debug.max_log_size_mb must be > 0");
        }
        if self.debug.max_history_size_mb == 0 {
            bail!("debug.max_history_size_mb must be > 0");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            width: 120,
            height: 40,
            fps: 15,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayeringConfig {
    pub order: Vec<String>,
}

impl Default for LayeringConfig {
    fn default() -> Self {
        Self {
            order: vec!["animation".to_string(), "systemd".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub region_y: u32,
    pub region_h: u32,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            region_y: 24,
            region_h: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationConfig {
    pub manifest: String,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            manifest: "/var/lib/boot-ui/intro/manifest.json".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HandoffConfig {
    pub write_state: String,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            write_state: DEFAULT_STATE_PATH.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub source: String,
    pub player: String,
    pub args: Vec<String>,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            source: "/var/lib/boot-ui/intro/video.mp4".to_string(),
            player: "mpv".to_string(),
            args: vec!["--fullscreen".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SddmConfig {
    pub video_background_enabled: bool,
    pub theme: String,
    pub theme_root: String,
    pub video_path: String,
    pub launch_external_player: bool,
}

impl Default for SddmConfig {
    fn default() -> Self {
        Self {
            video_background_enabled: false,
            theme: "breeze".to_string(),
            theme_root: "/usr/share/sddm/themes".to_string(),
            video_path: "/var/lib/boot-ui/intro/video.mp4".to_string(),
            launch_external_player: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InteractionConfig {
    pub force_text_mode: bool,
    pub stop_combo: String,
    pub any_key_to_login: bool,
    pub start_login_on_stop: bool,
}

impl Default for InteractionConfig {
    fn default() -> Self {
        Self {
            force_text_mode: false,
            stop_combo: "ctrl+q".to_string(),
            any_key_to_login: false,
            start_login_on_stop: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugConfig {
    pub log_file: String,
    pub history_file: String,
    pub export_enabled: bool,
    pub export_dir: String,
    pub flush_every: usize,
    pub log_frame_events: bool,
    pub log_overlay_events: bool,
    pub cleanup_enabled: bool,
    pub max_artifact_age_days: u64,
    pub max_artifacts: usize,
    pub max_log_size_mb: u64,
    pub max_history_size_mb: u64,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            log_file: "/var/log/boot-ui/boot-ui.log".to_string(),
            history_file: "/var/log/boot-ui/boot-ui-history.log".to_string(),
            export_enabled: true,
            export_dir: "/var/lib/boot-ui/debug".to_string(),
            flush_every: 64,
            log_frame_events: true,
            log_overlay_events: true,
            cleanup_enabled: true,
            max_artifact_age_days: 14,
            max_artifacts: 40,
            max_log_size_mb: 32,
            max_history_size_mb: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub frames: Vec<FrameMeta>,
}

impl Manifest {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read manifest: {}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse manifest: {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directory for manifest: {}",
                    parent.display()
                )
            })?;
        }
        let raw = serde_json::to_string_pretty(self).context("failed to serialize manifest")?;
        fs::write(path, raw)
            .with_context(|| format!("failed to write manifest: {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.fps == 0 {
            bail!("manifest fps must be > 0");
        }
        if self.width == 0 || self.height == 0 {
            bail!("manifest width/height must be > 0");
        }
        if self.frame_count as usize != self.frames.len() {
            bail!(
                "manifest frame_count ({}) does not match frame list length ({})",
                self.frame_count,
                self.frames.len()
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameMeta {
    pub index: u64,
    pub pts_ms: u64,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub frame_index: u64,
    pub pts_ms: u64,
}

impl State {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read state: {}", path.display()))?;
        let state: State = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse state: {}", path.display()))?;
        Ok(state)
    }

    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directory for state file: {}",
                    parent.display()
                )
            })?;
        }
        let raw = serde_json::to_string_pretty(self).context("failed to serialize state")?;
        fs::write(path, raw)
            .with_context(|| format!("failed to write state: {}", path.display()))?;
        Ok(())
    }
}
