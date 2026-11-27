//! # Clipboard: put pixels and text on the system clipboard
//!
//! Wayland uses the "resident offer" model: the data-source process must
//! stay alive for the clipboard to hold anything. Copy = hand the bytes
//! to a **background daemon** (a re-exec of ourselves with [`DAEMON_ARG`],
//! avoiding raw fork inside a multithreaded process) which connects to
//! the compositor and serves paste requests until someone else takes the
//! clipboard. N screenshots in a row are fine: each new daemon replaces
//! the previous one automatically.

// ── Public surface (platform-neutral) ────────────────────────────────

/// Decoded clipboard payload for `shotori pin --clipboard` / `Ctrl+V`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardData {
    Image { w: u32, h: u32, rgba: Vec<u8> },
    Text(String),
}

/// Put an image on the clipboard: the PNG bytes are handed to the
/// resident daemon, which serves `image/png` to paste targets. The raw
/// pixels are unused on Wayland (kept for a stable internal API).
pub fn copy_image(w: u32, h: u32, rgba: &[u8], png: &[u8]) -> anyhow::Result<()> {
    imp::copy_image(w, h, rgba, png)
}

/// Put plain text on the clipboard (the OCR output path).
pub fn copy_text(text: String) -> anyhow::Result<()> {
    imp::copy_text(text)
}

/// Read an image (`image/*`) or UTF-8 text (`text/plain*`) from the clipboard.
pub fn read_clipboard() -> anyhow::Result<ClipboardData> {
    imp::read_clipboard()
}

// ── Linux: resident daemon over zwlr_data_control ────────────────────

// main.rs dispatches the daemon child on these; re-exported at the
// crate surface so the internal `imp` split stays private.
#[cfg(target_os = "linux")]
pub use imp::{DAEMON_ARG, daemon_main};

#[cfg(target_os = "linux")]
mod imp {
    use std::io::{Read as _, Write as _};
    use std::os::fd::{AsFd as _, OwnedFd};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use anyhow::Context as _;
    use wayland_client::{
        Connection, Dispatch, QueueHandle, event_created_child,
        protocol::{wl_registry, wl_seat},
    };
    use wayland_protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
        zwlr_data_control_manager_v1::{self, ZwlrDataControlManagerV1},
        zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
        zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
    };

    /// The argv marker for daemon mode (main.rs dispatches on this)
    pub const DAEMON_ARG: &str = "--clipboard-daemon";
    /// Image MIME (accepted by GTK/Qt/browsers alike)
    const IMAGE_MIME: &str = "image/png";
    /// Text MIME (with charset suffix; GTK/Qt paste it correctly)
    const TEXT_MIME: &str = "text/plain;charset=utf-8";

    pub fn copy_image(_w: u32, _h: u32, _rgba: &[u8], png: &[u8]) -> anyhow::Result<()> {
        // The daemon serves image/png over zwlr_data_control — only the
        // bytes cross the pipe
        spawn_daemon(IMAGE_MIME, png)
    }

    pub fn copy_text(text: String) -> anyhow::Result<()> {
        spawn_daemon(TEXT_MIME, text.as_bytes())
    }

    /// Maximum bytes read from a clipboard offer (128 MiB) so a hostile or
    /// runaway clipboard owner cannot exhaust memory.
    const MAX_CLIPBOARD_READ: u64 = 128 * 1024 * 1024;

    const IMAGE_MIMES: &[&str] = &[
        "image/png",
        "image/webp",
        "image/jpeg",
        "image/bmp",
        "image/tiff",
    ];
    const TEXT_MIMES: &[&str] = &[
        "text/plain;charset=utf-8",
        "UTF8_STRING",
        "text/plain",
        "STRING",
    ];

    pub fn read_clipboard() -> anyhow::Result<super::ClipboardData> {
        let conn = Connection::connect_to_env().context(
            "cannot connect to the Wayland display (is WAYLAND_DISPLAY set?), cannot read clipboard",
        )?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());

        let mut reader = ClipReader::default();
        queue.roundtrip(&mut reader)?;

        let seat = reader
            .seat
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no wl_seat available"))?;
        let manager = reader.manager.take().ok_or_else(|| {
            anyhow::anyhow!(
                "compositor does not support zwlr_data_control_manager_v1, cannot read clipboard"
            )
        })?;

        let device = manager.get_data_device(&seat, &qh, ());
        queue.roundtrip(&mut reader)?;

        let cleanup = |reader: &mut ClipReader, device: ZwlrDataControlDeviceV1| {
            for (offer, _) in reader.offers.drain(..) {
                offer.destroy();
            }
            if let Some(sel) = reader.selection.take() {
                sel.destroy();
            }
            device.destroy();
        };

        let Some(selection) = reader.selection.clone() else {
            cleanup(&mut reader, device);
            anyhow::bail!("clipboard is empty");
        };
        let offered_mimes: Vec<String> = reader
            .offers
            .iter()
            .find(|(o, _)| o == &selection)
            .map(|(_, m)| m.clone())
            .unwrap_or_default();

        // Prefer image offers over plain text when both are advertised (e.g. browser copy).
        let chosen_image = IMAGE_MIMES
            .iter()
            .copied()
            .find(|want| offered_mimes.iter().any(|m| m.eq_ignore_ascii_case(want)));
        let chosen_text = TEXT_MIMES
            .iter()
            .copied()
            .find(|want| offered_mimes.iter().any(|m| m == want));

        let Some((mime, is_image)) = chosen_image
            .map(|m| (m, true))
            .or_else(|| chosen_text.map(|m| (m, false)))
        else {
            cleanup(&mut reader, device);
            anyhow::bail!(
                "clipboard contains no supported image or text (offered: {})",
                if offered_mimes.is_empty() {
                    "none".to_owned()
                } else {
                    offered_mimes.join(", ")
                }
            );
        };

        let (mut pipe_reader, pipe_writer) =
            std::io::pipe().context("creating clipboard transfer pipe")?;
        selection.receive(mime.to_owned(), std::os::fd::AsFd::as_fd(&pipe_writer));
        // Flushing sends `receive` (and its duplicated write fd via SCM_RIGHTS)
        // to the compositor; dropping our local `pipe_writer` immediately after
        // ensures `read_to_end` reaches EOF once the clipboard owner finishes.
        conn.flush()
            .context("flushing Wayland clipboard receive request")?;
        drop(pipe_writer);

        let mut bytes = Vec::new();
        (&mut pipe_reader)
            .take(MAX_CLIPBOARD_READ + 1)
            .read_to_end(&mut bytes)
            .context("reading clipboard bytes from owner")?;
        cleanup(&mut reader, device);
        drop(conn);

        anyhow::ensure!(
            (bytes.len() as u64) <= MAX_CLIPBOARD_READ,
            "clipboard data exceeds 128 MiB safety limit"
        );
        anyhow::ensure!(!bytes.is_empty(), "clipboard data is empty");

        if is_image {
            let rgba = image::load_from_memory(&bytes)
                .context("decoding image from clipboard")?
                .into_rgba8();
            let (w, h) = rgba.dimensions();
            anyhow::ensure!(w > 0 && h > 0, "clipboard image has zero dimensions");
            Ok(super::ClipboardData::Image {
                w,
                h,
                rgba: rgba.into_raw(),
            })
        } else {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            anyhow::ensure!(!text.trim().is_empty(), "clipboard text is blank");
            Ok(super::ClipboardData::Text(text))
        }
    }

    #[derive(Default)]
    struct ClipReader {
        seat: Option<wl_seat::WlSeat>,
        manager: Option<ZwlrDataControlManagerV1>,
        offers: Vec<(ZwlrDataControlOfferV1, Vec<String>)>,
        selection: Option<ZwlrDataControlOfferV1>,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for ClipReader {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_seat" if state.seat.is_none() => {
                        state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                    }
                    "zwlr_data_control_manager_v1" => {
                        state.manager = Some(registry.bind(name, version.min(2), qh, ()));
                    }
                    _ => {}
                }
            }
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for ClipReader {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrDataControlManagerV1, ()> for ClipReader {
        fn event(
            _: &mut Self,
            _: &ZwlrDataControlManagerV1,
            _: zwlr_data_control_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrDataControlDeviceV1, ()> for ClipReader {
        fn event(
            state: &mut Self,
            _: &ZwlrDataControlDeviceV1,
            event: zwlr_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                zwlr_data_control_device_v1::Event::DataOffer { id } => {
                    state.offers.push((id, Vec::new()));
                }
                zwlr_data_control_device_v1::Event::Selection { id } => {
                    state.selection = id;
                }
                _ => {}
            }
        }

        event_created_child!(ClipReader, ZwlrDataControlDeviceV1, [
            zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
        ]);
    }

    impl Dispatch<ZwlrDataControlOfferV1, ()> for ClipReader {
        fn event(
            state: &mut Self,
            offer: &ZwlrDataControlOfferV1,
            event: zwlr_data_control_offer_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
                && let Some((_, mimes)) = state.offers.iter_mut().find(|(o, _)| o == offer)
            {
                mimes.push(mime_type);
            }
        }
    }

    fn spawn_daemon(mime: &str, data: &[u8]) -> anyhow::Result<()> {
        if data.is_empty() {
            anyhow::bail!("refusing to copy empty data");
        }
        // Two failure modes with different fixes: no Wayland connection
        // is the environment (WAYLAND_DISPLAY), a missing manager is the
        // compositor — blaming the wrong one sends users chasing ghosts.
        match manager_support() {
            Some(true) => {}
            Some(false) => anyhow::bail!(
                "compositor does not support zwlr_data_control_manager_v1, cannot copy to clipboard"
            ),
            None => anyhow::bail!(
                "cannot connect to the Wayland display (is WAYLAND_DISPLAY set?), cannot copy to clipboard"
            ),
        }

        let exe = std::env::current_exe().context("cannot locate own executable")?;
        let mut child = Command::new(exe)
            .arg(DAEMON_ARG)
            .arg(mime) // the daemon decides what to offer based on this
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            // stderr inherited like the notify child: the daemon's
            // anyhow diagnostics ("no WAYLAND_DISPLAY", protocol
            // missing) must land somewhere — nulling them turned a
            // fixable environment problem into an unobservable one.
            .spawn()
            .context("failed to spawn clipboard daemon")?;

        // Write then close (drop) — the daemon reads stdin to EOF, then offers.
        // A multi-MB payload exceeding the pipe buffer is fine: the daemon is
        // reading concurrently, no deadlock.
        child
            .stdin
            .take()
            .expect("stdin was just set to piped")
            .write_all(data)
            .context("failed to send data to clipboard daemon")?;
        // Detect a daemon that dies at startup (compositor gone, no seat,
        // manager vanished between probe and offer): without this check the
        // user would get a "copied" notification for an empty clipboard.
        // A healthy daemon never exits on its own (it serves until the
        // selection is taken), so a live process after this window = success.
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(status) = child
            .try_wait()
            .context("waiting for clipboard daemon startup")?
        {
            anyhow::bail!("clipboard daemon exited with {status} at startup");
        }
        Ok(())
    }

    /// A quick probe for data-control support (one roundtrip, ~1ms) so that
    /// "unsupported" becomes a human-readable error at copy time instead of
    /// failing silently inside the daemon. `None` = cannot reach a Wayland
    /// display at all (environment, e.g. WAYLAND_DISPLAY unset) — a
    /// different problem from a compositor without the protocol, and the
    /// copy-time error must not blame the compositor for it.
    fn manager_support() -> Option<bool> {
        let Ok(conn) = Connection::connect_to_env() else {
            return None;
        };
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut probe = Probe::default();
        Some(queue.roundtrip(&mut probe).is_ok() && probe.manager)
    }

    #[derive(Default)]
    struct Probe {
        manager: bool,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
        fn event(
            state: &mut Self,
            _registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global { interface, .. } = event
                && interface == "zwlr_data_control_manager_v1"
            {
                state.manager = true;
            }
        }
    }

    // ── The daemon itself ────────────────────────────────────────────

    /// Daemon entry point: `shotori --clipboard-daemon <MIME>`
    /// Read stdin fully → connect to compositor, offer the data → serve paste
    /// requests until overridden
    pub fn daemon_main() -> anyhow::Result<()> {
        // args_os + lossy: the MIME comes from whatever the clipboard
        // offered (usually RFC-2045 ASCII, but a hostile target name
        // must not panic the daemon). MIME matching in the compositor
        // is byte-based, so lossy conversion only affects garbage names
        // that would never match anyway.
        let mime = std::env::args_os()
            .nth(2)
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_else(|| IMAGE_MIME.to_string());

        let mut payload = Vec::new();
        std::io::stdin()
            .read_to_end(&mut payload)
            .context("daemon failed to read stdin")?;
        if payload.is_empty() {
            anyhow::bail!("daemon received empty data, refusing to serve");
        }

        let conn = Connection::connect_to_env().context("daemon cannot connect to compositor")?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());

        // Offer list: the primary MIME first; text mode adds fallback MIMEs —
        // old xwayland apps only accept UTF8_STRING/STRING (bare text/plain
        // covers clients that ignore the charset suffix). Any hit in the list
        // gets the data written on paste.
        let mut mimes = vec![mime.clone()];
        if mime.starts_with("text/") {
            mimes.extend(["text/plain", "UTF8_STRING", "STRING"].map(String::from));
        }

        let mut app = Daemon {
            payload: Arc::from(payload),
            mimes: mimes.clone(),
            seat: None,
            manager: None,
            device: None,
            source: None,
            writers: Vec::new(),
            cancelled: false,
        };
        queue.roundtrip(&mut app)?;

        let seat = app
            .seat
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no wl_seat"))?;
        let manager = app.manager.take().ok_or_else(|| {
            anyhow::anyhow!("compositor does not support zwlr_data_control_manager_v1")
        })?;

        let device = manager.get_data_device(&seat, &qh, ());
        let source = manager.create_data_source(&qh, ());
        for m in &mimes {
            source.offer(m.clone());
        }
        device.set_selection(Some(&source));
        app.device = Some(device);
        app.source = Some(source);
        queue.roundtrip(&mut app)?; // make sure set_selection has been sent

        while !app.cancelled {
            queue.blocking_dispatch(&mut app)?;
        }

        // Destroy proxies and flush before waiting on in-flight paste writers
        // so the compositor sees our retirement immediately, while still
        // letting an active multi-MB paste finish instead of truncating on exit.
        if let Some(s) = app.source.take() {
            s.destroy();
        }
        if let Some(d) = app.device.take() {
            d.destroy();
        }
        if let Err(err) = conn.flush() {
            eprintln!("[shotori] clipboard daemon flush on exit: {err}");
        }
        for handle in app.writers.drain(..) {
            if handle.join().is_err() {
                eprintln!("[shotori] clipboard send worker panicked");
            }
        }
        Ok(())
    }

    /// Maximum time a clipboard paste writer waits for a full pipe buffer to
    /// drain before concluding the receiver stalled. Active INCR transfers
    /// drain 64 KiB chunks in milliseconds; 10 s is generous for slow clients
    /// while preventing a hung receiver from pinning a retiring daemon forever.
    const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// Write `data` completely to a compositor-provided pipe fd, handling both
    /// blocking and `O_NONBLOCK` pipes.
    ///
    /// wlroots' Xwayland selection bridge passes an `O_NONBLOCK` pipe fd in
    /// `zwlr_data_control_source_v1::Event::Send`. Standard `File::write_all`
    /// only retries `EINTR` and immediately fails with `WouldBlock` (`EAGAIN`)
    /// once the 64 KiB Linux pipe buffer fills — silently truncating every
    /// screenshot larger than 64 KiB pasted into an X11 client on Sway/wlroots.
    fn write_all_nonblock(
        fd: OwnedFd,
        mut data: &[u8],
        stall_timeout: Duration,
    ) -> std::io::Result<()> {
        let mut file = std::fs::File::from(fd); // fd closed on drop
        while !data.is_empty() {
            match file.write(data) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "clipboard pipe returned zero bytes written",
                    ));
                }
                Ok(n) => {
                    data = &data[n..];
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    wait_writable(file.as_fd(), stall_timeout)?;
                }
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Poll `fd` until it becomes writable (or reports an error/hangup so the
    /// subsequent `write` surfaces `EPIPE`), bounded by `stall_timeout`.
    fn wait_writable(
        fd: std::os::fd::BorrowedFd<'_>,
        stall_timeout: Duration,
    ) -> std::io::Result<()> {
        let deadline = Instant::now() + stall_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "clipboard receiver stalled",
                ));
            }
            let ts = rustix::event::Timespec {
                tv_sec: remaining.as_secs() as _,
                tv_nsec: remaining.subsec_nanos() as _,
            };
            let mut fds = [rustix::event::PollFd::new(
                &fd,
                rustix::event::PollFlags::OUT,
            )];
            match rustix::event::poll(&mut fds, Some(&ts)) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "clipboard receiver stalled",
                    ));
                }
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => return Err(std::io::Error::from(err)),
            }
        }
    }

    struct Daemon {
        payload: Arc<[u8]>,
        /// Every MIME this daemon offers (a paste request matching any of them
        /// gets the data)
        mimes: Vec<String>,
        seat: Option<wl_seat::WlSeat>,
        manager: Option<ZwlrDataControlManagerV1>,
        device: Option<ZwlrDataControlDeviceV1>,
        source: Option<ZwlrDataControlSourceV1>,
        writers: Vec<JoinHandle<()>>,
        cancelled: bool,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Daemon {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_seat" if state.seat.is_none() => {
                        state.seat = Some(registry.bind(name, version.min(7), qh, ()))
                    }
                    "zwlr_data_control_manager_v1" => {
                        state.manager = Some(registry.bind(name, version.min(2), qh, ()))
                    }
                    _ => {}
                }
            }
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for Daemon {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrDataControlManagerV1, ()> for Daemon {
        fn event(
            _: &mut Self,
            _: &ZwlrDataControlManagerV1,
            _: zwlr_data_control_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrDataControlDeviceV1, ()> for Daemon {
        fn event(
            _state: &mut Self,
            device: &ZwlrDataControlDeviceV1,
            event: zwlr_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            // We only write, never read; DataOffer/Selection notify "what's in
            // someone else's clipboard" — irrelevant. Note: after our
            // set_selection the compositor mirrors the new selection back to us;
            // that DataOffer event arrives too — destroy it politely as well.
            if let zwlr_data_control_device_v1::Event::DataOffer { id } = event {
                id.destroy();
                let _ = device;
            }
        }

        // The compositor creates offer objects on our behalf inside DataOffer
        // events; wayland-rs requires the parent interface to specialize
        // event_created_child (the default panics — a pitfall we hit: the
        // roundtrip after copying crashed immediately, wl-paste reported
        // "Nothing is copied")
        event_created_child!(Daemon, ZwlrDataControlDeviceV1, [
            zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
        ]);
    }

    impl Dispatch<ZwlrDataControlOfferV1, ()> for Daemon {
        fn event(
            _: &mut Self,
            _: &ZwlrDataControlOfferV1,
            _: zwlr_data_control_offer_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrDataControlSourceV1, ()> for Daemon {
        fn event(
            state: &mut Self,
            _source: &ZwlrDataControlSourceV1,
            event: zwlr_data_control_source_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                // Someone pastes: stream the data into the fd they provided on
                // a dedicated thread so slow/INCR receivers never block the
                // Wayland event loop and `O_NONBLOCK` pipes write to EOF.
                zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                    state.writers.retain(|h| !h.is_finished());
                    if state.mimes.contains(&mime_type) {
                        let payload = Arc::clone(&state.payload);
                        match std::thread::Builder::new()
                            .name("clipboard-send".into())
                            .spawn(move || {
                                if let Err(err) =
                                    write_all_nonblock(fd, &payload, WRITE_STALL_TIMEOUT)
                                    && err.kind() != std::io::ErrorKind::BrokenPipe
                                {
                                    eprintln!("[shotori] clipboard send aborted: {err}");
                                }
                            }) {
                            Ok(handle) => state.writers.push(handle),
                            Err(err) => {
                                eprintln!("[shotori] clipboard send thread spawn failed: {err}");
                            }
                        }
                    }
                    // MIME mismatch (or spawn error): `fd` drops and closes itself.
                }
                // The clipboard was taken over by someone else: retire
                zwlr_data_control_source_v1::Event::Cancelled => {
                    state.cancelled = true;
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn write_all_nonblock_streams_multi_buffer_payload_over_nonblocking_pipe() {
            let (mut reader, writer) = std::io::pipe().expect("pipe");
            rustix::io::ioctl_fionbio(&writer, true).expect("set O_NONBLOCK");

            // 512 KiB = 8x the default 64 KiB Linux pipe buffer, guaranteeing
            // repeated EAGAIN + poll(POLLOUT) cycles.
            let payload: Vec<u8> = (0..(512 * 1024))
                .map(|i| ((i * 31 + 17) & 0xFF) as u8)
                .collect();
            let expected_len = payload.len();

            let reader_thread = std::thread::spawn(move || {
                let mut out = Vec::with_capacity(expected_len);
                let mut buf = [0u8; 4096];
                loop {
                    let n = reader.read(&mut buf).expect("read chunk");
                    if n == 0 {
                        break;
                    }
                    out.extend_from_slice(&buf[..n]);
                    // Yield occasionally so the writer hits EAGAIN reliably.
                    if out.len() % (64 * 1024) == 0 {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                out
            });

            let write_res =
                write_all_nonblock(OwnedFd::from(writer), &payload, Duration::from_secs(5));
            assert!(write_res.is_ok(), "unexpected write error: {write_res:?}");

            let received = reader_thread.join().expect("reader thread");
            assert_eq!(received.len(), payload.len());
            assert_eq!(received, payload);
        }

        #[test]
        fn write_all_nonblock_returns_broken_pipe_when_reader_closes_early() {
            let (mut reader, writer) = std::io::pipe().expect("pipe");
            rustix::io::ioctl_fionbio(&writer, true).expect("set O_NONBLOCK");

            let payload = vec![0xAB; 512 * 1024];
            let reader_thread = std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                let _ = reader.read(&mut buf);
                // Close read end while writer still has >500 KiB left.
                drop(reader);
            });

            let err = write_all_nonblock(OwnedFd::from(writer), &payload, Duration::from_secs(5))
                .expect_err("closing reader early must fail with BrokenPipe");
            reader_thread.join().expect("reader thread");
            assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        }

        #[test]
        fn write_all_nonblock_times_out_when_receiver_stalls() {
            let (_reader_kept_open, writer) = std::io::pipe().expect("pipe");
            rustix::io::ioctl_fionbio(&writer, true).expect("set O_NONBLOCK");

            // Larger than the 64 KiB pipe buffer so the writer blocks on EAGAIN.
            let payload = vec![0xCD; 256 * 1024];
            let err =
                write_all_nonblock(OwnedFd::from(writer), &payload, Duration::from_millis(60))
                    .expect_err("stalled receiver must time out");
            assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        }

        #[test]
        fn write_all_nonblock_handles_blocking_pipe_and_empty_payload() {
            let (mut reader, writer) = std::io::pipe().expect("pipe");
            let payload = b"hello blocking clipboard".to_vec();
            let write_res =
                write_all_nonblock(OwnedFd::from(writer), &payload, Duration::from_secs(2));
            assert!(write_res.is_ok());

            let mut out = Vec::new();
            reader.read_to_end(&mut out).expect("read");
            assert_eq!(out, payload);

            // Empty payload closes cleanly without writing.
            let (mut empty_reader, empty_writer) = std::io::pipe().expect("pipe");
            assert!(
                write_all_nonblock(OwnedFd::from(empty_writer), &[], Duration::from_secs(1))
                    .is_ok()
            );
            let mut empty_out = Vec::new();
            empty_reader.read_to_end(&mut empty_out).expect("read");
            assert!(empty_out.is_empty());
        }
    }
}
