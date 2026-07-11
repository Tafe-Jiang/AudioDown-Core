use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_supervisor::build_proxy::{
    BoxedProxyStream, BuildProxy, BuildProxyError, Connector, ProxyLimits, Resolver,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const PUBLIC_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));

#[tokio::test]
async fn accepts_only_the_exact_registry_connect_authority() {
    let fixture = ProxyFixture::new(vec![PUBLIC_IP], ProxyLimits::default());
    let (mut client, task) = fixture.connect().await;
    client
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));
    assert_eq!(
        fixture.resolver.hosts.lock().unwrap().as_slice(),
        ["registry.npmjs.org"]
    );
    assert_eq!(
        fixture.connector.addresses.lock().unwrap().as_slice(),
        [SocketAddr::new(PUBLIC_IP, 443)]
    );
    task.abort();

    for request in [
        "CONNECT registry.npmjs.org:80 HTTP/1.1\r\n\r\n",
        "CONNECT sub.registry.npmjs.org:443 HTTP/1.1\r\n\r\n",
        "CONNECT localhost:443 HTTP/1.1\r\n\r\n",
        "CONNECT 127.0.0.1:443 HTTP/1.1\r\n\r\n",
        "CONNECT [::1]:443 HTTP/1.1\r\n\r\n",
        "GET http://registry.npmjs.org/package HTTP/1.1\r\n\r\n",
    ] {
        let fixture = ProxyFixture::new(vec![PUBLIC_IP], ProxyLimits::default());
        let (mut client, task) = fixture.connect().await;
        client.write_all(request.as_bytes()).await.unwrap();
        let response = read_response(&mut client).await;
        assert!(!response.starts_with("HTTP/1.1 200"), "{request}");
        task.await.unwrap().unwrap();
        assert!(fixture.connector.addresses.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn rejects_any_dns_answer_that_is_not_global() {
    for address in [
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "::1".parse().unwrap(),
        "fe80::1".parse().unwrap(),
    ] {
        let fixture = ProxyFixture::new(vec![PUBLIC_IP, address], ProxyLimits::default());
        let (mut client, task) = fixture.connect().await;
        client
            .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        assert!(read_response(&mut client).await.starts_with("HTTP/1.1 403"));
        task.await.unwrap().unwrap();
        assert!(fixture.connector.addresses.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn rejects_request_headers_over_sixteen_kibibytes() {
    let fixture = ProxyFixture::new(vec![PUBLIC_IP], ProxyLimits::default());
    let (mut client, task) = fixture.connect_with_capacity(32 * 1024).await;
    let oversized = format!(
        "CONNECT registry.npmjs.org:443 HTTP/1.1\r\nX-Fill: {}\r\n\r\n",
        "x".repeat(16 * 1024)
    );
    client.write_all(oversized.as_bytes()).await.unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 431"));
    assert!(matches!(
        task.await.unwrap(),
        Err(BuildProxyError::HeaderTooLarge)
    ));
}

#[tokio::test]
async fn rejects_the_thirty_third_concurrent_tunnel() {
    let fixture = ProxyFixture::new(vec![PUBLIC_IP], ProxyLimits::default());
    let mut active = Vec::new();
    for _ in 0..32 {
        let (mut client, task) = fixture.connect().await;
        client
            .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));
        active.push((client, task));
    }

    let (mut rejected, task) = fixture.connect().await;
    rejected
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut rejected)
        .await
        .starts_with("HTTP/1.1 503"));
    assert!(matches!(
        task.await.unwrap(),
        Err(BuildProxyError::TooManyTunnels)
    ));

    for (_, task) in active {
        task.abort();
    }
}

#[tokio::test]
async fn closes_a_tunnel_that_exceeds_its_byte_limit() {
    let fixture = ProxyFixture::new(
        vec![PUBLIC_IP],
        ProxyLimits {
            max_tunnel_bytes: 8,
            ..ProxyLimits::default()
        },
    );
    let (mut client, task) = fixture.connect().await;
    client
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));
    client.write_all(b"123456789").await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(BuildProxyError::TunnelByteLimitExceeded)
    ));
}

#[tokio::test]
async fn reports_a_fatal_aggregate_byte_limit() {
    let fixture = ProxyFixture::new(
        vec![PUBLIC_IP],
        ProxyLimits {
            max_tunnel_bytes: 64,
            max_total_bytes: 8,
            ..ProxyLimits::default()
        },
    );
    let (mut client, task) = fixture.connect().await;
    client
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));
    client.write_all(b"123456789").await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(BuildProxyError::AggregateByteLimitExceeded)
    ));
}

#[tokio::test]
async fn aggregate_limit_stops_the_proxy_service() {
    let fixture = ProxyFixture::new(
        vec![PUBLIC_IP],
        ProxyLimits {
            max_tunnel_bytes: 64,
            max_total_bytes: 8,
            ..ProxyLimits::default()
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = fixture.proxy.clone();
    let server = tokio::spawn(async move { proxy.serve(listener).await });

    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));
    client.write_all(b"123456789").await.unwrap();
    assert!(matches!(
        server.await.unwrap(),
        Err(BuildProxyError::AggregateByteLimitExceeded)
    ));
}

#[tokio::test(start_paused = true)]
async fn closes_an_idle_tunnel_after_sixty_seconds() {
    let fixture = ProxyFixture::new(vec![PUBLIC_IP], ProxyLimits::default());
    let (mut client, task) = fixture.connect().await;
    client
        .write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut client).await.starts_with("HTTP/1.1 200"));

    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(BuildProxyError::IdleTimeout)
    ));
}

struct ProxyFixture {
    proxy: Arc<BuildProxy<MockResolver, MockConnector>>,
    resolver: Arc<MockResolver>,
    connector: Arc<MockConnector>,
}

impl ProxyFixture {
    fn new(addresses: Vec<IpAddr>, limits: ProxyLimits) -> Self {
        let resolver = Arc::new(MockResolver {
            addresses,
            hosts: Mutex::new(Vec::new()),
        });
        let connector = Arc::new(MockConnector::default());
        let proxy = Arc::new(BuildProxy::new(resolver.clone(), connector.clone(), limits));
        Self {
            proxy,
            resolver,
            connector,
        }
    }

    async fn connect(&self) -> (DuplexStream, JoinHandle<Result<(), BuildProxyError>>) {
        self.connect_with_capacity(4096).await
    }

    async fn connect_with_capacity(
        &self,
        capacity: usize,
    ) -> (DuplexStream, JoinHandle<Result<(), BuildProxyError>>) {
        let (client, server) = tokio::io::duplex(capacity);
        let proxy = self.proxy.clone();
        let task = tokio::spawn(async move { proxy.handle_stream(server).await });
        (client, task)
    }
}

struct MockResolver {
    addresses: Vec<IpAddr>,
    hosts: Mutex<Vec<String>>,
}

#[async_trait]
impl Resolver for MockResolver {
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        self.hosts.lock().unwrap().push(host.to_string());
        Ok(self.addresses.clone())
    }
}

#[derive(Default)]
struct MockConnector {
    addresses: Mutex<Vec<SocketAddr>>,
    peers: Mutex<Vec<DuplexStream>>,
}

#[async_trait]
impl Connector for MockConnector {
    async fn connect(&self, address: SocketAddr) -> io::Result<BoxedProxyStream> {
        self.addresses.lock().unwrap().push(address);
        let (proxy, peer) = tokio::io::duplex(4096);
        self.peers.lock().unwrap().push(peer);
        Ok(Box::new(proxy))
    }
}

async fn read_response<S>(stream: &mut S) -> String
where
    S: AsyncRead + Unpin,
{
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        response.push(byte[0]);
    }
    String::from_utf8(response).unwrap()
}
