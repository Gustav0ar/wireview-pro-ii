use std::str::FromStr;

use zlink::connection::Socket;
use zlink::tokio::notified::{self, traits::State as _};

use crate::config::{DeviceConfiguration, DeviceSettings};
use crate::domain::{ConnectionState, DaemonState, Screen};
use crate::manager::{DisplayPauseState, ManagerHandle, configuration_revision};
use crate::theme::{ThemeAssetSlot, sha256_hex};
use crate::{build_info, build_info::API_VERSION};

pub use wireview_ipc::{
    ConfigurationDto, ConfigurationItemDto, DEFAULT_SOCKET_PATH, DeviceEventDto, DeviceInfoDto,
    DisplayPauseDto, HistoryChunkDto, HistoryDumpDto, INTERFACE_NAME, PollIntervalDto,
    RebootDeviceDto, ScreenDto, StatusDto, TelemetryDto, ThemeAssetDto, ThemeAssetWriteDto,
    WireViewError, WireViewProxy, api_compatibility_id,
};

#[derive(Clone)]
pub struct DeviceService {
    manager: ManagerHandle,
    events: notified::State<DeviceEventDto, DeviceEventDto>,
}

impl DeviceService {
    #[must_use]
    pub fn new(manager: ManagerHandle) -> Self {
        let status = status_from_state(manager.state());
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

fn status_from_state(value: DaemonState) -> StatusDto {
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
    StatusDto {
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

fn configuration_dto(
    configuration: &DeviceConfiguration,
    session_id: u64,
) -> Result<ConfigurationDto, WireViewError> {
    let settings = DeviceSettings::from_configuration(configuration);
    Ok(ConfigurationDto {
        configuration_json: serde_json::to_string(&settings).map_err(|error| {
            WireViewError::DeviceError {
                message: format!("failed to serialize configuration: {error}"),
            }
        })?,
        revision: configuration_revision(session_id, configuration)?,
    })
}

fn configuration_settings(dto: ConfigurationDto) -> Result<DeviceSettings, WireViewError> {
    DeviceSettings::from_json(&dto.configuration_json).map_err(Into::into)
}

fn display_pause_dto(value: DisplayPauseState) -> DisplayPauseDto {
    DisplayPauseDto {
        paused: value.paused,
        debug_lease_active: value.debug_lease_active,
        history_dump_active: value.history_dump_active,
        remaining_ms: value.remaining_ms,
    }
}

#[zlink::service(
    interface = "io.github.Gustav0ar.WireView",
    vendor = "wireviewd contributors",
    product = "wireviewd",
    version = env!("CARGO_PKG_VERSION"),
    url = "https://github.com/Gustav0ar/wireview-pro-ii",
    types = [StatusDto, TelemetryDto, ScreenDto, RebootDeviceDto, HistoryChunkDto, HistoryDumpDto, ThemeAssetDto, ThemeAssetWriteDto, ConfigurationDto, ConfigurationItemDto, DeviceInfoDto, PollIntervalDto, DisplayPauseDto, DeviceEventDto]
)]
impl<Sock> DeviceService
where
    Sock: Socket,
{
    async fn get_status(&self) -> StatusDto {
        let mut status = status_from_state(self.manager.state());
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

    async fn read_theme_asset(&self, slot: String) -> Result<ThemeAssetDto, WireViewError> {
        let slot: ThemeAssetSlot = slot.parse()?;
        let data = self.manager.read_theme_asset(slot).await?;
        if data.len() != slot.byte_len() {
            return Err(WireViewError::DeviceError {
                message: format!(
                    "device returned {} bytes for {slot}, expected {}",
                    data.len(),
                    slot.byte_len()
                ),
            });
        }
        Ok(ThemeAssetDto {
            slot: slot.to_string(),
            width: slot.width(),
            height: slot.height(),
            byte_length: u32::try_from(data.len()).expect("theme asset length fits u32"),
            sha256: sha256_hex(&data),
            data,
        })
    }

    async fn write_theme_asset(
        &self,
        slot: String,
        byte_length: u32,
        sha256: String,
        data: Vec<u8>,
        confirm: bool,
    ) -> Result<ThemeAssetWriteDto, WireViewError> {
        require_confirmation(confirm, "writing a theme asset")?;
        let slot: ThemeAssetSlot = slot.parse()?;
        let supplied_length =
            usize::try_from(byte_length).map_err(|_| WireViewError::InvalidArgument {
                message: "theme asset byte length is too large".into(),
            })?;
        if supplied_length != data.len() || data.len() != slot.byte_len() {
            return Err(WireViewError::InvalidArgument {
                message: format!(
                    "theme asset {slot} must be exactly {} bytes",
                    slot.byte_len()
                ),
            });
        }
        let actual_sha256 = sha256_hex(&data);
        if sha256 != actual_sha256 {
            return Err(WireViewError::InvalidArgument {
                message: "theme asset SHA-256 does not match its data".into(),
            });
        }
        self.manager.write_theme_asset(slot, data).await?;
        Ok(ThemeAssetWriteDto {
            slot: slot.to_string(),
            byte_length,
            sha256: actual_sha256,
        })
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
        Ok(display_pause_dto(
            self.manager.pause_display(milliseconds).await?,
        ))
    }

    async fn resume_display(&self) -> Result<DisplayPauseDto, WireViewError> {
        Ok(display_pause_dto(self.manager.resume_display().await?))
    }

    async fn get_configuration(&self) -> Result<ConfigurationDto, WireViewError> {
        let configuration = self.manager.read_configuration().await?;
        configuration_dto(&configuration, self.manager.state().session_id)
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
        let settings = configuration_settings(configuration)?;
        let configuration = settings.with_protocol_metadata(&current)?;
        let applied = self
            .manager
            .apply_configuration_if_revision(configuration, false, current_revision)
            .await?;
        configuration_dto(&applied, self.manager.state().session_id)
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
        let settings = configuration_settings(configuration)?;
        let configuration = settings.with_protocol_metadata(&current)?;
        let stored = self
            .manager
            .apply_configuration_if_revision(configuration, true, current_revision)
            .await?;
        configuration_dto(&stored, self.manager.state().session_id)
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
        configuration_dto(&configuration, self.manager.state().session_id)
    }

    async fn reset_configuration(&self, confirm: bool) -> Result<ConfigurationDto, WireViewError> {
        require_confirmation(confirm, "resetting device configuration")?;
        let configuration = self.manager.reset_configuration().await?;
        configuration_dto(&configuration, self.manager.state().session_id)
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

fn require_confirmation(confirm: bool, operation: &str) -> Result<(), WireViewError> {
    if confirm {
        Ok(())
    } else {
        Err(WireViewError::InvalidArgument {
            message: format!("{operation} requires explicit confirmation"),
        })
    }
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
            .strip_prefix("wireview-2-")
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
