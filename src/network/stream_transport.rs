//! QUIC over byte streams: an iroh custom transport for networks that carry
//! streams but not datagrams, Tor above all (see `docs/MODO-TOR.md`).
//!
//! Every QUIC datagram travels as one frame, a big-endian `u16` length and its
//! bytes, on a stream between two nodes. Whoever dials sends a hello first
//! (magic and version) and then frames both ways; the listener answers on that
//! same stream. Iroh's hybrid post-quantum TLS runs inside, unchanged: the
//! stream only carries its packets.
//!
//! Addresses are [`CustomAddr`]s with [`STREAM_TRANSPORT_ID`]:
//! - **dialable**: `host:port` (with Tor, `xxxx.onion:port`), what a listener
//!   publishes;
//! - **inbound**: a random token the listener gives each stream it accepts.
//!   It is never published and nobody can dial it; the dialer's claims about
//!   itself are ignored, so no one can redirect another node's replies.
//!
//! A full queue drops the frame, as a full socket drops a UDP datagram: QUIC
//! sends it again.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use iroh::endpoint::{
    presets::Preset,
    transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
    Builder,
};
use iroh_base::CustomAddr;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
};

/// "ZQST": the custom transport id of Zoka's stream transport.
pub const STREAM_TRANSPORT_ID: u64 = 0x5a51_5354;
const HELLO: &[u8] = b"ZQSTREAM1";
const KIND_DIAL: u8 = 1;
const KIND_INBOUND: u8 = 2;
/// Per-stream queue of frames waiting to be written.
const LINK_QUEUE: usize = 256;
/// Frames received from every stream, waiting for iroh to read them.
const INCOMING_QUEUE: usize = 1024;
/// Streams a listener keeps at once.
const MAX_INBOUND: usize = 64;
const MAX_TARGET: usize = 255;
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// The address a listener publishes: `host:port`, or `xxxx.onion:port`.
pub fn dial_addr(target: &str) -> io::Result<CustomAddr> {
    if target.is_empty() || target.len() > MAX_TARGET || !target.contains(':') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a stream address is host:port",
        ));
    }
    let mut data = Vec::with_capacity(1 + target.len());
    data.push(KIND_DIAL);
    data.extend_from_slice(target.as_bytes());
    Ok(CustomAddr::from_parts(STREAM_TRANSPORT_ID, &data))
}

/// The `host:port` of a dialable address of this transport.
pub fn dial_target(addr: &CustomAddr) -> Option<String> {
    let data = addr.data();
    (addr.id() == STREAM_TRANSPORT_ID && data.first() == Some(&KIND_DIAL))
        .then(|| std::str::from_utf8(&data[1..]).ok().map(str::to_owned))
        .flatten()
        .filter(|target| !target.is_empty() && target.len() <= MAX_TARGET)
}

fn inbound_addr() -> CustomAddr {
    let mut data = [0u8; 17];
    data[0] = KIND_INBOUND;
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut data[1..]);
    CustomAddr::from_parts(STREAM_TRANSPORT_ID, &data)
}

/// A byte stream in both directions.
pub trait ByteStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ByteStream for T {}

pub type DialFuture = Pin<Box<dyn Future<Output = io::Result<Box<dyn ByteStream>>> + Send>>;

/// How a stream to `host:port` is opened: plain TCP in tests, Tor in the node.
pub trait Dialer: fmt::Debug + Send + Sync + 'static {
    fn dial(&self, target: String) -> DialFuture;
}

/// Plain TCP, for tests and for a listener's own loopback checks.
#[derive(Debug, Default)]
pub struct TcpDialer;
impl Dialer for TcpDialer {
    fn dial(&self, target: String) -> DialFuture {
        Box::pin(
            async move { Ok(Box::new(TcpStream::connect(target).await?) as Box<dyn ByteStream>) },
        )
    }
}

/// Through a SOCKS5 proxy, Tor's above all (`tor.exe` with a `SocksPort`):
/// the proxy resolves the target, `xxxx.onion:port`, so the name never
/// reaches this machine's resolver. Desktop nodes use the system's `tor.exe`;
/// only the phone embeds Tor (Arti, in ZKAPriv).
#[derive(Debug, Clone, Copy)]
pub struct SocksDialer {
    pub proxy: std::net::SocketAddr,
}
impl Dialer for SocksDialer {
    fn dial(&self, target: String) -> DialFuture {
        let proxy = self.proxy;
        Box::pin(async move {
            let (host, port) = target.rsplit_once(':').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "a stream address is host:port")
            })?;
            let port: u16 = port
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?;
            let stream = tokio_socks::tcp::Socks5Stream::connect(proxy, (host, port))
                .await
                .map_err(io::Error::other)?;
            Ok(Box::new(stream) as Box<dyn ByteStream>)
        })
    }
}

struct Shared {
    /// What this transport publishes: its dialable address, if it listens.
    published: Option<CustomAddr>,
    dialer: Arc<dyn Dialer>,
    /// Open streams by the remote address frames for it are sent to.
    links: Mutex<HashMap<CustomAddr, mpsc::Sender<Bytes>>>,
    incoming: mpsc::Sender<(CustomAddr, Bytes)>,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamTransport")
            .field("published", &self.published)
            .field("dialer", &self.dialer)
            .finish_non_exhaustive()
    }
}

/// The transport; bind it into one iroh endpoint with [`StreamOnly`].
#[derive(Debug, Clone)]
pub struct StreamTransport {
    shared: Arc<Shared>,
    receiver: Arc<Mutex<Option<mpsc::Receiver<(CustomAddr, Bytes)>>>>,
}

impl StreamTransport {
    /// `published`: the `host:port` others dial to reach this node (behind an
    /// onion service, its `.onion:port`); `None` for a node that only dials.
    pub fn new(published: Option<&str>, dialer: Arc<dyn Dialer>) -> io::Result<Self> {
        let (incoming, receiver) = mpsc::channel(INCOMING_QUEUE);
        Ok(Self {
            shared: Arc::new(Shared {
                published: published.map(dial_addr).transpose()?,
                dialer,
                links: Mutex::new(HashMap::new()),
                incoming,
            }),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        })
    }

    /// Accept streams on `listener` (a loopback port an onion service
    /// forwards to) until the returned task is aborted.
    pub fn listen(&self, listener: TcpListener) -> JoinHandle<()> {
        let shared = self.shared.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let inbound = shared
                    .links
                    .lock()
                    .map(|links| links.len())
                    .unwrap_or(usize::MAX);
                if inbound >= MAX_INBOUND {
                    continue;
                }
                let shared = shared.clone();
                tokio::spawn(async move {
                    let (mut reader, writer) = tokio::io::split(stream);
                    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader)).await;
                    if !matches!(hello, Ok(Ok(Some(ref bytes))) if bytes.as_ref() == HELLO) {
                        return;
                    }
                    let peer = inbound_addr();
                    let (sender, queue) = mpsc::channel(LINK_QUEUE);
                    if let Ok(mut links) = shared.links.lock() {
                        links.insert(peer.clone(), sender.clone());
                    }
                    run_link(&shared, peer.clone(), reader, writer, queue).await;
                    forget(&shared, &peer, &sender);
                });
            }
        })
    }

    /// Links currently open, inbound and outbound.
    pub fn link_count(&self) -> usize {
        self.shared
            .links
            .lock()
            .map(|links| links.len())
            .unwrap_or(0)
    }
}

/// An iroh preset: this transport alone, without UDP sockets or relays.
#[derive(Debug, Clone)]
pub struct StreamOnly(pub StreamTransport);
impl Preset for StreamOnly {
    fn apply(self, builder: Builder) -> Builder {
        builder
            .clear_ip_transports()
            .clear_relay_transports()
            .add_custom_transport(Arc::new(self.0))
    }
}

impl CustomTransport for StreamTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let receiver = self
            .receiver
            .lock()
            .map_err(|_| io::Error::other("stream transport poisoned"))?
            .take()
            .ok_or_else(|| io::Error::other("a stream transport binds one endpoint"))?;
        let published: Vec<CustomAddr> = self.shared.published.iter().cloned().collect();
        Ok(Box::new(StreamEndpoint {
            shared: self.shared.clone(),
            receiver,
            local: n0_watcher::Watchable::new(published),
        }))
    }
}

struct StreamEndpoint {
    shared: Arc<Shared>,
    receiver: mpsc::Receiver<(CustomAddr, Bytes)>,
    local: n0_watcher::Watchable<Vec<CustomAddr>>,
}

impl fmt::Debug for StreamEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamEndpoint")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl CustomEndpoint for StreamEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.local.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(StreamSender {
            shared: self.shared.clone(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let n = bufs.len().min(metas.len()).min(recv_infos.len());
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        let mut packets = Vec::with_capacity(n);
        match self.receiver.poll_recv_many(cx, &mut packets, n) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(0) => return Poll::Ready(Err(io::Error::other("stream transport closed"))),
            Poll::Ready(_) => {}
        }
        let mut count = 0;
        for (from, data) in packets {
            // A frame larger than iroh's buffer cannot be a QUIC datagram it
            // sent; it is dropped like a truncated UDP datagram.
            if data.len() > bufs[count].len() {
                continue;
            }
            bufs[count][..data.len()].copy_from_slice(&data);
            metas[count].len = data.len();
            metas[count].stride = data.len();
            recv_infos[count] = RecvInfo::new(from, self.shared.published.clone());
            count += 1;
        }
        if count == 0 {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(Ok(count))
    }
}

#[derive(Debug)]
struct StreamSender {
    shared: Arc<Shared>,
}

impl CustomSender for StreamSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == STREAM_TRANSPORT_ID
    }

    fn poll_send(
        &self,
        _cx: &mut Context,
        dst: &CustomAddr,
        _src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(link) = self.link(dst) else {
            // An inbound stream that already closed: like a datagram to a
            // gone host, nothing arrives.
            return Poll::Ready(Ok(()));
        };
        let segment = transmit
            .segment_size
            .unwrap_or(transmit.contents.len())
            .max(1);
        for datagram in transmit.contents.chunks(segment) {
            if datagram.len() > u16::MAX as usize {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "datagram too large",
                )));
            }
            match link.try_send(Bytes::copy_from_slice(datagram)) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    forget(&self.shared, dst, &link);
                    break;
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl StreamSender {
    /// The open stream to `dst`, or a new one when `dst` is dialable.
    fn link(&self, dst: &CustomAddr) -> Option<mpsc::Sender<Bytes>> {
        let mut links = self.shared.links.lock().ok()?;
        if let Some(link) = links.get(dst).filter(|link| !link.is_closed()) {
            return Some(link.clone());
        }
        let target = dial_target(dst)?;
        let (sender, queue) = mpsc::channel(LINK_QUEUE);
        links.insert(dst.clone(), sender.clone());
        drop(links);
        let shared = self.shared.clone();
        let peer = dst.clone();
        let link = sender.clone();
        tokio::spawn(async move {
            if let Ok(stream) = shared.dialer.dial(target).await {
                let (reader, mut writer) = tokio::io::split(stream);
                if write_frame(&mut writer, HELLO).await.is_ok() {
                    run_link(&shared, peer.clone(), reader, writer, queue).await;
                }
            }
            forget(&shared, &peer, &link);
        });
        Some(sender)
    }
}

/// Drop `link` from the table, unless a newer one already replaced it.
fn forget(shared: &Shared, peer: &CustomAddr, link: &mpsc::Sender<Bytes>) {
    if let Ok(mut links) = shared.links.lock() {
        if links
            .get(peer)
            .is_some_and(|current| current.same_channel(link))
        {
            links.remove(peer);
        }
    }
}

/// Frames from `queue` to the stream, and from the stream to iroh, until
/// either side ends.
async fn run_link<R, W>(
    shared: &Shared,
    peer: CustomAddr,
    mut reader: R,
    mut writer: W,
    mut queue: mpsc::Receiver<Bytes>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let incoming = shared.incoming.clone();
    let inbound = async {
        while let Ok(Some(frame)) = read_frame(&mut reader).await {
            // A full queue drops the datagram, as a full UDP socket would.
            if let Err(mpsc::error::TrySendError::Closed(_)) =
                incoming.try_send((peer.clone(), frame))
            {
                break;
            }
        }
    };
    let outbound = async {
        while let Some(frame) = queue.recv().await {
            if write_frame(&mut writer, &frame).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = inbound => {}
        _ = outbound => {}
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> io::Result<()> {
    let len = u16::try_from(frame.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut buffer = Vec::with_capacity(2 + frame.len());
    buffer.extend_from_slice(&len.to_be_bytes());
    buffer.extend_from_slice(frame);
    writer.write_all(&buffer).await?;
    writer.flush().await
}

/// The next frame, or `None` at a clean end of the stream.
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Bytes>> {
    let mut len = [0u8; 2];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut frame = vec![0u8; u16::from_be_bytes(len) as usize];
    reader.read_exact(&mut frame).await?;
    Ok(Some(Bytes::from(frame)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::pq_tls::RequiredPostQuantumTls;
    use iroh::{
        endpoint::{presets, Connection},
        protocol::{AcceptError, ProtocolHandler, Router},
        Endpoint, EndpointAddr, TransportAddr,
    };

    const ALPN: &[u8] = b"zoka-test/stream-echo/1";

    #[derive(Debug, Clone)]
    struct Echo;
    impl ProtocolHandler for Echo {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let (mut send, mut recv) = connection.accept_bi().await?;
            tokio::io::copy(&mut recv, &mut send).await?;
            send.finish()?;
            connection.closed().await;
            Ok(())
        }
    }

    #[test]
    fn addresses_name_a_target_or_nothing() {
        let addr = dial_addr("abcdefghijklmnop.onion:9444").unwrap();
        assert_eq!(
            dial_target(&addr).as_deref(),
            Some("abcdefghijklmnop.onion:9444")
        );
        assert!(dial_addr("no-port").is_err());
        assert!(dial_addr(&"a".repeat(300)).is_err());
        // An inbound token is never dialable, and other transports are not ours.
        assert_eq!(dial_target(&inbound_addr()), None);
        assert_eq!(dial_target(&CustomAddr::from_parts(7, b"\x01x:1")), None);
    }

    #[tokio::test]
    async fn frames_keep_their_bounds_and_a_clean_end_is_none() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_frame(&mut a, b"one").await.unwrap();
        write_frame(&mut a, &[7u8; 1400]).await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        assert!(write_frame(&mut a, &vec![0u8; 70_000]).await.is_err());
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap().as_ref(), b"one");
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap().len(), 1400);
        assert!(read_frame(&mut b).await.unwrap().unwrap().is_empty());
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn quic_with_hybrid_tls_runs_over_a_byte_stream_alone() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let server_transport = StreamTransport::new(Some(&target), Arc::new(TcpDialer)).unwrap();
        let server = Endpoint::builder(RequiredPostQuantumTls(presets::Minimal))
            .preset(StreamOnly(server_transport.clone()))
            .bind()
            .await
            .unwrap();
        let _listening = server_transport.listen(listener);
        let router = Router::builder(server.clone()).accept(ALPN, Echo).spawn();

        let client_transport = StreamTransport::new(None, Arc::new(TcpDialer)).unwrap();
        let client = Endpoint::builder(RequiredPostQuantumTls(presets::Minimal))
            .preset(StreamOnly(client_transport.clone()))
            .bind()
            .await
            .unwrap();
        let to = EndpointAddr::from_parts(
            server.id(),
            [TransportAddr::Custom(dial_addr(&target).unwrap())],
        );
        let connection = tokio::time::timeout(Duration::from_secs(30), client.connect(to, ALPN))
            .await
            .expect("the handshake over the stream timed out")
            .unwrap();
        assert_eq!(connection.remote_id(), server.id());

        // Enough data for many datagrams, both ways.
        let message: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&message).await.unwrap();
        send.finish().unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(30), recv.read_to_end(1 << 20))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(echoed, message);
        assert_eq!(client_transport.link_count(), 1);
        assert_eq!(server_transport.link_count(), 1);

        connection.close(0u32.into(), b"done");
        router.shutdown().await.unwrap();
        client.close().await;
    }
}
