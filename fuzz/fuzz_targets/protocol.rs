#![no_main]

use libfuzzer_sys::fuzz_target;
use wireviewd::protocol::{
    BUILD_RESPONSE_SIZE, SENSOR_RESPONSE_SIZE, UsbCommand, decode_build_response, decode_faults,
    decode_sensor_response,
};

fuzz_target!(|data: &[u8]| {
    let _ = decode_sensor_response(data);
    let _ = decode_build_response(data);

    let mut sensors = [0_u8; SENSOR_RESPONSE_SIZE];
    let sensor_bytes = data.len().min(sensors.len());
    sensors[..sensor_bytes].copy_from_slice(&data[..sensor_bytes]);
    sensors[94] %= 4;
    let _ = decode_sensor_response(&sensors);

    let mut build = [0_u8; BUILD_RESPONSE_SIZE];
    let build_bytes = data.len().min(build.len());
    build[..build_bytes].copy_from_slice(&data[..build_bytes]);
    let _ = decode_build_response(&build);

    if let Some(&command) = data.first() {
        let _ = UsbCommand::try_from(command);
    }
    if let Some(mask) = data
        .get(..2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    {
        let _ = decode_faults(mask);
    }
});
