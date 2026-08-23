//! WireView Pro II USB protocol definitions and codecs.
//!
//! Commands are unframed byte sequences. The read-only transactions and screen
//! subcommands used here are covered by fixtures and hardware tests. NVM
//! configuration and the non-destructive device reboot are exposed with
//! safety controls. Calibration, arbitrary flash writes/erase, and bootloader
//! operations remain deliberately absent from the backend API.

use serde::{Deserialize, Serialize};

use crate::domain::{DeviceError, Metrics, PinMetrics, Temperatures};

pub const WELCOME_MESSAGE: &[u8; 31] = b"Thermal Grizzly WireView Pro II";
pub const WELCOME_RESPONSE_SIZE: usize = WELCOME_MESSAGE.len() + 1;
pub const VENDOR_RESPONSE_SIZE: usize = 3;
pub const UID_RESPONSE_SIZE: usize = 12;
pub const BUILD_RESPONSE_SIZE: usize = 68;
pub const CONFIG_VERSION_RESPONSE_SIZE: usize = 4;
pub const SENSOR_RESPONSE_SIZE: usize = 100;
pub const KNOWN_FAULT_MASK: u16 = 0x003f;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum UsbCommand {
    Welcome = 0,
    ReadVendorData = 1,
    ReadUid = 2,
    ReadDeviceData = 3,
    ReadSensorValues = 4,
    ReadConfig = 5,
    WriteConfig = 6,
    ReadCalibration = 7,
    WriteCalibration = 8,
    SpiFlashWritePage = 9,
    SpiFlashReadPage = 10,
    SpiFlashEraseSector = 11,
    ScreenChange = 12,
    ReadBuildInfo = 13,
    ClearFaults = 14,
    Reset = 240,
    Bootloader = 241,
    NvmConfig = 242,
    Nop = 255,
}

impl TryFrom<u8> for UsbCommand {
    type Error = UnknownCommand;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Welcome,
            1 => Self::ReadVendorData,
            2 => Self::ReadUid,
            3 => Self::ReadDeviceData,
            4 => Self::ReadSensorValues,
            5 => Self::ReadConfig,
            6 => Self::WriteConfig,
            7 => Self::ReadCalibration,
            8 => Self::WriteCalibration,
            9 => Self::SpiFlashWritePage,
            10 => Self::SpiFlashReadPage,
            11 => Self::SpiFlashEraseSector,
            12 => Self::ScreenChange,
            13 => Self::ReadBuildInfo,
            14 => Self::ClearFaults,
            240 => Self::Reset,
            241 => Self::Bootloader,
            242 => Self::NvmConfig,
            255 => Self::Nop,
            other => return Err(UnknownCommand(other)),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unknown WireView USB command {0:#04x}")]
pub struct UnknownCommand(pub u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ScreenCommand {
    Main = 224,
    Simple = 225,
    Current = 226,
    Temp = 227,
    Status = 228,
    Same = 239,
    PauseUpdates = 240,
    ResumeUpdates = 241,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSensors {
    pub metrics: Metrics,
    pub fault_status: u16,
    pub fault_log: u16,
}

/// Decode the 100-byte, pack-4 sensor response.
///
/// # Errors
///
/// Returns [`DeviceError::Transport`] if the response has the wrong size or a
/// cable capability ordinal is unknown.
pub fn decode_sensor_response(bytes: &[u8]) -> Result<DecodedSensors, DeviceError> {
    if bytes.len() != SENSOR_RESPONSE_SIZE {
        return Err(DeviceError::Transport(format!(
            "sensor response has {} bytes, expected {SENSOR_RESPONSE_SIZE}",
            bytes.len()
        )));
    }
    let temp = |offset| f64::from(read_i16(bytes, offset)) / 10.0;
    let external = |offset| {
        let value = temp(offset);
        (value > -99.9).then_some(value)
    };
    let mut pins = Vec::with_capacity(6);
    for index in 0..6 {
        let offset = 12 + index * 12;
        let voltage_v = f64::from(read_i16(bytes, offset)) / 1000.0;
        let current_a = f64::from(read_u32(bytes, offset + 4)) / 1000.0;
        pins.push(PinMetrics {
            voltage_v,
            current_a,
            power_w: voltage_v * current_a,
        });
    }
    let total_current_a = pins.iter().map(|pin| pin.current_a).sum();
    let total_power_w = pins.iter().map(|pin| pin.power_w).sum();
    let cable_capability_w = match bytes[94] {
        0 => 600,
        1 => 450,
        2 => 300,
        3 => 150,
        value => {
            return Err(DeviceError::Transport(format!(
                "unknown cable capability {value}"
            )));
        }
    };
    Ok(DecodedSensors {
        metrics: Metrics {
            vdd_v: f64::from(read_u16(bytes, 8)) / 1000.0,
            avg_voltage_v: f64::from(read_u16(bytes, 92)) / 1000.0,
            total_current_a,
            total_power_w,
            fan_duty_percent: f64::from(bytes[10]),
            cable_capability_w,
            pins,
            temperatures: Temperatures {
                input_c: temp(0),
                output_c: temp(2),
                external_1_c: external(4),
                external_2_c: external(6),
            },
        },
        fault_status: read_u16(bytes, 96),
        fault_log: read_u16(bytes, 98),
    })
}

#[must_use]
pub fn decode_faults(mask: u16) -> Vec<String> {
    const NAMES: [&str; 6] = [
        "chip_over_temperature",
        "sensor_over_temperature",
        "over_current",
        "wire_over_current",
        "over_power",
        "current_imbalance",
    ];
    NAMES
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, name)| (*name).to_owned())
        .collect()
}

#[must_use]
pub fn clear_faults_command(active_clear_mask: u16, logged_clear_mask: u16) -> [u8; 5] {
    let mut command = [0_u8; 5];
    command[0] = UsbCommand::ClearFaults as u8;
    // Firmware ANDs each fault register with the supplied mask. The daemon API
    // accepts bits selected for clearing, so invert them at the protocol edge.
    command[1..3].copy_from_slice(&(!active_clear_mask).to_le_bytes());
    command[3..5].copy_from_slice(&(!logged_clear_mask).to_le_bytes());
    command
}

/// Decode the ANSI, pack-4 `BuildStruct` returned by command `0x0d`.
///
/// The structure contains three vendor bytes, two fixed 32-character strings,
/// and a trailing product-name length byte. Both strings are exposed for
/// diagnostics.
pub fn decode_build_response(bytes: &[u8]) -> Result<(String, String), DeviceError> {
    if bytes.len() != BUILD_RESPONSE_SIZE {
        return Err(DeviceError::Transport(format!(
            "build response has {} bytes, expected {BUILD_RESPONSE_SIZE}",
            bytes.len()
        )));
    }
    let product_name = decode_fixed_ansi(&bytes[3..35])?;
    let build_info = decode_fixed_ansi(&bytes[35..67])?;
    Ok((product_name, build_info))
}

fn decode_fixed_ansi(bytes: &[u8]) -> Result<String, DeviceError> {
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = &bytes[..length];
    if !value.is_ascii() || value.iter().any(|byte| byte.is_ascii_control()) {
        return Err(DeviceError::Transport(
            "device build information is not printable ASCII".into(),
        ));
    }
    Ok(String::from_utf8(value.to_vec()).expect("ASCII is valid UTF-8"))
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i16(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_command_ordinals_round_trip() {
        for value in [
            0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 240, 241, 242, 255,
        ] {
            let command = UsbCommand::try_from(value).expect("known command");
            assert_eq!(command as u8, value);
        }
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert_eq!(UsbCommand::try_from(15), Err(UnknownCommand(15)));
    }

    #[test]
    fn sensor_struct_decodes_pack_4_layout_and_scaling() {
        let mut bytes = [0_u8; SENSOR_RESPONSE_SIZE];
        bytes[0..2].copy_from_slice(&428_i16.to_le_bytes());
        bytes[2..4].copy_from_slice(&380_i16.to_le_bytes());
        bytes[4..6].copy_from_slice(&(-1000_i16).to_le_bytes());
        bytes[6..8].copy_from_slice(&251_i16.to_le_bytes());
        bytes[8..10].copy_from_slice(&3300_u16.to_le_bytes());
        bytes[10] = 40;
        for index in 0..6 {
            let offset = 12 + index * 12;
            bytes[offset..offset + 2].copy_from_slice(&12080_i16.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&900_u32.to_le_bytes());
        }
        bytes[92..94].copy_from_slice(&12080_u16.to_le_bytes());
        bytes[94] = 0;
        bytes[96..98].copy_from_slice(&0b100100_u16.to_le_bytes());
        let decoded = decode_sensor_response(&bytes).unwrap();
        assert_eq!(decoded.metrics.pins.len(), 6);
        assert_eq!(decoded.metrics.avg_voltage_v, 12.08);
        assert_eq!(decoded.metrics.temperatures.external_1_c, None);
        assert_eq!(decoded.metrics.temperatures.external_2_c, Some(25.1));
        assert_eq!(decoded.metrics.cable_capability_w, 600);
        assert_eq!(
            decode_faults(decoded.fault_status),
            vec!["over_current", "current_imbalance"]
        );
    }

    #[test]
    fn sensor_decoder_rejects_wrong_size_and_capability() {
        assert!(decode_sensor_response(&[0; 99]).is_err());
        let mut bytes = [0_u8; SENSOR_RESPONSE_SIZE];
        bytes[94] = 9;
        assert!(decode_sensor_response(&bytes).is_err());
    }

    #[test]
    fn build_struct_decodes_pack_4_fixed_strings() {
        let mut bytes = [0_u8; BUILD_RESPONSE_SIZE];
        bytes[..3].copy_from_slice(&[0xef, 0x05, 4]);
        bytes[3..20].copy_from_slice(b"WireView Pro II\0\0");
        bytes[35..48].copy_from_slice(b"20260729_1902");
        let (product, build) = decode_build_response(&bytes).unwrap();
        assert_eq!(product, "WireView Pro II");
        assert_eq!(build, "20260729_1902");
    }

    #[test]
    fn fault_decoder_preserves_unknown_bits_separately() {
        assert_eq!(decode_faults(0x8004), vec!["over_current"]);
        assert_eq!(0x8004 & !KNOWN_FAULT_MASK, 0x8000);
    }

    #[test]
    fn clear_faults_frame_inverts_selected_bits_into_firmware_retain_masks() {
        assert_eq!(
            clear_faults_command(0x1234, 0xabcd),
            [0x0e, 0xcb, 0xed, 0x32, 0x54]
        );
        assert_eq!(
            clear_faults_command(0x0020, 0),
            [0x0e, 0xdf, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            clear_faults_command(0x0020, 0x0020),
            [0x0e, 0xdf, 0xff, 0xdf, 0xff]
        );
    }
}
