use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortBuilderExt, SerialStream,
    StopBits,
};

use crate::config::{
    CONFIG_WRITE_PAYLOAD, DeviceConfiguration, NvmOperation, configuration_size,
    decode_configuration, encode_configuration,
};
use crate::domain::{DeviceError, DeviceIdentity, Screen, Telemetry, unix_time_ms};
use crate::history::{
    ENTRY_SIZE, FLASH_LENGTH, FLASH_READ_PAGE_SIZE, FLASH_START_ADDRESS, MAX_CHUNK_SIZE,
};
use crate::protocol::{
    BUILD_RESPONSE_SIZE, CONFIG_VERSION_RESPONSE_SIZE, ScreenCommand, UID_RESPONSE_SIZE,
    UsbCommand, VENDOR_RESPONSE_SIZE, WELCOME_MESSAGE, WELCOME_RESPONSE_SIZE, clear_faults_command,
    decode_build_response, decode_faults, decode_sensor_response,
};

#[async_trait]
pub trait DeviceBackend: Send + 'static {
    async fn connect(&mut self, port: &str) -> Result<DeviceIdentity, DeviceError>;
    async fn disconnect(&mut self);
    async fn read_build_information(&mut self) -> Result<(String, String), DeviceError>;
    async fn read_telemetry(&mut self) -> Result<Telemetry, DeviceError>;
    async fn read_configuration(&mut self) -> Result<DeviceConfiguration, DeviceError>;
    async fn write_configuration(
        &mut self,
        configuration: &DeviceConfiguration,
    ) -> Result<(), DeviceError>;
    async fn nvm_configuration(&mut self, operation: NvmOperation) -> Result<(), DeviceError>;
    async fn reboot_device(&mut self) -> Result<(), DeviceError>;
    async fn clear_faults(&mut self, active_mask: u16, logged_mask: u16)
    -> Result<(), DeviceError>;
    async fn pause_history_updates(&mut self) -> Result<(), DeviceError>;
    async fn resume_history_updates(&mut self) -> Result<(), DeviceError>;
    async fn read_history_chunk(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError>;
    async fn set_screen(&mut self, screen: Screen) -> Result<(), DeviceError>;
}

#[derive(Default)]
struct MockState {
    connected: bool,
    connection_attempts: u64,
    fail_next_connect: bool,
    reads: u64,
    fail_next_read: bool,
    screens: Vec<Screen>,
    configuration: Option<DeviceConfiguration>,
    saved_configuration: Option<DeviceConfiguration>,
    nvm_operations: Vec<NvmOperation>,
    device_reboots: u64,
    change_crc_on_store: bool,
    active_fault_mask: u16,
    logged_fault_mask: u16,
    history_pause_depth: u32,
    fail_configuration_writes: u8,
    fail_nvm_operations: u8,
    transient_telemetry_failures: u8,
}

#[derive(Clone)]
pub struct MockControl(Arc<Mutex<MockState>>);

impl MockControl {
    pub fn fail_next_connect(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fail_next_connect = true;
    }

    #[must_use]
    pub fn connection_attempts(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection_attempts
    }

    pub fn fail_next_read(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fail_next_read = true;
    }

    #[must_use]
    pub fn telemetry_reads(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reads
    }

    #[must_use]
    pub fn screens(&self) -> Vec<Screen> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .screens
            .clone()
    }

    #[must_use]
    pub fn configuration(&self) -> Option<DeviceConfiguration> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .configuration
            .clone()
    }

    pub fn set_saved_configuration(&self, configuration: DeviceConfiguration) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .saved_configuration = Some(configuration);
    }

    pub fn change_crc_on_store(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .change_crc_on_store = true;
    }

    #[must_use]
    pub fn nvm_operations(&self) -> Vec<NvmOperation> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .nvm_operations
            .clone()
    }

    #[must_use]
    pub fn device_reboots(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .device_reboots
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connected
    }

    pub fn set_fault_masks(&self, active: u16, logged: u16) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active_fault_mask = active;
        state.logged_fault_mask = logged;
    }

    pub fn fail_next_configuration_write(&self) {
        self.fail_configuration_writes(1);
    }

    pub fn fail_next_nvm_operation(&self) {
        self.fail_nvm_operations(1);
    }

    pub fn fail_configuration_writes(&self, count: u8) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fail_configuration_writes = count;
    }

    pub fn fail_nvm_operations(&self, count: u8) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fail_nvm_operations = count;
    }

    pub fn fail_transient_telemetry_reads(&self, count: u8) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .transient_telemetry_failures = count;
    }

    #[must_use]
    pub fn history_pause_depth(&self) -> u32 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history_pause_depth
    }
}

pub struct MockBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockBackend {
    #[must_use]
    pub fn new() -> (Self, MockControl) {
        let state = Arc::new(Mutex::new(MockState::default()));
        (
            Self {
                state: Arc::clone(&state),
            },
            MockControl(state),
        )
    }
}

#[async_trait]
impl DeviceBackend for MockBackend {
    async fn connect(&mut self, _port: &str) -> Result<DeviceIdentity, DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.connection_attempts += 1;
        if std::mem::take(&mut state.fail_next_connect) {
            state.connected = false;
            return Err(DeviceError::Transport(
                "synthetic connection failure".into(),
            ));
        }
        state.connected = true;
        let factory_defaults = DeviceConfiguration::mock();
        let saved = state
            .saved_configuration
            .get_or_insert(factory_defaults)
            .clone();
        // A new controller session models a power cycle: temporary writes
        // disappear and the firmware activates its non-volatile copy.
        state.configuration = Some(saved);
        Ok(DeviceIdentity {
            unique_id: "MOCK-WIREVIEW-0001".into(),
            vendor_id: 0xef,
            product_id: 0x05,
            firmware_version: "mock-v3".into(),
            hardware_revision: "mock-2.0".into(),
            config_version: 3,
            product_name: "WireView Pro II".into(),
            build_string: "mock-build".into(),
            capabilities: vec![
                "telemetry".into(),
                "history".into(),
                "screen".into(),
                "device-info".into(),
                "fault-clear".into(),
                "config-v3".into(),
            ],
        })
    }

    async fn disconnect(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connected = false;
    }

    async fn read_build_information(&mut self) -> Result<(String, String), DeviceError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        Ok(("WireView Pro II".into(), "mock-build".into()))
    }

    async fn read_telemetry(&mut self) -> Result<Telemetry, DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        if std::mem::take(&mut state.fail_next_read) {
            state.connected = false;
            return Err(DeviceError::ConnectionLost);
        }
        if state.transient_telemetry_failures > 0 {
            state.transient_telemetry_failures -= 1;
            return Err(DeviceError::Transport(
                "synthetic transient telemetry failure".into(),
            ));
        }
        state.reads += 1;
        let mut telemetry = Telemetry::mock();
        telemetry.observed_at_ms = unix_time_ms();
        let bounded_reads =
            u32::try_from(state.reads.min(u64::from(u32::MAX))).expect("value was bounded to u32");
        telemetry.metrics.total_power_w += f64::from(bounded_reads) / 10.0;
        telemetry.active_fault_mask = state.active_fault_mask;
        telemetry.logged_fault_mask = state.logged_fault_mask;
        telemetry.active_faults = decode_faults(state.active_fault_mask);
        telemetry.logged_faults = decode_faults(state.logged_fault_mask);
        Ok(telemetry)
    }

    async fn read_history_chunk(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.connected {
                return Err(DeviceError::ConnectionLost);
            }
        }
        validate_history_range(offset, length)?;
        // Keep synthetic dumps long enough for process-level cancellation
        // tests to interrupt an active request deterministically.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let mut bytes = vec![0xff; length];
        let mut sample = [0_u8; ENTRY_SIZE];
        sample[..4].copy_from_slice(&(42_u32 << 2).to_le_bytes());
        sample[4..8].copy_from_slice(&[39, 34, (-100_i8) as u8, 25]);
        sample[8..14].copy_from_slice(&[121, 120, 119, 118, 117, 116]);
        sample[14..20].copy_from_slice(&[3, 4, 5, 6, 7, 8]);
        sample[20] = 0;
        let sample_start = offset;
        let sample_end = (offset + length).min(ENTRY_SIZE);
        if sample_start < sample_end {
            bytes[sample_start - offset..sample_end - offset]
                .copy_from_slice(&sample[sample_start..sample_end]);
        }
        Ok(bytes)
    }

    async fn read_configuration(&mut self) -> Result<DeviceConfiguration, DeviceError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state
            .configuration
            .clone()
            .ok_or_else(|| DeviceError::Transport("mock configuration is unavailable".into()))
    }

    async fn write_configuration(
        &mut self,
        configuration: &DeviceConfiguration,
    ) -> Result<(), DeviceError> {
        configuration.validate()?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        if state.fail_configuration_writes > 0 {
            state.fail_configuration_writes -= 1;
            return Err(DeviceError::Transport(
                "synthetic configuration write failure".into(),
            ));
        }
        state.configuration = Some(configuration.clone());
        Ok(())
    }

    async fn nvm_configuration(&mut self, operation: NvmOperation) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        if state.fail_nvm_operations > 0 {
            state.fail_nvm_operations -= 1;
            return Err(DeviceError::Transport(
                "synthetic NVM operation failure".into(),
            ));
        }
        match operation {
            NvmOperation::Reload => state.configuration = state.saved_configuration.clone(),
            NvmOperation::Store => {
                if state.change_crc_on_store
                    && let Some(configuration) = &mut state.configuration
                {
                    configuration.crc = configuration.crc.wrapping_add(1);
                }
                state.saved_configuration = state.configuration.clone();
            }
            NvmOperation::Reset => {
                let configuration = DeviceConfiguration::mock();
                state.configuration = Some(configuration.clone());
                state.saved_configuration = Some(configuration);
            }
        }
        state.nvm_operations.push(operation);
        Ok(())
    }

    async fn reboot_device(&mut self) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state.device_reboots += 1;
        Ok(())
    }

    async fn clear_faults(
        &mut self,
        active_mask: u16,
        logged_mask: u16,
    ) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state.active_fault_mask &= !active_mask;
        state.logged_fault_mask &= !logged_mask;
        Ok(())
    }

    async fn pause_history_updates(&mut self) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state.history_pause_depth = state.history_pause_depth.saturating_add(1);
        Ok(())
    }

    async fn resume_history_updates(&mut self) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state.history_pause_depth = state.history_pause_depth.saturating_sub(1);
        Ok(())
    }

    async fn set_screen(&mut self, screen: Screen) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        state.screens.push(screen);
        Ok(())
    }
}

pub struct SerialBackend {
    port: Option<SerialStream>,
    config_version: Option<u8>,
}

impl Default for SerialBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialBackend {
    #[must_use]
    pub fn new() -> Self {
        Self {
            port: None,
            config_version: None,
        }
    }

    async fn response(&mut self, command: &[u8], size: usize) -> Result<Vec<u8>, DeviceError> {
        let port = self.port.as_mut().ok_or(DeviceError::NotConnected)?;
        port.clear(ClearBuffer::Input).map_err(serial_error)?;
        port.write_all(command).await.map_err(io_error)?;
        port.flush().await.map_err(io_error)?;
        let mut response = vec![0_u8; size];
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            port.read_exact(&mut response),
        )
        .await
        .map_err(|_| DeviceError::Transport("serial response timed out".into()))?
        .map_err(io_error)?;
        Ok(response)
    }

    async fn send(&mut self, command: &[u8]) -> Result<(), DeviceError> {
        let port = self.port.as_mut().ok_or(DeviceError::NotConnected)?;
        port.clear(ClearBuffer::Input).map_err(serial_error)?;
        port.write_all(command).await.map_err(io_error)?;
        port.flush().await.map_err(io_error)
    }
}

#[async_trait]
impl DeviceBackend for SerialBackend {
    async fn connect(&mut self, port: &str) -> Result<DeviceIdentity, DeviceError> {
        let mut serial = tokio_serial::new(port, 115_200)
            .data_bits(DataBits::Eight)
            .stop_bits(StopBits::One)
            .parity(Parity::None)
            .flow_control(FlowControl::None)
            .open_native_async()
            .map_err(serial_error)?;
        serial.set_exclusive(true).map_err(serial_error)?;
        serial.clear(ClearBuffer::Input).map_err(serial_error)?;
        serial.write_request_to_send(true).map_err(serial_error)?;
        let mut welcome = [0_u8; WELCOME_RESPONSE_SIZE];
        let welcome_result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            serial.read_exact(&mut welcome),
        )
        .await;
        serial.write_request_to_send(false).map_err(serial_error)?;
        let greeting_timed_out = match welcome_result {
            Ok(Ok(_)) => {
                if welcome[..WELCOME_MESSAGE.len()] != WELCOME_MESSAGE[..]
                    || welcome[WELCOME_MESSAGE.len()] != 0
                {
                    return Err(DeviceError::Unsupported(
                        "serial device returned an unexpected greeting".into(),
                    ));
                }
                false
            }
            Ok(Err(error)) => return Err(io_error(error)),
            Err(_) => true,
        };
        self.port = Some(serial);
        let vendor = self
            .response(&[UsbCommand::ReadVendorData as u8], VENDOR_RESPONSE_SIZE)
            .await?;
        if vendor[0] != 0xef || vendor[1] != 0x05 {
            self.port = None;
            return Err(DeviceError::Unsupported(format!(
                "unexpected device identity {:02X}{:02X}",
                vendor[0], vendor[1]
            )));
        }
        if greeting_timed_out {
            tracing::info!("device greeting timed out; accepted the verified EF05 vendor response");
        }
        let mut config_version = None;
        for _ in 0..3 {
            let config = self
                .response(
                    &[UsbCommand::ReadConfig as u8],
                    CONFIG_VERSION_RESPONSE_SIZE,
                )
                .await?;
            if config[2] <= 2 {
                config_version = Some(config[2]);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let config_version = config_version.ok_or_else(|| {
            DeviceError::Unsupported("device returned an unknown configuration version".into())
        })?;
        // The firmware sends the complete configuration even when the version
        // probe reads only its four-byte prefix. The daemon keeps the tty open,
        // so wait for and discard the unread tail before issuing another
        // command.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.port
            .as_mut()
            .ok_or(DeviceError::NotConnected)?
            .clear(ClearBuffer::Input)
            .map_err(serial_error)?;
        let uid = self
            .response(&[UsbCommand::ReadUid as u8], UID_RESPONSE_SIZE)
            .await?;
        self.send(&[
            UsbCommand::ScreenChange as u8,
            ScreenCommand::ResumeUpdates as u8,
        ])
        .await?;
        self.config_version = Some(config_version);
        let mut capabilities = vec![
            "telemetry".into(),
            "history".into(),
            "screen".into(),
            "device-info".into(),
            "fault-clear".into(),
        ];
        if config_version <= 2 {
            capabilities.push(format!("config-v{}", config_version + 1));
        }
        Ok(DeviceIdentity {
            unique_id: uid.iter().map(|byte| format!("{byte:02X}")).collect(),
            vendor_id: vendor[0],
            product_id: vendor[1],
            firmware_version: vendor[2].to_string(),
            hardware_revision: format!("{:02X}{:02X}", vendor[0], vendor[1]),
            config_version: u32::from(config_version),
            product_name: String::new(),
            build_string: String::new(),
            capabilities,
        })
    }

    async fn disconnect(&mut self) {
        self.port = None;
        self.config_version = None;
    }

    async fn read_build_information(&mut self) -> Result<(String, String), DeviceError> {
        let response = self
            .response(&[UsbCommand::ReadBuildInfo as u8], BUILD_RESPONSE_SIZE)
            .await?;
        decode_build_response(&response)
    }

    async fn read_telemetry(&mut self) -> Result<Telemetry, DeviceError> {
        let response = self
            .response(
                &[UsbCommand::ReadSensorValues as u8],
                crate::protocol::SENSOR_RESPONSE_SIZE,
            )
            .await?;
        let sensors = decode_sensor_response(&response)?;
        Ok(Telemetry {
            sequence: 0,
            session_id: 0,
            observed_at_ms: unix_time_ms(),
            stale: false,
            metrics: sensors.metrics,
            active_fault_mask: sensors.fault_status,
            logged_fault_mask: sensors.fault_log,
            active_faults: decode_faults(sensors.fault_status),
            logged_faults: decode_faults(sensors.fault_log),
        })
    }

    async fn read_history_chunk(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        validate_history_range(offset, length)?;
        if length == 0 {
            return Ok(Vec::new());
        }

        let port = self.port.as_mut().ok_or(DeviceError::NotConnected)?;
        async {
            let mut bytes = vec![0_u8; length];
            let mut read = 0;
            while read < length {
                let page_length = (length - read).min(FLASH_READ_PAGE_SIZE);
                let address = FLASH_START_ADDRESS
                    .checked_add(u32::try_from(offset + read).map_err(|_| {
                        DeviceError::InvalidArgument("history offset is too large".into())
                    })?)
                    .ok_or_else(|| {
                        DeviceError::InvalidArgument("history address overflowed".into())
                    })?;
                let mut command = [0_u8; 9];
                command[0] = UsbCommand::SpiFlashReadPage as u8;
                command[1..5].copy_from_slice(&address.to_le_bytes());
                command[5..9].copy_from_slice(
                    &u32::try_from(page_length)
                        .expect("flash page length fits u32")
                        .to_le_bytes(),
                );
                port.write_all(&command).await.map_err(io_error)?;
                port.flush().await.map_err(io_error)?;
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    port.read_exact(&mut bytes[read..read + page_length]),
                )
                .await
                .map_err(|_| DeviceError::Transport("SPI flash read timed out".into()))?
                .map_err(io_error)?;
                read += page_length;
            }
            Ok(bytes)
        }
        .await
    }

    async fn read_configuration(&mut self) -> Result<DeviceConfiguration, DeviceError> {
        let version = self.config_version.ok_or(DeviceError::NotConnected)?;
        let size = configuration_size(version)?;
        let mut last_error = None;
        for _ in 0..3 {
            let bytes = self.response(&[UsbCommand::ReadConfig as u8], size).await?;
            match decode_configuration(version, &bytes) {
                Ok(configuration) => return Ok(configuration),
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Err(last_error.expect("configuration decode was attempted"))
    }

    async fn write_configuration(
        &mut self,
        configuration: &DeviceConfiguration,
    ) -> Result<(), DeviceError> {
        let version = self.config_version.ok_or(DeviceError::NotConnected)?;
        if configuration.raw_version != version {
            return Err(DeviceError::InvalidArgument(format!(
                "configuration is V{}, but the connected device uses V{}",
                configuration.raw_version + 1,
                version + 1
            )));
        }
        let bytes = encode_configuration(configuration)?;
        let port = self.port.as_mut().ok_or(DeviceError::NotConnected)?;
        port.clear(ClearBuffer::Input).map_err(serial_error)?;
        for (offset, payload) in bytes.chunks(CONFIG_WRITE_PAYLOAD).enumerate() {
            let byte_offset = offset * CONFIG_WRITE_PAYLOAD;
            let mut frame = Vec::with_capacity(payload.len() + 2);
            frame.push(UsbCommand::WriteConfig as u8);
            frame.push(
                u8::try_from(byte_offset).map_err(|_| {
                    DeviceError::Transport("configuration offset overflowed".into())
                })?,
            );
            frame.extend_from_slice(payload);
            port.write_all(&frame).await.map_err(io_error)?;
            port.flush().await.map_err(io_error)?;
        }
        Ok(())
    }

    async fn nvm_configuration(&mut self, operation: NvmOperation) -> Result<(), DeviceError> {
        self.send(&[
            UsbCommand::NvmConfig as u8,
            0x55,
            0xaa,
            0x55,
            0xaa,
            operation as u8,
        ])
        .await
    }

    async fn reboot_device(&mut self) -> Result<(), DeviceError> {
        self.send(&[UsbCommand::Reset as u8]).await
    }

    async fn clear_faults(
        &mut self,
        active_mask: u16,
        logged_mask: u16,
    ) -> Result<(), DeviceError> {
        let command = clear_faults_command(active_mask, logged_mask);
        self.send(&command).await
    }

    async fn pause_history_updates(&mut self) -> Result<(), DeviceError> {
        self.send(&[
            UsbCommand::ScreenChange as u8,
            ScreenCommand::PauseUpdates as u8,
        ])
        .await
    }

    async fn resume_history_updates(&mut self) -> Result<(), DeviceError> {
        self.send(&[
            UsbCommand::ScreenChange as u8,
            ScreenCommand::ResumeUpdates as u8,
        ])
        .await
    }

    async fn set_screen(&mut self, screen: Screen) -> Result<(), DeviceError> {
        let command = match screen {
            Screen::Main => ScreenCommand::Main,
            Screen::Current => ScreenCommand::Current,
            Screen::Temp => ScreenCommand::Temp,
            Screen::Status => ScreenCommand::Status,
            Screen::Simple => ScreenCommand::Simple,
            Screen::Same => ScreenCommand::Same,
        };
        self.send(&[UsbCommand::ScreenChange as u8, command as u8])
            .await
    }
}

fn validate_history_range(offset: usize, length: usize) -> Result<(), DeviceError> {
    if length > MAX_CHUNK_SIZE {
        return Err(DeviceError::InvalidArgument(format!(
            "history chunk is limited to {MAX_CHUNK_SIZE} bytes"
        )));
    }
    if offset > FLASH_LENGTH || length > FLASH_LENGTH.saturating_sub(offset) {
        return Err(DeviceError::InvalidArgument(
            "history range is outside the device log region".into(),
        ));
    }
    Ok(())
}

fn serial_error(error: tokio_serial::Error) -> DeviceError {
    match error.kind() {
        tokio_serial::ErrorKind::Io(std::io::ErrorKind::NotFound)
        | tokio_serial::ErrorKind::Io(std::io::ErrorKind::BrokenPipe)
        | tokio_serial::ErrorKind::Io(std::io::ErrorKind::NotConnected)
        | tokio_serial::ErrorKind::Io(std::io::ErrorKind::UnexpectedEof)
        | tokio_serial::ErrorKind::Io(std::io::ErrorKind::Other) => DeviceError::ConnectionLost,
        _ => DeviceError::Transport(error.to_string()),
    }
}

fn io_error(error: std::io::Error) -> DeviceError {
    // Linux reports unplug/driver-unbind/VM-passthrough loss most commonly as
    // EIO, ENXIO, or ENODEV. `ErrorKind` does not preserve all three as stable
    // named variants, so retain the Linux errno check for this Linux daemon.
    if matches!(error.raw_os_error(), Some(5 | 6 | 19)) {
        return DeviceError::ConnectionLost;
    }
    match error.kind() {
        std::io::ErrorKind::NotFound
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::UnexpectedEof
        | std::io::ErrorKind::NotConnected => DeviceError::ConnectionLost,
        _ => DeviceError::Transport(error.to_string()),
    }
}
