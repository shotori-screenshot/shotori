//! # E2E performance suites (`shotori --perf`) — developer-only
//!
//! Behind the `perf` cargo feature: measures user-visible latency of the
//! REAL binary end to end, complementing [`crate::bench`] (component
//! microbenchmarks). Suites:
//!
//! - `startup` — cold start: spawn N real GUI children (auto-quit via the
//!   e2e debug backdoor), parse their `SHOTORI_BOOT` marks, aggregate the
//!   exec→selectable breakdown + peak RSS
//! - `capture` — steady-state screencopy freeze latency + RSS per run
//! - `export` — crop + both PNG encode tiers + disk write on live pixels
//! - `warm-window` — 1st vs 2nd overlay in one GUI process: the delta is
//!   the wgpu/renderer init a resident process would amortize away
//! - `resident` — what the tray launcher holds while idle (it spawns a
//!   fresh gui process per shot, so this is today's standing cost)
//!
//! Every run also reports the measured binary's own size — the header
//! line says exactly which file the numbers apply to.
//!
//! Usage: `shotori --perf [suite...] [--runs N] [--json]`
//!
//! Children coordinate over env only (SHOTORI_BOOT marks + the e2e
//! backdoors), so the measured path is byte-for-byte the user's path.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gpui_kit::*;

pub const PERF_ARG: &str = "--perf";

const SUITES: &[&str] = &["startup", "capture", "export", "warm-window", "resident"];
const CHILD_TIMEOUT: Duration = Duration::from_secs(20);

/// One measured quantity: a label, N samples, and how to print them.
struct Row {
    label: String,
    unit: &'static str,
    values: Vec<f64>,
}

impl Row {
    fn new(label: &str, unit: &'static str) -> Self {
        Self {
            label: label.to_owned(),
            unit,
            values: Vec::new(),
        }
    }

    fn push(&mut self, v: f64) {
        self.values.push(v);
    }
}

/// min / median / p95 / max over the samples (nearest-rank p95)
fn stats(mut v: Vec<f64>) -> (f64, f64, f64, f64) {
    if v.is_empty() {
        return (0., 0., 0., 0.);
    }
    // total_cmp: perf samples come from Instant differences and can
    // contain NaN on clock weirdness — partial_cmp().unwrap() would
    // panic the perf harness exactly when it should report bad data.
    v.sort_by(f64::total_cmp);
    let n = v.len();
    (
        v[0],
        v[n / 2],
        v[((n as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1)],
        v[n - 1],
    )
}

/// Progress chatter: stderr in `--json` mode so stdout stays pure JSON
fn info(json: bool, msg: &str) {
    if json {
        eprintln!("{msg}");
    } else {
        println!("{msg}");
    }
}

fn print_row(label_width: usize, row: &Row) {
    let (min, med, p95, max) = stats(row.values.clone());
    println!(
        "  {:<width$} med {:>8.1} {}   (min {:>8.1}, p95 {:>8.1}, max {:>8.1}, n {})",
        row.label,
        med,
        row.unit,
        min,
        p95,
        max,
        row.values.len(),
        width = label_width,
    );
}

fn print_rows(rows: &[Row]) {
    let width = rows.iter().map(|r| r.label.len()).max().unwrap_or(0);
    for r in rows {
        print_row(width, r);
    }
}

fn rows_to_json(rows: &[Row]) -> serde_json::Value {
    serde_json::Value::Array(
        rows.iter()
            .map(|r| {
                let (min, med, p95, max) = stats(r.values.clone());
                serde_json::json!({
                    "label": r.label,
                    "unit": r.unit,
                    "min": min,
                    "median": med,
                    "p95": p95,
                    "max": max,
                    "samples": r.values,
                })
            })
            .collect(),
    )
}

/// `VmXXX` (kB) from /proc/self/status; None off Linux
fn status_kb(field: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

/// Child-side hook, called on the normal GUI exit path: with SHOTORI_BOOT
/// set, report peak RSS so the parent's startup suite can aggregate it.
pub fn report_child_rss() {
    if std::env::var_os("SHOTORI_BOOT").is_some()
        && let Some(kb) = status_kb("VmHWM")
    {
        eprintln!("[boot-rss] hwm_kb={kb}");
    }
}

/// Parse "[boot] <ms> ms  <label>" lines from a child's stderr
fn parse_boot(stderr: &str) -> Vec<(f64, String)> {
    stderr
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("[boot]")?.trim_start();
            let (ms, label) = rest.split_once(" ms")?;
            Some((ms.trim().parse().ok()?, label.trim().to_owned()))
        })
        .collect()
}

/// Last mark whose label contains `sub` (marks are cumulative ms)
fn milestone<'a>(marks: &'a [(f64, String)], sub: &str) -> Option<&'a (f64, String)> {
    marks.iter().rev().find(|(_, l)| l.contains(sub))
}

/// A finished GUI child: cumulative boot marks + peak-RSS kB
type ChildReport = (Vec<(f64, String)>, Option<f64>);

/// Spawn a real GUI child driven by the e2e backdoor (injects nothing;
/// `SHOTORI_DEBUG_ACTION=quit` ends it 1.5 s after the overlay maps) and
/// collect (boot marks, peak-RSS kB).
fn run_gui_child(extra_env: &[(&str, &str)]) -> anyhow::Result<ChildReport> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmd.env("SHOTORI_BOOT", "1")
        .env("SHOTORI_DEBUG_ACTION", "quit")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn()?;
    let stderr = child.stderr.take().expect("stderr piped");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut buf);
        buf
    });

    let started = Instant::now();
    let mut timed_out = false;
    while child.try_wait()?.is_none() {
        if started.elapsed() > CHILD_TIMEOUT {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait()?;
    let stderr = reader.join().unwrap_or_default();
    if timed_out {
        anyhow::bail!("child ran past {} s — killed", CHILD_TIMEOUT.as_secs());
    }

    let mut rss = None;
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("[boot-rss] hwm_kb=") {
            rss = rest.parse().ok();
        }
    }
    let marks = parse_boot(&stderr);
    Ok((marks, rss))
}

// ── suites ──────────────────────────────────────────────────────────────

/// Cold start: N fresh GUI processes, the real user path end to end
fn suite_startup(runs: usize, json: bool) -> anyhow::Result<serde_json::Value> {
    info(
        json,
        &format!("[perf] startup: {runs} cold starts (exec → selectable overlay)…"),
    );
    let mut rows = [
        Row::new("exec → capture done", "ms"),
        Row::new("exec → gpui run entered", "ms"),
        Row::new("exec → displays matched", "ms"),
        Row::new("window creation (wgpu init)", "ms"),
        Row::new("exec → selectable (first render)", "ms"),
        Row::new("peak RSS", "MB"),
    ];
    for _ in 0..runs {
        let (marks, rss) = run_gui_child(&[])?;
        let grab = |sub: &str| milestone(&marks, sub).map(|(v, _)| *v);
        let displays = grab("displays matched");
        let constructed = grab("overlay constructed");
        rows[0].push(grab("capture done").unwrap_or(f64::NAN));
        rows[1].push(grab("gpui run entered").unwrap_or(f64::NAN));
        rows[2].push(displays.unwrap_or(f64::NAN));
        if let (Some(d), Some(c)) = (displays, constructed) {
            rows[3].push(c - d);
        }
        rows[4].push(grab("overlay first render").unwrap_or(f64::NAN));
        if let Some(kb) = rss {
            rows[5].push(kb / 1024.0);
        }
    }
    if !json {
        print_rows(&rows);
    }
    Ok(rows_to_json(&rows))
}

/// Steady-state screencopy: each run opens its own wayland connection,
/// freezes every output and reads the frames back
fn suite_capture(runs: usize, json: bool) -> anyhow::Result<serde_json::Value> {
    info(
        json,
        &format!("[perf] capture: {runs} runs (steady state, first connect excluded)…"),
    );
    let caps = crate::platform::capture::capture_all_outputs()?;
    let screens = caps.len();
    drop(caps);

    let mut row = Row::new(&format!("freeze {screens} screen(s)"), "ms");
    let mut rss = Row::new("RSS after run", "MB");
    for _ in 0..runs {
        // spacing so the compositor has fresh frames to copy
        std::thread::sleep(Duration::from_millis(120));
        let t = Instant::now();
        crate::platform::capture::capture_all_outputs()?;
        row.push(t.elapsed().as_secs_f64() * 1e3);
        // captures are dropped between runs; what stays resident shows
        // the allocator's high-water behavior under repeated freezes
        if let Some(kb) = status_kb("VmRSS") {
            rss.push(kb / 1024.0);
        }
    }
    let rows = [row, rss];
    if !json {
        print_rows(&rows);
    }
    Ok(rows_to_json(&rows))
}

/// Export pipeline on live screen content: crop + encode tiers + write
fn suite_export(runs: usize, json: bool) -> anyhow::Result<serde_json::Value> {
    info(
        json,
        &format!("[perf] export: {runs} runs on a live capture…"),
    );
    let caps = crate::platform::capture::capture_all_outputs()?;
    let cap = &caps[0];
    let (w, h) = (cap.width, cap.height);

    // a typical selection: 800×600 logical, offset from the corner
    let sel = Bounds {
        origin: point(px(200.), px(150.)),
        size: size(px(800.), px(600.)),
    };

    let mut full_bal = Row::new(&format!("encode balanced {w}x{h}"), "ms");
    let mut full_fast = Row::new("encode fast (same pixels)", "ms");
    let mut size_bal = Row::new("balanced size", "KB");
    let mut size_fast = Row::new("fast size", "KB");
    let mut crop_row = Row::new("crop 800x600 logical", "ms");
    let mut crop_bal = Row::new("encode balanced (crop)", "ms");
    let mut write_row = Row::new("fs write (crop png)", "ms");
    let mut rss = Row::new("RSS after run", "MB");

    for _ in 0..runs {
        let t = Instant::now();
        let bal = crate::model::export::encode_png(w, h, &cap.rgba)?;
        full_bal.push(t.elapsed().as_secs_f64() * 1e3);
        size_bal.push(bal.len() as f64 / 1024.0);

        let t = Instant::now();
        let fast = crate::model::export::encode_png_fast(w, h, &cap.rgba)?;
        full_fast.push(t.elapsed().as_secs_f64() * 1e3);
        size_fast.push(fast.len() as f64 / 1024.0);

        let t = Instant::now();
        let Some((cw, ch, cropped)) = crate::model::export::crop(&cap.rgba, w, h, sel, cap.scale)
        else {
            anyhow::bail!("crop produced nothing");
        };
        crop_row.push(t.elapsed().as_secs_f64() * 1e3);

        let t = Instant::now();
        let png = crate::model::export::encode_png(cw, ch, &cropped)?;
        crop_bal.push(t.elapsed().as_secs_f64() * 1e3);

        // write through a temp file so suites don't litter ~/Pictures
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("perf.png");
        let t = Instant::now();
        std::fs::write(&path, &png)?;
        write_row.push(t.elapsed().as_secs_f64() * 1e3);
        drop((bal, fast, cropped, png));

        if let Some(kb) = status_kb("VmRSS") {
            rss.push(kb / 1024.0);
        }
    }
    let rows = [
        full_bal, full_fast, size_bal, size_fast, crop_row, crop_bal, write_row, rss,
    ];
    if !json {
        print_rows(&rows);
    }
    Ok(rows_to_json(&rows))
}

/// One GUI process, two overlays: the 2nd skips the wgpu renderer init —
/// exactly what a resident process would pay per shot
fn suite_warm_window(json: bool) -> anyhow::Result<serde_json::Value> {
    info(
        json,
        "[perf] warm-window: 1st (cold) vs 2nd overlay in one process…",
    );
    let (marks, rss_kb) = run_gui_child(&[("SHOTORI_PERF_WARM", "1")])?;

    let grab_after = |sub: &str, after: &str| -> Option<f64> {
        let pos = marks.iter().position(|(_, l)| l.contains(after))?;
        marks[pos + 1..]
            .iter()
            .find(|(_, l)| l.contains(sub))
            .map(|(v, _)| *v)
    };
    let displays = milestone(&marks, "displays matched").map(|(v, _)| *v);
    let cold_constructed = milestone(&marks, "overlay constructed").map(|(v, _)| *v);
    let cold_render = milestone(&marks, "overlay first render").map(|(v, _)| *v);
    let warm_requested = milestone(&marks, "warm reopen requested").map(|(v, _)| *v);
    let warm_constructed = grab_after("overlay constructed", "warm reopen requested");
    let warm_render = grab_after("overlay first render", "warm reopen requested");

    let mut rows = vec![
        Row::new("1st overlay: window creation (cold)", "ms"),
        Row::new("1st overlay: exec → selectable", "ms"),
        Row::new("2nd overlay: window creation (warm)", "ms"),
        Row::new("2nd overlay: reopen → selectable", "ms"),
        Row::new("resident-mode saving (creation)", "ms"),
        Row::new("peak RSS (whole session)", "MB"),
    ];
    if let (Some(d), Some(c), Some(r)) = (displays, cold_constructed, cold_render) {
        rows[0].push(c - d);
        rows[1].push(r);
    }
    if let (Some(q), Some(c)) = (warm_requested, warm_constructed) {
        rows[2].push(c - q);
    }
    if let Some(r) = warm_render
        && let Some(q) = warm_requested
    {
        rows[3].push(r - q);
    }
    if let (Some(cold), Some(warm)) = (
        rows[0].values.first().copied(),
        rows[2].values.first().copied(),
    ) {
        rows[4].push(cold - warm);
    }
    if let Some(kb) = rss_kb {
        rows[5].push(kb / 1024.0);
    }
    if !json {
        print_rows(&rows);
    }
    Ok(rows_to_json(&rows))
}

/// The tray is a launcher: it holds a StatusNotifierItem and spawns a
/// fresh gui process per shot (see tray.rs) — no wgpu, no captures. This
/// suite measures what such a resident launcher holds while idle, i.e.
/// the standing cost of today's tray architecture.
fn suite_resident(json: bool) -> anyhow::Result<serde_json::Value> {
    info(json, "[perf] resident: idle tray launcher footprint…");
    let exe = std::env::current_exe()?;
    let mut child = Command::new(&exe)
        .arg("tray")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    // let the icon register and allocations settle
    std::thread::sleep(Duration::from_millis(3000));
    if let Some(status) = child.try_wait()? {
        anyhow::bail!("tray exited early ({status}) — is a StatusNotifierItem host running?");
    }
    let status = std::fs::read_to_string(format!("/proc/{}/status", child.id()))?;
    let get = |field: &str| {
        status
            .lines()
            .find(|l| l.starts_with(field))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
    };

    let _ = child.kill();
    child.wait()?;

    let mut rows = vec![
        Row::new("idle tray launcher RSS", "MB"),
        Row::new("peak RSS since start", "MB"),
    ];
    if let Some(kb) = get("VmRSS") {
        rows[0].push(kb / 1024.0);
    }
    if let Some(kb) = get("VmHWM") {
        rows[1].push(kb / 1024.0);
    }
    if !json {
        print_rows(&rows);
    }
    Ok(rows_to_json(&rows))
}

/// Entry: `shotori --perf [suite...] [--runs N] [--json]`
pub fn perf_main() -> i32 {
    let mut suites: Vec<String> = Vec::new();
    let mut json = false;
    let mut runs = 5usize;
    let mut args = std::env::args().skip(2).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--runs" | "-n" => match args.peek().and_then(|v| v.parse().ok()) {
                Some(n) => {
                    args.next();
                    runs = n;
                }
                None => {
                    eprintln!("[perf] --runs needs a number");
                    return 2;
                }
            },
            "--help" | "-h" => {
                println!(
                    "usage: shotori --perf [suite...] [--runs N] [--json]\n\
                     suites: {} (default: all)",
                    SUITES.join(" | ")
                );
                return 0;
            }
            s if SUITES.contains(&s) => suites.push(a),
            other => {
                eprintln!("[perf] unknown argument '{other}' (--help for usage)");
                return 2;
            }
        }
    }
    if suites.is_empty() {
        suites = SUITES.iter().map(|s| s.to_string()).collect();
    }

    let t = Instant::now();
    let mut report = serde_json::json!({});

    // The measured binary itself: size is a first-class perf metric, and
    // the path says exactly which build these numbers apply to
    let exe = std::env::current_exe().ok();
    let exe_bytes = exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    if let (Some(p), Some(bytes)) = (&exe, exe_bytes) {
        if !json {
            println!(
                "[perf] binary: {:.1} MB  ({})",
                bytes as f64 / (1024.0 * 1024.0),
                p.display()
            );
        }
        report["binary"] = serde_json::json!({ "path": p, "bytes": bytes });
    }

    for suite in &suites {
        println!();
        let started = Instant::now();
        let result = match suite.as_str() {
            "startup" => suite_startup(runs, json),
            "capture" => suite_capture(runs, json),
            "export" => suite_export(runs, json),
            "warm-window" => suite_warm_window(json),
            "resident" => suite_resident(json),
            other => unreachable!("validated above: {other}"),
        };
        match result {
            Ok(v) => report[suite.as_str()] = v,
            Err(e) => {
                eprintln!("[perf] suite {suite} failed: {e:#}");
                return 1;
            }
        }
        if !json {
            println!(
                "  (suite {suite}: {:.1} s)",
                started.elapsed().as_secs_f64()
            );
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
    if !json {
        println!(
            "\n[perf] {} suite(s) in {:.1} s",
            suites.len(),
            t.elapsed().as_secs_f64()
        );
    }
    0
}
