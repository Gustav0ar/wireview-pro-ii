//! Build and IPC compatibility identifiers shared by both packaged binaries.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = match option_env!("WIREVIEW_BUILD_ID") {
    Some(value) => value,
    None => "development",
};

pub const API_VERSION: u32 = 2;
pub const API_CAPABILITIES: &[&str] = &[
    "configuration-items",
    "device-control",
    "device-info",
    "display-leases",
    "fault-registers",
    "history-dump",
    "telemetry",
    "theme-assets-read",
    "theme-assets-write",
];
