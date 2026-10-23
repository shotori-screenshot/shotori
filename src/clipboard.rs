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

// ── Linux: resident daemon over zwlr_data_control ────────────────────

// main.rs dispatches the daemon child on these; re-exported at the
// crate surface so the internal `imp` split stays private.
#[cfg(target_os = "linux")]
pub use imp::{DAEMON_ARG, daemon_main};

#[cfg(target_os = "linux")]
mod imp {
    use std::io::{Read as _, Write as _};
    use std::process::{Command, Stdio};

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

    fn spawn_daemon(mime: &str, data: &[u8]) -> anyhow::Result<()> {
        if data.is_empty() {
            anyhow::bail!("refusing to copy empty data");
        }
        if !manager_available() {
            anyhow::bail!(
                "compositor does not support zwlr_data_control_manager_v1, cannot copy to clipboard"
            );
        }

        let exe = std::env::current_exe().context("cannot locate own executable")?;
        let mut child = Command::new(exe)
            .arg(DAEMON_ARG)
            .arg(mime) // the daemon decides what to offer based on this
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
    /// failing silently inside the daemon
    fn manager_available() -> bool {
        let Ok(conn) = Connection::connect_to_env() else {
            return false;
        };
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut probe = Probe::default();
        queue.roundtrip(&mut probe).is_ok() && probe.manager
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
            payload,
            mimes: mimes.clone(),
            ..Daemon::default()
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

        // Destroy proxies before exiting (the connection drop would clean up
        // too; doing it explicitly is good manners toward the compositor)
        if let Some(s) = app.source.take() {
            s.destroy();
        }
        if let Some(d) = app.device.take() {
            d.destroy();
        }
        Ok(())
    }

    #[derive(Default)]
    struct Daemon {
        payload: Vec<u8>,
        /// Every MIME this daemon offers (a paste request matching any of them
        /// gets the data)
        mimes: Vec<String>,
        seat: Option<wl_seat::WlSeat>,
        manager: Option<ZwlrDataControlManagerV1>,
        device: Option<ZwlrDataControlDeviceV1>,
        source: Option<ZwlrDataControlSourceV1>,
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
            source: &ZwlrDataControlSourceV1,
            event: zwlr_data_control_source_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                // Someone pastes: write the data into the fd they provided (the
                // compositor hands it over; close when done)
                zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                    if state.mimes.contains(&mime_type) {
                        let mut file = std::fs::File::from(fd); // fd closed on drop
                        let _ = file.write_all(&state.payload); // broken pipe etc: ignore
                    }
                    // MIME mismatch: the fd (OwnedFd) drops at the end of this
                    // branch and closes itself
                    let _ = source;
                }
                // The clipboard was taken over by someone else: retire
                zwlr_data_control_source_v1::Event::Cancelled => {
                    state.cancelled = true;
                }
                _ => {}
            }
        }
    }
}
