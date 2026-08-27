use serde::{Deserialize, Serialize};

use crate::domain::DeviceError;

pub const CONFIG_V1_SIZE: usize = 72;
pub const CONFIG_V2_SIZE: usize = 74;
pub const CONFIG_V3_SIZE: usize = 96;
pub const CONFIG_WRITE_PAYLOAD: usize = 62;
pub const FAULT_MASK: u16 = 0x003f;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum NvmOperation {
    Reload = 1,
    Store = 2,
    Reset = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanMode {
    Curve,
    Fixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureSource {
    Input,
    Output,
    External1,
    External2,
    Maximum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    ChipOverTemperature,
    SensorOverTemperature,
    OverCurrent,
    WireOverCurrent,
    OverPower,
    CurrentImbalance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScreen {
    Main,
    Simple,
    Current,
    Temperature,
    Status,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutMode {
    Static,
    Cycle,
    Sleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerScale {
    Auto,
    Watts300,
    Watts600,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Background {
    ThermalGrizzlyOrange,
    ThermalGrizzlyDark,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanTheme {
    ThermalGrizzlyOrange,
    ThermalGrizzlyDark,
    BlackAndWhite,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanConfiguration {
    pub mode: FanMode,
    pub temperature_source: TemperatureSource,
    pub duty_min_percent: u8,
    pub duty_max_percent: u8,
    pub temperature_min_c: f64,
    pub temperature_max_c: f64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultActions {
    pub display: Vec<FaultKind>,
    pub buzzer: Vec<FaultKind>,
    pub soft_power: Vec<FaultKind>,
    pub hard_power: Vec<FaultKind>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultThresholds {
    pub temperature_c: f64,
    pub total_current_a: u8,
    pub wire_current_a: f64,
    pub total_power_w: u16,
    pub current_imbalance_percent: u8,
    pub current_imbalance_min_load_a: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfiguration {
    pub default_screen: ConfigScreen,
    pub current_scale_a: u8,
    pub power_scale: PowerScale,
    pub rotation_degrees: u16,
    pub timeout_mode: TimeoutMode,
    pub cycle_screens: Vec<ConfigScreen>,
    pub cycle_time_seconds: u8,
    pub timeout_seconds: u8,
    /// Internal ARGB color (`0xAARRGGBB`).
    #[serde(with = "argb_hex")]
    pub primary_color: u32,
    /// Internal ARGB color (`0xAARRGGBB`).
    #[serde(with = "argb_hex")]
    pub secondary_color: u32,
    /// Internal ARGB color (`0xAARRGGBB`).
    #[serde(with = "argb_hex")]
    pub highlight_color: u32,
    /// Internal ARGB color (`0xAARRGGBB`).
    #[serde(with = "argb_hex")]
    pub background_color: u32,
    pub background: Background,
    pub fan_theme: FanTheme,
    pub inverted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfiguration {
    /// Raw firmware configuration ordinal: 0=V1, 1=V2, 2=V3.
    pub raw_version: u8,
    /// Current device CRC. Used as an optimistic-concurrency token.
    pub crc: u16,
    pub friendly_name: String,
    pub backlight_percent: u8,
    pub fan: FanConfiguration,
    pub fault_actions: FaultActions,
    pub fault_thresholds: FaultThresholds,
    pub shutdown_wait_seconds: u8,
    pub logging_interval_seconds: u8,
    pub averaging_ms: u16,
    pub display: DisplayConfiguration,
}

/// User-editable settings. Protocol metadata is deliberately kept internal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSettings {
    pub friendly_name: String,
    pub backlight_percent: u8,
    pub fan: FanConfiguration,
    pub fault_actions: FaultActions,
    pub fault_thresholds: FaultThresholds,
    pub shutdown_wait_seconds: u8,
    pub logging_interval_seconds: u8,
    pub averaging_ms: u16,
    pub display: DisplayConfiguration,
}

impl DeviceSettings {
    /// Parses and validates the complete public configuration document.
    ///
    /// Keeping this in the daemon library gives every transport the same
    /// strict type, field-name, range, precision, and cross-field checks.
    pub fn from_json(json: &str) -> Result<Self, DeviceError> {
        let settings: Self = serde_json::from_str(json).map_err(|error| {
            DeviceError::InvalidArgument(format!("invalid configuration JSON: {error}"))
        })?;
        settings.validate()?;
        Ok(settings)
    }

    /// Returns one dotted-path configuration leaf as a typed JSON value.
    pub fn item(&self, key: &str) -> Result<serde_json::Value, DeviceError> {
        if key.is_empty() {
            return invalid("configuration key cannot be empty");
        }
        let document = serde_json::to_value(self).map_err(|error| {
            DeviceError::InvalidArgument(format!("failed to inspect configuration: {error}"))
        })?;
        let mut current = &document;
        for component in key.split('.') {
            if component.is_empty() {
                return invalid(&format!("invalid configuration key \"{key}\""));
            }
            let object = current.as_object().ok_or_else(|| {
                DeviceError::InvalidArgument(format!("\"{key}\" is not a configuration item"))
            })?;
            current = object.get(component).ok_or_else(|| {
                DeviceError::InvalidArgument(format!("unknown configuration key \"{key}\""))
            })?;
        }
        if current.is_object() {
            return invalid(&format!(
                "\"{key}\" is a configuration section, not an item"
            ));
        }
        Ok(current.clone())
    }

    /// Returns a validated copy with one dotted-path leaf changed.
    ///
    /// Scalar values use their normal CLI spelling. List settings accept
    /// comma-separated enum names, while `none` or an empty value clears the
    /// list.
    pub fn with_item(&self, key: &str, raw_value: &str) -> Result<Self, DeviceError> {
        let mut document = serde_json::to_value(self).map_err(|error| {
            DeviceError::InvalidArgument(format!("failed to inspect configuration: {error}"))
        })?;
        set_item_value(&mut document, key, raw_value)?;
        settings_from_value(&document)
    }

    /// Returns a validated copy after applying a group of dotted-path edits atomically.
    ///
    /// Cross-field constraints are checked against the final document. This lets a
    /// form change both sides of a range, such as fan duty minimum and maximum,
    /// without an otherwise-invalid intermediate state.
    pub fn with_items<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, DeviceError> {
        let mut document = serde_json::to_value(self).map_err(|error| {
            DeviceError::InvalidArgument(format!("failed to inspect configuration: {error}"))
        })?;
        for (key, raw_value) in items {
            set_item_value(&mut document, key, raw_value)?;
        }
        settings_from_value(&document)
    }

    #[must_use]
    pub fn from_configuration(configuration: &DeviceConfiguration) -> Self {
        Self {
            friendly_name: configuration.friendly_name.clone(),
            backlight_percent: configuration.backlight_percent,
            fan: configuration.fan.clone(),
            fault_actions: configuration.fault_actions.clone(),
            fault_thresholds: configuration.fault_thresholds.clone(),
            shutdown_wait_seconds: configuration.shutdown_wait_seconds,
            logging_interval_seconds: configuration.logging_interval_seconds,
            averaging_ms: configuration.averaging_ms,
            display: configuration.display.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), DeviceError> {
        if self.friendly_name.chars().count() > 32
            || !self
                .friendly_name
                .chars()
                .all(|character| character.is_ascii_graphic() || character == ' ')
        {
            return invalid("friendly_name must contain at most 32 printable ASCII characters");
        }
        percent("backlight_percent", self.backlight_percent)?;
        percent("fan.duty_min_percent", self.fan.duty_min_percent)?;
        percent("fan.duty_max_percent", self.fan.duty_max_percent)?;
        if self.fan.duty_min_percent > self.fan.duty_max_percent {
            return invalid("fan duty minimum cannot exceed its maximum");
        }
        validate_tenths_range(
            "fan.temperature_min_c",
            self.fan.temperature_min_c,
            0.0,
            50.0,
        )?;
        validate_tenths_range(
            "fan.temperature_max_c",
            self.fan.temperature_max_c,
            50.0,
            100.0,
        )?;
        if self.fan.temperature_min_c > self.fan.temperature_max_c {
            return invalid("fan temperature minimum cannot exceed its maximum");
        }
        validate_tenths_range(
            "fault_thresholds.temperature_c",
            self.fault_thresholds.temperature_c,
            0.0,
            120.0,
        )?;
        if self.fault_thresholds.total_current_a > 150 {
            return invalid("fault_thresholds.total_current_a must be between 0 and 150");
        }
        encode_tenths_u8(
            "fault_thresholds.wire_current_a",
            self.fault_thresholds.wire_current_a,
        )?;
        if self.fault_thresholds.total_power_w > 2000 {
            return invalid("fault_thresholds.total_power_w must be between 0 and 2000");
        }
        percent(
            "fault_thresholds.current_imbalance_percent",
            self.fault_thresholds.current_imbalance_percent,
        )?;
        if self.fault_thresholds.current_imbalance_min_load_a > 10 {
            return invalid(
                "fault_thresholds.current_imbalance_min_load_a must be between 0 and 10",
            );
        }
        for (name, faults) in [
            ("fault_actions.display", &self.fault_actions.display),
            ("fault_actions.buzzer", &self.fault_actions.buzzer),
            ("fault_actions.soft_power", &self.fault_actions.soft_power),
            ("fault_actions.hard_power", &self.fault_actions.hard_power),
        ] {
            let mask = encode_faults(faults);
            if mask.count_ones() as usize != faults.len() {
                return invalid(&format!("{name} contains duplicate entries"));
            }
        }
        if ![5, 10, 15, 20].contains(&self.display.current_scale_a) {
            return invalid("display.current_scale_a must be 5, 10, 15, or 20");
        }
        if ![0, 180].contains(&self.display.rotation_degrees) {
            return invalid("display.rotation_degrees must be 0 or 180");
        }
        let cycle_mask = encode_screens(&self.display.cycle_screens);
        if cycle_mask.count_ones() as usize != self.display.cycle_screens.len() {
            return invalid("display.cycle_screens contains duplicate entries");
        }
        if ![22, 44, 89, 177, 354, 709, 1417].contains(&self.averaging_ms) {
            return invalid("averaging_ms must be 22, 44, 89, 177, 354, 709, or 1417");
        }
        Ok(())
    }

    pub fn with_protocol_metadata(
        self,
        current: &DeviceConfiguration,
    ) -> Result<DeviceConfiguration, DeviceError> {
        self.validate()?;
        Ok(DeviceConfiguration {
            raw_version: current.raw_version,
            crc: current.crc,
            friendly_name: self.friendly_name,
            backlight_percent: self.backlight_percent,
            fan: self.fan,
            fault_actions: self.fault_actions,
            fault_thresholds: self.fault_thresholds,
            shutdown_wait_seconds: self.shutdown_wait_seconds,
            logging_interval_seconds: self.logging_interval_seconds,
            averaging_ms: self.averaging_ms,
            display: self.display,
        })
    }
}

fn parse_item_value(
    key: &str,
    current: &serde_json::Value,
    raw_value: &str,
) -> Result<serde_json::Value, DeviceError> {
    match current {
        serde_json::Value::String(_) => Ok(serde_json::Value::String(raw_value.to_owned())),
        serde_json::Value::Bool(_) => match raw_value {
            "true" => Ok(serde_json::Value::Bool(true)),
            "false" => Ok(serde_json::Value::Bool(false)),
            _ => invalid(&format!("\"{key}\" expects true or false")),
        },
        serde_json::Value::Number(_) => {
            let parsed: serde_json::Value = serde_json::from_str(raw_value)
                .map_err(|_| DeviceError::InvalidArgument(format!("\"{key}\" expects a number")))?;
            if parsed.is_number() {
                Ok(parsed)
            } else {
                invalid(&format!("\"{key}\" expects a number"))
            }
        }
        serde_json::Value::Array(_) => {
            if raw_value.is_empty() || raw_value == "none" {
                return Ok(serde_json::Value::Array(Vec::new()));
            }
            let values = raw_value
                .split(',')
                .map(str::trim)
                .map(|value| {
                    if value.is_empty() {
                        return invalid(&format!(
                            "\"{key}\" contains an empty comma-separated value"
                        ));
                    }
                    Ok(serde_json::Value::String(value.to_owned()))
                })
                .collect::<Result<Vec<_>, DeviceError>>()?;
            Ok(serde_json::Value::Array(values))
        }
        serde_json::Value::Object(_) => invalid(&format!(
            "\"{key}\" is a configuration section, not an editable item"
        )),
        serde_json::Value::Null => invalid(&format!("\"{key}\" cannot be changed")),
    }
}

fn set_item_value(
    document: &mut serde_json::Value,
    key: &str,
    raw_value: &str,
) -> Result<(), DeviceError> {
    if key.is_empty() {
        return invalid("configuration key cannot be empty");
    }
    let mut current = document;
    let mut components = key.split('.').peekable();
    while let Some(component) = components.next() {
        if component.is_empty() {
            return invalid(&format!("invalid configuration key \"{key}\""));
        }
        let object = current.as_object_mut().ok_or_else(|| {
            DeviceError::InvalidArgument(format!("\"{key}\" is not an editable configuration item"))
        })?;
        let next = object.get_mut(component).ok_or_else(|| {
            DeviceError::InvalidArgument(format!("unknown configuration key \"{key}\""))
        })?;
        if components.peek().is_none() {
            if next.is_object() {
                return invalid(&format!(
                    "\"{key}\" is a configuration section, not an editable item"
                ));
            }
            *next = parse_item_value(key, next, raw_value)?;
            return Ok(());
        }
        current = next;
    }
    invalid("configuration key cannot be empty")
}

fn settings_from_value(document: &serde_json::Value) -> Result<DeviceSettings, DeviceError> {
    let json = serde_json::to_string(document).map_err(|error| {
        DeviceError::InvalidArgument(format!("failed to update configuration: {error}"))
    })?;
    DeviceSettings::from_json(&json)
}

impl DeviceConfiguration {
    #[must_use]
    pub fn mock() -> Self {
        Self {
            raw_version: 2,
            crc: 0x1234,
            friendly_name: "Mock WireView".into(),
            backlight_percent: 100,
            fan: FanConfiguration {
                mode: FanMode::Curve,
                temperature_source: TemperatureSource::Maximum,
                duty_min_percent: 0,
                duty_max_percent: 100,
                temperature_min_c: 50.0,
                temperature_max_c: 80.0,
            },
            fault_actions: FaultActions {
                display: vec![
                    FaultKind::ChipOverTemperature,
                    FaultKind::SensorOverTemperature,
                    FaultKind::OverCurrent,
                    FaultKind::WireOverCurrent,
                    FaultKind::OverPower,
                    FaultKind::CurrentImbalance,
                ],
                buzzer: vec![
                    FaultKind::ChipOverTemperature,
                    FaultKind::SensorOverTemperature,
                    FaultKind::OverCurrent,
                    FaultKind::WireOverCurrent,
                    FaultKind::OverPower,
                ],
                soft_power: Vec::new(),
                hard_power: vec![
                    FaultKind::ChipOverTemperature,
                    FaultKind::SensorOverTemperature,
                    FaultKind::OverCurrent,
                    FaultKind::WireOverCurrent,
                    FaultKind::OverPower,
                ],
            },
            fault_thresholds: FaultThresholds {
                temperature_c: 80.0,
                total_current_a: 55,
                wire_current_a: 10.5,
                total_power_w: 660,
                current_imbalance_percent: 40,
                current_imbalance_min_load_a: 6,
            },
            shutdown_wait_seconds: 10,
            logging_interval_seconds: 60,
            averaging_ms: 1417,
            display: DisplayConfiguration {
                default_screen: ConfigScreen::Main,
                current_scale_a: 10,
                power_scale: PowerScale::Watts600,
                rotation_degrees: 0,
                timeout_mode: TimeoutMode::Static,
                cycle_screens: vec![
                    ConfigScreen::Main,
                    ConfigScreen::Simple,
                    ConfigScreen::Current,
                    ConfigScreen::Temperature,
                    ConfigScreen::Status,
                ],
                cycle_time_seconds: 10,
                timeout_seconds: 30,
                primary_color: 0xffff_ffff,
                secondary_color: 0xff64_6464,
                highlight_color: 0xffe6_4121,
                background_color: 0xff00_0000,
                background: Background::ThermalGrizzlyOrange,
                fan_theme: FanTheme::ThermalGrizzlyOrange,
                inverted: false,
            },
        }
    }

    pub fn validate(&self) -> Result<(), DeviceError> {
        if self.raw_version > 2 {
            return invalid("raw_version must be 0, 1, or 2");
        }
        DeviceSettings::from_configuration(self).validate()?;
        self.validate_version_specific_fields()
    }

    fn validate_version_specific_fields(&self) -> Result<(), DeviceError> {
        if self.raw_version == 0 && self.averaging_ms != 1417 {
            return invalid("averaging_ms is fixed at 1417 on V1 devices");
        }
        if self.raw_version >= 2 {
            return Ok(());
        }
        if self.display.default_screen != ConfigScreen::Main {
            return invalid("display.default_screen is fixed at main on V1 and V2 devices");
        }
        if self.display.inverted {
            return invalid("display.inverted is not supported on V1 and V2 devices");
        }
        let (expected_fan_theme, expected_colors) = legacy_theme_settings(self.display.background);
        if self.display.fan_theme != expected_fan_theme {
            return invalid("display.fan_theme must match display.background on V1 and V2 devices");
        }
        let colors = [
            self.display.primary_color,
            self.display.secondary_color,
            self.display.highlight_color,
            self.display.background_color,
        ];
        if colors != expected_colors {
            return invalid("display colors are fixed by display.background on V1 and V2 devices");
        }
        Ok(())
    }
}

pub fn decode_configuration(
    raw_version: u8,
    bytes: &[u8],
) -> Result<DeviceConfiguration, DeviceError> {
    let expected = configuration_size(raw_version)?;
    if bytes.len() != expected {
        return Err(DeviceError::Transport(format!(
            "configuration V{} has {} bytes, expected {expected}",
            raw_version + 1,
            bytes.len()
        )));
    }
    if bytes[2] != raw_version {
        let prefix = bytes
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(DeviceError::Transport(format!(
            "configuration payload version {} does not match probed version {raw_version} \
             (first bytes: {prefix})",
            bytes[2],
        )));
    }
    let ui_offset = match raw_version {
        0 => 64,
        1 => 65,
        2 => 68,
        _ => unreachable!(),
    };
    let legacy_theme = (raw_version < 2).then(|| decode_legacy_theme(bytes[ui_offset + 2]));
    let display = if raw_version < 2 {
        let (background, fan_theme, colors) = legacy_theme.expect("legacy version");
        DisplayConfiguration {
            default_screen: ConfigScreen::Main,
            current_scale_a: decode_current_scale(bytes[ui_offset])?,
            power_scale: decode_power_scale(bytes[ui_offset + 1])?,
            rotation_degrees: decode_rotation(bytes[ui_offset + 3])?,
            timeout_mode: decode_timeout_mode(bytes[ui_offset + 4])?,
            cycle_screens: decode_screens(bytes[ui_offset + 5])?,
            cycle_time_seconds: bytes[ui_offset + 6],
            timeout_seconds: bytes[ui_offset + 7],
            primary_color: colors[0],
            secondary_color: colors[1],
            highlight_color: colors[2],
            background_color: colors[3],
            background,
            fan_theme,
            inverted: false,
        }
    } else {
        DisplayConfiguration {
            default_screen: decode_screen(bytes[ui_offset])?,
            current_scale_a: decode_current_scale(bytes[ui_offset + 1])?,
            power_scale: decode_power_scale(bytes[ui_offset + 2])?,
            rotation_degrees: decode_rotation(bytes[ui_offset + 3])?,
            timeout_mode: decode_timeout_mode(bytes[ui_offset + 4])?,
            cycle_screens: decode_screens(bytes[ui_offset + 5])?,
            cycle_time_seconds: bytes[ui_offset + 6],
            timeout_seconds: bytes[ui_offset + 7],
            primary_color: read_u32(bytes, ui_offset + 8),
            secondary_color: read_u32(bytes, ui_offset + 12),
            highlight_color: read_u32(bytes, ui_offset + 16),
            background_color: read_u32(bytes, ui_offset + 20),
            background: decode_background(bytes[ui_offset + 24])?,
            fan_theme: decode_fan_theme(bytes[ui_offset + 25])?,
            inverted: match bytes[ui_offset + 26] {
                0 => false,
                1 => true,
                value => {
                    return Err(DeviceError::Transport(format!(
                        "unknown display inversion ordinal {value}"
                    )));
                }
            },
        }
    };
    let averaging_ms = if raw_version == 0 {
        1417
    } else {
        decode_averaging(bytes[64])?
    };
    let config = DeviceConfiguration {
        raw_version,
        crc: read_u16(bytes, 0),
        friendly_name: decode_name(&bytes[3..35])?,
        fan: FanConfiguration {
            mode: decode_fan_mode(bytes[36])?,
            temperature_source: decode_temperature_source(bytes[37])?,
            duty_min_percent: bytes[38],
            duty_max_percent: bytes[39],
            temperature_min_c: f64::from(read_i16(bytes, 40)) / 10.0,
            temperature_max_c: f64::from(read_i16(bytes, 42)) / 10.0,
        },
        backlight_percent: bytes[44],
        fault_actions: FaultActions {
            display: decode_faults(read_u16(bytes, 46))?,
            buzzer: decode_faults(read_u16(bytes, 48))?,
            soft_power: decode_faults(read_u16(bytes, 50))?,
            hard_power: decode_faults(read_u16(bytes, 52))?,
        },
        fault_thresholds: FaultThresholds {
            temperature_c: f64::from(read_i16(bytes, 54)) / 10.0,
            total_current_a: bytes[56],
            wire_current_a: f64::from(bytes[57]) / 10.0,
            total_power_w: read_u16(bytes, 58),
            current_imbalance_percent: bytes[60],
            current_imbalance_min_load_a: bytes[61],
        },
        shutdown_wait_seconds: bytes[62],
        logging_interval_seconds: bytes[63],
        averaging_ms,
        display,
    };
    config.validate()?;
    Ok(config)
}

pub fn encode_configuration(config: &DeviceConfiguration) -> Result<Vec<u8>, DeviceError> {
    config.validate()?;
    let mut bytes = vec![0_u8; configuration_size(config.raw_version)?];
    write_u16(&mut bytes, 0, config.crc);
    bytes[2] = config.raw_version;
    bytes[3..3 + config.friendly_name.len()].copy_from_slice(config.friendly_name.as_bytes());
    bytes[36] = config.fan.mode as u8;
    bytes[37] = config.fan.temperature_source as u8;
    bytes[38] = config.fan.duty_min_percent;
    bytes[39] = config.fan.duty_max_percent;
    write_i16(
        &mut bytes,
        40,
        encode_tenths_i16("fan.temperature_min_c", config.fan.temperature_min_c)?,
    );
    write_i16(
        &mut bytes,
        42,
        encode_tenths_i16("fan.temperature_max_c", config.fan.temperature_max_c)?,
    );
    bytes[44] = config.backlight_percent;
    write_u16(&mut bytes, 46, encode_faults(&config.fault_actions.display));
    write_u16(&mut bytes, 48, encode_faults(&config.fault_actions.buzzer));
    write_u16(
        &mut bytes,
        50,
        encode_faults(&config.fault_actions.soft_power),
    );
    write_u16(
        &mut bytes,
        52,
        encode_faults(&config.fault_actions.hard_power),
    );
    write_i16(
        &mut bytes,
        54,
        encode_tenths_i16(
            "fault_thresholds.temperature_c",
            config.fault_thresholds.temperature_c,
        )?,
    );
    bytes[56] = config.fault_thresholds.total_current_a;
    bytes[57] = encode_tenths_u8(
        "fault_thresholds.wire_current_a",
        config.fault_thresholds.wire_current_a,
    )?;
    write_u16(&mut bytes, 58, config.fault_thresholds.total_power_w);
    bytes[60] = config.fault_thresholds.current_imbalance_percent;
    bytes[61] = config.fault_thresholds.current_imbalance_min_load_a;
    bytes[62] = config.shutdown_wait_seconds;
    bytes[63] = config.logging_interval_seconds;

    let ui_offset = match config.raw_version {
        0 => 64,
        1 => {
            bytes[64] = encode_averaging(config.averaging_ms)?;
            65
        }
        2 => {
            bytes[64] = encode_averaging(config.averaging_ms)?;
            68
        }
        _ => unreachable!(),
    };
    if config.raw_version < 2 {
        bytes[ui_offset] = encode_current_scale(config.display.current_scale_a)?;
        bytes[ui_offset + 1] = config.display.power_scale as u8;
        bytes[ui_offset + 2] = encode_legacy_theme(config.display.background);
        bytes[ui_offset + 3] = encode_rotation(config.display.rotation_degrees)?;
        bytes[ui_offset + 4] = config.display.timeout_mode as u8;
        bytes[ui_offset + 5] = encode_screens(&config.display.cycle_screens);
        bytes[ui_offset + 6] = config.display.cycle_time_seconds;
        bytes[ui_offset + 7] = config.display.timeout_seconds;
    } else {
        bytes[ui_offset] = config.display.default_screen as u8;
        bytes[ui_offset + 1] = encode_current_scale(config.display.current_scale_a)?;
        bytes[ui_offset + 2] = config.display.power_scale as u8;
        bytes[ui_offset + 3] = encode_rotation(config.display.rotation_degrees)?;
        bytes[ui_offset + 4] = config.display.timeout_mode as u8;
        bytes[ui_offset + 5] = encode_screens(&config.display.cycle_screens);
        bytes[ui_offset + 6] = config.display.cycle_time_seconds;
        bytes[ui_offset + 7] = config.display.timeout_seconds;
        write_u32(&mut bytes, ui_offset + 8, config.display.primary_color);
        write_u32(&mut bytes, ui_offset + 12, config.display.secondary_color);
        write_u32(&mut bytes, ui_offset + 16, config.display.highlight_color);
        write_u32(&mut bytes, ui_offset + 20, config.display.background_color);
        bytes[ui_offset + 24] = encode_background(config.display.background);
        bytes[ui_offset + 25] = encode_fan_theme(config.display.fan_theme);
        bytes[ui_offset + 26] = u8::from(config.display.inverted);
    }
    Ok(bytes)
}

pub fn configuration_size(raw_version: u8) -> Result<usize, DeviceError> {
    match raw_version {
        0 => Ok(CONFIG_V1_SIZE),
        1 => Ok(CONFIG_V2_SIZE),
        2 => Ok(CONFIG_V3_SIZE),
        other => Err(DeviceError::Unsupported(format!(
            "configuration version {other}"
        ))),
    }
}

fn decode_name(bytes: &[u8]) -> Result<String, DeviceError> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec())
        .map_err(|_| DeviceError::Transport("configuration name is not valid UTF-8".into()))
}

fn decode_fan_mode(value: u8) -> Result<FanMode, DeviceError> {
    match value {
        0 => Ok(FanMode::Curve),
        1 => Ok(FanMode::Fixed),
        _ => unknown("fan mode", value),
    }
}

fn decode_temperature_source(value: u8) -> Result<TemperatureSource, DeviceError> {
    match value {
        0 => Ok(TemperatureSource::Input),
        1 => Ok(TemperatureSource::Output),
        2 => Ok(TemperatureSource::External1),
        3 => Ok(TemperatureSource::External2),
        4 => Ok(TemperatureSource::Maximum),
        _ => unknown("temperature source", value),
    }
}

fn decode_current_scale(value: u8) -> Result<u8, DeviceError> {
    [5, 10, 15, 20]
        .get(usize::from(value))
        .copied()
        .ok_or_else(|| DeviceError::Transport(format!("unknown current scale ordinal {value}")))
}

fn encode_current_scale(value: u8) -> Result<u8, DeviceError> {
    [5, 10, 15, 20]
        .iter()
        .position(|&candidate| candidate == value)
        .and_then(|ordinal| u8::try_from(ordinal).ok())
        .ok_or_else(|| {
            DeviceError::InvalidArgument("current scale must be 5, 10, 15, or 20".into())
        })
}

fn decode_power_scale(value: u8) -> Result<PowerScale, DeviceError> {
    match value {
        0 => Ok(PowerScale::Auto),
        1 => Ok(PowerScale::Watts300),
        2 => Ok(PowerScale::Watts600),
        _ => unknown("power scale", value),
    }
}

fn decode_rotation(value: u8) -> Result<u16, DeviceError> {
    match value {
        0 => Ok(0),
        1 => Ok(180),
        _ => unknown("display rotation", value),
    }
}

fn encode_rotation(value: u16) -> Result<u8, DeviceError> {
    match value {
        0 => Ok(0),
        180 => Ok(1),
        _ => invalid("display rotation must be 0 or 180"),
    }
}

fn decode_timeout_mode(value: u8) -> Result<TimeoutMode, DeviceError> {
    match value {
        0 => Ok(TimeoutMode::Static),
        1 => Ok(TimeoutMode::Cycle),
        2 => Ok(TimeoutMode::Sleep),
        _ => unknown("timeout mode", value),
    }
}

fn decode_screen(value: u8) -> Result<ConfigScreen, DeviceError> {
    match value {
        0 => Ok(ConfigScreen::Main),
        1 => Ok(ConfigScreen::Simple),
        2 => Ok(ConfigScreen::Current),
        3 => Ok(ConfigScreen::Temperature),
        4 => Ok(ConfigScreen::Status),
        _ => unknown("default screen", value),
    }
}

fn decode_screens(mask: u8) -> Result<Vec<ConfigScreen>, DeviceError> {
    if mask & !0x1f != 0 {
        return Err(DeviceError::Transport(format!(
            "cycle screen mask has unknown bits {:#04x}",
            mask & !0x1f
        )));
    }
    Ok((0..5)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| decode_screen(bit).expect("known screen bit"))
        .collect())
}

fn encode_screens(screens: &[ConfigScreen]) -> u8 {
    screens
        .iter()
        .fold(0, |mask, screen| mask | 1 << *screen as u8)
}

fn decode_faults(mask: u16) -> Result<Vec<FaultKind>, DeviceError> {
    if mask & !FAULT_MASK != 0 {
        return Err(DeviceError::Transport(format!(
            "fault action mask has unknown bits {:#06x}",
            mask & !FAULT_MASK
        )));
    }
    Ok((0..6)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| match bit {
            0 => FaultKind::ChipOverTemperature,
            1 => FaultKind::SensorOverTemperature,
            2 => FaultKind::OverCurrent,
            3 => FaultKind::WireOverCurrent,
            4 => FaultKind::OverPower,
            5 => FaultKind::CurrentImbalance,
            _ => unreachable!(),
        })
        .collect())
}

fn encode_faults(faults: &[FaultKind]) -> u16 {
    faults
        .iter()
        .fold(0, |mask, fault| mask | 1 << *fault as u8)
}

fn decode_averaging(value: u8) -> Result<u16, DeviceError> {
    [22, 44, 89, 177, 354, 709, 1417]
        .get(usize::from(value))
        .copied()
        .ok_or_else(|| DeviceError::Transport(format!("unknown averaging ordinal {value}")))
}

fn encode_averaging(value: u16) -> Result<u8, DeviceError> {
    [22, 44, 89, 177, 354, 709, 1417]
        .iter()
        .position(|&candidate| candidate == value)
        .and_then(|ordinal| u8::try_from(ordinal).ok())
        .ok_or_else(|| DeviceError::InvalidArgument(format!("unsupported averaging {value} ms")))
}

fn decode_background(value: u8) -> Result<Background, DeviceError> {
    match value {
        1 => Ok(Background::ThermalGrizzlyOrange),
        2 => Ok(Background::ThermalGrizzlyDark),
        255 => Ok(Background::Disabled),
        _ => unknown("background bitmap", value),
    }
}

fn encode_background(value: Background) -> u8 {
    match value {
        Background::ThermalGrizzlyOrange => 1,
        Background::ThermalGrizzlyDark => 2,
        Background::Disabled => 255,
    }
}

fn decode_fan_theme(value: u8) -> Result<FanTheme, DeviceError> {
    match value {
        100 => Ok(FanTheme::ThermalGrizzlyOrange),
        117 => Ok(FanTheme::ThermalGrizzlyDark),
        152 => Ok(FanTheme::BlackAndWhite),
        _ => unknown("fan bitmap", value),
    }
}

fn encode_fan_theme(value: FanTheme) -> u8 {
    match value {
        FanTheme::ThermalGrizzlyOrange => 100,
        FanTheme::ThermalGrizzlyDark => 117,
        FanTheme::BlackAndWhite => 152,
    }
}

fn decode_legacy_theme(value: u8) -> (Background, FanTheme, [u32; 4]) {
    let background = match value {
        0 => Background::ThermalGrizzlyOrange,
        1 => Background::ThermalGrizzlyDark,
        _ => Background::Disabled,
    };
    let (fan_theme, colors) = legacy_theme_settings(background);
    (background, fan_theme, colors)
}

fn legacy_theme_settings(background: Background) -> (FanTheme, [u32; 4]) {
    match background {
        Background::ThermalGrizzlyOrange => (
            FanTheme::ThermalGrizzlyOrange,
            [0xffff_ffff, 0xff64_6464, 0xffe6_4121, 0xff00_0000],
        ),
        Background::ThermalGrizzlyDark => (
            FanTheme::ThermalGrizzlyDark,
            [0xffff_ffff, 0xff64_6464, 0xffbe_bebe, 0xff00_0000],
        ),
        Background::Disabled => (
            FanTheme::BlackAndWhite,
            [0xff96_9696, 0xff50_5050, 0xffff_ffff, 0xff00_0000],
        ),
    }
}

fn encode_legacy_theme(background: Background) -> u8 {
    match background {
        Background::ThermalGrizzlyOrange => 0,
        Background::ThermalGrizzlyDark => 1,
        Background::Disabled => 2,
    }
}

fn encode_tenths_i16(name: &str, value: f64) -> Result<i16, DeviceError> {
    encode_scaled(name, value, f64::from(i16::MIN), f64::from(i16::MAX))
        .and_then(|scaled| i16::try_from(scaled).map_err(|_| unreachable!()))
}

fn encode_tenths_u8(name: &str, value: f64) -> Result<u8, DeviceError> {
    encode_scaled(name, value, 0.0, f64::from(u8::MAX))
        .and_then(|scaled| u8::try_from(scaled).map_err(|_| unreachable!()))
}

fn encode_scaled(name: &str, value: f64, min: f64, max: f64) -> Result<i64, DeviceError> {
    let scaled = value * 10.0;
    if !scaled.is_finite() || scaled < min || scaled > max || (scaled - scaled.round()).abs() > 1e-6
    {
        return invalid(&format!(
            "{name} must be representable with one decimal place"
        ));
    }
    Ok(scaled.round() as i64)
}

fn validate_tenths_range(
    name: &str,
    value: f64,
    minimum: f64,
    maximum: f64,
) -> Result<(), DeviceError> {
    let scaled = value * 10.0;
    if !scaled.is_finite() || (scaled - scaled.round()).abs() > 1e-6 {
        return invalid(&format!("{name} must have at most one decimal place"));
    }
    if value < minimum || value > maximum {
        return invalid(&format!(
            "{name} must be between {minimum:.1} and {maximum:.1}"
        ));
    }
    Ok(())
}

mod argb_hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if value & 0xff00_0000 == 0xff00_0000 {
            serializer.serialize_str(&format!("{:06X}", value & 0x00ff_ffff))
        } else {
            serializer.serialize_str(&format!("{value:08X}"))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if !matches!(value.len(), 6 | 8) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(D::Error::custom(
                "expected exactly 6 RGB or 8 ARGB hexadecimal characters",
            ));
        }
        let color = u32::from_str_radix(&value, 16).map_err(D::Error::custom)?;
        Ok(if value.len() == 6 {
            0xff00_0000 | color
        } else {
            color
        })
    }
}

fn percent(name: &str, value: u8) -> Result<(), DeviceError> {
    if value > 100 {
        return invalid(&format!("{name} must be between 0 and 100"));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, DeviceError> {
    Err(DeviceError::InvalidArgument(message.into()))
}

fn unknown<T>(name: &str, value: u8) -> Result<T, DeviceError> {
    Err(DeviceError::Transport(format!(
        "unknown {name} ordinal {value}"
    )))
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

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_i16(bytes: &mut [u8], offset: usize, value: i16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_json() -> serde_json::Value {
        serde_json::to_value(DeviceSettings::from_configuration(&config(2))).unwrap()
    }

    fn replace_json_field(document: &mut serde_json::Value, path: &str, value: serde_json::Value) {
        let mut current = document;
        let mut components = path.split('.').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                current
                    .as_object_mut()
                    .expect("configuration object")
                    .insert(component.into(), value);
                return;
            }
            current = current
                .get_mut(component)
                .unwrap_or_else(|| panic!("missing test field {component}"));
        }
    }

    fn assert_json_field_rejected(path: &str, value: serde_json::Value) {
        let mut document = settings_json();
        replace_json_field(&mut document, path, value);
        let json = serde_json::to_string(&document).unwrap();
        assert!(
            DeviceSettings::from_json(&json).is_err(),
            "{path} accepted an invalid JSON value"
        );
    }

    #[test]
    fn individual_item_updates_cover_every_editable_leaf_type() {
        let mut settings = DeviceSettings::from_configuration(&config(2));
        for (key, value) in [
            ("friendly_name", "CLI Device"),
            ("backlight_percent", "75"),
            ("fan.mode", "fixed"),
            ("fan.temperature_source", "input"),
            ("fan.duty_min_percent", "10"),
            ("fan.duty_max_percent", "90"),
            ("fan.temperature_min_c", "40.5"),
            ("fan.temperature_max_c", "85.5"),
            ("fault_actions.display", "over_current,over_power"),
            ("fault_actions.buzzer", "none"),
            ("fault_actions.soft_power", "over_power"),
            (
                "fault_actions.hard_power",
                "sensor_over_temperature,over_current",
            ),
            ("fault_thresholds.temperature_c", "85.5"),
            ("fault_thresholds.total_current_a", "60"),
            ("fault_thresholds.wire_current_a", "11.5"),
            ("fault_thresholds.total_power_w", "700"),
            ("fault_thresholds.current_imbalance_percent", "45"),
            ("fault_thresholds.current_imbalance_min_load_a", "7"),
            ("shutdown_wait_seconds", "15"),
            ("logging_interval_seconds", "30"),
            ("averaging_ms", "709"),
            ("display.default_screen", "simple"),
            ("display.current_scale_a", "15"),
            ("display.power_scale", "watts300"),
            ("display.rotation_degrees", "180"),
            ("display.timeout_mode", "sleep"),
            ("display.cycle_screens", "main,current,temperature,status"),
            ("display.cycle_time_seconds", "12"),
            ("display.timeout_seconds", "45"),
            ("display.primary_color", "112233"),
            ("display.secondary_color", "80112233"),
            ("display.highlight_color", "AABBCC"),
            ("display.background_color", "000000"),
            ("display.background", "thermal_grizzly_dark"),
            ("display.fan_theme", "thermal_grizzly_dark"),
            ("display.inverted", "true"),
        ] {
            settings = settings
                .with_item(key, value)
                .unwrap_or_else(|error| panic!("{key}={value} was rejected: {error}"));
        }
        settings.validate().unwrap();
        assert_eq!(settings.friendly_name, "CLI Device");
        assert!(settings.fault_actions.buzzer.is_empty());
        assert_eq!(settings.display.cycle_screens.len(), 4);
        assert!(settings.display.inverted);
    }

    #[test]
    fn grouped_item_updates_validate_cross_field_constraints_at_the_end() {
        let mut settings = DeviceSettings::from_configuration(&config(2));
        settings.fan.duty_min_percent = 20;
        settings.fan.duty_max_percent = 30;
        settings.validate().unwrap();

        assert!(settings.with_item("fan.duty_min_percent", "40").is_err());
        let updated = settings
            .with_items([
                ("fan.duty_min_percent", "40"),
                ("fan.duty_max_percent", "50"),
            ])
            .unwrap();
        assert_eq!(updated.fan.duty_min_percent, 40);
        assert_eq!(updated.fan.duty_max_percent, 50);
    }

    #[test]
    fn individual_items_are_read_as_typed_values() {
        let settings = DeviceSettings::from_configuration(&config(2));
        assert_eq!(
            settings.item("fan.mode").unwrap(),
            serde_json::json!("curve")
        );
        assert_eq!(
            settings.item("backlight_percent").unwrap(),
            serde_json::json!(100)
        );
        assert_eq!(
            settings.item("display.inverted").unwrap(),
            serde_json::json!(false)
        );
        assert!(settings.item("fault_actions.buzzer").unwrap().is_array());
        for key in ["", "unknown", "fan", "fan..mode"] {
            assert!(settings.item(key).is_err(), "{key:?} was accepted");
        }
    }

    #[test]
    fn individual_item_updates_reject_unknown_keys_wrong_types_and_bad_lists() {
        let settings = DeviceSettings::from_configuration(&config(2));
        for (key, value) in [
            ("", "1"),
            ("unknown", "1"),
            ("fan", "fixed"),
            ("fan.unknown", "fixed"),
            ("fan..mode", "fixed"),
            ("backlight_percent", "high"),
            ("backlight_percent", "101"),
            ("display.inverted", "yes"),
            ("display.inverted", "1"),
            ("display.cycle_screens", "main,,current"),
            ("display.cycle_screens", "main,unknown"),
            ("fault_actions.buzzer", "over_current,over_current"),
            ("fan.mode", "automatic"),
            ("display.primary_color", "#FFFFFF"),
        ] {
            assert!(
                settings.with_item(key, value).is_err(),
                "{key}={value} was unexpectedly accepted"
            );
        }
    }

    fn config(version: u8) -> DeviceConfiguration {
        DeviceConfiguration {
            raw_version: version,
            crc: 0x1234,
            friendly_name: "WireView".into(),
            backlight_percent: 100,
            fan: FanConfiguration {
                mode: FanMode::Curve,
                temperature_source: TemperatureSource::Maximum,
                duty_min_percent: 0,
                duty_max_percent: 100,
                temperature_min_c: 50.0,
                temperature_max_c: 80.0,
            },
            fault_actions: FaultActions {
                display: vec![FaultKind::OverCurrent, FaultKind::OverPower],
                buzzer: vec![FaultKind::OverCurrent],
                soft_power: Vec::new(),
                hard_power: vec![FaultKind::OverCurrent],
            },
            fault_thresholds: FaultThresholds {
                temperature_c: 80.0,
                total_current_a: 55,
                wire_current_a: 10.5,
                total_power_w: 660,
                current_imbalance_percent: 40,
                current_imbalance_min_load_a: 6,
            },
            shutdown_wait_seconds: 10,
            logging_interval_seconds: 60,
            averaging_ms: 1417,
            display: DisplayConfiguration {
                default_screen: ConfigScreen::Main,
                current_scale_a: 10,
                power_scale: PowerScale::Watts600,
                rotation_degrees: 0,
                timeout_mode: TimeoutMode::Static,
                cycle_screens: vec![ConfigScreen::Main, ConfigScreen::Temperature],
                cycle_time_seconds: 10,
                timeout_seconds: 30,
                primary_color: 0xffff_ffff,
                secondary_color: 0xff64_6464,
                highlight_color: 0xffe6_4121,
                background_color: 0xff00_0000,
                background: Background::ThermalGrizzlyOrange,
                fan_theme: FanTheme::ThermalGrizzlyOrange,
                inverted: false,
            },
        }
    }

    #[test]
    fn all_recovered_versions_round_trip() {
        for version in 0..=2 {
            let expected = config(version);
            let bytes = encode_configuration(&expected).unwrap();
            assert_eq!(bytes.len(), configuration_size(version).unwrap());
            let decoded = decode_configuration(version, &bytes).unwrap();
            assert_eq!(decoded, expected);
        }
    }

    #[test]
    fn recovered_offsets_and_scaling_match_protocol_layout() {
        let bytes = encode_configuration(&config(2)).unwrap();
        assert_eq!(&bytes[0..3], &[0x34, 0x12, 2]);
        assert_eq!(&bytes[3..11], b"WireView");
        assert_eq!(&bytes[40..42], &500_i16.to_le_bytes());
        assert_eq!(&bytes[54..56], &800_i16.to_le_bytes());
        assert_eq!(bytes[56], 55);
        assert_eq!(bytes[57], 105);
        assert_eq!(&bytes[58..60], &660_u16.to_le_bytes());
        assert_eq!(bytes[64], 6);
        assert_eq!(bytes[68], 0);
        assert_eq!(bytes[69], 1);
        assert_eq!(bytes[70], 2);
        assert_eq!(&bytes[76..80], &0xffff_ffff_u32.to_le_bytes());
        assert_eq!(bytes[92], 1);
        assert_eq!(bytes[93], 100);
    }

    #[test]
    fn unsafe_or_unrepresentable_values_are_rejected() {
        let mut candidate = config(2);
        candidate.backlight_percent = 101;
        assert!(matches!(
            encode_configuration(&candidate),
            Err(DeviceError::InvalidArgument(_))
        ));
        candidate = config(2);
        candidate.fault_thresholds.wire_current_a = 10.55;
        assert!(matches!(
            encode_configuration(&candidate),
            Err(DeviceError::InvalidArgument(_))
        ));
        candidate = config(2);
        candidate.display.cycle_screens.push(ConfigScreen::Main);
        assert!(matches!(
            encode_configuration(&candidate),
            Err(DeviceError::InvalidArgument(_))
        ));
    }

    #[test]
    fn every_public_field_rejects_the_wrong_json_type() {
        for (path, value) in [
            ("friendly_name", serde_json::json!(42)),
            ("backlight_percent", serde_json::json!("100")),
            ("fan.mode", serde_json::json!(0)),
            ("fan.temperature_source", serde_json::json!(false)),
            ("fan.duty_min_percent", serde_json::json!("0")),
            ("fan.duty_max_percent", serde_json::json!(100.0)),
            ("fan.temperature_min_c", serde_json::json!("50.0")),
            ("fan.temperature_max_c", serde_json::json!(true)),
            ("fault_actions.display", serde_json::json!("over_current")),
            ("fault_actions.buzzer", serde_json::json!([1])),
            ("fault_actions.soft_power", serde_json::json!(false)),
            ("fault_actions.hard_power", serde_json::json!({})),
            ("fault_thresholds.temperature_c", serde_json::json!("80.0")),
            ("fault_thresholds.total_current_a", serde_json::json!("55")),
            ("fault_thresholds.wire_current_a", serde_json::json!(true)),
            ("fault_thresholds.total_power_w", serde_json::json!("660")),
            (
                "fault_thresholds.current_imbalance_percent",
                serde_json::json!(40.0),
            ),
            (
                "fault_thresholds.current_imbalance_min_load_a",
                serde_json::json!("6"),
            ),
            ("shutdown_wait_seconds", serde_json::json!("10")),
            ("logging_interval_seconds", serde_json::json!(60.0)),
            ("averaging_ms", serde_json::json!("1417")),
            ("display.default_screen", serde_json::json!(0)),
            ("display.current_scale_a", serde_json::json!("10")),
            ("display.power_scale", serde_json::json!(600)),
            ("display.rotation_degrees", serde_json::json!("0")),
            ("display.timeout_mode", serde_json::json!(false)),
            ("display.cycle_screens", serde_json::json!("main")),
            ("display.cycle_time_seconds", serde_json::json!("10")),
            ("display.timeout_seconds", serde_json::json!(false)),
            ("display.primary_color", serde_json::json!(16777215)),
            ("display.secondary_color", serde_json::json!(-1)),
            ("display.highlight_color", serde_json::json!(true)),
            ("display.background_color", serde_json::json!({})),
            ("display.background", serde_json::json!(1)),
            ("display.fan_theme", serde_json::json!(false)),
            ("display.inverted", serde_json::json!("false")),
        ] {
            assert_json_field_rejected(path, value);
        }
    }

    #[test]
    fn every_enum_rejects_unknown_names() {
        for path in [
            "fan.mode",
            "fan.temperature_source",
            "display.default_screen",
            "display.power_scale",
            "display.timeout_mode",
            "display.background",
            "display.fan_theme",
        ] {
            assert_json_field_rejected(path, serde_json::json!("not_a_valid_option"));
        }
        assert_json_field_rejected(
            "fault_actions.display",
            serde_json::json!(["not_a_valid_fault"]),
        );
        assert_json_field_rejected(
            "display.cycle_screens",
            serde_json::json!(["not_a_valid_screen"]),
        );
    }

    #[test]
    fn rgb_and_argb_colors_round_trip_to_hardware_values() {
        let settings = DeviceSettings::from_configuration(&config(2));
        let document = serde_json::to_value(&settings).unwrap();
        assert_eq!(document["display"]["primary_color"], "FFFFFF");
        assert_eq!(document["display"]["secondary_color"], "646464");
        assert_eq!(document["display"]["highlight_color"], "E64121");
        assert_eq!(document["display"]["background_color"], "000000");

        let mut document = document;
        replace_json_field(
            &mut document,
            "display.highlight_color",
            serde_json::json!("a1b2c3"),
        );
        let parsed = DeviceSettings::from_json(&serde_json::to_string(&document).unwrap()).unwrap();
        assert_eq!(parsed.display.highlight_color, 0xffa1_b2c3);

        replace_json_field(
            &mut document,
            "display.highlight_color",
            serde_json::json!("80a1b2c3"),
        );
        let parsed = DeviceSettings::from_json(&serde_json::to_string(&document).unwrap()).unwrap();
        assert_eq!(parsed.display.highlight_color, 0x80a1_b2c3);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["display"]["highlight_color"],
            "80A1B2C3"
        );

        let hardware = parsed.with_protocol_metadata(&config(2)).unwrap();
        let encoded = encode_configuration(&hardware).unwrap();
        assert_eq!(&encoded[84..88], &0x80a1_b2c3_u32.to_le_bytes());
    }

    #[test]
    fn malformed_rgb_colors_are_rejected() {
        for value in [
            serde_json::json!(""),
            serde_json::json!("FFFFF"),
            serde_json::json!("FFFFFFF"),
            serde_json::json!("FFFFFFFFF"),
            serde_json::json!("#FFFFFF"),
            serde_json::json!("0xFFFFFF"),
            serde_json::json!("FFFFF "),
            serde_json::json!("GGGGGG"),
            serde_json::json!("FFFFFé"),
            serde_json::json!(16777215),
            serde_json::Value::Null,
        ] {
            assert_json_field_rejected("display.primary_color", value);
        }
    }

    #[test]
    fn integer_widths_unknown_fields_and_missing_fields_are_rejected() {
        for path in [
            "backlight_percent",
            "fan.duty_min_percent",
            "fault_thresholds.total_current_a",
            "fault_thresholds.current_imbalance_percent",
            "shutdown_wait_seconds",
            "logging_interval_seconds",
            "display.cycle_time_seconds",
            "display.timeout_seconds",
        ] {
            assert_json_field_rejected(path, serde_json::json!(256));
            assert_json_field_rejected(path, serde_json::json!(-1));
        }
        assert_json_field_rejected("fault_thresholds.total_power_w", serde_json::json!(65536));
        assert_json_field_rejected("averaging_ms", serde_json::json!(65536));

        for parent in ["", "fan", "fault_actions", "fault_thresholds", "display"] {
            let mut document = settings_json();
            let object = if parent.is_empty() {
                document.as_object_mut().unwrap()
            } else {
                document[parent].as_object_mut().unwrap()
            };
            object.insert("unknown_option".into(), serde_json::json!(1));
            assert!(
                DeviceSettings::from_json(&serde_json::to_string(&document).unwrap()).is_err(),
                "unknown field in {parent:?} was accepted"
            );
        }

        for (parent, field) in [
            ("", "friendly_name"),
            ("fan", "mode"),
            ("fault_actions", "display"),
            ("fault_thresholds", "temperature_c"),
            ("display", "default_screen"),
        ] {
            let mut document = settings_json();
            let object = if parent.is_empty() {
                document.as_object_mut().unwrap()
            } else {
                document[parent].as_object_mut().unwrap()
            };
            object.remove(field);
            assert!(
                DeviceSettings::from_json(&serde_json::to_string(&document).unwrap()).is_err(),
                "missing field {parent}.{field} was accepted"
            );
        }
    }

    #[test]
    fn all_numeric_ranges_precision_and_cross_field_rules_are_enforced() {
        let mut candidate = DeviceSettings::from_configuration(&config(2));
        candidate.friendly_name = "x".repeat(33);
        assert!(candidate.validate().is_err());
        candidate = DeviceSettings::from_configuration(&config(2));
        candidate.friendly_name = "x".repeat(32);
        assert!(candidate.validate().is_ok());
        candidate.friendly_name = "bad\nname".into();
        assert!(candidate.validate().is_err());
        candidate.friendly_name = "WireView 🔌".into();
        assert!(candidate.validate().is_err());

        for (path, value) in [
            ("backlight_percent", serde_json::json!(101)),
            ("fan.duty_min_percent", serde_json::json!(101)),
            ("fan.duty_max_percent", serde_json::json!(101)),
            (
                "fault_thresholds.current_imbalance_percent",
                serde_json::json!(101),
            ),
            ("fault_thresholds.total_current_a", serde_json::json!(151)),
            ("fault_thresholds.total_power_w", serde_json::json!(2001)),
            (
                "fault_thresholds.current_imbalance_min_load_a",
                serde_json::json!(11),
            ),
        ] {
            assert_json_field_rejected(path, value);
        }

        candidate = DeviceSettings::from_configuration(&config(2));
        candidate.fan.duty_min_percent = 51;
        candidate.fan.duty_max_percent = 50;
        assert!(candidate.validate().is_err());

        for (path, value) in [
            ("fan.temperature_min_c", serde_json::json!(-3276.9)),
            ("fan.temperature_max_c", serde_json::json!(3276.8)),
            ("fault_thresholds.wire_current_a", serde_json::json!(-0.1)),
            ("fault_thresholds.wire_current_a", serde_json::json!(25.6)),
            ("fan.temperature_min_c", serde_json::json!(20.01)),
            ("fault_thresholds.temperature_c", serde_json::json!(80.01)),
            ("fault_thresholds.wire_current_a", serde_json::json!(10.55)),
        ] {
            assert_json_field_rejected(path, value);
        }

        candidate = DeviceSettings::from_configuration(&config(2));
        candidate.fan.temperature_min_c = 81.0;
        candidate.fan.temperature_max_c = 80.0;
        assert!(candidate.validate().is_err());

        for faults in [
            &mut candidate.fault_actions.display,
            &mut candidate.fault_actions.buzzer,
            &mut candidate.fault_actions.soft_power,
            &mut candidate.fault_actions.hard_power,
        ] {
            *faults = vec![FaultKind::OverCurrent, FaultKind::OverCurrent];
        }
        assert!(candidate.validate().is_err());

        for (path, value) in [
            ("averaging_ms", serde_json::json!(100)),
            ("display.current_scale_a", serde_json::json!(12)),
            ("display.rotation_degrees", serde_json::json!(90)),
        ] {
            assert_json_field_rejected(path, value);
        }
        candidate = DeviceSettings::from_configuration(&config(2));
        candidate.display.cycle_screens = vec![ConfigScreen::Main, ConfigScreen::Main];
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn fan_curve_temperatures_enforce_operating_boundaries() {
        let mut candidate = DeviceSettings::from_configuration(&config(2));
        candidate.fan.temperature_min_c = 0.0;
        candidate.fan.temperature_max_c = 50.0;
        assert!(candidate.validate().is_ok());

        candidate.fan.temperature_min_c = 50.0;
        candidate.fan.temperature_max_c = 100.0;
        assert!(candidate.validate().is_ok());

        for (path, value) in [
            ("fan.temperature_min_c", serde_json::json!(-0.1)),
            ("fan.temperature_min_c", serde_json::json!(50.1)),
            ("fan.temperature_max_c", serde_json::json!(49.9)),
            ("fan.temperature_max_c", serde_json::json!(100.1)),
        ] {
            assert_json_field_rejected(path, value);
        }
    }

    #[test]
    fn fault_temperature_threshold_enforces_operating_boundaries() {
        for value in [0.0, 80.0, 120.0] {
            let mut candidate = DeviceSettings::from_configuration(&config(2));
            candidate.fault_thresholds.temperature_c = value;
            assert!(candidate.validate().is_ok(), "{value} °C was rejected");
        }

        for value in [-0.1, 120.1, 80.01] {
            assert_json_field_rejected("fault_thresholds.temperature_c", serde_json::json!(value));
        }
    }

    #[test]
    fn electrical_fault_thresholds_enforce_safe_boundaries() {
        let mut candidate = DeviceSettings::from_configuration(&config(2));
        candidate.fault_thresholds.total_current_a = 150;
        candidate.fault_thresholds.wire_current_a = 25.5;
        candidate.fault_thresholds.total_power_w = 2000;
        candidate.fault_thresholds.current_imbalance_percent = 100;
        candidate.fault_thresholds.current_imbalance_min_load_a = 10;
        assert!(candidate.validate().is_ok());

        for (path, value) in [
            ("fault_thresholds.total_current_a", serde_json::json!(151)),
            ("fault_thresholds.wire_current_a", serde_json::json!(25.6)),
            ("fault_thresholds.total_power_w", serde_json::json!(2001)),
            (
                "fault_thresholds.current_imbalance_percent",
                serde_json::json!(101),
            ),
            (
                "fault_thresholds.current_imbalance_min_load_a",
                serde_json::json!(11),
            ),
        ] {
            assert_json_field_rejected(path, value);
        }
    }

    #[test]
    fn legacy_versions_reject_fields_that_would_be_silently_discarded() {
        let mut candidate = config(0);
        candidate.averaging_ms = 709;
        assert!(candidate.validate().is_err());

        candidate = config(1);
        candidate.display.default_screen = ConfigScreen::Status;
        assert!(candidate.validate().is_err());

        candidate = config(1);
        candidate.display.inverted = true;
        assert!(candidate.validate().is_err());

        candidate = config(1);
        candidate.display.fan_theme = FanTheme::BlackAndWhite;
        assert!(candidate.validate().is_err());

        candidate = config(1);
        candidate.display.primary_color = 0;
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn editable_settings_hide_and_reject_protocol_metadata() {
        let configuration = DeviceConfiguration::mock();
        let settings = DeviceSettings::from_configuration(&configuration);
        let json = serde_json::to_value(&settings).unwrap();
        assert!(json.get("raw_version").is_none());
        assert!(json.get("crc").is_none());

        let mut json = json;
        json.as_object_mut()
            .unwrap()
            .insert("crc".into(), serde_json::json!(configuration.crc));
        assert!(serde_json::from_value::<DeviceSettings>(json).is_err());

        let restored = settings.with_protocol_metadata(&configuration).unwrap();
        assert_eq!(restored, configuration);
    }
}
