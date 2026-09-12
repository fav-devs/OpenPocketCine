//! One TCP join to a sharing host.
//!
//! Reads are non-blocking-ish: a short read timeout lets the caller tick the core's
//! deadlines on schedule instead of sitting in `read` while a silent host stalls.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::buffer::ReceiveBuffer;
use crate::ffi::{self, RelayError};

/// How long a read may block before the caller gets a turn to tick deadlines.
const READ_TIMEOUT: Duration = Duration::from_millis(50);
const READ_CHUNK: usize = 64 * 1024;

/// A complete relay message lifted out of the stream.
#[derive(Debug, Clone)]
pub struct Message {
    pub kind: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Relay(RelayError),
    Closed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Relay(error) => write!(f, "{error}"),
            Self::Closed => write!(f, "the connection closed"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<RelayError> for TransportError {
    fn from(error: RelayError) -> Self {
        Self::Relay(error)
    }
}

/// The watcher's side of one join.
#[derive(Debug)]
pub struct Transport {
    stream: TcpStream,
    buffer: ReceiveBuffer,
    chunk: Vec<u8>,
}

impl Transport {
    /// Tries each advertised address in turn and keeps the first that answers.
    pub fn connect(
        addresses: &[IpAddr],
        port: u16,
        timeout: Duration,
    ) -> Result<Self, TransportError> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address advertised");
        for address in addresses {
            let endpoint = SocketAddr::new(*address, port);
            match TcpStream::connect_timeout(&endpoint, timeout) {
                Ok(stream) => {
                    // Interactive video: never coalesce a frame write behind Nagle.
                    stream.set_nodelay(true)?;
                    stream.set_read_timeout(Some(READ_TIMEOUT))?;
                    return Ok(Self {
                        stream,
                        buffer: ReceiveBuffer::new(),
                        chunk: vec![0; READ_CHUNK],
                    });
                }
                Err(error) => last = error,
            }
        }
        Err(TransportError::Io(last))
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.stream.peer_addr().ok()
    }

    pub fn send(&mut self, kind: u8, payload: &[u8]) -> Result<(), TransportError> {
        let wire = ffi::encode_message(kind, payload)?;
        self.stream.write_all(&wire)?;
        Ok(())
    }

    /// Returns the next complete message, reading from the socket when the buffer does
    /// not already hold one. `Ok(None)` means nothing arrived inside the read timeout.
    ///
    /// The payload is copied out of the receive buffer so the caller can act on it while
    /// holding its own state mutably. At 25 fps on the relay's top rung that is about
    /// 1.3 MB/s of memcpy, which is far below the cost of the decode that follows.
    pub fn next_message(&mut self) -> Result<Option<Message>, TransportError> {
        loop {
            if let Some(message) = ffi::decode_message(self.buffer.readable())? {
                let payload = self.buffer.readable()[message.payload.clone()].to_vec();
                self.buffer.consume(message.consumed);
                return Ok(Some(Message {
                    kind: message.kind,
                    payload,
                }));
            }
            match self.stream.read(&mut self.chunk) {
                Ok(0) => return Err(TransportError::Closed),
                Ok(count) => {
                    let chunk = std::mem::take(&mut self.chunk);
                    self.buffer.extend(&chunk[..count]);
                    self.chunk = chunk;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(TransportError::Io(error)),
            }
        }
    }
}
