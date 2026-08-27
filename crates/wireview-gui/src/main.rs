#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;
use wireview_gui::{AppOptions, DemoKind, Page};
use wireview_ipc::DEFAULT_SOCKET_PATH;

#[derive(Debug, Parser)]
#[command(
    name = "wireview-gui",
    version = env!("WIREVIEW_GUI_VERSION"),
    about = "Native WireView Pro II desktop client"
)]
struct Cli {
    /// wireviewd Varlink socket.
    #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
    socket: PathBuf,

    /// Run without the desktop system tray icon.
    #[arg(long)]
    no_tray: bool,

    /// Render a deterministic state without connecting to wireviewd.
    #[arg(long, value_enum)]
    demo: Option<DemoKind>,

    /// Open a specific page at startup.
    #[arg(long, value_enum, default_value_t)]
    page: Page,
}

fn main() -> Result<(), slint::PlatformError> {
    let cli = Cli::parse();
    wireview_gui::run(AppOptions {
        socket: cli.socket,
        no_tray: cli.no_tray,
        demo: cli.demo,
        page: cli.page,
    })
}
