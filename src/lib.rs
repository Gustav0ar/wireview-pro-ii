#![forbid(unsafe_code)]

pub mod backend;
pub mod build_info;
pub mod config;
pub mod discovery;
pub mod domain;
pub mod history;
pub mod manager;
pub mod protocol;
pub mod varlink;

pub use backend::{DeviceBackend, MockBackend, MockControl, SerialBackend};
pub use domain::*;
pub use manager::{HostEvent, ManagerHandle, spawn_manager};
