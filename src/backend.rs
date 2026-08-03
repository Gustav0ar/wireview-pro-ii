use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
use crate::theme::ThemeAssetSlot;

const SPI_READ_ATTEMPTS: usize = 3;
const SPI_READ_TIMEOUT: Duration = Duration::from_secs(2);
const SPI_READ_RETRY_DELAY: Duration = Duration::from_millis(10);
const SPI_FLASH_SIZE: u32 = 0x0100_0000;
const SPI_SECTOR_SIZE: u32 = 4096;
const SPI_WRITE_PAGE_SIZE: usize = 256;

/// Cooperative cancellation for read-only SPI work. Cancellation is checked
/// at page/retry boundaries, so cleanup never interrupts a flash mutation or
/// abandons a partial serial page response.
#[derive(Clone)]
pub struct ReadCancellation {
    cancelled: tokio::sync::watch::Sender<bool>,
}

impl Default for ReadCancellation {
    fn default() -> Self {
        let (cancelled, _) = tokio::sync::watch::channel(false);
        Self { cancelled }
    }
}

impl ReadCancellation {
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    async fn cancelled(&self) {
        let mut cancelled = self.cancelled.subscribe();
        while !*cancelled.borrow_and_update() && cancelled.changed().await.is_ok() {}
    }
}

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
        cancellation: &ReadCancellation,
    ) -> Result<Vec<u8>, DeviceError>;
    async fn read_theme_asset(&mut self, slot: ThemeAssetSlot) -> Result<Vec<u8>, DeviceError>;
    async fn write_theme_asset(
        &mut self,
        slot: ThemeAssetSlot,
        data: &[u8],
    ) -> Result<(), DeviceError>;
    async fn set_screen(&mut self, screen: Screen) -> Result<(), DeviceError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockThemeWriteFailure {
    BeforeMutation,
    DisconnectBeforeMutation,
    FailedAndRolledBack,
    RollbackFailed,
    Disconnect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockDisplayResumeFailure {
    Transport,
    Disconnect,
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
    theme_assets: HashMap<ThemeAssetSlot, Vec<u8>>,
    theme_mutations: u64,
    next_theme_write_failure: Option<MockThemeWriteFailure>,
    next_display_resume_failure: Option<MockDisplayResumeFailure>,
    block_next_history_read: bool,
    history_reads_started: u64,
}

#[derive(Clone)]
pub struct MockControl(Arc<Mutex<MockState>>);

impl MockControl {
    pub fn block_next_history_read_until_cancelled(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .block_next_history_read = true;
    }

    #[must_use]
    pub fn history_reads_started(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history_reads_started
    }

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

    pub fn fail_next_theme_write(&self, failure: MockThemeWriteFailure) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_theme_write_failure = Some(failure);
    }

    pub fn fail_next_display_resume(&self, failure: MockDisplayResumeFailure) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_display_resume_failure = Some(failure);
    }

    #[must_use]
    pub fn theme_mutations(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .theme_mutations
    }

    #[must_use]
    pub fn theme_asset(&self, slot: ThemeAssetSlot) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .theme_assets
            .get(&slot)
            .cloned()
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
        let config_version = saved.raw_version;
        // A new controller session models a power cycle: temporary writes
        // disappear and the firmware activates its non-volatile copy.
        state.configuration = Some(saved);
        if state.theme_assets.is_empty() {
            for (slot_index, slot) in ThemeAssetSlot::ALL.into_iter().enumerate() {
                let value = u8::try_from(slot_index + 1).expect("eight slots fit in u8");
                state
                    .theme_assets
                    .insert(slot, vec![value; slot.byte_len()]);
            }
        }
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
        if config_version == 2 {
            capabilities.extend(["theme-assets-read".into(), "theme-assets-write".into()]);
        }
        Ok(DeviceIdentity {
            unique_id: "MOCK-WIREVIEW-0001".into(),
            vendor_id: 0xef,
            product_id: 0x05,
            firmware_version: "mock-v3".into(),
            hardware_revision: "mock-2.0".into(),
            config_version: u32::from(config_version),
            product_name: "WireView Pro II".into(),
            build_string: "mock-build".into(),
            capabilities,
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
        cancellation: &ReadCancellation,
    ) -> Result<Vec<u8>, DeviceError> {
        let block_until_cancelled = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.connected {
                return Err(DeviceError::ConnectionLost);
            }
            state.history_reads_started = state.history_reads_started.saturating_add(1);
            std::mem::take(&mut state.block_next_history_read)
        };
        validate_history_range(offset, length)?;
        if block_until_cancelled {
            cancellation.cancelled().await;
            return Err(DeviceError::OperationCancelled);
        }
        // Keep synthetic dumps long enough for process-level cancellation
        // tests to interrupt an active request deterministically.
        tokio::select! {
            () = cancellation.cancelled() => return Err(DeviceError::OperationCancelled),
            () = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
        }
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

    async fn read_theme_asset(&mut self, slot: ThemeAssetSlot) -> Result<Vec<u8>, DeviceError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        if state
            .configuration
            .as_ref()
            .map(|config| config.raw_version)
            != Some(2)
        {
            return Err(DeviceError::Unsupported(
                "theme assets require configuration V3".into(),
            ));
        }
        state
            .theme_assets
            .get(&slot)
            .cloned()
            .ok_or_else(|| DeviceError::Transport("mock theme asset is unavailable".into()))
    }

    async fn write_theme_asset(
        &mut self,
        slot: ThemeAssetSlot,
        data: &[u8],
    ) -> Result<(), DeviceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.connected {
            return Err(DeviceError::ConnectionLost);
        }
        if state
            .configuration
            .as_ref()
            .map(|config| config.raw_version)
            != Some(2)
        {
            return Err(DeviceError::Unsupported(
                "theme assets require configuration V3".into(),
            ));
        }
        if data.len() != slot.byte_len() {
            return Err(DeviceError::InvalidArgument(format!(
                "theme asset {slot} must be exactly {} bytes",
                slot.byte_len()
            )));
        }

        match state.next_theme_write_failure.take() {
            Some(MockThemeWriteFailure::BeforeMutation) => Err(DeviceError::Transport(
                "synthetic pre-mutation theme failure".into(),
            )),
            Some(MockThemeWriteFailure::DisconnectBeforeMutation) => {
                state.connected = false;
                Err(DeviceError::ConnectionLostBeforeMutation)
            }
            Some(MockThemeWriteFailure::FailedAndRolledBack) => {
                state.theme_mutations += 1;
                Err(DeviceError::FailedAndRolledBack(
                    "synthetic theme write failure".into(),
                ))
            }
            Some(MockThemeWriteFailure::RollbackFailed) => {
                state.theme_mutations += 1;
                state.theme_assets.insert(slot, vec![0; slot.byte_len()]);
                Err(DeviceError::RollbackFailed {
                    operation: "synthetic theme write failure".into(),
                    rollback: "synthetic theme rollback failure".into(),
                })
            }
            Some(MockThemeWriteFailure::Disconnect) => {
                state.theme_mutations += 1;
                state.connected = false;
                Err(DeviceError::ConnectionLost)
            }
            None => {
                state.theme_mutations += 1;
                state.theme_assets.insert(slot, data.to_vec());
                Ok(())
            }
        }
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
        match state.next_display_resume_failure.take() {
            Some(MockDisplayResumeFailure::Transport) => {
                return Err(DeviceError::Transport(
                    "synthetic display resume failure".into(),
                ));
            }
            Some(MockDisplayResumeFailure::Disconnect) => {
                state.connected = false;
                return Err(DeviceError::ConnectionLost);
            }
            None => {}
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
    port: Option<Box<dyn SerialIo>>,
    config_version: Option<u8>,
}

#[async_trait]
trait SerialIo: Send {
    fn clear_input(&mut self) -> Result<(), DeviceError>;
    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), DeviceError>;
    async fn flush(&mut self) -> Result<(), DeviceError>;
    async fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), DeviceError>;
}

#[async_trait]
impl SerialIo for SerialStream {
    fn clear_input(&mut self) -> Result<(), DeviceError> {
        self.clear(ClearBuffer::Input).map_err(serial_error)
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        AsyncWriteExt::write_all(self, bytes)
            .await
            .map_err(io_error)
    }

    async fn flush(&mut self) -> Result<(), DeviceError> {
        AsyncWriteExt::flush(self).await.map_err(io_error)
    }

    async fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), DeviceError> {
        AsyncReadExt::read_exact(self, bytes)
            .await
            .map(|_| ())
            .map_err(io_error)
    }
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
        port.clear_input()?;
        port.write_all(command).await?;
        port.flush().await?;
        let mut response = vec![0_u8; size];
        tokio::time::timeout(Duration::from_secs(1), port.read_exact(&mut response))
            .await
            .map_err(|_| DeviceError::Transport("serial response timed out".into()))??;
        Ok(response)
    }

    async fn send(&mut self, command: &[u8]) -> Result<(), DeviceError> {
        let port = self.port.as_mut().ok_or(DeviceError::NotConnected)?;
        port.clear_input()?;
        port.write_all(command).await?;
        port.flush().await
    }
}

async fn read_spi_page(
    port: &mut dyn SerialIo,
    address: u32,
    destination: &mut [u8],
    cancellation: &ReadCancellation,
) -> Result<(), DeviceError> {
    debug_assert!(!destination.is_empty());
    debug_assert!(destination.len() <= FLASH_READ_PAGE_SIZE);

    let length = u32::try_from(destination.len()).expect("SPI page length fits u32");
    let mut command = [0_u8; 9];
    command[0] = UsbCommand::SpiFlashReadPage as u8;
    command[1..5].copy_from_slice(&address.to_le_bytes());
    command[5..9].copy_from_slice(&length.to_le_bytes());

    for attempt in 1..=SPI_READ_ATTEMPTS {
        if cancellation.is_cancelled() {
            return Err(DeviceError::OperationCancelled);
        }
        port.clear_input()?;
        port.write_all(&command).await?;
        port.flush().await?;
        // Finish or time out the current page before observing cancellation.
        // Dropping read_exact after a partial response could leave late serial
        // bytes racing the display-resume command.
        let read = tokio::time::timeout(SPI_READ_TIMEOUT, port.read_exact(destination)).await;
        match read {
            Ok(result) => return result,
            Err(_) if cancellation.is_cancelled() => {
                return Err(DeviceError::OperationCancelled);
            }
            Err(_) if attempt < SPI_READ_ATTEMPTS => {
                tokio::select! {
                    () = cancellation.cancelled() => {
                        return Err(DeviceError::OperationCancelled);
                    }
                    () = tokio::time::sleep(SPI_READ_RETRY_DELAY) => {}
                }
            }
            Err(_) => {
                return Err(DeviceError::Transport(format!(
                    "SPI flash read at {address:#010x} timed out after {SPI_READ_ATTEMPTS} attempts"
                )));
            }
        }
    }
    unreachable!("SPI read attempt loop always returns")
}

async fn read_spi_bytes(
    port: &mut dyn SerialIo,
    address: u32,
    length: usize,
    cancellation: &ReadCancellation,
) -> Result<Vec<u8>, DeviceError> {
    if cancellation.is_cancelled() {
        return Err(DeviceError::OperationCancelled);
    }
    let mut bytes = vec![0_u8; length];
    let mut read = 0;
    while read < length {
        let page_length = (length - read).min(FLASH_READ_PAGE_SIZE);
        let page_address = address
            .checked_add(u32::try_from(read).map_err(|_| {
                DeviceError::InvalidArgument("SPI flash offset is too large".into())
            })?)
            .ok_or_else(|| DeviceError::InvalidArgument("SPI flash address overflowed".into()))?;
        read_spi_page(
            port,
            page_address,
            &mut bytes[read..read + page_length],
            cancellation,
        )
        .await?;
        read += page_length;
    }
    Ok(bytes)
}

async fn read_spi_status(
    port: &mut dyn SerialIo,
    timeout: Duration,
    operation: &str,
) -> Result<(), DeviceError> {
    let mut status = [0_u8; 1];
    tokio::time::timeout(timeout, port.read_exact(&mut status))
        .await
        .map_err(|_| DeviceError::Transport(format!("{operation} acknowledgement timed out")))??;
    if status[0] != 1 {
        return Err(DeviceError::Transport(format!(
            "{operation} returned status {}",
            status[0]
        )));
    }
    Ok(())
}

async fn erase_spi_sectors(
    port: &mut dyn SerialIo,
    address: u32,
    length: u32,
) -> Result<(), DeviceError> {
    if length == 0
        || !address.is_multiple_of(SPI_SECTOR_SIZE)
        || !length.is_multiple_of(SPI_SECTOR_SIZE)
    {
        return Err(DeviceError::InvalidArgument(
            "SPI erase range must contain complete aligned sectors".into(),
        ));
    }
    let end = address
        .checked_add(length)
        .ok_or_else(|| DeviceError::InvalidArgument("SPI erase range overflowed".into()))?;
    if end > SPI_FLASH_SIZE {
        return Err(DeviceError::InvalidArgument(
            "SPI erase range exceeds flash size".into(),
        ));
    }

    let mut command = [0_u8; 9];
    command[0] = UsbCommand::SpiFlashEraseSector as u8;
    command[1..5].copy_from_slice(&address.to_le_bytes());
    command[5..9].copy_from_slice(&length.to_le_bytes());
    port.clear_input()?;
    port.write_all(&command).await?;
    port.flush().await?;
    let sectors = length / SPI_SECTOR_SIZE;
    read_spi_status(
        port,
        Duration::from_millis(u64::from(sectors) * 100 + 1_000),
        &format!("SPI erase at {address:#010x}"),
    )
    .await
}

async fn write_spi_page(
    port: &mut dyn SerialIo,
    address: u32,
    data: &[u8],
) -> Result<(), DeviceError> {
    if data.is_empty() || data.len() > SPI_WRITE_PAGE_SIZE {
        return Err(DeviceError::InvalidArgument(format!(
            "SPI page write must contain 1..={SPI_WRITE_PAGE_SIZE} bytes"
        )));
    }
    let length = u32::try_from(data.len()).expect("SPI page length fits u32");
    let page_offset = usize::try_from(address & 0xff).expect("page offset fits usize");
    if page_offset + data.len() > SPI_WRITE_PAGE_SIZE {
        return Err(DeviceError::InvalidArgument(
            "SPI page write crosses a page boundary".into(),
        ));
    }
    if address
        .checked_add(length)
        .is_none_or(|end| end > SPI_FLASH_SIZE)
    {
        return Err(DeviceError::InvalidArgument(
            "SPI page write exceeds flash size".into(),
        ));
    }

    let mut command = [0_u8; 9];
    command[0] = UsbCommand::SpiFlashWritePage as u8;
    command[1..5].copy_from_slice(&address.to_le_bytes());
    command[5..9].copy_from_slice(&length.to_le_bytes());
    port.clear_input()?;
    port.write_all(&command).await?;
    port.write_all(data).await?;
    port.flush().await?;
    read_spi_status(
        port,
        SPI_READ_TIMEOUT,
        &format!("SPI page write at {address:#010x}"),
    )
    .await
}

async fn write_spi_pages(
    port: &mut dyn SerialIo,
    address: u32,
    data: &[u8],
) -> Result<(), DeviceError> {
    debug_assert_eq!(address % u32::try_from(SPI_WRITE_PAGE_SIZE).unwrap(), 0);
    debug_assert_eq!(data.len() % SPI_WRITE_PAGE_SIZE, 0);
    for (index, page) in data.chunks_exact(SPI_WRITE_PAGE_SIZE).enumerate() {
        let offset = u32::try_from(index * SPI_WRITE_PAGE_SIZE)
            .map_err(|_| DeviceError::InvalidArgument("SPI write offset is too large".into()))?;
        let page_address = address
            .checked_add(offset)
            .ok_or_else(|| DeviceError::InvalidArgument("SPI write address overflowed".into()))?;
        write_spi_page(port, page_address, page).await?;
    }
    Ok(())
}

async fn restore_spi_range(
    port: &mut dyn SerialIo,
    address: u32,
    backup: &[u8],
) -> Result<(), DeviceError> {
    let length = u32::try_from(backup.len())
        .map_err(|_| DeviceError::InvalidArgument("SPI backup is too large".into()))?;
    erase_spi_sectors(port, address, length).await?;
    write_spi_pages(port, address, backup).await?;
    let restored =
        read_spi_bytes(port, address, backup.len(), &ReadCancellation::default()).await?;
    if restored != backup {
        return Err(DeviceError::VerificationFailed(
            "theme rollback readback did not match the sector backup".into(),
        ));
    }
    Ok(())
}

async fn write_theme_asset_transaction(
    port: &mut dyn SerialIo,
    slot: ThemeAssetSlot,
    data: &[u8],
) -> Result<(), DeviceError> {
    if data.len() != slot.byte_len() {
        return Err(DeviceError::InvalidArgument(format!(
            "theme asset {slot} must be exactly {} bytes",
            slot.byte_len()
        )));
    }

    let asset_start = slot.address();
    let asset_length = u32::try_from(data.len()).expect("theme asset length fits u32");
    let asset_end = asset_start
        .checked_add(asset_length)
        .ok_or_else(|| DeviceError::InvalidArgument("theme asset range overflowed".into()))?;
    let sector_start = asset_start / SPI_SECTOR_SIZE * SPI_SECTOR_SIZE;
    let sector_end = asset_end
        .checked_add(SPI_SECTOR_SIZE - 1)
        .ok_or_else(|| DeviceError::InvalidArgument("theme sector range overflowed".into()))?
        / SPI_SECTOR_SIZE
        * SPI_SECTOR_SIZE;
    let sector_length =
        usize::try_from(sector_end - sector_start).expect("theme sector range fits usize");

    let backup = match read_spi_bytes(
        port,
        sector_start,
        sector_length,
        &ReadCancellation::default(),
    )
    .await
    {
        Err(DeviceError::ConnectionLost) => {
            return Err(DeviceError::ConnectionLostBeforeMutation);
        }
        result => result?,
    };
    let mut requested = backup.clone();
    let asset_offset =
        usize::try_from(asset_start - sector_start).expect("asset sector offset fits usize");
    requested[asset_offset..asset_offset + data.len()].copy_from_slice(data);

    let operation = async {
        erase_spi_sectors(
            port,
            sector_start,
            u32::try_from(sector_length).expect("theme sector length fits u32"),
        )
        .await?;
        write_spi_pages(port, sector_start, &requested).await?;
        let verified = read_spi_bytes(
            port,
            sector_start,
            sector_length,
            &ReadCancellation::default(),
        )
        .await?;
        if verified != requested {
            return Err(DeviceError::VerificationFailed(
                "theme asset sector readback did not match the requested bytes".into(),
            ));
        }
        Ok(())
    }
    .await;

    let Err(operation_error) = operation else {
        return Ok(());
    };
    if operation_error == DeviceError::ConnectionLost {
        return Err(operation_error);
    }

    match restore_spi_range(port, sector_start, &backup).await {
        Ok(()) => Err(DeviceError::FailedAndRolledBack(
            operation_error.to_string(),
        )),
        Err(DeviceError::ConnectionLost) => Err(DeviceError::ConnectionLost),
        Err(rollback_error) => Err(DeviceError::RollbackFailed {
            operation: operation_error.to_string(),
            rollback: rollback_error.to_string(),
        }),
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
            AsyncReadExt::read_exact(&mut serial, &mut welcome),
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
        self.port = Some(Box::new(serial));
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
            .clear_input()?;
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
        if config_version == 2 {
            capabilities.push("theme-assets-read".into());
            capabilities.push("theme-assets-write".into());
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
        cancellation: &ReadCancellation,
    ) -> Result<Vec<u8>, DeviceError> {
        validate_history_range(offset, length)?;
        if length == 0 {
            return Ok(Vec::new());
        }

        let address =
            FLASH_START_ADDRESS
                .checked_add(u32::try_from(offset).map_err(|_| {
                    DeviceError::InvalidArgument("history offset is too large".into())
                })?)
                .ok_or_else(|| DeviceError::InvalidArgument("history address overflowed".into()))?;
        read_spi_bytes(
            self.port
                .as_mut()
                .ok_or(DeviceError::NotConnected)?
                .as_mut(),
            address,
            length,
            cancellation,
        )
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

    async fn read_theme_asset(&mut self, slot: ThemeAssetSlot) -> Result<Vec<u8>, DeviceError> {
        if self.config_version != Some(2) {
            return Err(DeviceError::Unsupported(
                "theme assets require configuration V3".into(),
            ));
        }
        let cancellation = ReadCancellation::default();
        read_spi_bytes(
            self.port
                .as_mut()
                .ok_or(DeviceError::NotConnected)?
                .as_mut(),
            slot.address(),
            slot.byte_len(),
            &cancellation,
        )
        .await
    }

    async fn write_theme_asset(
        &mut self,
        slot: ThemeAssetSlot,
        data: &[u8],
    ) -> Result<(), DeviceError> {
        if self.config_version != Some(2) {
            return Err(DeviceError::Unsupported(
                "theme assets require configuration V3".into(),
            ));
        }
        write_theme_asset_transaction(
            self.port
                .as_mut()
                .ok_or(DeviceError::NotConnected)?
                .as_mut(),
            slot,
            data,
        )
        .await
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
        port.clear_input()?;
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
            port.write_all(&frame).await?;
            port.flush().await?;
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::pending;

    use super::*;

    enum ReadAction {
        Bytes(Vec<u8>),
        Pending,
        Error(DeviceError),
    }

    #[derive(Default)]
    struct FakeLog {
        clears: usize,
        flushes: usize,
        writes: Vec<Vec<u8>>,
    }

    struct FakeSerial {
        actions: VecDeque<ReadAction>,
        log: Arc<Mutex<FakeLog>>,
        write_error: Option<DeviceError>,
        flush_error: Option<DeviceError>,
    }

    impl FakeSerial {
        fn new(actions: impl IntoIterator<Item = ReadAction>) -> (Self, Arc<Mutex<FakeLog>>) {
            let log = Arc::new(Mutex::new(FakeLog::default()));
            (
                Self {
                    actions: actions.into_iter().collect(),
                    log: Arc::clone(&log),
                    write_error: None,
                    flush_error: None,
                },
                log,
            )
        }
    }

    #[async_trait]
    impl SerialIo for FakeSerial {
        fn clear_input(&mut self) -> Result<(), DeviceError> {
            self.log.lock().unwrap().clears += 1;
            Ok(())
        }

        async fn write_all(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
            if let Some(error) = self.write_error.take() {
                return Err(error);
            }
            self.log.lock().unwrap().writes.push(bytes.to_vec());
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), DeviceError> {
            self.log.lock().unwrap().flushes += 1;
            if let Some(error) = self.flush_error.take() {
                return Err(error);
            }
            Ok(())
        }

        async fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), DeviceError> {
            match self.actions.pop_front().expect("scripted read action") {
                ReadAction::Bytes(data) => {
                    if data.len() != bytes.len() {
                        return Err(DeviceError::Transport(format!(
                            "script supplied {} bytes for a {} byte read",
                            data.len(),
                            bytes.len()
                        )));
                    }
                    bytes.copy_from_slice(&data);
                    Ok(())
                }
                ReadAction::Pending => pending().await,
                ReadAction::Error(error) => Err(error),
            }
        }
    }

    fn spi_read_command(address: u32, length: u32) -> Vec<u8> {
        let mut command = vec![UsbCommand::SpiFlashReadPage as u8];
        command.extend_from_slice(&address.to_le_bytes());
        command.extend_from_slice(&length.to_le_bytes());
        command
    }

    #[tokio::test(start_paused = true)]
    async fn retries_the_same_page_after_a_timeout() {
        let (mut serial, log) =
            FakeSerial::new([ReadAction::Pending, ReadAction::Bytes(vec![1, 2, 3, 4])]);
        let mut destination = [0_u8; 4];

        read_spi_page(
            &mut serial,
            0x0080_1234,
            &mut destination,
            &ReadCancellation::default(),
        )
        .await
        .unwrap();

        assert_eq!(destination, [1, 2, 3, 4]);
        let log = log.lock().unwrap();
        assert_eq!(log.clears, 2);
        assert_eq!(log.flushes, 2);
        assert_eq!(log.writes, vec![spi_read_command(0x0080_1234, 4); 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn reports_an_error_after_three_timeouts_without_returning_bytes() {
        let (mut serial, log) = FakeSerial::new([
            ReadAction::Pending,
            ReadAction::Pending,
            ReadAction::Pending,
        ]);

        let error = read_spi_bytes(&mut serial, 0x0080_2000, 4, &ReadCancellation::default())
            .await
            .unwrap_err();

        assert!(matches!(error, DeviceError::Transport(message) if
            message.contains("0x00802000") && message.contains("3 attempts")));
        assert_eq!(log.lock().unwrap().writes.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_connection_loss() {
        let (mut serial, log) = FakeSerial::new([ReadAction::Error(DeviceError::ConnectionLost)]);

        assert_eq!(
            read_spi_bytes(&mut serial, 0x0080_0000, 4, &ReadCancellation::default(),).await,
            Err(DeviceError::ConnectionLost)
        );
        assert_eq!(log.lock().unwrap().writes.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_stops_after_the_current_spi_page_deadline_without_retry() {
        let (mut serial, log) = FakeSerial::new([ReadAction::Pending]);
        let cancellation = ReadCancellation::default();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });

        assert_eq!(
            read_spi_bytes(&mut serial, 0x0080_0000, 4, &cancellation).await,
            Err(DeviceError::OperationCancelled)
        );
        assert_eq!(log.lock().unwrap().writes.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_only_the_failed_page_in_a_bulk_read() {
        let first = vec![0x11; FLASH_READ_PAGE_SIZE];
        let second = vec![0x22; 44];
        let (mut serial, log) = FakeSerial::new([
            ReadAction::Bytes(first.clone()),
            ReadAction::Pending,
            ReadAction::Bytes(second.clone()),
        ]);

        let bytes = read_spi_bytes(&mut serial, 0x1000, 300, &ReadCancellation::default())
            .await
            .unwrap();

        assert_eq!(&bytes[..FLASH_READ_PAGE_SIZE], first);
        assert_eq!(&bytes[FLASH_READ_PAGE_SIZE..], second);
        let log = log.lock().unwrap();
        assert_eq!(
            log.writes,
            vec![
                spi_read_command(0x1000, 256),
                spi_read_command(0x1100, 44),
                spi_read_command(0x1100, 44),
            ]
        );
        assert_eq!(log.clears, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_command_write_or_flush_failures() {
        let (mut write_failure, write_log) = FakeSerial::new([]);
        write_failure.write_error = Some(DeviceError::Transport("write failed".into()));
        assert_eq!(
            read_spi_bytes(&mut write_failure, 0x1000, 1, &ReadCancellation::default(),).await,
            Err(DeviceError::Transport("write failed".into()))
        );
        assert_eq!(write_log.lock().unwrap().clears, 1);

        let (mut flush_failure, flush_log) = FakeSerial::new([]);
        flush_failure.flush_error = Some(DeviceError::Transport("flush failed".into()));
        assert_eq!(
            read_spi_bytes(&mut flush_failure, 0x1000, 1, &ReadCancellation::default(),).await,
            Err(DeviceError::Transport("flush failed".into()))
        );
        assert_eq!(flush_log.lock().unwrap().writes.len(), 1);
        assert_eq!(flush_log.lock().unwrap().flushes, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn erase_and_page_write_acknowledgement_timeouts_are_not_retried() {
        let (mut erase, erase_log) = FakeSerial::new([ReadAction::Pending]);
        assert!(matches!(
            erase_spi_sectors(&mut erase, 0x5000, SPI_SECTOR_SIZE).await,
            Err(DeviceError::Transport(message)) if message.contains("acknowledgement timed out")
        ));
        {
            let erase_log = erase_log.lock().unwrap();
            assert_eq!(erase_log.clears, 1);
            assert_eq!(erase_log.flushes, 1);
            assert_eq!(erase_log.writes.len(), 1);
            assert_eq!(
                erase_log.writes[0][0],
                UsbCommand::SpiFlashEraseSector as u8
            );
        }

        let (mut write, write_log) = FakeSerial::new([ReadAction::Pending]);
        let page = [0x5a; SPI_WRITE_PAGE_SIZE];
        assert!(matches!(
            write_spi_page(&mut write, 0x5000, &page).await,
            Err(DeviceError::Transport(message)) if message.contains("acknowledgement timed out")
        ));
        let write_log = write_log.lock().unwrap();
        assert_eq!(write_log.clears, 1);
        assert_eq!(write_log.flushes, 1);
        assert_eq!(write_log.writes.len(), 2);
        assert_eq!(write_log.writes[0][0], UsbCommand::SpiFlashWritePage as u8);
        assert_eq!(write_log.writes[1], page);
    }

    #[tokio::test]
    async fn disconnect_during_theme_backup_is_classified_before_mutation() {
        let (mut serial, log) = FakeSerial::new([ReadAction::Error(DeviceError::ConnectionLost)]);
        let slot = ThemeAssetSlot::FanDark1;

        assert_eq!(
            write_theme_asset_transaction(&mut serial, slot, &vec![0x41; slot.byte_len()]).await,
            Err(DeviceError::ConnectionLostBeforeMutation)
        );
        let log = log.lock().unwrap();
        assert_eq!(log.writes.len(), 1);
        assert_eq!(log.writes[0][0], UsbCommand::SpiFlashReadPage as u8);
    }

    #[derive(Clone, Copy)]
    enum FlashFailure {
        None,
        FirstPageStatus,
        RollbackReadbackMismatch,
        DisconnectDuringRollback,
    }

    struct MemoryFlashSerial {
        state: Arc<Mutex<MemoryFlashState>>,
        pending_read: Option<(u32, usize)>,
        pending_write: Option<(u32, usize)>,
        statuses: VecDeque<u8>,
    }

    struct MemoryFlashState {
        bytes: Vec<u8>,
        fail_page_status_once: bool,
        corrupt_rollback_readback: bool,
        disconnect_during_rollback: bool,
        erase_commands: usize,
        page_commands: usize,
    }

    impl MemoryFlashSerial {
        fn new(failure: FlashFailure) -> (Self, Arc<Mutex<MemoryFlashState>>) {
            let bytes = (0..0x0080_0000_usize)
                .map(|index| u8::try_from(index % 251).unwrap())
                .collect();
            let state = Arc::new(Mutex::new(MemoryFlashState {
                bytes,
                fail_page_status_once: !matches!(failure, FlashFailure::None),
                corrupt_rollback_readback: matches!(
                    failure,
                    FlashFailure::RollbackReadbackMismatch
                ),
                disconnect_during_rollback: matches!(
                    failure,
                    FlashFailure::DisconnectDuringRollback
                ),
                erase_commands: 0,
                page_commands: 0,
            }));
            (
                Self {
                    state: Arc::clone(&state),
                    pending_read: None,
                    pending_write: None,
                    statuses: VecDeque::new(),
                },
                state,
            )
        }

        fn decode_header(bytes: &[u8]) -> (u32, usize) {
            let address = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
            let length = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
            (address, usize::try_from(length).unwrap())
        }
    }

    #[async_trait]
    impl SerialIo for MemoryFlashSerial {
        fn clear_input(&mut self) -> Result<(), DeviceError> {
            self.statuses.clear();
            Ok(())
        }

        async fn write_all(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
            if let Some((address, length)) = self.pending_write.take() {
                assert_eq!(bytes.len(), length);
                let start = usize::try_from(address).unwrap();
                let mut state = self.state.lock().unwrap();
                state.bytes[start..start + length].copy_from_slice(bytes);
                state.page_commands += 1;
                if std::mem::take(&mut state.fail_page_status_once) {
                    self.statuses.push_back(0);
                } else {
                    self.statuses.push_back(1);
                }
                return Ok(());
            }

            assert_eq!(bytes.len(), 9);
            let (address, length) = Self::decode_header(bytes);
            match UsbCommand::try_from(bytes[0]).unwrap() {
                UsbCommand::SpiFlashReadPage => self.pending_read = Some((address, length)),
                UsbCommand::SpiFlashWritePage => self.pending_write = Some((address, length)),
                UsbCommand::SpiFlashEraseSector => {
                    let start = usize::try_from(address).unwrap();
                    let mut state = self.state.lock().unwrap();
                    if state.disconnect_during_rollback && state.erase_commands == 1 {
                        return Err(DeviceError::ConnectionLost);
                    }
                    state.bytes[start..start + length].fill(0xff);
                    state.erase_commands += 1;
                    self.statuses.push_back(1);
                }
                command => panic!("unexpected command {command:?}"),
            }
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), DeviceError> {
            Ok(())
        }

        async fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), DeviceError> {
            if let Some((address, length)) = self.pending_read.take() {
                assert_eq!(bytes.len(), length);
                let start = usize::try_from(address).unwrap();
                let mut state = self.state.lock().unwrap();
                bytes.copy_from_slice(&state.bytes[start..start + length]);
                if state.corrupt_rollback_readback && state.erase_commands >= 2 {
                    bytes[0] ^= 1;
                    state.corrupt_rollback_readback = false;
                }
                return Ok(());
            }
            assert_eq!(bytes.len(), 1);
            bytes[0] = self.statuses.pop_front().expect("status response");
            Ok(())
        }
    }

    #[tokio::test]
    async fn theme_transaction_changes_only_the_selected_slot() {
        let (mut serial, state) = MemoryFlashSerial::new(FlashFailure::None);
        let slot = ThemeAssetSlot::FanDark1;
        let sector_start = slot.address() / SPI_SECTOR_SIZE * SPI_SECTOR_SIZE;
        let sector_end = (slot.address() + u32::try_from(slot.byte_len()).unwrap())
            .div_ceil(SPI_SECTOR_SIZE)
            * SPI_SECTOR_SIZE;
        let before = state.lock().unwrap().bytes
            [usize::try_from(sector_start).unwrap()..usize::try_from(sector_end).unwrap()]
            .to_vec();
        let replacement = vec![0x5a; slot.byte_len()];

        write_theme_asset_transaction(&mut serial, slot, &replacement)
            .await
            .unwrap();

        let state = state.lock().unwrap();
        let after = &state.bytes
            [usize::try_from(sector_start).unwrap()..usize::try_from(sector_end).unwrap()];
        let offset = usize::try_from(slot.address() - sector_start).unwrap();
        assert_eq!(&after[..offset], &before[..offset]);
        assert_eq!(&after[offset..offset + replacement.len()], replacement);
        assert_eq!(
            &after[offset + replacement.len()..],
            &before[offset + replacement.len()..]
        );
        assert_eq!(state.erase_commands, 1);
        assert_eq!(
            state.page_commands,
            usize::try_from(sector_end - sector_start).unwrap() / SPI_WRITE_PAGE_SIZE
        );
    }

    #[tokio::test]
    async fn failed_page_write_restores_and_verifies_the_sector_backup() {
        let (mut serial, state) = MemoryFlashSerial::new(FlashFailure::FirstPageStatus);
        let slot = ThemeAssetSlot::FanOrange1;
        let original = state.lock().unwrap().bytes.clone();

        let error = write_theme_asset_transaction(&mut serial, slot, &vec![0xa5; slot.byte_len()])
            .await
            .unwrap_err();

        assert!(matches!(error, DeviceError::FailedAndRolledBack(_)));
        let state = state.lock().unwrap();
        assert_eq!(state.bytes, original);
        assert_eq!(state.erase_commands, 2);
    }

    #[tokio::test]
    async fn rollback_readback_mismatch_is_reported_distinctly() {
        let (mut serial, _) = MemoryFlashSerial::new(FlashFailure::RollbackReadbackMismatch);
        let slot = ThemeAssetSlot::FanOrange2;

        assert!(matches!(
            write_theme_asset_transaction(&mut serial, slot, &vec![0x71; slot.byte_len()]).await,
            Err(DeviceError::RollbackFailed { rollback, .. })
                if rollback.contains("rollback readback")
        ));
    }

    #[tokio::test]
    async fn disconnect_during_rollback_stops_without_another_mutation() {
        let (mut serial, state) = MemoryFlashSerial::new(FlashFailure::DisconnectDuringRollback);
        let slot = ThemeAssetSlot::FanDark2;

        assert_eq!(
            write_theme_asset_transaction(&mut serial, slot, &vec![0x72; slot.byte_len()]).await,
            Err(DeviceError::ConnectionLost)
        );
        let state = state.lock().unwrap();
        assert_eq!(state.erase_commands, 1);
        assert_eq!(state.page_commands, 1);
    }
}
