use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};

const IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_RESPONSE_LEN: usize = 1024 * 1024;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Send a v2 request to the jmux socket and return the response.
///
/// When `window_id` is `Some`, it is included as a top-level `"window_id"` field
/// in the request so the server can route the command to the correct window.
///
/// Retries transient connection failures (EAGAIN, ECONNREFUSED) up to 3 times
/// with 100ms backoff to handle startup races and momentary unavailability.
pub fn send_request(
    socket_path: &str,
    method: &str,
    params: Value,
    window_id: Option<&str>,
) -> anyhow::Result<Value> {
    let mut stream = connect_with_retry(socket_path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let mut request = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });
    if let Some(wid) = window_id {
        request["window_id"] = serde_json::json!(wid);
    }

    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let limited = (&stream).take((MAX_RESPONSE_LEN + 1) as u64);
    let mut reader = BufReader::new(limited);
    let mut line = String::new();
    let bytes_read = reader.read_line(&mut line)?;
    if bytes_read == 0 {
        anyhow::bail!("jmux closed socket without a response");
    }
    if line.len() > MAX_RESPONSE_LEN {
        anyhow::bail!("jmux response exceeded {} bytes", MAX_RESPONSE_LEN);
    }

    let response: Value = serde_json::from_str(line.trim())?;
    Ok(response)
}

/// Connect to the jmux socket, retrying transient errors.
fn connect_with_retry(socket_path: &str) -> anyhow::Result<UnixStream> {
    let mut last_err = None;
    for attempt in 0..3 {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                let retryable = matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::WouldBlock
                );
                last_err = Some(e);
                if !retryable || attempt == 2 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Err(anyhow::anyhow!(
        "Cannot connect to jmux at {}: {}",
        socket_path,
        last_err.unwrap()
    ))
}

pub fn default_socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = std::path::Path::new(&dir);
        if path.is_absolute() {
            if let Ok(meta) = std::fs::metadata(path) {
                let my_uid = unsafe { libc::getuid() };
                if meta.is_dir() && meta.uid() == my_uid && (meta.mode() & 0o777) == 0o700 {
                    return format!("{}/jmux.sock", dir);
                }
            }
        }
    }

    format!("/tmp/jmux-{}.sock", unsafe { libc::getuid() })
}
