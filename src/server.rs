use std::{
    fmt::Debug,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use color_eyre::owo_colors::OwoColorize;
use eyre::{eyre, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::instrument;

use crate::{
    dispatcher::{Dispatch, WeightedAddress, WeightedRoundRobinDispatcher},
    http_bridge,
    socks::SocksHandshake,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 50;

#[derive(Clone, Default, Debug)]
pub struct ServerStats {
    pub active: Arc<AtomicUsize>,
    pub total_connections: Arc<AtomicU64>,
    pub total_bytes: Arc<AtomicU64>,
}

pub const PHASE_STARTING: usize = 0;
pub const PHASE_RUNNING: usize = 1;
pub const PHASE_STOPPED: usize = 2;
pub const PHASE_FAILED: usize = 3;

pub struct ServerHandle {
    pub token: CancellationToken,
    pub stats: ServerStats,
    pub phase: Arc<AtomicUsize>,
    pub failure: Arc<Mutex<Option<String>>>,
}

impl ServerHandle {
    pub fn stop(&self) {
        self.token.cancel();
    }
}

pub fn spawn_server(
    handle: &tokio::runtime::Handle,
    socks_addr: SocketAddr,
    http_addr: Option<SocketAddr>,
    addresses: Vec<WeightedAddress>,
) -> ServerHandle {
    let token = CancellationToken::new();
    let tracker = TaskTracker::new();
    let stats = ServerStats::default();
    let phase = Arc::new(AtomicUsize::new(PHASE_STARTING));
    let failure = Arc::new(Mutex::new(None));

    let dispatcher = WeightedRoundRobinDispatcher::new(addresses);

    let socks_task = {
        let dispatcher = dispatcher.clone();
        let token = token.clone();
        let tracker = tracker.clone();
        let stats = stats.clone();
        handle.spawn(async move {
            let result = start_server(socks_addr, dispatcher, token.clone(), tracker, stats).await;
            if result.is_err() {
                token.cancel();
            }
            result
        })
    };

    let bridge_task = http_addr.map(|addr| {
        let dispatcher = dispatcher.clone();
        let token = token.clone();
        let tracker = tracker.clone();
        let stats = stats.clone();
        handle.spawn(async move {
            let result =
                http_bridge::start_bridge(addr, dispatcher, token.clone(), tracker, stats).await;
            if result.is_err() {
                token.cancel();
            }
            result
        })
    });

    {
        let tracker = tracker.clone();
        let phase = Arc::clone(&phase);
        let failure = Arc::clone(&failure);
        handle.spawn(async move {
            let socks_result = socks_task
                .await
                .unwrap_or_else(|err| Err(eyre!("SOCKS server task crashed: {err}")));
            let bridge_result = match bridge_task {
                Some(task) => task
                    .await
                    .unwrap_or_else(|err| Err(eyre!("HTTP bridge task crashed: {err}"))),
                None => Ok(()),
            };

            tracker.close();
            tracker.wait().await;

            let report = socks_result
                .err()
                .map(|err| format!("{err:?}"))
                .or_else(|| bridge_result.err().map(|err| format!("{err:?}")));
            let new_phase = if report.is_some() {
                PHASE_FAILED
            } else {
                PHASE_STOPPED
            };
            if let Ok(mut slot) = failure.lock() {
                *slot = report;
            }
            phase.store(new_phase, Ordering::SeqCst);
        });
    }

    ServerHandle {
        token,
        stats,
        phase,
        failure,
    }
}

#[instrument(skip_all)]
async fn handle_socket<D>(mut socket: TcpStream, dispatcher: D, stats: &ServerStats) -> Result<()>
where
    D: Dispatch + Debug,
{
    let mut server_socket = {
        let (client_reader, client_writer) = socket.split();

        let mut handshake = SocksHandshake::new(client_reader, client_writer, dispatcher);

        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake.handshake()).await {
            Err(_) => {
                return Err(eyre!(
                    "timed out after {}s waiting for the SOCKS handshake to complete",
                    HANDSHAKE_TIMEOUT.as_secs()
                ))
            }
            Ok(Err(err)) => {
                return Err(err.wrap_err(eyre::eyre!(
                    "An error occurred during the proxy handshake procedure"
                )))
            }
            Ok(Ok(server_socket)) => server_socket,
        }
    };

    let peer_addr = match socket.peer_addr() {
        Ok(peer_addr) => peer_addr,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::InvalidInput
                || err.kind() == std::io::ErrorKind::NotConnected
                || err.raw_os_error() == Some(22)
            {
                return Ok(());
            }
            return Err(err.into());
        }
    };
    let remote_addr = server_socket.peer_addr()?;
    tracing::debug!(
        "connection initiated between {} and {}",
        peer_addr,
        remote_addr
    );

    let (sent, received) =
        match tokio::io::copy_bidirectional(&mut socket, &mut server_socket).await {
            Ok(counts) => counts,
            Err(err) if is_benign_disconnect(&err) => {
                tracing::debug!("connection closed by peer: {err}");
                (0, 0)
            }
            Err(err) => return Err(eyre!(err)),
        };

    stats
        .total_bytes
        .fetch_add(sent.saturating_add(received), Ordering::Relaxed);

    tracing::debug!(
        "connection terminated between {} and {} ({} sent, {} received)",
        peer_addr,
        remote_addr,
        humanize_bytes(sent),
        humanize_bytes(received)
    );

    Ok(())
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

fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

#[instrument(skip(dispatcher, token, tracker, stats))]
async fn start_server<D>(
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

    tracing::info!("SOCKS proxy started on {}", addr.bold());
    tracing::info!("Dispatching through the configured network interfaces");

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
                tracing::warn!("Failed to accept incoming connection: {err}");
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(eyre!(
                        "accept loop failed {} times in a row: {err}",
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
                result = handle_socket(socket, dispatcher, &stats) => {
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

pub fn server(
    ip: IpAddr,
    port: u16,
    http_port: Option<u16>,
    addresses: Vec<WeightedAddress>,
) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    let http_addr = http_port.map(|http_port| SocketAddr::new(ip, http_port));
    let handle = spawn_server(rt.handle(), SocketAddr::new(ip, port), http_addr, addresses);

    rt.block_on(async {
        while handle.phase.load(Ordering::SeqCst) < PHASE_STOPPED {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });

    if handle.phase.load(Ordering::SeqCst) == PHASE_FAILED {
        let message = handle
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| "unknown server error".to_string());
        return Err(eyre!(message));
    }

    Ok(())
}
