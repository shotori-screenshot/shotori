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
use std::time::Duration;

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

/// Initialize cached models without starting an unconfirmed download.
fn init_engine() -> anyhow::Result<Mutex<rapidocr_core::RapidOcr>> {
    let dir = model_dir()?;
    anyhow::ensure!(
        assets_missing(&dir) == 0,
        "OCR models are missing; reopen OCR setup to download them"
    );
    let progress = DownloadProgress::default();
    let _lock = cache_lock(&dir, &progress)?;
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
fn cache_lock(dir: &Path, progress: &DownloadProgress) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".download.lock"))?;
    loop {
        check_cancelled(progress)?;
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
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
        .timeout(Duration::from_secs(10))
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
    let _lock = cache_lock(dir, progress)?;
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

#[cfg(test)]
mod tests {
    use super::{
        DownloadProgress, PPOCRV6_SMALL, asset_name, assets_needed_for, cache_lock,
        download_models_in, install_download,
    };
    use crate::model::settings::OcrModel;
    use std::io::{self, Read};
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
        let first = cache_lock(dir.path(), &DownloadProgress::default()).unwrap();
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
            tx.send(cache_lock(&path, &worker_progress).map(|_| ()))
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
}
