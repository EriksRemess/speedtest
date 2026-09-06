use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const COOKIE_SIZE: usize = 37;
const PARAM_EXCHANGE: i8 = 9;
const CREATE_STREAMS: i8 = 10;
const TEST_START: i8 = 1;
const TEST_RUNNING: i8 = 2;
const TEST_END: i8 = 4;
const EXCHANGE_RESULTS: i8 = 13;
const DISPLAY_RESULTS: i8 = 14;
const IPERF_DONE: i8 = 16;
const SERVER_ERROR: i8 = -2;
const MAX_JSON: usize = 1024 * 1024;
const MAX_STREAMS: usize = 128;
const COOKIE_TIMEOUT: Duration = Duration::from_secs(2);
const PARAMETER_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
struct Parameters {
    streams: usize,
    reverse: bool,
    block_size: usize,
    test_timeout: Duration,
}

#[derive(Clone)]
struct ActiveSession {
    cookie: [u8; COOKIE_SIZE],
    peer_ip: IpAddr,
    streams: SyncSender<TcpStream>,
}

struct WorkerGroup {
    running: Arc<AtomicBool>,
    handles: Option<Vec<JoinHandle<u64>>>,
}

impl WorkerGroup {
    fn new(handles: Vec<JoinHandle<u64>>, running: Arc<AtomicBool>) -> Self {
        Self {
            running,
            handles: Some(handles),
        }
    }

    fn finish(mut self) -> Vec<u64> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Vec<u64> {
        self.running.store(false, Ordering::Relaxed);
        self.handles
            .take()
            .unwrap_or_default()
            .into_iter()
            .map(|worker| worker.join().unwrap_or(0))
            .collect()
    }
}

impl Drop for WorkerGroup {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub fn serve(listener: TcpListener) -> io::Result<()> {
    let active = Arc::new(Mutex::new(None::<ActiveSession>));
    loop {
        let (mut stream, peer) = listener.accept()?;
        stream.set_read_timeout(Some(COOKIE_TIMEOUT))?;
        let mut cookie = [0_u8; COOKIE_SIZE];
        if let Err(error) =
            read_exact_until(&mut stream, &mut cookie, Instant::now() + COOKIE_TIMEOUT)
        {
            eprintln!("iperf3 cookie from {peer} failed: {error}");
            continue;
        }

        let mut current = active
            .lock()
            .map_err(|_| io::Error::other("iperf3 session lock poisoned"))?;
        if let Some(session) = current.as_ref() {
            if session.cookie == cookie && session.peer_ip == peer.ip() {
                match session.streams.try_send(stream) {
                    Ok(()) => {}
                    Err(
                        TrySendError::Full(mut stream) | TrySendError::Disconnected(mut stream),
                    ) => {
                        let _ = stream.write_all(&[0xff]);
                    }
                }
            } else {
                let _ = stream.write_all(&[0xff]);
            }
            continue;
        }

        let (sender, receiver) = sync_channel(MAX_STREAMS);
        *current = Some(ActiveSession {
            cookie,
            peer_ip: peer.ip(),
            streams: sender,
        });
        drop(current);

        let active_for_session = active.clone();
        thread::Builder::new()
            .name(format!("iperf3-session-{peer}"))
            .spawn(move || {
                if let Err(error) = session(stream, peer, receiver) {
                    eprintln!("iperf3 session from {peer} failed: {error}");
                }
                if let Ok(mut current) = active_for_session.lock()
                    && current.as_ref().is_some_and(|session| {
                        session.cookie == cookie && session.peer_ip == peer.ip()
                    })
                {
                    *current = None;
                }
            })?;
    }
}

fn session(
    mut control: TcpStream,
    peer: SocketAddr,
    streams: Receiver<TcpStream>,
) -> io::Result<()> {
    control.set_nodelay(true)?;
    control.set_read_timeout(Some(PARAMETER_TIMEOUT))?;
    control.set_write_timeout(Some(Duration::from_secs(30)))?;

    send_state(&mut control, PARAM_EXCHANGE)?;
    let params_json = read_json(&mut control)?;
    let params = match parse_parameters(&params_json) {
        Ok(params) => params,
        Err(error) => {
            let _ = send_server_error(&mut control, 13, error.raw_os_error().unwrap_or(0));
            return Err(error);
        }
    };
    send_state(&mut control, CREATE_STREAMS)?;

    let mut data = Vec::with_capacity(params.streams);
    let deadline = Instant::now() + STREAM_TIMEOUT;
    while data.len() < params.streams {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for iperf3 data streams",
            ));
        }
        let stream = streams.recv_timeout(remaining).map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for iperf3 data streams",
            )
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        data.push(stream);
    }

    control.set_read_timeout(Some(params.test_timeout))?;
    send_state(&mut control, TEST_START)?;
    send_state(&mut control, TEST_RUNNING)?;
    let running = Arc::new(AtomicBool::new(true));
    let started = Instant::now();
    let workers = data
        .into_iter()
        .map(|stream| {
            let running = running.clone();
            let size = params.block_size;
            if params.reverse {
                thread::spawn(move || send_data(stream, running, size))
            } else {
                thread::spawn(move || receive_data(stream, running, size))
            }
        })
        .collect();
    let workers = WorkerGroup::new(workers, running.clone());

    let mut state = [0_u8; 1];
    control.read_exact(&mut state)?;
    if state[0] as i8 != TEST_END {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected TEST_END, got {}", state[0] as i8),
        ));
    }
    let seconds = started.elapsed().as_secs_f64();
    let bytes = workers.finish();

    control.set_read_timeout(Some(PARAMETER_TIMEOUT))?;
    send_state(&mut control, EXCHANGE_RESULTS)?;
    let _client_results = read_json(&mut control)?;
    write_json(&mut control, &results_json(&bytes, seconds, params.reverse))?;
    send_state(&mut control, DISPLAY_RESULTS)?;
    control.read_exact(&mut state)?;
    if state[0] as i8 != IPERF_DONE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected IPERF_DONE, got {}", state[0] as i8),
        ));
    }
    let total: u64 = bytes.iter().sum();
    println!(
        "  iperf3  {peer}  {}  {:.2} Gbit/s",
        if params.reverse { "reverse" } else { "forward" },
        total as f64 * 8.0 / seconds / 1e9
    );
    Ok(())
}

fn receive_data(mut stream: TcpStream, running: Arc<AtomicBool>, block_size: usize) -> u64 {
    let mut buffer = vec![0_u8; block_size.clamp(1024, 1024 * 1024)];
    let mut total = 0_u64;
    while running.load(Ordering::Relaxed) {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => total += count as u64,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    total
}

fn send_data(mut stream: TcpStream, running: Arc<AtomicBool>, block_size: usize) -> u64 {
    let buffer = vec![0_u8; block_size.clamp(1024, 1024 * 1024)];
    let mut total = 0_u64;
    while running.load(Ordering::Relaxed) {
        match stream.write(&buffer) {
            Ok(0) => break,
            Ok(count) => total += count as u64,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    total
}

fn parse_parameters(json: &str) -> io::Result<Parameters> {
    let fields = parse_object_fields(json)?;
    if json_bool(&fields, "tcp")? != Some(true) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "only iperf3 TCP tests are supported",
        ));
    }
    if json_bool(&fields, "bidirectional")? == Some(true) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bidirectional tests are not supported; run forward and reverse separately",
        ));
    }
    let streams = json_number(&fields, "parallel")?.unwrap_or(1);
    if streams < 1 || streams > MAX_STREAMS as i64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid parallel stream count",
        ));
    }
    let block_size = json_number(&fields, "len")?.unwrap_or(128 * 1024);
    if block_size < 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid iperf3 block size",
        ));
    }
    let time = json_number(&fields, "time")?.unwrap_or(10);
    let omit = json_number(&fields, "omit")?.unwrap_or(0);
    let duration = time
        .checked_add(omit)
        .filter(|duration| time >= 0 && omit >= 0 && *duration <= 86_400)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "test duration must be between 0 and 86400 seconds including omit",
            )
        })?;
    Ok(Parameters {
        streams: streams as usize,
        reverse: json_bool(&fields, "reverse")?.unwrap_or(false),
        block_size: block_size.clamp(1024, 1024 * 1024) as usize,
        test_timeout: Duration::from_secs((duration as u64 + 30).max(120)),
    })
}

fn json_bool(fields: &[(&str, &str)], key: &str) -> io::Result<Option<bool>> {
    fields
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| match *value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("iperf3 parameter {key} must be a boolean"),
            )),
        })
        .transpose()
}

fn json_number(fields: &[(&str, &str)], key: &str) -> io::Result<Option<i64>> {
    fields
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| {
            value.parse::<i64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("iperf3 parameter {key} must be an integer"),
                )
            })
        })
        .transpose()
}

fn parse_object_fields(json: &str) -> io::Result<Vec<(&str, &str)>> {
    let bytes = json.as_bytes();
    let mut position = skip_whitespace(bytes, 0);
    if bytes.get(position) != Some(&b'{') {
        return Err(invalid_json());
    }
    position += 1;
    let mut fields = Vec::new();

    loop {
        position = skip_whitespace(bytes, position);
        if bytes.get(position) == Some(&b'}') {
            position += 1;
            break;
        }
        let (key_start, key_end, next) = parse_string(bytes, position)?;
        position = skip_whitespace(bytes, next);
        if bytes.get(position) != Some(&b':') {
            return Err(invalid_json());
        }
        position = skip_whitespace(bytes, position + 1);
        let value_start = position;
        position = skip_json_value(bytes, position)?;
        let value = json[value_start..position].trim();
        let key = &json[key_start..key_end];
        fields.push((key, value));

        position = skip_whitespace(bytes, position);
        match bytes.get(position) {
            Some(b',') => {
                position = skip_whitespace(bytes, position + 1);
                if bytes.get(position) == Some(&b'}') {
                    return Err(invalid_json());
                }
            }
            Some(b'}') => {
                position += 1;
                break;
            }
            _ => return Err(invalid_json()),
        }
    }

    if skip_whitespace(bytes, position) != bytes.len() {
        return Err(invalid_json());
    }
    Ok(fields)
}

fn parse_string(bytes: &[u8], position: usize) -> io::Result<(usize, usize, usize)> {
    if bytes.get(position) != Some(&b'"') {
        return Err(invalid_json());
    }
    let start = position + 1;
    let mut cursor = start;
    while let Some(byte) = bytes.get(cursor) {
        match byte {
            b'"' => return Ok((start, cursor, cursor + 1)),
            b'\\' => {
                cursor += 1;
                if bytes.get(cursor).is_none() {
                    return Err(invalid_json());
                }
            }
            0x00..=0x1f => return Err(invalid_json()),
            _ => {}
        }
        cursor += 1;
    }
    Err(invalid_json())
}

fn skip_json_value(bytes: &[u8], position: usize) -> io::Result<usize> {
    match bytes.get(position) {
        Some(b'"') => parse_string(bytes, position).map(|(_, _, next)| next),
        Some(b'{') | Some(b'[') => skip_nested_value(bytes, position),
        Some(_) => {
            let mut cursor = position;
            while let Some(byte) = bytes.get(cursor) {
                if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}' | b']') {
                    break;
                }
                cursor += 1;
            }
            let token =
                std::str::from_utf8(&bytes[position..cursor]).map_err(|_| invalid_json())?;
            if token == "true"
                || token == "false"
                || token == "null"
                || token.parse::<f64>().is_ok()
            {
                Ok(cursor)
            } else {
                Err(invalid_json())
            }
        }
        None => Err(invalid_json()),
    }
}

fn skip_nested_value(bytes: &[u8], position: usize) -> io::Result<usize> {
    let mut expected = Vec::new();
    let mut cursor = position;
    while let Some(byte) = bytes.get(cursor) {
        match byte {
            b'"' => cursor = parse_string(bytes, cursor)?.2,
            b'{' => {
                expected.push(b'}');
                cursor += 1;
            }
            b'[' => {
                expected.push(b']');
                cursor += 1;
            }
            b'}' | b']' if expected.pop() == Some(*byte) => {
                cursor += 1;
                if expected.is_empty() {
                    return Ok(cursor);
                }
            }
            b'}' | b']' => return Err(invalid_json()),
            _ => cursor += 1,
        }
    }
    Err(invalid_json())
}

fn skip_whitespace(bytes: &[u8], mut position: usize) -> usize {
    while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    position
}

fn invalid_json() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid iperf3 JSON parameters")
}

fn results_json(bytes: &[u64], seconds: f64, sender: bool) -> String {
    // iperf3 has historically numbered parallel streams 1, 3, 4, 5... .
    // Reproducing that oddity is required because the client validates IDs.
    let streams = bytes.iter().enumerate().map(|(index, bytes)| {
        let id = if index == 0 { 1 } else { index + 2 };
        format!("{{\"id\":{id},\"bytes\":{bytes},\"retransmits\":{},\"jitter\":0,\"errors\":0,\"omitted_errors\":0,\"packets\":0,\"omitted_packets\":0,\"start_time\":0,\"end_time\":{seconds:.6}}}", if sender { 0 } else { -1 })
    }).collect::<Vec<_>>().join(",");
    format!(
        "{{\"cpu_util_total\":0,\"cpu_util_user\":0,\"cpu_util_system\":0,\"sender_has_retransmits\":{},\"streams\":[{streams}]}}",
        if sender { 0 } else { -1 }
    )
}

fn send_state(stream: &mut TcpStream, state: i8) -> io::Result<()> {
    stream.write_all(&[state as u8])
}

fn send_server_error(stream: &mut TcpStream, code: i32, os_error: i32) -> io::Result<()> {
    send_state(stream, SERVER_ERROR)?;
    stream.write_all(&code.to_be_bytes())?;
    stream.write_all(&os_error.to_be_bytes())
}

// Bound the entire frame, even when a peer sends one byte per socket timeout.
fn read_exact_until(
    stream: &mut TcpStream,
    mut buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !buffer.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "iperf3 frame deadline exceeded",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete iperf3 frame",
                ));
            }
            Ok(count) => buffer = &mut buffer[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_json(stream: &mut TcpStream) -> io::Result<String> {
    let deadline = Instant::now() + stream.read_timeout()?.unwrap_or(PARAMETER_TIMEOUT);
    let mut length = [0_u8; 4];
    read_exact_until(stream, &mut length, deadline)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_JSON {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid iperf3 JSON frame length",
        ));
    }
    let mut data = vec![0_u8; length];
    read_exact_until(stream, &mut data, deadline)?;
    String::from_utf8(data)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "iperf3 JSON is not UTF-8"))
}

fn write_json(stream: &mut TcpStream, json: &str) -> io::Result<()> {
    let length = u32::try_from(json.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "JSON frame too large"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_parameters() {
        let parameters =
            parse_parameters(r#"{"tcp":true,"parallel":4,"reverse":true,"len":65536,"time":10}"#)
                .unwrap();
        assert_eq!(parameters.streams, 4);
        assert!(parameters.reverse);
        assert_eq!(parameters.block_size, 65536);
    }

    #[test]
    fn honors_long_test_duration_and_rejects_invalid_limits() {
        let parameters = parse_parameters(r#"{"tcp":true,"time":300,"omit":5}"#).unwrap();
        assert_eq!(parameters.test_timeout, Duration::from_secs(335));
        for json in [
            r#"{"tcp":true,"time":-1}"#,
            r#"{"tcp":true,"omit":-1}"#,
            r#"{"tcp":true,"time":86401}"#,
            r#"{"tcp":true,"time":9223372036854775807,"omit":1}"#,
        ] {
            assert!(parse_parameters(json).is_err());
        }
    }

    #[test]
    fn expired_frame_deadline_prevents_reading() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_exact_until(&mut stream, &mut [0], Instant::now())
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn ignores_key_text_inside_strings_and_nested_values() {
        let parameters = parse_parameters(
            r#"{"note":"\"parallel\":99","nested":{"parallel":88},"tcp":true,"parallel":2}"#,
        )
        .unwrap();
        assert_eq!(parameters.streams, 2);
    }

    #[test]
    fn rejects_negative_and_malformed_parameters() {
        assert!(parse_parameters(r#"{"tcp":true,"parallel":-1}"#).is_err());
        assert!(parse_parameters(r#"{"tcp":true,"len":-1}"#).is_err());
        assert!(parse_parameters(r#"{"tcp":true,}"#).is_err());
    }

    #[test]
    fn dropping_workers_stops_and_joins_them() {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = running.clone();
        let handle = thread::spawn(move || {
            while worker_running.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            7
        });
        drop(WorkerGroup::new(vec![handle], running.clone()));
        assert!(!running.load(Ordering::Relaxed));
    }

    #[test]
    fn uses_iperf_legacy_parallel_stream_ids() {
        let json = results_json(&[1, 2, 3, 4], 1.0, false);
        assert!(json.contains(r#""id":1"#));
        assert!(json.contains(r#""id":3"#));
        assert!(json.contains(r#""id":4"#));
        assert!(json.contains(r#""id":5"#));
        assert!(!json.contains(r#""id":2"#));
    }
}
