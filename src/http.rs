use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const INDEX: &[u8] = include_bytes!("../web/index.html");
const RESULTS_INDEX: &[u8] = include_bytes!("../web/results.html");
const STYLE: &[u8] = include_bytes!("../web/style.css");
const LOCALE_EN: &[u8] = include_bytes!("../web/locales/en.js");
const LOCALE_LV: &[u8] = include_bytes!("../web/locales/lv.js");
const I18N_APP: &[u8] = include_bytes!("../web/i18n.js");
const CHART_APP: &[u8] = include_bytes!("../web/chart.js");
const APP: &[u8] = include_bytes!("../web/app.js");
const RESULTS_APP: &[u8] = include_bytes!("../web/results.js");
static RENDERED_INDEX: OnceLock<String> = OnceLock::new();
static RENDERED_RESULTS_INDEX: OnceLock<String> = OnceLock::new();
const MAX_HEADER: usize = 32 * 1024;
const MAX_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;
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

// Socket timeouts alone reset on every read/write, allowing trickle traffic
// to occupy every worker indefinitely. Share one deadline for the whole request.
struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP request deadline exceeded",
            ));
        }
        Ok(remaining)
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(buffer)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn handle(stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.set_nodelay(true)?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut reader = BufReader::new(DeadlineStream {
        stream: stream.try_clone()?,
        deadline,
    });
    let mut stream = DeadlineStream { stream, deadline };
    let request_line = match read_line_limited(&mut reader, MAX_HEADER) {
        Ok(line) => line,
        Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
            return response(&mut stream, 431, "text/plain", b"headers too large", false);
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
            ) =>
        {
            return response(&mut stream, 400, "text/plain", b"invalid request", false);
        }
        Err(error) => return Err(error),
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || parts.next().is_some()
        || !target.starts_with('/')
    {
        return response(
            &mut stream,
            400,
            "text/plain",
            b"invalid request line",
            false,
        );
    }
    let mut content_length = None;
    let mut chunked_body = false;
    let mut expect_continue = false;
    let mut header_bytes = request_line.len();

    loop {
        let remaining = MAX_HEADER.saturating_sub(header_bytes);
        let line = match read_line_limited(&mut reader, remaining) {
            Ok(line) => line,
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
                return response(&mut stream, 431, "text/plain", b"headers too large", false);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
                ) =>
            {
                return response(&mut stream, 400, "text/plain", b"invalid headers", false);
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
        let Some((name, value)) = line.split_once(':') else {
            return response(&mut stream, 400, "text/plain", b"invalid header", false);
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return response(
                &mut stream,
                400,
                "text/plain",
                b"invalid header name",
                false,
            );
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
            {
                return response(
                    &mut stream,
                    400,
                    "text/plain",
                    b"invalid content length",
                    false,
                );
            }
            content_length = match value.parse::<u64>() {
                Ok(length) => Some(length),
                Err(_) => {
                    return response(
                        &mut stream,
                        400,
                        "text/plain",
                        b"invalid content length",
                        false,
                    );
                }
            };
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked_body || !value.eq_ignore_ascii_case("chunked") || version != "HTTP/1.1" {
                return response(
                    &mut stream,
                    400,
                    "text/plain",
                    b"unsupported transfer encoding",
                    false,
                );
            }
            chunked_body = true;
        } else if name.eq_ignore_ascii_case("expect") {
            if !value.eq_ignore_ascii_case("100-continue") {
                return response(
                    &mut stream,
                    417,
                    "text/plain",
                    b"unsupported expectation",
                    false,
                );
            }
            expect_continue = true;
        }
    }
    if chunked_body && content_length.is_some() {
        return response(
            &mut stream,
            400,
            "text/plain",
            b"ambiguous body framing",
            false,
        );
    }
    let content_length = content_length.unwrap_or(0);

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
        ("GET", "/locales/en.js") => asset(
            &mut stream,
            target,
            "text/javascript; charset=utf-8",
            LOCALE_EN,
        ),
        ("GET", "/locales/lv.js") => asset(
            &mut stream,
            target,
            "text/javascript; charset=utf-8",
            LOCALE_LV,
        ),
        ("GET", "/i18n.js") => asset(
            &mut stream,
            target,
            "text/javascript; charset=utf-8",
            I18N_APP,
        ),
        ("GET", "/chart.js") => asset(
            &mut stream,
            target,
            "text/javascript; charset=utf-8",
            CHART_APP,
        ),
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
        ("GET", "/health") => response(
            &mut stream,
            200,
            "application/json",
            b"{\"status\":\"ok\"}",
            false,
        ),
        ("GET", "/api/download") => download(&mut stream, query_size(target).min(MAX_DOWNLOAD)),
        ("POST", "/api/upload") if chunked_body => {
            if expect_continue {
                stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            }
            upload_chunked(&mut stream, &mut reader)
        }
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
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete HTTP line",
            ));
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(count) > limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
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

fn asset(stream: &mut impl Write, target: &str, kind: &str, content: &[u8]) -> io::Result<()> {
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
            .replace("__LOCALE_EN_HASH__", &content_hash(LOCALE_EN))
            .replace("__LOCALE_LV_HASH__", &content_hash(LOCALE_LV))
            .replace("__I18N_HASH__", &content_hash(I18N_APP))
            .replace("__CHART_HASH__", &content_hash(CHART_APP))
            .replace("__APP_HASH__", &content_hash(APP))
    })
}

fn rendered_results_index() -> &'static str {
    RENDERED_RESULTS_INDEX.get_or_init(|| {
        String::from_utf8_lossy(RESULTS_INDEX)
            .replace("__STYLE_HASH__", &content_hash(STYLE))
            .replace("__LOCALE_EN_HASH__", &content_hash(LOCALE_EN))
            .replace("__LOCALE_LV_HASH__", &content_hash(LOCALE_LV))
            .replace("__I18N_HASH__", &content_hash(I18N_APP))
            .replace("__CHART_HASH__", &content_hash(CHART_APP))
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
    stream: &mut impl Write,
    status: u16,
    kind: &str,
    body: &[u8],
    cache: bool,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        417 => "Expectation Failed",
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

fn download(stream: &mut impl Write, size: usize) -> io::Result<()> {
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

fn upload(stream: &mut impl Write, reader: &mut impl Read, size: u64) -> io::Result<()> {
    let started = Instant::now();
    let mut received = 0_u64;
    let mut buffer = [0_u8; 128 * 1024];
    while received < size {
        let count = ((size - received) as usize).min(buffer.len());
        let read = reader.read(&mut buffer[..count])?;
        if read == 0 {
            return response(stream, 400, "text/plain", b"incomplete upload", false);
        }
        received += read as u64;
    }
    upload_response(stream, received, started.elapsed().as_secs_f64())
}

fn upload_chunked(stream: &mut impl Write, reader: &mut impl BufRead) -> io::Result<()> {
    let started = Instant::now();
    let received = match read_chunked_body(reader) {
        Ok(received) => received,
        Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
            return response(stream, 413, "text/plain", b"upload too large", false);
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
            ) =>
        {
            return response(stream, 400, "text/plain", b"invalid chunked upload", false);
        }
        Err(error) => return Err(error),
    };
    upload_response(stream, received, started.elapsed().as_secs_f64())
}

fn read_chunked_body(reader: &mut impl BufRead) -> io::Result<u64> {
    let mut received = 0_u64;
    let mut buffer = [0_u8; 128 * 1024];

    loop {
        let line = read_line_limited(reader, 128)?;
        let size = line
            .trim()
            .split(';')
            .next()
            .and_then(|value| u64::from_str_radix(value, 16).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        if size == 0 {
            let mut remaining = MAX_HEADER;
            loop {
                let trailer = read_line_limited(reader, remaining)?;
                remaining -= trailer.len();
                if trailer == "\r\n" || trailer == "\n" {
                    break;
                }
            }
            break;
        }
        if received.saturating_add(size) > MAX_UPLOAD {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "upload too large",
            ));
        }

        let mut remaining = size;
        while remaining > 0 {
            let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            reader.read_exact(&mut buffer[..count])?;
            received += count as u64;
            remaining -= count as u64;
        }
        let mut terminator = [0_u8; 2];
        reader.read_exact(&mut terminator)?;
        if terminator != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
    }

    Ok(received)
}

fn upload_response(stream: &mut impl Write, received: u64, seconds: f64) -> io::Result<()> {
    let json = format!("{{\"bytes\":{received},\"seconds\":{:.6}}}", seconds);
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

    fn request(raw: &[u8]) -> String {
        use std::net::Shutdown;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let worker = thread::spawn(move || handle(server));
        client.write_all(raw).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        worker.join().unwrap().unwrap();
        response
    }

    #[test]
    fn rejects_ambiguous_and_invalid_request_framing() {
        for headers in [
            "Content-Length: 1\r\nContent-Length: 2",
            "Content-Length: 0\r\nTransfer-Encoding: chunked",
            "Content-Length: +1",
            "Content-Length: nope",
            "Content-Length : 0",
            "Transfer-Encoding: chunked, gzip",
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
            "Invalid header",
        ] {
            let raw = format!("POST /api/upload HTTP/1.1\r\n{headers}\r\n\r\n");
            assert!(
                request(raw.as_bytes()).starts_with("HTTP/1.1 400"),
                "{headers}"
            );
        }
        assert!(request(b"GET /health\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(request(b"GET /health HTTP/1.1\r\n").starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn only_complete_uploads_succeed() {
        assert!(
            request(b"POST /api/upload HTTP/1.1\r\nContent-Length: 5\r\n\r\nabc")
                .starts_with("HTTP/1.1 400")
        );
        let fixed = request(b"POST /api/upload HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc");
        assert!(fixed.starts_with("HTTP/1.1 200"));
        assert!(fixed.contains("\"bytes\":3"));
        let chunked = request(
            b"POST /api/upload HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        );
        assert!(chunked.starts_with("HTTP/1.1 200"));
        assert!(chunked.contains("\"bytes\":3"));
    }

    #[test]
    fn chunked_trailers_are_bounded_and_must_terminate() {
        for raw in [b"0\r\n".as_slice(), b"0\r\nX-Test: yes\r\n", b"3\r\nab"] {
            assert!(read_chunked_body(&mut Cursor::new(raw)).is_err());
        }
        let raw = format!("0\r\n{}\r\n", "X-Test: value\r\n".repeat(MAX_HEADER / 10));
        assert_eq!(
            read_chunked_body(&mut Cursor::new(raw)).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
    }

    #[test]
    fn expired_request_deadline_prevents_further_io() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut stream = DeadlineStream {
            stream,
            deadline: Instant::now(),
        };
        assert_eq!(
            stream.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            stream.write(b"x").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn header_line_limit_is_enforced_during_read() {
        let mut reader = BufReader::new(Cursor::new(b"123456789\n"));
        assert_eq!(read_line_limited(&mut reader, 10).unwrap(), "123456789\n");

        let mut reader = BufReader::new(Cursor::new(b"1234567890\n"));
        assert_eq!(
            read_line_limited(&mut reader, 10).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
    }

    #[test]
    fn rendered_index_contains_content_hashes() {
        let index = rendered_index();
        assert!(!index.contains("__STYLE_HASH__"));
        assert!(!index.contains("__LOCALE_EN_HASH__"));
        assert!(!index.contains("__LOCALE_LV_HASH__"));
        assert!(!index.contains("__I18N_HASH__"));
        assert!(!index.contains("__CHART_HASH__"));
        assert!(!index.contains("__APP_HASH__"));
        assert!(index.contains(&format!("style.css?hash={}", content_hash(STYLE))));
        assert!(index.contains(&format!("locales/en.js?hash={}", content_hash(LOCALE_EN))));
        assert!(index.contains(&format!("locales/lv.js?hash={}", content_hash(LOCALE_LV))));
        assert!(index.contains(&format!("i18n.js?hash={}", content_hash(I18N_APP))));
        assert!(index.contains(&format!("chart.js?hash={}", content_hash(CHART_APP))));
        assert!(index.contains(&format!("app.js?hash={}", content_hash(APP))));

        let results = rendered_results_index();
        assert!(!results.contains("__STYLE_HASH__"));
        assert!(!results.contains("__LOCALE_EN_HASH__"));
        assert!(!results.contains("__LOCALE_LV_HASH__"));
        assert!(!results.contains("__I18N_HASH__"));
        assert!(!results.contains("__CHART_HASH__"));
        assert!(!results.contains("__RESULTS_HASH__"));
        assert!(results.contains(&format!("locales/en.js?hash={}", content_hash(LOCALE_EN))));
        assert!(results.contains(&format!("locales/lv.js?hash={}", content_hash(LOCALE_LV))));
        assert!(results.contains(&format!("i18n.js?hash={}", content_hash(I18N_APP))));
        assert!(results.contains(&format!("chart.js?hash={}", content_hash(CHART_APP))));
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

    #[test]
    fn decodes_chunked_upload_bodies() {
        let body = b"4\r\ntest\r\n6;extension=yes\r\nstream\r\n0\r\nX-Test: yes\r\n\r\n";
        let mut reader = BufReader::new(Cursor::new(body));
        assert_eq!(read_chunked_body(&mut reader).unwrap(), 10);

        let mut invalid = BufReader::new(Cursor::new(b"4\r\ntestXX0\r\n\r\n"));
        assert!(read_chunked_body(&mut invalid).is_err());
    }
}
