//! A minimal HTTP/1.1 client over one configured Unix socket. One connection per request
//! (`Connection: close`). It never reads the environment (`DOCKER_HOST`, contexts, config files)
//! and has no other transport: the socket path it was given is the only thing it can reach.

use std::fmt;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Could not connect (no such socket, refused, permission).
    Connect(String),
    /// The deadline passed.
    Timeout(String),
    /// The peer closed the connection before the response was complete.
    Disconnected(String),
    /// The bytes were not a valid HTTP/1.1 response.
    Malformed(String),
    /// The body exceeded the configured cap; reading stopped.
    TooLarge(usize),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Connect(m) => write!(f, "cannot connect to the Docker socket: {m}"),
            HttpError::Timeout(m) => write!(f, "Docker did not answer in time: {m}"),
            HttpError::Disconnected(m) => {
                write!(f, "Docker closed the connection mid-response: {m}")
            }
            HttpError::Malformed(m) => write!(f, "malformed HTTP response from Docker: {m}"),
            HttpError::TooLarge(n) => write!(f, "Docker response body exceeded {n} bytes"),
        }
    }
}

impl std::error::Error for HttpError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct UnixHttp {
    socket: PathBuf,
    max_body: usize,
}

impl UnixHttp {
    pub fn new(socket: impl Into<PathBuf>) -> UnixHttp {
        UnixHttp {
            socket: socket.into(),
            max_body: 4 * 1024 * 1024,
        }
    }
    pub fn with_max_body(mut self, n: usize) -> UnixHttp {
        self.max_body = n;
        self
    }
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Send one request and read the whole response, all within `timeout` of now.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Response, HttpError> {
        if path.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(HttpError::Malformed(format!(
                "refusing to send a path with whitespace or control bytes: {path:?}"
            )));
        }
        let deadline = Instant::now() + timeout;
        let mut s = UnixStream::connect(&self.socket)
            .map_err(|e| HttpError::Connect(format!("{}: {e}", self.socket.display())))?;
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: docker\r\nUser-Agent: harpoon-deploy\r\nConnection: close\r\nAccept: application/json\r\n");
        if let Some(b) = body {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                b.len()
            ));
        }
        head.push_str("\r\n");
        set_timeouts(&s, deadline)?;
        let write = |s: &mut UnixStream, data: &[u8]| {
            s.write_all(data)
                .map_err(|e| io_err(e, "while sending the request"))
        };
        write(&mut s, head.as_bytes())?;
        if let Some(b) = body {
            write(&mut s, b)?;
        }
        read_response(&mut s, deadline, self.max_body, method == "HEAD")
    }
}

fn io_err(e: std::io::Error, ctx: &str) -> HttpError {
    use std::io::ErrorKind::*;
    match e.kind() {
        WouldBlock | TimedOut => HttpError::Timeout(ctx.to_string()),
        BrokenPipe | ConnectionReset | ConnectionAborted | UnexpectedEof => {
            HttpError::Disconnected(format!("{ctx}: {e}"))
        }
        _ => HttpError::Disconnected(format!("{ctx}: {e}")),
    }
}

fn set_timeouts(s: &UnixStream, deadline: Instant) -> Result<(), HttpError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(HttpError::Timeout("deadline reached".into()));
    }
    // setsockopt can fail with EINVAL on macOS once the peer has closed; reading then simply
    // reports EOF, so a failure here is not an error in itself.
    let _ = s.set_read_timeout(Some(left));
    let _ = s.set_write_timeout(Some(left));
    Ok(())
}

struct Reader<'a> {
    s: &'a mut UnixStream,
    buf: Vec<u8>,
    pos: usize,
    deadline: Instant,
    eof: bool,
}

impl Reader<'_> {
    /// Pull more bytes. `Ok(false)` at clean EOF.
    fn fill(&mut self) -> Result<bool, HttpError> {
        if self.eof {
            return Ok(false);
        }
        set_timeouts(self.s, self.deadline)?;
        let mut chunk = [0u8; 8192];
        loop {
            match self.s.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(e, "while reading the response")),
            }
        }
    }
    fn avail(&self) -> &[u8] {
        &self.buf[self.pos..]
    }
    fn take(&mut self, n: usize) -> Vec<u8> {
        let v = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        v
    }
    fn read_line(&mut self, max: usize) -> Result<String, HttpError> {
        loop {
            if let Some(i) = self.avail().windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.avail()[..i]).into_owned();
                self.pos += i + 2;
                return Ok(line);
            }
            if self.avail().len() > max {
                return Err(HttpError::Malformed("line too long".into()));
            }
            if !self.fill()? {
                return Err(HttpError::Disconnected(
                    "connection closed inside a line".into(),
                ));
            }
        }
    }
    fn read_exact_n(&mut self, n: usize, max: usize) -> Result<Vec<u8>, HttpError> {
        if n > max {
            return Err(HttpError::TooLarge(max));
        }
        while self.avail().len() < n {
            if !self.fill()? {
                return Err(HttpError::Disconnected(format!(
                    "body ended after {} of {} bytes",
                    self.avail().len(),
                    n
                )));
            }
        }
        Ok(self.take(n))
    }
}

fn read_response(
    s: &mut UnixStream,
    deadline: Instant,
    max_body: usize,
    head_only: bool,
) -> Result<Response, HttpError> {
    let mut r = Reader {
        s,
        buf: Vec::new(),
        pos: 0,
        deadline,
        eof: false,
    };
    let status_line = match r.read_line(MAX_HEADER_BYTES) {
        Ok(l) => l,
        Err(HttpError::Disconnected(_)) if r.buf.is_empty() => {
            return Err(HttpError::Disconnected(
                "closed before sending any response".into(),
            ))
        }
        Err(e) => return Err(e),
    };
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HttpError::Malformed(format!(
            "bad status line {status_line:?}"
        )));
    }
    let status: u16 = parts
        .next()
        .and_then(|c| c.parse().ok())
        .filter(|c| (100..600).contains(c))
        .ok_or_else(|| HttpError::Malformed(format!("bad status code in {status_line:?}")))?;
    let mut headers = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let line = r.read_line(MAX_HEADER_BYTES)?;
        if line.is_empty() {
            break;
        }
        header_bytes += line.len();
        if header_bytes > MAX_HEADER_BYTES {
            return Err(HttpError::Malformed("headers too large".into()));
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| HttpError::Malformed(format!("bad header line {line:?}")))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let h = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let no_body = head_only || status == 204 || status == 304 || (100..200).contains(&status);
    let body = if no_body {
        Vec::new()
    } else if h("Transfer-Encoding").map(|v| v.to_ascii_lowercase().contains("chunked"))
        == Some(true)
    {
        let mut body = Vec::new();
        loop {
            let size_line = r.read_line(64)?;
            let size_txt = size_line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_txt, 16)
                .map_err(|_| HttpError::Malformed(format!("bad chunk size {size_line:?}")))?;
            if size == 0 {
                // trailers until the blank line
                loop {
                    if r.read_line(MAX_HEADER_BYTES)?.is_empty() {
                        break;
                    }
                }
                break;
            }
            if body.len() + size > max_body {
                return Err(HttpError::TooLarge(max_body));
            }
            body.extend(r.read_exact_n(size, max_body)?);
            let crlf = r.read_exact_n(2, 2)?;
            if crlf != b"\r\n" {
                return Err(HttpError::Malformed("chunk not terminated by CRLF".into()));
            }
        }
        body
    } else if let Some(len) = h("Content-Length") {
        let n: usize = len
            .parse()
            .map_err(|_| HttpError::Malformed(format!("bad Content-Length {len:?}")))?;
        r.read_exact_n(n, max_body)?
    } else {
        // No framing: the body runs to EOF (we asked for Connection: close).
        loop {
            if r.avail().len() > max_body {
                return Err(HttpError::TooLarge(max_body));
            }
            if !r.fill()? {
                break;
            }
        }
        let n = r.avail().len();
        r.take(n)
    };
    Ok(Response {
        status,
        headers,
        body,
    })
}
