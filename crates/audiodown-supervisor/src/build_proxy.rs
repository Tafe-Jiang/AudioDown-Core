use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, Semaphore},
};

const REGISTRY_HOST: &str = "registry.npmjs.org";
const REGISTRY_AUTHORITY: &str = "registry.npmjs.org:443";

pub trait ProxyStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> ProxyStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
pub type BoxedProxyStream = Box<dyn ProxyStream>;

#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>>;
}

#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, address: SocketAddr) -> io::Result<BoxedProxyStream>;
}

#[derive(Debug, Clone, Copy)]
pub struct ProxyLimits {
    pub max_header_bytes: usize,
    pub max_tunnels: usize,
    pub max_tunnel_bytes: u64,
    pub max_total_bytes: u64,
    pub idle_timeout: Duration,
}

impl Default for ProxyLimits {
    fn default() -> Self {
        Self {
            max_header_bytes: 16 * 1024,
            max_tunnels: 32,
            max_tunnel_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            idle_timeout: Duration::from_secs(60),
        }
    }
}

pub struct BuildProxy<R, C> {
    resolver: Arc<R>,
    connector: Arc<C>,
    limits: ProxyLimits,
    tunnels: Arc<Semaphore>,
    total_bytes: Arc<AtomicU64>,
}

impl<R, C> BuildProxy<R, C>
where
    R: Resolver + 'static,
    C: Connector + 'static,
{
    pub fn new(resolver: Arc<R>, connector: Arc<C>, limits: ProxyLimits) -> Self {
        Self {
            resolver,
            connector,
            limits,
            tunnels: Arc::new(Semaphore::new(limits.max_tunnels)),
            total_bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn handle_stream<S>(&self, mut client: S) -> Result<(), BuildProxyError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let _permit = match self.tunnels.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                write_status(&mut client, 503, "Service Unavailable").await?;
                return Err(BuildProxyError::TooManyTunnels);
            }
        };

        let request = match read_request(&mut client, self.limits.max_header_bytes).await {
            Ok(request) => request,
            Err(BuildProxyError::HeaderTooLarge) => {
                write_status(&mut client, 431, "Request Header Fields Too Large").await?;
                return Err(BuildProxyError::HeaderTooLarge);
            }
            Err(error) => return Err(error),
        };
        if request.method != "CONNECT"
            || request.authority != REGISTRY_AUTHORITY
            || request.version != "HTTP/1.1"
        {
            write_status(&mut client, 405, "Method Not Allowed").await?;
            return Ok(());
        }

        let addresses = self
            .resolver
            .resolve(REGISTRY_HOST)
            .await
            .map_err(|_| BuildProxyError::ResolveFailed)?;
        if addresses.is_empty() || addresses.iter().any(|address| !is_global(*address)) {
            write_status(&mut client, 403, "Forbidden").await?;
            return Ok(());
        }

        let upstream = match self
            .connector
            .connect(SocketAddr::new(addresses[0], 443))
            .await
        {
            Ok(upstream) => upstream,
            Err(_) => {
                write_status(&mut client, 502, "Bad Gateway").await?;
                return Err(BuildProxyError::ConnectFailed);
            }
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .map_err(BuildProxyError::Io)?;

        transfer(client, upstream, self.limits, self.total_bytes.clone()).await
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener) -> Result<(), BuildProxyError> {
        let (fatal_tx, mut fatal_rx) = mpsc::channel(1);
        loop {
            tokio::select! {
                fatal = fatal_rx.recv() => {
                    if let Some(error) = fatal {
                        return Err(error);
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(BuildProxyError::Io)?;
                    let proxy = self.clone();
                    let fatal_tx = fatal_tx.clone();
                    tokio::spawn(async move {
                        if let Err(error) = proxy.handle_stream(stream).await {
                            if matches!(&error, BuildProxyError::AggregateByteLimitExceeded) {
                                let _ = fatal_tx.send(error).await;
                            }
                        }
                    });
                }
            }
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:18081").await?;
    let proxy = Arc::new(BuildProxy::new(
        Arc::new(SystemResolver),
        Arc::new(SystemConnector),
        ProxyLimits::default(),
    ));
    proxy.serve(listener).await?;
    Ok(())
}

pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((host, 443)).await?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

pub struct SystemConnector;

#[async_trait]
impl Connector for SystemConnector {
    async fn connect(&self, address: SocketAddr) -> io::Result<BoxedProxyStream> {
        Ok(Box::new(TcpStream::connect(address).await?))
    }
}

struct ParsedRequest {
    method: String,
    authority: String,
    version: String,
}

async fn read_request<S>(
    stream: &mut S,
    max_header_bytes: usize,
) -> Result<ParsedRequest, BuildProxyError>
where
    S: AsyncRead + Unpin,
{
    let mut header = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await.map_err(BuildProxyError::Io)?;
        if read == 0 {
            return Err(BuildProxyError::InvalidRequest);
        }
        header.push(byte[0]);
        if header.len() > max_header_bytes {
            return Err(BuildProxyError::HeaderTooLarge);
        }
    }

    let header = std::str::from_utf8(&header).map_err(|_| BuildProxyError::InvalidRequest)?;
    let request_line = header
        .split("\r\n")
        .next()
        .ok_or(BuildProxyError::InvalidRequest)?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or(BuildProxyError::InvalidRequest)?;
    let authority = parts.next().ok_or(BuildProxyError::InvalidRequest)?;
    let version = parts.next().ok_or(BuildProxyError::InvalidRequest)?;
    if parts.next().is_some() || method.is_empty() || authority.is_empty() || version.is_empty() {
        return Err(BuildProxyError::InvalidRequest);
    }
    Ok(ParsedRequest {
        method: method.to_string(),
        authority: authority.to_string(),
        version: version.to_string(),
    })
}

async fn write_status<S>(stream: &mut S, code: u16, reason: &str) -> Result<(), BuildProxyError>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(format!("HTTP/1.1 {code} {reason}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .map_err(BuildProxyError::Io)
}

async fn transfer<S>(
    client: S,
    upstream: BoxedProxyStream,
    limits: ProxyLimits,
    total_bytes: Arc<AtomicU64>,
) -> Result<(), BuildProxyError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (client_read, client_write) = tokio::io::split(client);
    let (upstream_read, upstream_write) = tokio::io::split(upstream);
    let tunnel_bytes = Arc::new(AtomicU64::new(0));
    let (activity_tx, activity_rx) = watch::channel(0_u64);

    let upload = copy_limited(
        client_read,
        upstream_write,
        limits,
        tunnel_bytes.clone(),
        total_bytes.clone(),
        activity_tx.clone(),
    );
    let download = copy_limited(
        upstream_read,
        client_write,
        limits,
        tunnel_bytes,
        total_bytes,
        activity_tx,
    );
    let transfers = async {
        tokio::try_join!(upload, download)?;
        Ok(())
    };
    tokio::select! {
        result = transfers => result,
        () = wait_for_idle(activity_rx, limits.idle_timeout) => Err(BuildProxyError::IdleTimeout),
    }
}

async fn copy_limited<R, W>(
    mut reader: R,
    mut writer: W,
    limits: ProxyLimits,
    tunnel_bytes: Arc<AtomicU64>,
    total_bytes: Arc<AtomicU64>,
    activity: watch::Sender<u64>,
) -> Result<(), BuildProxyError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(BuildProxyError::Io)?;
        if read == 0 {
            writer.shutdown().await.map_err(BuildProxyError::Io)?;
            return Ok(());
        }
        let read = read as u64;
        let tunnel_total = tunnel_bytes.fetch_add(read, Ordering::SeqCst) + read;
        if tunnel_total > limits.max_tunnel_bytes {
            return Err(BuildProxyError::TunnelByteLimitExceeded);
        }
        let aggregate_total = total_bytes.fetch_add(read, Ordering::SeqCst) + read;
        if aggregate_total > limits.max_total_bytes {
            return Err(BuildProxyError::AggregateByteLimitExceeded);
        }
        writer
            .write_all(&buffer[..read as usize])
            .await
            .map_err(BuildProxyError::Io)?;
        activity.send_modify(|sequence| *sequence = sequence.wrapping_add(1));
    }
}

async fn wait_for_idle(mut activity: watch::Receiver<u64>, idle_timeout: Duration) {
    loop {
        let sleep = tokio::time::sleep(idle_timeout);
        tokio::pin!(sleep);
        tokio::select! {
            _ = &mut sleep => return,
            changed = activity.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

fn is_global(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_v4(address),
        IpAddr::V6(address) => is_global_v6(address),
    }
}

fn is_global_v4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !address.is_private()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_broadcast()
        && !address.is_documentation()
        && !address.is_unspecified()
        && !address.is_multicast()
        && octets[0] != 0
        && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
        && !(octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        && !(octets[0] == 198 && matches!(octets[1], 18 | 19))
        && octets[0] < 240
}

fn is_global_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_global_v4(mapped);
    }
    let segments = address.segments();
    (segments[0] & 0xe000) == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[derive(Debug, thiserror::Error)]
pub enum BuildProxyError {
    #[error("proxy I/O failed")]
    Io(#[source] io::Error),
    #[error("proxy request is invalid")]
    InvalidRequest,
    #[error("proxy request headers exceed the limit")]
    HeaderTooLarge,
    #[error("proxy tunnel concurrency limit exceeded")]
    TooManyTunnels,
    #[error("registry DNS resolution failed")]
    ResolveFailed,
    #[error("registry connection failed")]
    ConnectFailed,
    #[error("proxy tunnel byte limit exceeded")]
    TunnelByteLimitExceeded,
    #[error("proxy aggregate byte limit exceeded")]
    AggregateByteLimitExceeded,
    #[error("proxy tunnel idle timeout elapsed")]
    IdleTimeout,
}
