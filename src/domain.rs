use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectReason {
    Startup,
    RemovedFromHost,
    SerialHangup,
    PresenceCheckFailed,
    Replaced,
    DeviceReboot,
    Shutdown,
}

impl fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self)
                .expect("serializable")
                .as_str()
                .unwrap_or("unknown")
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    Absent {
        reason: DisconnectReason,
    },
    Discovering,
    Connecting {
        port: String,
    },
    Ready {
        session_id: u64,
    },
    Busy {
        session_id: u64,
        operation: String,
    },
    Disconnecting {
        session_id: u64,
        cause: DisconnectReason,
    },
    Recovering {
        attempt: u32,
        cause: String,
    },
    Unsupported {
        reason: String,
    },
    AmbiguousDevice {
        candidates: Vec<String>,
    },
}

impl ConnectionState {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Absent { .. } => "absent",
            Self::Discovering => "discovering",
            Self::Connecting { .. } => "connecting",
            Self::Ready { .. } => "ready",
            Self::Busy { .. } => "busy",
            Self::Disconnecting { .. } => "disconnecting",
            Self::Recovering { .. } => "recovering",
            Self::Unsupported { .. } => "unsupported",
            Self::AmbiguousDevice { .. } => "ambiguous_device",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceIdentity {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Main,
    Current,
    Temp,
    Status,
    Simple,
    Same,
}

impl std::str::FromStr for Screen {
    type Err = DeviceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "main" => Ok(Self::Main),
            "current" => Ok(Self::Current),
            "temp" | "temperature" => Ok(Self::Temp),
            "status" => Ok(Self::Status),
            "simple" => Ok(Self::Simple),
            "same" => Ok(Self::Same),
            _ => Err(DeviceError::InvalidArgument(format!(
                "unknown screen {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinMetrics {
    pub voltage_v: f64,
    pub current_a: f64,
    pub power_w: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Temperatures {
    pub input_c: f64,
    pub output_c: f64,
    pub external_1_c: Option<f64>,
    pub external_2_c: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub vdd_v: f64,
    pub avg_voltage_v: f64,
    pub total_current_a: f64,
    pub total_power_w: f64,
    pub fan_duty_percent: f64,
    pub cable_capability_w: u16,
    pub pins: Vec<PinMetrics>,
    pub temperatures: Temperatures,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Telemetry {
    pub sequence: u64,
    pub session_id: u64,
    pub observed_at_ms: u64,
    pub stale: bool,
    pub metrics: Metrics,
    pub active_fault_mask: u16,
    pub logged_fault_mask: u16,
    pub active_faults: Vec<String>,
    pub logged_faults: Vec<String>,
}

impl Telemetry {
    #[must_use]
    pub fn mock() -> Self {
        let pins = (0..6)
            .map(|index| {
                let current_a = 0.9 + f64::from(index) / 100.0;
                PinMetrics {
                    voltage_v: 12.08,
                    current_a,
                    power_w: 12.08 * current_a,
                }
            })
            .collect();
        Self {
            sequence: 0,
            session_id: 0,
            observed_at_ms: unix_time_ms(),
            stale: false,
            metrics: Metrics {
                vdd_v: 3.3,
                avg_voltage_v: 12.08,
                total_current_a: 5.55,
                total_power_w: 67.04,
                fan_duty_percent: 40.0,
                cable_capability_w: 600,
                pins,
                temperatures: Temperatures {
                    input_c: 42.8,
                    output_c: 38.0,
                    external_1_c: None,
                    external_2_c: None,
                },
            },
            active_fault_mask: 0,
            logged_fault_mask: 0,
            active_faults: Vec::new(),
            logged_faults: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonState {
    pub sequence: u64,
    pub session_id: u64,
    pub connection: ConnectionState,
    pub connected_port: Option<String>,
    pub last_disconnect_reason: Option<DisconnectReason>,
    pub identity: Option<DeviceIdentity>,
    pub telemetry: Option<Telemetry>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            sequence: 0,
            session_id: 0,
            connection: ConnectionState::Absent {
                reason: DisconnectReason::Startup,
            },
            connected_port: None,
            last_disconnect_reason: Some(DisconnectReason::Startup),
            identity: None,
            telemetry: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DeviceEvent {
    Connected {
        sequence: u64,
        session_id: u64,
        port: String,
    },
    Disconnected {
        sequence: u64,
        session_id: u64,
        reason: DisconnectReason,
    },
    TelemetryUpdated {
        sequence: u64,
        session_id: u64,
    },
    ScreenChanged {
        sequence: u64,
        session_id: u64,
        screen: Screen,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DeviceError {
    #[error("device is not connected")]
    NotConnected,
    #[error("device connection was lost")]
    ConnectionLost,
    #[error("device connection was lost before mutation started")]
    ConnectionLostBeforeMutation,
    #[error("operation outcome is unknown because the device disconnected")]
    OperationOutcomeUnknown,
    #[error("operation was cancelled")]
    OperationCancelled,
    #[error("more than one matching device is present")]
    AmbiguousDevice,
    #[error("unsupported device or firmware: {0}")]
    Unsupported(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("device is busy: {0}")]
    Busy(String),
    #[error("protocol is not verified: {0}")]
    ProtocolUnverified(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("device mutation failed and the previous state was restored: {0}")]
    FailedAndRolledBack(String),
    #[error("device mutation failed ({operation}); rollback also failed ({rollback})")]
    RollbackFailed { operation: String, rollback: String },
    #[error("configuration revision conflict: {0}")]
    RevisionConflict(String),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("manager task stopped")]
    ManagerStopped,
}

#[must_use]
pub fn unix_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
