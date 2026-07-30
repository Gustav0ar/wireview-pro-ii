#![no_main]

use libfuzzer_sys::fuzz_target;
use wireviewd::config::{
    CONFIG_V1_SIZE, CONFIG_V2_SIZE, CONFIG_V3_SIZE, DeviceSettings, decode_configuration,
};

fuzz_target!(|data: &[u8]| {
    if let Some((&version_seed, bytes)) = data.split_first() {
        let _ = decode_configuration(version_seed % 3, bytes);
    }
    for (version, size) in [
        (0, CONFIG_V1_SIZE),
        (1, CONFIG_V2_SIZE),
        (2, CONFIG_V3_SIZE),
    ] {
        let mut configuration = vec![0_u8; size];
        let copied = data.len().min(size);
        configuration[..copied].copy_from_slice(&data[..copied]);
        let _ = decode_configuration(version, &configuration);
    }
    if let Ok(json) = std::str::from_utf8(data) {
        let _ = DeviceSettings::from_json(json);
    }
});
