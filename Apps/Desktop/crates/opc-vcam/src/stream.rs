//! MJPEG over HTTP on the loopback interface.
//!
//! `GET /stream` is a `multipart/x-mixed-replace` body that never ends: one JPEG part
//! per frame. `GET /frame.jpg` is the latest frame alone, `GET /` a page that shows the
//! stream. OBS reads `/stream` as a Media Source (Local File off, Input Format `mjpeg`)
//! and its Virtual Camera hands it to every other app; VLC, ffmpeg and a browser read
//! it directly. The server binds `127.0.0.1` only — nothing leaves the machine.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{Error, Frame, Sink};

/// The port the settings default to.
pub const DEFAULT_PORT: u16 = 8890;

const BOUNDARY: &str = "opcframe";
const JPEG_QUALITY: u8 = 85;

/// The newest encoded frame, and a count so a reader knows it is new.
#[derive(Debug, Default)]
struct Latest {
    jpeg: Arc<Vec<u8>>,
    seq: u64,
    closing: bool,
}

type Shared = Arc<(Mutex<Latest>, Condvar)>;

/// The listening server. Dropping it closes every connection.
#[derive(Debug)]
pub struct StreamServer {
    address: SocketAddr,
    shared: Shared,
    acceptor: Option<JoinHandle<()>>,
}

impl StreamServer {
    /// Listens on `127.0.0.1:port`; port 0 picks a free one.
    pub fn bind(port: u16) -> Result<Self, Error> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let address = listener.local_addr()?;
        let shared: Shared = Arc::new((Mutex::new(Latest::default()), Condvar::new()));
        let acceptor = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("opc-vcam-http".to_string())
                .spawn(move || accept_loop(&listener, &shared))?
        };
        Ok(Self {
            address,
            shared,
            acceptor: Some(acceptor),
        })
    }

    /// Where it listens.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The stream's URL.
    pub fn url(&self) -> String {
        format!("http://{}/stream", self.address)
    }

    /// Encodes and publishes one frame to every reader.
    pub fn publish(&self, frame: &Frame) -> Result<(), Error> {
        let jpeg = encode(frame)?;
        let (latest, wake) = &*self.shared;
        let mut inside = latest.lock().map_err(|_| Error::Frame("poisoned".into()))?;
        inside.jpeg = Arc::new(jpeg);
        inside.seq += 1;
        wake.notify_all();
        Ok(())
    }
}

impl Sink for StreamServer {
    fn describe(&self) -> String {
        format!("Stream · {}", self.url())
    }

    fn push(&mut self, frame: &Frame) -> Result<(), Error> {
        self.publish(frame)
    }
}

impl Drop for StreamServer {
    fn drop(&mut self) {
        let (latest, wake) = &*self.shared;
        if let Ok(mut inside) = latest.lock() {
            inside.closing = true;
            wake.notify_all();
        }
        // Unblock accept() by knocking once.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(200));
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

fn encode(frame: &Frame) -> Result<Vec<u8>, Error> {
    if !frame.is_sane() {
        return Err(Error::Frame("malformed frame".into()));
    }
    let width = u16::try_from(frame.width).map_err(|_| Error::Frame("too wide".into()))?;
    let height = u16::try_from(frame.height).map_err(|_| Error::Frame("too tall".into()))?;
    let mut out = Vec::with_capacity(frame.rgba.len() / 8);
    jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY)
        .encode(&frame.rgba, width, height, jpeg_encoder::ColorType::Rgba)
        .map_err(|error| Error::Frame(error.to_string()))?;
    Ok(out)
}

fn closing(shared: &Shared) -> bool {
    shared.0.lock().map(|inside| inside.closing).unwrap_or(true)
}

fn accept_loop(listener: &TcpListener, shared: &Shared) {
    for stream in listener.incoming() {
        if closing(shared) {
            break;
        }
        let Ok(stream) = stream else {
            continue;
        };
        let shared = shared.clone();
        let _ = std::thread::Builder::new()
            .name("opc-vcam-reader".to_string())
            .spawn(move || serve(stream, &shared));
    }
}

/// The request's path, or `None` for anything but a GET.
fn request_path(stream: &TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    // Drain the headers so the client sees a clean read.
    let mut header = String::new();
    while reader.read_line(&mut header).ok()? > 2 {
        header.clear();
    }
    (method == "GET").then(|| path.to_string())
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Some(path) = request_path(&stream) else {
        let _ = write!(
            stream,
            "HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n"
        );
        return;
    };
    let result = match path.as_str() {
        "/stream" => serve_stream(&mut stream, shared),
        "/frame.jpg" => serve_frame(&mut stream, shared),
        "/" => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n{PAGE}"
        ),
        _ => write!(stream, "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n"),
    };
    let _ = result;
    let _ = stream.shutdown(Shutdown::Both);
}

const PAGE: &str = "<!doctype html><title>OpenPocketCine</title>\
<body style=\"margin:0;background:#000\"><img src=\"/stream\" style=\"width:100vw;height:100vh;object-fit:contain\"></body>";

/// Waits for a frame newer than `seen`; `None` once the server is closing.
fn next_frame(shared: &Shared, seen: u64) -> Option<(Arc<Vec<u8>>, u64)> {
    let (latest, wake) = &**shared;
    let mut inside = latest.lock().ok()?;
    loop {
        if inside.closing {
            return None;
        }
        if inside.seq > seen {
            return Some((inside.jpeg.clone(), inside.seq));
        }
        let (guard, _) = wake.wait_timeout(inside, Duration::from_millis(500)).ok()?;
        inside = guard;
    }
}

fn serve_stream(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={BOUNDARY}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
    )?;
    let mut seen = 0;
    while let Some((jpeg, seq)) = next_frame(shared, seen) {
        seen = seq;
        write!(
            stream,
            "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            jpeg.len()
        )?;
        stream.write_all(&jpeg)?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    Ok(())
}

fn serve_frame(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    match next_frame(shared, 0) {
        Some((jpeg, _)) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                jpeg.len()
            )?;
            stream.write_all(&jpeg)
        }
        None => write!(
            stream,
            "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn frame() -> Frame {
        let (width, height) = (16u32, 8u32);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[(x * 16) as u8, (y * 32) as u8, 128, 255]);
            }
        }
        Frame {
            width,
            height,
            rgba,
        }
    }

    fn get(address: SocketAddr, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        stream
    }

    fn read_headers(reader: &mut BufReader<TcpStream>) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end().to_string();
            if line.is_empty() {
                return lines;
            }
            lines.push(line);
        }
    }

    fn content_length(headers: &[String]) -> usize {
        headers
            .iter()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|n| n.parse().ok())
            .expect("a length")
    }

    #[test]
    fn the_stream_serves_one_jpeg_part_per_frame() {
        let server = StreamServer::bind(0).unwrap();
        assert!(server.url().starts_with("http://127.0.0.1:"));
        assert_eq!(server.describe(), format!("Stream · {}", server.url()));
        server.publish(&frame()).unwrap();

        let mut reader = BufReader::new(get(server.address(), "/stream"));
        let head = read_headers(&mut reader);
        assert_eq!(head[0], "HTTP/1.1 200 OK");
        assert!(head
            .iter()
            .any(|line| line == "Content-Type: multipart/x-mixed-replace; boundary=opcframe"));

        // First part: the frame published before the reader arrived.
        let part = read_headers(&mut reader);
        assert_eq!(part[0], "--opcframe");
        assert!(part.iter().any(|line| line == "Content-Type: image/jpeg"));
        let mut jpeg = vec![0; content_length(&part)];
        reader.read_exact(&mut jpeg).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG starts with SOI");
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9], "and ends with EOI");

        // Second part only arrives with a second frame.
        server.publish(&frame()).unwrap();
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf).unwrap();
        let part = read_headers(&mut reader);
        assert_eq!(part[0], "--opcframe");
        drop(reader);

        let mut reader = BufReader::new(get(server.address(), "/frame.jpg"));
        let head = read_headers(&mut reader);
        assert_eq!(head[0], "HTTP/1.1 200 OK");
        let mut jpeg = vec![0; content_length(&head)];
        reader.read_exact(&mut jpeg).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);

        let mut reader = BufReader::new(get(server.address(), "/nothing"));
        assert_eq!(read_headers(&mut reader)[0], "HTTP/1.1 404 Not Found");
        let mut reader = BufReader::new(get(server.address(), "/"));
        let head = read_headers(&mut reader);
        assert_eq!(head[0], "HTTP/1.1 200 OK");
        let mut page = String::new();
        reader.read_to_string(&mut page).unwrap();
        assert!(page.contains("/stream"));

        let address = server.address();
        drop(server);
        assert!(
            TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_err(),
            "dropping the server closes the port"
        );
    }

    #[test]
    fn a_malformed_frame_is_refused() {
        let bad = Frame {
            width: 4,
            height: 4,
            rgba: vec![0; 3],
        };
        assert!(matches!(encode(&bad), Err(Error::Frame(_))));
    }
}
