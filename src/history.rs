//! Decoder for telemetry history stored in the device's SPI flash.

use serde::{Deserialize, Serialize};

use crate::domain::{PinMetrics, Temperatures};

pub const FLASH_START_ADDRESS: u32 = 0x0080_0000;
pub const FLASH_LENGTH: usize = 0x0080_0000;
pub const FLASH_READ_PAGE_SIZE: usize = 256;
pub const FLASH_SECTOR_SIZE: usize = 4096;
pub const ENTRY_SIZE: usize = 21;
pub const MAX_CHUNK_SIZE: usize = 64 * 1024;

const EMPTY_DATA: u32 = u32::MAX;
const EMPTY_RUN_END: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntryKind {
    Measurement,
    PowerOn,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub kind: HistoryEntryKind,
    /// Wrapping MCU tick counter when this entry was recorded.
    pub device_time_ms: u32,
    pub metrics: HistoryMetrics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryMetrics {
    pub avg_voltage_v: f64,
    pub total_current_a: f64,
    pub total_power_w: f64,
    pub cable_capability_w: u16,
    pub pins: Vec<PinMetrics>,
    pub temperatures: Temperatures,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedHistory {
    pub entries: Vec<HistoryEntry>,
    /// True when the parser found the erased-entry run that marks the logical
    /// end of recorded history.
    pub end_found: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistorySummary {
    pub entries: usize,
    pub end_found: bool,
}

#[must_use]
pub fn parse_history(data: &[u8]) -> ParsedHistory {
    let mut entries = Vec::new();
    let summary = visit_history(data, |entry| {
        entries.push(entry);
        Ok::<_, std::convert::Infallible>(())
    })
    .expect("an infallible history visitor cannot fail");
    ParsedHistory {
        entries,
        end_found: summary.end_found,
    }
}

#[must_use]
pub fn history_end_found(data: &[u8]) -> bool {
    visit_history(data, |_| Ok::<_, std::convert::Infallible>(()))
        .expect("an infallible history visitor cannot fail")
        .end_found
}

/// Decodes one record at a time without retaining the expanded entries.
///
/// The visitor can return an error to stop traversal immediately, which lets
/// CLI exporters propagate I/O errors and cancellation without allocating a
/// complete decoded log.
pub fn visit_history<E>(
    data: &[u8],
    mut visitor: impl FnMut(HistoryEntry) -> Result<(), E>,
) -> Result<HistorySummary, E> {
    let mut entries = 0;
    let end_found = walk_history(data, |record, encoded, kind| {
        visitor(decode_entry(record, encoded, kind))?;
        entries += 1;
        Ok(())
    })?;
    Ok(HistorySummary { entries, end_found })
}

fn walk_history<E>(
    data: &[u8],
    mut on_entry: impl FnMut(&[u8], u32, HistoryEntryKind) -> Result<(), E>,
) -> Result<bool, E> {
    let sectors = data.len() / FLASH_SECTOR_SIZE;
    let complete_length = sectors * FLASH_SECTOR_SIZE;
    let entries_per_sector = FLASH_SECTOR_SIZE / ENTRY_SIZE;
    let mut found_first = false;
    let mut empty_run = 0;

    for sector in 0..sectors {
        let sector_start = sector * FLASH_SECTOR_SIZE;
        let mut slot = 0;
        while slot < entries_per_sector {
            let mut offset = sector_start + slot * ENTRY_SIZE;
            if offset + ENTRY_SIZE > complete_length {
                break;
            }

            // Once valid data begins, the firmware pads entries that would
            // cross a 256-byte flash page.
            if found_first && offset & 0xff > FLASH_READ_PAGE_SIZE - ENTRY_SIZE {
                offset += FLASH_READ_PAGE_SIZE - (offset & 0xff);
                let remainder = offset % ENTRY_SIZE;
                if remainder != 0 {
                    offset += ENTRY_SIZE - remainder;
                }
                let next_slot = (offset - sector_start) / ENTRY_SIZE;
                slot = next_slot.max(slot + 1);
                continue;
            }

            let record = &data[offset..offset + ENTRY_SIZE];
            let encoded = read_u32(record, 0);
            if encoded == EMPTY_DATA {
                if found_first {
                    empty_run += 1;
                    if empty_run >= EMPTY_RUN_END {
                        return Ok(true);
                    }
                }
                slot += 1;
                continue;
            }

            empty_run = 0;
            match encoded & 0b11 {
                0 => {
                    if record[20] > 3 {
                        slot += 1;
                        continue;
                    }

                    // A structurally valid measurement starts the recorded
                    // region even when its voltages reveal a partial/corrupt
                    // write. This matches the device parser's traversal rules.
                    found_first = true;
                    let voltage_sum: usize = record[8..14]
                        .iter()
                        .map(|&voltage| usize::from(voltage))
                        .sum();
                    if voltage_sum > 60 && voltage_sum < 900 {
                        on_entry(record, encoded, HistoryEntryKind::Measurement)?;
                    }
                    slot += 1;
                }
                2 => {
                    // POWER_ON is a boundary marker. Its sensor payload is not
                    // a measurement and must never be decoded as one.
                    found_first = true;
                    slot += 1;
                }
                _ => {
                    slot += 1;
                }
            }
        }
    }

    Ok(complete_length == FLASH_LENGTH)
}

fn decode_entry(record: &[u8], encoded: u32, kind: HistoryEntryKind) -> HistoryEntry {
    let voltages = &record[8..14];
    let currents = &record[14..20];
    let pins = voltages
        .iter()
        .zip(currents)
        .map(|(&voltage, &current)| {
            let voltage_v = f64::from(voltage) / 10.0;
            let current_a = f64::from(current) / 10.0;
            PinMetrics {
                voltage_v,
                current_a,
                power_w: voltage_v * current_a,
            }
        })
        .collect::<Vec<_>>();
    let total_current_a = pins.iter().map(|pin| pin.current_a).sum();
    let total_power_w = pins.iter().map(|pin| pin.power_w).sum();
    let avg_voltage_v = if pins.is_empty() {
        0.0
    } else {
        pins.iter().map(|pin| pin.voltage_v).sum::<f64>() / pins.len() as f64
    };

    HistoryEntry {
        kind,
        device_time_ms: (encoded >> 2) & 0x3fff_ffff,
        metrics: HistoryMetrics {
            avg_voltage_v,
            total_current_a,
            total_power_w,
            cable_capability_w: match record[20] {
                0 => 600,
                1 => 450,
                2 => 300,
                3 => 150,
                _ => unreachable!("cable capability is validated before decoding"),
            },
            pins,
            temperatures: Temperatures {
                input_c: f64::from(record[4] as i8),
                output_c: f64::from(record[5] as i8),
                external_1_c: decode_temperature(record[6]),
                external_2_c: decode_temperature(record[7]),
            },
        },
    }
}

fn decode_temperature(value: u8) -> Option<f64> {
    let value = value as i8;
    (value != 0 && value > -100).then_some(f64::from(value))
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

    fn entry(kind: u32, milliseconds: u32) -> [u8; ENTRY_SIZE] {
        let mut entry = [0_u8; ENTRY_SIZE];
        entry[..4].copy_from_slice(&((milliseconds << 2) | kind).to_le_bytes());
        entry[4..8].copy_from_slice(&[39, 34, (-100_i8) as u8, 25]);
        entry[8..14].copy_from_slice(&[121, 120, 119, 118, 117, 116]);
        entry[14..20].copy_from_slice(&[3, 4, 5, 6, 7, 8]);
        entry[20] = 0;
        entry
    }

    #[test]
    fn decodes_compact_measurements_and_scaling() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        data[..ENTRY_SIZE].copy_from_slice(&entry(0, 42));
        let parsed = parse_history(&data);
        assert!(parsed.end_found);
        assert_eq!(parsed.entries.len(), 1);
        let first = &parsed.entries[0];
        assert_eq!(first.kind, HistoryEntryKind::Measurement);
        assert_eq!(first.device_time_ms, 42);
        assert_eq!(first.metrics.temperatures.input_c, 39.0);
        assert_eq!(first.metrics.temperatures.external_1_c, None);
        assert_eq!(first.metrics.temperatures.external_2_c, Some(25.0));
        assert_eq!(first.metrics.pins[0].voltage_v, 12.1);
        assert_eq!(first.metrics.pins[0].current_a, 0.3);
        assert_eq!(first.metrics.cable_capability_w, 600);

        let mut visited = Vec::new();
        let summary = visit_history(&data, |entry| {
            visited.push(entry);
            Ok::<_, std::convert::Infallible>(())
        })
        .unwrap();
        assert_eq!(summary.entries, 1);
        assert!(summary.end_found);
        assert_eq!(visited, parsed.entries);
    }

    #[test]
    fn power_on_marks_the_log_start_but_is_not_a_measurement() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        data[..ENTRY_SIZE].copy_from_slice(&entry(1, 1));
        let mut power_on = entry(2, 2);
        power_on[8..].fill(0);
        power_on[20] = u8::MAX;
        data[ENTRY_SIZE..ENTRY_SIZE * 2].copy_from_slice(&power_on);
        let parsed = parse_history(&data);
        assert!(parsed.entries.is_empty());
        assert!(parsed.end_found);
    }

    #[test]
    fn rejects_unknown_cable_capability() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        let mut accepted = entry(0, 1);
        accepted[20] = 3;
        data[..ENTRY_SIZE].copy_from_slice(&accepted);
        let mut rejected = entry(0, 2);
        rejected[20] = 4;
        data[ENTRY_SIZE..ENTRY_SIZE * 2].copy_from_slice(&rejected);

        let parsed = parse_history(&data);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].device_time_ms, 1);
        assert_eq!(parsed.entries[0].kind, HistoryEntryKind::Measurement);
    }

    #[test]
    fn voltage_plausibility_bounds_are_strict() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        for (index, sum) in [60_u16, 61, 899, 900].into_iter().enumerate() {
            let mut record = entry(0, u32::try_from(index).unwrap());
            record[8..14].fill(0);
            record[8] = u8::try_from(sum.min(255)).unwrap();
            record[9] = u8::try_from(sum.saturating_sub(255).min(255)).unwrap();
            record[10] = u8::try_from(sum.saturating_sub(510).min(255)).unwrap();
            record[11] = u8::try_from(sum.saturating_sub(765).min(255)).unwrap();
            data[index * ENTRY_SIZE..(index + 1) * ENTRY_SIZE].copy_from_slice(&record);
        }

        let parsed = parse_history(&data);
        assert_eq!(
            parsed
                .entries
                .iter()
                .map(|entry| entry.device_time_ms)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(
            parsed
                .entries
                .iter()
                .all(|entry| entry.kind == HistoryEntryKind::Measurement)
        );
    }

    #[test]
    fn rejected_voltage_sample_still_enables_erased_run_termination() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        let mut rejected = entry(0, 1);
        rejected[8..14].fill(10);
        data[..ENTRY_SIZE].copy_from_slice(&rejected);

        let parsed = parse_history(&data);
        assert!(parsed.entries.is_empty());
        assert!(parsed.end_found);
    }

    #[test]
    fn zero_external_temperature_is_a_disconnected_probe() {
        let mut data = vec![0xff; FLASH_SECTOR_SIZE];
        let mut record = entry(0, 1);
        record[6] = 0;
        record[7] = 0;
        data[..ENTRY_SIZE].copy_from_slice(&record);
        let parsed = parse_history(&data);
        assert_eq!(parsed.entries[0].metrics.temperatures.external_1_c, None);
        assert_eq!(parsed.entries[0].metrics.temperatures.external_2_c, None);
    }

    #[test]
    fn incomplete_flash_without_erased_terminator_is_not_complete() {
        let mut data = vec![0; FLASH_SECTOR_SIZE];
        data[..ENTRY_SIZE].copy_from_slice(&entry(0, 1));
        let parsed = parse_history(&data);
        assert!(!parsed.end_found);
    }
}
