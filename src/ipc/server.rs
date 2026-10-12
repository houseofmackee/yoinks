//! A Unix socket in the data folder that the `yoinks` command line talks
//! to. Requests are read on a background thread and handled on the main
//! thread, where all app state lives.

use std::cell::RefCell;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use dispatch2::DispatchQueue;

use super::protocol::{Request, Response};

/// Longest request accepted (a few thousand paths is plenty).
const MAX_REQUEST_BYTES: u64 = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(10);

type Handler = Box<dyn Fn(Request) -> Response>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Sets the main-thread function that answers requests (socket and links).
pub fn set_handler(handler: impl Fn(Request) -> Response + 'static) {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
}

/// Answers a request; must be called on the main thread.
pub fn handle(request: Request) -> Response {
    HANDLER.with(|h| match &*h.borrow() {
        Some(handler) => handler(request),
        None => Response::error("Yoinks is still starting"),
    })
}

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("yoinks.sock")
}

/// Listens on `socket`. Fails if another Yoinks is already listening.
pub fn start(socket: PathBuf) -> io::Result<()> {
    if socket.exists() {
        if UnixStream::connect(&socket).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another Yoinks is already running",
            ));
        }
        // Left behind by a crash.
        fs::remove_file(&socket)?;
    }
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket)?;
    // Only this user may talk to us.
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    thread::Builder::new()
        .name("yoinks-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Err(error) = serve(stream) {
                    eprintln!("yoinks: command failed: {error}");
                }
            }
        })?;
    Ok(())
}

fn serve(stream: UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_BYTES)).read_line(&mut line)?;
    // A client checking whether Yoinks is running connects and closes
    // without a request; there is nobody to answer.
    if line.trim().is_empty() {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => on_main(request),
        Err(error) => Response::error(format!("bad request: {error}")),
    };
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    (&stream).write_all(&out)
}

/// Runs the handler on the main thread and waits for its answer.
fn on_main(request: Request) -> Response {
    let (sender, receiver) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(handle(request));
    });
    receiver
        .recv_timeout(TIMEOUT)
        .unwrap_or_else(|_| Response::error("Yoinks did not answer in time"))
}
