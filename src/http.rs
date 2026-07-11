use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const INDEX: &[u8] = include_bytes!("../web/index.html");
const RESULTS_INDEX: &[u8] = include_bytes!("../web/results.html");
const STYLE: &[u8] = include_bytes!("../web/style.css");
const APP: &[u8] = include_bytes!("../web/app.js");
const RESULTS_APP: &[u8] = include_bytes!("../web/results.js");
static RENDERED_INDEX: OnceLock<String> = OnceLock::new();
static RENDERED_RESULTS_INDEX: OnceLock<String> = OnceLock::new();
const MAX_HEADER: usize = 32 * 1024;
const MAX_UPLOAD: u64 = 512 * 1024 * 1024;
const MAX_DOWNLOAD: usize = 512 * 1024 * 1024;
const MIN_HTTP_WORKERS: usize = 4;
const MAX_HTTP_WORKERS: usize = 64;

pub fn serve(listener: TcpListener) -> io::Result<()> {
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(MIN_HTTP_WORKERS)
        .saturating_mul(4)
        .clamp(MIN_HTTP_WORKERS, MAX_HTTP_WORKERS);
    let (sender, receiver) = sync_channel(worker_count * 4);
    let receiver = Arc::new(Mutex::new(receiver));

    for id in 0..worker_count {
        let receiver = receiver.clone();
        thread::Builder::new()
            .name(format!("http-worker-{id}"))
            .spawn(move || worker(receiver))?;
    }

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => enqueue(&sender, stream)?,
            Err(error) => eprintln!("http accept failed: {error}"),
        }
    }
    Ok(())
}

fn enqueue(sender: &SyncSender<TcpStream>, stream: TcpStream) -> io::Result<()> {
    sender
        .send(stream)
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "HTTP worker pool stopped"))
}

fn worker(receiver: Arc<Mutex<Receiver<TcpStream>>>) {
    loop {
        let stream = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        let Ok(stream) = stream else { return };
        if let Err(error) = handle(stream)
            && !matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            )
        {
            eprintln!("http request failed: {error}");
        }
    }
}

fn handle(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.set_nodelay(true)?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let request_line = match read_line_limited(&mut reader, MAX_HEADER) {
        Ok(line) => line,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return response(&mut stream, 431, "text/plain", b"headers too large", false);
        }
        Err(error) => return Err(error),
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let mut content_length = 0_u64;
    let mut expect_continue = false;
    let mut header_bytes = request_line.len();

    loop {
        let remaining = MAX_HEADER.saturating_sub(header_bytes);
        let line = match read_line_limited(&mut reader, remaining) {
            Ok(line) => line,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                return response(&mut stream, 431, "text/plain", b"headers too large", false);
            }
            Err(error) => return Err(error),
        };
        header_bytes += line.len();
        if header_bytes > MAX_HEADER {
            return response(&mut stream, 431, "text/plain", b"headers too large", false);
        }
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(u64::MAX);
            } else if name.eq_ignore_ascii_case("expect")
                && value.trim().eq_ignore_ascii_case("100-continue")
            {
                expect_continue = true;
            }
        }
    }

    let path = target.split('?').next().unwrap_or(target);
    match (method, path) {
        ("GET", "/") => response(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            rendered_index().as_bytes(),
            false,
        ),
        ("GET", "/results" | "/results/") => response(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            rendered_results_index().as_bytes(),
            false,
        ),
        ("GET", "/style.css") => asset(&mut stream, target, "text/css; charset=utf-8", STYLE),
        ("GET", "/app.js") => asset(&mut stream, target, "text/javascript; charset=utf-8", APP),
        ("GET", "/results.js") => asset(
            &mut stream,
            target,
            "text/javascript; charset=utf-8",
            RESULTS_APP,
        ),
        ("GET", "/api/ping") => response(
            &mut stream,
            200,
            "application/json",
            b"{\"ok\":true}",
            false,
        ),
        ("GET", "/api/download") => download(&mut stream, query_size(target).min(MAX_DOWNLOAD)),
        ("POST", "/api/upload") if content_length <= MAX_UPLOAD => {
            if expect_continue {
                stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            }
            upload(&mut stream, &mut reader, content_length)
        }
        ("POST", "/api/upload") => {
            response(&mut stream, 413, "text/plain", b"upload too large", false)
        }
        _ => response(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"not found",
            false,
        ),
    }
}

fn read_line_limited(reader: &mut impl BufRead, limit: usize) -> io::Result<String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(count) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header line exceeds limit",
            ));
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if line.last() == Some(&b'\n') {
            break;
        }
    }
    String::from_utf8(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HTTP headers are not UTF-8"))
}

fn asset(stream: &mut TcpStream, target: &str, kind: &str, content: &[u8]) -> io::Result<()> {
    match query_value(target, "hash") {
        Some(hash) if hash == content_hash(content) => response(stream, 200, kind, content, true),
        Some(_) => response(stream, 404, "text/plain", b"asset not found", false),
        None => response(stream, 200, kind, content, false),
    }
}

fn rendered_index() -> &'static str {
    RENDERED_INDEX.get_or_init(|| {
        String::from_utf8_lossy(INDEX)
            .replace("__STYLE_HASH__", &content_hash(STYLE))
            .replace("__APP_HASH__", &content_hash(APP))
    })
}

fn rendered_results_index() -> &'static str {
    RENDERED_RESULTS_INDEX.get_or_init(|| {
        String::from_utf8_lossy(RESULTS_INDEX)
            .replace("__STYLE_HASH__", &content_hash(STYLE))
            .replace("__RESULTS_HASH__", &content_hash(RESULTS_APP))
    })
}

fn content_hash(content: &[u8]) -> String {
    // Stable FNV-1a fingerprint; sufficient for browser cache invalidation.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in content {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn response(
    stream: &mut TcpStream,
    status: u16,
    kind: &str,
    body: &[u8],
    cache: bool,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let caching = if cache {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: {caching}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn download(stream: &mut TcpStream, size: usize) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {size}\r\nCache-Control: no-store\r\nContent-Encoding: identity\r\nConnection: close\r\n\r\n"
    )?;
    let block = [0x5a_u8; 128 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let count = remaining.min(block.len());
        stream.write_all(&block[..count])?;
        remaining -= count;
    }
    Ok(())
}

fn upload(stream: &mut TcpStream, reader: &mut impl Read, size: u64) -> io::Result<()> {
    let started = Instant::now();
    let mut received = 0_u64;
    let mut buffer = [0_u8; 128 * 1024];
    while received < size {
        let count = ((size - received) as usize).min(buffer.len());
        let read = reader.read(&mut buffer[..count])?;
        if read == 0 {
            break;
        }
        received += read as u64;
    }
    let json = format!(
        "{{\"bytes\":{received},\"seconds\":{:.6}}}",
        started.elapsed().as_secs_f64()
    );
    response(stream, 200, "application/json", json.as_bytes(), false)
}

fn query_size(target: &str) -> usize {
    query_value(target, "size")
        .and_then(|value| value.parse().ok())
        .unwrap_or(32 * 1024 * 1024)
}

fn query_value<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    target
        .split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.split_once('=').filter(|(name, _)| *name == key))
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn header_line_limit_is_enforced_during_read() {
        let mut reader = BufReader::new(Cursor::new(b"123456789\n"));
        assert_eq!(read_line_limited(&mut reader, 10).unwrap(), "123456789\n");

        let mut reader = BufReader::new(Cursor::new(b"1234567890\n"));
        assert_eq!(
            read_line_limited(&mut reader, 10).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rendered_index_contains_content_hashes() {
        let index = rendered_index();
        assert!(!index.contains("__STYLE_HASH__"));
        assert!(!index.contains("__APP_HASH__"));
        assert!(index.contains(&format!("style.css?hash={}", content_hash(STYLE))));
        assert!(index.contains(&format!("app.js?hash={}", content_hash(APP))));

        let results = rendered_results_index();
        assert!(!results.contains("__STYLE_HASH__"));
        assert!(!results.contains("__RESULTS_HASH__"));
        assert!(results.contains(&format!("results.js?hash={}", content_hash(RESULTS_APP))));
    }

    #[test]
    fn query_values_are_parsed_without_affecting_defaults() {
        assert_eq!(query_size("/api/download?size=42&n=x"), 42);
        assert_eq!(query_size("/api/download?size=bad"), 32 * 1024 * 1024);
        assert_eq!(query_value("/style.css?hash=abc", "hash"), Some("abc"));
    }

    #[test]
    fn content_hash_is_stable_and_content_sensitive() {
        assert_eq!(content_hash(b"speedtest"), "ce343323f90e7164");
        assert_ne!(content_hash(b"speedtest"), content_hash(b"Speedtest"));
    }
}
