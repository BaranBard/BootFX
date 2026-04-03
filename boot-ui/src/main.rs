//! boot-ui -- Runtime ASCII boot animation renderer.
//!
//! Captures tty output, plays precomputed ASCII frames, overlays journal messages,
//! and writes handoff state for the graphical continuation stage.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use bootfx_core::{Config, Manifest, State, DEFAULT_CONFIG_PATH};

#[derive(Debug)]
struct Args {
    config_path: PathBuf,
    max_frames: Option<u64>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("boot-ui error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let config = Config::load_from_path(&args.config_path)?;

    let manifest_path = PathBuf::from(&config.animation.manifest);
    let manifest = Manifest::load_from_path(&manifest_path)?;

    if config.screen.width != manifest.width || config.screen.height != manifest.height {
        bail!(
            "screen dimensions in config ({}x{}) do not match manifest ({}x{})",
            config.screen.width,
            config.screen.height,
            manifest.width,
            manifest.height
        );
    }

    let width = config.screen.width as usize;
    let height = config.screen.height as usize;

    let overlay_capacity = config.overlay.region_h.max(1) as usize;
    let overlay_lines = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(overlay_capacity)));
    let stop_flag = Arc::new(AtomicBool::new(false));
    spawn_journal_reader(overlay_lines.clone(), overlay_capacity, stop_flag.clone());

    let graphical_reached = Arc::new(AtomicBool::new(false));
    spawn_graphical_target_watcher(graphical_reached.clone(), stop_flag.clone());

    let _term_guard = TerminalGuard::enter()?;

    let frame_interval = Duration::from_millis((1000 / config.screen.fps.max(1)) as u64);
    let manifest_base_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut last_state = State {
        frame_index: 0,
        pts_ms: 0,
    };

    for (processed, frame) in manifest.frames.iter().enumerate() {
        if graphical_reached.load(Ordering::Relaxed) {
            break;
        }
        if let Some(max_frames) = args.max_frames {
            if processed as u64 >= max_frames {
                break;
            }
        }

        let frame_path = manifest_base_dir.join(&frame.file);
        let frame_bytes = fs::read(&frame_path)
            .with_context(|| format!("failed to read frame {}", frame_path.display()))?;
        if frame_bytes.len() != width * height {
            bail!(
                "frame {} has invalid size {} (expected {})",
                frame_path.display(),
                frame_bytes.len(),
                width * height
            );
        }

        let snapshot = snapshot_overlay_lines(&overlay_lines);
        let composed = compose_layers(
            &frame_bytes,
            &snapshot,
            width,
            height,
            config.overlay.region_y as usize,
            config.overlay.region_h as usize,
            &config.layering.order,
        );
        render_frame(&composed, width, height)?;

        last_state = State {
            frame_index: frame.index,
            pts_ms: frame.pts_ms,
        };

        let frame_start = Instant::now();
        while frame_start.elapsed() < frame_interval {
            thread::sleep(Duration::from_millis(1));
        }
    }

    stop_flag.store(true, Ordering::Relaxed);
    write_handoff_state(&config, &last_state)?;

    eprintln!(
        "boot-ui wrote handoff state: frame_index={}, pts_ms={}",
        last_state.frame_index, last_state.pts_ms
    );

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
    let mut max_frames = None;

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
            "--max-frames" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --max-frames"))?;
                max_frames = Some(
                    val.parse::<u64>()
                        .with_context(|| format!("invalid --max-frames value `{val}`"))?,
                );
            }
            other => bail!("unknown argument `{other}`. Use --help"),
        }
    }

    Ok(Args {
        config_path,
        max_frames,
    })
}

fn print_help() {
    println!(
        "\
boot-ui

Usage:
  boot-ui [options]

Options:
  --config <path>      Config TOML path (default: /etc/boot-ui/config.toml)
  --max-frames <n>     Process only the first N frames (debug)
"
    );
}

fn spawn_journal_reader(
    overlay: Arc<Mutex<VecDeque<String>>>,
    capacity: usize,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut child = match Command::new("journalctl")
            .args(["-b", "-f", "-n", "0", "-o", "cat"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                push_overlay_line(
                    &overlay,
                    capacity,
                    format!("[INFO ] journald unavailable: {err}"),
                );
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(stream) => stream,
            None => {
                push_overlay_line(
                    &overlay,
                    capacity,
                    "[INFO ] journald stream unavailable".to_string(),
                );
                let _ = child.kill();
                return;
            }
        };

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match line {
                Ok(raw) => {
                    let formatted = classify_journal_line(raw);
                    push_overlay_line(&overlay, capacity, formatted);
                }
                Err(_) => break,
            }
        }

        let _ = child.kill();
    });
}

fn classify_journal_line(line: String) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return "[INFO ]".to_string();
    }

    let lower = trimmed.to_ascii_lowercase();
    let tag = if lower.contains("failed") {
        "[FAILED]"
    } else if lower.starts_with("started") || lower.contains(" started ") {
        "[  OK  ]"
    } else if lower.starts_with("starting") || lower.contains(" starting ") {
        "[    ]"
    } else {
        "[INFO ]"
    };

    format!("{tag} {trimmed}")
}

fn push_overlay_line(overlay: &Arc<Mutex<VecDeque<String>>>, capacity: usize, line: String) {
    if let Ok(mut guard) = overlay.lock() {
        if guard.len() == capacity {
            guard.pop_front();
        }
        guard.push_back(sanitize_ascii_line(&line));
    }
}

fn sanitize_ascii_line(line: &str) -> String {
    line.chars()
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '?'
            }
        })
        .collect()
}

fn snapshot_overlay_lines(overlay: &Arc<Mutex<VecDeque<String>>>) -> Vec<String> {
    overlay
        .lock()
        .map(|guard| guard.iter().cloned().collect())
        .unwrap_or_default()
}

fn spawn_graphical_target_watcher(reached: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) && !reached.load(Ordering::Relaxed) {
            if is_graphical_target_active() {
                reached.store(true, Ordering::Relaxed);
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn is_graphical_target_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "graphical.target"])
        .output()
        .ok()
        .map(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "active"
        })
        .unwrap_or(false)
}

fn compose_layers(
    animation_frame: &[u8],
    overlay_lines: &[String],
    width: usize,
    height: usize,
    overlay_region_y: usize,
    overlay_region_h: usize,
    order: &[String],
) -> Vec<u8> {
    let mut canvas = vec![0u8; width * height];
    let overlay_layer = build_overlay_layer(
        overlay_lines,
        width,
        height,
        overlay_region_y,
        overlay_region_h,
    );

    for layer in order {
        match layer.as_str() {
            "animation" => blit(animation_frame, &mut canvas),
            "systemd" => blit(&overlay_layer, &mut canvas),
            _ => {}
        }
    }

    if !order.iter().any(|layer| layer == "animation") {
        blit(animation_frame, &mut canvas);
    }

    canvas
}

fn build_overlay_layer(
    lines: &[String],
    width: usize,
    height: usize,
    region_y: usize,
    region_h: usize,
) -> Vec<u8> {
    let mut layer = vec![0u8; width * height];
    let displayed_lines = lines.iter().rev().take(region_h).collect::<Vec<_>>();

    for (idx, line) in displayed_lines.into_iter().rev().enumerate() {
        let y = region_y + idx;
        if y >= height {
            break;
        }

        for (x, byte) in line.bytes().take(width).enumerate() {
            if byte != b' ' {
                layer[y * width + x] = byte;
            }
        }
    }

    layer
}

fn blit(src: &[u8], dst: &mut [u8]) {
    for (idx, byte) in src.iter().enumerate() {
        if *byte != 0 {
            dst[idx] = *byte;
        }
    }
}

fn render_frame(canvas: &[u8], width: usize, height: usize) -> Result<()> {
    let mut buffer = String::with_capacity(width * height + height + 8);
    buffer.push_str("\x1b[H");

    for y in 0..height {
        let start = y * width;
        let end = start + width;
        for byte in &canvas[start..end] {
            buffer.push(if *byte == 0 { ' ' } else { *byte as char });
        }
        buffer.push('\n');
    }

    let mut stdout = io::stdout().lock();
    stdout
        .write_all(buffer.as_bytes())
        .context("failed to write frame to terminal")?;
    stdout.flush().context("failed to flush terminal output")?;
    Ok(())
}

fn write_handoff_state(config: &Config, state: &State) -> Result<()> {
    let state_path = PathBuf::from(&config.handoff.write_state);
    state
        .write_to_path(&state_path)
        .with_context(|| format!("failed to write handoff state to {}", state_path.display()))
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
            .context("failed to initialize terminal")?;
        stdout.flush().context("failed to flush terminal init")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = io::stdout().write_all(b"\x1b[?25h\x1b[0m\n");
        let _ = io::stdout().flush();
    }
}