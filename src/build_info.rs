//! Build and IPC compatibility identifiers shared by both packaged binaries.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = match option_env!("WIREVIEW_BUILD_ID") {
    Some(value) => value,
    None => "development",
};

pub use wireview_ipc::{API_CAPABILITIES, API_VERSION};
