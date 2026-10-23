use std::{fmt::Debug, net::SocketAddr, sync::atomic::Ordering, time::Duration};

use eyre::{eyre, Result, WrapErr};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::instrument;

use crate::{dispatcher::Dispatch, net::bind_socket, server::ServerStats, socks::lookup};

const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 50;

#[instrument(skip(dispatcher, token, tracker, stats))]
pub async fn start_bridge<D>(
    addr: SocketAddr,
    dispatcher: D,
    token: CancellationToken,
    tracker: TaskTracker,
    stats: ServerStats,
) -> Result<()>
where
    D: Dispatch + Clone + Debug + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("Built-in HTTP proxy started on {}", addr);

    let mut consecutive_errors: u32 = 0;

    loop {
        let accepted = tokio::select! {
            _ = token.cancelled() => break,
            accepted = listener.accept() => accepted,
        };

        let (socket, peer) = match accepted {
            Ok(pair) => {
                consecutive_errors = 0;
                pair
            }
            Err(err) => {
                consecutive_errors += 1;
                tracing::warn!("HTTP proxy failed to accept connection: {err}");
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(eyre!(
                        "HTTP proxy accept loop failed {} times in a row: {err}",
                        MAX_CONSECUTIVE_ACCEPT_ERRORS
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let _ = peer;

        stats.total_connections.fetch_add(1, Ordering::Relaxed);

        let dispatcher = dispatcher.clone();
        let stats = stats.clone();
        let conn_token = token.clone();
        tracker.spawn(async move {
            stats.active.fetch_add(1, Ordering::Relaxed);
            tokio::select! {
                _ = conn_token.cancelled() => {}
                result = handle_client(socket, dispatcher, &stats) => {
                    if let Err(err) = result {
                        tracing::warn!("{:?}", err);
                    }
                }
            }
            stats.active.fetch_sub(1, Ordering::Relaxed);
        });
    }

    Ok(())
}

#[instrument(skip_all)]
async fn handle_client<D>(mut client: TcpStream, dispatcher: D, stats: &ServerStats) -> Result<()>
where
    D: Dispatch + Debug,
{
    let mut head = Vec::new();
    match tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client, &mut head)).await {
        Err(_) => return Err(eyre!("timed out waiting for the HTTP request head")),
        Ok(Err(err)) => return Err(err),
        Ok(Ok(())) => {}
    }

    let (method, target, _version, header_block) = parse_head(&head)?;
    let method = method.to_ascii_uppercase();

    if method == "CONNECT" {
        return handle_connect(client, &target, dispatcher, stats).await;
    }

    handle_forward_request(client, method, target, header_block, dispatcher, stats).await
}

#[instrument(skip_all, fields(target = %target))]
async fn handle_connect<D>(
    mut client: TcpStream,
    target: &str,
    dispatcher: D,
    stats: &ServerStats,
) -> Result<()>
where
    D: Dispatch + Debug,
{
    let (host, port) = match parse_authority(target, 443) {
        Ok(pair) => pair,
        Err(_) => {
            let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            return Err(eyre!("malformed CONNECT target `{target}`"));
        }
    };

    tracing::debug!("HTTP CONNECT {host}:{port}");

    let remote = match lookup((host.as_str(), port)).await {
        Ok(addr) => addr,
        Err(err) => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return Err(eyre!(err).wrap_err(format!("failed to resolve `{host}`")));
        }
    };

    match connect_via_dispatch(&dispatcher, &remote).await {
        Ok(mut upstream) => {
            if client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .is_err()
            {
                return Ok(());
            }

            let (sent, received) =
                match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                    Ok(counts) => counts,
                    Err(err) if is_benign_disconnect(&err) => (0, 0),
                    Err(err) => return Err(eyre!(err)),
                };
            stats
                .total_bytes
                .fetch_add(sent.saturating_add(received), Ordering::Relaxed);
            tracing::debug!(
                "HTTP CONNECT {host}:{port} closed ({} relayed)",
                sent + received
            );
            Ok(())
        }
        Err(err) => {
            let status = if matches!(
                err.downcast_ref::<std::io::Error>(),
                Some(io_err) if io_err.kind() == std::io::ErrorKind::TimedOut
            ) || err.to_string().contains("timed out")
            {
                "HTTP/1.1 504 Gateway Timeout\r\n\r\n"
            } else {
                "HTTP/1.1 502 Bad Gateway\r\n\r\n"
            };
            let _ = client.write_all(status.as_bytes()).await;
            Err(eyre!(err).wrap_err(format!("failed to connect to `{host}:{port}`")))
        }
    }
}

#[instrument(skip_all)]
async fn handle_forward_request<D>(
    mut client: TcpStream,
    method: String,
    target: String,
    header_block: String,
    dispatcher: D,
    stats: &ServerStats,
) -> Result<()>
where
    D: Dispatch + Debug,
{
    let (host, port, path) = match split_absolute_uri(&target) {
        Ok(parts) => parts,
        Err(_) => {
            let _ = client
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nThis is an HTTP proxy: requests must use an absolute URI (e.g. GET http://example.com/ HTTP/1.1).\r\n\r\n")
                .await;
            return Err(eyre!("request target `{target}` is not an absolute URI"));
        }
    };

    tracing::debug!("HTTP {method} {host}:{port}{path}");

    let remote = match lookup((host.as_str(), port)).await {
        Ok(addr) => addr,
        Err(err) => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return Err(eyre!(err).wrap_err(format!("failed to resolve `{host}`")));
        }
    };

    let mut upstream = match connect_via_dispatch(&dispatcher, &remote).await {
        Ok(upstream) => upstream,
        Err(err) => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return Err(eyre!(err).wrap_err(format!("failed to connect to `{host}:{port}`")));
        }
    };

    let mut forwarded = format!("{method} {path} HTTP/1.1\r\n");
    let mut had_host = false;
    let mut content_length: Option<u64> = None;
    for line in header_block.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("host:") {
            had_host = true;
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        } else if lower.starts_with("content-length:") {
            content_length = line
                .split(':')
                .nth(1)
                .and_then(|v| v.trim().parse::<u64>().ok());
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        } else if lower.starts_with("connection:")
            || lower.starts_with("proxy-connection:")
            || lower.starts_with("proxy-authorization:")
            || lower.starts_with("expect:")
        {
        } else {
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
    }
    if !had_host {
        if port == 80 {
            forwarded.push_str(&format!("Host: {host}\r\n"));
        } else {
            forwarded.push_str(&format!("Host: {host}:{port}\r\n"));
        }
    }
    forwarded.push_str("Connection: close\r\n\r\n");

    upstream.write_all(forwarded.as_bytes()).await?;

    if let Some(length) = content_length.filter(|&n| n > 0) {
        let mut remaining = length;
        let mut buffer = [0u8; 16 * 1024];
        while remaining > 0 {
            let want = std::cmp::min(remaining, buffer.len() as u64) as usize;
            let read = client.read(&mut buffer[..want]).await?;
            if read == 0 {
                break;
            }
            upstream.write_all(&buffer[..read]).await?;
            remaining -= read as u64;
            stats.total_bytes.fetch_add(read as u64, Ordering::Relaxed);
        }
    }

    let (mut upstream_read, _upstream_write) = upstream.split();
    let mut relayed: u64 = 0;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = upstream_read.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if client.write_all(&buffer[..read]).await.is_err() {
            break;
        }
        relayed += read as u64;
    }
    stats.total_bytes.fetch_add(relayed, Ordering::Relaxed);
    let _ = client.shutdown().await;

    tracing::debug!("HTTP {method} {host}:{port} completed ({relayed} B response)");
    Ok(())
}

async fn connect_via_dispatch<D>(dispatcher: &D, remote: &SocketAddr) -> Result<TcpStream>
where
    D: Dispatch + Debug,
{
    let local_ip = dispatcher
        .dispatch(remote)
        .await
        .wrap_err("An error occurred during dispatching")?;

    let socket = bind_socket(local_ip)?;

    match tokio::time::timeout(CONNECT_TIMEOUT, socket.connect(*remote)).await {
        Ok(result) => result.map_err(|err| eyre!(err)),
        Err(_elapsed) => Err(eyre!(
            "connection to `{}` timed out after {}s",
            remote,
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

async fn read_head<R: AsyncRead + Unpin>(reader: &mut R, out: &mut Vec<u8>) -> Result<()> {
    let mut chunk = [0u8; 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Err(eyre!(
                "client closed the connection before the request was complete"
            ));
        }
        out.extend_from_slice(&chunk[..read]);
        if out.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(());
        }
        if out.len() > MAX_HEAD_BYTES {
            return Err(eyre!("HTTP request head exceeds {} bytes", MAX_HEAD_BYTES));
        }
    }
}

fn parse_head(head: &[u8]) -> Result<(String, String, String, String)> {
    let text = std::str::from_utf8(head)?;
    let end = text
        .find("\r\n\r\n")
        .ok_or_else(|| eyre!("incomplete HTTP head"))?;
    let mut lines = text[..end].split("\r\n");
    let request_line = lines.next().ok_or_else(|| eyre!("empty HTTP head"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| eyre!("missing HTTP method"))?;
    let target = parts
        .next()
        .ok_or_else(|| eyre!("missing request target"))?;
    let version = parts.next().unwrap_or("HTTP/1.0");
    let headers = lines.collect::<Vec<_>>().join("\r\n");
    Ok((
        method.to_string(),
        target.to_string(),
        version.to_string(),
        headers,
    ))
}

fn parse_authority(target: &str, default_port: u16) -> Result<(String, u16)> {
    if target.starts_with('[') {
        let close = target
            .find(']')
            .ok_or_else(|| eyre!("malformed IPv6 authority"))?;
        let host = target[1..close].to_string();
        let port = if target.len() > close + 2 && target.as_bytes()[close + 1] == b':' {
            target[close + 2..].parse()?
        } else {
            default_port
        };
        return Ok((host, port));
    }
    match target.rsplit_once(':') {
        Some((host, port)) => {
            let port: u16 = port
                .parse()
                .map_err(|_| eyre!("invalid port in `{target}`"))?;
            Ok((host.to_string(), port))
        }
        None => Ok((target.to_string(), default_port)),
    }
}

fn split_absolute_uri(target: &str) -> Result<(String, u16, String)> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))
        .ok_or_else(|| eyre!("only `http://` absolute URIs are supported"))?;
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = parse_authority(authority, 80)?;
    Ok((host, port, path.to_string()))
}

fn is_benign_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    ) || matches!(
        err.raw_os_error(),
        Some(32) | Some(54) | Some(104) | Some(10053) | Some(10054)
    )
}
