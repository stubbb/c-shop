//! Enough HTTP/1.1 to carry the protocol, and no more.
//!
//! A blocking accept loop with a thread per connection. That is the right
//! shape here rather than an async runtime: the work behind every request is
//! a GPU render that is serialised onto one editor thread anyway, so
//! concurrency above the transport buys nothing and would cost the project
//! its whole dependency tree.
//!
//! Every limit in this file exists because the socket faces a network. Header
//! and body sizes are capped, reads time out, and a connection that stops
//! making sense is dropped rather than reasoned with.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How many requests one connection may make before it is asked to reconnect.
/// Keep-alive is worth having — a client making forty tool calls should not
/// pay for forty handshakes — but an unbounded loop is a way to pin a thread.
const MAX_KEEPALIVE_REQUESTS: usize = 512;

pub struct Request {
    pub method: String,
    pub path: String,
    /// Lowercased names, because HTTP field names are case-insensitive and
    /// every lookup here would otherwise have to remember that.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The path with any query string removed.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type: content_type.to_string(),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response::new(status, "application/json", body)
    }

    pub fn text(status: u16, body: &str) -> Response {
        Response::new(status, "text/plain; charset=utf-8", body.as_bytes().to_vec())
    }

    /// An empty 202, which is what a JSON-RPC notification is owed: it has no
    /// id, so there is nothing to answer with.
    pub fn accepted() -> Response {
        Response { status: 202, content_type: String::new(), headers: Vec::new(), body: Vec::new() }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            413 => "Payload Too Large",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Unknown",
        }
    }

    fn write_to(&self, out: &mut impl Write, keep_alive: bool) -> std::io::Result<()> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason());
        if !self.content_type.is_empty() {
            head.push_str(&format!("Content-Type: {}\r\n", self.content_type));
        }
        head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        head.push_str(if keep_alive {
            "Connection: keep-alive\r\n"
        } else {
            "Connection: close\r\n"
        });
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(&self.body)?;
        out.flush()
    }
}

/// Accept connections until the listener dies, handing each to `handle`.
///
/// `handle` is called from a fresh thread per connection and so must be
/// shareable; everything it needs that is not shareable belongs behind the
/// channel to the editor thread.
pub fn serve<H>(listener: TcpListener, handle: H)
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handle = std::sync::Arc::new(handle);
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            // One refused connection is not a reason to stop serving.
            Err(e) => {
                log::warn!("could not accept a connection: {e}");
                continue;
            }
        };
        let handle = handle.clone();
        std::thread::spawn(move || {
            if let Err(e) = converse(stream, handle.as_ref()) {
                log::debug!("connection ended: {e}");
            }
        });
    }
}

fn converse<H>(stream: TcpStream, handle: &H) -> std::io::Result<()>
where
    H: Fn(&Request) -> Response,
{
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    let _ = stream.set_nodelay(true);
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    for served in 0..MAX_KEEPALIVE_REQUESTS {
        let request = match read_request(&mut reader) {
            Ok(Some(request)) => request,
            // A clean close between requests, which is normal.
            Ok(None) => return Ok(()),
            Err(status) => {
                let response = Response::text(status.0, status.1);
                response.write_to(&mut writer, false)?;
                return Ok(());
            }
        };

        let wants_close = request
            .header("connection")
            .map(|v| v.eq_ignore_ascii_case("close"))
            .unwrap_or(false);
        let keep_alive = !wants_close && served + 1 < MAX_KEEPALIVE_REQUESTS;

        let response = handle(&request);
        response.write_to(&mut writer, keep_alive)?;
        if !keep_alive {
            return Ok(());
        }
    }
    Ok(())
}

/// Read one request, or `None` if the peer closed cleanly before sending one.
///
/// The error carries the status to answer with, because a request that cannot
/// be read is still owed a reply the client can understand.
fn read_request(reader: &mut BufReader<TcpStream>) -> Result<Option<Request>, (u16, &'static str)> {
    let mut head = String::new();
    let mut consumed = 0usize;

    // The request line, skipping the blank line some clients leave behind.
    let (method, path) = loop {
        head.clear();
        match reader.read_line(&mut head) {
            Ok(0) => return Ok(None),
            Ok(n) => consumed += n,
            Err(_) => return Ok(None),
        }
        if consumed > MAX_HEADER_BYTES {
            return Err((431, "the request line is too long"));
        }
        let line = head.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
            return Err((400, "the request line does not parse"));
        };
        break (method.to_string(), path.to_string());
    };

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err((400, "the headers end early")),
            Ok(n) => consumed += n,
            Err(_) => return Err((408, "timed out reading the headers")),
        }
        if consumed > MAX_HEADER_BYTES {
            return Err((431, "the headers are too long"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    // Chunked bodies are not accepted rather than mis-read: every client of
    // this protocol sends a length, and guessing would be worse than refusing.
    if headers.get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        return Err((400, "chunked bodies are not accepted; send a Content-Length"));
    }

    let length: usize = match headers.get("content-length") {
        None => 0,
        Some(v) => v.trim().parse().map_err(|_| (400, "Content-Length does not parse"))?,
    };
    if length > MAX_BODY_BYTES {
        return Err((413, "the body is larger than this server accepts"));
    }

    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return Err((400, "the body is shorter than its Content-Length"));
    }

    Ok(Some(Request { method, path, headers, body }))
}
