//! Persistent bounded floating-pin history (`$XDG_DATA_HOME/shotori/pin-history`).
//!
//! Stores lossless PNG snapshots alongside an atomic `index.json` manifest so
//! closed pins can be restored across processes (`shotori pin --restore`,
//! `Ctrl+Shift+T`, tray menu) and browsed in the Pin History window
//! (`shotori pin --history`).
//!
//! Resilience contract (matches `Settings::load`):
//! - Missing directory or `index.json` yields an empty list with no error.
//! - Corrupt `index.json` degrades to an empty list with a stderr warning
//!   instead of bricking pin creation or startup.
//! - Entries whose `.png` payload was externally deleted are skipped cleanly.
//! - Every write uses a temporary sibling file + `fs::rename` so concurrent
//!   readers never observe a half-written manifest or truncated PNG.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use gpui_kit::{Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};

const INDEX_FILE: &str = "index.json";
/// Environment override used by isolated headless/E2E runs and tests.
pub const HISTORY_DIR_ENV: &str = "SHOTORI_PIN_HISTORY_DIR";

static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);

/// Input bundle when recording a newly created floating pin.
pub struct NewPinRecord<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
    pub rect: Bounds<Pixels>,
    pub opacity: u8,
    pub label: Option<String>,
}

/// Metadata for one persisted pin snapshot (ordered newest-first in `index.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinHistoryEntry {
    pub id: String,
    pub created_at_ms: i64,
    pub width: u32,
    pub height: u32,
    /// Desktop-global logical `[x, y, width, height]` at creation or last close.
    pub rect: [f32; 4],
    #[serde(default = "default_opacity")]
    pub opacity: u8,
    pub png_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

const fn default_opacity() -> u8 {
    100
}

impl PinHistoryEntry {
    pub fn logical_bounds(&self) -> Bounds<Pixels> {
        let [x, y, w, h] = self.rect;
        let clean = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        Bounds::new(
            point(px(clean(x, 0.)), px(clean(y, 0.))),
            size(
                px(clean(w, self.width.max(1) as f32).max(1.)),
                px(clean(h, self.height.max(1) as f32).max(1.)),
            ),
        )
    }

    pub fn formatted_timestamp(&self) -> String {
        use chrono::{Local, TimeZone as _};
        Local
            .timestamp_millis_opt(self.created_at_ms)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "—".to_owned())
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct HistoryManifest {
    version: u32,
    entries: Vec<PinHistoryEntry>,
}

/// Bounded directory-backed store of pinned image snapshots.
#[derive(Clone, Debug)]
pub struct PinHistoryStore {
    dir: PathBuf,
}

impl PinHistoryStore {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn default_store() -> anyhow::Result<Self> {
        Ok(Self::at(default_history_dir()?))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn png_path(&self, entry: &PinHistoryEntry) -> PathBuf {
        // Prevent path traversal if `index.json` was hand-edited.
        let file_name = Path::new(&entry.png_file)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("invalid.png"));
        self.dir.join(file_name)
    }

    /// List valid history entries (newest first), skipping any whose PNG was removed.
    pub fn entries(&self) -> Vec<PinHistoryEntry> {
        self.read_manifest()
            .entries
            .into_iter()
            .filter(|e| {
                e.width > 0 && e.height > 0 && !e.png_file.is_empty() && self.png_path(e).is_file()
            })
            .collect()
    }

    /// Most recently recorded valid history entry.
    pub fn latest(&self) -> Option<PinHistoryEntry> {
        self.entries().into_iter().next()
    }

    /// Persist a newly created pin when `max_entries > 0`.
    pub fn record(
        &self,
        rec: NewPinRecord<'_>,
        max_entries: u16,
    ) -> anyhow::Result<Option<PinHistoryEntry>> {
        let NewPinRecord {
            width,
            height,
            rgba,
            rect,
            opacity,
            label,
        } = rec;
        if max_entries == 0 {
            return Ok(None);
        }
        anyhow::ensure!(width > 0 && height > 0, "empty pin dimensions");
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .context("pin dimensions overflow")?;
        anyhow::ensure!(rgba.len() == expected, "invalid RGBA byte length");

        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        let id = format!("{now_ms}-{}-{seq:04}", std::process::id());
        let png_file = format!("{id}.png");
        let png_bytes = crate::model::export::encode_png_fast(width, height, rgba)?;
        write_atomic(&self.dir.join(&png_file), &png_bytes)?;

        let entry = PinHistoryEntry {
            id,
            created_at_ms: now_ms,
            width,
            height,
            rect: bounds_to_array(rect),
            opacity: opacity.clamp(20, 100),
            png_file,
            label: label.and_then(|s| {
                let t = s.trim();
                (!t.is_empty()).then(|| truncate_chars(t, 80))
            }),
        };

        let mut entries = self.entries();
        entries.insert(0, entry.clone());
        self.prune_and_save(&mut entries, usize::from(max_entries))?;
        Ok(Some(entry))
    }

    /// Update an existing entry's geometry, opacity, and optionally its raster
    /// (e.g. when a pin was moved, zoomed, rotated, or annotated before closing).
    pub fn update_entry(
        &self,
        id: &str,
        rect: Bounds<Pixels>,
        opacity: u8,
        updated_raster: Option<(u32, u32, &[u8])>,
    ) -> anyhow::Result<bool> {
        let mut entries = self.entries();
        let Some(pos) = entries.iter().position(|e| e.id == id) else {
            return Ok(false);
        };
        if let Some((w, h, rgba)) = updated_raster
            && w > 0
            && h > 0
        {
            let png = crate::model::export::encode_png_fast(w, h, rgba)?;
            let path = self.png_path(&entries[pos]);
            write_atomic(&path, &png)?;
            entries[pos].width = w;
            entries[pos].height = h;
        }
        entries[pos].rect = bounds_to_array(rect);
        entries[pos].opacity = opacity.clamp(20, 100);
        self.write_manifest(&entries)?;
        Ok(true)
    }

    /// Decode `(entry, rgba8)` for one stored pin by ID.
    pub fn load_entry(&self, id: &str) -> anyhow::Result<(PinHistoryEntry, Vec<u8>)> {
        let entry = self
            .entries()
            .into_iter()
            .find(|e| e.id == id)
            .with_context(|| format!("pin history entry '{id}' not found"))?;
        let path = self.png_path(&entry);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .with_context(|| format!("decoding {}", path.display()))?
            .into_rgba8();
        let (w, h) = decoded.dimensions();
        let mut resolved = entry;
        resolved.width = w;
        resolved.height = h;
        Ok((resolved, decoded.into_raw()))
    }

    /// Delete a single history entry and its backing PNG file.
    pub fn remove(&self, id: &str) -> anyhow::Result<bool> {
        let mut manifest = self.read_manifest();
        let before = manifest.entries.len();
        let mut removed_files = Vec::new();
        manifest.entries.retain(|e| {
            if e.id == id {
                removed_files.push(self.png_path(e));
                false
            } else {
                true
            }
        });
        if manifest.entries.len() == before {
            return Ok(false);
        }
        for path in removed_files {
            drop(std::fs::remove_file(path));
        }
        let valid: Vec<_> = manifest
            .entries
            .into_iter()
            .filter(|e| self.png_path(e).is_file())
            .collect();
        self.write_manifest(&valid)?;
        Ok(true)
    }

    /// Delete every recorded history snapshot in this store.
    pub fn clear(&self) -> anyhow::Result<()> {
        if !self.dir.exists() {
            return Ok(());
        }
        if let Ok(read_dir) = std::fs::read_dir(&self.dir) {
            for item in read_dir.flatten() {
                let path = item.path();
                if path.is_file() {
                    drop(std::fs::remove_file(path));
                }
            }
        }
        self.write_manifest(&[])
    }

    fn read_manifest(&self) -> HistoryManifest {
        let path = self.dir.join(INDEX_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return HistoryManifest::default();
            }
            Err(e) => {
                eprintln!("[shotori] pin history read {}: {e}", path.display());
                return HistoryManifest::default();
            }
        };
        match serde_json::from_str::<HistoryManifest>(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "[shotori] corrupt pin history manifest {} (resetting in memory): {e}",
                    path.display()
                );
                HistoryManifest::default()
            }
        }
    }

    fn prune_and_save(
        &self,
        entries: &mut Vec<PinHistoryEntry>,
        max_entries: usize,
    ) -> anyhow::Result<()> {
        if entries.len() > max_entries {
            for evicted in entries.drain(max_entries..) {
                drop(std::fs::remove_file(self.png_path(&evicted)));
            }
        }
        self.write_manifest(entries)?;
        self.sweep_orphan_pngs(entries);
        Ok(())
    }

    fn write_manifest(&self, entries: &[PinHistoryEntry]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let payload = serde_json::to_vec_pretty(&HistoryManifest {
            version: 1,
            entries: entries.to_vec(),
        })
        .context("serializing pin history manifest")?;
        write_atomic(&self.dir.join(INDEX_FILE), &payload)
    }

    fn sweep_orphan_pngs(&self, kept: &[PinHistoryEntry]) {
        let keep_names: std::collections::BTreeSet<&str> =
            kept.iter().map(|e| e.png_file.as_str()).collect();
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for item in read_dir.flatten() {
            let name = item.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str.ends_with(".png") && !keep_names.contains(name_str) {
                drop(std::fs::remove_file(item.path()));
            }
        }
    }
}

/// Resolve the XDG data directory used for persistent pin history.
pub fn default_history_dir() -> anyhow::Result<PathBuf> {
    if let Some(override_dir) = std::env::var_os(HISTORY_DIR_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(override_dir));
    }
    let base = if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var_os("HOME").context("HOME environment variable is not set")?;
        PathBuf::from(home).join(".local/share")
    };
    Ok(base.join("shotori/pin-history"))
}

/// Ensure the history directory exists on disk and return it (for "Open folder").
pub fn ensure_history_dir() -> anyhow::Result<PathBuf> {
    let dir = default_history_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn bounds_to_array(b: Bounds<Pixels>) -> [f32; 4] {
    [
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    ]
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

fn write_atomic(dest: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .context("target path has no parent directory")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;
    std::io::Write::write_all(&mut tmp, bytes)
        .with_context(|| format!("writing temp file for {}", dest.display()))?;
    tmp.persist(dest)
        .map_err(|e| e.error)
        .with_context(|| format!("persisting {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NewPinRecord, PinHistoryStore};
    use gpui_kit::{Bounds, point, px, size};

    fn sample_rgba(w: u32, h: u32, r: u8) -> Vec<u8> {
        (0..(w * h)).flat_map(|_| [r, 40, 90, 255]).collect()
    }

    #[test]
    fn record_load_update_prune_and_clear_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinHistoryStore::at(tmp.path().join("history"));

        // Zero cap disables persistence cleanly.
        assert!(
            store
                .record(
                    NewPinRecord {
                        width: 4,
                        height: 3,
                        rgba: &sample_rgba(4, 3, 10),
                        rect: Bounds::new(point(px(10.), px(20.)), size(px(4.), px(3.))),
                        opacity: 100,
                        label: None,
                    },
                    0
                )
                .unwrap()
                .is_none()
        );
        assert!(store.entries().is_empty());

        // Record 3 entries with max_entries = 2 -> oldest is pruned + PNG removed.
        let e1 = store
            .record(
                NewPinRecord {
                    width: 4,
                    height: 3,
                    rgba: &sample_rgba(4, 3, 11),
                    rect: Bounds::new(point(px(1.), px(2.)), size(px(40.), px(30.))),
                    opacity: 90,
                    label: Some("first".into()),
                },
                2,
            )
            .unwrap()
            .unwrap();
        let e2 = store
            .record(
                NewPinRecord {
                    width: 4,
                    height: 3,
                    rgba: &sample_rgba(4, 3, 22),
                    rect: Bounds::new(point(px(5.), px(6.)), size(px(40.), px(30.))),
                    opacity: 85,
                    label: Some("second".into()),
                },
                2,
            )
            .unwrap()
            .unwrap();
        let e3 = store
            .record(
                NewPinRecord {
                    width: 2,
                    height: 2,
                    rgba: &sample_rgba(2, 2, 33),
                    rect: Bounds::new(point(px(9.), px(10.)), size(px(20.), px(20.))),
                    opacity: 75,
                    label: None,
                },
                2,
            )
            .unwrap()
            .unwrap();

        assert!(
            !store.png_path(&e1).exists(),
            "evicted PNG must be unlinked"
        );
        let list = store.entries();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, e3.id);
        assert_eq!(list[1].id, e2.id);

        // Update geometry, opacity, and edited raster on close.
        let edited = sample_rgba(6, 4, 99);
        assert!(
            store
                .update_entry(
                    &e3.id,
                    Bounds::new(point(px(100.), px(200.)), size(px(60.), px(40.))),
                    65,
                    Some((6, 4, &edited)),
                )
                .unwrap()
        );

        let (loaded, rgba) = store.load_entry(&e3.id).unwrap();
        assert_eq!((loaded.width, loaded.height), (6, 4));
        assert_eq!(loaded.opacity, 65);
        assert_eq!(
            loaded.logical_bounds(),
            Bounds::new(point(px(100.), px(200.)), size(px(60.), px(40.)))
        );
        assert_eq!(rgba, edited);

        assert!(store.remove(&e2.id).unwrap());
        assert_eq!(store.entries().len(), 1);
        store.clear().unwrap();
        assert!(store.entries().is_empty());
    }

    #[test]
    fn corrupt_manifest_and_missing_png_degrade_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.json"), b"{not valid json!!").unwrap();

        let store = PinHistoryStore::at(dir.clone());
        assert!(store.entries().is_empty());

        // Subsequent record recovers the store cleanly.
        let saved = store
            .record(
                NewPinRecord {
                    width: 2,
                    height: 2,
                    rgba: &sample_rgba(2, 2, 77),
                    rect: Bounds::new(point(px(0.), px(0.)), size(px(2.), px(2.))),
                    opacity: 100,
                    label: None,
                },
                10,
            )
            .unwrap()
            .unwrap();
        assert_eq!(store.entries().len(), 1);

        // External deletion of the PNG hides the broken entry instead of failing `entries()`.
        std::fs::remove_file(store.png_path(&saved)).unwrap();
        assert!(store.entries().is_empty());
        assert!(store.load_entry(&saved.id).is_err());
    }
}
