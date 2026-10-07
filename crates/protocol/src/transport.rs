//! Message transports. Every connection is a pair of channels; TCP connections pump
//! those channels from reader and writer threads, so game code never touches sockets.
//!
//! Every message carries when it arrived: when it was sent, in process, or read off the
//! socket, over TCP. Never when the game got round to it: a server busy with a long tick
//! judges a player's timed actions (how long a block took to break) by when the player's
//! messages came, not by when it read them (see the server's `actions`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, unbounded};

use crate::{ClientMessage, ServerMessage};

/// The peer closed the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

/// One end of a bidirectional message stream.
pub struct Connection<Out, In> {
    tx: Sender<(Instant, Out)>,
    rx: Receiver<(Instant, In)>,
    local: bool,
    /// Whether the peer is on this machine: in process, or over TCP from a loopback
    /// address.
    loopback: bool,
    /// Set when the outgoing queue filled up because the peer stopped reading: the
    /// connection then counts as closed (see `send`).
    overflowed: AtomicBool,
}

pub type ClientConnection = Connection<ClientMessage, ServerMessage>;
pub type ServerConnection = Connection<ServerMessage, ClientMessage>;

impl<Out, In> Connection<Out, In> {
    /// Queues a message. Returns false once the peer has gone away. A TCP peer that lets
    /// its queue fill (it stopped reading) is cut off: the connection then reads as
    /// disconnected, rather than holding ever more messages for it.
    pub fn send(&self, message: Out) -> bool {
        match self.tx.try_send((Instant::now(), message)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                if !self.overflowed.swap(true, Ordering::Relaxed) {
                    tracing::warn!("peer is not reading its messages, closing the connection");
                }
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// The next received message, `Ok(None)` if none is pending, `Err` when disconnected.
    pub fn try_recv(&self) -> Result<Option<In>, Disconnected> {
        Ok(self.try_recv_at()?.map(|(_, message)| message))
    }

    /// The next received message with when it arrived (see the module docs).
    pub fn try_recv_at(&self) -> Result<Option<(Instant, In)>, Disconnected> {
        if self.overflowed.load(Ordering::Relaxed) {
            return Err(Disconnected);
        }
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Disconnected),
        }
    }

    /// Whether this is an in-process connection, which only the integrated server's own
    /// client can make.
    pub fn is_local(&self) -> bool {
        self.local
    }

    /// Whether the peer is on this machine (in process, or over TCP from a loopback
    /// address), which a server may trust with what it would not give a stranger.
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    fn new(tx: Sender<(Instant, Out)>, rx: Receiver<(Instant, In)>, local: bool, loopback: bool) -> Self {
        Self { tx, rx, local, loopback, overflowed: AtomicBool::new(false) }
    }
}

/// An in-process connection, used by singleplayer's integrated server.
pub fn local_pair() -> (ClientConnection, ServerConnection) {
    let (client_tx, server_rx) = unbounded();
    let (server_tx, client_rx) = unbounded();
    (Connection::new(client_tx, client_rx, true, true), Connection::new(server_tx, server_rx, true, true))
}

pub mod tcp {
    use std::io::{self, BufReader, BufWriter, Read, Write};
    use std::net::{TcpListener, TcpStream, ToSocketAddrs};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::thread;
    use std::time::Duration;

    use crossbeam_channel::bounded;

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::*;

    /// The largest message a server sends (a column with its block names is well under
    /// this) and the largest a client may send: lines of chat and commands are a few
    /// hundred characters, everything else is smaller, so a bigger frame is never honest.
    /// Each side sizes its buffer to a frame before decoding it, so this caps its memory.
    const MAX_SERVER_FRAME: usize = 64 << 20;
    pub const MAX_CLIENT_FRAME: usize = 64 << 10;
    /// Messages received but not yet handled, at most: a server handles a player's
    /// messages every few milliseconds, so a full queue means the peer floods it, and
    /// is cut off.
    const MAX_QUEUED_IN: usize = 1024;
    /// Messages waiting to be written to a peer, at most, before it is cut off for not
    /// reading them (see `Connection::send`).
    const MAX_QUEUED_OUT: usize = 16 * 1024;
    /// TCP connections a listener keeps open at once; more are closed as they come.
    pub const MAX_CONNECTIONS: usize = 128;
    /// How long to wait for a server to answer before giving up on an address.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

    /// Connects to the first of the addresses `addr` resolves to that answers in time.
    pub fn connect(addr: impl ToSocketAddrs) -> io::Result<ClientConnection> {
        let mut error = io::Error::new(io::ErrorKind::NotFound, "the address resolved to nothing");
        for addr in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => return spawn_pumps(stream, Side::Client),
                Err(e) => error = e,
            }
        }
        Err(error)
    }

    /// Accepts connections on a background thread and hands each one to `accepted`.
    pub fn listen(addr: impl ToSocketAddrs, accepted: Sender<ServerConnection>) -> io::Result<std::net::SocketAddr> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        let open = Arc::new(AtomicUsize::new(0));
        thread::Builder::new().name("net-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    tracing::warn!("{MAX_CONNECTIONS} connections are open, closing a new one");
                    continue;
                }
                match stream.and_then(|stream| spawn_pumps(stream, Side::Server(Open::new(&open)))) {
                    Ok(connection) => {
                        if accepted.send(connection).is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!("rejected connection: {e}"),
                }
            }
        })?;
        Ok(local)
    }

    /// One open connection, counted while its reader runs.
    struct Open(Arc<AtomicUsize>);

    impl Open {
        fn new(count: &Arc<AtomicUsize>) -> Self {
            count.fetch_add(1, Ordering::Relaxed);
            Self(count.clone())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Which end of a connection pumps are for. A server guards against its clients: their
    /// frames are small and their queue bounded. A client trusts its server, whose bursts
    /// of columns may run far ahead of a client busy loading.
    enum Side {
        Client,
        Server(Open),
    }

    fn spawn_pumps<Out, In>(stream: TcpStream, side: Side) -> io::Result<Connection<Out, In>>
    where
        Out: Serialize + Send + 'static,
        In: DeserializeOwned + Send + 'static,
    {
        stream.set_nodelay(true)?;
        let loopback = stream.peer_addr()?.ip().is_loopback();
        let (out_tx, out_rx) = bounded::<(Instant, Out)>(MAX_QUEUED_OUT);
        let (max_frame, (in_tx, in_rx), open) = match side {
            Side::Client => (MAX_SERVER_FRAME, unbounded::<(Instant, In)>(), None),
            Side::Server(open) => (MAX_CLIENT_FRAME, bounded(MAX_QUEUED_IN), Some(open)),
        };

        let mut writer = BufWriter::new(stream.try_clone()?);
        thread::Builder::new().name("net-write".into()).spawn(move || {
            let mut buffer = Vec::new();
            while let Ok((_, message)) = out_rx.recv() {
                buffer.clear();
                let ok = postcard::to_io(&message, &mut buffer).is_ok()
                    && writer.write_all(&(buffer.len() as u32).to_le_bytes()).is_ok()
                    && writer.write_all(&buffer).is_ok()
                    // Batch everything already queued into one flush.
                    && (!out_rx.is_empty() || writer.flush().is_ok());
                if !ok {
                    break;
                }
            }
            // Gone, or cut off: close the socket, which ends the reader too.
            let _ = writer.get_ref().shutdown(std::net::Shutdown::Both);
        })?;

        let mut reader = BufReader::new(stream);
        thread::Builder::new().name("net-read".into()).spawn(move || {
            let _open = open;
            let mut buffer = Vec::new();
            loop {
                let mut len = [0u8; 4];
                if reader.read_exact(&mut len).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(len) as usize;
                if len > max_frame {
                    tracing::warn!("frame of {len} bytes exceeds limit, closing connection");
                    break;
                }
                buffer.resize(len, 0);
                if reader.read_exact(&mut buffer).is_err() {
                    break;
                }
                match postcard::from_bytes(&buffer) {
                    Ok(message) => match in_tx.try_send((Instant::now(), message)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            tracing::warn!("peer sent more than {MAX_QUEUED_IN} messages ahead, closing connection");
                            break;
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    },
                    Err(e) => {
                        tracing::warn!("malformed message: {e}");
                        break;
                    }
                }
            }
            let _ = reader.get_ref().shutdown(std::net::Shutdown::Both);
        })?;

        Ok(Connection::new(out_tx, in_rx, false, loopback))
    }
}
