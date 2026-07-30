use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{ArgAction, Parser};
use listenfd::ListenFd;
use tracing_subscriber::EnvFilter;
use wireviewd::backend::{MockBackend, SerialBackend};
use wireviewd::discovery::{mock_port, spawn_discovery};
use wireviewd::manager::{HostEvent, spawn_manager};
use wireviewd::varlink::{DEFAULT_SOCKET_PATH, DeviceService};

#[derive(Debug, Parser)]
#[command(about)]
struct Args {
    /// Print the daemon version and build identifier.
    #[arg(short = 'V', long, action = ArgAction::SetTrue)]
    version: bool,

    /// Run against deterministic synthetic hardware.
    #[arg(long)]
    mock: bool,

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

#[tokio::main]
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
        let (backend, _) = MockBackend::new();
        let (manager, manager_task) = spawn_manager(backend, poll_interval);
        let server = zlink::Server::new(listener, DeviceService::new(manager.clone()));
        manager
            .observe(HostEvent::Candidates(vec![
                mock_port().to_string_lossy().into_owned(),
            ]))
            .await?;
        run(manager, manager_task, None, server).await
    } else {
        let (manager, manager_task) = spawn_manager(SerialBackend::new(), poll_interval);
        let server = zlink::Server::new(listener, DeviceService::new(manager.clone()));
        let discovery = spawn_discovery(
            manager.clone(),
            Duration::from_millis(args.discovery_ms.max(100)),
        );
        run(manager, manager_task, Some(discovery), server).await
    }
}

async fn run(
    manager: wireviewd::ManagerHandle,
    manager_task: tokio::task::JoinHandle<()>,
    discovery: Option<tokio::task::JoinHandle<Result<(), wireviewd::domain::DeviceError>>>,
    server: zlink::Server<zlink::tokio::unix::Listener, DeviceService>,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        mock = manager.state().connected_port.as_deref() == Some("/dev/wireview-mock"),
        version = wireviewd::build_info::VERSION,
        build_id = wireviewd::build_info::BUILD_ID,
        api_compatibility_id = wireviewd::varlink::api_compatibility_id(),
        "wireviewd is ready"
    );
    let server_result = tokio::select! {
        result = server.run() => Some(result),
        result = shutdown_signal() => {
            result?;
            None
        }
    };
    manager.observe(HostEvent::Shutdown).await?;
    if let Some(task) = discovery {
        task.abort();
    }
    manager_task.await?;
    if let Some(result) = server_result {
        result?;
    }
    Ok(())
}

fn listener(
    socket_path: &Path,
) -> Result<(zlink::tokio::unix::Listener, Option<SocketPathGuard>), Box<dyn std::error::Error>> {
    let mut inherited = ListenFd::from_env();
    if inherited.len() > 1 {
        return Err("expected at most one socket-activation file descriptor".into());
    }
    if let Some(listener) = inherited.take_unix_listener(0)? {
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        return Ok((listener.into(), None));
    }

    let listener = zlink::tokio::unix::bind(socket_path)?;
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
