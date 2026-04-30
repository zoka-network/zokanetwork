use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zoka::network::p2p::{NetworkPrivacyConfig, Node};

async fn spawn_mock_socks5_proxy() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock socks5 listener");
    let endpoint = format!(
        "127.0.0.1:{}",
        listener.local_addr().expect("local addr").port()
    );
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept socks5 conn");

        let mut greeting = [0u8; 2];
        socket
            .read_exact(&mut greeting)
            .await
            .expect("read greeting");
        let mut methods = vec![0u8; greeting[1] as usize];
        socket.read_exact(&mut methods).await.expect("read methods");
        socket
            .write_all(&[0x05, 0x00])
            .await
            .expect("write auth ok");

        let mut req_hdr = [0u8; 4];
        socket
            .read_exact(&mut req_hdr)
            .await
            .expect("read request header");
        match req_hdr[3] {
            0x01 => {
                let mut payload = [0u8; 6];
                socket
                    .read_exact(&mut payload)
                    .await
                    .expect("read ipv4 payload");
            }
            0x03 => {
                let mut len = [0u8; 1];
                socket.read_exact(&mut len).await.expect("read domain len");
                let mut payload = vec![0u8; len[0] as usize + 2];
                socket
                    .read_exact(&mut payload)
                    .await
                    .expect("read domain payload");
            }
            0x04 => {
                let mut payload = [0u8; 18];
                socket
                    .read_exact(&mut payload)
                    .await
                    .expect("read ipv6 payload");
            }
            atyp => panic!("unexpected socks atyp: {atyp}"),
        }

        let reply = [0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        socket.write_all(&reply).await.expect("write connect reply");
    });

    (endpoint, handle)
}

#[tokio::test]
async fn tor_proxy_probe_accepts_onion_destination() {
    let (proxy_endpoint, proxy_task) = spawn_mock_socks5_proxy().await;
    let mut node = Node::new(0).await.expect("node");
    node.set_privacy_config(NetworkPrivacyConfig {
        tor_proxy: Some(proxy_endpoint),
        enforce_anonymous_transport: true,
        ..NetworkPrivacyConfig::default()
    });

    let result = node
        .probe_anonymous_route(
            "/onion3/vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd:1234",
        )
        .await;
    assert!(result.is_ok());

    let _ = proxy_task.await;
}
