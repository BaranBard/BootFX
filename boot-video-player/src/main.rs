//! boot-video-player -- Post-boot graphical continuation.
//!
//! Reads handoff state from boot-ui and resumes the original video from pts_ms.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use bootfx_core::{Config, SddmConfig, State, DEFAULT_CONFIG_PATH, DEFAULT_STATE_PATH};

#[derive(Debug)]
struct Args {
    config_path: PathBuf,
    state_path: PathBuf,
    video_path_override: Option<PathBuf>,
    dry_run: bool,
}

#[derive(Debug, Clone, Default)]
struct ResolvedSessionEnv {
    display: Option<String>,
    xdg_runtime_dir: Option<String>,
    xauthority: Option<String>,
    wayland_display: Option<String>,
    sources: Vec<String>,
}

impl ResolvedSessionEnv {
    fn add_source(&mut self, source: &str) {
        if !self.sources.iter().any(|existing| existing == source) {
            self.sources.push(source.to_string());
        }
    }

    fn merge_missing(&mut self, other: ResolvedSessionEnv) {
        if self.display.is_none() {
            self.display = other.display;
        }
        if self.xdg_runtime_dir.is_none() {
            self.xdg_runtime_dir = other.xdg_runtime_dir;
        }
        if self.xauthority.is_none() {
            self.xauthority = other.xauthority;
        }
        if self.wayland_display.is_none() {
            self.wayland_display = other.wayland_display;
        }
        for source in other.sources {
            self.add_source(&source);
        }
    }

    fn source_label(&self) -> String {
        if self.sources.is_empty() {
            "none".to_string()
        } else {
            self.sources.join("+")
        }
    }
}

#[derive(Debug, Clone)]
struct LoginSession {
    id: String,
    active: bool,
    state: String,
    session_type: String,
    class: String,
    name: String,
    display: String,
    leader: Option<u32>,
    uid: Option<u32>,
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
        State::load_from_path(&args.state_path)
            .with_context(|| format!("failed to read state file: {}", args.state_path.display()))?
    } else {
        State {
            frame_index: 0,
            pts_ms: 0,
        }
    };

    if !args.dry_run {
        consume_state_file(&args.state_path);
    }

    let video_path = select_video_path(&args, &config)?;
    if config.sddm.video_background_enabled {
        let sddm_video_path = if config.sddm.video_path.trim().is_empty() {
            video_path.clone()
        } else {
            PathBuf::from(config.sddm.video_path.clone())
        };

        if args.dry_run {
            eprintln!(
                "boot-video-player: dry-run: would update SDDM theme `{}` with video={} start_ms={}",
                config.sddm.theme,
                sddm_video_path.display(),
                state.pts_ms
            );
        } else {
            let conf_path =
                update_sddm_theme_background(&config.sddm, &sddm_video_path, state.pts_ms)?;
            eprintln!(
                "boot-video-player: SDDM background updated: theme={}, conf={}",
                config.sddm.theme,
                conf_path.display()
            );
        }

        if !config.sddm.launch_external_player {
            eprintln!("boot-video-player: external player launch disabled by sddm.launch_external_player=false");
            return Ok(());
        }
    }

    let player = choose_player(&config);
    let mut command = build_player_command(&player, &config.video.args, &video_path, state.pts_ms);

    let session_env = resolve_session_env_with_wait();
    apply_session_env(&mut command, &session_env);

    eprintln!(
        "boot-video-player: player=`{}`, start_ms={}, video={}",
        player,
        state.pts_ms,
        video_path.display()
    );
    eprintln!(
        "boot-video-player: session-env source={}, DISPLAY={}, XDG_RUNTIME_DIR={}, XAUTHORITY={}, WAYLAND_DISPLAY={}",
        session_env.source_label(),
        session_env.display.as_deref().unwrap_or("<unset>"),
        session_env
            .xdg_runtime_dir
            .as_deref()
            .unwrap_or("<unset>"),
        session_env.xauthority.as_deref().unwrap_or("<unset>"),
        session_env
            .wayland_display
            .as_deref()
            .unwrap_or("<unset>")
    );
    if session_env.xauthority.is_none() {
        eprintln!(
            "boot-video-player: warning: XAUTHORITY is not set; X11 authorization may fail"
        );
    }

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

fn resolve_session_env_with_wait() -> ResolvedSessionEnv {
    const SESSION_WAIT_MAX: Duration = Duration::from_secs(18);
    const SESSION_WAIT_STEP: Duration = Duration::from_millis(600);

    let start = std::time::Instant::now();
    let mut attempts = 0u32;
    let mut last = ResolvedSessionEnv::default();

    loop {
        attempts += 1;
        let current = resolve_session_env();

        if session_env_is_usable(&current) {
            if attempts > 1 {
                eprintln!(
                    "boot-video-player: session env became usable after {} attempts ({} ms)",
                    attempts,
                    start.elapsed().as_millis()
                );
            }
            return current;
        }

        last = current;
        if start.elapsed() >= SESSION_WAIT_MAX {
            eprintln!(
                "boot-video-player: session env did not become fully usable after {} attempts ({} ms), continuing with best effort",
                attempts,
                start.elapsed().as_millis()
            );
            return last;
        }

        thread::sleep(SESSION_WAIT_STEP);
    }
}

fn resolve_session_env() -> ResolvedSessionEnv {
    let mut resolved = ResolvedSessionEnv::default();

    let process_display = read_env_var("DISPLAY");
    let process_xdg_runtime = read_env_var("XDG_RUNTIME_DIR");
    let process_xauthority = read_env_var("XAUTHORITY");
    let process_wayland_display = read_env_var("WAYLAND_DISPLAY");

    if process_display.is_some()
        || process_xdg_runtime.is_some()
        || process_xauthority.is_some()
        || process_wayland_display.is_some()
    {
        resolved.add_source("process-env");
    }
    resolved.display = process_display;
    resolved.xdg_runtime_dir = process_xdg_runtime;
    resolved.xauthority = process_xauthority;
    resolved.wayland_display = process_wayland_display;

    match detect_session_env_from_loginctl() {
        Ok(Some(detected)) => resolved.merge_missing(detected),
        Ok(None) => {}
        Err(err) => {
            eprintln!("boot-video-player: session autodetect via loginctl failed: {err:#}");
        }
    }

    if resolved.xauthority.is_none() {
        if let Some(path) = detect_sddm_xauthority() {
            resolved.xauthority = Some(path);
            resolved.add_source("sddm-xauth-fallback");
        }
    }

    if resolved.xdg_runtime_dir.is_none() {
        let sddm_runtime = Path::new("/run/sddm");
        if sddm_runtime.is_dir() {
            resolved.xdg_runtime_dir = Some(sddm_runtime.to_string_lossy().to_string());
            resolved.add_source("sddm-runtime-fallback");
        }
    }

    if resolved.display.is_none() && resolved.wayland_display.is_none() {
        resolved.display = Some(":0".to_string());
        resolved.add_source("display-fallback");
    }

    resolved
}

fn apply_session_env(command: &mut Command, session_env: &ResolvedSessionEnv) {
    let prefer_wayland_only = session_env.wayland_display.is_some() && session_env.xauthority.is_none();

    if prefer_wayland_only {
        command.env_remove("DISPLAY");
        eprintln!(
            "boot-video-player: wayland session without XAUTHORITY detected; skipping DISPLAY to avoid X11 fallback"
        );
    } else if let Some(value) = &session_env.display {
        command.env("DISPLAY", value);
    } else {
        command.env_remove("DISPLAY");
    }
    if let Some(value) = &session_env.xdg_runtime_dir {
        command.env("XDG_RUNTIME_DIR", value);
    } else {
        command.env_remove("XDG_RUNTIME_DIR");
    }
    if let Some(value) = &session_env.xauthority {
        command.env("XAUTHORITY", value);
    } else {
        command.env_remove("XAUTHORITY");
    }
    if let Some(value) = &session_env.wayland_display {
        command.env("WAYLAND_DISPLAY", value);
    } else {
        command.env_remove("WAYLAND_DISPLAY");
    }
}

fn session_env_is_usable(session_env: &ResolvedSessionEnv) -> bool {
    let has_wayland = session_env.wayland_display.is_some() && session_env.xdg_runtime_dir.is_some();
    let has_x11 = session_env.display.is_some() && session_env.xauthority.is_some();
    has_wayland || has_x11
}

fn consume_state_file(state_path: &Path) {
    match fs::remove_file(state_path) {
        Ok(()) => {
            eprintln!(
                "boot-video-player: consumed state file and removed {}",
                state_path.display()
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            eprintln!(
                "boot-video-player: warning: failed to remove state file {}: {}",
                state_path.display(),
                err
            );
        }
    }
}

fn detect_sddm_xauthority() -> Option<String> {
    let sddm_dir = Path::new("/run/sddm");
    let entries = fs::read_dir(sddm_dir).ok()?;

    let mut candidates: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = path.file_name()?.to_string_lossy();
        if !file_name.starts_with("xauth_") {
            continue;
        }
        let modified = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);
        candidates.push((path, modified));
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates
        .first()
        .map(|(path, _)| path.to_string_lossy().to_string())
}

fn detect_session_env_from_loginctl() -> Result<Option<ResolvedSessionEnv>> {
    let session_ids = list_session_ids()?;
    if session_ids.is_empty() {
        return Ok(None);
    }

    let mut sessions = Vec::new();
    for session_id in session_ids {
        if let Some(session) = session_from_id(&session_id)? {
            sessions.push(session);
        }
    }
    let session = match pick_best_session(sessions) {
        Some(session) => session,
        None => return Ok(None),
    };

    let leader_env = session
        .leader
        .map(read_leader_environment)
        .unwrap_or_default();
    let leader_display = leader_env.get("DISPLAY").cloned();
    let leader_xdg_runtime = leader_env.get("XDG_RUNTIME_DIR").cloned();
    let leader_xauthority = leader_env.get("XAUTHORITY").cloned();
    let leader_wayland_display = leader_env.get("WAYLAND_DISPLAY").cloned();

    let runtime_from_uid = session
        .uid
        .map(|uid| format!("/run/user/{uid}"))
        .filter(|path| Path::new(path).is_dir());
    let xdg_runtime_dir = choose_non_empty([leader_xdg_runtime, runtime_from_uid]);

    let wayland_display = choose_non_empty([
        leader_wayland_display,
        xdg_runtime_dir
            .as_deref()
            .and_then(detect_wayland_display),
    ]);

    let xauthority = choose_non_empty([
        leader_xauthority,
        first_existing_file(candidate_xauthority_paths(&session, xdg_runtime_dir.as_deref())),
    ]);

    let display = choose_non_empty([leader_display, Some(session.display.clone())]);

    let mut resolved = ResolvedSessionEnv {
        display,
        xdg_runtime_dir,
        xauthority,
        wayland_display,
        sources: Vec::new(),
    };
    resolved.add_source("loginctl");
    resolved.add_source("proc-environ");

    eprintln!(
        "boot-video-player: selected session id={}, user={}, type={}, class={}, state={}, active={}",
        session.id, session.name, session.session_type, session.class, session.state, session.active
    );

    Ok(Some(resolved))
}

fn list_session_ids() -> Result<Vec<String>> {
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .context("failed to run `loginctl list-sessions --no-legend`")?;
    if !output.status.success() {
        bail!(
            "`loginctl list-sessions` exited with status {}",
            output.status
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut session_ids = Vec::new();
    for line in stdout.lines() {
        if let Some(session_id) = line.split_whitespace().next() {
            if !session_id.is_empty() {
                session_ids.push(session_id.to_string());
            }
        }
    }
    Ok(session_ids)
}

fn session_from_id(session_id: &str) -> Result<Option<LoginSession>> {
    let output = Command::new("loginctl")
        .args([
            "show-session",
            session_id,
            "-p",
            "Active",
            "-p",
            "State",
            "-p",
            "Type",
            "-p",
            "Class",
            "-p",
            "Name",
            "-p",
            "Display",
            "-p",
            "Leader",
            "-p",
            "User",
        ])
        .output()
        .with_context(|| format!("failed to run `loginctl show-session {session_id}`"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let fields = parse_key_value_block(&String::from_utf8_lossy(&output.stdout));
    let active = fields
        .get("Active")
        .map(|value| value == "yes")
        .unwrap_or(false);

    Ok(Some(LoginSession {
        id: session_id.to_string(),
        active,
        state: fields.get("State").cloned().unwrap_or_default(),
        session_type: fields
            .get("Type")
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase(),
        class: fields
            .get("Class")
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase(),
        name: fields.get("Name").cloned().unwrap_or_default(),
        display: fields.get("Display").cloned().unwrap_or_default(),
        leader: fields.get("Leader").and_then(|value| value.parse::<u32>().ok()),
        uid: fields.get("User").and_then(|value| value.parse::<u32>().ok()),
    }))
}

fn parse_key_value_block(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if !key.is_empty() {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

fn pick_best_session(sessions: Vec<LoginSession>) -> Option<LoginSession> {
    sessions
        .into_iter()
        .max_by_key(|session| session_score(session))
}

fn session_score(session: &LoginSession) -> i32 {
    let mut score = 0;
    if session.active {
        score += 100;
    }
    if session.state == "active" {
        score += 40;
    }
    if session.session_type == "x11" || session.session_type == "wayland" {
        score += 30;
    }
    if session.class == "user" {
        score += 20;
    } else if session.class == "greeter" {
        score += 10;
    }
    if !session.display.is_empty() {
        score += 8;
    }
    if session.leader.is_some() {
        score += 4;
    }
    if session.uid.is_some() {
        score += 2;
    }
    score
}

fn read_leader_environment(pid: u32) -> HashMap<String, String> {
    let path = format!("/proc/{pid}/environ");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    for item in bytes.split(|byte| *byte == 0u8) {
        if item.is_empty() {
            continue;
        }
        let raw = match std::str::from_utf8(item) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let mut parts = raw.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if !key.is_empty() {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

fn read_env_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn choose_non_empty<I>(candidates: I) -> Option<String>
where
    I: IntoIterator<Item = Option<String>>,
{
    for candidate in candidates {
        if let Some(value) = candidate {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn candidate_xauthority_paths(session: &LoginSession, xdg_runtime_dir: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(runtime_dir) = xdg_runtime_dir {
        paths.push(PathBuf::from(runtime_dir).join("Xauthority"));
        paths.push(PathBuf::from(runtime_dir).join("gdm").join("Xauthority"));
    }

    if !session.name.is_empty() {
        paths.push(PathBuf::from(format!("/home/{}/.Xauthority", session.name)));
        paths.push(PathBuf::from(format!("/var/lib/{}/.Xauthority", session.name)));
    }

    paths.push(PathBuf::from("/var/lib/sddm/.Xauthority"));
    paths.push(PathBuf::from("/root/.Xauthority"));

    paths
}

fn first_existing_file(paths: Vec<PathBuf>) -> Option<String> {
    for path in paths {
        if path.is_file() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

fn detect_wayland_display(runtime_dir: &str) -> Option<String> {
    let entries = fs::read_dir(runtime_dir).ok()?;
    let mut candidate = None;

    for entry in entries {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("wayland-") {
            candidate = Some(name.to_string());
            if name == "wayland-0" {
                return candidate;
            }
        }
    }

    candidate
}

fn update_sddm_theme_background(
    sddm: &SddmConfig,
    video_path: &Path,
    start_ms: u64,
) -> Result<PathBuf> {
    let theme_dir = Path::new(&sddm.theme_root).join(&sddm.theme);
    if !theme_dir.is_dir() {
        bail!(
            "sddm theme directory does not exist: {}",
            theme_dir.display()
        );
    }

    let conf_user = theme_dir.join("theme.conf.user");
    upsert_sddm_general_keys(
        &conf_user,
        &[
            ("BootFXVideoEnabled", "true".to_string()),
            (
                "BootFXVideoPath",
                video_path.as_os_str().to_string_lossy().to_string(),
            ),
            ("BootFXStartMs", start_ms.to_string()),
            ("BootFXUseVideoBackground", "true".to_string()),
        ],
    )?;
    Ok(conf_user)
}

fn upsert_sddm_general_keys(path: &Path, kv: &[(&str, String)]) -> Result<()> {
    let original = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = if original.is_empty() {
        Vec::new()
    } else {
        original.lines().map(|line| line.to_string()).collect()
    };

    let section = find_ini_section_bounds(&lines, "General");
    if let Some((section_start, mut section_end)) = section {
        for (key, value) in kv {
            let mut updated = false;
            for line in lines.iter_mut().take(section_end).skip(section_start + 1) {
                if ini_line_key_equals(line, key) {
                    *line = format!("{key}={value}");
                    updated = true;
                    break;
                }
            }
            if !updated {
                lines.insert(section_end, format!("{key}={value}"));
                section_end += 1;
            }
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[General]".to_string());
        for (key, value) in kv {
            lines.push(format!("{key}={value}"));
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create SDDM theme dir: {}", parent.display()))?;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(path, out)
        .with_context(|| format!("failed to write SDDM theme config: {}", path.display()))?;
    Ok(())
}

fn find_ini_section_bounds(lines: &[String], section: &str) -> Option<(usize, usize)> {
    let marker = format!("[{section}]");
    let start = lines.iter().position(|line| line.trim() == marker)?;
    let mut end = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            end = idx;
            break;
        }
    }
    Some((start, end))
}

fn ini_line_key_equals(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with(';') {
        return false;
    }
    if let Some((lhs, _)) = trimmed.split_once('=') {
        return lhs.trim() == key;
    }
    false
}
