//! boot-ui -- Runtime ASCII boot animation renderer.
//!
//! Captures tty output, plays precomputed ASCII frames, overlays journal messages,
//! and writes handoff state for the graphical continuation stage.

use std::collections::VecDeque;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use bootfx_core::{Config, InteractionConfig, Manifest, State, DEFAULT_CONFIG_PATH};

#[derive(Debug)]
struct Args {
    config_path: PathBuf,
    max_frames: Option<u64>,
    force_console: bool,
    donut_mode: bool,
    hash_test_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Manifest,
    Donut,
    HashTest,
}

#[derive(Debug, Clone)]
struct InputControl {
    force_text_mode: bool,
    any_key_to_login: bool,
    start_login_on_stop: bool,
    stop_combo_label: String,
    stop_combo_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
enum InputEvent {
    StopCombo,
    AnyKey,
}

#[derive(Clone)]
struct DebugLogger {
    log_file: Arc<Mutex<fs::File>>,
    history: Arc<Mutex<Vec<String>>>,
    history_path: PathBuf,
    flush_every: usize,
}

impl DebugLogger {
    fn new(log_path: &Path, history_path: &Path, flush_every: usize) -> Result<Self> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create debug log directory: {}", parent.display())
            })?;
        }
        if let Some(parent) = history_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create debug history directory: {}",
                    parent.display()
                )
            })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("failed to open debug log file: {}", log_path.display()))?;

        Ok(Self {
            log_file: Arc::new(Mutex::new(file)),
            history: Arc::new(Mutex::new(Vec::new())),
            history_path: history_path.to_path_buf(),
            flush_every,
        })
    }

    fn info(&self, message: impl AsRef<str>) {
        self.log("INFO", message.as_ref());
    }

    fn warn(&self, message: impl AsRef<str>) {
        self.log("WARN", message.as_ref());
    }

    fn error(&self, message: impl AsRef<str>) {
        self.log("ERROR", message.as_ref());
    }

    fn log(&self, level: &str, message: &str) {
        let ts = utc_millis();
        let line = format!("{ts} [{level}] {message}");

        if let Ok(mut file) = self.log_file.lock() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }

        let mut should_flush = false;
        if let Ok(mut history) = self.history.lock() {
            history.push(line);
            if history.len() % self.flush_every == 0 {
                should_flush = true;
            }
        }

        if should_flush {
            let _ = self.flush_history_snapshot();
        }
    }

    fn flush_history_snapshot(&self) -> Result<()> {
        let snapshot = {
            let history = self
                .history
                .lock()
                .map_err(|_| anyhow!("debug history mutex is poisoned"))?;
            history.join("\n")
        };
        fs::write(&self.history_path, snapshot).with_context(|| {
            format!(
                "failed to write debug history file: {}",
                self.history_path.display()
            )
        })?;
        Ok(())
    }
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
    if let Err(err) = prepare_debug_runtime(&config) {
        eprintln!("boot-ui debug runtime prepare failed: {err:#}");
    }
    let logger = match DebugLogger::new(
        Path::new(&config.debug.log_file),
        Path::new(&config.debug.history_file),
        config.debug.flush_every,
    ) {
        Ok(logger) => Some(logger),
        Err(err) => {
            eprintln!("boot-ui debug logger init failed: {err:#}");
            None
        }
    };

    if let Some(log) = logger.clone() {
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            log.error(format!("panic captured: {panic_info}"));
            let _ = log.flush_history_snapshot();
            old_hook(panic_info);
        }));
    }

    if let Some(log) = logger.as_ref() {
        log.info(format!(
            "boot-ui startup: config_path={}, log_file={}, history_file={}",
            args.config_path.display(),
            config.debug.log_file,
            config.debug.history_file
        ));
    }
    let input_control = build_input_control(&config.interaction)?;
    let force_text_mode = args.force_console || input_control.force_text_mode;
    if let Some(log) = logger.as_ref() {
        log.info(format!(
            "interaction config: force_text_mode={}, any_key_to_login={}, stop_combo={}, start_login_on_stop={}",
            force_text_mode,
            input_control.any_key_to_login,
            input_control.stop_combo_label,
            input_control.start_login_on_stop
        ));
    }

    let manifest_path = PathBuf::from(&config.animation.manifest);
    let mut render_mode = if args.hash_test_mode {
        RenderMode::HashTest
    } else if args.donut_mode {
        RenderMode::Donut
    } else {
        RenderMode::Manifest
    };
    let mut manifest: Option<Manifest> = None;
    let manifest_base_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if render_mode == RenderMode::Manifest {
        match Manifest::load_from_path(&manifest_path) {
            Ok(loaded) => {
                if config.screen.width != loaded.width || config.screen.height != loaded.height {
                    bail!(
                        "screen dimensions in config ({}x{}) do not match manifest ({}x{})",
                        config.screen.width,
                        config.screen.height,
                        loaded.width,
                        loaded.height
                    );
                }
                if let Some(log) = logger.as_ref() {
                    log.info(format!(
                        "manifest loaded: path={}, frames={}, size={}x{}, fps={}",
                        manifest_path.display(),
                        loaded.frame_count,
                        loaded.width,
                        loaded.height,
                        loaded.fps
                    ));
                }
                manifest = Some(loaded);
            }
            Err(err) => {
                if manifest_path.exists() {
                    return Err(err).with_context(|| {
                        format!("failed to load manifest at {}", manifest_path.display())
                    });
                }
                render_mode = RenderMode::Donut;
                if let Some(log) = logger.as_ref() {
                    log.warn(format!(
                        "manifest missing at {}, switching to donut fallback",
                        manifest_path.display()
                    ));
                }
            }
        }
    }

    if let Some(log) = logger.as_ref() {
        log.info(format!("render mode: {:?}", render_mode));
    }

    let width = config.screen.width as usize;
    let height = config.screen.height as usize;

    let overlay_capacity = config.overlay.region_h.max(1) as usize;
    let overlay_lines = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(overlay_capacity)));
    let stop_flag = Arc::new(AtomicBool::new(false));
    spawn_journal_reader(
        overlay_lines.clone(),
        overlay_capacity,
        stop_flag.clone(),
        logger.clone(),
        config.debug.log_overlay_events,
    );

    let graphical_reached = Arc::new(AtomicBool::new(false));
    if force_text_mode {
        if let Some(log) = logger.as_ref() {
            log.info("force text mode enabled: graphical target watcher disabled");
        }
    } else {
        spawn_graphical_target_watcher(graphical_reached.clone(), stop_flag.clone(), logger.clone());
    }

    let _term_guard = TerminalGuard::enter()?;
    if let Some(log) = logger.as_ref() {
        log.info("terminal initialized");
    }
    let input_rx = spawn_keyboard_input_reader(
        stop_flag.clone(),
        input_control.clone(),
        logger.clone(),
    );

    let fps = config.screen.fps.max(1) as u64;
    let frame_interval = Duration::from_millis((1000 / fps).max(1));

    let mut last_state = State {
        frame_index: 0,
        pts_ms: 0,
    };
    let mut rendered_frames = 0u64;
    let mut source_index = 0u64;
    let mut exit_reason = "completed".to_string();
    let mut handoff_blocked_by_input = false;

    'frame_loop: loop {
        if let Some(rx) = input_rx.as_ref() {
            while let Ok(event) = rx.try_recv() {
                match event {
                    InputEvent::StopCombo => {
                        if let Some(log) = logger.as_ref() {
                            log.info(format!(
                                "keyboard hotkey received: {}, stopping playback",
                                input_control.stop_combo_label
                            ));
                        }
                        exit_reason = format!(
                            "stopped: playback disabled via hotkey {}",
                            input_control.stop_combo_label
                        );
                        handoff_blocked_by_input = true;
                        if input_control.start_login_on_stop {
                            let _ = request_login_environment(logger.as_ref());
                        }
                        break 'frame_loop;
                    }
                    InputEvent::AnyKey => {
                        if let Some(log) = logger.as_ref() {
                            log.info("keyboard input received: requesting login environment");
                        }
                        exit_reason = "stopped: keyboard input requested login screen".to_string();
                        handoff_blocked_by_input = true;
                        let _ = request_login_environment(logger.as_ref());
                        break 'frame_loop;
                    }
                }
            }
        }
        if !force_text_mode && graphical_reached.load(Ordering::Relaxed) {
            if let Some(log) = logger.as_ref() {
                log.info("graphical.target reached, stopping frame loop");
            }
            exit_reason = "stopped: graphical.target reached".to_string();
            break;
        }
        if let Some(max_frames) = args.max_frames {
            if rendered_frames >= max_frames {
                if let Some(log) = logger.as_ref() {
                    log.info(format!("debug max-frames reached: {}", max_frames));
                }
                exit_reason = format!("stopped: max-frames={max_frames}");
                break;
            }
        }
        let (frame_bytes, frame_index, pts_ms) = match render_mode {
            RenderMode::Manifest => {
                let loaded = manifest
                    .as_ref()
                    .ok_or_else(|| anyhow!("manifest mode selected but manifest is missing"))?;
                if source_index as usize >= loaded.frames.len() {
                    exit_reason = "completed: frame-list-exhausted".to_string();
                    break;
                }
                let frame = &loaded.frames[source_index as usize];
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
                source_index += 1;
                (frame_bytes, frame.index, frame.pts_ms)
            }
            RenderMode::Donut => {
                let idx = source_index;
                source_index += 1;
                let pts_ms = idx.saturating_mul(1000).saturating_div(fps);
                (build_donut_frame(width, height, idx), idx, pts_ms)
            }
            RenderMode::HashTest => {
                let idx = source_index;
                source_index += 1;
                let pts_ms = idx.saturating_mul(1000).saturating_div(fps);
                (build_hash_frame(width, height), idx, pts_ms)
            }
        };

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
        if config.debug.log_frame_events {
            if let Some(log) = logger.as_ref() {
                log.info(format!(
                    "frame rendered: index={}, pts_ms={}, overlay_lines={}",
                    frame_index,
                    pts_ms,
                    snapshot.len()
                ));
            }
        }
        rendered_frames += 1;

        last_state = State {
            frame_index,
            pts_ms,
        };

        let frame_start = Instant::now();
        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }

    stop_flag.store(true, Ordering::Relaxed);
    let should_write_handoff =
        !force_text_mode && render_mode == RenderMode::Manifest && !handoff_blocked_by_input;
    if should_write_handoff {
        if let Some(log) = logger.as_ref() {
            log.info("writing handoff state");
        }
        write_handoff_state(&config, &last_state)?;
        eprintln!(
            "boot-ui wrote handoff state: frame_index={}, pts_ms={}",
            last_state.frame_index, last_state.pts_ms
        );
        if let Some(log) = logger.as_ref() {
            log.info(format!(
                "handoff state written: frame_index={}, pts_ms={}, path={}",
                last_state.frame_index,
                last_state.pts_ms,
                config.handoff.write_state
            ));
        }
    } else {
        eprintln!("boot-ui skipped handoff state write for current render mode");
        if let Some(log) = logger.as_ref() {
            log.info("handoff state write skipped for current render mode");
        }
    }
    if let Some(log) = logger.as_ref() {
        if let Err(err) = log.flush_history_snapshot() {
            eprintln!("boot-ui failed to flush debug history: {err:#}");
        }
    }
    if let Some(log) = logger.as_ref() {
        log.info(format!(
            "run summary: rendered_frames={}, last_frame_index={}, last_pts_ms={}, exit_reason={}",
            rendered_frames, last_state.frame_index, last_state.pts_ms, exit_reason
        ));
    }
    if let Err(err) = export_debug_bundle(
        &config,
        &args.config_path,
        &manifest_path,
        &last_state,
        rendered_frames,
        &exit_reason,
        logger.as_ref(),
    ) {
        if let Some(log) = logger.as_ref() {
            log.warn(format!("failed to export debug bundle: {err:#}"));
        } else {
            eprintln!("failed to export debug bundle: {err:#}");
        }
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
    let mut max_frames = None;
    let mut force_console = false;
    let mut donut_mode = false;
    let mut hash_test_mode = false;

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
            "--force-console" | "--force-text" => force_console = true,
            "--donut" => donut_mode = true,
            "--hash-test" | "--hash-fill" => hash_test_mode = true,
            other => bail!("unknown argument `{other}`. Use --help"),
        }
    }

    if donut_mode && hash_test_mode {
        bail!("`--donut` and `--hash-test` cannot be used together");
    }

    Ok(Args {
        config_path,
        max_frames,
        force_console,
        donut_mode,
        hash_test_mode,
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
  --force-console      Ignore graphical.target and keep rendering until end/max-frames
  --force-text         Alias for --force-console
  --donut              Render built-in spinning 3D donut instead of manifest frames
  --hash-test          Render fullscreen `#` symbols for visibility testing
"
    );
}

fn build_input_control(cfg: &InteractionConfig) -> Result<InputControl> {
    let stop_combo_label = cfg.stop_combo.trim().to_string();
    let stop_combo_bytes = parse_stop_combo(&cfg.stop_combo)?;
    Ok(InputControl {
        force_text_mode: cfg.force_text_mode,
        any_key_to_login: cfg.any_key_to_login,
        start_login_on_stop: cfg.start_login_on_stop,
        stop_combo_label,
        stop_combo_bytes,
    })
}

fn parse_stop_combo(combo: &str) -> Result<Option<Vec<u8>>> {
    let normalized = combo.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized == "none"
        || normalized == "off"
        || normalized == "disabled"
    {
        return Ok(None);
    }

    if normalized == "esc" || normalized == "escape" {
        return Ok(Some(vec![0x1b]));
    }
    if normalized == "enter" || normalized == "return" {
        return Ok(Some(vec![b'\n']));
    }

    if let Some(raw) = normalized.strip_prefix("ctrl+") {
        if raw.len() == 1 {
            let ch = raw.as_bytes()[0];
            if ch.is_ascii_alphabetic() {
                return Ok(Some(vec![ch.to_ascii_lowercase() & 0x1f]));
            }
        }
        bail!(
            "unsupported interaction.stop_combo `{}` (expected ctrl+<letter>, e.g. ctrl+q)",
            combo
        );
    }

    if let Some(raw) = normalized.strip_prefix("alt+") {
        if raw.len() == 1 {
            let ch = raw.as_bytes()[0];
            if ch.is_ascii_graphic() {
                return Ok(Some(vec![0x1b, ch]));
            }
        }
        bail!(
            "unsupported interaction.stop_combo `{}` (expected alt+<char>, e.g. alt+q)",
            combo
        );
    }

    if let Some(hex) = normalized.strip_prefix("0x") {
        let value = u8::from_str_radix(hex, 16)
            .with_context(|| format!("invalid hex byte in interaction.stop_combo `{combo}`"))?;
        return Ok(Some(vec![value]));
    }

    if normalized.len() == 1 {
        return Ok(Some(vec![normalized.as_bytes()[0]]));
    }

    bail!(
        "unsupported interaction.stop_combo `{}`; supported formats: ctrl+q, alt+q, q, esc, enter, none",
        combo
    );
}

fn spawn_keyboard_input_reader(
    stop: Arc<AtomicBool>,
    input_control: InputControl,
    logger: Option<DebugLogger>,
) -> Option<Receiver<InputEvent>> {
    if !input_control.any_key_to_login && input_control.stop_combo_bytes.is_none() {
        if let Some(log) = logger.as_ref() {
            log.info("keyboard input controls disabled");
        }
        return None;
    }

    let (tx, rx) = mpsc::channel::<InputEvent>();
    thread::spawn(move || {
        if let Some(log) = logger.as_ref() {
            log.info("keyboard input reader started");
        }
        let mut stdin = io::stdin().lock();
        let mut byte = [0u8; 1];
        let mut recent = VecDeque::<u8>::new();
        let max_combo_len = input_control
            .stop_combo_bytes
            .as_ref()
            .map(|seq| seq.len())
            .unwrap_or(1)
            .max(1);

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match stdin.read(&mut byte) {
                Ok(0) => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(_) => {
                    let b = byte[0];
                    if input_control.any_key_to_login {
                        let _ = tx.send(InputEvent::AnyKey);
                        break;
                    }

                    if let Some(stop_seq) = input_control.stop_combo_bytes.as_ref() {
                        recent.push_back(b);
                        while recent.len() > max_combo_len {
                            recent.pop_front();
                        }
                        if sequence_matches_tail(&recent, stop_seq) {
                            let _ = tx.send(InputEvent::StopCombo);
                            break;
                        }
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    if let Some(log) = logger.as_ref() {
                        log.warn(format!("keyboard input reader failed: {err}"));
                    }
                    break;
                }
            }
        }
        if let Some(log) = logger.as_ref() {
            log.info("keyboard input reader stopped");
        }
    });

    Some(rx)
}

fn sequence_matches_tail(recent: &VecDeque<u8>, seq: &[u8]) -> bool {
    if recent.len() < seq.len() {
        return false;
    }
    recent
        .iter()
        .skip(recent.len() - seq.len())
        .zip(seq.iter())
        .all(|(lhs, rhs)| lhs == rhs)
}

fn request_login_environment(logger: Option<&DebugLogger>) -> bool {
    let attempts: [(&str, [&str; 2]); 2] = [
        ("display-manager", ["start", "display-manager.service"]),
        ("graphical-target", ["start", "graphical.target"]),
    ];

    for (label, args) in attempts {
        let status = Command::new("systemctl").args(args).status();
        match status {
            Ok(status) if status.success() => {
                if let Some(log) = logger {
                    log.info(format!(
                        "login environment request succeeded via {} ({})",
                        label, status
                    ));
                }
                return true;
            }
            Ok(status) => {
                if let Some(log) = logger {
                    log.warn(format!(
                        "login environment request via {} failed: {}",
                        label, status
                    ));
                }
            }
            Err(err) => {
                if let Some(log) = logger {
                    log.warn(format!(
                        "login environment request via {} errored: {}",
                        label, err
                    ));
                }
            }
        }
    }

    false
}

fn spawn_journal_reader(
    overlay: Arc<Mutex<VecDeque<String>>>,
    capacity: usize,
    stop: Arc<AtomicBool>,
    logger: Option<DebugLogger>,
    log_overlay_events: bool,
) {
    thread::spawn(move || {
        if let Some(log) = logger.as_ref() {
            log.info("journal reader thread started");
        }

        let mut child = match Command::new("journalctl")
            .args(["-b", "-f", "-n", "0", "-o", "cat"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                if let Some(log) = logger.as_ref() {
                    log.warn(format!("journalctl spawn failed: {err}"));
                }
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
                if let Some(log) = logger.as_ref() {
                    log.warn("journalctl stdout pipe unavailable");
                }
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
                    if log_overlay_events {
                        if let Some(log) = logger.as_ref() {
                            log.info(format!("overlay event: {formatted}"));
                        }
                    }
                    push_overlay_line(&overlay, capacity, formatted);
                }
                Err(_) => break,
            }
        }

        let _ = child.kill();
        if let Some(log) = logger.as_ref() {
            log.info("journal reader thread stopped");
        }
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

fn spawn_graphical_target_watcher(
    reached: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    logger: Option<DebugLogger>,
) {
    thread::spawn(move || {
        if let Some(log) = logger.as_ref() {
            log.info("graphical target watcher started");
        }
        while !stop.load(Ordering::Relaxed) && !reached.load(Ordering::Relaxed) {
            if is_graphical_target_active(logger.as_ref()) {
                reached.store(true, Ordering::Relaxed);
                if let Some(log) = logger.as_ref() {
                    log.info("graphical.target is active");
                }
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
        if let Some(log) = logger.as_ref() {
            log.info("graphical target watcher stopped");
        }
    });
}

fn is_graphical_target_active(logger: Option<&DebugLogger>) -> bool {
    let output = match Command::new("systemctl")
        .args(["is-active", "graphical.target"])
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            if let Some(log) = logger {
                log.warn(format!("systemctl is-active call failed: {err}"));
            }
            return false;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Some(log) = logger {
        log.info(format!(
            "systemctl is-active graphical.target: status={}, output={}",
            output.status, stdout
        ));
    }

    output.status.success() && stdout == "active"
}

fn build_hash_frame(width: usize, height: usize) -> Vec<u8> {
    vec![b'#'; width * height]
}

fn build_donut_frame(width: usize, height: usize, tick: u64) -> Vec<u8> {
    let mut output = vec![b' '; width * height];
    let mut zbuf = vec![0.0f32; width * height];
    const SHADES: &[u8] = b".,-~:;=!*#$@";

    let a = tick as f32 * 0.07;
    let b = tick as f32 * 0.03;
    let sin_a = a.sin();
    let cos_a = a.cos();
    let sin_b = b.sin();
    let cos_b = b.cos();

    let width_f = width as f32;
    let height_f = height as f32;
    let scale = (width_f.min(height_f) * 0.28).max(6.0);
    let y_scale = 0.55f32;

    let mut theta = 0.0f32;
    while theta < std::f32::consts::TAU {
        let sin_t = theta.sin();
        let cos_t = theta.cos();

        let mut phi = 0.0f32;
        while phi < std::f32::consts::TAU {
            let sin_p = phi.sin();
            let cos_p = phi.cos();

            let circle_x = 2.0 + cos_t;
            let circle_y = sin_t;

            let x =
                circle_x * (cos_b * cos_p + sin_a * sin_b * sin_p) - circle_y * cos_a * sin_b;
            let y =
                circle_x * (sin_b * cos_p - sin_a * cos_b * sin_p) + circle_y * cos_a * cos_b;
            let z = cos_a * circle_x * sin_p + circle_y * sin_a + 5.0;
            let ooz = 1.0 / z;

            let xp = (width_f * 0.5 + scale * ooz * x) as isize;
            let yp = (height_f * 0.5 + scale * y_scale * ooz * y) as isize;

            if xp >= 0 && xp < width as isize && yp >= 0 && yp < height as isize {
                let idx = yp as usize * width + xp as usize;
                if ooz > zbuf[idx] {
                    zbuf[idx] = ooz;
                    let luminance = cos_p * cos_t * sin_b
                        - cos_a * cos_t * sin_p
                        - sin_a * sin_t
                        + cos_b * (cos_a * sin_t - cos_t * sin_a * sin_p);
                    let raw = ((luminance + 1.0) * 0.5 * (SHADES.len() as f32 - 1.0)) as isize;
                    let shade = raw.clamp(0, SHADES.len() as isize - 1) as usize;
                    output[idx] = SHADES[shade];
                }
            }

            phi += 0.045;
        }

        theta += 0.020;
    }

    output
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

fn prepare_debug_runtime(config: &Config) -> Result<()> {
    let log_path = PathBuf::from(&config.debug.log_file);
    let history_path = PathBuf::from(&config.debug.history_file);
    let export_dir = PathBuf::from(&config.debug.export_dir);

    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create debug log dir: {}", parent.display()))?;
    }
    if let Some(parent) = history_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create debug history dir: {}", parent.display())
        })?;
    }
    fs::create_dir_all(&export_dir)
        .with_context(|| format!("failed to create debug export dir: {}", export_dir.display()))?;

    let _ = rotate_if_oversized(&log_path, config.debug.max_log_size_mb)?;
    let _ = rotate_if_oversized(&history_path, config.debug.max_history_size_mb)?;

    if config.debug.cleanup_enabled {
        cleanup_rotated_files(
            &log_path,
            config.debug.max_artifact_age_days,
            config.debug.max_artifacts,
        )?;
        cleanup_rotated_files(
            &history_path,
            config.debug.max_artifact_age_days,
            config.debug.max_artifacts,
        )?;
        cleanup_artifact_dir(
            &export_dir,
            config.debug.max_artifact_age_days,
            config.debug.max_artifacts,
        )?;
    }

    Ok(())
}

fn rotate_if_oversized(path: &Path, max_size_mb: u64) -> Result<Option<PathBuf>> {
    let metadata = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    let max_bytes = max_size_mb.saturating_mul(1024 * 1024);
    if metadata.len() <= max_bytes {
        return Ok(None);
    }

    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "boot-ui.log".to_string());
    let rotated_name = format!("{file_name}.old-{}", utc_millis());
    let rotated_path = path
        .parent()
        .map(|dir| dir.join(&rotated_name))
        .unwrap_or_else(|| PathBuf::from(rotated_name));
    fs::rename(path, &rotated_path).with_context(|| {
        format!(
            "failed to rotate oversized debug file {} to {}",
            path.display(),
            rotated_path.display()
        )
    })?;
    Ok(Some(rotated_path))
}

fn cleanup_rotated_files(base_path: &Path, max_age_days: u64, max_keep: usize) -> Result<()> {
    let parent = match base_path.parent() {
        Some(parent) => parent,
        None => return Ok(()),
    };
    let base_name = match base_path.file_name() {
        Some(name) => name.to_string_lossy(),
        None => return Ok(()),
    };
    let prefix = format!("{base_name}.old-");

    let mut paths = Vec::new();
    for entry in fs::read_dir(parent)
        .with_context(|| format!("failed to read dir for log cleanup: {}", parent.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            paths.push(entry.path());
        }
    }
    cleanup_paths(paths, max_age_days, max_keep)
}

fn cleanup_artifact_dir(dir: &Path, max_age_days: u64, max_keep: usize) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read artifact dir: {}", dir.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        paths.push(entry.path());
    }
    cleanup_paths(paths, max_age_days, max_keep)
}

fn cleanup_paths(paths: Vec<PathBuf>, max_age_days: u64, max_keep: usize) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let now = SystemTime::now();
    let max_age = Duration::from_secs(max_age_days.saturating_mul(24 * 60 * 60));
    let cutoff = now.checked_sub(max_age).unwrap_or(UNIX_EPOCH);

    let mut existing = Vec::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        let modified = path_modified(&path);
        if modified < cutoff {
            remove_path(&path)?;
        } else {
            existing.push((path, modified));
        }
    }

    existing.sort_by(|a, b| b.1.cmp(&a.1));
    if existing.len() > max_keep {
        for (path, _) in existing.into_iter().skip(max_keep) {
            remove_path(&path)?;
        }
    }

    Ok(())
}

fn path_modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove dir {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("failed to remove file {}", path.display()))?;
    }
    Ok(())
}

fn export_debug_bundle(
    config: &Config,
    config_path: &Path,
    manifest_path: &Path,
    last_state: &State,
    rendered_frames: u64,
    exit_reason: &str,
    logger: Option<&DebugLogger>,
) -> Result<()> {
    if !config.debug.export_enabled {
        return Ok(());
    }

    let export_root = PathBuf::from(&config.debug.export_dir);
    fs::create_dir_all(&export_root).with_context(|| {
        format!(
            "failed to create debug export root directory: {}",
            export_root.display()
        )
    })?;

    let run_id = utc_millis();
    let run_dir = export_root.join(format!("run-{run_id}"));
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create run bundle dir: {}", run_dir.display()))?;

    let state_path = PathBuf::from(&config.handoff.write_state);
    let log_path = PathBuf::from(&config.debug.log_file);
    let history_path = PathBuf::from(&config.debug.history_file);

    let mut report = Vec::new();
    report.push(format!("run_id={run_id}"));
    report.push(format!("exit_reason={exit_reason}"));
    report.push(format!("rendered_frames={rendered_frames}"));
    report.push(format!("last_frame_index={}", last_state.frame_index));
    report.push(format!("last_pts_ms={}", last_state.pts_ms));
    report.push(format!("config_path={}", config_path.display()));
    report.push(format!("manifest_path={}", manifest_path.display()));
    report.push(format!("state_path={}", state_path.display()));
    report.push(format!("log_path={}", log_path.display()));
    report.push(format!("history_path={}", history_path.display()));
    report.push(String::new());

    copy_file_if_exists(config_path, &run_dir.join("config.toml"), &mut report);
    copy_file_if_exists(manifest_path, &run_dir.join("manifest.json"), &mut report);
    copy_file_if_exists(&state_path, &run_dir.join("state.json"), &mut report);
    copy_file_if_exists(&log_path, &run_dir.join("boot-ui.log"), &mut report);
    copy_file_if_exists(
        &history_path,
        &run_dir.join("boot-ui-history.log"),
        &mut report,
    );

    report.push(String::new());
    report.push("last_log_lines:".to_string());
    report.extend(read_last_lines(&log_path, 120).into_iter().map(|line| format!("  {line}")));
    report.push(String::new());
    report.push("last_history_lines:".to_string());
    report.extend(
        read_last_lines(&history_path, 120)
            .into_iter()
            .map(|line| format!("  {line}")),
    );

    let summary = report.join("\n");
    let summary_path = run_dir.join("debug-summary.txt");
    fs::write(&summary_path, &summary)
        .with_context(|| format!("failed to write summary file: {}", summary_path.display()))?;

    let latest_path = export_root.join("debug-latest.txt");
    fs::write(&latest_path, summary)
        .with_context(|| format!("failed to write latest debug file: {}", latest_path.display()))?;

    if config.debug.cleanup_enabled {
        cleanup_artifact_dir(
            &export_root,
            config.debug.max_artifact_age_days,
            config.debug.max_artifacts,
        )?;
    }

    if let Some(log) = logger {
        log.info(format!(
            "debug bundle exported: run_dir={}, latest={}",
            run_dir.display(),
            latest_path.display()
        ));
    }

    Ok(())
}

fn copy_file_if_exists(src: &Path, dst: &Path, report: &mut Vec<String>) {
    if !src.exists() {
        report.push(format!("copy skipped (missing): {}", src.display()));
        return;
    }
    match fs::copy(src, dst) {
        Ok(bytes) => report.push(format!(
            "copied: {} -> {} ({} bytes)",
            src.display(),
            dst.display(),
            bytes
        )),
        Err(err) => report.push(format!(
            "copy failed: {} -> {} ({err})",
            src.display(),
            dst.display()
        )),
    }
}

fn read_last_lines(path: &Path, lines: usize) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return vec![format!("(unavailable) {}", path.display())],
    };
    let mut out = content
        .lines()
        .rev()
        .take(lines)
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    out.reverse();
    out
}

fn utc_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

struct TerminalGuard {
    saved_tty_mode: Option<String>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let saved_tty_mode = configure_tty_for_immediate_input();
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
            .context("failed to initialize terminal")?;
        stdout.flush().context("failed to flush terminal init")?;
        Ok(Self { saved_tty_mode })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(saved_mode) = self.saved_tty_mode.as_ref() {
            let _ = Command::new("stty").arg(saved_mode).status();
        }
        let _ = io::stdout().write_all(b"\x1b[?25h\x1b[0m\n");
        let _ = io::stdout().flush();
    }
}

fn configure_tty_for_immediate_input() -> Option<String> {
    let saved_mode = Command::new("stty").arg("-g").output().ok().and_then(|out| {
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            None
        }
    })?;

    let status = Command::new("stty")
        .args(["-echo", "-icanon", "-isig", "-ixon", "min", "1", "time", "0"])
        .status()
        .ok()?;
    if status.success() {
        Some(saved_mode)
    } else {
        None
    }
}
