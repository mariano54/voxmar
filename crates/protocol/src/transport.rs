//! Message transports. Every connection is a pair of channels; TCP connections pump
//! those channels from reader and writer threads, so game code never touches sockets.

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};

use crate::{ClientMessage, ServerMessage};

/// The peer closed the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

/// One end of a bidirectional message stream.
pub struct Connection<Out, In> {
    tx: Sender<Out>,
    rx: Receiver<In>,
}

pub type ClientConnection = Connection<ClientMessage, ServerMessage>;
pub type ServerConnection = Connection<ServerMessage, ClientMessage>;

impl<Out, In> Connection<Out, In> {
    /// Queues a message. Returns false once the peer has gone away.
    pub fn send(&self, message: Out) -> bool {
        self.tx.send(message).is_ok()
    }

    /// The next received message, `Ok(None)` if none is pending, `Err` when disconnected.
    pub fn try_recv(&self) -> Result<Option<In>, Disconnected> {
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Disconnected),
        }
    }
}

/// An in-process connection, used by singleplayer's integrated server.
pub fn local_pair() -> (ClientConnection, ServerConnection) {
    let (client_tx, server_rx) = unbounded();
    let (server_tx, client_rx) = unbounded();
    (Connection { tx: client_tx, rx: client_rx }, Connection { tx: server_tx, rx: server_rx })
}

pub mod tcp {
    use std::io::{self, BufReader, BufWriter, Read, Write};
    use std::net::{TcpListener, TcpStream, ToSocketAddrs};
    use std::thread;

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::*;

    const MAX_FRAME: usize = 64 << 20;

    pub fn connect(addr: impl ToSocketAddrs) -> io::Result<ClientConnection> {
        spawn_pumps(TcpStream::connect(addr)?)
    }

    /// Accepts connections on a background thread and hands each one to `accepted`.
    pub fn listen(addr: impl ToSocketAddrs, accepted: Sender<ServerConnection>) -> io::Result<std::net::SocketAddr> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        thread::Builder::new().name("net-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                match stream.and_then(spawn_pumps) {
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

    fn spawn_pumps<Out, In>(stream: TcpStream) -> io::Result<Connection<Out, In>>
    where
        Out: Serialize + Send + 'static,
        In: DeserializeOwned + Send + 'static,
    {
        stream.set_nodelay(true)?;
        let (out_tx, out_rx) = unbounded::<Out>();
        let (in_tx, in_rx) = unbounded::<In>();

        let mut writer = BufWriter::new(stream.try_clone()?);
        thread::Builder::new().name("net-write".into()).spawn(move || {
            let mut buffer = Vec::new();
            while let Ok(message) = out_rx.recv() {
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
        })?;

        let mut reader = BufReader::new(stream);
        thread::Builder::new().name("net-read".into()).spawn(move || {
            let mut buffer = Vec::new();
            loop {
                let mut len = [0u8; 4];
                if reader.read_exact(&mut len).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(len) as usize;
                if len > MAX_FRAME {
                    tracing::warn!("frame of {len} bytes exceeds limit, closing connection");
                    break;
                }
                buffer.resize(len, 0);
                if reader.read_exact(&mut buffer).is_err() {
                    break;
                }
                match postcard::from_bytes(&buffer) {
                    Ok(message) => {
                        if in_tx.send(message).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("malformed message: {e}");
                        break;
                    }
                }
            }
            let _ = reader.get_ref().shutdown(std::net::Shutdown::Both);
        })?;

        Ok(Connection { tx: out_tx, rx: in_rx })
    }
}
