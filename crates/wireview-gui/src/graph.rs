use std::collections::VecDeque;
use std::fmt::Write as _;

use crate::client::TelemetrySnapshot;

pub(crate) const SAMPLE_LIMIT: usize = 1_200;
const BUCKET_LIMIT: usize = 384;
const PATH_COORDINATE_MAX: f32 = 1_000.0;

const POWER_LABELS: [&str; 6] = ["TOTAL", "", "", "", "", ""];
const CURRENT_LABELS: [&str; 6] = ["I1", "I2", "I3", "I4", "I5", "I6"];
const VOLTAGE_LABELS: [&str; 6] = ["V1", "V2", "V3", "V4", "V5", "V6"];
const TEMPERATURE_LABELS: [&str; 6] = ["INPUT", "OUTPUT", "EXT 1", "EXT 2", "", ""];

pub(crate) type SharedGraphState = std::sync::Arc<std::sync::Mutex<GraphState>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GraphKind {
    Power,
    Current,
    Voltage,
    Temperature,
}

impl GraphKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "power" => Some(Self::Power),
            "current" => Some(Self::Current),
            "voltage" => Some(Self::Voltage),
            "temperature" => Some(Self::Temperature),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Power => 0,
            Self::Current => 1,
            Self::Voltage => 2,
            Self::Temperature => 3,
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Power => "power",
            Self::Current => "current",
            Self::Voltage => "voltage",
            Self::Temperature => "temperature",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Power => "POWER FIELD",
            Self::Current => "CURRENT FIELD",
            Self::Voltage => "VOLTAGE FIELD",
            Self::Temperature => "THERMAL FIELD",
        }
    }

    const fn unit(self) -> &'static str {
        match self {
            Self::Power => "W",
            Self::Current => "A",
            Self::Voltage => "V",
            Self::Temperature => "C",
        }
    }

    const fn labels(self) -> &'static [&'static str; 6] {
        match self {
            Self::Power => &POWER_LABELS,
            Self::Current => &CURRENT_LABELS,
            Self::Voltage => &VOLTAGE_LABELS,
            Self::Temperature => &TEMPERATURE_LABELS,
        }
    }

    const fn series_count(self) -> usize {
        match self {
            Self::Power => 1,
            Self::Current | Self::Voltage => 6,
            Self::Temperature => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GraphWindow {
    Minute,
    FiveMinutes,
    TenMinutes,
}

impl GraphWindow {
    const fn from_seconds(seconds: u64) -> Option<Self> {
        match seconds {
            60 => Some(Self::Minute),
            300 => Some(Self::FiveMinutes),
            600 => Some(Self::TenMinutes),
            _ => None,
        }
    }

    const fn seconds(self) -> u64 {
        match self {
            Self::Minute => 60,
            Self::FiveMinutes => 300,
            Self::TenMinutes => 600,
        }
    }

    const fn axis_label(self) -> &'static str {
        match self {
            Self::Minute => "-60 S",
            Self::FiveMinutes => "-5 M",
            Self::TenMinutes => "-10 M",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GraphSample {
    observed_at_ms: u64,
    total_power: f32,
    total_current: f32,
    average_voltage: f32,
    currents: [f32; 6],
    voltages: [f32; 6],
    temperatures: [f32; 4],
    temperature_mask: u8,
}

impl From<&TelemetrySnapshot> for GraphSample {
    fn from(telemetry: &TelemetrySnapshot) -> Self {
        let mut temperature_mask = 0b0011;
        if telemetry.external_1_temperature.is_some() {
            temperature_mask |= 0b0100;
        }
        if telemetry.external_2_temperature.is_some() {
            temperature_mask |= 0b1000;
        }
        Self {
            observed_at_ms: telemetry.observed_at_ms,
            total_power: telemetry.total_power as f32,
            total_current: telemetry.total_current as f32,
            average_voltage: telemetry.average_voltage as f32,
            currents: std::array::from_fn(|index| telemetry.pins[index].current as f32),
            voltages: std::array::from_fn(|index| telemetry.pins[index].voltage as f32),
            temperatures: [
                telemetry.input_temperature as f32,
                telemetry.output_temperature as f32,
                telemetry.external_1_temperature.unwrap_or_default() as f32,
                telemetry.external_2_temperature.unwrap_or_default() as f32,
            ],
            temperature_mask,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecordOutcome {
    pub(crate) appended: bool,
    pub(crate) session_reset: bool,
}

#[derive(Debug)]
pub(crate) struct GraphMetadata {
    pub(crate) kind: &'static str,
    pub(crate) title: &'static str,
    pub(crate) labels: [&'static str; 6],
    pub(crate) visible: [bool; 6],
    pub(crate) series_count: usize,
    pub(crate) window_seconds: u64,
    pub(crate) window_axis_label: &'static str,
    pub(crate) paused: bool,
}

#[derive(Debug)]
pub(crate) struct GraphFrame {
    pub(crate) paths: [String; 6],
    pub(crate) values: [String; 6],
    pub(crate) range_label: String,
    pub(crate) y_max_label: String,
    pub(crate) y_mid_label: String,
    pub(crate) y_min_label: String,
    pub(crate) summary_name: &'static str,
    pub(crate) summary_value: String,
    pub(crate) sample_count: usize,
    pub(crate) buffer_count: usize,
}

#[derive(Debug)]
pub(crate) struct OverviewFrame {
    pub(crate) path: String,
    pub(crate) range_label: String,
    pub(crate) sample_count: usize,
}

#[derive(Debug)]
pub(crate) struct GraphState {
    samples: VecDeque<GraphSample>,
    session_id: Option<u64>,
    last_sequence: Option<u64>,
    last_observed_at_ms: Option<u64>,
    kind: GraphKind,
    window: GraphWindow,
    paused: bool,
    visible_masks: [u8; 4],
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(SAMPLE_LIMIT),
            session_id: None,
            last_sequence: None,
            last_observed_at_ms: None,
            kind: GraphKind::Current,
            window: GraphWindow::Minute,
            paused: false,
            visible_masks: [0b0001, 0b11_1111, 0b11_1111, 0b0011],
        }
    }
}

impl GraphState {
    pub(crate) fn shared() -> SharedGraphState {
        std::sync::Arc::new(std::sync::Mutex::new(Self::default()))
    }

    pub(crate) fn record(&mut self, telemetry: &TelemetrySnapshot) -> RecordOutcome {
        let session_reset = self
            .session_id
            .is_some_and(|session_id| session_id != telemetry.session_id);
        if session_reset {
            self.reset_session();
        }

        let duplicate = self.session_id == Some(telemetry.session_id)
            && self
                .last_sequence
                .is_some_and(|sequence| telemetry.sequence <= sequence);
        if duplicate {
            return RecordOutcome {
                appended: false,
                session_reset: false,
            };
        }

        self.session_id = Some(telemetry.session_id);
        self.last_sequence = Some(telemetry.sequence);
        let observed_at_ms = self
            .last_observed_at_ms
            .map_or(telemetry.observed_at_ms, |observed| {
                telemetry.observed_at_ms.max(observed.saturating_add(1))
            });
        self.last_observed_at_ms = Some(observed_at_ms);
        if self.samples.len() == SAMPLE_LIMIT {
            self.samples.pop_front();
        }
        let mut sample = GraphSample::from(telemetry);
        sample.observed_at_ms = observed_at_ms;
        self.samples.push_back(sample);
        RecordOutcome {
            appended: true,
            session_reset,
        }
    }

    pub(crate) fn reset_session(&mut self) {
        self.samples.clear();
        self.session_id = None;
        self.last_sequence = None;
        self.last_observed_at_ms = None;
    }

    pub(crate) fn clear_history(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn select_kind(&mut self, kind: GraphKind) {
        self.kind = kind;
    }

    pub(crate) fn set_window_seconds(&mut self, seconds: u64) {
        if let Some(window) = GraphWindow::from_seconds(seconds) {
            self.window = window;
        }
    }

    pub(crate) fn toggle_paused(&mut self) {
        self.paused = !self.paused;
    }

    pub(crate) const fn paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn toggle_series(&mut self, index: usize) {
        if index < self.kind.series_count() {
            self.visible_masks[self.kind.index()] ^= 1 << index;
        }
    }

    pub(crate) fn metadata(&self) -> GraphMetadata {
        let mask = self.visible_masks[self.kind.index()];
        GraphMetadata {
            kind: self.kind.id(),
            title: self.kind.title(),
            labels: *self.kind.labels(),
            visible: std::array::from_fn(|index| mask & (1 << index) != 0),
            series_count: self.kind.series_count(),
            window_seconds: self.window.seconds(),
            window_axis_label: self.window.axis_label(),
            paused: self.paused,
        }
    }

    pub(crate) fn frame(&self) -> GraphFrame {
        self.frame_for(self.kind, self.window)
    }

    pub(crate) fn overview_frame(&self) -> OverviewFrame {
        let Some(latest) = self.samples.back() else {
            return OverviewFrame {
                path: String::new(),
                range_label: "WAITING FOR LIVE SAMPLES".into(),
                sample_count: 0,
            };
        };
        let window_ms = GraphWindow::Minute.seconds() * 1_000;
        let start_ms = latest.observed_at_ms.saturating_sub(window_ms);
        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        let mut sample_count = 0;
        for sample in self
            .samples
            .iter()
            .filter(|sample| sample.observed_at_ms >= start_ms)
        {
            sample_count += 1;
            minimum = minimum.min(sample.total_power);
            maximum = maximum.max(sample.total_power);
        }
        if !minimum.is_finite() || !maximum.is_finite() {
            return OverviewFrame {
                path: String::new(),
                range_label: "WAITING FOR LIVE SAMPLES".into(),
                sample_count,
            };
        }
        let (axis_minimum, axis_maximum) = axis_bounds(GraphKind::Power, minimum, maximum);
        OverviewFrame {
            path: if sample_count >= 2 {
                build_path(
                    self.samples.iter(),
                    GraphKind::Power,
                    0,
                    start_ms,
                    latest.observed_at_ms,
                    axis_minimum,
                    axis_maximum,
                )
            } else {
                String::new()
            },
            range_label: format!(
                "AUTO / {} TO {} W",
                format_number(GraphKind::Power, axis_minimum),
                format_number(GraphKind::Power, axis_maximum)
            ),
            sample_count,
        }
    }

    fn frame_for(&self, kind: GraphKind, window: GraphWindow) -> GraphFrame {
        let empty_paths = std::array::from_fn(|_| String::new());
        let empty_values = std::array::from_fn(|_| String::new());
        let Some(latest) = self.samples.back() else {
            return GraphFrame {
                paths: empty_paths,
                values: empty_values,
                range_label: "WAITING FOR LIVE SAMPLES".into(),
                y_max_label: "--".into(),
                y_mid_label: "--".into(),
                y_min_label: "--".into(),
                summary_name: summary_name(kind),
                summary_value: format_missing(kind),
                sample_count: 0,
                buffer_count: 0,
            };
        };

        let window_ms = window.seconds() * 1_000;
        let start_ms = latest.observed_at_ms.saturating_sub(window_ms);
        let sample_count = self
            .samples
            .iter()
            .filter(|sample| sample.observed_at_ms >= start_ms)
            .count();
        let visibility = if kind == self.kind {
            self.visible_masks[kind.index()]
        } else {
            0b0001
        };

        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        for sample in self
            .samples
            .iter()
            .filter(|sample| sample.observed_at_ms >= start_ms)
        {
            for series in 0..kind.series_count() {
                if visibility & (1 << series) == 0 {
                    continue;
                }
                if let Some(value) = sample.value(kind, series) {
                    minimum = minimum.min(value);
                    maximum = maximum.max(value);
                }
            }
        }

        let mut paths = empty_paths;
        let mut range_label = "SELECT A SERIES".to_owned();
        let mut y_max_label = "--".to_owned();
        let mut y_mid_label = "--".to_owned();
        let mut y_min_label = "--".to_owned();
        if minimum.is_finite() && maximum.is_finite() {
            let (axis_minimum, axis_maximum) = axis_bounds(kind, minimum, maximum);
            let midpoint = (axis_minimum + axis_maximum) / 2.0;
            range_label = format!(
                "AUTO / {} TO {} {}",
                format_number(kind, axis_minimum),
                format_number(kind, axis_maximum),
                kind.unit()
            );
            y_max_label = format_number(kind, axis_maximum);
            y_mid_label = format_number(kind, midpoint);
            y_min_label = format_number(kind, axis_minimum);
            if sample_count >= 2 {
                for (series, path) in paths.iter_mut().enumerate().take(kind.series_count()) {
                    if visibility & (1 << series) != 0 {
                        *path = build_path(
                            self.samples.iter(),
                            kind,
                            series,
                            start_ms,
                            latest.observed_at_ms,
                            axis_minimum,
                            axis_maximum,
                        );
                    }
                }
            }
        }

        let values = std::array::from_fn(|series| {
            if series < kind.series_count() {
                latest
                    .value(kind, series)
                    .map_or_else(|| "N/A".into(), |value| format_value(kind, value))
            } else {
                String::new()
            }
        });
        GraphFrame {
            paths,
            values,
            range_label,
            y_max_label,
            y_mid_label,
            y_min_label,
            summary_name: summary_name(kind),
            summary_value: summary_value(kind, latest),
            sample_count,
            buffer_count: self.samples.len(),
        }
    }
}

impl GraphSample {
    fn value(self, kind: GraphKind, series: usize) -> Option<f32> {
        match kind {
            GraphKind::Power => (series == 0).then_some(self.total_power),
            GraphKind::Current => self.currents.get(series).copied(),
            GraphKind::Voltage => self.voltages.get(series).copied(),
            GraphKind::Temperature => self
                .temperatures
                .get(series)
                .copied()
                .filter(|_| self.temperature_mask & (1 << series) != 0),
        }
    }
}

#[derive(Clone, Copy)]
struct Bucket {
    populated: bool,
    minimum_time: u64,
    minimum: f32,
    maximum_time: u64,
    maximum: f32,
}

impl Bucket {
    const EMPTY: Self = Self {
        populated: false,
        minimum_time: 0,
        minimum: 0.0,
        maximum_time: 0,
        maximum: 0.0,
    };

    fn add(&mut self, observed_at_ms: u64, value: f32) {
        if !self.populated {
            self.populated = true;
            self.minimum_time = observed_at_ms;
            self.minimum = value;
            self.maximum_time = observed_at_ms;
            self.maximum = value;
            return;
        }
        if value < self.minimum {
            self.minimum_time = observed_at_ms;
            self.minimum = value;
        }
        if value > self.maximum {
            self.maximum_time = observed_at_ms;
            self.maximum = value;
        }
    }
}

fn build_path<'a>(
    samples: impl Iterator<Item = &'a GraphSample>,
    kind: GraphKind,
    series: usize,
    start_ms: u64,
    end_ms: u64,
    axis_minimum: f32,
    axis_maximum: f32,
) -> String {
    let duration_ms = end_ms.saturating_sub(start_ms).max(1);
    let mut buckets = [Bucket::EMPTY; BUCKET_LIMIT];
    for sample in samples.filter(|sample| sample.observed_at_ms >= start_ms) {
        let Some(value) = sample.value(kind, series) else {
            continue;
        };
        let elapsed = sample.observed_at_ms.saturating_sub(start_ms);
        let index =
            ((u128::from(elapsed) * BUCKET_LIMIT as u128) / (u128::from(duration_ms) + 1)) as usize;
        buckets[index.min(BUCKET_LIMIT - 1)].add(sample.observed_at_ms, value);
    }

    let point_count = buckets
        .iter()
        .filter(|bucket| bucket.populated)
        .map(|bucket| usize::from(bucket.minimum_time != bucket.maximum_time) + 1)
        .sum::<usize>();
    let mut commands = String::with_capacity(point_count * 12);
    let mut first = true;
    for bucket in buckets.into_iter().filter(|bucket| bucket.populated) {
        if bucket.minimum_time <= bucket.maximum_time {
            append_path_point(
                &mut commands,
                &mut first,
                bucket.minimum_time,
                bucket.minimum,
                start_ms,
                duration_ms,
                axis_minimum,
                axis_maximum,
            );
            if bucket.maximum_time != bucket.minimum_time {
                append_path_point(
                    &mut commands,
                    &mut first,
                    bucket.maximum_time,
                    bucket.maximum,
                    start_ms,
                    duration_ms,
                    axis_minimum,
                    axis_maximum,
                );
            }
        } else {
            append_path_point(
                &mut commands,
                &mut first,
                bucket.maximum_time,
                bucket.maximum,
                start_ms,
                duration_ms,
                axis_minimum,
                axis_maximum,
            );
            append_path_point(
                &mut commands,
                &mut first,
                bucket.minimum_time,
                bucket.minimum,
                start_ms,
                duration_ms,
                axis_minimum,
                axis_maximum,
            );
        }
    }
    commands
}

#[allow(clippy::too_many_arguments)]
fn append_path_point(
    commands: &mut String,
    first: &mut bool,
    observed_at_ms: u64,
    value: f32,
    start_ms: u64,
    duration_ms: u64,
    axis_minimum: f32,
    axis_maximum: f32,
) {
    let elapsed = observed_at_ms.saturating_sub(start_ms);
    let x = ((elapsed as f64 / duration_ms as f64) * f64::from(PATH_COORDINATE_MAX))
        .round()
        .clamp(0.0, f64::from(PATH_COORDINATE_MAX)) as u16;
    let y = (((axis_maximum - value) / (axis_maximum - axis_minimum)) * PATH_COORDINATE_MAX)
        .round()
        .clamp(0.0, PATH_COORDINATE_MAX) as u16;
    if *first {
        let _ = write!(commands, "M{x} {y}");
        *first = false;
    } else {
        let _ = write!(commands, " L{x} {y}");
    }
}

fn axis_bounds(kind: GraphKind, minimum: f32, maximum: f32) -> (f32, f32) {
    match kind {
        GraphKind::Power | GraphKind::Current => {
            let floor = if kind == GraphKind::Power { 1.0 } else { 0.1 };
            (0.0, (maximum.max(0.0) * 1.1).max(floor))
        }
        GraphKind::Voltage | GraphKind::Temperature => {
            let minimum_padding = if kind == GraphKind::Voltage {
                0.02
            } else {
                0.5
            };
            let padding = ((maximum - minimum).abs() * 0.1).max(minimum_padding);
            (minimum - padding, maximum + padding)
        }
    }
}

fn format_number(kind: GraphKind, value: f32) -> String {
    match kind {
        GraphKind::Power | GraphKind::Temperature => format!("{value:.1}"),
        GraphKind::Current => format!("{value:.2}"),
        GraphKind::Voltage => format!("{value:.3}"),
    }
}

fn format_value(kind: GraphKind, value: f32) -> String {
    format!("{} {}", format_number(kind, value), kind.unit())
}

const fn summary_name(kind: GraphKind) -> &'static str {
    match kind {
        GraphKind::Power => "TOTAL POWER",
        GraphKind::Current => "TOTAL CURRENT",
        GraphKind::Voltage => "AVERAGE VOLTAGE",
        GraphKind::Temperature => "HOTTEST SENSOR",
    }
}

fn format_missing(kind: GraphKind) -> String {
    format!("-- {}", kind.unit())
}

fn summary_value(kind: GraphKind, sample: &GraphSample) -> String {
    let value = match kind {
        GraphKind::Power => Some(sample.total_power),
        GraphKind::Current => Some(sample.total_current),
        GraphKind::Voltage => Some(sample.average_voltage),
        GraphKind::Temperature => sample
            .temperatures
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, _)| sample.temperature_mask & (1 << index) != 0)
            .map(|(_, value)| value)
            .reduce(f32::max),
    };
    value.map_or_else(|| format_missing(kind), |value| format_value(kind, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::PinSample;

    fn telemetry(sequence: u64, session_id: u64, observed_at_ms: u64) -> TelemetrySnapshot {
        TelemetrySnapshot {
            sequence,
            session_id,
            observed_at_ms,
            stale: false,
            controller_vdd: 3.3,
            average_voltage: 12.0,
            total_current: 9.0,
            total_power: 108.0 + (sequence % 7) as f64,
            fan_duty: 40.0,
            cable_capability: 600,
            pins: std::array::from_fn(|index| PinSample {
                current: 1.0 + index as f64 * 0.2 + (sequence % 5) as f64 * 0.01,
                voltage: 12.0 + index as f64 * 0.01,
                power: 12.0 + index as f64 * 2.4,
            }),
            input_temperature: 45.0,
            output_temperature: 46.0,
            external_1_temperature: None,
            external_2_temperature: Some(47.0),
            active_fault_mask: 0,
            logged_fault_mask: 0,
            unknown_active_fault_mask: 0,
            unknown_logged_fault_mask: 0,
        }
    }

    #[test]
    fn repeated_and_out_of_order_samples_are_not_retained() {
        let mut graph = GraphState::default();
        assert!(graph.record(&telemetry(4, 7, 2_000)).appended);
        assert!(!graph.record(&telemetry(4, 7, 2_000)).appended);
        assert!(!graph.record(&telemetry(3, 7, 1_500)).appended);
        assert!(graph.record(&telemetry(5, 7, 2_000)).appended);
        assert_eq!(graph.samples.len(), 2);
        assert_eq!(graph.samples.back().unwrap().observed_at_ms, 2_001);
    }

    #[test]
    fn a_new_session_discards_the_previous_session() {
        let mut graph = GraphState::default();
        graph.record(&telemetry(4, 7, 2_000));
        let outcome = graph.record(&telemetry(1, 8, 2_500));
        assert!(outcome.appended);
        assert!(outcome.session_reset);
        assert_eq!(graph.samples.len(), 1);
        assert_eq!(graph.session_id, Some(8));
    }

    #[test]
    fn history_memory_stays_bounded_during_a_long_run() {
        let mut graph = GraphState::default();
        for sequence in 1..=100_000 {
            graph.record(&telemetry(sequence, 7, sequence * 500));
        }
        assert_eq!(graph.samples.len(), SAMPLE_LIMIT);
        assert_eq!(graph.samples.capacity(), SAMPLE_LIMIT);
        assert!(
            graph.samples.capacity() * std::mem::size_of::<GraphSample>() <= 128 * 1_024,
            "the retained telemetry buffer exceeded its memory budget"
        );
    }

    #[test]
    fn paths_are_bounded_and_hidden_series_do_not_allocate_commands() {
        let mut graph = GraphState::default();
        graph.set_window_seconds(600);
        for sequence in 1..=SAMPLE_LIMIT as u64 {
            graph.record(&telemetry(sequence, 7, sequence * 500));
        }
        graph.toggle_series(5);
        let frame = graph.frame();
        assert!(frame.paths[5].is_empty());
        assert!(
            frame.paths[..5]
                .iter()
                .all(|path| path.matches('L').count() <= BUCKET_LIMIT * 2)
        );
        assert_eq!(frame.buffer_count, SAMPLE_LIMIT);
    }

    #[test]
    fn clear_keeps_the_last_key_so_a_poll_duplicate_stays_cleared() {
        let mut graph = GraphState::default();
        let sample = telemetry(4, 7, 2_000);
        graph.record(&sample);
        graph.clear_history();
        assert!(!graph.record(&sample).appended);
        assert!(graph.samples.is_empty());
    }
}
