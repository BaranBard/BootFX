//! boot-ui-precompute -- Offline video-to-ASCII converter.
//!
//! Pipeline: mp4 -> frames -> resize -> grayscale/edges -> ASCII -> .frame

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use bootfx_core::{FrameMeta, Manifest};
use image::GrayImage;

const DEFAULT_ASCII_CHARSET: &str = " .:-=+*#%@";

#[derive(Debug, Clone, Copy)]
enum Mode {
    Grayscale,
    Edges,
}

impl Mode {
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "grayscale" => Ok(Self::Grayscale),
            "edges" => Ok(Self::Edges),
            other => bail!("unknown mode `{other}`. Expected `grayscale` or `edges`"),
        }
    }
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output_dir: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    mode: Mode,
    charset: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("boot-ui-precompute error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    ensure_ffmpeg()?;

    if args.charset.is_empty() {
        bail!("charset must not be empty");
    }
    if args.width == 0 || args.height == 0 {
        bail!("width and height must be > 0");
    }
    if args.fps == 0 {
        bail!("fps must be > 0");
    }

    fs::create_dir_all(&args.output_dir).with_context(|| {
        format!(
            "failed to create output directory: {}",
            args.output_dir.display()
        )
    })?;
    let frames_dir = args.output_dir.join("frames");
    fs::create_dir_all(&frames_dir)
        .with_context(|| format!("failed to create frames dir: {}", frames_dir.display()))?;

    let temp_png_dir = args.output_dir.join(".tmp-png");
    if temp_png_dir.exists() {
        fs::remove_dir_all(&temp_png_dir)
            .with_context(|| format!("failed to clean temp dir: {}", temp_png_dir.display()))?;
    }
    fs::create_dir_all(&temp_png_dir)
        .with_context(|| format!("failed to create temp dir: {}", temp_png_dir.display()))?;

    extract_frames_with_ffmpeg(&args, &temp_png_dir)?;
    let png_frames = collect_png_frames(&temp_png_dir)?;
    if png_frames.is_empty() {
        bail!("ffmpeg produced zero frames in {}", temp_png_dir.display());
    }

    let charset = args.charset.as_bytes();
    let mut frame_meta = Vec::with_capacity(png_frames.len());
    for (idx, frame_path) in png_frames.iter().enumerate() {
        let gray = image::open(frame_path)
            .with_context(|| format!("failed to decode image frame: {}", frame_path.display()))?
            .to_luma8();
        if gray.width() != args.width || gray.height() != args.height {
            bail!(
                "frame dimensions {}x{} do not match requested {}x{} for {}",
                gray.width(),
                gray.height(),
                args.width,
                args.height,
                frame_path.display()
            );
        }

        let ascii_bytes = match args.mode {
            Mode::Grayscale => grayscale_to_ascii(&gray, charset),
            Mode::Edges => edges_to_ascii(&gray, charset),
        };
        let frame_file_name = format!("{idx:06}.frame");
        let frame_output_path = frames_dir.join(&frame_file_name);
        fs::write(&frame_output_path, ascii_bytes).with_context(|| {
            format!("failed to write frame file: {}", frame_output_path.display())
        })?;

        frame_meta.push(FrameMeta {
            index: idx as u64,
            pts_ms: (idx as u64 * 1000) / args.fps as u64,
            file: format!("frames/{frame_file_name}"),
        });
    }

    let manifest = Manifest {
        fps: args.fps,
        width: args.width,
        height: args.height,
        frame_count: frame_meta.len() as u64,
        frames: frame_meta,
    };
    let manifest_path = args.output_dir.join("manifest.json");
    manifest.write_to_path(&manifest_path)?;

    fs::remove_dir_all(&temp_png_dir)
        .with_context(|| format!("failed to remove temp dir: {}", temp_png_dir.display()))?;

    println!(
        "Generated {} frames and manifest at {}",
        manifest.frame_count,
        manifest_path.display()
    );
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut input: Option<PathBuf> = None;
    let mut output_dir: Option<PathBuf> = None;
    let mut width = 120;
    let mut height = 40;
    let mut fps = 15;
    let mut mode = Mode::Grayscale;
    let mut charset = DEFAULT_ASCII_CHARSET.to_string();

    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--input" => input = Some(PathBuf::from(next_value("--input", &mut iter)?)),
            "--output-dir" => {
                output_dir = Some(PathBuf::from(next_value("--output-dir", &mut iter)?))
            }
            "--width" => width = parse_u32_arg("--width", &next_value("--width", &mut iter)?)?,
            "--height" => {
                height = parse_u32_arg("--height", &next_value("--height", &mut iter)?)?
            }
            "--fps" => fps = parse_u32_arg("--fps", &next_value("--fps", &mut iter)?)?,
            "--mode" => mode = Mode::parse(&next_value("--mode", &mut iter)?)?,
            "--charset" => charset = next_value("--charset", &mut iter)?,
            other => bail!("unknown argument `{other}`. Use --help"),
        }
    }

    let input = input.ok_or_else(|| anyhow!("missing required argument --input"))?;
    let output_dir =
        output_dir.ok_or_else(|| anyhow!("missing required argument --output-dir"))?;

    Ok(Args {
        input,
        output_dir,
        width,
        height,
        fps,
        mode,
        charset,
    })
}

fn print_help() {
    println!(
        "\
boot-ui-precompute

Usage:
  boot-ui-precompute --input <video.mp4> --output-dir <dir> [options]

Options:
  --input <path>        Source video file path (required)
  --output-dir <path>   Output directory for frames + manifest (required)
  --width <n>           Character-grid width (default: 120)
  --height <n>          Character-grid height (default: 40)
  --fps <n>             Target frame rate (default: 15)
  --mode <name>         grayscale | edges (default: grayscale)
  --charset <chars>     ASCII ramp from dark to bright
"
    );
}

fn next_value(flag: &str, iter: &mut impl Iterator<Item = String>) -> Result<String> {
    iter.next().ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn parse_u32_arg(flag: &str, raw: &str) -> Result<u32> {
    raw.parse::<u32>()
        .with_context(|| format!("invalid value for {flag}: `{raw}`"))
}

fn ensure_ffmpeg() -> Result<()> {
    let output = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .context("failed to execute ffmpeg; ensure it is installed and on PATH")?;
    if !output.status.success() {
        bail!("ffmpeg is installed but returned non-zero status for -version");
    }
    Ok(())
}

fn extract_frames_with_ffmpeg(args: &Args, temp_png_dir: &Path) -> Result<()> {
    let output_pattern = temp_png_dir.join("%06d.png");
    let vf = format!(
        "fps={},scale={}:{},format=gray",
        args.fps, args.width, args.height
    );

    let status = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(&args.input)
        .arg("-vf")
        .arg(vf)
        .arg(output_pattern)
        .status()
        .context("failed to run ffmpeg for frame extraction")?;

    if !status.success() {
        bail!("ffmpeg frame extraction failed");
    }
    Ok(())
}

fn collect_png_frames(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut frames = fs::read_dir(dir)
        .with_context(|| format!("failed to list temp frames dir: {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    frames.sort();
    Ok(frames)
}

fn grayscale_to_ascii(gray: &GrayImage, charset: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity((gray.width() * gray.height()) as usize);
    for pixel in gray.pixels() {
        out.push(map_luma_to_ascii(pixel.0[0], charset));
    }
    out
}

fn edges_to_ascii(gray: &GrayImage, charset: &[u8]) -> Vec<u8> {
    let width = gray.width() as usize;
    let height = gray.height() as usize;
    let mut out = vec![charset[0]; width * height];
    if width < 3 || height < 3 {
        return grayscale_to_ascii(gray, charset);
    }

    for y in 1..(height - 1) {
        for x in 1..(width - 1) {
            let p = |xx: usize, yy: usize| gray.get_pixel(xx as u32, yy as u32).0[0] as f32;

            let gx = -p(x - 1, y - 1) + p(x + 1, y - 1) - 2.0 * p(x - 1, y)
                + 2.0 * p(x + 1, y)
                - p(x - 1, y + 1)
                + p(x + 1, y + 1);
            let gy = p(x - 1, y - 1) + 2.0 * p(x, y - 1) + p(x + 1, y - 1)
                - p(x - 1, y + 1)
                - 2.0 * p(x, y + 1)
                - p(x + 1, y + 1);

            let magnitude = (gx * gx + gy * gy).sqrt().min(255.0) as u8;
            out[y * width + x] = map_luma_to_ascii(magnitude, charset);
        }
    }

    out
}

fn map_luma_to_ascii(luma: u8, charset: &[u8]) -> u8 {
    if charset.len() == 1 {
        return charset[0];
    }
    let idx = (luma as usize * (charset.len() - 1)) / 255;
    charset[idx]
}