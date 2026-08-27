use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use wireview_core::config::{DeviceConfiguration, DeviceSettings};
use wireview_core::history::{FLASH_LENGTH, HistoryEntry, visit_history};
use wireview_core::theme::{ThemeAssetSlot, sha256_hex};
use wireview_ipc::{
    API_CAPABILITIES, API_VERSION, ConfigurationDto, DeviceInfoDto, StatusDto, TelemetryDto,
    ThemeAssetDto, WireViewError, WireViewProxy, api_compatibility_id, validate_status,
};

use crate::export::{ExportError, HistoryFormat, export_bytes, export_history};

const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const HISTORY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(7);
const HISTORY_CHUNK_SIZE: usize = 16 * 1024;
const HISTORY_ROW_LIMIT: usize = 80;

pub(crate) type EventSink = Arc<dyn Fn(UiEvent) + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum DemoKind {
    Ready,
    Fault,
    Stale,
    Offline,
}

#[derive(Clone, Debug)]
pub(crate) struct PinSample {
    pub(crate) current: f64,
    pub(crate) voltage: f64,
    pub(crate) power: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct TelemetrySnapshot {
    pub(crate) sequence: u64,
    pub(crate) session_id: u64,
    pub(crate) observed_at_ms: u64,
    pub(crate) stale: bool,
    pub(crate) controller_vdd: f64,
    pub(crate) average_voltage: f64,
    pub(crate) total_current: f64,
    pub(crate) total_power: f64,
    pub(crate) fan_duty: f64,
    pub(crate) cable_capability: u16,
    pub(crate) pins: [PinSample; 6],
    pub(crate) input_temperature: f64,
    pub(crate) output_temperature: f64,
    pub(crate) external_1_temperature: Option<f64>,
    pub(crate) external_2_temperature: Option<f64>,
    pub(crate) active_fault_mask: u16,
    pub(crate) logged_fault_mask: u16,
    pub(crate) unknown_active_fault_mask: u16,
    pub(crate) unknown_logged_fault_mask: u16,
}

impl TryFrom<TelemetryDto> for TelemetrySnapshot {
    type Error = ClientError;

    fn try_from(value: TelemetryDto) -> Result<Self, Self::Error> {
        let voltages: [f64; 6] = value
            .pin_voltages_v
            .try_into()
            .map_err(|values: Vec<f64>| {
                ClientError::Protocol(format!(
                    "wireviewd returned {} pin voltages, expected 6",
                    values.len()
                ))
            })?;
        let currents: [f64; 6] = value
            .pin_currents_a
            .try_into()
            .map_err(|values: Vec<f64>| {
                ClientError::Protocol(format!(
                    "wireviewd returned {} pin currents, expected 6",
                    values.len()
                ))
            })?;
        let powers: [f64; 6] = value.pin_power_w.try_into().map_err(|values: Vec<f64>| {
            ClientError::Protocol(format!(
                "wireviewd returned {} pin powers, expected 6",
                values.len()
            ))
        })?;
        let pins = std::array::from_fn(|index| PinSample {
            current: currents[index],
            voltage: voltages[index],
            power: powers[index],
        });
        let finite_values = [
            value.vdd_v,
            value.avg_voltage_v,
            value.total_current_a,
            value.total_power_w,
            value.fan_duty_percent,
            value.input_temp_c,
            value.output_temp_c,
        ];
        if finite_values.into_iter().any(|number| !number.is_finite())
            || (value.external_1_present && !value.external_1_temp_c.is_finite())
            || (value.external_2_present && !value.external_2_temp_c.is_finite())
            || pins.iter().any(|pin| {
                !pin.current.is_finite() || !pin.voltage.is_finite() || !pin.power.is_finite()
            })
        {
            return Err(ClientError::Protocol(
                "wireviewd returned a non-finite telemetry value".into(),
            ));
        }
        if value.unknown_active_fault_mask != (value.active_fault_mask & !0x003f)
            || value.unknown_logged_fault_mask != (value.logged_fault_mask & !0x003f)
        {
            return Err(ClientError::Protocol(
                "wireviewd returned inconsistent fault-register masks".into(),
            ));
        }
        Ok(Self {
            sequence: value.sequence,
            session_id: value.session_id,
            observed_at_ms: value.observed_at_ms,
            stale: value.stale,
            controller_vdd: value.vdd_v,
            average_voltage: value.avg_voltage_v,
            total_current: value.total_current_a,
            total_power: value.total_power_w,
            fan_duty: value.fan_duty_percent,
            cable_capability: value.cable_capability_w,
            pins,
            input_temperature: value.input_temp_c,
            output_temperature: value.output_temp_c,
            external_1_temperature: value.external_1_present.then_some(value.external_1_temp_c),
            external_2_temperature: value.external_2_present.then_some(value.external_2_temp_c),
            active_fault_mask: value.active_fault_mask,
            logged_fault_mask: value.logged_fault_mask,
            unknown_active_fault_mask: value.unknown_active_fault_mask,
            unknown_logged_fault_mask: value.unknown_logged_fault_mask,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryRowData {
    pub(crate) device_time_ms: u32,
    pub(crate) total_power: f64,
    pub(crate) total_current: f64,
    pub(crate) average_voltage: f64,
    pub(crate) input_temperature: f64,
}

#[derive(Clone, Debug)]
pub(crate) enum UiEvent {
    Offline(String),
    SessionChanged,
    Status(StatusDto),
    Telemetry(TelemetrySnapshot),
    DeviceInfo(DeviceInfoDto),
    Configuration {
        settings: DeviceSettings,
        poll_interval_ms: u64,
    },
    ScreenChanged(String),
    Operation {
        state: OperationState,
        message: String,
    },
    HistoryProgress {
        fraction: f64,
        message: String,
        active: bool,
    },
    HistoryLoaded {
        entries: usize,
        end_found: bool,
        rows: Vec<HistoryRowData>,
    },
    ThemeAsset {
        slot: ThemeAssetSlot,
        width: u32,
        height: u32,
        sha256: String,
        data: Vec<u8>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationState {
    Running,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigEdits {
    items: Vec<ConfigEdit>,
    poll_interval_ms: u64,
}

#[derive(Clone, Debug)]
struct ConfigEdit {
    key: &'static str,
    value: String,
}

impl ConfigEdits {
    pub(crate) fn new(
        items: impl IntoIterator<Item = (&'static str, String)>,
        poll_interval_ms: u64,
    ) -> Result<Self, ClientError> {
        if !(100..=5000).contains(&poll_interval_ms) {
            return Err(ClientError::InvalidInput(
                "poll interval must be between 100 and 5000 milliseconds".into(),
            ));
        }
        Ok(Self {
            items: items
                .into_iter()
                .map(|(key, value)| ConfigEdit { key, value })
                .collect(),
            poll_interval_ms,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Command {
    Refresh,
    SetScreen(String),
    ClearFaults { active_mask: u16, logged_mask: u16 },
    ApplyConfig { edits: ConfigEdits, persist: bool },
    ReloadConfig,
    ResetConfig,
    RebootDevice,
    LoadHistory,
    CancelHistory,
    ExportHistory { format: String, path: PathBuf },
    ReadTheme(ThemeAssetSlot),
    ExportTheme { slot: ThemeAssetSlot, path: PathBuf },
    WriteTheme { slot: ThemeAssetSlot, path: PathBuf },
    Shutdown,
}

pub(crate) struct WorkerHandle {
    sender: mpsc::UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub(crate) fn sender(&self) -> mpsc::UnboundedSender<Command> {
        self.sender.clone()
    }

    pub(crate) fn stop(mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn start_worker(socket: PathBuf, events: EventSink) -> WorkerHandle {
    let (sender, receiver) = mpsc::unbounded_channel();
    let thread = std::thread::Builder::new()
        .name("wireview-gui-ipc".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create the desktop IPC runtime");
            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(Worker::new(socket, events, receiver).run()));
        })
        .expect("failed to start the desktop IPC thread");
    WorkerHandle {
        sender,
        thread: Some(thread),
    }
}

struct Worker {
    socket: PathBuf,
    events: EventSink,
    commands: mpsc::UnboundedReceiver<Command>,
    connection: Option<zlink::tokio::unix::Connection>,
    next_connect: Instant,
    reconnect_delay: Duration,
    session_id: Option<u64>,
    device_info_loaded: bool,
    configuration: Option<ConfigurationDto>,
    poll_interval_ms: u64,
    history: Option<HistoryData>,
    history_cancel: Option<Arc<AtomicBool>>,
    history_generation: u64,
    history_results_tx: mpsc::UnboundedSender<(u64, Result<HistoryData, ClientError>)>,
    history_results_rx: mpsc::UnboundedReceiver<(u64, Result<HistoryData, ClientError>)>,
    themes: HashMap<ThemeAssetSlot, Vec<u8>>,
}

impl Worker {
    fn new(socket: PathBuf, events: EventSink, commands: mpsc::UnboundedReceiver<Command>) -> Self {
        let (history_results_tx, history_results_rx) = mpsc::unbounded_channel();
        Self {
            socket,
            events,
            commands,
            connection: None,
            next_connect: Instant::now(),
            reconnect_delay: Duration::from_millis(500),
            session_id: None,
            device_info_loaded: false,
            configuration: None,
            poll_interval_ms: 500,
            history: None,
            history_cancel: None,
            history_generation: 0,
            history_results_tx,
            history_results_rx,
            themes: HashMap::new(),
        }
    }

    async fn run(mut self) {
        let mut interval = tokio::time::interval(MIN_REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else { break };
                    if matches!(command, Command::Shutdown) {
                        if let Some(cancel) = &self.history_cancel {
                            cancel.store(true, Ordering::Relaxed);
                            let generation = self.history_generation;
                            let _ = tokio::time::timeout(
                                HISTORY_SHUTDOWN_TIMEOUT,
                                async {
                                    while let Some((completed, _)) =
                                        self.history_results_rx.recv().await
                                    {
                                        if completed == generation {
                                            break;
                                        }
                                    }
                                },
                            )
                            .await;
                        }
                        break;
                    }
                    self.handle_command(command).await;
                }
                result = self.history_results_rx.recv() => {
                    if let Some((generation, result)) = result
                        && generation == self.history_generation
                    {
                        self.finish_history(result);
                    }
                }
                _ = interval.tick() => {
                    self.refresh().await;
                    interval.reset_after(self.refresh_interval());
                }
            }
        }
    }

    fn refresh_interval(&self) -> Duration {
        effective_refresh_interval(self.poll_interval_ms, self.connection.is_some())
    }

    async fn refresh(&mut self) {
        if self.connection.is_none() {
            if Instant::now() < self.next_connect {
                return;
            }
            match connect(&self.socket).await {
                Ok(connection) => {
                    self.connection = Some(connection);
                    self.reconnect_delay = Duration::from_millis(500);
                }
                Err(error) => {
                    self.connection_failed(error);
                    return;
                }
            }
        }

        let result = self.refresh_connected().await;
        if let Err(error) = result {
            self.connection_failed(error);
        }
    }

    async fn refresh_connected(&mut self) -> Result<(), ClientError> {
        let status = get_status(self.connection_mut()?).await?;
        validate_status(&status)?;
        self.poll_interval_ms = status.poll_interval_ms;
        (self.events)(UiEvent::Status(status.clone()));

        if self.session_id != Some(status.session_id) {
            self.session_id = Some(status.session_id);
            self.device_info_loaded = false;
            self.configuration = None;
            self.history = None;
            self.themes.clear();
            self.invalidate_history_task();
            (self.events)(UiEvent::SessionChanged);
        }

        if (status.state == "ready" || status.state == "busy")
            && (!self.device_info_loaded || self.configuration.is_none())
        {
            self.load_session_data(false).await;
        }

        if status.state == "ready" || status.state == "busy" {
            match rpc(self.connection_mut()?.get_telemetry()).await {
                Ok(telemetry) => (self.events)(UiEvent::Telemetry(telemetry.try_into()?)),
                Err(ClientError::Remote(WireViewError::Unavailable { .. })) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn load_session_data(&mut self, force: bool) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        if (force || !self.device_info_loaded)
            && let Ok(info) = rpc(connection.get_device_info()).await
        {
            self.device_info_loaded = true;
            (self.events)(UiEvent::DeviceInfo(info));
        }
        if (force || self.configuration.is_none())
            && let Ok(configuration) = rpc(connection.get_configuration()).await
        {
            match DeviceSettings::from_json(&configuration.configuration_json) {
                Ok(settings) => {
                    self.configuration = Some(configuration);
                    (self.events)(UiEvent::Configuration {
                        settings,
                        poll_interval_ms: self.poll_interval_ms,
                    });
                }
                Err(error) => self.operation_error(error.to_string()),
            }
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Refresh => {
                self.next_connect = Instant::now();
                self.refresh().await;
                if self.connection.is_some() {
                    self.load_session_data(true).await;
                }
            }
            Command::SetScreen(screen) => {
                self.operation_running("Changing the device screen");
                let result = self.set_screen(&screen).await;
                self.finish_operation(result.map(|active| {
                    (self.events)(UiEvent::ScreenChanged(active.clone()));
                    format!("Showing {active} on the device")
                }));
            }
            Command::ClearFaults {
                active_mask,
                logged_mask,
            } => {
                self.operation_running("Clearing the selected device fault");
                let result = self.clear_faults(active_mask, logged_mask).await;
                let result = result
                    .and_then(TelemetrySnapshot::try_from)
                    .map(|snapshot| {
                        (self.events)(UiEvent::Telemetry(snapshot));
                        "Fault register refreshed".into()
                    });
                self.finish_operation(result);
            }
            Command::ApplyConfig { edits, persist } => {
                self.operation_running(if persist {
                    "Storing configuration"
                } else {
                    "Applying active configuration"
                });
                let result = self.apply_configuration(edits, persist).await;
                self.finish_operation(result.map(|change| change.message(persist).into()));
            }
            Command::ReloadConfig => {
                self.operation_running("Reloading stored configuration");
                let result = self.reload_configuration().await;
                self.finish_operation(result.map(|()| "Stored configuration reloaded".into()));
            }
            Command::ResetConfig => {
                self.operation_running("Resetting stored configuration");
                let result = self.reset_configuration().await;
                self.finish_operation(result.map(|()| "Factory configuration restored".into()));
            }
            Command::RebootDevice => {
                self.operation_running("Rebooting the device");
                let result = self.reboot_device().await;
                self.finish_operation(result.map(|()| "Device reboot accepted".into()));
            }
            Command::LoadHistory => self.start_history(),
            Command::CancelHistory => {
                if let Some(cancel) = &self.history_cancel {
                    cancel.store(true, Ordering::Relaxed);
                    (self.events)(UiEvent::HistoryProgress {
                        fraction: 0.0,
                        message: "Cancelling history read".into(),
                        active: true,
                    });
                }
            }
            Command::ExportHistory { format, path } => {
                self.operation_running("Exporting device history");
                let result = self.export_history(&format, &path);
                self.finish_operation(result.map(|count| {
                    if format == "raw" {
                        format!("Exact history bytes written to {}", path.display())
                    } else {
                        format!("{count} history entries written to {}", path.display())
                    }
                }));
            }
            Command::ReadTheme(slot) => {
                self.operation_running("Reading the selected theme slot");
                let result = self.read_theme(slot).await;
                self.finish_operation(result.map(|()| format!("Read {slot}")));
            }
            Command::ExportTheme { slot, path } => {
                self.operation_running("Exporting the theme backup");
                let result = self.export_theme(slot, &path);
                self.finish_operation(
                    result.map(|()| format!("Theme backup written to {}", path.display())),
                );
            }
            Command::WriteTheme { slot, path } => {
                self.operation_running("Replacing and verifying the selected theme slot");
                let result = self.write_theme(slot, &path).await;
                self.finish_operation(result.map(|()| format!("Replaced and verified {slot}")));
            }
            Command::Shutdown => unreachable!("shutdown is handled by the worker loop"),
        }
    }

    async fn set_screen(&mut self, screen: &str) -> Result<String, ClientError> {
        let connection = self.connection_mut()?;
        Ok(rpc(connection.set_screen(screen))
            .await?
            .active
            .to_ascii_lowercase())
    }

    async fn clear_faults(
        &mut self,
        active_mask: u16,
        logged_mask: u16,
    ) -> Result<TelemetryDto, ClientError> {
        let connection = self.connection_mut()?;
        rpc(connection.clear_faults(active_mask, logged_mask, true)).await
    }

    async fn apply_configuration(
        &mut self,
        edits: ConfigEdits,
        persist: bool,
    ) -> Result<ConfigurationChange, ClientError> {
        let current = self
            .configuration
            .clone()
            .ok_or_else(|| ClientError::InvalidInput("configuration is not loaded".into()))?;
        let (candidate, settings_changed) = edited_configuration(&current, &edits)?;
        let poll_changed = edits.poll_interval_ms != self.poll_interval_ms;
        let previous_poll_interval = self.poll_interval_ms;
        if poll_changed {
            let poll = rpc(self
                .connection_mut()?
                .set_poll_interval(edits.poll_interval_ms))
            .await?;
            if poll.milliseconds != edits.poll_interval_ms {
                return Err(ClientError::Protocol(format!(
                    "wireviewd applied a {} ms poll interval after {} ms was requested",
                    poll.milliseconds, edits.poll_interval_ms
                )));
            }
            self.poll_interval_ms = poll.milliseconds;
        }

        let updated = if settings_changed {
            let result = if persist {
                rpc(self.connection_mut()?.store_configuration(candidate, true)).await
            } else {
                rpc(self.connection_mut()?.apply_configuration(candidate)).await
            };
            match result {
                Ok(configuration) => configuration,
                Err(error) => {
                    if poll_changed && !error.disconnects_client() {
                        let rollback = rpc(self
                            .connection_mut()?
                            .set_poll_interval(previous_poll_interval))
                        .await;
                        match rollback {
                            Ok(poll) if poll.milliseconds == previous_poll_interval => {
                                self.poll_interval_ms = previous_poll_interval;
                            }
                            Ok(poll) => {
                                return Err(ClientError::Protocol(format!(
                                    "configuration failed ({error}); poll rollback requested {} ms but wireviewd applied {} ms",
                                    previous_poll_interval, poll.milliseconds
                                )));
                            }
                            Err(rollback) => {
                                return Err(ClientError::Protocol(format!(
                                    "configuration failed ({error}); poll rollback failed ({rollback})"
                                )));
                            }
                        }
                    }
                    return Err(error);
                }
            }
        } else {
            current
        };
        let settings = DeviceSettings::from_json(&updated.configuration_json)?;
        self.configuration = Some(updated);
        (self.events)(UiEvent::Configuration {
            settings,
            poll_interval_ms: self.poll_interval_ms,
        });
        Ok(ConfigurationChange {
            settings_changed,
            poll_changed,
        })
    }

    async fn reload_configuration(&mut self) -> Result<(), ClientError> {
        let connection = self.connection_mut()?;
        let configuration = rpc(connection.reload_configuration()).await?;
        self.publish_configuration(configuration)
    }

    async fn reset_configuration(&mut self) -> Result<(), ClientError> {
        let connection = self.connection_mut()?;
        let configuration = rpc(connection.reset_configuration(true)).await?;
        self.publish_configuration(configuration)
    }

    async fn reboot_device(&mut self) -> Result<(), ClientError> {
        let connection = self.connection_mut()?;
        let response = rpc(connection.reboot_device(true)).await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ClientError::Protocol(
                "wireviewd did not accept the reboot request".into(),
            ))
        }
    }

    fn publish_configuration(
        &mut self,
        configuration: ConfigurationDto,
    ) -> Result<(), ClientError> {
        let settings = DeviceSettings::from_json(&configuration.configuration_json)?;
        self.configuration = Some(configuration);
        (self.events)(UiEvent::Configuration {
            settings,
            poll_interval_ms: self.poll_interval_ms,
        });
        Ok(())
    }

    fn start_history(&mut self) {
        if self.history_cancel.is_some() {
            self.operation_error("A history read is already active");
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.history_cancel = Some(cancel.clone());
        self.history_generation = self.history_generation.wrapping_add(1);
        let generation = self.history_generation;
        let socket = self.socket.clone();
        let events = self.events.clone();
        let results = self.history_results_tx.clone();
        tokio::task::spawn_local(async move {
            let result = download_history(&socket, &cancel, &events).await;
            let _ = results.send((generation, result));
        });
    }

    fn invalidate_history_task(&mut self) {
        self.history_generation = self.history_generation.wrapping_add(1);
        if let Some(cancel) = self.history_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn finish_history(&mut self, result: Result<HistoryData, ClientError>) {
        self.history_cancel = None;
        match result {
            Ok(history) => {
                (self.events)(UiEvent::HistoryLoaded {
                    entries: history.entries,
                    end_found: history.end_found,
                    rows: history.rows.clone(),
                });
                self.history = Some(history);
                self.operation_success("Device history loaded");
            }
            Err(ClientError::Cancelled) => {
                (self.events)(UiEvent::HistoryProgress {
                    fraction: 0.0,
                    message: "History read cancelled".into(),
                    active: false,
                });
            }
            Err(error) => {
                (self.events)(UiEvent::HistoryProgress {
                    fraction: 0.0,
                    message: error.to_string(),
                    active: false,
                });
                self.operation_error(error.to_string());
            }
        }
    }

    fn export_history(&self, format: &str, path: &Path) -> Result<usize, ClientError> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| ClientError::InvalidInput("load device history first".into()))?;
        export_history(&history.bytes, HistoryFormat::parse(format)?, path).map_err(Into::into)
    }

    async fn read_theme(&mut self, slot: ThemeAssetSlot) -> Result<(), ClientError> {
        let connection = self.connection_mut()?;
        let asset = rpc(connection.read_theme_asset(slot.name())).await?;
        validate_theme_asset(slot, &asset)?;
        self.themes.insert(slot, asset.data.clone());
        (self.events)(UiEvent::ThemeAsset {
            slot,
            width: asset.width,
            height: asset.height,
            sha256: asset.sha256,
            data: asset.data,
        });
        Ok(())
    }

    fn export_theme(&self, slot: ThemeAssetSlot, path: &Path) -> Result<(), ClientError> {
        let bytes = self.themes.get(&slot).ok_or_else(|| {
            ClientError::InvalidInput(format!("read {slot} before exporting its backup"))
        })?;
        export_bytes(bytes, path).map_err(Into::into)
    }

    async fn write_theme(&mut self, slot: ThemeAssetSlot, path: &Path) -> Result<(), ClientError> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            ClientError::InvalidInput(format!("failed to inspect {}: {error}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(ClientError::InvalidInput(
                "theme input must be a regular file".into(),
            ));
        }
        let data = std::fs::read(path).map_err(|error| {
            ClientError::InvalidInput(format!("failed to read {}: {error}", path.display()))
        })?;
        if data.len() != slot.byte_len() {
            return Err(ClientError::InvalidInput(format!(
                "{slot} must be exactly {} bytes",
                slot.byte_len()
            )));
        }
        let digest = sha256_hex(&data);
        let connection = self.connection_mut()?;
        let result = rpc(connection.write_theme_asset(
            slot.name(),
            u32::try_from(data.len()).expect("theme slot length fits u32"),
            &digest,
            data.clone(),
            true,
        ))
        .await?;
        if result.slot != slot.name()
            || result.byte_length != u32::try_from(data.len()).expect("theme length fits u32")
            || result.sha256 != digest
        {
            return Err(ClientError::Protocol(
                "wireviewd returned inconsistent theme write metadata".into(),
            ));
        }
        self.themes.insert(slot, data.clone());
        (self.events)(UiEvent::ThemeAsset {
            slot,
            width: slot.width(),
            height: slot.height(),
            sha256: digest,
            data,
        });
        Ok(())
    }

    fn connection_mut(&mut self) -> Result<&mut zlink::tokio::unix::Connection, ClientError> {
        self.connection.as_mut().ok_or(ClientError::Disconnected)
    }

    fn connection_failed(&mut self, error: ClientError) {
        let history_was_active = self.history_cancel.is_some();
        self.invalidate_history_task();
        self.connection = None;
        self.session_id = None;
        self.next_connect = Instant::now() + self.reconnect_delay;
        self.reconnect_delay = (self.reconnect_delay * 2).min(Duration::from_secs(8));
        if history_was_active {
            (self.events)(UiEvent::HistoryProgress {
                fraction: 0.0,
                message: "History read stopped because the daemon connection was lost".into(),
                active: false,
            });
        }
        (self.events)(UiEvent::Offline(error.to_string()));
    }

    fn finish_operation(&mut self, result: Result<String, ClientError>) {
        match result {
            Ok(message) => self.operation_success(message),
            Err(error) => {
                if error.disconnects_client() {
                    self.connection_failed(error);
                } else {
                    self.operation_error(error.to_string());
                }
            }
        }
    }

    fn operation_running(&self, message: impl Into<String>) {
        (self.events)(UiEvent::Operation {
            state: OperationState::Running,
            message: message.into(),
        });
    }

    fn operation_success(&self, message: impl Into<String>) {
        (self.events)(UiEvent::Operation {
            state: OperationState::Success,
            message: message.into(),
        });
    }

    fn operation_error(&self, message: impl Into<String>) {
        (self.events)(UiEvent::Operation {
            state: OperationState::Error,
            message: message.into(),
        });
    }
}

fn effective_refresh_interval(poll_interval_ms: u64, connected: bool) -> Duration {
    if connected {
        Duration::from_millis(poll_interval_ms.clamp(500, 5_000))
    } else {
        MIN_REFRESH_INTERVAL
    }
}

#[derive(Debug)]
struct HistoryData {
    bytes: Vec<u8>,
    entries: usize,
    end_found: bool,
    rows: Vec<HistoryRowData>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConfigurationChange {
    settings_changed: bool,
    poll_changed: bool,
}

impl ConfigurationChange {
    const fn message(self, persist: bool) -> &'static str {
        match (self.settings_changed, self.poll_changed, persist) {
            (true, true, true) => {
                "Device configuration stored; daemon polling updated for this run"
            }
            (true, false, true) => "Configuration stored permanently",
            (false, true, true) => {
                "Daemon polling updated for this run; no device settings changed"
            }
            (true, true, false) => "Configuration and daemon polling applied until reload",
            (true, false, false) => "Configuration applied until reload",
            (false, true, false) => "Daemon polling updated for this run",
            (false, false, _) => "No effective configuration changes",
        }
    }
}

impl HistoryData {
    fn parse(bytes: Vec<u8>) -> Self {
        let mut rows = VecDeque::with_capacity(HISTORY_ROW_LIMIT);
        let summary = visit_history(&bytes, |entry: HistoryEntry| {
            if rows.len() == HISTORY_ROW_LIMIT {
                rows.pop_front();
            }
            rows.push_back(HistoryRowData {
                device_time_ms: entry.device_time_ms,
                total_power: entry.metrics.total_power_w,
                total_current: entry.metrics.total_current_a,
                average_voltage: entry.metrics.avg_voltage_v,
                input_temperature: entry.metrics.temperatures.input_c,
            });
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("an infallible history visitor cannot fail");
        Self {
            bytes,
            entries: summary.entries,
            end_found: summary.end_found,
            rows: rows.into_iter().collect(),
        }
    }
}

async fn download_history(
    socket: &Path,
    cancel: &AtomicBool,
    events: &EventSink,
) -> Result<HistoryData, ClientError> {
    (events)(UiEvent::HistoryProgress {
        fraction: 0.0,
        message: "Opening a device history lease".into(),
        active: true,
    });
    let mut connection = connect(socket).await?;
    let status = get_status(&mut connection).await?;
    validate_status(&status)?;
    let dump = rpc(connection.begin_history_dump()).await?;
    let expected = usize::try_from(dump.total_bytes)
        .map_err(|_| ClientError::Protocol("history size does not fit this platform".into()))?;
    if expected != FLASH_LENGTH {
        let _ = rpc(connection.end_history_dump(dump.dump_id)).await;
        return Err(ClientError::Protocol(format!(
            "wireviewd reported {expected} history bytes, expected {FLASH_LENGTH}"
        )));
    }
    if dump.session_id != status.session_id {
        let _ = rpc(connection.end_history_dump(dump.dump_id)).await;
        return Err(ClientError::Protocol(format!(
            "history lease belongs to session {}, expected {}",
            dump.session_id, status.session_id
        )));
    }

    let download = async {
        let mut bytes = Vec::with_capacity(expected);
        while bytes.len() < expected {
            if cancel.load(Ordering::Relaxed) {
                return Err(ClientError::Cancelled);
            }
            let remaining = expected - bytes.len();
            let length = remaining.min(HISTORY_CHUNK_SIZE);
            let offset = u32::try_from(bytes.len()).expect("history offset fits u32");
            let chunk = rpc(connection.read_history_dump_chunk(
                dump.dump_id,
                offset,
                u32::try_from(length).expect("history chunk length fits u32"),
            ))
            .await?;
            if chunk.offset != offset
                || usize::try_from(chunk.total_bytes).ok() != Some(expected)
                || chunk.data.is_empty()
                || chunk.data.len() > length
            {
                return Err(ClientError::Protocol(
                    "wireviewd returned an invalid history chunk".into(),
                ));
            }
            bytes.extend_from_slice(&chunk.data);
            (events)(UiEvent::HistoryProgress {
                fraction: bytes.len() as f64 / expected as f64,
                message: format!("Read {} of {} KiB", bytes.len() / 1024, expected / 1024),
                active: true,
            });
        }
        Ok(HistoryData::parse(bytes))
    }
    .await;
    let cleanup = rpc(connection.end_history_dump(dump.dump_id)).await;
    match (download, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(history), Ok(())) => Ok(history),
    }
}

async fn connect(path: &Path) -> Result<zlink::tokio::unix::Connection, ClientError> {
    tokio::time::timeout(RPC_TIMEOUT, zlink::tokio::unix::connect(path))
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(ClientError::Transport)
}

async fn get_status(
    connection: &mut zlink::tokio::unix::Connection,
) -> Result<StatusDto, ClientError> {
    rpc(connection.get_status()).await
}

async fn rpc<T>(
    future: impl Future<Output = zlink::Result<Result<T, WireViewError>>>,
) -> Result<T, ClientError> {
    tokio::time::timeout(RPC_TIMEOUT, future)
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(ClientError::Transport)?
        .map_err(ClientError::Remote)
}

fn edited_configuration(
    current: &ConfigurationDto,
    edits: &ConfigEdits,
) -> Result<(ConfigurationDto, bool), ClientError> {
    let original = DeviceSettings::from_json(&current.configuration_json)?;
    let edited = original.with_items(
        edits
            .items
            .iter()
            .map(|edit| (edit.key, edit.value.as_str())),
    )?;
    let changed = edited != original;
    Ok((
        ConfigurationDto {
            configuration_json: serde_json::to_string(&edited)
                .map_err(|error| ClientError::Protocol(error.to_string()))?,
            revision: current.revision.clone(),
        },
        changed,
    ))
}

fn validate_theme_asset(slot: ThemeAssetSlot, asset: &ThemeAssetDto) -> Result<(), ClientError> {
    if asset.slot != slot.name()
        || asset.width != slot.width()
        || asset.height != slot.height()
        || usize::try_from(asset.byte_length).ok() != Some(asset.data.len())
        || asset.data.len() != slot.byte_len()
    {
        return Err(ClientError::Protocol(format!(
            "wireviewd returned inconsistent metadata for {slot}"
        )));
    }
    if sha256_hex(&asset.data) != asset.sha256 {
        return Err(ClientError::Protocol(format!(
            "wireviewd returned a bad SHA-256 digest for {slot}"
        )));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClientError {
    #[error("wireviewd transport failed: {0}")]
    Transport(zlink::Error),
    #[error("wireviewd did not respond within five seconds")]
    Timeout,
    #[error("wireviewd is not connected")]
    Disconnected,
    #[error("{0}")]
    Remote(WireViewError),
    #[error("{0}")]
    Compatibility(#[from] wireview_ipc::CompatibilityError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid daemon response: {0}")]
    Protocol(String),
    #[error("history read cancelled")]
    Cancelled,
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Device(#[from] wireview_core::domain::DeviceError),
}

impl ClientError {
    fn disconnects_client(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Timeout | Self::Disconnected
        )
    }
}

pub(crate) fn demo_events(kind: DemoKind) -> Vec<UiEvent> {
    if kind == DemoKind::Offline {
        return vec![UiEvent::Offline(
            "wireviewd socket is not available in this demo state".into(),
        )];
    }
    let faulted = kind == DemoKind::Fault;
    let stale = kind == DemoKind::Stale;
    let status = StatusDto {
        api_version: API_VERSION,
        api_compatibility_id: api_compatibility_id().into(),
        api_capabilities: API_CAPABILITIES.iter().map(ToString::to_string).collect(),
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        daemon_build_id: "demo".into(),
        state: "ready".into(),
        sequence: 84,
        session_id: 84,
        connected_port: "/dev/ttyACM0".into(),
        last_disconnect_reason: String::new(),
        busy_operation: String::new(),
        recovery_cause: String::new(),
        candidates: Vec::new(),
        poll_interval_ms: 500,
        display_paused: false,
        display_pause_debug_active: false,
        display_pause_history_active: false,
        display_pause_remaining_ms: 0,
    };
    let base_currents = if faulted {
        [8.42, 8.55, 13.60, 7.40, 7.31, 7.32]
    } else {
        [8.42, 8.55, 9.60, 7.40, 7.31, 7.32]
    };
    let base_voltages = [12.03, 12.02, 11.98, 12.01, 12.04, 12.03];
    let settings = DeviceSettings::from_configuration(&DeviceConfiguration::mock());
    let now = unix_time_ms().saturating_sub(if stale { 12_000 } else { 180 });
    let mut events = Vec::with_capacity(65);
    events.push(UiEvent::SessionChanged);
    events.push(UiEvent::Status(status));
    for sample_index in 0_u64..=60 {
        let triangle = (sample_index % 12) as f64;
        let variation = if triangle <= 6.0 {
            triangle / 300.0
        } else {
            (12.0 - triangle) / 300.0
        };
        let currents: [f64; 6] = std::array::from_fn(|index| {
            base_currents[index] * (1.0 + variation)
                + ((sample_index + index as u64 * 3) % 5) as f64 * 0.015
        });
        let voltages: [f64; 6] = std::array::from_fn(|index| {
            base_voltages[index] + ((sample_index + index as u64) % 4) as f64 * 0.004
        });
        let pins = std::array::from_fn(|index| PinSample {
            current: currents[index],
            voltage: voltages[index],
            power: currents[index] * voltages[index],
        });
        events.push(UiEvent::Telemetry(TelemetrySnapshot {
            sequence: 1_000 + sample_index,
            session_id: 84,
            observed_at_ms: now.saturating_sub((60 - sample_index) * 1_000),
            stale,
            controller_vdd: 3.31,
            average_voltage: voltages.into_iter().sum::<f64>() / 6.0,
            total_current: currents.into_iter().sum(),
            total_power: pins.iter().map(|pin| pin.power).sum(),
            fan_duty: if faulted { 92.0 } else { 78.0 },
            cable_capability: 600,
            pins,
            input_temperature: if faulted { 92.4 } else { 48.2 } + variation * 8.0,
            output_temperature: 47.8 + variation * 5.0,
            external_1_temperature: None,
            external_2_temperature: Some(41.2 + variation * 3.0),
            active_fault_mask: if faulted { 0x0028 } else { 0 },
            logged_fault_mask: if faulted { 0x0020 } else { 0 },
            unknown_active_fault_mask: 0,
            unknown_logged_fault_mask: 0,
        }));
    }
    events.push(UiEvent::Configuration {
        settings,
        poll_interval_ms: 500,
    });
    events.push(UiEvent::DeviceInfo(DeviceInfoDto {
        unique_id: "DEMO-84-0A32".into(),
        vendor_id: 0xEF,
        product_id: 0x05,
        firmware_version: "1.0.7".into(),
        hardware_revision: "2".into(),
        config_version: 3,
        product_name: "WireView Pro II".into(),
        build_string: "desktop demo fixture".into(),
        capabilities: vec!["telemetry".into(), "configuration-v3".into()],
    }));
    events
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telemetry() -> TelemetryDto {
        TelemetryDto {
            sequence: 1,
            session_id: 1,
            observed_at_ms: 1,
            stale: false,
            vdd_v: 3.3,
            avg_voltage_v: 12.0,
            total_current_a: 6.0,
            total_power_w: 72.0,
            fan_duty_percent: 40.0,
            cable_capability_w: 600,
            pin_voltages_v: vec![12.0; 6],
            pin_currents_a: vec![1.0; 6],
            pin_power_w: vec![12.0; 6],
            input_temp_c: 40.0,
            output_temp_c: 38.0,
            external_1_present: false,
            external_1_temp_c: 0.0,
            external_2_present: false,
            external_2_temp_c: 0.0,
            active_fault_mask: 0,
            logged_fault_mask: 0,
            unknown_active_fault_mask: 0,
            unknown_logged_fault_mask: 0,
            active_faults: Vec::new(),
            logged_faults: Vec::new(),
        }
    }

    #[test]
    fn telemetry_boundary_requires_six_finite_conductors() {
        assert!(TelemetrySnapshot::try_from(telemetry()).is_ok());

        let mut short = telemetry();
        short.pin_currents_a.pop();
        assert!(matches!(
            TelemetrySnapshot::try_from(short),
            Err(ClientError::Protocol(_))
        ));

        let mut non_finite = telemetry();
        non_finite.pin_power_w[2] = f64::NAN;
        assert!(matches!(
            TelemetrySnapshot::try_from(non_finite),
            Err(ClientError::Protocol(_))
        ));

        let mut bad_controller_supply = telemetry();
        bad_controller_supply.vdd_v = f64::INFINITY;
        assert!(matches!(
            TelemetrySnapshot::try_from(bad_controller_supply),
            Err(ClientError::Protocol(_))
        ));

        let mut bad_external_probe = telemetry();
        bad_external_probe.external_1_present = true;
        bad_external_probe.external_1_temp_c = f64::NAN;
        assert!(matches!(
            TelemetrySnapshot::try_from(bad_external_probe),
            Err(ClientError::Protocol(_))
        ));

        let mut inconsistent_faults = telemetry();
        inconsistent_faults.active_fault_mask = 0x8000;
        assert!(matches!(
            TelemetrySnapshot::try_from(inconsistent_faults),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn gui_refresh_is_capped_at_two_hertz_and_respects_slower_device_polling() {
        assert_eq!(
            effective_refresh_interval(100, true),
            Duration::from_millis(500)
        );
        assert_eq!(
            effective_refresh_interval(500, true),
            Duration::from_millis(500)
        );
        assert_eq!(
            effective_refresh_interval(2_000, true),
            Duration::from_secs(2)
        );
        assert_eq!(
            effective_refresh_interval(9_000, true),
            Duration::from_secs(5)
        );
        assert_eq!(
            effective_refresh_interval(2_000, false),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn configuration_edits_reuse_domain_validation_and_revision() {
        let settings = DeviceSettings::from_configuration(&DeviceConfiguration::mock());
        let current = ConfigurationDto {
            configuration_json: serde_json::to_string(&settings).unwrap(),
            revision: "session-revision".into(),
        };
        let edits = ConfigEdits::new([("fan.mode", "fixed".into())], 500).unwrap();
        let (edited, changed) = edited_configuration(&current, &edits).unwrap();
        assert!(changed);
        assert_eq!(edited.revision, current.revision);
        assert_eq!(
            DeviceSettings::from_json(&edited.configuration_json)
                .unwrap()
                .fan
                .mode,
            wireview_core::config::FanMode::Fixed
        );
    }

    #[test]
    fn configuration_result_names_runtime_only_poll_changes() {
        assert_eq!(
            ConfigurationChange {
                settings_changed: false,
                poll_changed: true,
            }
            .message(true),
            "Daemon polling updated for this run; no device settings changed"
        );
        assert_eq!(
            ConfigurationChange {
                settings_changed: true,
                poll_changed: true,
            }
            .message(true),
            "Device configuration stored; daemon polling updated for this run"
        );
    }

    #[test]
    fn theme_boundary_rejects_inconsistent_slot_metadata_and_digest() {
        let slot = ThemeAssetSlot::FanDark1;
        let data = vec![0; slot.byte_len()];
        let valid = ThemeAssetDto {
            slot: slot.name().into(),
            width: slot.width(),
            height: slot.height(),
            byte_length: u32::try_from(data.len()).unwrap(),
            sha256: sha256_hex(&data),
            data,
        };
        assert!(validate_theme_asset(slot, &valid).is_ok());

        let mut wrong_slot = valid.clone();
        wrong_slot.slot = ThemeAssetSlot::FanDark2.name().into();
        assert!(matches!(
            validate_theme_asset(slot, &wrong_slot),
            Err(ClientError::Protocol(_))
        ));

        let mut wrong_length = valid.clone();
        wrong_length.byte_length -= 1;
        assert!(matches!(
            validate_theme_asset(slot, &wrong_length),
            Err(ClientError::Protocol(_))
        ));

        let mut wrong_digest = valid;
        wrong_digest.sha256 = "0".repeat(64);
        assert!(matches!(
            validate_theme_asset(slot, &wrong_digest),
            Err(ClientError::Protocol(_))
        ));
    }
}
