use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};
use wireview_gui::{AppData, MainWindow};

const WIDTH: usize = 1440;
const HEIGHT: usize = 900;

thread_local! {
    static WINDOW: Rc<MinimalSoftwareWindow> =
        MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
}

struct TestPlatform;

impl slint::platform::Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(WINDOW.with(Clone::clone))
    }
}

fn click(window: &slint::Window, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn key(window: &slint::Window, text: &str) {
    window.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
}

fn render() -> Vec<Rgb8Pixel> {
    let mut frame = vec![Rgb8Pixel::default(); WIDTH * HEIGHT];
    WINDOW.with(|window| {
        window.request_redraw();
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(frame.as_mut_slice(), WIDTH);
        }));
    });
    frame
}

fn region(frame: &[Rgb8Pixel], x: usize, y: usize, width: usize, height: usize) -> Vec<Rgb8Pixel> {
    (y..y + height)
        .flat_map(|row| {
            frame[row * WIDTH + x..row * WIDTH + x + width]
                .iter()
                .copied()
        })
        .collect()
}

#[test]
fn controls_accept_input_and_update_rendered_state() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let app = MainWindow::new().unwrap();
    WINDOW.with(|window| {
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    });
    app.show().unwrap();
    app.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));

    let data = app.global::<AppData>();
    assert_eq!(data.get_page(), 0);
    let overview = render();

    click(app.window(), 100.0, 612.0);
    assert_eq!(data.get_page(), 1, "the Pins navigation item was not hit");
    assert_ne!(overview, render(), "navigation did not redraw the page");

    click(app.window(), 1_300.0, 320.0);
    assert_eq!(
        data.get_page(),
        2,
        "the protection rail action button was not hit"
    );

    data.set_screen_choice("main".into());
    click(app.window(), 1_300.0, 426.0);
    assert_eq!(
        data.get_screen_choice(),
        "simple",
        "the screen cycle field was not hit"
    );
    key(app.window(), "\n");
    assert_eq!(
        data.get_screen_choice(),
        "current",
        "the focused cycle field ignored keyboard activation"
    );

    let set_screen_count = Rc::new(Cell::new(0));
    let selected_screen = Rc::new(RefCell::new(String::new()));
    let count = set_screen_count.clone();
    let selected = selected_screen.clone();
    data.on_set_screen(move |screen| {
        count.set(count.get() + 1);
        *selected.borrow_mut() = screen.to_string();
    });

    data.set_device_ready(false);
    render();
    click(app.window(), 1_300.0, 471.0);
    assert_eq!(
        set_screen_count.get(),
        0,
        "a disabled action button accepted pointer input"
    );

    data.set_device_ready(true);
    render();
    click(app.window(), 1_300.0, 471.0);
    assert_eq!(
        set_screen_count.get(),
        1,
        "an enabled action button ignored pointer input"
    );
    assert_eq!(selected_screen.borrow().as_str(), "current");

    data.set_page(5);
    render();
    click(app.window(), 300.0, 175.0);
    assert_eq!(
        data.get_selected_theme_slot(),
        "background-orange",
        "the theme slot row was not hit"
    );
    assert_eq!(data.get_theme_path(), "background-orange.rgb565");

    data.set_theme_path("".into());
    render();
    click(app.window(), 800.0, 524.0);
    key(app.window(), "x");
    assert_eq!(
        data.get_theme_path(),
        "x",
        "the theme path text input did not accept keyboard input"
    );

    data.set_page(4);
    data.set_config_loaded(true);
    data.set_config_dirty(false);
    data.set_fan_mode("curve".into());
    render();
    click(app.window(), 793.0, 220.0);
    assert_eq!(
        data.get_fan_mode(),
        "fixed",
        "a cycle field inside the configuration scroll view was not hit"
    );
    assert!(
        data.get_config_dirty(),
        "editing a configuration selector did not mark it dirty"
    );

    data.set_page(0);
    data.set_confirm_kind("reboot".into());
    render();
    click(app.window(), 100.0, 612.0);
    assert_eq!(
        data.get_page(),
        0,
        "the confirmation overlay leaked input to controls behind it"
    );

    click(app.window(), 790.0, 524.0);
    assert_eq!(
        data.get_confirm_kind(),
        "",
        "the confirmation dialog cancel button was not hit"
    );

    data.set_connection_detail("FIRST CONNECTION DETAIL".into());
    let first_connection_detail = region(&render(), 1_180, 590, 240, 70);
    data.set_connection_detail("SECOND CONNECTION DETAIL".into());
    assert!(
        first_connection_detail != region(&render(), 1_180, 590, 240, 70),
        "the connection failure detail is not rendered"
    );

    let selected_graph_kind = Rc::new(RefCell::new(String::new()));
    let selected = selected_graph_kind.clone();
    data.on_select_graph_kind(move |kind| {
        *selected.borrow_mut() = kind.to_string();
    });
    let toggled_series = Rc::new(Cell::new(-1));
    let toggled = toggled_series.clone();
    data.on_toggle_graph_series(move |index| toggled.set(index));

    data.set_page(7);
    data.set_graph_sample_count(61);
    data.set_graph_range_label("AUTO / 0.00 TO 1.20 A".into());
    data.set_graph_y_max_label("1.20".into());
    data.set_graph_y_mid_label("0.60".into());
    data.set_graph_y_min_label("0.00".into());
    data.set_graph_summary_value("5.75 A".into());
    data.set_graph_buffer_label("61 / 1200".into());
    data.set_graph_path_1(
        "M0 500 L120 470 L240 510 L360 430 L480 460 L600 390 L720 420 L840 350 L1000 380".into(),
    );
    data.set_graph_path_2(
        "M0 640 L120 610 L240 650 L360 580 L480 600 L600 550 L720 570 L840 510 L1000 530".into(),
    );
    data.set_graph_path_3(
        "M0 710 L120 680 L240 720 L360 650 L480 670 L600 610 L720 640 L840 580 L1000 600".into(),
    );
    data.set_graph_path_4(
        "M0 450 L120 420 L240 470 L360 380 L480 420 L600 330 L720 370 L840 280 L1000 320".into(),
    );
    data.set_graph_path_5(
        "M0 560 L120 530 L240 570 L360 500 L480 520 L600 470 L720 490 L840 430 L1000 450".into(),
    );
    data.set_graph_path_6(
        "M0 760 L120 730 L240 770 L360 700 L480 720 L600 670 L720 690 L840 630 L1000 650".into(),
    );
    data.set_graph_series_1_visible(true);
    let graph = render();
    assert!(
        region(&graph, 1_200, 278, 196, 548).contains(&Rgb8Pixel::new(67, 199, 122)),
        "the graph did not use the width previously occupied by the protection rail"
    );

    click(app.window(), 470.0, 155.0);
    assert_eq!(
        selected_graph_kind.borrow().as_str(),
        "voltage",
        "the graph measurement selector ignored pointer input"
    );
    click(app.window(), 280.0, 245.0);
    assert_eq!(
        toggled_series.get(),
        0,
        "the graph series selector ignored pointer input"
    );

    WINDOW.with(|window| {
        window.set_size(PhysicalSize::new(1_120, 720));
    });
    render();
    click(app.window(), 100.0, 703.0);
    assert_eq!(
        data.get_page(),
        6,
        "the last navigation item was clipped at the minimum window height"
    );
}
