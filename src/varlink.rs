use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::OnceLock;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zlink::connection::Socket;
use zlink::tokio::notified::{self, traits::State as _};

use crate::config::{DeviceConfiguration, DeviceSettings};
use crate::domain::{ConnectionState, DeviceError, DeviceEvent, DeviceIdentity, Screen, Telemetry};
use crate::manager::{ManagerHandle, configuration_revision};
use crate::{build_info, build_info::API_VERSION};

pub const INTERFACE_NAME: &str = "io.github.Gustav0ar.WireView";
pub const DEFAULT_SOCKET_PATH: &str = "/run/wireviewd/io.github.Gustav0ar.WireView";

#[derive(Clone)]
pub struct DeviceService {
    manager: ManagerHandle,
    events: notified::State<DeviceEventDto, DeviceEventDto>,
}

impl DeviceService {
    #[must_use]
    pub fn new(manager: ManagerHandle) -> Self {
        let status: StatusDto = manager.state().into();
        let events = notified::State::new(DeviceEventDto::snapshot(&status));
        let mut event_publisher = events.clone();
        let mut manager_events = manager.subscribe_events();
        tokio::spawn(async move {
            loop {
                match manager_events.recv().await {
                    Ok(event) => event_publisher.set(event.into()).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Varlink event publisher lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Self { manager, events }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct StatusDto {
    pub api_version: u32,
    #[serde(default)]
    pub api_compatibility_id: String,
    #[serde(default)]
    pub api_capabilities: Vec<String>,
    #[serde(default)]
    pub daemon_version: String,
    #[serde(default)]
    pub daemon_build_id: String,
    pub state: String,
    pub sequence: u64,
    pub session_id: u64,
    pub connected_port: String,
    pub last_disconnect_reason: String,
    pub busy_operation: String,
    pub recovery_cause: String,
    pub candidates: Vec<String>,
    pub poll_interval_ms: u64,
    pub display_paused: bool,
    pub display_pause_debug_active: bool,
    pub display_pause_history_active: bool,
    pub display_pause_remaining_ms: u64,
}

impl From<crate::domain::DaemonState> for StatusDto {
    fn from(value: crate::domain::DaemonState) -> Self {
        let busy_operation = match &value.connection {
            ConnectionState::Busy { operation, .. } => operation.clone(),
            _ => String::new(),
        };
        let recovery_cause = match &value.connection {
            ConnectionState::Recovering { cause, .. }
            | ConnectionState::Unsupported { reason: cause } => cause.clone(),
            _ => String::new(),
        };
        let candidates = match &value.connection {
            ConnectionState::AmbiguousDevice { candidates } => candidates.clone(),
            _ => Vec::new(),
        };
        Self {
            api_version: API_VERSION,
            api_compatibility_id: api_compatibility_id().into(),
            api_capabilities: build_info::API_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            daemon_version: build_info::VERSION.into(),
            daemon_build_id: build_info::BUILD_ID.into(),
            state: value.connection.name().into(),
            sequence: value.sequence,
            session_id: value.session_id,
            connected_port: value.connected_port.unwrap_or_default(),
            last_disconnect_reason: value
                .last_disconnect_reason
                .map(|reason| reason.to_string())
                .unwrap_or_default(),
            busy_operation,
            recovery_cause,
            candidates,
            poll_interval_ms: 0,
            display_paused: false,
            display_pause_debug_active: false,
            display_pause_history_active: false,
            display_pause_remaining_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct TelemetryDto {
    pub sequence: u64,
    pub session_id: u64,
    pub observed_at_ms: u64,
    pub stale: bool,
    pub vdd_v: f64,
    pub avg_voltage_v: f64,
    pub total_current_a: f64,
    pub total_power_w: f64,
    pub fan_duty_percent: f64,
    pub cable_capability_w: u16,
    pub pin_voltages_v: Vec<f64>,
    pub pin_currents_a: Vec<f64>,
    pub pin_power_w: Vec<f64>,
    pub input_temp_c: f64,
    pub output_temp_c: f64,
    pub external_1_present: bool,
    pub external_1_temp_c: f64,
    pub external_2_present: bool,
    pub external_2_temp_c: f64,
    pub active_fault_mask: u16,
    pub logged_fault_mask: u16,
    pub unknown_active_fault_mask: u16,
    pub unknown_logged_fault_mask: u16,
    pub active_faults: Vec<String>,
    pub logged_faults: Vec<String>,
}

impl From<Telemetry> for TelemetryDto {
    fn from(value: Telemetry) -> Self {
        let temperatures = &value.metrics.temperatures;
        Self {
            sequence: value.sequence,
            session_id: value.session_id,
            observed_at_ms: value.observed_at_ms,
            stale: value.stale,
            vdd_v: value.metrics.vdd_v,
            avg_voltage_v: value.metrics.avg_voltage_v,
            total_current_a: value.metrics.total_current_a,
            total_power_w: value.metrics.total_power_w,
            fan_duty_percent: value.metrics.fan_duty_percent,
            cable_capability_w: value.metrics.cable_capability_w,
            pin_voltages_v: value.metrics.pins.iter().map(|pin| pin.voltage_v).collect(),
            pin_currents_a: value.metrics.pins.iter().map(|pin| pin.current_a).collect(),
            pin_power_w: value.metrics.pins.iter().map(|pin| pin.power_w).collect(),
            input_temp_c: temperatures.input_c,
            output_temp_c: temperatures.output_c,
            external_1_present: temperatures.external_1_c.is_some(),
            external_1_temp_c: temperatures.external_1_c.unwrap_or_default(),
            external_2_present: temperatures.external_2_c.is_some(),
            external_2_temp_c: temperatures.external_2_c.unwrap_or_default(),
            active_fault_mask: value.active_fault_mask,
            logged_fault_mask: value.logged_fault_mask,
            unknown_active_fault_mask: value.active_fault_mask & !crate::protocol::KNOWN_FAULT_MASK,
            unknown_logged_fault_mask: value.logged_fault_mask & !crate::protocol::KNOWN_FAULT_MASK,
            active_faults: value.active_faults,
            logged_faults: value.logged_faults,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct ScreenDto {
    pub active: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct RebootDeviceDto {
    pub accepted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct HistoryChunkDto {
    pub offset: u32,
    pub total_bytes: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct HistoryDumpDto {
    pub dump_id: u64,
    pub session_id: u64,
    pub total_bytes: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct ConfigurationDto {
    pub configuration_json: String,
    pub revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct ConfigurationItemDto {
    pub key: String,
    pub value_json: String,
}

impl ConfigurationDto {
    fn from_configuration(
        configuration: &DeviceConfiguration,
        session_id: u64,
    ) -> Result<Self, WireViewError> {
        let settings = DeviceSettings::from_configuration(configuration);
        Ok(Self {
            configuration_json: serde_json::to_string(&settings).map_err(|error| {
                WireViewError::DeviceError {
                    message: format!("failed to serialize configuration: {error}"),
                }
            })?,
            revision: configuration_revision(session_id, configuration)?,
        })
    }

    fn into_settings(self) -> Result<DeviceSettings, WireViewError> {
        DeviceSettings::from_json(&self.configuration_json).map_err(Into::into)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct DeviceInfoDto {
    pub unique_id: String,
    pub vendor_id: u8,
    pub product_id: u8,
    pub firmware_version: String,
    pub hardware_revision: String,
    pub config_version: u32,
    pub product_name: String,
    pub build_string: String,
    pub capabilities: Vec<String>,
}

impl From<DeviceIdentity> for DeviceInfoDto {
    fn from(value: DeviceIdentity) -> Self {
        Self {
            unique_id: value.unique_id,
            vendor_id: value.vendor_id,
            product_id: value.product_id,
            firmware_version: value.firmware_version,
            hardware_revision: value.hardware_revision,
            config_version: value.config_version,
            product_name: value.product_name,
            build_string: value.build_string,
            capabilities: value.capabilities,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct PollIntervalDto {
    pub milliseconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct DisplayPauseDto {
    pub paused: bool,
    pub debug_lease_active: bool,
    pub history_dump_active: bool,
    pub remaining_ms: u64,
}

impl From<crate::manager::DisplayPauseState> for DisplayPauseDto {
    fn from(value: crate::manager::DisplayPauseState) -> Self {
        Self {
            paused: value.paused,
            debug_lease_active: value.debug_lease_active,
            history_dump_active: value.history_dump_active,
            remaining_ms: value.remaining_ms,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct DeviceEventDto {
    pub event: String,
    pub sequence: u64,
    pub session_id: u64,
    pub port: Option<String>,
    pub reason: Option<String>,
    pub screen: Option<String>,
}

impl DeviceEventDto {
    fn snapshot(status: &StatusDto) -> Self {
        Self {
            event: "snapshot".into(),
            sequence: status.sequence,
            session_id: status.session_id,
            port: (!status.connected_port.is_empty()).then(|| status.connected_port.clone()),
            reason: (!status.last_disconnect_reason.is_empty())
                .then(|| status.last_disconnect_reason.clone()),
            screen: None,
        }
    }
}

impl From<DeviceEvent> for DeviceEventDto {
    fn from(value: DeviceEvent) -> Self {
        match value {
            DeviceEvent::Connected {
                sequence,
                session_id,
                port,
            } => Self {
                event: "connected".into(),
                sequence,
                session_id,
                port: Some(port),
                reason: None,
                screen: None,
            },
            DeviceEvent::Disconnected {
                sequence,
                session_id,
                reason,
            } => Self {
                event: "disconnected".into(),
                sequence,
                session_id,
                port: None,
                reason: Some(reason.to_string()),
                screen: None,
            },
            DeviceEvent::TelemetryUpdated {
                sequence,
                session_id,
            } => Self {
                event: "telemetry_updated".into(),
                sequence,
                session_id,
                port: None,
                reason: None,
                screen: None,
            },
            DeviceEvent::ScreenChanged {
                sequence,
                session_id,
                screen,
            } => Self {
                event: "screen_changed".into(),
                sequence,
                session_id,
                port: None,
                reason: None,
                screen: Some(format!("{screen:?}")),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.github.Gustav0ar.WireView")]
pub enum WireViewError {
    Unavailable { message: String },
    InvalidArgument { message: String },
    RevisionConflict { message: String },
    Busy { message: String },
    OperationOutcomeUnknown { message: String },
    VerificationFailed { message: String },
    FailedAndRolledBack { message: String },
    RollbackFailed { message: String },
    Unsupported { message: String },
    DeviceError { message: String },
}

impl fmt::Display for WireViewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, message) = match self {
            Self::Unavailable { message } => ("unavailable", message),
            Self::InvalidArgument { message } => ("invalid argument", message),
            Self::RevisionConflict { message } => ("revision conflict", message),
            Self::Busy { message } => ("busy", message),
            Self::OperationOutcomeUnknown { message } => ("operation outcome unknown", message),
            Self::VerificationFailed { message } => ("verification failed", message),
            Self::FailedAndRolledBack { message } => ("failed and rolled back", message),
            Self::RollbackFailed { message } => ("rollback failed", message),
            Self::Unsupported { message } => ("unsupported", message),
            Self::DeviceError { message } => ("device error", message),
        };
        write!(formatter, "{name}: {message}")
    }
}

impl std::error::Error for WireViewError {}

impl From<DeviceError> for WireViewError {
    fn from(value: DeviceError) -> Self {
        match value {
            DeviceError::InvalidArgument(message) => Self::InvalidArgument { message },
            DeviceError::RevisionConflict(message) => Self::RevisionConflict { message },
            DeviceError::Busy(message) => Self::Busy { message },
            DeviceError::OperationOutcomeUnknown => Self::OperationOutcomeUnknown {
                message: DeviceError::OperationOutcomeUnknown.to_string(),
            },
            DeviceError::VerificationFailed(message) => Self::VerificationFailed { message },
            DeviceError::FailedAndRolledBack(message) => Self::FailedAndRolledBack { message },
            DeviceError::RollbackFailed {
                operation,
                rollback,
            } => Self::RollbackFailed {
                message: format!("operation failed ({operation}); rollback failed ({rollback})"),
            },
            DeviceError::Unsupported(message) | DeviceError::ProtocolUnverified(message) => {
                Self::Unsupported { message }
            }
            other => Self::DeviceError {
                message: other.to_string(),
            },
        }
    }
}

#[zlink::service(
    interface = "io.github.Gustav0ar.WireView",
    vendor = "wireviewd contributors",
    product = "wireviewd",
    version = env!("CARGO_PKG_VERSION"),
    url = "https://github.com/Gustav0ar/wireview-pro-ii",
    types = [StatusDto, TelemetryDto, ScreenDto, RebootDeviceDto, HistoryChunkDto, HistoryDumpDto, ConfigurationDto, ConfigurationItemDto, DeviceInfoDto, PollIntervalDto, DisplayPauseDto, DeviceEventDto]
)]
impl<Sock> DeviceService
where
    Sock: Socket,
{
    async fn get_status(&self) -> StatusDto {
        let mut status: StatusDto = self.manager.state().into();
        status.poll_interval_ms = self.manager.poll_interval_ms().await.unwrap_or_default();
        if let Ok(pause) = self.manager.display_pause_state().await {
            status.display_paused = pause.paused;
            status.display_pause_debug_active = pause.debug_lease_active;
            status.display_pause_history_active = pause.history_dump_active;
            status.display_pause_remaining_ms = pause.remaining_ms;
        }
        status
    }

    async fn get_telemetry(&self) -> Result<TelemetryDto, WireViewError> {
        self.manager
            .state()
            .telemetry
            .map(Into::into)
            .ok_or_else(|| WireViewError::Unavailable {
                message: "telemetry is not available".into(),
            })
    }

    async fn set_screen(&self, screen: String) -> Result<ScreenDto, WireViewError> {
        let screen = Screen::from_str(&screen)?;
        self.manager.set_screen(screen).await?;
        Ok(ScreenDto {
            active: format!("{screen:?}"),
        })
    }

    async fn read_history_chunk(
        &self,
        offset: u32,
        length: u32,
    ) -> Result<HistoryChunkDto, WireViewError> {
        let offset = usize::try_from(offset).map_err(|_| WireViewError::InvalidArgument {
            message: "history offset is too large".into(),
        })?;
        let length = usize::try_from(length).map_err(|_| WireViewError::InvalidArgument {
            message: "history length is too large".into(),
        })?;
        let data = self.manager.read_history_chunk(offset, length).await?;
        Ok(HistoryChunkDto {
            offset: u32::try_from(offset).expect("Varlink offset originated as u32"),
            total_bytes: u32::try_from(crate::history::FLASH_LENGTH)
                .expect("history length fits u32"),
            data,
        })
    }

    async fn begin_history_dump(&self) -> Result<HistoryDumpDto, WireViewError> {
        let dump = self.manager.begin_history_dump().await?;
        Ok(HistoryDumpDto {
            dump_id: dump.id,
            session_id: dump.session_id,
            total_bytes: u32::try_from(dump.total_bytes).map_err(|_| {
                WireViewError::DeviceError {
                    message: "history length does not fit the Varlink contract".into(),
                }
            })?,
        })
    }

    async fn read_history_dump_chunk(
        &self,
        dump_id: u64,
        offset: u32,
        length: u32,
    ) -> Result<HistoryChunkDto, WireViewError> {
        let offset = usize::try_from(offset).map_err(|_| WireViewError::InvalidArgument {
            message: "history offset is too large".into(),
        })?;
        let length = usize::try_from(length).map_err(|_| WireViewError::InvalidArgument {
            message: "history length is too large".into(),
        })?;
        let data = self
            .manager
            .read_history_dump_chunk(dump_id, offset, length)
            .await?;
        Ok(HistoryChunkDto {
            offset: u32::try_from(offset).expect("offset originated as u32"),
            total_bytes: u32::try_from(crate::history::FLASH_LENGTH)
                .expect("history length fits u32"),
            data,
        })
    }

    async fn end_history_dump(&self, dump_id: u64) -> Result<(), WireViewError> {
        self.manager.end_history_dump(dump_id).await?;
        Ok(())
    }

    async fn get_device_info(&self) -> Result<DeviceInfoDto, WireViewError> {
        Ok(self.manager.read_device_info().await?.into())
    }

    async fn clear_faults(
        &self,
        active_mask: u16,
        logged_mask: u16,
        confirm: bool,
    ) -> Result<TelemetryDto, WireViewError> {
        require_confirmation(confirm, "clearing device faults")?;
        Ok(self
            .manager
            .clear_faults(active_mask, logged_mask)
            .await?
            .into())
    }

    async fn get_poll_interval(&self) -> Result<PollIntervalDto, WireViewError> {
        Ok(PollIntervalDto {
            milliseconds: self.manager.poll_interval_ms().await?,
        })
    }

    async fn set_poll_interval(&self, milliseconds: u64) -> Result<PollIntervalDto, WireViewError> {
        Ok(PollIntervalDto {
            milliseconds: self.manager.set_poll_interval_ms(milliseconds).await?,
        })
    }

    async fn pause_display(&self, milliseconds: u64) -> Result<DisplayPauseDto, WireViewError> {
        Ok(self.manager.pause_display(milliseconds).await?.into())
    }

    async fn resume_display(&self) -> Result<DisplayPauseDto, WireViewError> {
        Ok(self.manager.resume_display().await?.into())
    }

    async fn get_configuration(&self) -> Result<ConfigurationDto, WireViewError> {
        let configuration = self.manager.read_configuration().await?;
        ConfigurationDto::from_configuration(&configuration, self.manager.state().session_id)
    }

    async fn get_configuration_item(
        &self,
        key: String,
    ) -> Result<ConfigurationItemDto, WireViewError> {
        let value_json = self.manager.read_configuration_item(key.clone()).await?;
        Ok(ConfigurationItemDto { key, value_json })
    }

    async fn apply_configuration(
        &self,
        configuration: ConfigurationDto,
    ) -> Result<ConfigurationDto, WireViewError> {
        let current = self.manager.read_configuration().await?;
        let current_revision = configuration_revision(self.manager.state().session_id, &current)?;
        if configuration.revision != current_revision {
            return Err(WireViewError::RevisionConflict {
                message: "configuration changed; run `wireview config show --json` again".into(),
            });
        }
        let settings = configuration.into_settings()?;
        let configuration = settings.with_protocol_metadata(&current)?;
        let applied = self
            .manager
            .apply_configuration_if_revision(configuration, false, current_revision)
            .await?;
        ConfigurationDto::from_configuration(&applied, self.manager.state().session_id)
    }

    async fn store_configuration(
        &self,
        configuration: ConfigurationDto,
        confirm: bool,
    ) -> Result<ConfigurationDto, WireViewError> {
        require_confirmation(confirm, "storing device configuration")?;
        let current = self.manager.read_configuration().await?;
        let current_revision = configuration_revision(self.manager.state().session_id, &current)?;
        if configuration.revision != current_revision {
            return Err(WireViewError::RevisionConflict {
                message: "configuration changed; run `wireview config show --json` again".into(),
            });
        }
        let settings = configuration.into_settings()?;
        let configuration = settings.with_protocol_metadata(&current)?;
        let stored = self
            .manager
            .apply_configuration_if_revision(configuration, true, current_revision)
            .await?;
        ConfigurationDto::from_configuration(&stored, self.manager.state().session_id)
    }

    async fn set_configuration_item(
        &self,
        key: String,
        value: String,
        persist: bool,
        confirm: bool,
    ) -> Result<ConfigurationItemDto, WireViewError> {
        if persist {
            require_confirmation(confirm, "storing a device configuration item")?;
        }
        let configuration = self
            .manager
            .set_configuration_item(key.clone(), value, persist)
            .await?;
        let value = DeviceSettings::from_configuration(&configuration).item(&key)?;
        let value_json =
            serde_json::to_string(&value).map_err(|error| WireViewError::DeviceError {
                message: format!("failed to serialize configuration item: {error}"),
            })?;
        Ok(ConfigurationItemDto { key, value_json })
    }

    async fn reload_configuration(&self) -> Result<ConfigurationDto, WireViewError> {
        let configuration = self.manager.reload_configuration().await?;
        ConfigurationDto::from_configuration(&configuration, self.manager.state().session_id)
    }

    async fn reset_configuration(&self, confirm: bool) -> Result<ConfigurationDto, WireViewError> {
        require_confirmation(confirm, "resetting device configuration")?;
        let configuration = self.manager.reset_configuration().await?;
        ConfigurationDto::from_configuration(&configuration, self.manager.state().session_id)
    }

    async fn reboot_device(&self, confirm: bool) -> Result<RebootDeviceDto, WireViewError> {
        require_confirmation(confirm, "rebooting the device")?;
        self.manager.reboot_device().await?;
        Ok(RebootDeviceDto { accepted: true })
    }

    #[zlink(more)]
    async fn monitor(&self, more: bool) -> notified::Stream<DeviceEventDto> {
        if more {
            self.events.stream()
        } else {
            self.events.stream_once()
        }
    }
}

/// A deterministic fingerprint of the generated Varlink API 1 interface and its
/// semantic capability set. It changes automatically when either contract does.
#[must_use]
pub fn api_compatibility_id() -> &'static str {
    static COMPATIBILITY_ID: OnceLock<String> = OnceLock::new();
    COMPATIBILITY_ID.get_or_init(|| {
        let contract = format!(
            "api={}\n{}\ncapabilities={}",
            API_VERSION,
            __DEVICESERVICE_INTERFACE_IO_GITHUB_GUSTAV0AR_WIREVIEW,
            build_info::API_CAPABILITIES.join(",")
        );
        let digest = Sha256::digest(contract.as_bytes());
        let mut short_digest = String::with_capacity(16);
        for byte in &digest[..8] {
            write!(&mut short_digest, "{byte:02x}").expect("writing to a String cannot fail");
        }
        format!("wireview-{API_VERSION}-{short_digest}")
    })
}

#[cfg(test)]
mod compatibility_tests {
    use super::{__DEVICESERVICE_INTERFACE_IO_GITHUB_GUSTAV0AR_WIREVIEW, api_compatibility_id};

    fn contract_without_whitespace(contract: &str) -> String {
        contract
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

    #[test]
    fn generated_compatibility_id_has_a_stable_machine_readable_shape() {
        let compatibility_id = api_compatibility_id();
        assert_eq!(compatibility_id, api_compatibility_id());
        let digest = compatibility_id
            .strip_prefix("wireview-1-")
            .expect("API compatibility ID should contain the API version");
        assert_eq!(digest.len(), 16);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, digest.to_ascii_lowercase());
    }

    #[test]
    fn checked_in_idl_matches_the_zlink_generated_contract() {
        let checked_in = include_str!("../interfaces/io.github.Gustav0ar.WireView.varlink");
        let generated = __DEVICESERVICE_INTERFACE_IO_GITHUB_GUSTAV0AR_WIREVIEW.to_string();
        assert_eq!(
            contract_without_whitespace(checked_in),
            contract_without_whitespace(&generated),
            "update the checked-in Varlink IDL together with the zlink service contract"
        );
    }
}

fn require_confirmation(confirm: bool, operation: &str) -> Result<(), WireViewError> {
    if confirm {
        Ok(())
    } else {
        Err(WireViewError::InvalidArgument {
            message: format!("{operation} requires explicit confirmation"),
        })
    }
}

#[zlink::proxy("io.github.Gustav0ar.WireView")]
pub trait WireViewProxy {
    async fn get_status(&mut self) -> zlink::Result<Result<StatusDto, WireViewError>>;

    async fn get_telemetry(&mut self) -> zlink::Result<Result<TelemetryDto, WireViewError>>;

    async fn set_screen(&mut self, screen: &str)
    -> zlink::Result<Result<ScreenDto, WireViewError>>;

    async fn read_history_chunk(
        &mut self,
        offset: u32,
        length: u32,
    ) -> zlink::Result<Result<HistoryChunkDto, WireViewError>>;

    async fn begin_history_dump(&mut self) -> zlink::Result<Result<HistoryDumpDto, WireViewError>>;

    async fn read_history_dump_chunk(
        &mut self,
        dump_id: u64,
        offset: u32,
        length: u32,
    ) -> zlink::Result<Result<HistoryChunkDto, WireViewError>>;

    async fn end_history_dump(&mut self, dump_id: u64) -> zlink::Result<Result<(), WireViewError>>;

    async fn get_device_info(&mut self) -> zlink::Result<Result<DeviceInfoDto, WireViewError>>;

    async fn clear_faults(
        &mut self,
        active_mask: u16,
        logged_mask: u16,
        confirm: bool,
    ) -> zlink::Result<Result<TelemetryDto, WireViewError>>;

    async fn get_poll_interval(&mut self) -> zlink::Result<Result<PollIntervalDto, WireViewError>>;

    async fn set_poll_interval(
        &mut self,
        milliseconds: u64,
    ) -> zlink::Result<Result<PollIntervalDto, WireViewError>>;

    async fn pause_display(
        &mut self,
        milliseconds: u64,
    ) -> zlink::Result<Result<DisplayPauseDto, WireViewError>>;

    async fn resume_display(&mut self) -> zlink::Result<Result<DisplayPauseDto, WireViewError>>;

    async fn get_configuration(&mut self)
    -> zlink::Result<Result<ConfigurationDto, WireViewError>>;

    async fn get_configuration_item(
        &mut self,
        key: &str,
    ) -> zlink::Result<Result<ConfigurationItemDto, WireViewError>>;

    async fn apply_configuration(
        &mut self,
        configuration: ConfigurationDto,
    ) -> zlink::Result<Result<ConfigurationDto, WireViewError>>;

    async fn store_configuration(
        &mut self,
        configuration: ConfigurationDto,
        confirm: bool,
    ) -> zlink::Result<Result<ConfigurationDto, WireViewError>>;

    async fn set_configuration_item(
        &mut self,
        key: &str,
        value: &str,
        persist: bool,
        confirm: bool,
    ) -> zlink::Result<Result<ConfigurationItemDto, WireViewError>>;

    async fn reload_configuration(
        &mut self,
    ) -> zlink::Result<Result<ConfigurationDto, WireViewError>>;

    async fn reset_configuration(
        &mut self,
        confirm: bool,
    ) -> zlink::Result<Result<ConfigurationDto, WireViewError>>;

    async fn reboot_device(
        &mut self,
        confirm: bool,
    ) -> zlink::Result<Result<RebootDeviceDto, WireViewError>>;

    #[zlink(more)]
    async fn monitor(
        &mut self,
    ) -> zlink::Result<impl Stream<Item = zlink::Result<Result<DeviceEventDto, WireViewError>>>>;
}
