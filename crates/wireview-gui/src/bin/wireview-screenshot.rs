#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{PlatformError, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};
use wireview_gui::{DemoKind, Page, demo_window};

const WIDTH: usize = 1440;
const HEIGHT: usize = 900;

thread_local! {
    static WINDOW: Rc<MinimalSoftwareWindow> =
        MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
}

struct ScreenshotPlatform;

impl slint::platform::Platform for ScreenshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(WINDOW.with(Clone::clone))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: wireview-screenshot OUTPUT.ppm overview|graphs")?;
    let page = match args.next().and_then(|value| value.into_string().ok()) {
        Some(value) if value == "overview" => Page::Overview,
        Some(value) if value == "graphs" => Page::Graphs,
        _ => return Err("usage: wireview-screenshot OUTPUT.ppm overview|graphs".into()),
    };
    if args.next().is_some() {
        return Err("usage: wireview-screenshot OUTPUT.ppm overview|graphs".into());
    }

    slint::platform::set_platform(Box::new(ScreenshotPlatform))?;
    WINDOW.with(|window| {
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    });
    let app = demo_window(DemoKind::Ready, page)?;
    app.show()?;

    let mut frame = vec![Rgb8Pixel::default(); WIDTH * HEIGHT];
    WINDOW.with(|window| {
        window.request_redraw();
        if !window.draw_if_needed(|renderer| {
            renderer.render(frame.as_mut_slice(), WIDTH);
        }) {
            return Err("Slint did not render the screenshot");
        }
        Ok(())
    })?;

    let mut file = BufWriter::new(File::create(output)?);
    write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n")?;
    for pixel in frame {
        file.write_all(&[pixel.r, pixel.g, pixel.b])?;
    }
    file.flush()?;
    Ok(())
}
