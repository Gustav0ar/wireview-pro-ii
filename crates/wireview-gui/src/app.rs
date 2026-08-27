use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};
use tokio::sync::mpsc;
use wireview_core::config::{DeviceSettings, FaultKind};
use wireview_core::theme::ThemeAssetSlot;

use crate::client::{
    ClientError, Command, ConfigEdits, EventSink, OperationState, TelemetrySnapshot, UiEvent,
    demo_events, start_worker,
};
use crate::graph::{
    GraphFrame, GraphKind, GraphMetadata, GraphState, OverviewFrame, SAMPLE_LIMIT, SharedGraphState,
};
use crate::{
    AppData, AppTray, DemoKind, FaultView, HistoryRow, MainWindow, PinView, ThemeSlotView,
};

const APPLICATION_ID: &str = "io.github.Gustav0ar.WireView";
const GRAPHS_PAGE_INDEX: i32 = 7;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum Page {
    #[default]
    Overview,
    Pins,
    Graphs,
    Faults,
    History,
    Configure,
    Themes,
    Device,
}

impl Page {
    const fn index(self) -> i32 {
        match self {
            Self::Overview => 0,
            Self::Pins => 1,
            Self::Graphs => GRAPHS_PAGE_INDEX,
            Self::Faults => 2,
            Self::History => 3,
            Self::Configure => 4,
            Self::Themes => 5,
            Self::Device => 6,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub socket: PathBuf,
    pub no_tray: bool,
    pub demo: Option<DemoKind>,
    pub page: Page,
}

pub fn run(options: AppOptions) -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    slint::set_xdg_app_id(APPLICATION_ID)?;
    let icon = application_icon();
    window.set_app_icon(icon.clone());
    window.global::<AppData>().set_page(options.page.index());
    let graphs = GraphState::shared();
    render_graph(&window, &graphs);
    render_overview_graph(&window, &graphs);

    let tray = create_tray(options.no_tray, &window, &icon);
    let sink = event_sink(&window, tray.as_ref(), graphs.clone());
    let worker = match options.demo {
        Some(kind) => {
            install_callbacks(&window, None, graphs.clone());
            for event in demo_events(kind) {
                apply_event(&window, event, &graphs);
            }
            None
        }
        None => {
            let worker = start_worker(options.socket, sink);
            install_callbacks(&window, Some(worker.sender()), graphs.clone());
            Some(worker)
        }
    };

    update_tray(&window, tray.as_ref());
    window.show()?;
    let event_loop_result = slint::run_event_loop();
    if let Some(worker) = worker {
        worker.stop();
    }
    event_loop_result
}

/// Creates the real application window with deterministic demo telemetry for
/// the documentation screenshot tool.
#[cfg(feature = "screenshots")]
#[doc(hidden)]
pub fn demo_window(kind: DemoKind, page: Page) -> Result<MainWindow, slint::PlatformError> {
    let window = MainWindow::new()?;
    window.global::<AppData>().set_page(page.index());
    let graphs = GraphState::shared();
    render_graph(&window, &graphs);
    render_overview_graph(&window, &graphs);
    for event in demo_events(kind) {
        apply_event(&window, event, &graphs);
    }
    Ok(window)
}

fn create_tray(no_tray: bool, window: &MainWindow, icon: &Image) -> Option<AppTray> {
    if no_tray {
        return None;
    }
    let tray = match AppTray::new() {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("wireview-gui: system tray unavailable: {error}");
            return None;
        }
    };
    tray.set_tray_icon(icon.clone());

    let window_weak = window.as_weak();
    tray.on_show_window(move || {
        if let Some(window) = window_weak.upgrade()
            && let Err(error) = window.show()
        {
            eprintln!("wireview-gui: failed to show the window: {error}");
        }
    });
    tray.on_quit(|| {
        if let Err(error) = slint::quit_event_loop() {
            eprintln!("wireview-gui: failed to stop the event loop: {error}");
        }
    });
    if let Err(error) = tray.show() {
        eprintln!("wireview-gui: failed to show the system tray icon: {error}");
        return None;
    }
    Some(tray)
}

fn event_sink(window: &MainWindow, tray: Option<&AppTray>, graphs: SharedGraphState) -> EventSink {
    let window_weak = window.as_weak();
    let tray_weak = tray.map(AppTray::as_weak);
    Arc::new(move |event| {
        let tray_weak = tray_weak.clone();
        let graphs = graphs.clone();
        if let Err(error) = window_weak.upgrade_in_event_loop(move |window| {
            apply_event(&window, event, &graphs);
            if let Some(tray) = tray_weak.as_ref().and_then(slint::Weak::upgrade) {
                update_tray(&window, Some(&tray));
            }
        }) {
            eprintln!("wireview-gui: failed to deliver a daemon event: {error}");
        }
    })
}

fn install_callbacks(
    window: &MainWindow,
    commands: Option<mpsc::UnboundedSender<Command>>,
    graphs: SharedGraphState,
) {
    let data = window.global::<AppData>();

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_refresh(move || queue_with_window(&weak, &target, Ok(Command::Refresh)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_set_screen(move |screen| {
        queue_with_window(
            &weak,
            &target,
            Ok(Command::SetScreen(screen.trim().to_owned())),
        );
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_clear_faults(move |active_mask, logged_mask| {
        let masks = u16::try_from(active_mask)
            .ok()
            .zip(u16::try_from(logged_mask).ok())
            .filter(|(active, logged)| {
                (*active | *logged) != 0 && (*active | *logged) & !0x003f == 0
            })
            .ok_or_else(|| {
                ClientError::InvalidInput("fault masks must contain known device bits".into())
            });
        queue_with_window(
            &weak,
            &target,
            masks.map(|(active_mask, logged_mask)| Command::ClearFaults {
                active_mask,
                logged_mask,
            }),
        );
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_apply_config(move || {
        queue_config(&weak, &target, false);
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_store_config(move || {
        queue_config(&weak, &target, true);
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_reload_config(move || queue_with_window(&weak, &target, Ok(Command::ReloadConfig)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_reset_config(move || queue_with_window(&weak, &target, Ok(Command::ResetConfig)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_reboot_device(move || queue_with_window(&weak, &target, Ok(Command::RebootDevice)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_load_history(move || queue_with_window(&weak, &target, Ok(Command::LoadHistory)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_cancel_history(move || queue_with_window(&weak, &target, Ok(Command::CancelHistory)));

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_export_history(move |format, path| {
        queue_with_window(
            &weak,
            &target,
            output_path(&path).map(|path| Command::ExportHistory {
                format: format.into(),
                path,
            }),
        );
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_read_theme(move |slot| {
        queue_with_window(
            &weak,
            &target,
            parse_theme_slot(&slot).map(Command::ReadTheme),
        );
    });

    let weak = window.as_weak();
    let target = commands.clone();
    data.on_export_theme(move |slot, path| {
        queue_with_window(
            &weak,
            &target,
            parse_theme_command(&slot, &path, |slot, path| Command::ExportTheme {
                slot,
                path,
            }),
        );
    });

    let weak = window.as_weak();
    data.on_write_theme(move |slot, path| {
        queue_with_window(
            &weak,
            &commands,
            parse_theme_command(&slot, &path, |slot, path| Command::WriteTheme {
                slot,
                path,
            }),
        );
    });

    install_graph_callbacks(window, graphs);
}

fn install_graph_callbacks(window: &MainWindow, graphs: SharedGraphState) {
    let data = window.global::<AppData>();

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_show_graphs(move || {
        if let Some(window) = weak.upgrade() {
            render_graph(&window, &state);
        }
    });

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_show_overview(move || {
        if let Some(window) = weak.upgrade() {
            render_overview_graph(&window, &state);
        }
    });

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_select_graph_kind(move |kind| {
        let Some(kind) = GraphKind::parse(&kind) else {
            return;
        };
        if let Some(window) = weak.upgrade() {
            update_graph(&window, &state, |graph| graph.select_kind(kind));
        }
    });

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_set_graph_window(move |seconds| {
        let Ok(seconds) = u64::try_from(seconds) else {
            return;
        };
        if let Some(window) = weak.upgrade() {
            update_graph(&window, &state, |graph| {
                graph.set_window_seconds(seconds);
            });
        }
    });

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_toggle_graph_series(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if let Some(window) = weak.upgrade() {
            update_graph(&window, &state, |graph| graph.toggle_series(index));
        }
    });

    let weak = window.as_weak();
    let state = graphs.clone();
    data.on_toggle_graph_paused(move || {
        if let Some(window) = weak.upgrade() {
            update_graph(&window, &state, GraphState::toggle_paused);
        }
    });

    let weak = window.as_weak();
    data.on_clear_graph(move || {
        if let Some(window) = weak.upgrade() {
            update_graph(&window, &graphs, GraphState::clear_history);
            render_overview_graph(&window, &graphs);
        }
    });
}

fn update_graph(
    window: &MainWindow,
    graphs: &SharedGraphState,
    update: impl FnOnce(&mut GraphState),
) {
    let (metadata, frame) = {
        let mut graph = graphs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut graph);
        (graph.metadata(), graph.frame())
    };
    let data = window.global::<AppData>();
    apply_graph_metadata(&data, metadata);
    apply_graph_frame(&data, frame);
}

fn render_graph(window: &MainWindow, graphs: &SharedGraphState) {
    update_graph(window, graphs, |_| {});
}

fn render_overview_graph(window: &MainWindow, graphs: &SharedGraphState) {
    let frame = graphs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .overview_frame();
    apply_overview_frame(&window.global::<AppData>(), frame);
}

fn apply_graph_metadata(data: &AppData<'_>, metadata: GraphMetadata) {
    data.set_graph_kind(metadata.kind.into());
    data.set_graph_title(metadata.title.into());
    data.set_graph_series_count(saturating_i32(metadata.series_count));
    data.set_graph_window_seconds(saturating_i32(metadata.window_seconds as usize));
    data.set_graph_window_axis_label(metadata.window_axis_label.into());
    data.set_graph_paused(metadata.paused);
    let [label_1, label_2, label_3, label_4, label_5, label_6] = metadata.labels;
    data.set_graph_series_1_label(label_1.into());
    data.set_graph_series_2_label(label_2.into());
    data.set_graph_series_3_label(label_3.into());
    data.set_graph_series_4_label(label_4.into());
    data.set_graph_series_5_label(label_5.into());
    data.set_graph_series_6_label(label_6.into());
    let [
        visible_1,
        visible_2,
        visible_3,
        visible_4,
        visible_5,
        visible_6,
    ] = metadata.visible;
    data.set_graph_series_1_visible(visible_1);
    data.set_graph_series_2_visible(visible_2);
    data.set_graph_series_3_visible(visible_3);
    data.set_graph_series_4_visible(visible_4);
    data.set_graph_series_5_visible(visible_5);
    data.set_graph_series_6_visible(visible_6);
}

fn apply_graph_frame(data: &AppData<'_>, frame: GraphFrame) {
    let [path_1, path_2, path_3, path_4, path_5, path_6] = frame.paths;
    data.set_graph_path_1(path_1.into());
    data.set_graph_path_2(path_2.into());
    data.set_graph_path_3(path_3.into());
    data.set_graph_path_4(path_4.into());
    data.set_graph_path_5(path_5.into());
    data.set_graph_path_6(path_6.into());
    let [value_1, value_2, value_3, value_4, value_5, value_6] = frame.values;
    data.set_graph_series_1_value(value_1.into());
    data.set_graph_series_2_value(value_2.into());
    data.set_graph_series_3_value(value_3.into());
    data.set_graph_series_4_value(value_4.into());
    data.set_graph_series_5_value(value_5.into());
    data.set_graph_series_6_value(value_6.into());
    data.set_graph_range_label(frame.range_label.into());
    data.set_graph_y_max_label(frame.y_max_label.into());
    data.set_graph_y_mid_label(frame.y_mid_label.into());
    data.set_graph_y_min_label(frame.y_min_label.into());
    data.set_graph_summary_name(frame.summary_name.into());
    data.set_graph_summary_value(frame.summary_value.into());
    data.set_graph_sample_count(saturating_i32(frame.sample_count));
    data.set_graph_buffer_label(format!("{} / {SAMPLE_LIMIT}", frame.buffer_count).into());
}

fn apply_overview_frame(data: &AppData<'_>, frame: OverviewFrame) {
    data.set_overview_power_path(frame.path.into());
    data.set_overview_power_range(frame.range_label.into());
    data.set_overview_power_sample_count(saturating_i32(frame.sample_count));
}

fn queue_config(
    weak: &slint::Weak<MainWindow>,
    target: &Option<mpsc::UnboundedSender<Command>>,
    persist: bool,
) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let command = config_edits(&window).map(|edits| Command::ApplyConfig { edits, persist });
    queue(&window, target, command);
}

fn config_edits(window: &MainWindow) -> Result<ConfigEdits, ClientError> {
    let data = window.global::<AppData>();
    let poll_interval_ms = data
        .get_poll_interval()
        .trim()
        .parse()
        .map_err(|_| ClientError::InvalidInput("poll interval must be an integer".into()))?;
    ConfigEdits::new(
        [
            ("friendly_name", data.get_friendly_name().to_string()),
            ("fan.mode", data.get_fan_mode().to_string()),
            (
                "fan.temperature_source",
                data.get_temperature_source().to_string(),
            ),
            ("fan.duty_min_percent", data.get_duty_min().to_string()),
            ("fan.duty_max_percent", data.get_duty_max().to_string()),
            ("fan.temperature_min_c", data.get_fan_temp_min().to_string()),
            ("fan.temperature_max_c", data.get_fan_temp_max().to_string()),
            ("backlight_percent", data.get_backlight().to_string()),
            ("averaging_ms", data.get_averaging().to_string()),
            (
                "logging_interval_seconds",
                data.get_logging_interval().to_string(),
            ),
            (
                "shutdown_wait_seconds",
                data.get_shutdown_wait().to_string(),
            ),
            (
                "display.default_screen",
                data.get_default_screen().to_string(),
            ),
            (
                "fault_thresholds.temperature_c",
                data.get_fault_temperature().to_string(),
            ),
            (
                "fault_thresholds.total_current_a",
                data.get_fault_total_current().to_string(),
            ),
            (
                "fault_thresholds.wire_current_a",
                data.get_fault_wire_current().to_string(),
            ),
            (
                "fault_thresholds.total_power_w",
                data.get_fault_total_power().to_string(),
            ),
            (
                "fault_thresholds.current_imbalance_percent",
                data.get_fault_imbalance().to_string(),
            ),
            (
                "fault_thresholds.current_imbalance_min_load_a",
                data.get_fault_min_load().to_string(),
            ),
            (
                "fault_actions.display",
                data.get_fault_actions_display().to_string(),
            ),
            (
                "fault_actions.buzzer",
                data.get_fault_actions_buzzer().to_string(),
            ),
            (
                "fault_actions.soft_power",
                data.get_fault_actions_soft_power().to_string(),
            ),
            (
                "fault_actions.hard_power",
                data.get_fault_actions_hard_power().to_string(),
            ),
            (
                "display.current_scale_a",
                data.get_current_scale().to_string(),
            ),
            ("display.power_scale", data.get_power_scale().to_string()),
            ("display.rotation_degrees", data.get_rotation().to_string()),
            ("display.timeout_mode", data.get_timeout_mode().to_string()),
            (
                "display.cycle_screens",
                data.get_cycle_screens().to_string(),
            ),
            (
                "display.cycle_time_seconds",
                data.get_cycle_time().to_string(),
            ),
            (
                "display.timeout_seconds",
                data.get_display_timeout().to_string(),
            ),
            (
                "display.primary_color",
                data.get_primary_color().to_string(),
            ),
            (
                "display.secondary_color",
                data.get_secondary_color().to_string(),
            ),
            (
                "display.highlight_color",
                data.get_highlight_color().to_string(),
            ),
            (
                "display.background_color",
                data.get_background_color().to_string(),
            ),
            ("display.background", data.get_background_mode().to_string()),
            ("display.fan_theme", data.get_fan_theme().to_string()),
            ("display.inverted", data.get_inverted().to_string()),
        ],
        poll_interval_ms,
    )
}

fn queue_with_window(
    weak: &slint::Weak<MainWindow>,
    target: &Option<mpsc::UnboundedSender<Command>>,
    command: Result<Command, ClientError>,
) {
    if let Some(window) = weak.upgrade() {
        queue(&window, target, command);
    }
}

fn queue(
    window: &MainWindow,
    target: &Option<mpsc::UnboundedSender<Command>>,
    command: Result<Command, ClientError>,
) {
    let result = command.and_then(|command| {
        let target = target.as_ref().ok_or_else(|| {
            ClientError::InvalidInput("mutations are disabled in demo mode".into())
        })?;
        target.send(command).map_err(|_| ClientError::Disconnected)
    });
    if let Err(error) = result {
        set_operation(window, "error", error.to_string());
    }
}

fn output_path(path: &str) -> Result<PathBuf, ClientError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(ClientError::InvalidInput(
            "output path cannot be empty".into(),
        ));
    }
    Ok(PathBuf::from(path))
}

fn parse_theme_slot(value: &str) -> Result<ThemeAssetSlot, ClientError> {
    ThemeAssetSlot::from_str(value).map_err(Into::into)
}

fn parse_theme_command(
    slot: &str,
    path: &str,
    make: impl FnOnce(ThemeAssetSlot, PathBuf) -> Command,
) -> Result<Command, ClientError> {
    Ok(make(parse_theme_slot(slot)?, output_path(path)?))
}

fn apply_event(window: &MainWindow, event: UiEvent, graphs: &SharedGraphState) {
    match event {
        UiEvent::Offline(reason) => apply_offline(window, reason),
        UiEvent::SessionChanged => reset_session(window, graphs),
        UiEvent::Status(status) => apply_status(window, status),
        UiEvent::Telemetry(telemetry) => apply_telemetry(window, telemetry, graphs),
        UiEvent::DeviceInfo(info) => {
            let data = window.global::<AppData>();
            data.set_device_name(info.product_name.into());
            data.set_device_uid(info.unique_id.into());
            data.set_firmware_version(info.firmware_version.into());
            data.set_hardware_revision(info.hardware_revision.into());
            data.set_config_version(info.config_version.to_string().into());
            data.set_device_build(info.build_string.into());
            data.set_device_capabilities(info.capabilities.join(", ").into());
        }
        UiEvent::Configuration {
            settings,
            poll_interval_ms,
        } => apply_configuration(window, &settings, poll_interval_ms),
        UiEvent::ScreenChanged(screen) => {
            let data = window.global::<AppData>();
            data.set_current_screen(screen.clone().into());
            data.set_screen_choice(screen.into());
        }
        UiEvent::Operation { state, message } => {
            let state = match state {
                OperationState::Running => "running",
                OperationState::Success => "success",
                OperationState::Error => "error",
            };
            set_operation(window, state, message);
        }
        UiEvent::HistoryProgress {
            fraction,
            message,
            active,
        } => {
            let data = window.global::<AppData>();
            data.set_history_loading(active);
            data.set_history_progress(fraction as f32);
            data.set_history_status(message.into());
        }
        UiEvent::HistoryLoaded {
            entries,
            end_found,
            rows,
        } => {
            let data = window.global::<AppData>();
            data.set_history_loading(false);
            data.set_history_loaded(true);
            data.set_history_progress(1.0);
            data.set_history_entry_count(saturating_i32(entries));
            data.set_history_end_found(end_found);
            data.set_history_status(
                if end_found {
                    format!("Loaded {entries} entries; end marker found")
                } else {
                    format!("Loaded {entries} entries; flash contains no end marker")
                }
                .into(),
            );
            let rows = rows
                .into_iter()
                .map(|row| HistoryRow {
                    device_time: format!("{} ms", row.device_time_ms).into(),
                    power: format!("{:.1} W", row.total_power).into(),
                    current: format!("{:.2} A", row.total_current).into(),
                    voltage: format!("{:.2} V", row.average_voltage).into(),
                    input_temp: format!("{:.1} C", row.input_temperature).into(),
                })
                .collect::<Vec<_>>();
            data.set_history_row_count(saturating_i32(rows.len()));
            data.set_history_rows(model(rows));
        }
        UiEvent::ThemeAsset {
            slot,
            width,
            height,
            sha256,
            data: bytes,
        } => apply_theme(window, slot, width, height, &sha256, &bytes),
    }
}

fn apply_offline(window: &MainWindow, reason: String) {
    let data = window.global::<AppData>();
    data.set_daemon_online(false);
    data.set_device_ready(false);
    data.set_stale(true);
    data.set_config_loaded(false);
    data.set_connection_label("OFFLINE".into());
    data.set_connection_detail(reason.clone().into());
    data.set_sample_age("NO LIVE SAMPLE".into());
    data.set_busy_operation(SharedString::default());
    if data.get_operation_state() == "running" {
        set_operation(window, "error", reason);
    }
}

fn reset_session(window: &MainWindow, graphs: &SharedGraphState) {
    graphs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .reset_session();
    let data = window.global::<AppData>();
    reset_telemetry(&data);
    render_graph(window, graphs);
    render_overview_graph(window, graphs);
    data.set_config_loaded(false);
    data.set_config_dirty(false);
    data.set_history_loading(false);
    data.set_history_loaded(false);
    data.set_history_progress(0.0);
    data.set_history_entry_count(0);
    data.set_history_row_count(0);
    data.set_history_end_found(false);
    data.set_history_status("No device history loaded for this session".into());
    data.set_history_rows(model(Vec::new()));
    data.set_theme_preview(Image::default());
    data.set_theme_preview_available(false);
    data.set_theme_status("Read a named slot to preview its exact RGB565 bytes.".into());
    data.set_theme_slots(model(theme_slots(None, None)));
    data.set_device_uid("Unavailable".into());
    data.set_firmware_version("Unavailable".into());
    data.set_hardware_revision("Unavailable".into());
    data.set_config_version("Unavailable".into());
    data.set_device_build("Unavailable".into());
    data.set_device_capabilities("Unavailable".into());
}

fn reset_telemetry(data: &AppData<'_>) {
    data.set_device_ready(false);
    data.set_stale(true);
    data.set_sample_age("WAITING FOR SAMPLE".into());
    data.set_total_power(0.0);
    data.set_total_power_label("--".into());
    data.set_total_current(0.0);
    data.set_total_current_label("--".into());
    data.set_mean_current_label("--".into());
    data.set_average_voltage(0.0);
    data.set_average_voltage_label("--".into());
    data.set_controller_vdd(0.0);
    data.set_controller_vdd_label("--".into());
    data.set_input_temperature(0.0);
    data.set_input_temperature_label("--".into());
    data.set_output_temperature(0.0);
    data.set_output_temperature_label("--".into());
    data.set_external_1_temperature(0.0);
    data.set_external_1_temperature_label("NOT FITTED".into());
    data.set_external_1_present(false);
    data.set_external_2_temperature(0.0);
    data.set_external_2_temperature_label("NOT FITTED".into());
    data.set_external_2_present(false);
    data.set_fan_duty(0.0);
    data.set_fan_duty_label("--".into());
    data.set_cable_capability(0);
    data.set_cable_capability_label("--".into());
    data.set_pins(model(empty_pins()));
    data.set_overview_power_path(SharedString::default());
    data.set_overview_power_range("WAITING FOR LIVE SAMPLES".into());
    data.set_overview_power_sample_count(0);
    data.set_geometry_read("No complete six-conductor sample".into());
    data.set_faults(model(empty_faults()));
    data.set_fault_row_count(saturating_i32(FAULT_DEFINITIONS.len()));
    data.set_active_fault_mask(0);
    data.set_logged_fault_mask(0);
    data.set_unknown_active_mask(0);
    data.set_unknown_logged_mask(0);
    data.set_active_fault_hex("0000".into());
    data.set_logged_fault_hex("0000".into());
    data.set_unknown_active_hex("0000".into());
    data.set_unknown_logged_hex("0000".into());
    data.set_active_fault_count(0);
    data.set_recorded_fault_count(0);
    set_active_fault_limits(data, FaultLimits::default());
}

fn apply_status(window: &MainWindow, status: wireview_ipc::StatusDto) {
    let data = window.global::<AppData>();
    let ready = matches!(status.state.as_str(), "ready" | "busy");
    let (label, detail) = match status.state.as_str() {
        "ready" => ("READY", format!("Connected on {}", status.connected_port)),
        "busy" => (
            "BUSY",
            if status.busy_operation.is_empty() {
                "Device operation in progress".into()
            } else {
                status.busy_operation.clone()
            },
        ),
        "absent" => (
            "NO DEVICE",
            nonempty_or(
                &status.last_disconnect_reason,
                "No compatible device detected",
            ),
        ),
        "ambiguous" => (
            "MULTIPLE",
            format!("{} candidate devices detected", status.candidates.len()),
        ),
        "unsupported" => (
            "UNSUPPORTED",
            nonempty_or(&status.recovery_cause, "Unsupported device or firmware"),
        ),
        "recovering" => (
            "RECOVERING",
            nonempty_or(&status.recovery_cause, "Re-establishing the device session"),
        ),
        _ => ("CONNECTING", format!("Daemon state: {}", status.state)),
    };
    data.set_daemon_online(true);
    data.set_device_ready(ready);
    data.set_connection_label(label.into());
    data.set_connection_detail(detail.into());
    data.set_daemon_version(format!("wireviewd {}", status.daemon_version).into());
    data.set_daemon_build(status.daemon_build_id.into());
    data.set_connected_port(status.connected_port.into());
    data.set_session_label(format!("SESSION {:02}", status.session_id).into());
    if !data.get_config_dirty() {
        data.set_poll_interval(status.poll_interval_ms.to_string().into());
    }
    data.set_busy_operation(status.busy_operation.into());
    if !ready {
        data.set_stale(true);
    }
}

fn apply_telemetry(window: &MainWindow, telemetry: TelemetrySnapshot, graphs: &SharedGraphState) {
    let data = window.global::<AppData>();
    let page = data.get_page();
    let (outcome, graph_frame, overview_frame) = {
        let mut graph = graphs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = graph.record(&telemetry);
        let graph_frame = (outcome.appended && page == GRAPHS_PAGE_INDEX && !graph.paused())
            .then(|| graph.frame());
        let overview_frame =
            (outcome.appended && page == Page::Overview.index()).then(|| graph.overview_frame());
        (outcome, graph_frame, overview_frame)
    };
    if outcome.session_reset {
        reset_telemetry(&data);
    }
    let age_ms = unix_time_ms().saturating_sub(telemetry.observed_at_ms);
    let stale = telemetry.stale;
    data.set_device_ready(true);
    data.set_stale(stale);
    data.set_sample_age(sample_age_label(age_ms, stale).into());
    if !outcome.appended {
        return;
    }
    data.set_total_power(round(telemetry.total_power, 1));
    data.set_total_power_label(format!("{:.1}", telemetry.total_power).into());
    data.set_total_current(round(telemetry.total_current, 2));
    data.set_total_current_label(format!("{:.2}", telemetry.total_current).into());
    data.set_mean_current_label(format!("{:.2}", telemetry.total_current / 6.0).into());
    data.set_average_voltage(round(telemetry.average_voltage, 2));
    data.set_average_voltage_label(format!("{:.2}", telemetry.average_voltage).into());
    data.set_controller_vdd(round(telemetry.controller_vdd, 2));
    data.set_controller_vdd_label(format!("{:.2}", telemetry.controller_vdd).into());
    data.set_input_temperature(round(telemetry.input_temperature, 1));
    data.set_input_temperature_label(format!("{:.1}", telemetry.input_temperature).into());
    data.set_output_temperature(round(telemetry.output_temperature, 1));
    data.set_output_temperature_label(format!("{:.1}", telemetry.output_temperature).into());
    data.set_external_1_present(telemetry.external_1_temperature.is_some());
    data.set_external_1_temperature(round(
        telemetry.external_1_temperature.unwrap_or_default(),
        1,
    ));
    data.set_external_1_temperature_label(
        telemetry
            .external_1_temperature
            .map_or_else(|| "NOT FITTED".into(), |value| format!("{value:.1}"))
            .into(),
    );
    data.set_external_2_present(telemetry.external_2_temperature.is_some());
    data.set_external_2_temperature(round(
        telemetry.external_2_temperature.unwrap_or_default(),
        1,
    ));
    data.set_external_2_temperature_label(
        telemetry
            .external_2_temperature
            .map_or_else(|| "NOT FITTED".into(), |value| format!("{value:.1}"))
            .into(),
    );
    data.set_fan_duty(round(telemetry.fan_duty, 1));
    data.set_fan_duty_label(format!("{:.0}", telemetry.fan_duty).into());
    data.set_cable_capability(i32::from(telemetry.cable_capability));
    data.set_cable_capability_label(telemetry.cable_capability.to_string().into());

    let wire_limit = f64::from(data.get_active_fault_wire_current());
    let mean = telemetry.total_current / 6.0;
    let pins = telemetry
        .pins
        .iter()
        .enumerate()
        .map(|(index, pin)| {
            let deviation = if mean.abs() < f64::EPSILON {
                0.0
            } else {
                (pin.current - mean) / mean * 100.0
            };
            PinView {
                id: format!("P{}", index + 1).into(),
                current: round(pin.current, 2),
                current_label: format!("{:.2}", pin.current).into(),
                voltage: round(pin.voltage, 2),
                voltage_label: format!("{:.2}", pin.voltage).into(),
                power: round(pin.power, 1),
                power_label: format!("{:.1}", pin.power).into(),
                deviation: round(deviation, 1),
                deviation_label: format!("{deviation:+.1}").into(),
                load: if wire_limit > 0.0 {
                    (pin.current / wire_limit).clamp(0.0, 1.0) as f32
                } else if pin.current > 0.0 {
                    1.0
                } else {
                    0.0
                },
                alert: pin.current > wire_limit,
            }
        })
        .collect();
    data.set_pins(model(pins));
    data.set_geometry_read(geometry_read(&telemetry, wire_limit).into());

    apply_faults(&data, &telemetry);
    if let Some(frame) = graph_frame {
        apply_graph_frame(&data, frame);
    }
    if let Some(frame) = overview_frame {
        apply_overview_frame(&data, frame);
    }
}

#[derive(Clone, Copy)]
struct FaultDefinition {
    mask: u16,
    name: &'static str,
    label: &'static str,
    action: &'static str,
}

const FAULT_DEFINITIONS: [FaultDefinition; 6] = [
    FaultDefinition {
        mask: 1,
        name: "chip_over_temperature",
        label: "Chip over temperature",
        action: "Reduce load and check WireView airflow and fan operation.",
    },
    FaultDefinition {
        mask: 2,
        name: "sensor_over_temperature",
        label: "Sensor over temperature",
        action: "Reduce load. Reseat and inspect the connector and cable.",
    },
    FaultDefinition {
        mask: 4,
        name: "over_current",
        label: "Total over current",
        action: "Reduce GPU load or verify the configured total-current limit.",
    },
    FaultDefinition {
        mask: 8,
        name: "wire_over_current",
        label: "Wire over current",
        action: "Inspect and reseat the connector and cable for uneven contact.",
    },
    FaultDefinition {
        mask: 16,
        name: "over_power",
        label: "Over power",
        action: "Reduce GPU load or verify the configured total-power limit.",
    },
    FaultDefinition {
        mask: 32,
        name: "current_imbalance",
        label: "Current imbalance",
        action: "Reseat the connector and inspect the cable for poor contact.",
    },
];

#[derive(Clone, Copy)]
struct FaultLimits {
    temperature: f64,
    total_current: f64,
    wire_current: f64,
    total_power: f64,
    imbalance: f64,
    minimum_load: f64,
}

impl Default for FaultLimits {
    fn default() -> Self {
        Self {
            temperature: 80.0,
            total_current: 55.0,
            wire_current: 10.5,
            total_power: 660.0,
            imbalance: 40.0,
            minimum_load: 6.0,
        }
    }
}

fn active_fault_limits(data: &AppData<'_>) -> FaultLimits {
    FaultLimits {
        temperature: f64::from(data.get_active_fault_temperature()),
        total_current: f64::from(data.get_active_fault_total_current()),
        wire_current: f64::from(data.get_active_fault_wire_current()),
        total_power: f64::from(data.get_active_fault_total_power()),
        imbalance: f64::from(data.get_active_fault_imbalance()),
        minimum_load: f64::from(data.get_active_fault_min_load()),
    }
}

fn set_active_fault_limits(data: &AppData<'_>, limits: FaultLimits) {
    data.set_active_fault_temperature(limits.temperature as f32);
    data.set_active_fault_total_current(limits.total_current as f32);
    data.set_active_fault_wire_current(limits.wire_current as f32);
    data.set_active_fault_wire_current_label(format!("{:.1}", limits.wire_current).into());
    data.set_active_fault_total_power(limits.total_power as f32);
    data.set_active_fault_imbalance(limits.imbalance as f32);
    data.set_active_fault_min_load(limits.minimum_load as f32);
}

fn empty_pins() -> Vec<PinView> {
    (1..=6)
        .map(|index| PinView {
            id: format!("P{index}").into(),
            current: 0.0,
            current_label: "--".into(),
            voltage: 0.0,
            voltage_label: "--".into(),
            power: 0.0,
            power_label: "--".into(),
            deviation: 0.0,
            deviation_label: "--".into(),
            load: 0.0,
            alert: false,
        })
        .collect()
}

fn empty_faults() -> Vec<FaultView> {
    FAULT_DEFINITIONS
        .into_iter()
        .map(|definition| FaultView {
            name: definition.name.into(),
            label: definition.label.into(),
            detail: "No active or recorded alarm".into(),
            action: definition.action.into(),
            mask: i32::from(definition.mask),
            active: false,
            recorded: false,
            known: true,
        })
        .collect()
}

fn apply_faults(data: &AppData<'_>, telemetry: &TelemetrySnapshot) {
    let limits = active_fault_limits(data);
    let mut faults = FAULT_DEFINITIONS
        .into_iter()
        .map(|definition| {
            let active = telemetry.active_fault_mask & definition.mask != 0;
            let recorded = telemetry.logged_fault_mask & definition.mask != 0;
            FaultView {
                name: definition.name.into(),
                label: definition.label.into(),
                detail: fault_detail(definition.mask, active, recorded, telemetry, limits).into(),
                action: definition.action.into(),
                mask: i32::from(definition.mask),
                active,
                recorded,
                known: true,
            }
        })
        .collect::<Vec<_>>();
    if telemetry.unknown_active_fault_mask != 0 || telemetry.unknown_logged_fault_mask != 0 {
        faults.push(FaultView {
            name: "unknown_bits".into(),
            label: "Unknown register bits".into(),
            detail: "Preserved for diagnosis. This client will not clear unknown bits.".into(),
            action: "Update wireviewd before diagnosing or clearing this firmware bit.".into(),
            mask: 0,
            active: telemetry.unknown_active_fault_mask != 0,
            recorded: telemetry.unknown_logged_fault_mask != 0,
            known: false,
        });
    }
    data.set_faults(model(faults));
    data.set_fault_row_count(saturating_i32(data.get_faults().row_count()));
    data.set_active_fault_mask(i32::from(telemetry.active_fault_mask));
    data.set_logged_fault_mask(i32::from(telemetry.logged_fault_mask));
    data.set_unknown_active_mask(i32::from(telemetry.unknown_active_fault_mask));
    data.set_unknown_logged_mask(i32::from(telemetry.unknown_logged_fault_mask));
    data.set_active_fault_hex(format!("{:04X}", telemetry.active_fault_mask).into());
    data.set_logged_fault_hex(format!("{:04X}", telemetry.logged_fault_mask).into());
    data.set_unknown_active_hex(format!("{:04X}", telemetry.unknown_active_fault_mask).into());
    data.set_unknown_logged_hex(format!("{:04X}", telemetry.unknown_logged_fault_mask).into());
    data.set_active_fault_count(saturating_i32(
        telemetry.active_fault_mask.count_ones() as usize
    ));
    data.set_recorded_fault_count(saturating_i32(
        telemetry.logged_fault_mask.count_ones() as usize
    ));
}

fn fault_detail(
    mask: u16,
    active: bool,
    recorded: bool,
    telemetry: &TelemetrySnapshot,
    limits: FaultLimits,
) -> String {
    if !active && !recorded {
        return "No active or recorded alarm".into();
    }
    match mask {
        1 => "The device's internal safety temperature was exceeded".into(),
        2 => {
            let (source, temperature) = hottest_temperature(telemetry);
            format!(
                "{source} {temperature:.1} C now / {:.1} C limit",
                limits.temperature
            )
        }
        4 => format!(
            "{:.2} A now / {:.0} A total limit",
            telemetry.total_current, limits.total_current
        ),
        8 => {
            let (high_index, maximum, _, _) = current_extremes(telemetry);
            format!(
                "P{} {maximum:.2} A now / {:.1} A per-conductor limit",
                high_index + 1,
                limits.wire_current
            )
        }
        16 => format!(
            "{:.1} W now / {:.0} W total limit",
            telemetry.total_power, limits.total_power
        ),
        32 => {
            let (high_index, maximum, low_index, minimum) = current_extremes(telemetry);
            format!(
                "P{} {maximum:.2} A high, P{} {minimum:.2} A low / {:.0}% above {:.0} A load",
                high_index + 1,
                low_index + 1,
                limits.imbalance,
                limits.minimum_load
            )
        }
        _ => "Device fault register asserted".into(),
    }
}

fn hottest_temperature(telemetry: &TelemetrySnapshot) -> (&'static str, f64) {
    let mut hottest = ("Input", telemetry.input_temperature);
    for candidate in [
        Some(("Output", telemetry.output_temperature)),
        telemetry
            .external_1_temperature
            .map(|value| ("External 1", value)),
        telemetry
            .external_2_temperature
            .map(|value| ("External 2", value)),
    ]
    .into_iter()
    .flatten()
    {
        if candidate.1 > hottest.1 {
            hottest = candidate;
        }
    }
    hottest
}

fn current_extremes(telemetry: &TelemetrySnapshot) -> (usize, f64, usize, f64) {
    let mut high = (0, telemetry.pins[0].current);
    let mut low = high;
    for (index, pin) in telemetry.pins.iter().enumerate().skip(1) {
        if pin.current > high.1 {
            high = (index, pin.current);
        }
        if pin.current < low.1 {
            low = (index, pin.current);
        }
    }
    (high.0, high.1, low.0, low.1)
}

fn geometry_read(telemetry: &TelemetrySnapshot, wire_limit: f64) -> String {
    let spread = current_spread_percent(telemetry);
    let over_limit = telemetry
        .pins
        .iter()
        .filter(|pin| pin.current > wire_limit)
        .count();
    if over_limit > 0 {
        format!("{over_limit} conductor(s) exceed {wire_limit:.1} A; spread is {spread:.1}%")
    } else {
        format!("All conductors are within {wire_limit:.1} A; spread is {spread:.1}%")
    }
}

fn current_spread_percent(telemetry: &TelemetrySnapshot) -> f64 {
    let minimum = telemetry
        .pins
        .iter()
        .map(|pin| pin.current)
        .fold(f64::INFINITY, f64::min);
    let maximum = telemetry
        .pins
        .iter()
        .map(|pin| pin.current)
        .fold(f64::NEG_INFINITY, f64::max);
    if telemetry.total_current.abs() < f64::EPSILON {
        0.0
    } else {
        (maximum - minimum) / (telemetry.total_current / 6.0) * 100.0
    }
}

fn apply_configuration(window: &MainWindow, settings: &DeviceSettings, poll_interval_ms: u64) {
    let data = window.global::<AppData>();
    data.set_config_loaded(true);
    data.set_config_dirty(false);
    data.set_friendly_name(settings.friendly_name.clone().into());
    data.set_fan_mode(json_enum(serde_json::to_value(settings.fan.mode)).into());
    data.set_temperature_source(
        json_enum(serde_json::to_value(settings.fan.temperature_source)).into(),
    );
    data.set_duty_min(settings.fan.duty_min_percent.to_string().into());
    data.set_duty_max(settings.fan.duty_max_percent.to_string().into());
    data.set_fan_temp_min(format!("{:.1}", settings.fan.temperature_min_c).into());
    data.set_fan_temp_max(format!("{:.1}", settings.fan.temperature_max_c).into());
    data.set_backlight(settings.backlight_percent.to_string().into());
    data.set_averaging(settings.averaging_ms.to_string().into());
    data.set_logging_interval(settings.logging_interval_seconds.to_string().into());
    data.set_shutdown_wait(settings.shutdown_wait_seconds.to_string().into());
    data.set_default_screen(
        json_enum(serde_json::to_value(settings.display.default_screen)).into(),
    );
    data.set_poll_interval(poll_interval_ms.to_string().into());
    data.set_fault_temperature(format!("{:.1}", settings.fault_thresholds.temperature_c).into());
    data.set_fault_total_current(settings.fault_thresholds.total_current_a.to_string().into());
    data.set_fault_wire_current(format!("{:.1}", settings.fault_thresholds.wire_current_a).into());
    data.set_fault_total_power(settings.fault_thresholds.total_power_w.to_string().into());
    data.set_fault_imbalance(
        settings
            .fault_thresholds
            .current_imbalance_percent
            .to_string()
            .into(),
    );
    data.set_fault_min_load(
        settings
            .fault_thresholds
            .current_imbalance_min_load_a
            .to_string()
            .into(),
    );
    set_active_fault_limits(
        &data,
        FaultLimits {
            temperature: settings.fault_thresholds.temperature_c,
            total_current: f64::from(settings.fault_thresholds.total_current_a),
            wire_current: settings.fault_thresholds.wire_current_a,
            total_power: f64::from(settings.fault_thresholds.total_power_w),
            imbalance: f64::from(settings.fault_thresholds.current_imbalance_percent),
            minimum_load: f64::from(settings.fault_thresholds.current_imbalance_min_load_a),
        },
    );
    data.set_fault_actions_display(fault_actions(&settings.fault_actions.display).into());
    data.set_fault_actions_buzzer(fault_actions(&settings.fault_actions.buzzer).into());
    data.set_fault_actions_soft_power(fault_actions(&settings.fault_actions.soft_power).into());
    data.set_fault_actions_hard_power(fault_actions(&settings.fault_actions.hard_power).into());
    data.set_current_scale(settings.display.current_scale_a.to_string().into());
    data.set_power_scale(json_enum(serde_json::to_value(settings.display.power_scale)).into());
    data.set_timeout_mode(json_enum(serde_json::to_value(settings.display.timeout_mode)).into());
    data.set_cycle_screens(screen_list(&settings.display.cycle_screens).into());
    data.set_cycle_time(settings.display.cycle_time_seconds.to_string().into());
    data.set_display_timeout(settings.display.timeout_seconds.to_string().into());
    data.set_primary_color(color_hex(settings.display.primary_color).into());
    data.set_secondary_color(color_hex(settings.display.secondary_color).into());
    data.set_highlight_color(color_hex(settings.display.highlight_color).into());
    data.set_background_color(color_hex(settings.display.background_color).into());
    data.set_background_mode(json_enum(serde_json::to_value(settings.display.background)).into());
    data.set_fan_theme(json_enum(serde_json::to_value(settings.display.fan_theme)).into());
    data.set_rotation(settings.display.rotation_degrees.to_string().into());
    data.set_inverted(settings.display.inverted);
}

fn apply_theme(
    window: &MainWindow,
    slot: ThemeAssetSlot,
    width: u32,
    height: u32,
    sha256: &str,
    bytes: &[u8],
) {
    let data = window.global::<AppData>();
    let image = rgb565_image(width, height, bytes);
    data.set_theme_preview(image);
    data.set_theme_preview_available(true);
    let short_digest = sha256.get(..12).unwrap_or(sha256);
    data.set_theme_status(
        format!(
            "{}: {} x {}, {} bytes, SHA-256 {}",
            slot,
            width,
            height,
            bytes.len(),
            short_digest
        )
        .into(),
    );
    data.set_selected_theme_slot(slot.name().into());
    let slots = data.get_theme_slots();
    let updated = (0..slots.row_count())
        .filter_map(|index| slots.row_data(index))
        .map(|mut row| {
            if row.id == slot.name() {
                row.status = format!("READ {short_digest}").into();
            }
            row
        })
        .collect();
    data.set_theme_slots(model(updated));
}

fn theme_slots(read_slot: Option<ThemeAssetSlot>, digest: Option<&str>) -> Vec<ThemeSlotView> {
    ThemeAssetSlot::ALL
        .into_iter()
        .map(|slot| ThemeSlotView {
            id: slot.name().into(),
            label: theme_slot_label(slot).into(),
            geometry: format!(
                "{} x {} / {} bytes",
                slot.width(),
                slot.height(),
                slot.byte_len()
            )
            .into(),
            status: if Some(slot) == read_slot {
                format!("READ {}", digest.unwrap_or("verified")).into()
            } else {
                "Not read".into()
            },
        })
        .collect()
}

const fn theme_slot_label(slot: ThemeAssetSlot) -> &'static str {
    match slot {
        ThemeAssetSlot::BackgroundOrange => "Background orange",
        ThemeAssetSlot::BackgroundDark => "Background dark",
        ThemeAssetSlot::FanOrange1 => "Fan orange 1",
        ThemeAssetSlot::FanOrange2 => "Fan orange 2",
        ThemeAssetSlot::FanDark1 => "Fan dark 1",
        ThemeAssetSlot::FanDark2 => "Fan dark 2",
        ThemeAssetSlot::FanBlackWhite1 => "Fan black and white 1",
        ThemeAssetSlot::FanBlackWhite2 => "Fan black and white 2",
    }
}

fn rgb565_image(width: u32, height: u32, bytes: &[u8]) -> Image {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(2));
    if expected != Some(bytes.len()) {
        return Image::default();
    }
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
    for (pixel, encoded) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(bytes.chunks_exact(2))
    {
        *pixel = rgb565_pixel([encoded[0], encoded[1]]);
    }
    Image::from_rgb8(buffer)
}

fn rgb565_pixel(bytes: [u8; 2]) -> Rgb8Pixel {
    let value = u16::from_le_bytes(bytes);
    let red = u8::try_from(((value >> 11) & 0x1f) * 255 / 31).expect("red fits u8");
    let green = u8::try_from(((value >> 5) & 0x3f) * 255 / 63).expect("green fits u8");
    let blue = u8::try_from((value & 0x1f) * 255 / 31).expect("blue fits u8");
    Rgb8Pixel::new(red, green, blue)
}

fn application_icon() -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(32, 32);
    for (index, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = index % 32;
        let y = index / 32;
        let conductor = (6..=25).contains(&x) && matches!(y, 8..=10 | 15..=17 | 22..=24);
        let terminal = (4..=8).contains(&x) && matches!(y, 7..=11 | 14..=18 | 21..=25);
        *pixel = if terminal {
            Rgba8Pixel::new(217, 222, 227, 255)
        } else if conductor {
            Rgba8Pixel::new(255, 177, 92, 255)
        } else {
            Rgba8Pixel::new(0, 0, 0, 255)
        };
    }
    Image::from_rgba8(buffer)
}

fn update_tray(window: &MainWindow, tray: Option<&AppTray>) {
    let Some(tray) = tray else {
        return;
    };
    let data = window.global::<AppData>();
    let status = if data.get_active_fault_count() > 0 {
        format!(
            "WireView: {} active fault(s)",
            data.get_active_fault_count()
        )
    } else {
        format!("WireView: {}", data.get_connection_label())
    };
    tray.set_status(status.into());
}

fn set_operation(window: &MainWindow, state: &str, message: impl Into<SharedString>) {
    let data = window.global::<AppData>();
    data.set_operation_state(state.into());
    data.set_operation_message(message.into());
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}

fn json_enum(value: Result<serde_json::Value, serde_json::Error>) -> String {
    value
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

fn fault_actions(values: &[FaultKind]) -> String {
    if values.is_empty() {
        return "none".into();
    }
    values
        .iter()
        .filter_map(|value| serde_json::to_value(value).ok())
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>()
        .join(",")
}

fn screen_list(values: &[wireview_core::config::ConfigScreen]) -> String {
    if values.is_empty() {
        return "none".into();
    }
    values
        .iter()
        .filter_map(|value| serde_json::to_value(value).ok())
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>()
        .join(",")
}

fn color_hex(value: u32) -> String {
    if value & 0xff00_0000 == 0xff00_0000 {
        format!("{:06X}", value & 0x00ff_ffff)
    } else {
        format!("{value:08X}")
    }
}

fn nonempty_or(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.into()
    } else {
        value.into()
    }
}

fn sample_age_label(age_ms: u64, stale: bool) -> String {
    let prefix = if stale { "STALE " } else { "SAMPLE " };
    if age_ms < 1_000 {
        format!("{prefix}{age_ms} MS")
    } else {
        format!("{prefix}{:.1} S", age_ms as f64 / 1_000.0)
    }
}

fn round(value: f64, decimal_places: u32) -> f32 {
    let factor = 10_f64.powi(i32::try_from(decimal_places).unwrap_or(0));
    ((value * factor).round() / factor) as f32
}

fn saturating_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::Mutex;

    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::platform::{EventLoopProxy, Platform, PlatformError, WindowAdapter};

    use super::*;

    enum TestEvent {
        Invoke(Box<dyn FnOnce() + Send>),
        Quit,
    }

    #[derive(Clone)]
    struct TestEventLoopProxy {
        events: Arc<Mutex<VecDeque<TestEvent>>>,
    }

    impl EventLoopProxy for TestEventLoopProxy {
        fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
            self.events.lock().unwrap().push_back(TestEvent::Quit);
            Ok(())
        }

        fn invoke_from_event_loop(
            &self,
            event: Box<dyn FnOnce() + Send>,
        ) -> Result<(), slint::EventLoopError> {
            self.events
                .lock()
                .unwrap()
                .push_back(TestEvent::Invoke(event));
            Ok(())
        }
    }

    struct TestPlatform {
        window: Rc<MinimalSoftwareWindow>,
        events: Arc<Mutex<VecDeque<TestEvent>>>,
    }

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            Ok(self.window.clone())
        }

        fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
            Some(Box::new(TestEventLoopProxy {
                events: self.events.clone(),
            }))
        }

        fn run_event_loop(&self) -> Result<(), PlatformError> {
            loop {
                let event = self.events.lock().unwrap().pop_front();
                match event {
                    Some(TestEvent::Invoke(event)) => event(),
                    Some(TestEvent::Quit) | None => return Ok(()),
                }
            }
        }
    }

    fn telemetry() -> TelemetrySnapshot {
        TelemetrySnapshot {
            sequence: 1,
            session_id: 1,
            observed_at_ms: 1,
            stale: false,
            controller_vdd: 3.3,
            average_voltage: 12.0,
            total_current: 9.0,
            total_power: 108.0,
            fan_duty: 40.0,
            cable_capability: 600,
            pins: std::array::from_fn(|index| crate::client::PinSample {
                current: 1.0 + index as f64 * 0.2,
                voltage: 12.0,
                power: 12.0 + index as f64 * 2.4,
            }),
            input_temperature: 45.0,
            output_temperature: 46.0,
            external_1_temperature: None,
            external_2_temperature: Some(70.0),
            active_fault_mask: 0,
            logged_fault_mask: 0,
            unknown_active_fault_mask: 0,
            unknown_logged_fault_mask: 0,
        }
    }

    #[test]
    fn daemon_events_cross_the_event_loop_and_update_ui_state() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let window_adapter = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(TestPlatform {
            window: window_adapter,
            events,
        }))
        .unwrap();

        let window = MainWindow::new().unwrap();
        let data = window.global::<AppData>();
        assert_eq!(data.get_total_power_label(), "--");

        let graphs = GraphState::shared();
        install_callbacks(&window, None, graphs.clone());
        let sink = event_sink(&window, None, graphs.clone());
        let update = telemetry();
        std::thread::spawn(move || sink(UiEvent::Telemetry(update)))
            .join()
            .unwrap();
        slint::run_event_loop().unwrap();

        assert_eq!(data.get_total_power_label(), "108.0");
        assert_eq!(data.get_total_current_label(), "9.00");
        assert!(data.get_device_ready());
        assert_eq!(data.get_overview_power_sample_count(), 1);

        let mut next = telemetry();
        next.sequence = 2;
        next.observed_at_ms = 501;
        apply_event(&window, UiEvent::Telemetry(next), &graphs);
        data.set_page(GRAPHS_PAGE_INDEX);
        data.invoke_show_graphs();
        assert_eq!(data.get_graph_sample_count(), 2);
        assert!(!data.get_graph_path_1().is_empty());

        data.invoke_select_graph_kind("voltage".into());
        assert_eq!(data.get_graph_kind(), "voltage");
        assert_eq!(data.get_graph_series_count(), 6);
        assert!(!data.get_graph_path_6().is_empty());

        data.invoke_toggle_graph_series(5);
        assert!(!data.get_graph_series_6_visible());
        assert!(data.get_graph_path_6().is_empty());

        data.invoke_clear_graph();
        assert_eq!(data.get_graph_sample_count(), 0);
        assert!(data.get_graph_path_1().is_empty());
    }

    #[test]
    fn rgb565_preview_decodes_little_endian_primary_colors() {
        assert_eq!(
            rgb565_pixel(0xf800_u16.to_le_bytes()),
            Rgb8Pixel::new(255, 0, 0)
        );
        assert_eq!(
            rgb565_pixel(0x07e0_u16.to_le_bytes()),
            Rgb8Pixel::new(0, 255, 0)
        );
        assert_eq!(
            rgb565_pixel(0x001f_u16.to_le_bytes()),
            Rgb8Pixel::new(0, 0, 255)
        );
    }

    #[test]
    fn theme_slot_model_uses_only_recovered_named_regions() {
        let slots = theme_slots(None, None);
        assert_eq!(slots.len(), ThemeAssetSlot::ALL.len());
        assert!(slots.iter().all(|slot| !slot.id.contains("0x")));
    }

    #[test]
    fn sample_age_distinguishes_live_and_stale_values() {
        assert_eq!(sample_age_label(180, false), "SAMPLE 180 MS");
        assert_eq!(sample_age_label(12_000, true), "STALE 12.0 S");
    }

    #[test]
    fn fault_context_uses_external_probes_and_active_limits() {
        let telemetry = telemetry();
        assert_eq!(hottest_temperature(&telemetry), ("External 2", 70.0));
        assert_eq!(
            fault_detail(
                2,
                true,
                false,
                &telemetry,
                FaultLimits {
                    temperature: 65.0,
                    ..FaultLimits::default()
                },
            ),
            "External 2 70.0 C now / 65.0 C limit"
        );
    }

    #[test]
    fn empty_conductors_never_seed_fixture_measurements() {
        let pins = empty_pins();
        assert_eq!(pins.len(), 6);
        assert!(pins.iter().all(|pin| pin.current == 0.0));
        assert!(pins.iter().all(|pin| pin.current_label == "--"));
    }
}
