#![forbid(unsafe_code)]

use std::fmt::{self, Write as _};
use std::sync::OnceLock;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wireview_core::config::FAULT_MASK;
use wireview_core::domain::{DeviceError, DeviceEvent, DeviceIdentity, Telemetry};

pub const INTERFACE_NAME: &str = "io.github.Gustav0ar.WireView";
pub const DEFAULT_SOCKET_PATH: &str = "/run/wireviewd/io.github.Gustav0ar.WireView";
pub const API_VERSION: u32 = 2;
pub const API_CAPABILITIES: &[&str] = &[
    "configuration-items",
    "device-control",
    "device-info",
    "display-leases",
    "fault-registers",
    "history-dump",
    "telemetry",
    "theme-assets-read",
    "theme-assets-write",
];
// v1.1.1 generated this fingerprint from the same IDL before it was canonicalized.
const COMPATIBLE_SCHEMA_IDS: &[&str] = &["wireview-2-047f86fdb168c045"];
const API_CONTRACT: &str = include_str!("../../../interfaces/io.github.Gustav0ar.WireView.varlink");

/// Returns the deterministic fingerprint shared by the daemon and every typed client.
#[must_use]
pub fn api_compatibility_id() -> &'static str {
    static COMPATIBILITY_ID: OnceLock<String> = OnceLock::new();
    COMPATIBILITY_ID.get_or_init(|| {
        let canonical_contract = API_CONTRACT
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        let contract = format!(
            "api={API_VERSION}\n{canonical_contract}\ncapabilities={}",
            API_CAPABILITIES.join(",")
        );
        let digest = Sha256::digest(contract.as_bytes());
        let mut short_digest = String::with_capacity(16);
        for byte in &digest[..8] {
            write!(&mut short_digest, "{byte:02x}").expect("writing to a String cannot fail");
        }
        format!("wireview-{API_VERSION}-{short_digest}")
    })
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
            unknown_active_fault_mask: value.active_fault_mask & !FAULT_MASK,
            unknown_logged_fault_mask: value.logged_fault_mask & !FAULT_MASK,
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
pub struct ThemeAssetDto {
    pub slot: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: u32,
    pub sha256: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize, zlink::introspect::CustomType)]
pub struct ThemeAssetWriteDto {
    pub slot: String,
    pub byte_length: u32,
    pub sha256: String,
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
    #[must_use]
    pub fn snapshot(status: &StatusDto) -> Self {
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

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CompatibilityError {
    #[error("wireviewd API {reported} is not supported; this client requires API {required}")]
    ApiVersion { reported: u32, required: u32 },
    #[error("wireviewd API schema {reported:?} is incompatible; this client requires {required:?}")]
    ApiSchema { reported: String, required: String },
    #[error("wireviewd is missing required capabilities: {0}")]
    MissingCapabilities(String),
}

pub fn validate_status(status: &StatusDto) -> Result<(), CompatibilityError> {
    if status.api_version != API_VERSION {
        return Err(CompatibilityError::ApiVersion {
            reported: status.api_version,
            required: API_VERSION,
        });
    }
    let required_schema = api_compatibility_id();
    if status.api_compatibility_id != required_schema
        && !COMPATIBLE_SCHEMA_IDS.contains(&status.api_compatibility_id.as_str())
    {
        let reported = if status.api_compatibility_id.is_empty() {
            "not reported".into()
        } else {
            status.api_compatibility_id.clone()
        };
        return Err(CompatibilityError::ApiSchema {
            reported,
            required: required_schema.into(),
        });
    }
    let missing = API_CAPABILITIES
        .iter()
        .filter(|required| {
            !status
                .api_capabilities
                .iter()
                .any(|available| available == **required)
        })
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(CompatibilityError::MissingCapabilities(missing.join(", ")))
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

    async fn read_theme_asset(
        &mut self,
        slot: &str,
    ) -> zlink::Result<Result<ThemeAssetDto, WireViewError>>;

    async fn write_theme_asset(
        &mut self,
        slot: &str,
        byte_length: u32,
        sha256: &str,
        data: Vec<u8>,
        confirm: bool,
    ) -> zlink::Result<Result<ThemeAssetWriteDto, WireViewError>>;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> StatusDto {
        StatusDto {
            api_version: API_VERSION,
            api_compatibility_id: api_compatibility_id().into(),
            api_capabilities: API_CAPABILITIES.iter().map(ToString::to_string).collect(),
            daemon_version: "1.1.1".into(),
            daemon_build_id: "test".into(),
            state: "ready".into(),
            sequence: 1,
            session_id: 1,
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
        }
    }

    #[test]
    fn compatibility_rejects_version_schema_and_capability_drift() {
        assert_eq!(validate_status(&status()), Ok(()));

        let mut wrong_version = status();
        wrong_version.api_version += 1;
        assert!(matches!(
            validate_status(&wrong_version),
            Err(CompatibilityError::ApiVersion { .. })
        ));

        let mut wrong_schema = status();
        wrong_schema.api_compatibility_id = "wireview-2-incompatible".into();
        assert!(matches!(
            validate_status(&wrong_schema),
            Err(CompatibilityError::ApiSchema { .. })
        ));

        let mut missing = status();
        missing
            .api_capabilities
            .retain(|value| value != "telemetry");
        assert_eq!(
            validate_status(&missing),
            Err(CompatibilityError::MissingCapabilities("telemetry".into()))
        );
    }

    #[test]
    fn compatibility_accepts_the_released_v2_schema_fingerprint() {
        let mut released = status();
        released.api_compatibility_id = "wireview-2-047f86fdb168c045".into();

        assert_eq!(validate_status(&released), Ok(()));

        released
            .api_capabilities
            .retain(|value| value != "telemetry");
        assert_eq!(
            validate_status(&released),
            Err(CompatibilityError::MissingCapabilities("telemetry".into()))
        );
    }

    #[test]
    fn compatibility_id_has_a_stable_machine_readable_shape() {
        let compatibility_id = api_compatibility_id();
        assert_eq!(compatibility_id, api_compatibility_id());
        let digest = compatibility_id
            .strip_prefix("wireview-2-")
            .expect("API compatibility ID should contain the API version");
        assert_eq!(digest.len(), 16);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, digest.to_ascii_lowercase());
    }
}
