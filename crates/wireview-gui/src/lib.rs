#![deny(unsafe_code)]

slint::include_modules!();

mod app;
mod client;
mod export;
mod graph;

#[cfg(feature = "screenshots")]
#[doc(hidden)]
pub use app::demo_window;
pub use app::{AppOptions, Page, run};
pub use client::DemoKind;
