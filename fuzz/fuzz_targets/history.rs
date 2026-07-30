#![no_main]

use libfuzzer_sys::fuzz_target;
use wireviewd::history::{FLASH_SECTOR_SIZE, history_end_found, parse_history, visit_history};

fuzz_target!(|data: &[u8]| {
    let _ = parse_history(data);
    let _ = history_end_found(data);
    let _ = visit_history(data, |_| Ok::<_, std::convert::Infallible>(()));

    // Exercise complete-sector traversal from the first mutation instead of
    // waiting for the fuzzer to grow an input to 4096 bytes.
    let mut sector = [0xff_u8; FLASH_SECTOR_SIZE];
    let copied = data.len().min(sector.len());
    sector[..copied].copy_from_slice(&data[..copied]);
    if copied < 21 {
        sector[copied..21].fill(0);
    }
    sector[0] &= !1;
    sector[20] %= 4;
    let _ = visit_history(&sector, |_| Ok::<_, std::convert::Infallible>(()));
});
