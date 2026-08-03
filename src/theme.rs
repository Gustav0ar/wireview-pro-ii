//! Typed access to the fixed RGB565 bitmap slots in device SPI flash.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::DeviceError;

pub const BACKGROUND_WIDTH: u32 = 320;
pub const BACKGROUND_HEIGHT: u32 = 170;
pub const BACKGROUND_BYTES: usize = BACKGROUND_WIDTH as usize * BACKGROUND_HEIGHT as usize * 2;
pub const FAN_WIDTH: u32 = 73;
pub const FAN_HEIGHT: u32 = 73;
pub const FAN_FRAME_BYTES: usize = FAN_WIDTH as usize * FAN_HEIGHT as usize * 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeAssetSlot {
    BackgroundOrange,
    BackgroundDark,
    FanOrange1,
    FanOrange2,
    FanDark1,
    FanDark2,
    FanBlackWhite1,
    FanBlackWhite2,
}

impl ThemeAssetSlot {
    pub const ALL: [Self; 8] = [
        Self::BackgroundOrange,
        Self::BackgroundDark,
        Self::FanOrange1,
        Self::FanOrange2,
        Self::FanDark1,
        Self::FanDark2,
        Self::FanBlackWhite1,
        Self::FanBlackWhite2,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BackgroundOrange => "background-orange",
            Self::BackgroundDark => "background-dark",
            Self::FanOrange1 => "fan-orange-1",
            Self::FanOrange2 => "fan-orange-2",
            Self::FanDark1 => "fan-dark-1",
            Self::FanDark2 => "fan-dark-2",
            Self::FanBlackWhite1 => "fan-black-white-1",
            Self::FanBlackWhite2 => "fan-black-white-2",
        }
    }

    #[must_use]
    pub(crate) const fn address(self) -> u32 {
        match self {
            Self::BackgroundOrange => 0x0000_3000,
            Self::BackgroundDark => 0x0001_D900,
            Self::FanOrange1 => 0x0005_6374,
            Self::FanOrange2 => 0x0005_B6BC,
            Self::FanDark1 => 0x0005_8D18,
            Self::FanDark2 => 0x0005_E060,
            Self::FanBlackWhite1 => 0x0006_0A04,
            Self::FanBlackWhite2 => 0x0006_33A8,
        }
    }

    #[must_use]
    pub const fn byte_len(self) -> usize {
        match self {
            Self::BackgroundOrange | Self::BackgroundDark => BACKGROUND_BYTES,
            Self::FanOrange1
            | Self::FanOrange2
            | Self::FanDark1
            | Self::FanDark2
            | Self::FanBlackWhite1
            | Self::FanBlackWhite2 => FAN_FRAME_BYTES,
        }
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        match self {
            Self::BackgroundOrange | Self::BackgroundDark => BACKGROUND_WIDTH,
            _ => FAN_WIDTH,
        }
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        match self {
            Self::BackgroundOrange | Self::BackgroundDark => BACKGROUND_HEIGHT,
            _ => FAN_HEIGHT,
        }
    }
}

impl fmt::Display for ThemeAssetSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for ThemeAssetSlot {
    type Err = DeviceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|slot| slot.name().eq_ignore_ascii_case(value))
            .ok_or_else(|| {
                DeviceError::InvalidArgument(format!("unknown theme asset slot {value:?}"))
            })
    }
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_has_the_recovered_address_geometry_and_size() {
        let expected = [
            (
                ThemeAssetSlot::BackgroundOrange,
                0x0000_3000,
                320,
                170,
                108_800,
            ),
            (
                ThemeAssetSlot::BackgroundDark,
                0x0001_D900,
                320,
                170,
                108_800,
            ),
            (ThemeAssetSlot::FanOrange1, 0x0005_6374, 73, 73, 10_658),
            (ThemeAssetSlot::FanOrange2, 0x0005_B6BC, 73, 73, 10_658),
            (ThemeAssetSlot::FanDark1, 0x0005_8D18, 73, 73, 10_658),
            (ThemeAssetSlot::FanDark2, 0x0005_E060, 73, 73, 10_658),
            (ThemeAssetSlot::FanBlackWhite1, 0x0006_0A04, 73, 73, 10_658),
            (ThemeAssetSlot::FanBlackWhite2, 0x0006_33A8, 73, 73, 10_658),
        ];

        for (slot, address, width, height, byte_len) in expected {
            assert_eq!(slot.address(), address);
            assert_eq!(slot.width(), width);
            assert_eq!(slot.height(), height);
            assert_eq!(slot.byte_len(), byte_len);
            assert_eq!(slot.byte_len(), width as usize * height as usize * 2);
            assert!(address as usize + byte_len <= crate::history::FLASH_START_ADDRESS as usize);
            assert_eq!(slot.to_string().parse(), Ok(slot));
        }
    }

    #[test]
    fn rejects_addresses_and_unknown_names() {
        assert!("0x3000".parse::<ThemeAssetSlot>().is_err());
        assert!("background".parse::<ThemeAssetSlot>().is_err());
        assert!("fan-orange-3".parse::<ThemeAssetSlot>().is_err());
    }

    #[test]
    fn sha256_is_lowercase_and_stable() {
        assert_eq!(
            sha256_hex(b"wireview"),
            "3e8a2b8400941fd1c353f09ef9848f0ad498986d8bb0ee0e0ea114fdfd135290"
        );
    }
}
