//! # In-binary microbenchmark harness
//!
//! `shotori --bench [name]` — dispatched in `main.rs` on `argv[1]` BEFORE
//! clap runs (same convention as the `--notify` / `--clipboard-daemon`
//! child entry points). Zero new dependencies; release build only — debug
//! timings of the pure-Rust pixel code are meaningless (10–100× slower;
//! benchmark with the release install).
//!
//! Inputs are generated ONCE per benchmark (fixed-seed LCG noise, so A/B
//! runs are bit-identical) and only the measured function runs inside the
//! timing loop. Each benchmark samples until at least ~300 ms have
//! accumulated (≥2 runs, ≤25 runs; the first run is discarded as warmup)
//! and reports mean/min ms.

use std::time::{Duration, Instant};

pub const BENCH_ARG: &str = "--bench";

/// Deterministic LCG (fixed seed → reproducible A/B inputs)
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 32
    }
    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }
}

/// Smooth diagonal gradient (best-case PNG compression)
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w as usize) * (h as usize) * 4];
    let mut i = 0;
    for y in 0..h {
        for x in 0..w {
            v[i] = (x % 256) as u8;
            v[i + 1] = (y % 256) as u8;
            v[i + 2] = ((x + y) % 256) as u8;
            v[i + 3] = 255;
            i += 4;
        }
    }
    v
}

/// Pure noise (worst-case PNG compression — incompressible)
fn noise(w: u32, h: u32) -> Vec<u8> {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut v = vec![0u8; (w as usize) * (h as usize) * 4];
    for chunk in v.chunks_mut(4) {
        chunk[0] = rng.next_u8();
        chunk[1] = rng.next_u8();
        chunk[2] = rng.next_u8();
        chunk[3] = 255;
    }
    v
}

/// Screenshot-like: flat colored blocks separated by thin 1px lines, with
/// 1–3 LSB dither (typical desktop UI — the real-world middle ground)
fn ui(w: u32, h: u32) -> Vec<u8> {
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let mut v = vec![0u8; (w as usize) * (h as usize) * 4];
    const BW: u32 = 128;
    const BH: u32 = 96;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) as usize) * 4;
            if x % BW == 0 || y % BH == 0 {
                v[i..i + 4].copy_from_slice(&[40, 40, 44, 255]);
            } else {
                let block = (x / BW).wrapping_mul(7) + y / BH;
                let c = match block % 5 {
                    0 => [236, 238, 241],
                    1 => [245, 246, 248],
                    2 => [228, 231, 236],
                    3 => [250, 250, 252],
                    _ => [222, 226, 232],
                };
                let d = (rng.next_u8() & 3) as i32;
                v[i] = (c[0] + d) as u8;
                v[i + 1] = (c[1] + d) as u8;
                v[i + 2] = (c[2] + d) as u8;
                v[i + 3] = 255;
            }
        }
    }
    v
}

type Workload = Box<dyn FnMut() -> Vec<u8>>;

/// Run `workload` until ≥300 ms of sample time has accumulated (≥2 runs,
/// ≤25 runs); discard the first run; report mean/min ms and output size.
fn run(name: &str, mut workload: Workload) {
    let min_time = Duration::from_millis(300);
    let max_runs = 25u32;
    let _ = (workload)(); // untimed warmup (allocation caches, page faults)
    let mut times = Vec::new();
    let start = Instant::now();
    let mut last = (workload)();
    let dt = start.elapsed();
    times.push(dt);
    let mut total = dt;
    while total < min_time && (times.len() as u32) < max_runs {
        let start = Instant::now();
        last = (workload)();
        let dt = start.elapsed();
        times.push(dt);
        total += dt;
    }
    let mean: Duration = times.iter().sum::<Duration>() / times.len() as u32;
    let min = times.iter().copied().min().unwrap();
    let size_note = if last.len() > 1024 && name.starts_with("png") {
        format!(" → {} KB", last.len() / 1024)
    } else {
        String::new()
    };
    println!(
        "[bench] {name}: mean {:.1} ms  (min {:.1} ms, {} runs){size_note}",
        mean.as_secs_f64() * 1e3,
        min.as_secs_f64() * 1e3,
        times.len(),
    );
}

pub fn bench_main() -> i32 {
    let requested = std::env::args().nth(2).unwrap_or_else(|| "all".into());
    println!(
        "[bench] shotori {} — profile: {}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "DEBUG (meaningless timings!)"
        } else {
            "release"
        }
    );
    let mut ran = 0;

    // ── PNG encoding: the Enter-key cost ──────────────────────────────
    if matches!(requested.as_str(), "all" | "png") {
        let cases = || -> Vec<(&'static str, u32, u32, Vec<u8>)> {
            vec![
                ("png1080-gradient", 1920, 1080, gradient(1920, 1080)),
                ("png1080-ui", 1920, 1080, ui(1920, 1080)),
                ("png1080-noise", 1920, 1080, noise(1920, 1080)),
                ("png4k-ui", 3840, 2160, ui(3840, 2160)),
                ("png4k-noise", 3840, 2160, noise(3840, 2160)),
            ]
        };
        for (suffix, encode) in [
            (
                "-bal",
                crate::model::export::encode_png as fn(u32, u32, &[u8]) -> anyhow::Result<Vec<u8>>,
            ),
            (
                "-fast",
                crate::model::export::encode_png_fast
                    as fn(u32, u32, &[u8]) -> anyhow::Result<Vec<u8>>,
            ),
        ] {
            for (tag, w, h, input) in cases() {
                let name = format!("{tag}{suffix}");
                let workload: Workload = Box::new(move || encode(w, h, &input).unwrap());
                run(&name, workload);
                ran += 1;
            }
        }
    }

    // ── Full export pipeline: crop (row memcpy) + encode ─────────────
    if matches!(requested.as_str(), "all" | "crop") {
        for (tag, cap, sel) in [
            (
                "crop+png1080-out-of-4k",
                ui(3840, 2160),
                (960.0_f32, 540.0, 1920.0, 1080.0),
            ),
            (
                "crop+png4k-full",
                ui(3840, 2160),
                (0.0_f32, 0.0, 3840.0, 2160.0),
            ),
        ] {
            use crate::model::export;
            use gpui_kit::{Bounds, point, px, size};
            let (x, y, sw, sh) = sel;
            let bounds = Bounds {
                origin: point(px(x), px(y)),
                size: size(px(sw), px(sh)),
            };
            let name = tag.to_string();
            let workload: Workload = Box::new(move || {
                let (w, h, rgba) = export::crop(&cap, 3840, 2160, bounds, 1.0).unwrap();
                export::encode_png(w, h, &rgba).unwrap()
            });
            run(&name, workload);
            ran += 1;
        }
    }

    // ── Capture format conversion (Linux screencopy path) ────────────
    #[cfg(target_os = "linux")]
    if matches!(requested.as_str(), "all" | "convert") {
        use wayland_client::protocol::wl_shm;
        let (w, h) = (3840, 2160);
        // 4K XRGB8888 buffer in memory byte order B,G,R,X (stride = w*4)
        let src = ui(w, h);
        let xrgb = std::sync::Arc::new(
            src.chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect::<Vec<u8>>(),
        );
        let xrgb2 = xrgb.clone();
        let workload: Workload = Box::new(move || {
            crate::platform::capture::convert_to_rgba(
                &xrgb,
                wl_shm::Format::Xrgb8888,
                w as i32,
                h as i32,
                w as i32 * 4,
                false,
            )
        });
        run("convert4k-xrgb", workload);
        let workload: Workload = Box::new(move || {
            crate::platform::capture::convert_to_rgba(
                &xrgb2,
                wl_shm::Format::Xrgb8888,
                w as i32,
                h as i32,
                w as i32 * 4,
                true,
            )
        });
        run("convert4k-xrgb-yinvert", workload);
        ran += 2;
    }

    // ── Rotation (rare: rotated panels) ──────────────────────────────
    if matches!(requested.as_str(), "all" | "rotate") {
        let src = std::sync::Arc::new(ui(1920, 1080));
        let src2 = src.clone();
        let workload: Workload = Box::new(move || {
            crate::platform::capture::rotate_rgba(
                (src.as_ref()).clone(),
                1920,
                1080,
                crate::platform::capture::Transform::Rot90,
            )
        });
        run("rotate1080p-90", workload);
        let workload: Workload = Box::new(move || {
            crate::platform::capture::rotate_rgba(
                (src2.as_ref()).clone(),
                1920,
                1080,
                crate::platform::capture::Transform::Rot180,
            )
        });
        run("rotate1080p-180", workload);
        ran += 2;
    }

    // ── Annotation filters on a 1080p selection (the drag-jank path) ─
    if matches!(requested.as_str(), "all" | "filter") {
        use crate::annotation::{Annotations, ShapeKind};
        use gpui_kit::{Bounds, point, px, size};
        let (w, h) = (1920u32, 1080u32);
        let sel = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let make = |kind: ShapeKind| {
            let mut a = Annotations::default();
            a.toggle(kind);
            a.set_tool_size(24.); // strongest filter (spec max is 48, 24 = old L)
            a.begin(point(px(100.), px(100.)), sel, false);
            a.drag_to(point(px(800.), px(600.)), sel, false);
            a.end();
            a
        };
        let base = std::sync::Arc::new(ui(w, h));
        for (ann, tag) in [
            (make(ShapeKind::Mosaic), "mosaic1080p-strength24"),
            (make(ShapeKind::Blur), "blur1080p-strength24"),
        ] {
            let base2 = base.clone();
            let workload: Workload = Box::new(move || {
                let mut rgba = base2.as_ref().clone();
                ann.rasterize(&mut rgba, w, h, point(px(0.), px(0.)), 1.0);
                rgba
            });
            run(tag, workload);
            ran += 1;
        }
    }

    // ── Dense freehand stroke export (the Ctrl+C cost after a long
    //    pencil stroke: full coverage() replay over every capsule) ──────
    if matches!(requested.as_str(), "all" | "stroke") {
        use crate::annotation::{Annotations, ShapeKind};
        use gpui_kit::{Bounds, point, px, size};

        /// Deterministic dense Lissajous scribble with per-point jitter.
        fn scribble(n: usize, w: f32, h: f32) -> Vec<gpui_kit::Point<gpui_kit::Pixels>> {
            let mut rng = Rng(0x51ed270b);
            let mut pts = Vec::with_capacity(n);
            for i in 0..n {
                let t = i as f32 * 0.021;
                let x = (0.5 + 0.45 * t.sin()) * w + f32::from(rng.next_u8()) * 0.8;
                let y = (0.5 + 0.45 * (2.3 * t).sin()) * h + f32::from(rng.next_u8()) * 0.8;
                pts.push(point(px(x), px(y)));
            }
            pts
        }

        for (w, h, n, tag) in [
            (1920u32, 1080u32, 2000usize, "stroke1080p-p2000"),
            (3840u32, 2160u32, 6000usize, "stroke4k-p6000"),
        ] {
            let sel = Bounds::new(point(px(0.), px(0.)), size(px(w as f32), px(h as f32)));
            let pts = scribble(n, w as f32, h as f32);
            let mut ann = Annotations::default();
            ann.toggle(ShapeKind::Pencil);
            ann.begin(pts[0], sel, false);
            for p in &pts[1..] {
                ann.drag_to(*p, sel, false);
            }
            ann.end();
            let base = std::sync::Arc::new(ui(w, h));
            let workload: Workload = Box::new(move || {
                let mut rgba = base.as_ref().clone();
                ann.rasterize(&mut rgba, w, h, point(px(0.), px(0.)), 1.0);
                rgba
            });
            run(tag, workload);
            ran += 1;
        }
    }

    if ran == 0 {
        eprintln!(
            "[bench] unknown benchmark '{requested}' (available: all png crop convert rotate filter stroke)"
        );
        return 1;
    }
    0
}
