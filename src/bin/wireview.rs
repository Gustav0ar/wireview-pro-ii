#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use futures_util::{StreamExt, pin_mut};
use serde::{Deserialize, Serialize};
use wireview_ipc::{CompatibilityError, validate_status};
use wireviewd::build_info::{BUILD_ID, VERSION};
use wireviewd::config::{DeviceSettings, FaultKind};
use wireviewd::history::{FLASH_LENGTH, HistoryEntry, visit_history};
use wireviewd::theme::{ThemeAssetSlot, sha256_hex};
use wireviewd::varlink::{
    ConfigurationDto, ConfigurationItemDto, DEFAULT_SOCKET_PATH, DeviceInfoDto, StatusDto,
    TelemetryDto, WireViewError, WireViewProxy,
};

const HISTORY_CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
// Keep an abandoned Varlink reply small enough that the daemon can finish
// serializing it and accept the explicit cleanup connection promptly.
const HISTORY_CLIENT_CHUNK_SIZE: usize = 16 * 1024;
const OUTPUT_WRITE_CHUNK: usize = 64 * 1024;
static TEMPORARY_FILE_ID: AtomicU64 = AtomicU64::new(0);

const SCREEN_HELP: &str = "\
Screen modes:
  main          Main overview
  current       Current-focused view
  temp          Temperature view
  temperature   Alias for temp
  status        Status view
  simple        Simplified view
  same          Keep the current screen";

const FAULT_HELP: &str = "\
Fault names:
  chip_over_temperature
  sensor_over_temperature
  over_current
  wire_over_current
  over_power
  current_imbalance

Omit FAULT to clear every known bit in the selected active or logged register.
Unknown/reserved mask bits are never cleared by the normal CLI.";

const HISTORY_HELP: &str = "\
Press Ctrl+C to cancel a dump. The CLI closes the daemon-side dump session and
resumes device display updates before exiting. If the CLI is forcibly killed or
crashes, the daemon's 10-second safety lease performs the same cleanup. File
destinations are replaced atomically only after a complete export.";

const CONFIG_HELP: &str = "\
Accepted configuration values (factory defaults in parentheses):
  friendly_name                 Up to 32 printable ASCII characters (empty)
  backlight_percent             0..100 (100)
  fan.mode                      curve | fixed (curve)
  fan.temperature_source        input | output | external1 | external2 | maximum (maximum)
  fan.duty_min_percent          0..100 (0)
  fan.duty_max_percent          0..100 (100)
  fan.temperature_min_c         0.0..50.0, one decimal place (50.0)
  fan.temperature_max_c         50.0..100.0, one decimal place (80.0)
  shutdown_wait_seconds         0..255 (10)
  logging_interval_seconds      0..255 (60)
  averaging_ms                  22 | 44 | 89 | 177 | 354 | 709 | 1417 (1417)

Fault names:
  chip_over_temperature | sensor_over_temperature | over_current |
  wire_over_current | over_power | current_imbalance
  List settings use comma-separated names; use none to clear a list.

Factory fault actions:
  fault_actions.display         sensor_over_temperature, over_current,
                                wire_over_current, over_power, current_imbalance
  fault_actions.buzzer          sensor_over_temperature, over_current,
                                wire_over_current, over_power
  fault_actions.soft_power      none
  fault_actions.hard_power      sensor_over_temperature, over_current,
                                wire_over_current, over_power

Fault thresholds:
  fault_thresholds.temperature_c
                                0.0..120.0, one decimal place (80.0)
  fault_thresholds.total_current_a
                                0..150 (55)
  fault_thresholds.wire_current_a
                                0.0..25.5, one decimal place (10.5)
  fault_thresholds.total_power_w
                                0..2000 (660)
  fault_thresholds.current_imbalance_percent
                                0..100 (40)
  fault_thresholds.current_imbalance_min_load_a
                                0..10 (6)

Display:
  display.default_screen        main | simple | current | temperature | status (main)
  display.current_scale_a       5 | 10 | 15 | 20 (10)
  display.power_scale           auto | watts300 | watts600 (watts600)
  display.rotation_degrees      0 | 180 (0)
  display.timeout_mode          static | cycle | sleep (static)
  display.cycle_screens         unique screen-name array (main,current,temperature)
  display.cycle_time_seconds    0..255 (10)
  display.timeout_seconds       0..255 (30)
  display.primary_color         6 RGB or 8 ARGB hex characters (FFFFFF)
  display.secondary_color       6 RGB or 8 ARGB hex characters (646464)
  display.highlight_color       6 RGB or 8 ARGB hex characters (E64121)
  display.background_color      6 RGB or 8 ARGB hex characters (000000)
  display.background            thermal_grizzly_orange |
                                thermal_grizzly_dark | disabled
                                (thermal_grizzly_orange)
  display.fan_theme             thermal_grizzly_orange |
                                thermal_grizzly_dark | black_and_white
                                (thermal_grizzly_orange)
  display.inverted              true | false (false)

Use `wireview config show --json` as the editable template. Edit only fields
inside `settings`; the opaque `revision` prevents stale bulk writes and must
remain unchanged. Protocol metadata is managed internally and is not present in
the file. Six-digit RRGGBB colors are opaque; eight-digit AARRGGBB colors
preserve alpha. Do not use # or 0x. Input is case-insensitive and output is
uppercase. See docs/usage.md (installed as
/usr/share/doc/wireviewd/usage.md) for version differences and safety guidance.

Configuration operations:
  get      Read one dotted-path item without printing the complete configuration.
  set      Change one dotted-path item without editing JSON; temporary by default.
  apply    Change active settings only; reload, reboot, or power loss reverts them.
  reload   Replace active settings with the permanently stored settings.
  store    Apply the supplied settings and save them permanently.
  reset    Permanently replace stored settings with firmware defaults.";

#[derive(Debug, Parser)]
#[command(name = "wireview", version, about)]
struct Args {
    /// Varlink Unix socket path.
    #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
    socket: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show daemon connection, build, API, and display-pause status.
    Status,
    /// Show device identity, firmware, build, and capabilities.
    Info {
        /// Print structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect or selectively clear device fault flags.
    Faults {
        /// Print structured fault data as JSON.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        command: Option<FaultCommand>,
    },
    /// Show the latest measurements or refresh them continuously.
    Telemetry {
        /// Print the raw telemetry object as JSON.
        #[arg(long)]
        json: bool,
        /// Refresh the readable display in place until interrupted with Ctrl+C.
        #[arg(long, conflicts_with = "json")]
        watch: bool,
    },
    /// Dump telemetry history stored in the WireView device.
    #[command(after_help = HISTORY_HELP)]
    History {
        /// Output representation.
        #[arg(long, value_enum, default_value_t = HistoryFormat::Table)]
        format: HistoryFormat,
        /// Write the dump to a file instead of standard output.
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Read or change device configuration.
    #[command(after_long_help = CONFIG_HELP)]
    Config {
        /// Print configuration results as JSON.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Back up or replace fixed device display bitmap slots.
    Theme {
        #[command(subcommand)]
        command: ThemeCommand,
    },
    /// Show available screen modes or select one.
    #[command(after_help = SCREEN_HELP)]
    Screen {
        /// Display mode. Omit this value or use "help" to show every mode.
        #[arg(value_name = "SCREEN")]
        screen: Option<String>,
    },
    /// Low-level commands for diagnostics, scripts, and integrations.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
    /// Print the installed CLI version and build identifier.
    Version,
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Stream raw JSON daemon events for diagnostics and integrations.
    Monitor {
        /// Exit after this many events; zero keeps monitoring.
        #[arg(long, default_value_t = 0)]
        count: usize,
    },
    /// Reboot the device controller without erasing stored configuration.
    RebootDevice {
        /// Confirm the device reboot.
        #[arg(long)]
        yes: bool,
    },
    /// Permanently replace stored settings with firmware defaults.
    FactoryReset {
        /// Confirm the permanent configuration reset.
        #[arg(long)]
        yes: bool,
    },
    /// Show or change the daemon telemetry polling interval.
    PollInterval {
        /// New interval in milliseconds (100..=5000); omit to show it.
        milliseconds: Option<u64>,
    },
    /// Temporarily freeze physical-screen updates for diagnostics.
    PauseDisplay {
        /// Lease duration in seconds (1..=300); defaults to 30.
        #[arg(default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        seconds: u64,
    },
    /// End a debug display-pause lease; active history dumps remain paused.
    ResumeDisplay,
}

#[derive(Debug, Subcommand)]
enum FaultCommand {
    /// Selectively clear active or logged fault bits.
    #[command(after_long_help = FAULT_HELP)]
    Clear {
        /// Which fault register to clear.
        #[arg(value_enum)]
        target: FaultTarget,
        /// Fault names; omit to clear all known faults in the selected register.
        #[arg(value_name = "FAULT")]
        faults: Vec<String>,
        /// Confirm the fault-clear command.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FaultTarget {
    Active,
    Logged,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show every active device setting.
    Show,
    /// Show one active setting.
    Get {
        /// Dotted configuration key, such as fan.mode.
        key: String,
    },
    /// Change one setting without editing a JSON file.
    #[command(after_long_help = CONFIG_HELP)]
    Set {
        /// Dotted configuration key, such as fan.mode.
        key: String,
        /// New value; lists use comma-separated names or "none".
        value: String,
        /// Store the changed configuration permanently.
        #[arg(long)]
        store: bool,
        /// Confirm the permanent write.
        #[arg(long, requires = "store")]
        yes: bool,
    },
    /// Apply configuration until reload, device reboot, or power loss.
    #[command(after_long_help = CONFIG_HELP)]
    Apply {
        /// JSON file produced by `wireview config show --json`.
        file: PathBuf,
    },
    /// Apply and permanently store a complete JSON configuration.
    #[command(after_long_help = CONFIG_HELP)]
    Store {
        /// JSON file produced by `wireview config show --json`.
        file: PathBuf,
        /// Confirm the persistent write.
        #[arg(long)]
        yes: bool,
    },
    /// Discard temporary changes and reload the permanently stored settings.
    Reload,
    /// Permanently replace stored configuration with firmware defaults.
    Reset {
        /// Confirm resetting all device configuration.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ThemeCommand {
    /// Read exact device-format RGB565 bytes from a named slot.
    Read {
        /// Fixed theme asset slot.
        ///
        /// Available slots: background-orange, background-dark, fan-orange-1,
        /// fan-orange-2, fan-dark-1, fan-dark-2, fan-black-white-1, and
        /// fan-black-white-2.
        slot: ThemeAssetSlot,
        /// Destination file for the exact RGB565 bytes.
        #[arg(short, long, value_name = "PATH", required = true)]
        output: PathBuf,
    },
    /// Replace one named slot from an exact-size RGB565 file.
    Write {
        /// Fixed theme asset slot. See `theme read --help` for the complete list.
        slot: ThemeAssetSlot,
        /// Exact device-format RGB565 input file.
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Confirm the flash erase/write transaction.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HistoryFormat {
    /// Human-readable rows with every recorded sensor.
    Table,
    /// Comma-separated values suitable for spreadsheets and plotting.
    Csv,
    /// Structured JSON.
    Json,
    /// Exact bytes from the device's 8 MiB logging region.
    Raw,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationDocument {
    revision: String,
    settings: DeviceSettings,
}

#[derive(Default)]
struct HistoryCancellation {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl HistoryCancellation {
    fn request(&self) {
        self.requested.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    async fn requested(&self) {
        while !self.is_requested() {
            self.notify.notified().await;
        }
    }
}

struct SignalTask(tokio::task::JoinHandle<()>);

impl Drop for SignalTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct AtomicOutput {
    destination: PathBuf,
    temporary: PathBuf,
    writer: Option<BufWriter<File>>,
    committed: bool,
}

impl AtomicOutput {
    fn create(destination: &Path) -> std::io::Result<Self> {
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = destination.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "output path must name a file",
            )
        })?;
        for _ in 0..100 {
            let id = TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(
                ".{}.wireview-{}-{id}.tmp",
                name.to_string_lossy(),
                std::process::id()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok(Self {
                        destination: destination.to_owned(),
                        temporary,
                        writer: Some(BufWriter::new(file)),
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a temporary output file",
        ))
    }

    fn writer(&mut self) -> &mut BufWriter<File> {
        self.writer
            .as_mut()
            .expect("atomic output writer exists until commit")
    }

    fn commit(mut self) -> std::io::Result<()> {
        let mut writer = self
            .writer
            .take()
            .expect("atomic output writer exists until commit");
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&self.temporary, &self.destination)?;
        self.committed = true;
        if let Some(parent) = self.destination.parent()
            && !parent.as_os_str().is_empty()
        {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(error_exit_code(error.as_ref()));
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw_arguments = std::env::args_os().skip(1);
    if raw_arguments
        .next()
        .is_some_and(|argument| argument == "__generate-assets")
    {
        let root = raw_arguments.next().ok_or_else(|| {
            std::io::Error::other("__generate-assets requires exactly one staging root")
        })?;
        if raw_arguments.next().is_some() {
            return Err(std::io::Error::other(
                "__generate-assets requires exactly one staging root",
            )
            .into());
        }
        generate_cli_assets(Path::new(&root))?;
        return Ok(());
    }

    let args = Args::parse();
    match &args.command {
        Command::Version => {
            println!("wireview {VERSION} (build {BUILD_ID})");
            return Ok(());
        }
        Command::Screen { screen }
            if screen
                .as_deref()
                .is_none_or(|value| value.eq_ignore_ascii_case("help")) =>
        {
            print_screen_help()?;
            return Ok(());
        }
        Command::Config {
            command: ConfigCommand::Store { yes: false, .. },
            ..
        } => {
            return Err(std::io::Error::other("permanent storage requires --yes").into());
        }
        Command::Config {
            command:
                ConfigCommand::Set {
                    store: true,
                    yes: false,
                    ..
                },
            ..
        } => {
            return Err(std::io::Error::other("permanent item storage requires --yes").into());
        }
        Command::Config {
            command: ConfigCommand::Reset { yes: false },
            ..
        } => {
            return Err(std::io::Error::other("factory reset requires --yes").into());
        }
        Command::Debug {
            command: DebugCommand::RebootDevice { yes: false },
        } => {
            return Err(std::io::Error::other("device reboot requires --yes").into());
        }
        Command::Debug {
            command: DebugCommand::FactoryReset { yes: false },
        } => {
            return Err(std::io::Error::other("factory reset requires --yes").into());
        }
        Command::Faults {
            command: Some(FaultCommand::Clear { yes: false, .. }),
            ..
        } => {
            return Err(std::io::Error::other("clearing device faults requires --yes").into());
        }
        Command::Theme {
            command: ThemeCommand::Write { yes: false, .. },
        } => {
            return Err(std::io::Error::other("theme asset write requires --yes").into());
        }
        Command::Theme {
            command:
                ThemeCommand::Write {
                    slot,
                    file,
                    yes: true,
                },
        } => validate_theme_asset_file(*slot, file)?,
        _ => {}
    }

    let socket = args.socket;
    let mut connection = zlink::tokio::unix::connect(&socket).await?;
    let status = connection.get_status().await?.map_err(boxed_api_error)?;
    require_api_version(&status)?;

    match args.command {
        Command::Status => {
            println!(
                "state={} sequence={} session={} port={} poll={}ms display_paused={} \
                 daemon={} build={} api={} compatibility={}",
                status.state,
                status.sequence,
                status.session_id,
                status.connected_port,
                status.poll_interval_ms,
                status.display_paused,
                status.daemon_version,
                status.daemon_build_id,
                status.api_version,
                status.api_compatibility_id
            );
        }
        Command::Info { json } => {
            let info = connection
                .get_device_info()
                .await?
                .map_err(boxed_api_error)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                print!("{}", format_device_info(&info));
            }
        }
        Command::Faults { json, command } => {
            let telemetry = match command {
                None => connection.get_telemetry().await?.map_err(boxed_api_error)?,
                Some(FaultCommand::Clear {
                    target,
                    faults,
                    yes,
                }) => {
                    debug_assert!(yes);
                    let mask = fault_mask(&faults)?;
                    let (active, logged) = match target {
                        FaultTarget::Active => (mask, 0),
                        FaultTarget::Logged => (0, mask),
                    };
                    connection
                        .clear_faults(active, logged, true)
                        .await?
                        .map_err(boxed_api_error)?
                }
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&fault_json(&telemetry))?);
            } else {
                print!("{}", format_fault_registers(&telemetry));
            }
        }
        Command::Telemetry { json, watch } => {
            let mut telemetry = connection.get_telemetry().await?.map_err(boxed_api_error)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&telemetry)?);
            } else if watch {
                let mut event_connection = zlink::tokio::unix::connect(&socket).await?;
                let event_status = event_connection
                    .get_status()
                    .await?
                    .map_err(boxed_api_error)?;
                require_api_version(&event_status)?;
                let events = event_connection.monitor().await?;
                pin_mut!(events);
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                let mut rendered_lines = 0;
                loop {
                    rendered_lines = write_watch_frame(
                        &mut stdout,
                        &format_telemetry(&telemetry),
                        rendered_lines,
                    )?;
                    stdout.flush()?;

                    let event = tokio::select! {
                        result = tokio::signal::ctrl_c() => {
                            result?;
                            clear_watch_frame(&mut stdout, rendered_lines)?;
                            stdout.flush()?;
                            break;
                        }
                        event = events.next() => event.ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "daemon event stream ended",
                            )
                        })?,
                    };
                    event?.map_err(boxed_api_error)?;
                    telemetry = connection.get_telemetry().await?.map_err(boxed_api_error)?;
                }
            } else {
                print!("{}", format_telemetry(&telemetry));
            }
        }
        Command::History { format, output } => {
            let (cancellation, _signal_task) = start_history_cancellation()?;
            // Begin is deliberately allowed to finish after a signal so the
            // CLI always receives the dump ID needed for explicit cleanup.
            let dump = connection
                .begin_history_dump()
                .await?
                .map_err(boxed_api_error)?;
            let mut interrupted = cancellation.is_requested();
            let read_result: Result<Vec<u8>, Box<dyn std::error::Error>> = async {
                if interrupted {
                    return Err(history_interrupted_error());
                }
                let expected_total = usize::try_from(dump.total_bytes)?;
                if expected_total != FLASH_LENGTH {
                    return Err(std::io::Error::other(format!(
                        "daemon reported an unexpected history size of {expected_total} bytes"
                    ))
                    .into());
                }
                let mut bytes = Vec::with_capacity(expected_total);
                while bytes.len() < expected_total {
                    let length = (expected_total - bytes.len()).min(HISTORY_CLIENT_CHUNK_SIZE);
                    let chunk = tokio::select! {
                        () = cancellation.requested() => {
                            interrupted = true;
                            return Err(history_interrupted_error());
                        }
                        result = connection.read_history_dump_chunk(
                            dump.dump_id,
                            u32::try_from(bytes.len()).expect("history offset fits u32"),
                            u32::try_from(length).expect("history chunk length fits u32"),
                        ) => result?.map_err(boxed_api_error)?,
                    };
                    if usize::try_from(chunk.offset).ok() != Some(bytes.len()) {
                        return Err(std::io::Error::other(
                            "daemon returned a history chunk at the wrong offset",
                        )
                        .into());
                    }
                    if chunk.data.len() != length {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "daemon returned a short history chunk",
                        )
                        .into());
                    }
                    bytes.extend_from_slice(&chunk.data);
                    let percent = bytes.len() as f64 * 100.0 / expected_total as f64;
                    eprint!("\rReading device history: {percent:5.1}%");
                    std::io::stderr().flush()?;
                }
                Ok(bytes)
            }
            .await;
            // A cancelled request can leave its reply queued on the original
            // Varlink connection. Close it before opening a fresh connection
            // so a single-connection server can accept the cleanup request and
            // so cleanup cannot be confused with the abandoned reply.
            let end_result: Result<(), Box<dyn std::error::Error>> = if interrupted {
                drop(connection);
                let cleanup = async {
                    let mut cleanup_connection = zlink::tokio::unix::connect(&socket).await?;
                    cleanup_connection
                        .end_history_dump(dump.dump_id)
                        .await?
                        .map_err(boxed_api_error)?;
                    Ok(())
                };
                match tokio::time::timeout(HISTORY_CLEANUP_TIMEOUT, cleanup).await {
                    Ok(result) => result,
                    Err(_) => Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out waiting for daemon history cleanup",
                    )
                    .into()),
                }
            } else {
                async {
                    connection
                        .end_history_dump(dump.dump_id)
                        .await?
                        .map_err(boxed_api_error)?;
                    Ok(())
                }
                .await
            };
            eprint!("\r\x1b[2K");
            std::io::stderr().flush()?;
            let bytes = match (read_result, end_result) {
                (Ok(bytes), Ok(())) => bytes,
                (Err(read_error), Ok(())) => return Err(read_error),
                (Ok(_), Err(cleanup_error)) => {
                    return Err(std::io::Error::other(format!(
                        "history data was read, but daemon cleanup failed: {cleanup_error}; \
                         the daemon safety lease will retry within 10 seconds"
                    ))
                    .into());
                }
                (Err(read_error), Err(cleanup_error)) => {
                    return Err(std::io::Error::other(format!(
                        "history read failed: {read_error}; daemon cleanup also failed: \
                         {cleanup_error}; the daemon safety lease will retry within 10 seconds"
                    ))
                    .into());
                }
            };
            if matches!(format, HistoryFormat::Raw) {
                write_raw_history(&bytes, output.as_ref(), &cancellation)?;
            } else {
                write_history(&bytes, format, output.as_ref(), &cancellation)?;
            }
        }
        Command::Config { json, command } => match command {
            ConfigCommand::Show => {
                let configuration = connection
                    .get_configuration()
                    .await?
                    .map_err(boxed_api_error)?;
                let document = decode_configuration_dto(configuration)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&document)?);
                } else {
                    print!("{}", format_configuration(&document.settings));
                }
            }
            ConfigCommand::Get { key } => {
                let item = connection
                    .get_configuration_item(&key)
                    .await?
                    .map_err(boxed_api_error)?;
                print_configuration_item(&item, json)?;
            }
            ConfigCommand::Set {
                key,
                value,
                store,
                yes,
            } => {
                debug_assert!(!store || yes);
                let item = connection
                    .set_configuration_item(&key, &value, store, store && yes)
                    .await?
                    .map_err(boxed_api_error)?;
                let value = decode_configuration_item(&item)?;
                let message = if store {
                    format!(
                        "Stored {} = {} permanently.",
                        item.key,
                        format_configuration_value(&value)
                    )
                } else {
                    format!(
                        "Applied {} = {} temporarily.",
                        item.key,
                        format_configuration_value(&value)
                    )
                };
                print_success(&message, json)?;
            }
            ConfigCommand::Apply { file } => {
                let request = read_configuration_file(&file)?;
                connection
                    .apply_configuration(request)
                    .await?
                    .map_err(boxed_api_error)?;
                print_success("Applied temporary configuration successfully.", json)?;
            }
            ConfigCommand::Store { file, yes } => {
                debug_assert!(yes);
                let request = read_configuration_file(&file)?;
                connection
                    .store_configuration(request, yes)
                    .await?
                    .map_err(boxed_api_error)?;
                print_success("Stored configuration permanently.", json)?;
            }
            ConfigCommand::Reload => {
                connection
                    .reload_configuration()
                    .await?
                    .map_err(boxed_api_error)?;
                print_success("Reloaded permanently stored configuration.", json)?;
            }
            ConfigCommand::Reset { yes } => {
                debug_assert!(yes);
                connection
                    .reset_configuration(yes)
                    .await?
                    .map_err(boxed_api_error)?;
                print_success("Reset configuration to firmware defaults.", json)?;
            }
        },
        Command::Theme { command } => match command {
            ThemeCommand::Read { slot, output } => {
                let asset = connection
                    .read_theme_asset(slot.name())
                    .await?
                    .map_err(boxed_api_error)?;
                if asset.slot != slot.name()
                    || usize::try_from(asset.byte_length).ok() != Some(slot.byte_len())
                    || asset.data.len() != slot.byte_len()
                    || asset.width != slot.width()
                    || asset.height != slot.height()
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "daemon returned inconsistent theme asset metadata",
                    )
                    .into());
                }
                let digest = sha256_hex(&asset.data);
                if asset.sha256 != digest {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "daemon returned a theme asset with an invalid SHA-256",
                    )
                    .into());
                }
                let mut destination = AtomicOutput::create(&output)?;
                destination.writer().write_all(&asset.data)?;
                destination.commit()?;
                println!(
                    "Read {slot}: {} bytes, SHA-256 {digest}, written to {}.",
                    asset.data.len(),
                    output.display()
                );
            }
            ThemeCommand::Write { slot, file, yes } => {
                debug_assert!(yes);
                validate_theme_asset_file(slot, &file)?;
                let data = fs::read(&file)?;
                if data.len() != slot.byte_len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "theme asset file changed while it was being read",
                    )
                    .into());
                }
                let digest = sha256_hex(&data);
                let result = connection
                    .write_theme_asset(
                        slot.name(),
                        u32::try_from(data.len()).expect("theme asset length fits u32"),
                        &digest,
                        data,
                        true,
                    )
                    .await?
                    .map_err(boxed_api_error)?;
                if result.slot != slot.name()
                    || usize::try_from(result.byte_length).ok() != Some(slot.byte_len())
                    || result.sha256 != digest
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "daemon returned inconsistent theme write verification",
                    )
                    .into());
                }
                println!("Wrote and verified {slot}: SHA-256 {digest}.");
            }
        },
        Command::Screen {
            screen: Some(screen),
        } => {
            let selected = connection
                .set_screen(&screen)
                .await?
                .map_err(boxed_api_error)?;
            println!("{}", selected.active);
        }
        Command::Debug { command } => match command {
            DebugCommand::Monitor { count } => {
                let events = connection.monitor().await?;
                pin_mut!(events);
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                let mut seen = 0;
                while let Some(event) = events.next().await {
                    let event = event?.map_err(boxed_api_error)?;
                    writeln!(stdout, "{}", serde_json::to_string(&event)?)?;
                    stdout.flush()?;
                    seen += 1;
                    if count != 0 && seen >= count {
                        break;
                    }
                }
            }
            DebugCommand::RebootDevice { yes } => {
                debug_assert!(yes);
                let result = connection
                    .reboot_device(yes)
                    .await?
                    .map_err(boxed_api_error)?;
                if !result.accepted {
                    return Err(
                        std::io::Error::other("daemon did not accept the device reboot").into(),
                    );
                }
                println!(
                    "Device reboot command sent. Stored configuration is unchanged; \
                     the daemon will reconnect automatically."
                );
            }
            DebugCommand::FactoryReset { yes } => {
                debug_assert!(yes);
                connection
                    .reset_configuration(yes)
                    .await?
                    .map_err(boxed_api_error)?;
                println!(
                    "Factory defaults restored and stored permanently. \
                    Existing saved configuration was replaced."
                );
            }
            DebugCommand::PollInterval { milliseconds } => {
                let interval = if let Some(milliseconds) = milliseconds {
                    connection
                        .set_poll_interval(milliseconds)
                        .await?
                        .map_err(boxed_api_error)?
                } else {
                    connection
                        .get_poll_interval()
                        .await?
                        .map_err(boxed_api_error)?
                };
                println!("Telemetry polling interval: {} ms", interval.milliseconds);
            }
            DebugCommand::PauseDisplay { seconds } => {
                let milliseconds = seconds.checked_mul(1000).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "display pause duration is too large",
                    )
                })?;
                let state = connection
                    .pause_display(milliseconds)
                    .await?
                    .map_err(boxed_api_error)?;
                println!(
                    "Display updates paused for up to {} seconds. \
                     Run `wireview debug resume-display` to resume sooner.",
                    state.remaining_ms.div_ceil(1000)
                );
            }
            DebugCommand::ResumeDisplay => {
                let state = connection
                    .resume_display()
                    .await?
                    .map_err(boxed_api_error)?;
                if state.history_dump_active {
                    println!(
                        "Debug display pause ended; the active history dump still owns its pause."
                    );
                } else if state.paused {
                    println!("Display is still paused; retry resume-display or reboot the device.");
                } else {
                    println!("Display updates resumed.");
                }
            }
        },
        Command::Screen { screen: None } | Command::Version => unreachable!(),
    }
    Ok(())
}

fn validate_theme_asset_file(slot: ThemeAssetSlot, file: &Path) -> std::io::Result<()> {
    let metadata = fs::metadata(file)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "theme asset input must be a regular file",
        ));
    }
    if usize::try_from(metadata.len()).ok() != Some(slot.byte_len()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "theme asset {slot} must be exactly {} bytes",
                slot.byte_len()
            ),
        ));
    }
    Ok(())
}

fn read_configuration_file(path: &PathBuf) -> Result<ConfigurationDto, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let document: ConfigurationDocument = serde_json::from_str(&contents).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "configuration must be the revision/settings document produced by \
                 `wireview config show --json`: {error}"
            ),
        )
    })?;
    let configuration_json = serde_json::to_string(&document.settings)?;
    let configuration = DeviceSettings::from_json(&configuration_json)?;
    Ok(ConfigurationDto {
        configuration_json: serde_json::to_string(&configuration)?,
        revision: document.revision,
    })
}

fn decode_configuration_dto(
    dto: ConfigurationDto,
) -> Result<ConfigurationDocument, Box<dyn std::error::Error>> {
    Ok(ConfigurationDocument {
        revision: dto.revision,
        settings: serde_json::from_str(&dto.configuration_json)?,
    })
}

fn require_api_version(status: &StatusDto) -> Result<(), Box<dyn std::error::Error>> {
    validate_status(status).map_err(|error| {
        let message = match error {
            CompatibilityError::ApiVersion { reported, required } => format!(
                "Unsupported wireviewd API version {reported}; this wireview CLI requires version \
                 {required}. Install matching wireview and wireviewd packages."
            ),
            CompatibilityError::ApiSchema { reported, required } => format!(
                "Incompatible wireviewd API schema {reported:?}; this CLI requires \
                 {required:?}. Install matching wireview and wireviewd packages."
            ),
            CompatibilityError::MissingCapabilities(missing) => format!(
                "wireviewd is missing required API capabilities: {missing}. Install matching \
                 wireview and wireviewd packages."
            ),
        };
        Box::<dyn std::error::Error>::from(std::io::Error::other(message))
    })
}

fn generate_cli_assets(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let man_dir = root.join("usr/share/man/man1");
    let bash_dir = root.join("usr/share/bash-completion/completions");
    let zsh_dir = root.join("usr/share/zsh/site-functions");
    let fish_dir = root.join("usr/share/fish/vendor_completions.d");
    for directory in [&man_dir, &bash_dir, &zsh_dir, &fish_dir] {
        fs::create_dir_all(directory)?;
    }

    let mut man_page = BufWriter::new(File::create(man_dir.join("wireview.1"))?);
    clap_mangen::Man::new(Args::command()).render(&mut man_page)?;
    man_page.flush()?;

    for (shell, path) in [
        (Shell::Bash, bash_dir.join("wireview")),
        (Shell::Zsh, zsh_dir.join("_wireview")),
        (Shell::Fish, fish_dir.join("wireview.fish")),
    ] {
        let mut command = Args::command();
        let mut completion = BufWriter::new(File::create(path)?);
        clap_complete::generate(shell, &mut command, "wireview", &mut completion);
        completion.flush()?;
    }
    Ok(())
}

fn history_interrupted_error() -> Box<dyn std::error::Error> {
    std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "History dump interrupted; daemon state cleaned up.",
    )
    .into()
}

fn error_exit_code(error: &(dyn std::error::Error + 'static)) -> i32 {
    if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
    {
        130
    } else {
        1
    }
}

fn start_history_cancellation() -> std::io::Result<(Arc<HistoryCancellation>, SignalTask)> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let cancellation = Arc::new(HistoryCancellation::default());
    let signal_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        let received = tokio::select! {
            result = tokio::signal::ctrl_c() => result.is_ok(),
            signal = terminate.recv() => signal.is_some(),
        };
        if received {
            signal_cancellation.request();
        }
    });
    Ok((cancellation, SignalTask(task)))
}

fn format_device_info(info: &DeviceInfoDto) -> String {
    format!(
        "Device: {}\nUID: {}\nVendor / product: {:02X}:{:02X}\nHardware revision: {}\n\
         Firmware version: {}\nConfiguration version: V{}\nBuild: {}\nCapabilities: {}\n",
        if info.product_name.is_empty() {
            "WireView Pro II"
        } else {
            &info.product_name
        },
        info.unique_id,
        info.vendor_id,
        info.product_id,
        info.hardware_revision,
        info.firmware_version,
        info.config_version + 1,
        if info.build_string.is_empty() {
            "Not reported"
        } else {
            &info.build_string
        },
        info.capabilities.join(", ")
    )
}

fn fault_mask(names: &[String]) -> Result<u16, Box<dyn std::error::Error>> {
    if names.is_empty() {
        return Ok(wireviewd::protocol::KNOWN_FAULT_MASK);
    }
    let mut mask = 0_u16;
    for name in names {
        let bit = match name.to_ascii_lowercase().replace('-', "_").as_str() {
            "chip_over_temperature" => 0,
            "sensor_over_temperature" => 1,
            "over_current" => 2,
            "wire_over_current" => 3,
            "over_power" => 4,
            "current_imbalance" => 5,
            _ => {
                return Err(std::io::Error::other(format!(
                    "unknown fault {name:?}; use chip_over_temperature, \
                     sensor_over_temperature, over_current, wire_over_current, over_power, \
                     or current_imbalance"
                ))
                .into());
            }
        };
        mask |= 1 << bit;
    }
    Ok(mask)
}

fn fault_json(telemetry: &TelemetryDto) -> serde_json::Value {
    serde_json::json!({
        "active_mask": telemetry.active_fault_mask,
        "active_unknown_mask": telemetry.unknown_active_fault_mask,
        "active": telemetry.active_faults,
        "logged_mask": telemetry.logged_fault_mask,
        "logged_unknown_mask": telemetry.unknown_logged_fault_mask,
        "logged": telemetry.logged_faults,
    })
}

fn format_fault_registers(telemetry: &TelemetryDto) -> String {
    let active = if telemetry.active_faults.is_empty() {
        "None".into()
    } else {
        telemetry.active_faults.join(", ")
    };
    let logged = if telemetry.logged_faults.is_empty() {
        "None".into()
    } else {
        telemetry.logged_faults.join(", ")
    };
    format!(
        "Active faults: {active}\n  Raw mask: {:#06X}\n  Unknown bits: {:#06X}\n\
         Logged faults: {logged}\n  Raw mask: {:#06X}\n  Unknown bits: {:#06X}\n",
        telemetry.active_fault_mask,
        telemetry.unknown_active_fault_mask,
        telemetry.logged_fault_mask,
        telemetry.unknown_logged_fault_mask
    )
}

fn format_configuration(configuration: &DeviceSettings) -> String {
    use std::fmt::Write as _;

    let fan = &configuration.fan;
    let thresholds = &configuration.fault_thresholds;
    let display = &configuration.display;
    let mut output = String::new();
    writeln!(output, "Device configuration").expect("String writes are infallible");
    writeln!(output, "  Friendly name: {}", configuration.friendly_name)
        .expect("String writes are infallible");
    writeln!(output, "  Backlight: {}%", configuration.backlight_percent)
        .expect("String writes are infallible");
    writeln!(
        output,
        "  Device logging interval: {} s",
        configuration.logging_interval_seconds
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Sensor averaging: {} ms",
        configuration.averaging_ms
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Shutdown wait: {} s\n",
        configuration.shutdown_wait_seconds
    )
    .expect("String writes are infallible");

    writeln!(output, "Fan").expect("String writes are infallible");
    writeln!(output, "  Mode: {}", enum_name(fan.mode)).expect("String writes are infallible");
    writeln!(
        output,
        "  Temperature source: {}",
        enum_name(fan.temperature_source)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Duty range: {}–{}%",
        fan.duty_min_percent, fan.duty_max_percent
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Temperature range: {:.1}–{:.1} °C\n",
        fan.temperature_min_c, fan.temperature_max_c
    )
    .expect("String writes are infallible");

    writeln!(output, "Fault actions").expect("String writes are infallible");
    writeln!(
        output,
        "  Display: {}",
        format_faults(&configuration.fault_actions.display)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Buzzer: {}",
        format_faults(&configuration.fault_actions.buzzer)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Soft power: {}",
        format_faults(&configuration.fault_actions.soft_power)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Hard power: {}\n",
        format_faults(&configuration.fault_actions.hard_power)
    )
    .expect("String writes are infallible");

    writeln!(output, "Fault thresholds").expect("String writes are infallible");
    writeln!(
        output,
        "  Temperature limit: {:.1} °C",
        thresholds.temperature_c
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Total current limit: {} A",
        thresholds.total_current_a
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Wire current limit: {:.1} A",
        thresholds.wire_current_a
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Total power limit: {} W",
        thresholds.total_power_w
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Current imbalance limit: {}%",
        thresholds.current_imbalance_percent
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Imbalance minimum load: {} A\n",
        thresholds.current_imbalance_min_load_a
    )
    .expect("String writes are infallible");

    writeln!(output, "Display / UI").expect("String writes are infallible");
    writeln!(
        output,
        "  Default screen: {}",
        enum_name(display.default_screen)
    )
    .expect("String writes are infallible");
    writeln!(output, "  Current scale: {} A", display.current_scale_a)
        .expect("String writes are infallible");
    writeln!(output, "  Power scale: {}", enum_name(display.power_scale))
        .expect("String writes are infallible");
    writeln!(output, "  Rotation: {}°", display.rotation_degrees)
        .expect("String writes are infallible");
    writeln!(
        output,
        "  Timeout mode: {}",
        enum_name(display.timeout_mode)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Cycle screens: {}",
        display
            .cycle_screens
            .iter()
            .map(|screen| enum_name(*screen))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Cycle / timeout: {} s / {} s",
        display.cycle_time_seconds, display.timeout_seconds
    )
    .expect("String writes are infallible");
    writeln!(output, "  Background: {}", enum_name(display.background))
        .expect("String writes are infallible");
    writeln!(output, "  Fan theme: {}", enum_name(display.fan_theme))
        .expect("String writes are infallible");
    writeln!(
        output,
        "  Display inversion: {}",
        if display.inverted { "On" } else { "Off" }
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Primary / secondary colors: {} / {}",
        format_config_color(display.primary_color),
        format_config_color(display.secondary_color)
    )
    .expect("String writes are infallible");
    writeln!(
        output,
        "  Highlight / background colors: {} / {}",
        format_config_color(display.highlight_color),
        format_config_color(display.background_color)
    )
    .expect("String writes are infallible");
    output
}

fn format_config_color(color: u32) -> String {
    if color & 0xff00_0000 == 0xff00_0000 {
        format!("{:06X}", color & 0x00ff_ffff)
    } else {
        format!("{color:08X}")
    }
}

fn format_faults(faults: &[FaultKind]) -> String {
    if faults.is_empty() {
        "None".into()
    } else {
        faults
            .iter()
            .map(|fault| enum_name(*fault))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn enum_name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
        .replace('_', " ")
}

fn write_history(
    bytes: &[u8],
    format: HistoryFormat,
    output_path: Option<&PathBuf>,
    cancellation: &HistoryCancellation,
) -> Result<(), Box<dyn std::error::Error>> {
    let records = with_output(output_path, |output| {
        stream_history(output, bytes, format, cancellation)
    })?;
    if let Some(path) = output_path {
        eprintln!("Wrote {records} records to {}", path.display());
    }
    Ok(())
}

fn write_raw_history(
    bytes: &[u8],
    output_path: Option<&PathBuf>,
    cancellation: &HistoryCancellation,
) -> Result<(), Box<dyn std::error::Error>> {
    with_output(output_path, |output| {
        for chunk in bytes.chunks(OUTPUT_WRITE_CHUNK) {
            ensure_history_not_cancelled(cancellation)?;
            output.write_all(chunk)?;
        }
        ensure_history_not_cancelled(cancellation)?;
        Ok(())
    })?;
    if let Some(path) = output_path {
        eprintln!("Wrote {} raw bytes to {}", bytes.len(), path.display());
    }
    Ok(())
}

fn with_output<T>(
    output_path: Option<&PathBuf>,
    operation: impl FnOnce(&mut dyn Write) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    if let Some(path) = output_path {
        let mut output = AtomicOutput::create(path)?;
        let result = operation(output.writer())?;
        output.commit()?;
        Ok(result)
    } else {
        let stdout = std::io::stdout();
        let mut output = BufWriter::new(stdout.lock());
        let result = operation(&mut output)?;
        output.flush()?;
        Ok(result)
    }
}

fn ensure_history_not_cancelled(
    cancellation: &HistoryCancellation,
) -> Result<(), Box<dyn std::error::Error>> {
    if cancellation.is_requested() {
        Err(history_interrupted_error())
    } else {
        Ok(())
    }
}

fn stream_history(
    output: &mut dyn Write,
    bytes: &[u8],
    format: HistoryFormat,
    cancellation: &HistoryCancellation,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut started = false;
    let summary = visit_history(bytes, |entry| {
        ensure_history_not_cancelled(cancellation)?;
        if !started {
            match format {
                HistoryFormat::Table => write_history_table_header(output)?,
                HistoryFormat::Csv => write_history_csv_header(output)?,
                HistoryFormat::Json => writeln!(output, "[")?,
                HistoryFormat::Raw => {
                    unreachable!("raw history is written without record decoding")
                }
            }
            started = true;
        } else if matches!(format, HistoryFormat::Json) {
            writeln!(output, ",")?;
        }
        match format {
            HistoryFormat::Table => write_history_table_entry(output, &entry)?,
            HistoryFormat::Csv => write_history_csv_entry(output, &entry)?,
            HistoryFormat::Json => write_history_json_entry(output, &entry)?,
            HistoryFormat::Raw => unreachable!("raw history is written without record decoding"),
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    ensure_history_not_cancelled(cancellation)?;
    if summary.entries == 0 {
        writeln!(output, "No telemetry history is stored on the device.")?;
    } else if matches!(format, HistoryFormat::Json) {
        writeln!(output, "\n]")?;
    }
    Ok(summary.entries)
}

fn write_history_table_header(output: &mut dyn Write) -> std::io::Result<()> {
    writeln!(
        output,
        "Device time  Event        Total       Average      Temperatures (°C)       Connector pins (V/A)"
    )?;
    writeln!(
        output,
        "                         power/current voltage      in/out/ext1/ext2"
    )?;
    Ok(())
}

fn write_history_table_entry(output: &mut dyn Write, entry: &HistoryEntry) -> std::io::Result<()> {
    let metrics = &entry.metrics;
    let temperatures = &metrics.temperatures;
    let external_1 = format_optional_number(temperatures.external_1_c);
    let external_2 = format_optional_number(temperatures.external_2_c);
    let pins = metrics
        .pins
        .iter()
        .enumerate()
        .map(|(index, pin)| format!("{}:{:.1}/{:.1}", index + 1, pin.voltage_v, pin.current_a))
        .collect::<Vec<_>>()
        .join("  ");
    writeln!(
        output,
        "{:>11}  {:<11} {:>6.1} W/{:<5.1} A {:>6.1} V  {:>4.0}/{:>4.0}/{:>4}/{:>4}  {}",
        format_device_time(entry.device_time_ms),
        history_kind(entry),
        metrics.total_power_w,
        metrics.total_current_a,
        metrics.avg_voltage_v,
        temperatures.input_c,
        temperatures.output_c,
        external_1,
        external_2,
        pins,
    )
}

fn write_history_csv_header(output: &mut dyn Write) -> std::io::Result<()> {
    write!(
        output,
        "device_time_ms,event,total_power_w,total_current_a,average_voltage_v"
    )?;
    for pin in 1..=6 {
        write!(
            output,
            ",pin_{pin}_voltage_v,pin_{pin}_current_a,pin_{pin}_power_w"
        )?;
    }
    writeln!(
        output,
        ",onboard_input_c,onboard_output_c,external_1_c,external_2_c,cable_rating_w"
    )
}

fn write_history_csv_entry(output: &mut dyn Write, entry: &HistoryEntry) -> std::io::Result<()> {
    let metrics = &entry.metrics;
    write!(
        output,
        "{},{},{:.1},{:.1},{:.1}",
        entry.device_time_ms,
        history_kind(entry),
        metrics.total_power_w,
        metrics.total_current_a,
        metrics.avg_voltage_v,
    )?;
    for pin in &metrics.pins {
        write!(
            output,
            ",{:.1},{:.1},{:.1}",
            pin.voltage_v, pin.current_a, pin.power_w
        )?;
    }
    write!(
        output,
        ",{:.0},{:.0},",
        metrics.temperatures.input_c, metrics.temperatures.output_c
    )?;
    write_optional_csv(output, metrics.temperatures.external_1_c)?;
    write!(output, ",")?;
    write_optional_csv(output, metrics.temperatures.external_2_c)?;
    writeln!(output, ",{}", metrics.cable_capability_w)
}

fn write_history_json_entry(
    output: &mut dyn Write,
    entry: &HistoryEntry,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(entry)?;
    write!(output, "  {}", json.replace('\n', "\n  "))?;
    Ok(())
}

fn write_optional_csv(
    output: &mut (impl Write + ?Sized),
    value: Option<f64>,
) -> std::io::Result<()> {
    if let Some(value) = value {
        write!(output, "{value:.0}")?;
    }
    Ok(())
}

fn format_optional_number(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.0}"))
}

fn history_kind(entry: &HistoryEntry) -> &'static str {
    match entry.kind {
        wireviewd::history::HistoryEntryKind::Measurement => "measurement",
        wireviewd::history::HistoryEntryKind::PowerOn => "power-on",
    }
}

fn format_device_time(milliseconds: u32) -> String {
    let total_seconds = milliseconds / 1_000;
    let days = total_seconds / 86_400;
    let hours = total_seconds % 86_400 / 3_600;
    let minutes = total_seconds % 3_600 / 60;
    let seconds = total_seconds % 60;
    let milliseconds = milliseconds % 1_000;
    if days == 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{milliseconds:03}")
    } else {
        format!("{days}d {hours:02}:{minutes:02}:{seconds:02}.{milliseconds:03}")
    }
}

fn write_watch_frame(
    output: &mut impl Write,
    report: &str,
    previous_lines: usize,
) -> std::io::Result<usize> {
    let lines = report.lines().collect::<Vec<_>>();
    let frame_height = previous_lines.max(lines.len());

    if previous_lines != 0 {
        write!(output, "\x1b[{previous_lines}A")?;
    }

    for index in 0..frame_height {
        write!(output, "\r\x1b[2K")?;
        if let Some(line) = lines.get(index) {
            write!(output, "{line}")?;
        }
        writeln!(output)?;
    }
    Ok(frame_height)
}

fn clear_watch_frame(output: &mut impl Write, lines: usize) -> std::io::Result<()> {
    if lines == 0 {
        return Ok(());
    }

    write!(output, "\x1b[{lines}A")?;
    for index in 0..=lines {
        write!(output, "\r\x1b[2K")?;
        if index < lines {
            write!(output, "\x1b[1B")?;
        }
    }
    write!(output, "\x1b[{lines}A\r")?;
    Ok(())
}

fn boxed_api_error(error: WireViewError) -> Box<dyn std::error::Error> {
    Box::new(error)
}

fn decode_configuration_item(
    item: &ConfigurationItemDto,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    serde_json::from_str(&item.value_json).map_err(Into::into)
}

fn format_configuration_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| match value {
                serde_json::Value::String(value) => value.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

fn print_configuration_item(
    item: &ConfigurationItemDto,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let value = decode_configuration_item(item)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "key": item.key,
                "value": value,
            }))?
        );
    } else {
        println!("{} = {}", item.key, format_configuration_value(&value));
    }
    Ok(())
}

fn print_success(message: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "success": true,
                "message": message,
            }))?
        );
    } else {
        println!("{message}");
    }
    Ok(())
}

fn print_screen_help() -> std::io::Result<()> {
    let command = Args::command();
    let mut screen = command
        .find_subcommand("screen")
        .cloned()
        .ok_or_else(|| std::io::Error::other("screen command metadata is unavailable"))?
        .bin_name("wireview screen");
    screen.print_help()?;
    println!();
    Ok(())
}

fn format_telemetry(telemetry: &TelemetryDto) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    writeln!(
        output,
        "Connection: {}",
        if telemetry.stale {
            "Disconnected (last known data)"
        } else {
            "Connected"
        }
    )
    .expect("writing to a String cannot fail");
    writeln!(output, "Sequence: {}", telemetry.sequence).expect("writing to a String cannot fail");
    writeln!(output, "Session: {}", telemetry.session_id).expect("writing to a String cannot fail");
    writeln!(
        output,
        "Last updated: {}",
        format_unix_millis_utc(telemetry.observed_at_ms)
    )
    .expect("writing to a String cannot fail");

    writeln!(output, "\nElectrical").expect("writing to a String cannot fail");
    writeln!(
        output,
        "  Average voltage: {:.3} V",
        telemetry.avg_voltage_v
    )
    .expect("writing to a String cannot fail");
    writeln!(
        output,
        "  Total current: {:.3} A",
        telemetry.total_current_a
    )
    .expect("writing to a String cannot fail");
    writeln!(output, "  Total power: {:.3} W", telemetry.total_power_w)
        .expect("writing to a String cannot fail");
    writeln!(output, "  Internal supply (VDD): {:.3} V", telemetry.vdd_v)
        .expect("writing to a String cannot fail");
    writeln!(
        output,
        "  Cable power rating: {} W",
        telemetry.cable_capability_w
    )
    .expect("writing to a String cannot fail");
    writeln!(output, "  Fan duty: {:.1}%", telemetry.fan_duty_percent)
        .expect("writing to a String cannot fail");

    writeln!(output, "\nConnector pins").expect("writing to a String cannot fail");
    let pin_count = telemetry
        .pin_voltages_v
        .len()
        .max(telemetry.pin_currents_a.len())
        .max(telemetry.pin_power_w.len());
    for index in 0..pin_count {
        let voltage = telemetry.pin_voltages_v.get(index);
        let current = telemetry.pin_currents_a.get(index);
        let power = telemetry.pin_power_w.get(index);
        match (voltage, current, power) {
            (Some(voltage), Some(current), Some(power)) => {
                writeln!(
                    output,
                    "  Pin {}: {:.3} V, {:.3} A, {:.3} W",
                    index + 1,
                    voltage,
                    current,
                    power
                )
                .expect("writing to a String cannot fail");
            }
            _ => {
                writeln!(output, "  Pin {}: Incomplete data", index + 1)
                    .expect("writing to a String cannot fail");
            }
        }
    }
    if pin_count == 0 {
        writeln!(output, "  No pin data").expect("writing to a String cannot fail");
    }

    writeln!(output, "\nTemperatures").expect("writing to a String cannot fail");
    writeln!(output, "  Onboard input: {:.1} °C", telemetry.input_temp_c)
        .expect("writing to a String cannot fail");
    writeln!(
        output,
        "  Onboard output: {:.1} °C",
        telemetry.output_temp_c
    )
    .expect("writing to a String cannot fail");
    write_optional_temperature(
        &mut output,
        "External sensor 1",
        telemetry.external_1_present,
        telemetry.external_1_temp_c,
    );
    write_optional_temperature(
        &mut output,
        "External sensor 2",
        telemetry.external_2_present,
        telemetry.external_2_temp_c,
    );

    writeln!(output, "\nFaults").expect("writing to a String cannot fail");
    write_faults(&mut output, "Active", &telemetry.active_faults);
    write_faults(&mut output, "Logged", &telemetry.logged_faults);
    output
}

fn write_optional_temperature(output: &mut String, label: &str, present: bool, value: f64) {
    use std::fmt::Write as _;

    if present {
        writeln!(output, "  {label}: {value:.1} °C").expect("writing to a String cannot fail");
    } else {
        writeln!(output, "  {label}: Not connected").expect("writing to a String cannot fail");
    }
}

fn write_faults(output: &mut String, label: &str, faults: &[String]) {
    use std::fmt::Write as _;

    if faults.is_empty() {
        writeln!(output, "  {label}: None").expect("writing to a String cannot fail");
    } else {
        writeln!(
            output,
            "  {label}: {}",
            faults
                .iter()
                .map(|fault| fault.replace('_', " "))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("writing to a String cannot fail");
    }
}

fn format_unix_millis_utc(timestamp_ms: u64) -> String {
    let seconds = timestamp_ms / 1_000;
    let milliseconds = timestamp_ms % 1_000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;

    // Convert days since 1970-01-01 to a proleptic Gregorian date.
    let shifted_days = days.saturating_add(719_468);
    let era = shifted_days / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{milliseconds:03} UTC")
}

#[cfg(test)]
mod tests {
    use super::{
        clear_watch_frame, format_config_color, format_device_info, format_telemetry,
        format_unix_millis_utc, require_api_version, with_output, write_watch_frame,
    };
    use wireviewd::varlink::{DeviceInfoDto, StatusDto, TelemetryDto};

    #[test]
    fn configuration_colors_use_short_rgb_only_when_opaque() {
        assert_eq!(format_config_color(0xffff_ffff), "FFFFFF");
        assert_eq!(format_config_color(0xff00_0000), "000000");
        assert_eq!(format_config_color(0x80ff_ffff), "80FFFFFF");
        assert_eq!(format_config_color(0x0000_0000), "00000000");
    }

    #[test]
    fn failed_atomic_output_preserves_the_destination() {
        let root =
            std::env::temp_dir().join(format!("wireview-atomic-output-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let destination = root.join("history.csv");
        std::fs::write(&destination, b"original").unwrap();

        let result: Result<(), Box<dyn std::error::Error>> =
            with_output(Some(&destination), |output| {
                output.write_all(b"partial")?;
                Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled").into())
            });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);

        with_output(Some(&destination), |output| {
            output.write_all(b"complete")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"complete");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daemon_api_version_two_is_required() {
        let status = |api_version| StatusDto {
            api_version,
            api_compatibility_id: wireviewd::varlink::api_compatibility_id().into(),
            api_capabilities: wireviewd::build_info::API_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            daemon_build_id: "test".into(),
            state: "ready".into(),
            sequence: 1,
            session_id: 1,
            connected_port: "/dev/mock".into(),
            last_disconnect_reason: String::new(),
            busy_operation: String::new(),
            recovery_cause: String::new(),
            candidates: Vec::new(),
            poll_interval_ms: 500,
            display_paused: false,
            display_pause_debug_active: false,
            display_pause_history_active: false,
            display_pause_remaining_ms: 0,
        };
        let encoded = serde_json::to_value(status(2)).expect("status should serialize");
        assert_eq!(encoded["api_version"], serde_json::json!(2));
        assert!(encoded["api_version"].is_number());
        assert!(require_api_version(&status(2)).is_ok());

        let mut released_v2 = status(2);
        released_v2.api_compatibility_id = "wireview-2-047f86fdb168c045".into();
        assert!(require_api_version(&released_v2).is_ok());

        for version in [0, 1, u32::MAX] {
            let error = require_api_version(&status(version))
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("API version {version}")));
            assert!(error.contains("requires version 2"));
        }

        let mut incompatible = status(2);
        incompatible.api_compatibility_id.clear();
        let error = require_api_version(&incompatible).unwrap_err().to_string();
        assert!(error.contains("Incompatible wireviewd API schema"));
        assert!(error.contains("not reported"));

        let mut incomplete = status(2);
        incomplete
            .api_capabilities
            .retain(|capability| capability != "history-dump");
        let error = require_api_version(&incomplete).unwrap_err().to_string();
        assert!(error.contains("missing required API capabilities"));
        assert!(error.contains("history-dump"));
    }

    #[test]
    fn device_info_report_labels_firmware_identity_and_capabilities() {
        let output = format_device_info(&DeviceInfoDto {
            unique_id: "A2004C0001".into(),
            vendor_id: 0xef,
            product_id: 0x05,
            firmware_version: "v04".into(),
            hardware_revision: "2.0".into(),
            config_version: 3,
            product_name: "WireView Pro II".into(),
            build_string: "TG-WV-PRO2-FW_20260225_1902".into(),
            capabilities: vec!["telemetry".into(), "history".into()],
        });

        for expected in [
            "Device: WireView Pro II",
            "UID: A2004C0001",
            "Vendor / product: EF:05",
            "Hardware revision: 2.0",
            "Firmware version: v04",
            "Configuration version: V4",
            "Build: TG-WV-PRO2-FW_20260225_1902",
            "Capabilities: telemetry, history",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
    }

    #[test]
    fn telemetry_report_labels_every_metric() {
        let telemetry = TelemetryDto {
            sequence: 42,
            session_id: 3,
            observed_at_ms: 1_700_000_000_000,
            stale: false,
            vdd_v: 3.3,
            avg_voltage_v: 12.047,
            total_current_a: 5.216,
            total_power_w: 62.832,
            fan_duty_percent: 40.0,
            cable_capability_w: 600,
            pin_voltages_v: vec![12.0; 6],
            pin_currents_a: vec![0.67, 0.72, 0.99, 0.90, 0.99, 0.96],
            pin_power_w: vec![8.04, 8.64, 11.88, 10.8, 11.88, 11.52],
            input_temp_c: 26.9,
            output_temp_c: 26.3,
            external_1_present: false,
            external_1_temp_c: 0.0,
            external_2_present: true,
            external_2_temp_c: 25.1,
            active_fault_mask: 0,
            logged_fault_mask: 0x20,
            unknown_active_fault_mask: 0,
            unknown_logged_fault_mask: 0,
            active_faults: Vec::new(),
            logged_faults: vec!["current_imbalance".into()],
        };

        let output = format_telemetry(&telemetry);
        for expected in [
            "Connection: Connected",
            "Sequence: 42",
            "Session: 3",
            "Last updated: 2023-11-14 22:13:20.000 UTC",
            "Average voltage: 12.047 V",
            "Total current: 5.216 A",
            "Total power: 62.832 W",
            "Internal supply (VDD): 3.300 V",
            "Cable power rating: 600 W",
            "Fan duty: 40.0%",
            "Pin 1: 12.000 V, 0.670 A, 8.040 W",
            "Pin 6: 12.000 V, 0.960 A, 11.520 W",
            "Onboard input: 26.9 °C",
            "Onboard output: 26.3 °C",
            "External sensor 1: Not connected",
            "External sensor 2: 25.1 °C",
            "Active: None",
            "Logged: current imbalance",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
    }

    #[test]
    fn unix_milliseconds_are_formatted_as_utc() {
        assert_eq!(format_unix_millis_utc(0), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(
            format_unix_millis_utc(1_785_327_715_440),
            "2026-07-29 12:21:55.440 UTC"
        );
    }

    #[test]
    fn watch_frames_rewrite_existing_lines_without_clearing_the_terminal() {
        let mut output = Vec::new();
        let height = write_watch_frame(&mut output, "first\nsecond\n", 0).unwrap();
        assert_eq!(height, 2);
        let height = write_watch_frame(&mut output, "updated\nshort\n", height).unwrap();
        assert_eq!(height, 2);

        let output = String::from_utf8(output).unwrap();
        assert_eq!(
            output,
            "\r\u{1b}[2Kfirst\n\r\u{1b}[2Ksecond\n\
             \u{1b}[2A\r\u{1b}[2Kupdated\n\r\u{1b}[2Kshort\n"
        );
        assert!(!output.contains("\u{1b}[2J"));
        assert!(!output.contains("\u{1b}[?1049"));
    }

    #[test]
    fn stopping_watch_erases_the_live_frame_and_returns_to_its_first_line() {
        let mut output = Vec::new();
        clear_watch_frame(&mut output, 2).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(
            output,
            "\u{1b}[2A\r\u{1b}[2K\u{1b}[1B\r\u{1b}[2K\u{1b}[1B\
             \r\u{1b}[2K\u{1b}[2A\r"
        );
        assert!(!output.contains("\u{1b}[2J"));
        assert!(!output.contains("\u{1b}[?1049"));
    }
}
