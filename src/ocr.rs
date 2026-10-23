//! # OCR: selection → configurable PP-OCR model set (rapidocr-core +
//! ONNX Runtime) → text
//!
//! Which model set runs (and whether the orientation-classifier stage
//! is skipped) comes from `settings.toml`'s `[ocr]` section — see
//! [`crate::model::settings`]. The overlay is a run-and-exit process
//! that reads settings once at startup, so the engine cache never sees
//! a mid-process model switch; the settings window never runs OCR.
//!
//! Engine loading pre-warms cached models in the background. Missing models
//! are downloaded only through the setup workflow, with progress and cancellation.
//! Downloads and cache repair share a cross-process lock; verified files are
//! installed atomically, and initialization failures are never cached.
//!

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use rapidocr_core::config::PipelineConfig;
use rapidocr_core::model::{self, ModelAssetSpec, ModelSetSpec, PPOCRV6_SMALL};

use crate::model::settings::{self, OcrModel};

/// Resolve the user's model-set choice to rapidocr's registry entry.
/// The settings loader already degrades unknown ids to the default, so
/// the fallback here only covers a non-initialized [`Settings`]
/// (`--no-config` tests, unit tests): the factory default, same thing.
fn selected_set() -> &'static ModelSetSpec {
    let id = settings::current().ocr_model().id();
    model::model_set_by_name(id).unwrap_or(&PPOCRV6_SMALL)
}

/// Which pipeline stages run: "assume upright" (settings) skips the
/// orientation classifier — screenshot text is almost always upright,
/// and the stage's model file is then never loaded or downloaded.
fn selected_pipeline() -> PipelineConfig {
    if settings::current().ocr_assume_upright() {
        PipelineConfig::without_cls()
    } else {
        PipelineConfig::full()
    }
}

/// The files the current settings combination actually needs, in
/// download/progress order.
fn selected_assets() -> Vec<ModelAssetSpec> {
    selected_set().assets_for_pipeline(selected_pipeline())
}

/// Explicit variant of [`selected_assets`] for callers that preview a
/// not-yet-committed choice (the settings page's readiness chips):
/// "would this model + this toggle run without a download?"
pub fn assets_needed_for(model: OcrModel, assume_upright: bool) -> Vec<ModelAssetSpec> {
    let pipeline = if assume_upright {
        PipelineConfig::without_cls()
    } else {
        PipelineConfig::full()
    };
    match model::model_set_by_name(model.id()) {
        Some(set) => set.assets_for_pipeline(pipeline),
        None => Vec::new(),
    }
}

/// Model cache directory (XDG-aware)
fn model_dir() -> anyhow::Result<PathBuf> {
    let base = if let Ok(d) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(d)
    } else {
        let home = std::env::var("HOME").context("HOME environment variable is not set")?;
        PathBuf::from(home).join(".local/share")
    };
    Ok(base.join("shotori/ocr-models"))
}

/// Make sure the cache directory exists and return it — the settings
/// page's "open" button runs before a first download, and xdg-open on
/// a missing directory either fails or opens something else entirely.
pub fn open_model_dir() -> anyhow::Result<PathBuf> {
    let dir = model_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// The user's custom rapidocr config (`[ocr] custom_config`), with
/// `~/` expanded against `$HOME` — hand-edited settings naturally
/// write `~`, but no library involved expands it.
/// The configured custom-config path, normalized: `~`/`~/…` expand
/// against `$HOME`, and a bare relative value resolves against `$HOME`
/// too — a desktop-launched app's CWD is arbitrary (see the rebase
/// comment below), so a CWD-relative reading would point nowhere.
pub fn custom_config() -> Option<PathBuf> {
    let path = expand_tilde(settings::current().ocr_custom_config()?)?;
    // A bare relative value resolves against $HOME (a desktop-launched
    // app's CWD is arbitrary, see the rebase comment below) — but a
    // `~name` value is another user's home: leave it unjoined so the
    // engine's error names the mistake, matching the rebase rule.
    let tildeish = path.to_str().is_some_and(|s| s.starts_with('~'));
    if path.is_relative() && !tildeish {
        return Some(match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(path),
            None => path, // no HOME to anchor to; the engine's error will name it
        });
    }
    Some(path)
}

/// `~` and `~/…` resolve against `$HOME`; `Some` carries the expanded
/// path, `None` means "not a tilde path we own" (absolute, or `~name`
/// — another user's home, which we cannot know and must not mangle
/// into `$HOME/name`). Callers that just want a path use
/// [`expand_tilde`], which passes non-tilde shapes through.
fn tilde_path(raw: &str) -> Option<PathBuf> {
    let rest = match raw.strip_prefix('~')? {
        "" => return std::env::var_os("HOME").map(PathBuf::from),
        rest => rest,
    };
    let rest = rest.strip_prefix('/')?; // `~name` is another user's home — not ours
    // `~//x` must not fall through with an absolute remainder: pushing
    // that onto HOME would REPLACE the buffer (HOME//x → /x).
    let rest = rest.trim_start_matches('/');
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(rest);
    Some(path)
}

/// A missing `$HOME` leaves a tilde path unexpanded (the engine's
/// error then names it — more actionable than silently dropping the
/// feature, which would fall back to the registry models the user
/// opted out of).
fn expand_tilde(raw: &str) -> Option<PathBuf> {
    Some(tilde_path(raw).unwrap_or_else(|| PathBuf::from(raw)))
}

/// Load and normalize a custom rapidocr config:
/// - relative model paths resolve against the config file's directory
///   (the process CWD of a desktop-launched app is arbitrary, so a
///   bare relative path would otherwise point nowhere);
/// - the upright-assume toggle still applies (it can only turn the
///   classifier OFF — a config that already skipped it stays skipped).
///
/// Pure in its arguments so it stays unit-testable without touching
/// the process-wide settings.
fn load_custom_config(
    path: &Path,
    assume_upright: bool,
) -> anyhow::Result<rapidocr_core::config::RapidOcrConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading custom OCR config {}", path.display()))?;
    let mut cfg = rapidocr_core::config::RapidOcrConfig::from_toml_str(&text)
        .with_context(|| format!("parsing custom OCR config {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let rebase = |p: &mut PathBuf| {
        // `~`/`~/…` expand against $HOME first (the docs promise this
        // for model paths too); `~name` is returned untouched by
        // tilde_path and must not fall through to the config-dir join,
        // which would bury the typo as `<config dir>/~name/…`.
        if let Some(raw) = p.to_str()
            && raw.starts_with('~')
        {
            if let Some(home_path) = tilde_path(raw) {
                *p = home_path;
            }
            return;
        }
        if p.is_relative() {
            *p = base.join(&*p);
        }
    };
    if let Some(det) = cfg.det.as_mut() {
        rebase(&mut det.model_path);
    }
    if let Some(cls) = cfg.cls.as_mut() {
        rebase(&mut cls.model_path);
    }
    if let Some(rec) = cfg.rec.as_mut() {
        rebase(&mut rec.model_path);
        rebase(&mut rec.dict_path);
    }
    if assume_upright {
        cfg.pipeline.use_cls = false;
    }
    Ok(cfg)
}

/// Validate the custom config the way the settings page's readiness
/// chip shows it: file parses, every enabled stage's model files
/// exist. `upright` comes from the caller (the page passes its live
/// toggle, not the startup global — `persist()` never reinstalls the
/// global, so a mid-session toggle would otherwise leave the chip
/// stale). `None` = no custom config set; `Some(Err)` carries the
/// first problem found (parse errors beat file checks — an
/// unparseable config's paths are not trustworthy).
pub fn custom_config_status(assume_upright: bool) -> Option<Result<PathBuf, String>> {
    let path = custom_config()?;
    cached_custom_status(path, assume_upright)
}

/// Last (path, mtime, upright) verdict of [`custom_config_status_for`].
type StatusCacheEntry = (
    PathBuf,
    Option<std::time::SystemTime>,
    bool,
    Result<PathBuf, String>,
);

/// `custom_config_status_for` behind a (path, mtime, upright) cache:
/// `page_ocr` renders on every settings-window interaction, and a
/// full TOML parse + up to four stats per frame is wasteful. A
/// changed mtime, different path or flipped toggle re-validates.
fn cached_custom_status(path: PathBuf, assume_upright: bool) -> Option<Result<PathBuf, String>> {
    static CACHE: std::sync::Mutex<Option<StatusCacheEntry>> = std::sync::Mutex::new(None);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    // A poisoned lock only means "recompute"; losing the cache is
    // never worth panicking over.
    let Ok(mut slot) = CACHE.lock() else {
        return custom_config_status_for(&path, assume_upright);
    };
    if let Some((cached_path, cached_mtime, cached_upright, cached)) = slot.as_ref()
        && *cached_path == path
        && *cached_mtime == mtime
        && *cached_upright == assume_upright
    {
        return Some(cached.clone());
    }
    let fresh = custom_config_status_for(&path, assume_upright)?;
    *slot = Some((path, mtime, assume_upright, fresh.clone()));
    Some(fresh)
}

/// [`custom_config_status`]'s uncached core — the pure shape the
/// tests drive. Only enabled stages are checked: rapidocr skips
/// disabled stages entirely (their files never get opened), so a
/// stale `[det]` section under `use_det = false` must not flag a
/// working config broken.
fn custom_config_status_for(path: &Path, assume_upright: bool) -> Option<Result<PathBuf, String>> {
    let cfg = match load_custom_config(path, assume_upright) {
        Ok(cfg) => cfg,
        Err(e) => return Some(Err(format!("{e:#}"))),
    };
    let mut paths = Vec::new();
    if cfg.pipeline.use_det
        && let Some(det) = &cfg.det
    {
        paths.push(&det.model_path);
    }
    if cfg.pipeline.use_cls
        && let Some(cls) = &cfg.cls
    {
        paths.push(&cls.model_path);
    }
    if cfg.pipeline.use_rec
        && let Some(rec) = &cfg.rec
    {
        paths.push(&rec.model_path);
        paths.push(&rec.dict_path);
    }
    for p in paths {
        if !p.is_file() {
            return Some(Err(format!("missing model file: {}", p.display())));
        }
    }
    Some(Ok(path.to_path_buf()))
}

/// Initialize cached models without starting an unconfirmed download.
fn init_engine() -> anyhow::Result<Mutex<rapidocr_core::RapidOcr>> {
    // Custom config: the user owns the files — no registry assets, no
    // download lock, no checksum repair. Errors carry the config path
    // and surface through the OCR-failed notification and the
    // settings page's readiness chip.
    if let Some(path) = custom_config() {
        let upright = settings::current().ocr_assume_upright();
        let cfg = load_custom_config(&path, upright)?;
        return rapidocr_core::RapidOcr::new(cfg)
            .map(Mutex::new)
            .context("custom OCR engine init failed; check the config in settings → OCR");
    }
    let dir = model_dir()?;
    anyhow::ensure!(
        assets_missing(&dir) == 0,
        "OCR models are missing; reopen OCR setup to download them"
    );
    let progress = DownloadProgress::default();
    // Bounded wait: this progress has no cancel flag (no UI owns it), so an
    // unbounded poll would hang the OCR action forever if another process's
    // downloader is wedged. 30 s covers a finishing download; a retry after
    // the clear error is cheap.
    let _lock = cache_lock(&dir, &progress, Some(Duration::from_secs(30)))?;
    anyhow::ensure!(
        assets_missing(&dir) == 0,
        "OCR models are missing; reopen OCR setup to download them"
    );

    let set = selected_set();
    let mut cfg = set.config(&dir);
    // `config()` always builds the full pipeline; the upright-assume
    // toggle removes the classifier stage here (RapidOcr::new skips
    // loading the cls model entirely when use_cls is false).
    cfg.pipeline = selected_pipeline();
    match rapidocr_core::RapidOcr::new(cfg) {
        Ok(eng) => Ok(Mutex::new(eng)),
        Err(e) => {
            // A runtime/session error is not proof that every model is corrupt.
            // Remove only files with a confirmed checksum mismatch, under the
            // same lock used by installers. The next OCR action offers setup.
            // The whole SET is checked (not just the pipeline's files): a
            // corrupt cls model only matters once the toggle goes off, but
            // repairing it now spares that future run the failure.
            for asset in set.assets() {
                let path = dir.join(asset.filename);
                if let Some(expected) = asset.sha256
                    && sha256_file(&path).is_ok_and(|actual| actual != expected)
                {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("removing corrupt model {}", path.display()))?;
                }
            }
            Err(e.context("OCR engine init failed; retry OCR to repair missing models"))
        }
    }
}

/// How many of the needed model files are absent from `dir` (the
/// pipeline's files — an upright-assume user is not blocked by a
/// missing classifier they will never run).
fn assets_missing(dir: &std::path::Path) -> usize {
    selected_assets()
        .iter()
        .filter(|a| !dir.join(a.filename).exists())
        .count()
}

/// Whether any model files are missing (cheap existence check; used to
/// decide between "run OCR" and "show the setup dialog")
pub fn models_missing() -> bool {
    // Custom config: never route to the download dialog — it installs
    // registry models the user explicitly opted out of. A broken
    // custom config fails at engine init instead, and the settings
    // page's readiness chip catches it at edit time.
    if custom_config().is_some() {
        return false;
    }
    model_dir().map(|d| assets_missing(&d) > 0).unwrap_or(true)
}

/// Whether every file `model` + `assume_upright` would need is already
/// cached (the settings page's per-row readiness chip). Shares and
/// files between sets are naturally accounted for — the check is per
/// FILE, and sets with distinct variants reuse each other's assets.
pub fn set_ready(model: OcrModel, assume_upright: bool) -> bool {
    let Ok(dir) = model_dir() else {
        return false;
    };
    let needed = assets_needed_for(model, assume_upright);
    !needed.is_empty() && needed.iter().all(|a| dir.join(a.filename).is_file())
}

/// Human-readable model cache path (shown in the setup dialog)
pub fn model_dir_display() -> String {
    model_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into())
}

/// Filename of the 1-based asset `idx` (progress readout).
/// Zero means the download has not started; invalid indices return `?`.
pub fn asset_name(idx: u8) -> &'static str {
    let Some(index) = usize::from(idx).checked_sub(1) else {
        return "?";
    };
    selected_assets()
        .get(index)
        .map(|a| a.filename)
        .unwrap_or("?")
}

// ── First-run download with progress + cancellation ───────────────────

/// Shared state between the download thread and the setup UI.
/// All fields are lock-free; the UI polls at ~80ms.
pub struct DownloadProgress {
    /// Set by the UI to abort; checked between chunks
    pub cancel: AtomicBool,
    /// Bytes written for the current file
    pub bytes: AtomicU64,
    /// Content length of the current file (zero when unknown)
    pub total: AtomicU64,
    /// 1-based index of the file currently downloading
    pub file_idx: AtomicU8,
    /// Bytes already installed from earlier files in the set — the UI
    /// adds the current file's `bytes` on top for a set-wide total
    pub done_bytes: AtomicU64,
    /// Number of files in the set
    pub file_count: u8,
    status: AtomicU8, // 0 running, 1 ok, 2 failed
    error: Mutex<String>,
}

impl Default for DownloadProgress {
    /// file_count is UNKNOWN here — `Default` only serves lock/cancel
    /// holders that never render progress (`init_engine`, tests). The
    /// UI must construct through [`DownloadProgress::for_selected_set`].
    fn default() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
            total: AtomicU64::new(0),
            file_idx: AtomicU8::new(0),
            done_bytes: AtomicU64::new(0),
            file_count: 0,
            status: AtomicU8::new(0),
            error: Mutex::new(String::new()),
        }
    }
}

impl DownloadProgress {
    /// A progress carrier for the settings-selected model set — the
    /// one construction path that knows how many files will download.
    pub fn for_selected_set() -> Self {
        Self {
            file_count: selected_assets().len() as u8,
            ..Self::default()
        }
    }

    pub fn is_running(&self) -> bool {
        self.status.load(Ordering::Relaxed) == 0
    }
    pub fn finished_ok(&self) -> bool {
        self.status.load(Ordering::Relaxed) == 1
    }
    pub fn error(&self) -> String {
        self.error.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// Spawn the model download on its own thread (reqwest::blocking cannot run
/// in async contexts). Unique temporary files are removed on cancellation or
/// error; only complete, checksum-verified files become installed models.
pub fn spawn_download(progress: std::sync::Arc<DownloadProgress>) {
    // The thread gets its own Arc; the caller's (and the UI's) stays valid
    // for cancellation even if the thread fails to spawn
    let thread_progress = std::sync::Arc::clone(&progress);
    let spawn = std::thread::Builder::new()
        .name("shotori-ocr-download".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let result = download_models(&thread_progress);
            match result {
                Ok(()) => thread_progress.status.store(1, Ordering::Relaxed),
                Err(e) => {
                    if let Ok(mut err) = thread_progress.error.lock() {
                        *err = format!("{e:#}");
                    }
                    thread_progress.status.store(2, Ordering::Relaxed);
                }
            }
        });
    if let Err(e) = spawn {
        // Thread spawn failure must not leave the UI waiting forever
        if let Ok(mut err) = progress.error.lock() {
            *err = format!("failed to spawn download thread: {e:#}");
        }
        progress.status.store(2, Ordering::Relaxed);
    }
}

fn check_cancelled(progress: &DownloadProgress) -> anyhow::Result<()> {
    anyhow::ensure!(!progress.cancel.load(Ordering::Relaxed), "cancelled");
    Ok(())
}

/// Keep the lock file in place: unlinking it would let another process lock a
/// different inode. Dropping the handle releases the OS lock, including on exit.
/// `wait` bounds how long to poll before giving up — `None` waits until the
/// holder finishes or `progress.cancel` fires (the download UI path, whose
/// progress is real); a bounded wait is mandatory for callers whose progress
/// is a throwaway (`init_engine`), or a wedged downloader in another process
/// would hang the OCR action forever with no error surfaced.
fn cache_lock(
    dir: &Path,
    progress: &DownloadProgress,
    wait: Option<Duration>,
) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".download.lock"))?;
    let deadline = wait.map(|w| Instant::now() + w);
    loop {
        check_cancelled(progress)?;
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    anyhow::bail!(
                        "another shotori process holds the OCR model lock; try again when it finishes"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

fn download_models(progress: &DownloadProgress) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("shotori")
        .connect_timeout(Duration::from_secs(10))
        // Bound blocking network operations so a cancelled worker also exits
        // when a server stops sending data. The UI dismisses immediately.
        // reqwest 0.12's blocking builder has no read_timeout, so the bound
        // is a generous total (≥ 50 KB/s sustained for the ~31 MB set) — a
        // tighter cap killed slow-but-alive downloads mid-transfer; a wedged
        // server still cannot hang the worker past the cap, and the user can
        // always cancel from the dialog.
        .timeout(Duration::from_secs(600))
        .build()
        .context("building HTTP client")?;
    download_models_in(&model_dir()?, &selected_assets(), progress, &client)
}

fn download_models_in(
    dir: &Path,
    assets: &[ModelAssetSpec],
    progress: &DownloadProgress,
    client: &reqwest::blocking::Client,
) -> anyhow::Result<()> {
    let _lock = cache_lock(dir, progress, None)?;
    for (i, asset) in assets.iter().enumerate() {
        check_cancelled(progress)?;
        let dest = dir.join(asset.filename);
        if dest.is_file()
            && asset.sha256.map_or(Ok(true), |expected| {
                sha256_file(&dest).map(|actual| actual == expected)
            })?
        {
            continue;
        }
        progress.bytes.store(0, Ordering::Relaxed);
        progress.total.store(0, Ordering::Relaxed);
        progress.file_idx.store(i as u8 + 1, Ordering::Relaxed);
        let mut resp = client
            .get(asset.url)
            .send()
            .with_context(|| format!("fetching {}", asset.url))?
            .error_for_status()
            .with_context(|| format!("bad status for {}", asset.url))?;
        check_cancelled(progress)?;
        if let Some(len) = resp.content_length() {
            progress.total.store(len, Ordering::Relaxed);
        }
        install_download(&mut resp, &dest, asset.sha256, progress)?;
        // Set-wide running total: the UI shows it so a 5-file set doesn't
        // look stalled on file 1. `bytes` ends at this file's size on Ok.
        progress
            .done_bytes
            .fetch_add(progress.bytes.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    check_cancelled(progress)
}

/// RAII cleanup covers read/write/hash/install errors as well as cancellation.
fn install_download(
    reader: &mut impl std::io::Read,
    dest: &Path,
    expected_sha256: Option<&str>,
    progress: &DownloadProgress,
) -> anyhow::Result<()> {
    use std::io::Write as _;

    check_cancelled(progress)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".shotori-model-")
        .suffix(".part")
        .tempfile_in(dest.parent().context("model path has no parent")?)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        check_cancelled(progress)?;
        let n = reader.read(&mut buf).context("reading download chunk")?;
        // Cancellation may happen while read is blocked, including its EOF.
        check_cancelled(progress)?;
        if n == 0 {
            break;
        }
        temp.write_all(&buf[..n])
            .context("writing download chunk")?;
        progress.bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    temp.flush().context("flushing model download")?;
    if let Some(expected) = expected_sha256 {
        anyhow::ensure!(
            sha256_file(temp.path())? == expected,
            "sha256 mismatch for {}",
            dest.display()
        );
    }
    check_cancelled(progress)?;
    temp.persist(dest)
        .map_err(|error| error.error)
        .with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

fn sha256_file(path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    // Manual hex: sha2's output type lost its LowerHex impl in newer
    // patch releases (hybrid-array migration) — don't depend on it
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

static ENG: OnceLock<Mutex<rapidocr_core::RapidOcr>> = OnceLock::new();
/// Initialization mutex: concurrent callers initialize the engine only once
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the engine (lock-guarded). OCR is inherently serial: inference
/// runs on gpui's background thread pool, so lock contention only happens
/// when Ctrl+O is pressed in rapid succession (requests simply queue).
/// Initialization failures are not cached — OnceLock stays unset, so the
/// next call retries init.
fn engine() -> anyhow::Result<MutexGuard<'static, rapidocr_core::RapidOcr>> {
    // Fast path: already initialized
    if let Some(m) = ENG.get() {
        return m
            .lock()
            .map_err(|_| anyhow::anyhow!("OCR engine lock poisoned"));
    }
    // Slow path: hold the init lock → double-check → initialize
    let _g = INIT_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("OCR init lock poisoned"))?;
    if let Some(m) = ENG.get() {
        return m
            .lock()
            .map_err(|_| anyhow::anyhow!("OCR engine lock poisoned"));
    }
    let eng = init_engine()?;
    // Race backstop: a concurrent initializer may have set ENG first — ours
    // gets dropped, theirs is used (the two engines are equivalent)
    let _ = ENG.set(eng);
    ENG.get()
        .context("OCR engine vanished after init")?
        .lock()
        .map_err(|_| anyhow::anyhow!("OCR engine lock poisoned"))
}

/// Pre-initialize the engine in the background (no inference). The overlay
/// calls this on open so the ~1s init hides behind the user drawing their
/// selection. Only runs when models are already cached — a first-ever run
/// must not surprise the user with a 31MB download during a plain screenshot.
pub fn warmup() {
    // Custom config: warmable whenever the config file itself is
    // readable — the registry's missing-asset check does not apply
    // (its files may legitimately be absent).
    if let Some(path) = custom_config() {
        if path.is_file() {
            drop(engine());
        }
        return;
    }
    if let Ok(dir) = model_dir()
        && assets_missing(&dir) == 0
    {
        // We only want the init side effect; the guard is released
        // immediately (that's the point of a warmup)
        drop(engine());
    }
}

/// RGBA pixels → OCR text.
/// `w`, `h` are physical pixel dimensions (output of the crop).
pub fn run_ocr(rgba: &[u8], w: u32, h: u32) -> anyhow::Result<String> {
    // RGBA → RGB (drop the alpha channel)
    let rgb: Vec<u8> = rgba
        .chunks(4)
        .flat_map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect();
    let img = image::RgbImage::from_raw(w, h, rgb).context("RGBA→RGB conversion failed")?;

    let out = engine()?.run_image(&img).context("OCR inference failed")?;
    let text: String = out
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(text.trim().to_string())
}

// ── Result delivery (the background exit's tail) ───────────────────────

/// Deliver a finished OCR run: text → clipboard (resident daemon) →
/// notification. Every outcome notifies — the overlays are gone and the
/// OSD chip ([`crate::ui::ocr_osd`]) is non-interactive, so the
/// notification is the only feedback a keybinding user gets. Runs on the
/// background executor (the daemon handshake sleeps 100 ms); the OSD's
/// completion callback quits only after this returns.
pub fn deliver_text(w: u32, h: u32, result: anyhow::Result<String>) {
    match result {
        Ok(t) if t.trim().is_empty() => {
            crate::notify::send(
                crate::i18n::t!("notify.ocr.empty.title").as_ref(),
                crate::i18n::t!("notify.ocr.empty.body").as_ref(),
            );
        }
        Ok(t) => {
            if let Err(e) = crate::clipboard::copy_text(t.clone()) {
                eprintln!("[shotori] OCR copy failed: {e:#}");
                crate::notify::send(
                    crate::i18n::t!("notify.ocr.copy_failed.title").as_ref(),
                    crate::i18n::t!("notify.ocr.copy_failed.body").as_ref(),
                );
                return;
            }
            let lines = t.lines().count();
            let preview: String = t
                .replace('\n', " ")
                .chars()
                .filter(|c| !c.is_control())
                .take(60)
                .collect();
            println!(
                "[shotori] OCR done {w}x{h} → {lines} line(s) → clipboard (preview: {preview})"
            );
            // The full text is cached as a `.txt` so clicking the
            // notification reopens it in the default editor — the body
            // only carries a one-line preview.
            crate::notify::ocr_copied(&t, &preview);
        }
        Err(e) => {
            eprintln!("[shotori] OCR failed: {e:#}");
            // The notification is the only feedback surface for this
            // path, and the generic “try again” hid model problems
            // (corrupt cache, broken custom config) it can't fix —
            // lead with the first line of the error chain instead.
            let chain = format!("{e:#}");
            let first = chain.lines().next().unwrap_or(&chain);
            let cut: String = first.chars().take(120).collect();
            let reason = if cut.len() < first.len() {
                format!("{cut}…")
            } else {
                cut
            };
            crate::notify::send(
                crate::i18n::t!("notify.ocr.failed.title").as_ref(),
                crate::i18n::t!("notify.ocr.failed.body", reason = reason).as_ref(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DownloadProgress, PPOCRV6_SMALL, asset_name, assets_needed_for, cache_lock,
        download_models_in, install_download,
    };
    use crate::model::settings::OcrModel;
    use std::io::{self, Read};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn assert_no_partial_files(dir: &std::path::Path) {
        assert!(std::fs::read_dir(dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }

    #[test]
    fn initial_download_progress_has_no_asset_name() {
        let progress = DownloadProgress::default();
        assert_eq!(asset_name(progress.file_idx.load(Ordering::Relaxed)), "?");
    }

    #[test]
    fn asset_names_use_one_based_indices() {
        // tests run against the factory settings (no settings::init):
        // v6-small with the full pipeline — exactly PPOCRV6_SMALL's
        // four assets, cls included
        for (index, asset) in PPOCRV6_SMALL.assets().iter().enumerate() {
            assert_eq!(asset_name((index + 1) as u8), asset.filename);
        }
    }

    #[test]
    fn out_of_range_asset_index_has_no_name() {
        let index = u8::try_from(PPOCRV6_SMALL.assets().len() + 1).unwrap();
        assert_eq!(asset_name(index), "?");
    }

    #[test]
    fn assume_upright_drops_the_classifier_asset() {
        // the upright-assume toggle must remove the cls model from the
        // needed set — that is what makes it save a download
        let full = assets_needed_for(OcrModel::Ppocrv6Small, false);
        let upright = assets_needed_for(OcrModel::Ppocrv6Small, true);
        assert_eq!(full.len(), 4);
        assert_eq!(upright.len(), 3);
        assert!(full.iter().any(|a| a.filename.contains("cls")));
        assert!(!upright.iter().any(|a| a.filename.contains("cls")));
        // det/rec/dict survive the toggle
        for a in &upright {
            assert!(full.contains(a));
        }
    }

    #[test]
    fn tiny_also_drops_cls_when_upright() {
        // every curated set carries a classifier; the toggle must
        // remove it from any of them, not just the default
        assert_eq!(assets_needed_for(OcrModel::Ppocrv6Tiny, true).len(), 3);
        assert_eq!(assets_needed_for(OcrModel::Ppocrv6Tiny, false).len(), 4);
    }

    #[test]
    fn verified_download_replaces_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model");
        std::fs::write(&dest, b"corrupt").unwrap();
        install_download(
            &mut &b"abc"[..],
            &dest,
            Some(ABC_SHA256),
            &DownloadProgress::default(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
        assert_no_partial_files(dir.path());
    }

    #[test]
    fn checksum_failure_preserves_existing_file_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model");
        std::fs::write(&dest, b"existing").unwrap();
        let result = install_download(
            &mut &b"bad"[..],
            &dest,
            Some(ABC_SHA256),
            &DownloadProgress::default(),
        );

        assert!(result.unwrap_err().to_string().contains("sha256 mismatch"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        assert_no_partial_files(dir.path());
    }

    #[test]
    fn interrupted_read_cleans_temp_and_retry_succeeds() {
        struct Interrupted(bool);

        impl Read for Interrupted {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection lost",
                    ));
                }
                self.0 = true;
                buf[0] = b'a';

                Ok(1)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model");

        assert!(
            install_download(
                &mut Interrupted(false),
                &dest,
                Some(ABC_SHA256),
                &DownloadProgress::default()
            )
            .is_err()
        );
        assert!(!dest.exists());
        assert_no_partial_files(dir.path());

        install_download(
            &mut &b"abc"[..],
            &dest,
            Some(ABC_SHA256),
            &DownloadProgress::default(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
    }

    #[test]
    fn cancellation_at_eof_does_not_install_model() {
        struct CancelAtEof<'a>(&'a DownloadProgress);

        impl Read for CancelAtEof<'_> {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                self.0.cancel.store(true, Ordering::Relaxed);
                Ok(0)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model");
        let progress = DownloadProgress::default();
        let result = install_download(&mut CancelAtEof(&progress), &dest, None, &progress);

        assert_eq!(result.unwrap_err().to_string(), "cancelled");
        assert!(!dest.exists());
        assert_no_partial_files(dir.path());
    }

    #[test]
    fn failed_install_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model");
        std::fs::create_dir(&dest).unwrap();
        let error = install_download(
            &mut &b"abc"[..],
            &dest,
            Some(ABC_SHA256),
            &DownloadProgress::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("finalizing"));
        // Cleanup must happen even while the caller retains the error.
        assert_no_partial_files(dir.path());
    }

    #[test]
    fn cache_lock_serializes_handles_and_wait_is_cancellable() {
        let dir = tempfile::tempdir().unwrap();
        let first = cache_lock(dir.path(), &DownloadProgress::default(), None).unwrap();
        let other = std::fs::File::options()
            .read(true)
            .write(true)
            .open(dir.path().join(".download.lock"))
            .unwrap();

        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));

        let progress = Arc::new(DownloadProgress::default());
        let worker_progress = progress.clone();
        let path = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(cache_lock(&path, &worker_progress, None).map(|_| ()))
                .unwrap();
        });
        progress.cancel.store(true, Ordering::Relaxed);

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err()
                .to_string(),
            "cancelled"
        );

        worker.join().unwrap();
        drop(first);

        assert!(other.try_lock().is_ok());
    }

    #[test]
    fn verified_cached_model_needs_no_network() {
        let dir = tempfile::tempdir().unwrap();
        let mut asset = PPOCRV6_SMALL.assets()[0];

        asset.filename = "model";
        asset.url = "http://127.0.0.1:1/must-not-be-requested";
        asset.sha256 = Some(ABC_SHA256);
        std::fs::write(dir.path().join(asset.filename), b"abc").unwrap();

        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let progress = DownloadProgress::default();

        download_models_in(dir.path(), &[asset], &progress, &client).unwrap();
        assert_eq!(progress.bytes.load(Ordering::Relaxed), 0);
    }

    // ── Custom model config (`[ocr] custom_config`) ──────────────────

    /// A minimal valid rec-only rapidocr config (the smallest pipeline
    /// that validates) whose model paths are relative on purpose.
    fn write_rec_only_config(dir: &std::path::Path, extra: &str) -> std::path::PathBuf {
        std::fs::write(dir.join("rec.onnx"), b"onnx").unwrap();
        std::fs::write(dir.join("dict.txt"), b"a\n").unwrap();
        let path = dir.join("custom.toml");
        std::fs::write(
            &path,
            format!(
                "text_score = 0.5\nmin_side_len = 30\nmax_side_len = 2000\n\
                 min_height = 30\nwidth_height_ratio = 8.0\n\n\
                 [pipeline]\nuse_det = false\nuse_cls = false\nuse_rec = true\n\n\
                 [rec]\nmodel_path = \"rec.onnx\"\ndict_path = \"dict.txt\"\n\
                 image_shape = [3, 48, 320]\nbatch_size = 16\n{extra}"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn custom_config_rebases_relative_paths_against_the_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rec_only_config(dir.path(), "");

        let cfg = super::load_custom_config(&path, false).unwrap();
        let rec = cfg.rec.unwrap();
        assert_eq!(rec.model_path, dir.path().join("rec.onnx"));
        assert_eq!(rec.dict_path, dir.path().join("dict.txt"));
        // already-absolute paths must survive untouched
        let abs = dir.path().join("elsewhere.onnx");
        std::fs::write(&abs, b"onnx").unwrap();
        let text = std::fs::read_to_string(&path).unwrap().replace(
            "model_path = \"rec.onnx\"",
            &format!("model_path = \"{}\"", abs.display()),
        );
        std::fs::write(&path, text).unwrap();
        let cfg = super::load_custom_config(&path, false).unwrap();
        assert_eq!(cfg.rec.unwrap().model_path, abs);
    }

    #[test]
    fn custom_config_upright_toggle_only_disables_cls() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rec_only_config(dir.path(), "");

        // rec-only stays rec-only (the toggle must never re-enable cls
        // — there is no cls model to load)
        let cfg = super::load_custom_config(&path, true).unwrap();
        assert!(!cfg.pipeline.use_cls);
        assert!(!cfg.pipeline.use_det);
        assert!(cfg.pipeline.use_rec);

        // a full pipeline loses its classifier
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("use_det = false", "use_det = true")
            .replace("use_cls = false", "use_cls = true");
        // needs det+cls model files to validate
        std::fs::write(dir.path().join("det.onnx"), b"onnx").unwrap();
        std::fs::write(dir.path().join("cls.onnx"), b"onnx").unwrap();
        let text = format!(
            "{text}\n[det]\nmodel_path = \"det.onnx\"\nlimit_side_len = 960\n\
             limit_type = \"Max\"\nmean = [0.485, 0.456, 0.406]\n\
             std = [0.229, 0.224, 0.225]\nthresh = 0.3\nbox_thresh = 0.6\n\
             max_candidates = 1000\nunclip_ratio = 1.6\nmin_size = 3\n\n\
             [cls]\nmodel_path = \"cls.onnx\"\nimage_shape = [3, 48, 192]\n\
             batch_size = 6\nthresh = 0.9\nlabels = [\"0\", \"180\"]\n"
        );
        let full_path = dir.path().join("full.toml");
        std::fs::write(&full_path, text).unwrap();
        let cfg = super::load_custom_config(&full_path, true).unwrap();
        assert!(!cfg.pipeline.use_cls, "upright must drop the classifier");
        assert!(cfg.pipeline.use_det && cfg.pipeline.use_rec);
    }

    #[test]
    fn custom_config_status_reports_missing_model_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rec_only_config(dir.path(), "");
        std::fs::remove_file(dir.path().join("rec.onnx")).unwrap();

        let status = super::custom_config_status_for(&path, false);
        let err = status.unwrap().unwrap_err();
        assert!(err.contains("rec.onnx"), "names the missing file: {err}");

        std::fs::write(dir.path().join("rec.onnx"), b"onnx").unwrap();
        assert!(
            super::custom_config_status_for(&path, false)
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn custom_config_status_ignores_disabled_stages_files() {
        // A stale [det] section pointing at a deleted file must not
        // flag a rec-only config broken: rapidocr never opens files
        // of stages whose use_* flag is off — and assume_upright
        // turns cls off the same way, so a config carrying [cls]
        // without the file stays ready under the toggle.
        let dir = tempfile::tempdir().unwrap();
        let path = write_rec_only_config(
            dir.path(),
            "\n[det]\nmodel_path = \"gone.onnx\"\nlimit_side_len = 960\n\
             limit_type = \"Max\"\nmean = [0.485, 0.456, 0.406]\n\
             std = [0.229, 0.224, 0.225]\nthresh = 0.3\nbox_thresh = 0.6\n\
             max_candidates = 1000\nunclip_ratio = 1.6\nmin_size = 3\n",
        );
        assert!(
            super::custom_config_status_for(&path, false)
                .unwrap()
                .is_ok(),
            "disabled det must not require its file"
        );

        // the same section enabled for real does require it
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("use_det = false", "use_det = true");
        std::fs::write(&path, text).unwrap();
        let err = super::custom_config_status_for(&path, false)
            .unwrap()
            .unwrap_err();
        assert!(err.contains("gone.onnx"), "names the missing file: {err}");
    }

    #[test]
    fn custom_config_status_reports_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "not = a = valid = config").unwrap();
        let err = super::custom_config_status_for(&path, false)
            .unwrap()
            .unwrap_err();
        assert!(
            err.contains("broken.toml"),
            "carries the config path: {err}"
        );
    }

    #[test]
    fn tilde_expansion_resolves_against_home() {
        let home = std::env::var_os("HOME").expect("tests run with HOME set");
        let mut expected = PathBuf::from(&home);
        expected.push("models/ocr.toml");
        assert_eq!(
            super::expand_tilde("~/models/ocr.toml"),
            Some(expected.clone())
        );
        // bare ~ is HOME itself
        assert_eq!(super::expand_tilde("~"), Some(PathBuf::from(&home)));
        // absolute paths pass through untouched
        assert_eq!(
            super::expand_tilde("/etc/ocr.toml"),
            Some(PathBuf::from("/etc/ocr.toml"))
        );
        // ~name is another user's home — never mangled into $HOME/name
        assert_eq!(
            super::expand_tilde("~other/ocr.toml"),
            Some(PathBuf::from("~other/ocr.toml"))
        );
    }

    #[test]
    fn custom_config_rebase_expands_tilde_model_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = std::env::var_os("HOME").expect("tests run with HOME set");
        std::fs::write(dir.path().join("rec.onnx"), b"onnx").unwrap();
        std::fs::write(dir.path().join("dict.txt"), b"a\n").unwrap();
        let path = dir.path().join("custom.toml");
        std::fs::write(
            &path,
            "text_score = 0.5\nmin_side_len = 30\nmax_side_len = 2000\n\
             min_height = 30\nwidth_height_ratio = 8.0\n\n\
             [pipeline]\nuse_det = false\nuse_cls = false\nuse_rec = true\n\n\
             [rec]\nmodel_path = \"~/rec.onnx\"\ndict_path = \"dict.txt\"\n\
             image_shape = [3, 48, 320]\nbatch_size = 16\n",
        )
        .unwrap();
        let mut expected = PathBuf::from(&home);
        expected.push("rec.onnx");
        let cfg = super::load_custom_config(&path, false).unwrap();
        assert_eq!(cfg.rec.as_ref().unwrap().model_path, expected);
        // the non-tilde dict path still rebases against the config dir
        assert_eq!(
            cfg.rec.as_ref().unwrap().dict_path,
            dir.path().join("dict.txt")
        );

        // `~name` (another user's home) survives the rebase untouched —
        // the early return must not bury it as <config dir>/~other/…
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\"~/rec.onnx\"", "\"~other/rec.onnx\"");
        std::fs::write(&path, text).unwrap();
        let cfg = super::load_custom_config(&path, false).unwrap();
        assert_eq!(
            cfg.rec.as_ref().unwrap().model_path,
            PathBuf::from("~other/rec.onnx")
        );
    }
}
