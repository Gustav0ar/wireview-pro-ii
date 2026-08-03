use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::backend::{DeviceBackend, ReadCancellation};
use crate::config::{DeviceConfiguration, DeviceSettings, NvmOperation, encode_configuration};
use crate::domain::{
    ConnectionState, DaemonState, DeviceError, DeviceEvent, DeviceIdentity, DisconnectReason,
    Screen, Telemetry,
};
use crate::history::FLASH_LENGTH;
use crate::protocol::KNOWN_FAULT_MASK;
use crate::theme::ThemeAssetSlot;

const MIN_NVM_MUTATION_INTERVAL: Duration = Duration::from_secs(1);
const DISPLAY_RESUME_RETRY_DELAY: Duration = Duration::from_secs(1);

type ActiveHistoryCancellation = Arc<Mutex<Option<(u64, ReadCancellation)>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEvent {
    Candidates(Vec<String>),
    Shutdown,
}

enum Command {
    SetScreen {
        screen: Screen,
        reply: oneshot::Sender<Result<(), DeviceError>>,
    },
    ReadHistoryChunk {
        offset: usize,
        length: usize,
        reply: oneshot::Sender<Result<Vec<u8>, DeviceError>>,
    },
    ReadThemeAsset {
        slot: ThemeAssetSlot,
        reply: oneshot::Sender<Result<Vec<u8>, DeviceError>>,
    },
    WriteThemeAsset {
        slot: ThemeAssetSlot,
        data: Box<[u8]>,
        reply: oneshot::Sender<Result<(), DeviceError>>,
    },
    BeginHistoryDump {
        reply: oneshot::Sender<Result<HistoryDump, DeviceError>>,
    },
    ReadHistoryDumpChunk {
        dump_id: u64,
        offset: usize,
        length: usize,
        reply: oneshot::Sender<Result<Vec<u8>, DeviceError>>,
    },
    EndHistoryDump {
        dump_id: u64,
        reply: oneshot::Sender<Result<(), DeviceError>>,
    },
    ReadDeviceInfo {
        reply: oneshot::Sender<Result<DeviceIdentity, DeviceError>>,
    },
    ClearFaults {
        active_mask: u16,
        logged_mask: u16,
        reply: oneshot::Sender<Result<Telemetry, DeviceError>>,
    },
    GetPollInterval {
        reply: oneshot::Sender<u64>,
    },
    SetPollInterval {
        milliseconds: u64,
        reply: oneshot::Sender<Result<u64, DeviceError>>,
    },
    GetDisplayPause {
        reply: oneshot::Sender<DisplayPauseState>,
    },
    PauseDisplay {
        milliseconds: u64,
        reply: oneshot::Sender<Result<DisplayPauseState, DeviceError>>,
    },
    ResumeDisplay {
        reply: oneshot::Sender<Result<DisplayPauseState, DeviceError>>,
    },
    ReadConfiguration {
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    },
    ReadConfigurationItem {
        key: String,
        reply: oneshot::Sender<Result<String, DeviceError>>,
    },
    ApplyConfiguration {
        configuration: Box<DeviceConfiguration>,
        persist: bool,
        expected_revision: Option<String>,
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    },
    SetConfigurationItem {
        key: String,
        value: String,
        persist: bool,
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    },
    ReloadConfiguration {
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    },
    ResetConfiguration {
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    },
    RebootDevice {
        reply: oneshot::Sender<Result<(), DeviceError>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryDump {
    pub id: u64,
    pub session_id: u64,
    pub total_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayPauseState {
    pub paused: bool,
    pub debug_lease_active: bool,
    pub history_dump_active: bool,
    pub remaining_ms: u64,
}

pub fn configuration_revision(
    session_id: u64,
    configuration: &DeviceConfiguration,
) -> Result<String, DeviceError> {
    let bytes = encode_configuration(configuration)?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in session_id.to_le_bytes().into_iter().chain(bytes) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(format!("{session_id}:{hash:016x}"))
}

#[derive(Clone)]
pub struct ManagerHandle {
    host_tx: mpsc::Sender<HostEvent>,
    command_tx: mpsc::Sender<Command>,
    state_rx: watch::Receiver<DaemonState>,
    event_tx: broadcast::Sender<DeviceEvent>,
    history_cancellation: ActiveHistoryCancellation,
}

impl ManagerHandle {
    pub async fn observe(&self, event: HostEvent) -> Result<(), DeviceError> {
        if event == HostEvent::Shutdown
            && let Some((_, cancellation)) = self
                .history_cancellation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
        {
            cancellation.cancel();
        }
        self.host_tx
            .send(event)
            .await
            .map_err(|_| DeviceError::ManagerStopped)
    }

    #[must_use]
    pub fn state(&self) -> DaemonState {
        self.state_rx.borrow().clone()
    }

    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<DaemonState> {
        self.state_rx.clone()
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<DeviceEvent> {
        self.event_tx.subscribe()
    }

    pub async fn set_screen(&self, screen: Screen) -> Result<(), DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::SetScreen { screen, reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_history_chunk(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadHistoryChunk {
                offset,
                length,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_theme_asset(&self, slot: ThemeAssetSlot) -> Result<Vec<u8>, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadThemeAsset { slot, reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn write_theme_asset(
        &self,
        slot: ThemeAssetSlot,
        data: Vec<u8>,
    ) -> Result<(), DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::WriteThemeAsset {
                slot,
                data: data.into_boxed_slice(),
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn begin_history_dump(&self) -> Result<HistoryDump, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::BeginHistoryDump { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_history_dump_chunk(
        &self,
        dump_id: u64,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadHistoryDumpChunk {
                dump_id,
                offset,
                length,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn end_history_dump(&self, dump_id: u64) -> Result<(), DeviceError> {
        if let Some((_, cancellation)) = self
            .history_cancellation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|(active_id, _)| *active_id == dump_id)
        {
            cancellation.cancel();
        }
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::EndHistoryDump { dump_id, reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_device_info(&self) -> Result<DeviceIdentity, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadDeviceInfo { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn clear_faults(
        &self,
        active_mask: u16,
        logged_mask: u16,
    ) -> Result<Telemetry, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ClearFaults {
                active_mask,
                logged_mask,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn poll_interval_ms(&self) -> Result<u64, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::GetPollInterval { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)
    }

    pub async fn set_poll_interval_ms(&self, milliseconds: u64) -> Result<u64, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::SetPollInterval {
                milliseconds,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn display_pause_state(&self) -> Result<DisplayPauseState, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::GetDisplayPause { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)
    }

    pub async fn pause_display(&self, milliseconds: u64) -> Result<DisplayPauseState, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::PauseDisplay {
                milliseconds,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn resume_display(&self) -> Result<DisplayPauseState, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ResumeDisplay { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_configuration(&self) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadConfiguration { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn read_configuration_item(&self, key: String) -> Result<String, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReadConfigurationItem { key, reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn apply_configuration(
        &self,
        configuration: DeviceConfiguration,
        persist: bool,
    ) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ApplyConfiguration {
                configuration: Box::new(configuration),
                persist,
                expected_revision: None,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn apply_configuration_if_revision(
        &self,
        configuration: DeviceConfiguration,
        persist: bool,
        expected_revision: String,
    ) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ApplyConfiguration {
                configuration: Box::new(configuration),
                persist,
                expected_revision: Some(expected_revision),
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn reload_configuration(&self) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ReloadConfiguration { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn set_configuration_item(
        &self,
        key: String,
        value: String,
        persist: bool,
    ) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::SetConfigurationItem {
                key,
                value,
                persist,
                reply,
            })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn reset_configuration(&self) -> Result<DeviceConfiguration, DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::ResetConfiguration { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }

    pub async fn reboot_device(&self) -> Result<(), DeviceError> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(Command::RebootDevice { reply })
            .await
            .map_err(|_| DeviceError::ManagerStopped)?;
        response.await.map_err(|_| DeviceError::ManagerStopped)?
    }
}

pub fn spawn_manager<B: DeviceBackend>(
    backend: B,
    poll_interval: Duration,
) -> (ManagerHandle, JoinHandle<()>) {
    let (host_tx, host_rx) = mpsc::channel(32);
    let (command_tx, command_rx) = mpsc::channel(32);
    let (state_tx, state_rx) = watch::channel(DaemonState::default());
    let (event_tx, _) = broadcast::channel(64);
    let history_cancellation = Arc::new(Mutex::new(None));
    let handle = ManagerHandle {
        host_tx,
        command_tx,
        state_rx,
        event_tx: event_tx.clone(),
        history_cancellation: history_cancellation.clone(),
    };
    let task = tokio::spawn(
        Manager {
            backend,
            poll_interval,
            host_rx,
            command_rx,
            state_tx,
            event_tx,
            state: DaemonState::default(),
            current_port: None,
            retry: None,
            consecutive_poll_failures: 0,
            last_successful_poll: None,
            active_history_dump: None,
            next_history_dump_id: 0,
            debug_pause_expires_at: None,
            display_physically_paused: false,
            display_resume_retry_at: None,
            history_cancellation,
            last_nvm_mutation: None,
        }
        .run(),
    );
    (handle, task)
}

struct Manager<B> {
    backend: B,
    poll_interval: Duration,
    host_rx: mpsc::Receiver<HostEvent>,
    command_rx: mpsc::Receiver<Command>,
    state_tx: watch::Sender<DaemonState>,
    event_tx: broadcast::Sender<DeviceEvent>,
    state: DaemonState,
    current_port: Option<String>,
    retry: Option<RetryState>,
    consecutive_poll_failures: u8,
    last_successful_poll: Option<Instant>,
    active_history_dump: Option<ActiveHistoryDump>,
    next_history_dump_id: u64,
    debug_pause_expires_at: Option<Instant>,
    display_physically_paused: bool,
    display_resume_retry_at: Option<Instant>,
    history_cancellation: ActiveHistoryCancellation,
    last_nvm_mutation: Option<Instant>,
}

struct RetryState {
    port: String,
    attempt: u32,
    not_before: Option<Instant>,
}

struct ActiveHistoryDump {
    id: u64,
    session_id: u64,
    expires_at: Instant,
    cancellation: ReadCancellation,
}

impl<B: DeviceBackend> Manager<B> {
    async fn run(mut self) {
        let mut next_poll = Instant::now();
        loop {
            let mut wake_at = next_poll;
            if let Some(expires_at) = self
                .active_history_dump
                .as_ref()
                .map(|dump| dump.expires_at)
                .into_iter()
                .chain(self.debug_pause_expires_at)
                .chain(self.display_resume_retry_at)
                .min()
            {
                wake_at = wake_at.min(expires_at);
            }
            tokio::select! {
                _ = tokio::time::sleep_until(wake_at) => {
                    self.expire_history_dump().await;
                    self.expire_debug_pause().await;
                    self.retry_display_resume_cleanup().await;
                    if Instant::now() >= next_poll {
                        self.poll_once().await;
                        next_poll = Instant::now() + self.poll_interval;
                    }
                },
                Some(event) = self.host_rx.recv() => {
                    if event == HostEvent::Shutdown {
                        self.disconnect(DisconnectReason::Shutdown).await;
                        break;
                    }
                    self.handle_host_event(event).await;
                }
                Some(command) = self.command_rx.recv() => self.handle_command(command).await,
                else => break,
            }
        }
    }

    async fn handle_host_event(&mut self, event: HostEvent) {
        let HostEvent::Candidates(mut candidates) = event else {
            return;
        };
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [] => {
                self.retry = None;
                self.disconnect(DisconnectReason::RemovedFromHost).await;
            }
            [port] if self.current_port.as_deref() == Some(port) => {}
            [port] => {
                if !self.retry_is_due(port) {
                    return;
                }
                if self.current_port.is_some() {
                    self.disconnect(DisconnectReason::Replaced).await;
                }
                self.connect(port.clone()).await;
            }
            _ => {
                self.retry = None;
                if matches!(
                    &self.state.connection,
                    ConnectionState::AmbiguousDevice {
                        candidates: existing
                    } if existing == &candidates
                ) {
                    return;
                }
                self.disconnect(DisconnectReason::RemovedFromHost).await;
                self.state.connection = ConnectionState::AmbiguousDevice { candidates };
                self.bump_and_publish();
            }
        }
    }

    async fn connect(&mut self, port: String) {
        self.state.connection = ConnectionState::Connecting { port: port.clone() };
        self.bump_and_publish();
        match self.backend.connect(&port).await {
            Ok(identity) => match self.backend.read_telemetry().await {
                Ok(mut telemetry) => {
                    self.state.session_id += 1;
                    telemetry.session_id = self.state.session_id;
                    telemetry.sequence = self.state.sequence + 1;
                    self.current_port = Some(port.clone());
                    self.state.connected_port = Some(port.clone());
                    self.state.identity = Some(identity);
                    self.state.telemetry = Some(telemetry);
                    self.state.connection = ConnectionState::Ready {
                        session_id: self.state.session_id,
                    };
                    self.consecutive_poll_failures = 0;
                    self.last_successful_poll = Some(Instant::now());
                    self.active_history_dump = None;
                    self.clear_history_cancellation(None);
                    self.debug_pause_expires_at = None;
                    self.display_physically_paused = false;
                    self.display_resume_retry_at = None;
                    self.retry = None;
                    self.bump_and_publish();
                    let _ = self.event_tx.send(DeviceEvent::Connected {
                        sequence: self.state.sequence,
                        session_id: self.state.session_id,
                        port,
                    });
                }
                Err(error) => self.connection_failed(&port, error).await,
            },
            Err(error) => self.connection_failed(&port, error).await,
        }
    }

    async fn connection_failed(&mut self, port: &str, error: DeviceError) {
        self.backend.disconnect().await;
        self.active_history_dump = None;
        self.clear_history_cancellation(None);
        self.debug_pause_expires_at = None;
        self.display_physically_paused = false;
        self.display_resume_retry_at = None;
        self.consecutive_poll_failures = 0;
        self.last_successful_poll = None;
        self.current_port = None;
        self.state.connected_port = None;
        self.state.identity = None;
        self.state.connection = match &error {
            DeviceError::ProtocolUnverified(reason) | DeviceError::Unsupported(reason) => {
                self.retry = Some(RetryState {
                    port: port.to_owned(),
                    attempt: 1,
                    not_before: None,
                });
                ConnectionState::Unsupported {
                    reason: reason.clone(),
                }
            }
            other => {
                let attempt = self
                    .retry
                    .as_ref()
                    .filter(|retry| retry.port == port)
                    .map_or(1, |retry| retry.attempt.saturating_add(1));
                let shift = attempt.saturating_sub(1).min(7);
                let delay = Duration::from_millis(250)
                    .saturating_mul(1_u32 << shift)
                    .min(Duration::from_secs(30));
                self.retry = Some(RetryState {
                    port: port.to_owned(),
                    attempt,
                    not_before: Some(Instant::now() + delay),
                });
                ConnectionState::Recovering {
                    attempt,
                    cause: other.to_string(),
                }
            }
        };
        tracing::warn!(
            port,
            error = %error,
            state = self.state.connection.name(),
            "device connection failed"
        );
        self.bump_and_publish();
    }

    fn retry_is_due(&mut self, port: &str) -> bool {
        let Some(retry) = &self.retry else {
            return true;
        };
        if retry.port != port {
            self.retry = None;
            return true;
        }
        retry
            .not_before
            .is_some_and(|not_before| Instant::now() >= not_before)
    }

    async fn disconnect(&mut self, reason: DisconnectReason) {
        let was_attached = self.current_port.is_some()
            || matches!(
                self.state.connection,
                ConnectionState::Ready { .. } | ConnectionState::Busy { .. }
            );
        if !was_attached {
            if !matches!(self.state.connection, ConnectionState::Absent { .. }) {
                self.state.connection = ConnectionState::Absent {
                    reason: reason.clone(),
                };
                self.state.last_disconnect_reason = Some(reason);
                self.bump_and_publish();
            }
            return;
        }
        self.state.connection = ConnectionState::Disconnecting {
            session_id: self.state.session_id,
            cause: reason.clone(),
        };
        self.bump_and_publish();
        self.backend.disconnect().await;
        self.active_history_dump = None;
        self.clear_history_cancellation(None);
        self.debug_pause_expires_at = None;
        self.display_physically_paused = false;
        self.display_resume_retry_at = None;
        self.current_port = None;
        self.state.connected_port = None;
        if let Some(telemetry) = &mut self.state.telemetry {
            telemetry.stale = true;
        }
        self.state.connection = ConnectionState::Absent {
            reason: reason.clone(),
        };
        self.state.last_disconnect_reason = Some(reason.clone());
        self.bump_and_publish();
        let _ = self.event_tx.send(DeviceEvent::Disconnected {
            sequence: self.state.sequence,
            session_id: self.state.session_id,
            reason,
        });
    }

    async fn poll_once(&mut self) {
        if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
            return;
        }
        match self.backend.read_telemetry().await {
            Ok(mut telemetry) => {
                telemetry.sequence = self.state.sequence + 1;
                telemetry.session_id = self.state.session_id;
                self.state.telemetry = Some(telemetry);
                self.consecutive_poll_failures = 0;
                self.last_successful_poll = Some(Instant::now());
                self.bump_and_publish();
                let _ = self.event_tx.send(DeviceEvent::TelemetryUpdated {
                    sequence: self.state.sequence,
                    session_id: self.state.session_id,
                });
            }
            Err(DeviceError::ConnectionLost) => {
                self.disconnect(DisconnectReason::SerialHangup).await;
            }
            Err(error) => {
                self.consecutive_poll_failures = self.consecutive_poll_failures.saturating_add(1);
                tracing::warn!(
                    %error,
                    consecutive_failures = self.consecutive_poll_failures,
                    "telemetry poll failed"
                );
                if self
                    .last_successful_poll
                    .is_some_and(|last| last.elapsed() >= self.poll_interval.saturating_mul(2))
                    && self
                        .state
                        .telemetry
                        .as_ref()
                        .is_some_and(|telemetry| !telemetry.stale)
                {
                    if let Some(telemetry) = &mut self.state.telemetry {
                        telemetry.stale = true;
                    }
                    self.bump_and_publish();
                }
                if self.consecutive_poll_failures >= 3 {
                    self.disconnect(DisconnectReason::PresenceCheckFailed).await;
                }
            }
        }
    }

    async fn expire_history_dump(&mut self) {
        let Some(dump) = &self.active_history_dump else {
            return;
        };
        if Instant::now() < dump.expires_at {
            return;
        }
        tracing::warn!(dump_id = dump.id, "history dump lease expired");
        self.active_history_dump = None;
        self.clear_history_cancellation(None);
        let result = self.resume_display_if_unowned().await;
        match result {
            Ok(()) => self.set_ready(),
            Err(DeviceError::ConnectionLost) => {
                self.disconnect(DisconnectReason::SerialHangup).await;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to resume display after history dump expiry");
                self.set_ready();
            }
        }
    }

    async fn expire_debug_pause(&mut self) {
        let Some(expires_at) = self.debug_pause_expires_at else {
            return;
        };
        if Instant::now() < expires_at {
            return;
        }
        tracing::info!("debug display-pause lease expired");
        self.debug_pause_expires_at = None;
        match self.resume_display_if_unowned().await {
            Ok(()) => {}
            Err(DeviceError::ConnectionLost) => {
                self.disconnect(DisconnectReason::SerialHangup).await;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to resume display after debug pause expiry");
            }
        }
    }

    async fn ensure_display_paused(&mut self) -> Result<(), DeviceError> {
        if self.display_physically_paused {
            return Ok(());
        }
        self.backend.pause_history_updates().await?;
        self.display_physically_paused = true;
        self.display_resume_retry_at = None;
        Ok(())
    }

    async fn resume_display_if_unowned(&mut self) -> Result<(), DeviceError> {
        if self.active_history_dump.is_some()
            || self.debug_pause_expires_at.is_some()
            || !self.display_physically_paused
        {
            self.display_resume_retry_at = None;
            return Ok(());
        }
        match self.backend.resume_history_updates().await {
            Ok(()) => {
                self.display_physically_paused = false;
                self.display_resume_retry_at = None;
                Ok(())
            }
            Err(error) => {
                if error != DeviceError::ConnectionLost {
                    self.display_resume_retry_at =
                        Some(Instant::now() + DISPLAY_RESUME_RETRY_DELAY);
                }
                Err(error)
            }
        }
    }

    async fn retry_display_resume_cleanup(&mut self) {
        let Some(retry_at) = self.display_resume_retry_at else {
            return;
        };
        if Instant::now() < retry_at
            || !matches!(self.state.connection, ConnectionState::Ready { .. })
        {
            return;
        }
        if self.active_history_dump.is_some()
            || self.debug_pause_expires_at.is_some()
            || !self.display_physically_paused
        {
            self.display_resume_retry_at = None;
            return;
        }
        match self.resume_display_if_unowned().await {
            Ok(()) => {
                tracing::info!("display-resume cleanup succeeded after retry");
                self.bump_and_publish();
            }
            Err(DeviceError::ConnectionLost) => {
                self.disconnect(DisconnectReason::SerialHangup).await;
            }
            Err(error) => {
                tracing::warn!(%error, "display-resume cleanup retry failed");
            }
        }
    }

    fn clear_history_cancellation(&self, dump_id: Option<u64>) {
        let mut active = self
            .history_cancellation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if dump_id.is_none()
            || active
                .as_ref()
                .is_some_and(|(active_id, _)| Some(*active_id) == dump_id)
        {
            *active = None;
        }
    }

    fn display_pause_state(&self) -> DisplayPauseState {
        let remaining_ms = self.debug_pause_expires_at.map_or(0, |expires_at| {
            expires_at
                .saturating_duration_since(Instant::now())
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX)
        });
        DisplayPauseState {
            paused: self.display_physically_paused,
            debug_lease_active: self.debug_pause_expires_at.is_some(),
            history_dump_active: self.active_history_dump.is_some(),
            remaining_ms,
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::SetScreen { screen, reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                let result = self.backend.set_screen(screen).await;
                match result {
                    Ok(()) => {
                        self.bump_and_publish();
                        let _ = self.event_tx.send(DeviceEvent::ScreenChanged {
                            sequence: self.state.sequence,
                            session_id: self.state.session_id,
                            screen,
                        });
                        let _ = reply.send(Ok(()));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::OperationOutcomeUnknown));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::BeginHistoryDump { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let error = if self.active_history_dump.is_some() {
                        DeviceError::Busy("another history dump is active".into())
                    } else {
                        DeviceError::NotConnected
                    };
                    let _ = reply.send(Err(error));
                    return;
                }
                match self.ensure_display_paused().await {
                    Ok(()) => {
                        self.next_history_dump_id = self.next_history_dump_id.saturating_add(1);
                        let dump = HistoryDump {
                            id: self.next_history_dump_id,
                            session_id: self.state.session_id,
                            total_bytes: FLASH_LENGTH,
                        };
                        let cancellation = ReadCancellation::default();
                        *self
                            .history_cancellation
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some((dump.id, cancellation.clone()));
                        self.active_history_dump = Some(ActiveHistoryDump {
                            id: dump.id,
                            session_id: dump.session_id,
                            expires_at: Instant::now() + Duration::from_secs(10),
                            cancellation,
                        });
                        self.set_busy("reading_device_history");
                        let _ = reply.send(Ok(dump));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ReadHistoryDumpChunk {
                dump_id,
                offset,
                length,
                reply,
            } => {
                let Some(dump) = &mut self.active_history_dump else {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "history dump is not active".into(),
                    )));
                    return;
                };
                if dump.id != dump_id || dump.session_id != self.state.session_id {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "history dump belongs to a different session".into(),
                    )));
                    return;
                }
                dump.expires_at = Instant::now() + Duration::from_secs(10);
                let cancellation = dump.cancellation.clone();
                let expires_at = dump.expires_at;
                let lease_cancellation = cancellation.clone();
                let lease_timer = tokio::spawn(async move {
                    tokio::time::sleep_until(expires_at).await;
                    lease_cancellation.cancel();
                });
                let result = self
                    .backend
                    .read_history_chunk(offset, length, &cancellation)
                    .await;
                lease_timer.abort();
                match result {
                    Ok(bytes) => {
                        let _ = reply.send(Ok(bytes));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::EndHistoryDump { dump_id, reply } => {
                let Some(dump) = &self.active_history_dump else {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "history dump is not active".into(),
                    )));
                    return;
                };
                if dump.id != dump_id || dump.session_id != self.state.session_id {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "history dump belongs to a different session".into(),
                    )));
                    return;
                }
                self.active_history_dump = None;
                self.clear_history_cancellation(Some(dump_id));
                match self.resume_display_if_unowned().await {
                    Ok(()) => {
                        self.set_ready();
                        let _ = reply.send(Ok(()));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ReadHistoryChunk {
                offset,
                length,
                reply,
            } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.state.connection = ConnectionState::Busy {
                    session_id: self.state.session_id,
                    operation: "reading_device_history".into(),
                };
                self.bump_and_publish();
                let cancellation = ReadCancellation::default();
                let result = match self.ensure_display_paused().await {
                    Ok(()) => {
                        let read = self
                            .backend
                            .read_history_chunk(offset, length, &cancellation)
                            .await;
                        let resume = self.resume_display_if_unowned().await;
                        match (read, resume) {
                            (Ok(bytes), Ok(())) => Ok(bytes),
                            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok(bytes) => {
                        self.state.connection = ConnectionState::Ready {
                            session_id: self.state.session_id,
                        };
                        self.bump_and_publish();
                        let _ = reply.send(Ok(bytes));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        self.state.connection = ConnectionState::Ready {
                            session_id: self.state.session_id,
                        };
                        self.bump_and_publish();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ReadThemeAsset { slot, reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let error = if self.active_history_dump.is_some() {
                        DeviceError::Busy("a history dump is active".into())
                    } else {
                        DeviceError::NotConnected
                    };
                    let _ = reply.send(Err(error));
                    return;
                }
                self.set_busy("reading_theme_asset");
                if let Err(error) = self.ensure_display_paused().await {
                    if error == DeviceError::ConnectionLost {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                    } else {
                        self.set_ready();
                    }
                    let _ = reply.send(Err(error));
                    return;
                }

                let read = self.backend.read_theme_asset(slot).await;
                let resume = self.resume_display_if_unowned().await;
                if let Err(error) = &resume {
                    tracing::warn!(%error, "failed to resume display after theme read");
                }
                let connection_lost = matches!(&read, Err(DeviceError::ConnectionLost))
                    || matches!(&resume, Err(DeviceError::ConnectionLost));
                if connection_lost {
                    self.disconnect(DisconnectReason::SerialHangup).await;
                } else {
                    self.set_ready();
                }
                // A cleanup failure does not invalidate bytes already read.
                // Connection loss is reflected in daemon state independently.
                let _ = reply.send(read);
            }
            Command::WriteThemeAsset { slot, data, reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let error = if self.active_history_dump.is_some() {
                        DeviceError::Busy("a history dump is active".into())
                    } else {
                        DeviceError::NotConnected
                    };
                    let _ = reply.send(Err(error));
                    return;
                }
                self.set_busy("writing_theme_asset");
                if let Err(error) = self.ensure_display_paused().await {
                    if error == DeviceError::ConnectionLost {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                    } else {
                        self.set_ready();
                    }
                    // Pausing precedes every SPI operation, so its failure
                    // cannot make the flash outcome ambiguous.
                    let _ = reply.send(Err(error));
                    return;
                }

                let write = self.backend.write_theme_asset(slot, &data).await;
                let resume = self.resume_display_if_unowned().await;
                if let Err(error) = &resume {
                    tracing::warn!(%error, "failed to resume display after theme write");
                }
                let connection_lost = matches!(
                    &write,
                    Err(DeviceError::ConnectionLost | DeviceError::ConnectionLostBeforeMutation)
                ) || matches!(&resume, Err(DeviceError::ConnectionLost));
                if connection_lost {
                    self.disconnect(DisconnectReason::SerialHangup).await;
                } else {
                    self.set_ready();
                }

                let result = match write {
                    Err(DeviceError::ConnectionLost) => Err(DeviceError::OperationOutcomeUnknown),
                    Err(DeviceError::ConnectionLostBeforeMutation) => {
                        Err(DeviceError::ConnectionLost)
                    }
                    result => result,
                };
                // Resume failure is lifecycle state, not flash outcome. A
                // verified commit or rollback remains authoritative.
                let _ = reply.send(result);
            }
            Command::ReadDeviceInfo { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("reading_device_information");
                let result = self.backend.read_build_information().await;
                match result {
                    Ok((product_name, build_string)) => {
                        let mut identity = self
                            .state
                            .identity
                            .clone()
                            .ok_or(DeviceError::NotConnected)
                            .expect("ready state has an identity");
                        identity.product_name = product_name;
                        identity.build_string = build_string;
                        self.state.identity = Some(identity.clone());
                        self.set_ready();
                        let _ = reply.send(Ok(identity));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ClearFaults {
                active_mask,
                logged_mask,
                reply,
            } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                if active_mask & !KNOWN_FAULT_MASK != 0 || logged_mask & !KNOWN_FAULT_MASK != 0 {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(format!(
                        "fault masks may contain only known bits {KNOWN_FAULT_MASK:#06x}"
                    ))));
                    return;
                }
                if active_mask == 0 && logged_mask == 0 {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "at least one active or logged fault must be selected".into(),
                    )));
                    return;
                }
                self.set_busy("clearing_faults");
                let result = async {
                    self.backend.clear_faults(active_mask, logged_mask).await?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    self.backend.read_telemetry().await
                }
                .await;
                match result {
                    Ok(mut telemetry) => {
                        telemetry.session_id = self.state.session_id;
                        telemetry.sequence = self.state.sequence + 1;
                        self.state.telemetry = Some(telemetry.clone());
                        self.set_ready();
                        let _ = reply.send(Ok(telemetry));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::OperationOutcomeUnknown));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::GetPollInterval { reply } => {
                let _ = reply.send(
                    self.poll_interval
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                );
            }
            Command::SetPollInterval {
                milliseconds,
                reply,
            } => {
                if !(100..=5000).contains(&milliseconds) {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "poll interval must be from 100 to 5000 milliseconds".into(),
                    )));
                    return;
                }
                self.poll_interval = Duration::from_millis(milliseconds);
                let _ = reply.send(Ok(milliseconds));
            }
            Command::GetDisplayPause { reply } => {
                let _ = reply.send(self.display_pause_state());
            }
            Command::PauseDisplay {
                milliseconds,
                reply,
            } => {
                if !(100..=300_000).contains(&milliseconds) {
                    let _ = reply.send(Err(DeviceError::InvalidArgument(
                        "display pause must be from 100 to 300000 milliseconds".into(),
                    )));
                    return;
                }
                if !matches!(
                    self.state.connection,
                    ConnectionState::Ready { .. } | ConnectionState::Busy { .. }
                ) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                match self.ensure_display_paused().await {
                    Ok(()) => {
                        self.debug_pause_expires_at =
                            Some(Instant::now() + Duration::from_millis(milliseconds));
                        let _ = reply.send(Ok(self.display_pause_state()));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ResumeDisplay { reply } => {
                self.debug_pause_expires_at = None;
                match self.resume_display_if_unowned().await {
                    Ok(()) => {
                        let _ = reply.send(Ok(self.display_pause_state()));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ReadConfiguration { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("reading_configuration");
                let result = self.backend.read_configuration().await;
                match result {
                    Ok(configuration) => {
                        self.set_ready();
                        let _ = reply.send(Ok(configuration));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ReadConfigurationItem { key, reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("reading_configuration_item");
                let result = self
                    .backend
                    .read_configuration()
                    .await
                    .and_then(|configuration| {
                        let value =
                            DeviceSettings::from_configuration(&configuration).item(&key)?;
                        serde_json::to_string(&value).map_err(|error| {
                            DeviceError::Transport(format!(
                                "failed to serialize configuration item: {error}"
                            ))
                        })
                    });
                match result {
                    Ok(value_json) => {
                        self.set_ready();
                        let _ = reply.send(Ok(value_json));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::SerialHangup).await;
                        let _ = reply.send(Err(DeviceError::ConnectionLost));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::ApplyConfiguration {
                configuration,
                persist,
                expected_revision,
                reply,
            } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                let operation = if persist {
                    "storing_configuration"
                } else {
                    "applying_configuration"
                };
                self.set_busy(operation);
                let result = self
                    .apply_configuration(*configuration, persist, expected_revision.as_deref())
                    .await;
                self.finish_configuration_mutation(result, reply).await;
            }
            Command::SetConfigurationItem {
                key,
                value,
                persist,
                reply,
            } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                let operation = if persist {
                    "storing_configuration_item"
                } else {
                    "applying_configuration_item"
                };
                self.set_busy(operation);
                let result = self.set_configuration_item(&key, &value, persist).await;
                self.finish_configuration_mutation(result, reply).await;
            }
            Command::ReloadConfiguration { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("reloading_configuration");
                let result = self.run_nvm_operation(NvmOperation::Reload).await;
                self.finish_configuration_mutation(result, reply).await;
            }
            Command::ResetConfiguration { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("resetting_configuration");
                let result = self.run_nvm_operation(NvmOperation::Reset).await;
                self.finish_configuration_mutation(result, reply).await;
            }
            Command::RebootDevice { reply } => {
                if !matches!(self.state.connection, ConnectionState::Ready { .. }) {
                    let _ = reply.send(Err(DeviceError::NotConnected));
                    return;
                }
                self.set_busy("rebooting_device");
                match self.backend.reboot_device().await {
                    Ok(()) => {
                        self.disconnect(DisconnectReason::DeviceReboot).await;
                        let _ = reply.send(Ok(()));
                    }
                    Err(DeviceError::ConnectionLost) => {
                        self.disconnect(DisconnectReason::DeviceReboot).await;
                        let _ = reply.send(Err(DeviceError::OperationOutcomeUnknown));
                    }
                    Err(error) => {
                        self.set_ready();
                        let _ = reply.send(Err(error));
                    }
                }
            }
        }
    }

    async fn apply_configuration(
        &mut self,
        configuration: DeviceConfiguration,
        persist: bool,
        expected_revision: Option<&str>,
    ) -> Result<DeviceConfiguration, DeviceError> {
        configuration.validate()?;
        let current = self.backend.read_configuration().await?;
        if let Some(expected) = expected_revision {
            let actual = configuration_revision(self.state.session_id, &current)?;
            if expected != actual {
                return Err(DeviceError::RevisionConflict(
                    "configuration changed; run `wireview config show --json` again".into(),
                ));
            }
        }
        if current.raw_version != configuration.raw_version {
            return Err(DeviceError::InvalidArgument(format!(
                "configuration version changed from V{} to V{}",
                configuration.raw_version + 1,
                current.raw_version + 1
            )));
        }
        if current.crc != configuration.crc {
            return Err(DeviceError::InvalidArgument(
                "configuration is stale; run `wireview config show --json` again".into(),
            ));
        }
        let operation = async {
            self.backend.write_configuration(&configuration).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let applied = self.backend.read_configuration().await?;
            if applied != configuration {
                return Err(DeviceError::VerificationFailed(
                    "configuration readback did not match the requested values".into(),
                ));
            }
            if persist {
                self.wait_for_nvm_mutation_slot().await;
                self.backend.nvm_configuration(NvmOperation::Store).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stored = self.backend.read_configuration().await?;
                stored.validate()?;
                if stored.raw_version != configuration.raw_version
                    || DeviceSettings::from_configuration(&stored)
                        != DeviceSettings::from_configuration(&configuration)
                {
                    return Err(DeviceError::VerificationFailed(
                        "stored configuration did not match the requested values".into(),
                    ));
                }
                return Ok(stored);
            }
            Ok(applied)
        }
        .await;
        match operation {
            Ok(configuration) => Ok(configuration),
            Err(DeviceError::ConnectionLost) => Err(DeviceError::ConnectionLost),
            Err(error) => {
                let operation_message = error.to_string();
                match self.rollback_configuration(&current, persist).await {
                    Ok(()) => Err(DeviceError::FailedAndRolledBack(operation_message)),
                    Err(DeviceError::ConnectionLost) => Err(DeviceError::ConnectionLost),
                    Err(rollback) => Err(DeviceError::RollbackFailed {
                        operation: operation_message,
                        rollback: rollback.to_string(),
                    }),
                }
            }
        }
    }

    async fn rollback_configuration(
        &mut self,
        original: &DeviceConfiguration,
        persist: bool,
    ) -> Result<(), DeviceError> {
        self.backend.write_configuration(original).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let restored = self.backend.read_configuration().await?;
        if restored != *original {
            return Err(DeviceError::VerificationFailed(
                "active configuration rollback did not restore the original bytes".into(),
            ));
        }
        if persist {
            self.last_nvm_mutation = Some(Instant::now());
            self.backend.nvm_configuration(NvmOperation::Store).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stored = self.backend.read_configuration().await?;
            stored.validate()?;
            if stored.raw_version != original.raw_version
                || DeviceSettings::from_configuration(&stored)
                    != DeviceSettings::from_configuration(original)
            {
                return Err(DeviceError::VerificationFailed(
                    "stored configuration rollback did not restore the original settings".into(),
                ));
            }
        }
        Ok(())
    }

    async fn set_configuration_item(
        &mut self,
        key: &str,
        value: &str,
        persist: bool,
    ) -> Result<DeviceConfiguration, DeviceError> {
        let current = self.backend.read_configuration().await?;
        let settings = DeviceSettings::from_configuration(&current).with_item(key, value)?;
        let configuration = settings.with_protocol_metadata(&current)?;
        self.apply_configuration(configuration, persist, None).await
    }

    async fn run_nvm_operation(
        &mut self,
        operation: NvmOperation,
    ) -> Result<DeviceConfiguration, DeviceError> {
        let rollback = if operation == NvmOperation::Reload {
            Some(self.backend.read_configuration().await?)
        } else {
            None
        };
        if operation == NvmOperation::Reset {
            self.wait_for_nvm_mutation_slot().await;
        }
        self.backend.nvm_configuration(operation).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let loaded = self
            .backend
            .read_configuration()
            .await
            .and_then(|configuration| {
                configuration.validate()?;
                if rollback
                    .as_ref()
                    .is_some_and(|previous| previous.raw_version != configuration.raw_version)
                {
                    return Err(DeviceError::InvalidArgument(
                        "stored configuration has a different format version".into(),
                    ));
                }
                Ok(configuration)
            });
        match loaded {
            Ok(configuration) => Ok(configuration),
            Err(load_error) => {
                let Some(previous) = rollback else {
                    return Err(load_error);
                };
                self.backend.write_configuration(&previous).await?;
                tokio::time::sleep(Duration::from_millis(50)).await;
                let restored = self.backend.read_configuration().await?;
                if restored != previous {
                    return Err(DeviceError::Transport(
                        "saved configuration was invalid and restoring the active settings failed"
                            .into(),
                    ));
                }
                Err(DeviceError::InvalidArgument(format!(
                    "saved configuration is invalid; the previous active settings were restored \
                     ({load_error})"
                )))
            }
        }
    }

    async fn wait_for_nvm_mutation_slot(&mut self) {
        if let Some(last) = self.last_nvm_mutation {
            let elapsed = last.elapsed();
            if elapsed < MIN_NVM_MUTATION_INTERVAL {
                tokio::time::sleep(MIN_NVM_MUTATION_INTERVAL - elapsed).await;
            }
        }
        self.last_nvm_mutation = Some(Instant::now());
    }

    async fn finish_configuration_mutation(
        &mut self,
        result: Result<DeviceConfiguration, DeviceError>,
        reply: oneshot::Sender<Result<DeviceConfiguration, DeviceError>>,
    ) {
        match result {
            Ok(configuration) => {
                self.set_ready();
                let _ = reply.send(Ok(configuration));
            }
            Err(DeviceError::ConnectionLost) => {
                self.disconnect(DisconnectReason::SerialHangup).await;
                let _ = reply.send(Err(DeviceError::OperationOutcomeUnknown));
            }
            Err(error) => {
                self.set_ready();
                let _ = reply.send(Err(error));
            }
        }
    }

    fn set_busy(&mut self, operation: &str) {
        self.state.connection = ConnectionState::Busy {
            session_id: self.state.session_id,
            operation: operation.into(),
        };
        self.bump_and_publish();
    }

    fn set_ready(&mut self) {
        self.state.connection = ConnectionState::Ready {
            session_id: self.state.session_id,
        };
        self.bump_and_publish();
    }

    fn bump_and_publish(&mut self) {
        self.state.sequence += 1;
        self.state_tx.send_replace(self.state.clone());
    }
}
