use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use clap::{ArgAction, Parser};
use futures_util::{Future, StreamExt, stream::FuturesUnordered};
use listenfd::ListenFd;
use tracing_subscriber::EnvFilter;
use wireviewd::backend::{MockBackend, SerialBackend};
use wireviewd::discovery::{mock_port, spawn_discovery};
use wireviewd::manager::{HostEvent, spawn_manager};
use wireviewd::varlink::{DEFAULT_SOCKET_PATH, DeviceService};

const MAX_VARLINK_CONNECTIONS: usize = 32;

#[derive(Debug, Parser)]
#[command(about)]
struct Args {
    /// Print the daemon version and build identifier.
    #[arg(short = 'V', long, action = ArgAction::SetTrue)]
    version: bool,

    /// Run against deterministic synthetic hardware.
    #[arg(long)]
    mock: bool,

    /// Test-only: stall the first mock history read until it is cancelled.
    #[arg(long, hide = true, requires = "mock")]
    mock_stall_history: bool,

    /// Varlink Unix socket path when not started through socket activation.
    #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
    socket: PathBuf,

    /// Telemetry polling interval.
    #[arg(long, default_value_t = 500)]
    poll_ms: u64,

    /// Device discovery interval.
    #[arg(long, default_value_t = 750)]
    discovery_ms: u64,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.version {
        println!(
            "wireviewd {} (build {})",
            wireviewd::build_info::VERSION,
            wireviewd::build_info::BUILD_ID
        );
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("wireviewd=info")),
        )
        .init();
    if !(100..=5000).contains(&args.poll_ms) {
        return Err("poll interval must be from 100 to 5000 milliseconds".into());
    }
    let poll_interval = Duration::from_millis(args.poll_ms);
    let (listener, _socket_guard) = listener(&args.socket)?;

    if args.mock {
        let (backend, control) = MockBackend::new();
        if args.mock_stall_history {
            control.block_next_history_read_until_cancelled();
        }
        let (manager, manager_task) = spawn_manager(backend, poll_interval);
        manager
            .observe(HostEvent::Candidates(vec![
                mock_port().to_string_lossy().into_owned(),
            ]))
            .await?;
        run(manager, manager_task, None, listener).await
    } else {
        let (manager, manager_task) = spawn_manager(SerialBackend::new(), poll_interval);
        let discovery = spawn_discovery(
            manager.clone(),
            Duration::from_millis(args.discovery_ms.max(100)),
        );
        run(manager, manager_task, Some(discovery), listener).await
    }
}

async fn run(
    manager: wireviewd::ManagerHandle,
    manager_task: tokio::task::JoinHandle<()>,
    discovery: Option<tokio::task::JoinHandle<Result<(), wireviewd::domain::DeviceError>>>,
    listener: tokio::net::UnixListener,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        mock = manager.state().connected_port.as_deref() == Some("/dev/wireview-mock"),
        version = wireviewd::build_info::VERSION,
        build_id = wireviewd::build_info::BUILD_ID,
        api_compatibility_id = wireviewd::varlink::api_compatibility_id(),
        "wireviewd is ready"
    );
    let service = DeviceService::new(manager.clone());
    let local = tokio::task::LocalSet::new();
    let server_result = local.run_until(serve(listener, service)).await;
    manager.observe(HostEvent::Shutdown).await?;
    if let Some(task) = discovery {
        task.abort();
    }
    manager_task.await?;
    server_result?;
    Ok(())
}

async fn serve(
    listener: tokio::net::UnixListener,
    service: DeviceService,
) -> Result<(), Box<dyn std::error::Error>> {
    type ConnectionFuture = Pin<Box<dyn Future<Output = zlink::Result<()>>>>;

    let mut connections = FuturesUnordered::<ConnectionFuture>::new();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            result = &mut shutdown => {
                result?;
                return Ok(());
            }
            accepted = listener.accept(), if connections.len() < MAX_VARLINK_CONNECTIONS => {
                let (stream, _) = accepted?;
                let stream = zlink::tokio::unix::Stream::try_from(stream)?;
                let listener = zlink::ReadyListener::new(stream);
                let server = zlink::Server::new(listener, service.clone());
                connections.push(Box::pin(server.run()));
            }
            result = connections.next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::debug!(%error, "Varlink connection closed with an error");
                }
            }
        }
    }
}

fn listener(
    socket_path: &Path,
) -> Result<(tokio::net::UnixListener, Option<SocketPathGuard>), Box<dyn std::error::Error>> {
    let mut inherited = ListenFd::from_env();
    if inherited.len() > 1 {
        return Err("expected at most one socket-activation file descriptor".into());
    }
    if let Some(listener) = inherited.take_unix_listener(0)? {
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        return Ok((listener, None));
    }

    let listener = tokio::net::UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    Ok((
        listener,
        Some(SocketPathGuard {
            path: socket_path.to_owned(),
        }),
    ))
}

struct SocketPathGuard {
    path: PathBuf,
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "failed to remove Varlink socket");
        }
    }
}

async fn shutdown_signal() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}
