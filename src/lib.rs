#![forbid(unsafe_code)]

pub mod backend;
pub mod build_info;
pub mod discovery;
pub mod manager;
pub mod protocol;
pub mod varlink;

pub use wireview_core::{config, domain, history, theme};

pub use backend::{
    DeviceBackend, MockBackend, MockControl, MockDisplayResumeFailure, MockThemeWriteFailure,
    SerialBackend,
};
pub use manager::{HostEvent, ManagerHandle, spawn_manager};
pub use wireview_core::domain::*;
