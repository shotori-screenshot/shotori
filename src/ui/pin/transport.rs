//! Per-Wayland-session pin ownership. A locked file elects one owner; a
//! private Unix socket transports bounded RGBA payloads. Acknowledgement
//! happens only after the scene accepts the image, never just after a write.
use super::PinSpec;
use gpui_kit::{Bounds, point, px, size};
use std::{
    fs::{File, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

const MAGIC: &[u8; 8] = b"SHTRPIN1";
const MAX_BYTES: usize = 512 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) enum Prepared {
    Forwarded,
    Owner(PinSpec, Server),
}

pub(super) struct Request {
    pub spec: PinSpec,
    pub reply: std::sync::mpsc::Sender<Result<(), String>>,
}

/// Keep the lock inode stable. Only the elected owner may unlink a stale
/// socket, and removal must happen before releasing the lock.
struct Ownership {
    socket: PathBuf,
    _lock: File,
}
impl Drop for Ownership {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

pub(crate) struct Server {
    listener: UnixListener,
    ownership: Ownership,
}

impl Server {
    pub(super) fn start(self) -> anyhow::Result<async_channel::Receiver<Request>> {
        self.listener.set_nonblocking(true)?;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("shotori-pins".into())
            .spawn(move || {
                let _ownership = self.ownership;
                while !tx.is_closed() {
                    let mut stream = match self.listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        Err(_) => break,
                    };
                    let result = (|| -> anyhow::Result<()> {
                        stream.set_read_timeout(Some(TIMEOUT))?;
                        stream.set_write_timeout(Some(TIMEOUT))?;
                        let spec = read_spec(&mut stream)?;
                        let (reply, completed) = std::sync::mpsc::channel();
                        tx.send_blocking(Request { spec, reply })?;
                        completed.recv()?.map_err(anyhow::Error::msg)
                    })();
                    let _ = stream.write_all(&[u8::from(result.is_ok())]);
                    if tx.is_closed() {
                        break;
                    }
                }
            })?;
        Ok(rx)
    }
}

fn endpoint() -> anyhow::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR is not set"))?;
    let directory = PathBuf::from(runtime).join("shotori-pins");
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    let uid = std::fs::metadata("/proc/self")?.uid();
    anyhow::ensure!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "pin runtime directory must be private and owned by this user"
    );
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .ok_or_else(|| anyhow::anyhow!("WAYLAND_DISPLAY is not set"))?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    display.hash(&mut hash);
    Ok(directory.join(format!("{:016x}.sock", hash.finish())))
}

pub(crate) fn prepare(spec: PinSpec) -> anyhow::Result<Prepared> {
    prepare_at(spec, &endpoint()?)
}

fn prepare_at(spec: PinSpec, socket: &Path) -> anyhow::Result<Prepared> {
    validate(&spec)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(socket.with_extension("lock"))?;
    match lock.try_lock() {
        Ok(()) => {
            let ownership = Ownership {
                socket: socket.to_owned(),
                _lock: lock,
            };
            // A previous owner may have crashed. Holding the lock proves this
            // socket is stale; connection errors alone do not prove that.
            match std::fs::remove_file(socket) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            let listener = UnixListener::bind(socket)?;
            Ok(Prepared::Owner(
                spec,
                Server {
                    listener,
                    ownership,
                },
            ))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            // Allow the elected owner to finish binding. Retry only before
            // sending a request: replaying an unacknowledged request can duplicate it.
            let mut connection = None;
            for _ in 0..40 {
                match UnixStream::connect(socket) {
                    Ok(stream) => {
                        connection = Some(stream);
                        break;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let mut stream = connection
                .ok_or_else(|| anyhow::anyhow!("pin owner is starting or closing; try again"))?;
            stream.set_read_timeout(Some(TIMEOUT))?;
            stream.set_write_timeout(Some(TIMEOUT))?;
            write_spec(&mut stream, &spec)?;
            let mut ack = [0];
            stream.read_exact(&mut ack)?;
            anyhow::ensure!(ack == [1], "pin owner could not add the image; try again");
            Ok(Prepared::Forwarded)
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn byte_len(w: u32, h: u32) -> anyhow::Result<usize> {
    let len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|len| len.checked_mul(4));
    anyhow::ensure!(w > 0 && h > 0, "invalid pin dimensions");
    // The checked_mul chain is the length authority: None here means an
    // overflow the ensure! above must have caught — surface it as an
    // error instead of unwrapping.
    len.ok_or_else(|| anyhow::anyhow!("pin dimensions overflow"))
        .and_then(|len| {
            if len <= MAX_BYTES {
                Ok(len)
            } else {
                Err(anyhow::anyhow!("invalid pin dimensions"))
            }
        })
}
fn validate(spec: &PinSpec) -> anyhow::Result<()> {
    anyhow::ensure!(
        spec.rgba.len() == byte_len(spec.w, spec.h)?,
        "invalid pin pixels"
    );
    let [x, y, w, h] = geometry(spec);
    anyhow::ensure!(
        [x, y, w, h].iter().all(|n| n.is_finite())
            && w > 0.
            && h > 0.
            && x.abs() < 1e7
            && y.abs() < 1e7
            && w < 1e7
            && h < 1e7,
        "invalid pin geometry"
    );
    Ok(())
}
fn geometry(spec: &PinSpec) -> [f32; 4] {
    [
        spec.rect.origin.x.into(),
        spec.rect.origin.y.into(),
        spec.rect.size.width.into(),
        spec.rect.size.height.into(),
    ]
}
fn write_spec(writer: &mut impl Write, spec: &PinSpec) -> anyhow::Result<()> {
    validate(spec)?;
    writer.write_all(MAGIC)?;
    writer.write_all(&spec.w.to_le_bytes())?;
    writer.write_all(&spec.h.to_le_bytes())?;
    for value in geometry(spec) {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.write_all(&spec.rgba)?;
    Ok(())
}
fn read_spec(reader: &mut impl Read) -> anyhow::Result<PinSpec> {
    let mut header = [0u8; 32];
    reader.read_exact(&mut header)?;
    anyhow::ensure!(&header[..8] == MAGIC, "incompatible pin protocol");
    let w = u32::from_le_bytes(header[8..12].try_into()?);
    let h = u32::from_le_bytes(header[12..16].try_into()?);
    let mut geometry = [0.; 4];
    for (ix, value) in geometry.iter_mut().enumerate() {
        *value = f32::from_le_bytes(header[16 + ix * 4..20 + ix * 4].try_into()?);
    }
    let mut spec = PinSpec {
        w,
        h,
        rgba: vec![0; byte_len(w, h)?],
        rect: Bounds::new(
            point(px(geometry[0]), px(geometry[1])),
            size(px(geometry[2]), px(geometry[3])),
        ),
    };
    validate(&spec)?;
    reader.read_exact(&mut spec.rgba)?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::{PinSpec, Prepared, prepare_at, read_spec, write_spec};
    use gpui_kit::{Bounds, point, px, size};
    use std::io::Cursor;

    fn spec() -> PinSpec {
        PinSpec {
            w: 2,
            h: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            rect: Bounds::new(point(px(-12.5), px(20.)), size(px(1.6), px(0.8))),
        }
    }
    #[test]
    fn transport_preserves_pixels_and_fractional_global_geometry() {
        let mut encoded = Vec::new();
        write_spec(&mut encoded, &spec()).unwrap();
        let decoded = read_spec(&mut Cursor::new(&encoded)).unwrap();
        assert_eq!(decoded.rgba, spec().rgba);
        assert_eq!(decoded.rect, spec().rect);
        assert_eq!((decoded.w, decoded.h), (2, 1));
        assert!(read_spec(&mut Cursor::new(&encoded[..encoded.len() - 1])).is_err());
        encoded[0] = 0;
        assert!(read_spec(&mut Cursor::new(encoded)).is_err());
    }
    #[test]
    fn rejects_invalid_dimensions_geometry_and_payloads() {
        let mut invalid = spec();
        invalid.rgba.pop();
        assert!(write_spec(&mut Vec::new(), &invalid).is_err());
        let mut invalid = spec();
        invalid.rect.size.width = px(f32::NAN);
        assert!(write_spec(&mut Vec::new(), &invalid).is_err());
        let mut encoded = Vec::new();
        write_spec(&mut encoded, &spec()).unwrap();
        encoded[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_spec(&mut Cursor::new(encoded)).is_err());
    }
    #[test]
    fn one_owner_receives_other_sessions_and_acknowledges_acceptance() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("pins.sock");
        let Prepared::Owner(_, server) = prepare_at(spec(), &socket).unwrap() else {
            panic!("first session must own the scene")
        };
        let requests = server.start().unwrap();
        let mut client = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ui::pin::transport::tests::forward_from_a_separate_process",
                "--nocapture",
            ])
            .env("SHOTORI_TEST_PIN_SOCKET", &socket)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let request = requests.recv_blocking().unwrap();
        assert_eq!(request.spec.rgba, spec().rgba);
        assert!(
            client.try_wait().unwrap().is_none(),
            "sender must wait for scene acceptance"
        );
        request.reply.send(Ok(())).unwrap();
        assert!(client.wait().unwrap().success());
        // A failed insertion is surfaced, not reported as successful transfer.
        let destination = socket.clone();
        let client = std::thread::spawn(move || prepare_at(spec(), &destination));
        requests
            .recv_blocking()
            .unwrap()
            .reply
            .send(Err("closing".into()))
            .unwrap();
        assert!(client.join().unwrap().is_err());
        drop(requests);
        for _ in 0..100 {
            if !socket.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!socket.exists(), "owner must remove its socket on shutdown");
        assert!(matches!(
            prepare_at(spec(), &socket).unwrap(),
            Prepared::Owner(..)
        ));
    }
    #[test]
    fn forward_from_a_separate_process() {
        // Invoked by the owner test above; ordinary test runs have no endpoint.
        let Some(socket) = std::env::var_os("SHOTORI_TEST_PIN_SOCKET") else {
            return;
        };
        assert!(matches!(
            prepare_at(spec(), std::path::Path::new(&socket)).unwrap(),
            Prepared::Forwarded
        ));
    }
    #[test]
    fn stale_socket_is_recovered_only_by_the_lock_owner() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("pins.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        let prepared = prepare_at(spec(), &socket).unwrap();
        assert!(matches!(prepared, Prepared::Owner(..)));
        assert!(socket.exists());
        drop(prepared);
        assert!(!socket.exists());
    }
}
