//! boot-video-player -- Post-boot graphical continuation.
//!
//! Reads handoff state from boot-ui and resumes the original video from pts_ms.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use bootfx_core::{Config, State, DEFAULT_CONFIG_PATH, DEFAULT_STATE_PATH};

#[derive(Debug)]
struct Args {
    config_path: PathBuf,
    state_path: PathBuf,
    video_path_override: Option<PathBuf>,
    dry_run: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("boot-video-player error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;

    let config = Config::load_from_path(&args.config_path).with_context(|| {
        format!(
            "failed to load config file for video player: {}",
            args.config_path.display()
        )
    })?;

    let state = if args.state_path.exists() {
        State::load_from_path(&args.state_path).with_context(|| {
            format!("failed to read state file: {}", args.state_path.display())
        })?
    } else {
        State {
            frame_index: 0,
            pts_ms: 0,
        }
    };

    let video_path = select_video_path(&args, &config)?;
    let player = choose_player(&config);
    let mut command = build_player_command(&player, &config.video.args, &video_path, state.pts_ms);

    eprintln!(
        "boot-video-player: player=`{}`, start_ms={}, video={}",
        player,
        state.pts_ms,
        video_path.display()
    );

    if args.dry_run {
        eprintln!("dry-run enabled, not executing player");
        return Ok(());
    }

    let status = command.status().context("failed to launch video player")?;
    if !status.success() {
        bail!("video player exited with non-zero status: {status}");
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
    let mut state_path = PathBuf::from(DEFAULT_STATE_PATH);
    let mut video_path_override = None;
    let mut dry_run = false;

    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--config" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --config"))?;
                config_path = PathBuf::from(val);
            }
            "--state" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --state"))?;
                state_path = PathBuf::from(val);
            }
            "--video" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --video"))?;
                video_path_override = Some(PathBuf::from(val));
            }
            "--dry-run" => dry_run = true,
            other => bail!("unknown argument `{other}`. Use --help"),
        }
    }

    Ok(Args {
        config_path,
        state_path,
        video_path_override,
        dry_run,
    })
}

fn print_help() {
    println!(
        "\
boot-video-player

Usage:
  boot-video-player [options]

Options:
  --config <path>      Config TOML path (default: /etc/boot-ui/config.toml)
  --state <path>       Handoff state JSON path (default: /run/boot-ui/state.json)
  --video <path>       Override source video path
  --dry-run            Print resolved launch info without running player
"
    );
}

fn select_video_path(args: &Args, config: &Config) -> Result<PathBuf> {
    if let Some(path) = &args.video_path_override {
        return Ok(path.clone());
    }

    if !config.video.source.trim().is_empty() {
        return Ok(PathBuf::from(config.video.source.clone()));
    }

    if let Ok(raw) = env::var("BOOTFX_VIDEO") {
        if !raw.trim().is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }

    bail!("no video path configured (use --video, config.video.source, or BOOTFX_VIDEO)")
}

fn choose_player(config: &Config) -> String {
    if !config.video.player.trim().is_empty() {
        return config.video.player.clone();
    }

    if let Ok(raw) = env::var("BOOTFX_PLAYER") {
        if !raw.trim().is_empty() {
            return raw;
        }
    }

    "mpv".to_string()
}

fn build_player_command(
    player: &str,
    configured_args: &[String],
    video_path: &Path,
    pts_ms: u64,
) -> Command {
    let start_secs = pts_ms as f64 / 1000.0;
    let player_lower = player.to_ascii_lowercase();

    let mut command = Command::new(player);

    if player_lower.contains("mpv") {
        command.arg("--no-terminal");
        command.arg(format!("--start={start_secs:.3}"));
    } else if player_lower.contains("ffplay") {
        command.arg("-ss");
        command.arg(format!("{start_secs:.3}"));
        command.arg("-autoexit");
    } else if player_lower.contains("vlc") {
        command.arg("--start-time");
        command.arg((pts_ms / 1000).to_string());
    }

    command.args(configured_args);
    command.arg(video_path);
    command
}