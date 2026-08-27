fn main() {
    println!("cargo:rerun-if-env-changed=WIREVIEW_BUILD_ID");
    let build_id = std::env::var("WIREVIEW_BUILD_ID").unwrap_or_else(|_| "development".into());
    println!(
        "cargo:rustc-env=WIREVIEW_GUI_VERSION={} (build {build_id})",
        std::env::var("CARGO_PKG_VERSION").expect("Cargo sets CARGO_PKG_VERSION")
    );
    slint_build::compile("ui/app.slint").expect("failed to compile the Slint interface");
}
