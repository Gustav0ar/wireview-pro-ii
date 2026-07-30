use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, SystemTime},
};

struct TestDaemon {
    child: Child,
    root: PathBuf,
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wireview(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wireview"))
        .args(arguments)
        .output()
        .expect("wireview should run")
}

fn wireviewd(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wireviewd"))
        .args(arguments)
        .output()
        .expect("wireviewd should run")
}

#[test]
fn screen_help_forms_list_every_mode_without_a_daemon() {
    for arguments in [
        &["screen"][..],
        &["screen", "help"][..],
        &["screen", "--help"][..],
    ] {
        let output = wireview(arguments);
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
        for mode in [
            "main",
            "current",
            "temp",
            "temperature",
            "status",
            "simple",
            "same",
        ] {
            assert!(stdout.contains(mode), "{arguments:?} omitted {mode:?}");
        }
    }
}

#[test]
fn both_version_forms_work_without_a_daemon() {
    for arguments in [&["--version"][..], &["version"][..]] {
        let output = wireview(arguments);
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        let stdout = String::from_utf8(output.stdout).expect("version should be UTF-8");
        assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn daemon_and_cli_report_the_same_release_identity() {
    let cli = wireview(&["version"]);
    let daemon = wireviewd(&["--version"]);
    assert!(cli.status.success(), "{cli:?}");
    assert!(daemon.status.success(), "{daemon:?}");

    let cli = String::from_utf8(cli.stdout).expect("version should be UTF-8");
    let daemon = String::from_utf8(daemon.stdout).expect("version should be UTF-8");
    assert_eq!(daemon, cli.replacen("wireview ", "wireviewd ", 1));
    assert!(daemon.contains("(build "));
}

#[test]
fn packaged_cli_assets_are_generated_from_the_actual_command_tree() {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("wireview-assets-{}-{unique}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_wireview"))
        .arg("__generate-assets")
        .arg(&root)
        .output()
        .expect("asset generation should run");
    assert!(output.status.success(), "{output:?}");

    for path in [
        "usr/share/man/man1/wireview.1",
        "usr/share/bash-completion/completions/wireview",
        "usr/share/zsh/site-functions/_wireview",
        "usr/share/fish/vendor_completions.d/wireview.fish",
    ] {
        let contents =
            fs::read_to_string(root.join(path)).expect("generated asset should be UTF-8");
        assert!(
            contents.contains("wireview"),
            "{path} omitted the command name"
        );
        assert!(
            !contents.contains("__generate-assets"),
            "{path} exposed the packaging-only command"
        );
    }
    fs::remove_dir_all(root).expect("asset fixture should be removed");
}

#[test]
fn connection_failures_do_not_use_rust_debug_error_formatting() {
    let output = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "status",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(!stderr.starts_with("Error:"));
    assert!(!stderr.contains("Custom {"));
}

#[test]
fn monitor_is_documented_under_debug_only() {
    let top_level = wireview(&["--help"]);
    assert!(top_level.status.success());
    let top_level_help = String::from_utf8(top_level.stdout).expect("help should be UTF-8");
    assert!(top_level_help.contains("debug"));
    assert!(top_level_help.contains("diagnostics, scripts, and integrations"));
    assert!(!top_level_help.contains("\n  monitor"));

    let debug = wireview(&["debug", "--help"]);
    assert!(debug.status.success());
    let debug_help = String::from_utf8(debug.stdout).expect("help should be UTF-8");
    assert!(debug_help.contains("monitor"));
    assert!(debug_help.contains("reboot-device"));
    assert!(debug_help.contains("factory-reset"));
    assert!(debug_help.contains("raw JSON daemon events"));
    assert!(debug_help.contains("diagnostics and integrations"));

    let monitor = wireview(&["debug", "monitor", "--help"]);
    assert!(monitor.status.success());
    let monitor_help = String::from_utf8(monitor.stdout).expect("help should be UTF-8");
    assert!(monitor_help.contains("--count"));
    assert!(monitor_help.contains("zero keeps monitoring"));

    let old_command = wireview(&["monitor"]);
    assert!(!old_command.status.success());
    let old_error = String::from_utf8(old_command.stderr).expect("error should be UTF-8");
    assert!(old_error.contains("unrecognized subcommand 'monitor'"));
}

#[test]
fn device_reboot_is_guarded_and_documented_as_non_destructive_recovery() {
    let help = wireview(&["debug", "reboot-device", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    assert!(help.contains("--yes"));
    assert!(help.contains("Reboot the device controller"));
    assert!(help.contains("without erasing stored configuration"));

    let reset = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "debug",
        "reboot-device",
    ]);
    assert!(!reset.status.success());
    assert_eq!(
        String::from_utf8(reset.stderr).expect("error should be UTF-8"),
        "device reboot requires --yes\n"
    );
}

#[test]
fn debug_factory_reset_is_guarded_and_documented_as_permanent() {
    let help = wireview(&["debug", "factory-reset", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    assert!(help.contains("--yes"));
    assert!(help.contains("Permanently replace stored settings"));
    assert!(help.contains("firmware defaults"));

    let reset = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "debug",
        "factory-reset",
    ]);
    assert!(!reset.status.success());
    assert_eq!(
        String::from_utf8(reset.stderr).expect("error should be UTF-8"),
        "factory reset requires --yes\n"
    );
}

#[test]
fn interface_is_not_exposed_as_a_user_command() {
    let help = wireview(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    assert!(!help.contains("\n  interface"));

    let output = wireview(&["interface"]);
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(error.contains("unrecognized subcommand 'interface'"));
}

#[test]
fn telemetry_watch_is_documented_and_excludes_json() {
    let help = wireview(&["telemetry", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    assert!(help.contains("--watch"));
    assert!(help.contains("Refresh the readable display in place"));
    assert!(help.contains("Ctrl+C"));

    let conflict = wireview(&["telemetry", "--watch", "--json"]);
    assert!(!conflict.status.success());
    let error = String::from_utf8(conflict.stderr).expect("error should be UTF-8");
    assert!(error.contains("cannot be used with"));
}

#[test]
fn history_dump_formats_and_output_are_documented() {
    let top_level = wireview(&["--help"]);
    assert!(top_level.status.success());
    let top_level = String::from_utf8(top_level.stdout).expect("help should be UTF-8");
    assert!(top_level.contains("history"));
    assert!(top_level.contains("telemetry history stored in the WireView device"));

    let help = wireview(&["history", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    assert!(help.contains("--format"));
    assert!(help.contains("table"));
    assert!(help.contains("csv"));
    assert!(help.contains("json"));
    assert!(help.contains("raw"));
    assert!(help.contains("--output"));
    assert!(help.contains("Ctrl+C"));
    assert!(help.contains("closes the daemon-side dump session"));
    assert!(help.contains("10-second safety lease"));
}

#[test]
fn ctrl_c_closes_an_active_history_dump_immediately() {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "wireview-history-cancel-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("temporary test directory should be created");
    let socket = root.join("wireview.sock");
    let output_path = root.join("history.raw");
    let daemon = TestDaemon {
        child: Command::new(env!("CARGO_BIN_EXE_wireviewd"))
            .args([
                "--mock",
                "--socket",
                socket
                    .to_str()
                    .expect("temporary socket path should be UTF-8"),
                "--discovery-ms",
                "10",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("mock daemon should start"),
        root,
    };

    let status = || {
        Command::new(env!("CARGO_BIN_EXE_wireview"))
            .args([
                "--socket",
                socket
                    .to_str()
                    .expect("temporary socket path should be UTF-8"),
                "status",
            ])
            .output()
    };
    let mut ready = false;
    for _ in 0..200 {
        if let Ok(output) = status()
            && output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("state=ready")
        {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready, "mock daemon did not become ready");

    fs::write(&output_path, b"existing history export")
        .expect("existing destination fixture should be written");
    let mut history = Command::new(env!("CARGO_BIN_EXE_wireview"))
        .args([
            "--socket",
            socket
                .to_str()
                .expect("temporary socket path should be UTF-8"),
            "history",
            "--format",
            "raw",
            "--output",
            output_path
                .to_str()
                .expect("temporary output path should be UTF-8"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("history client should start");

    let mut dump_started = false;
    for _ in 0..200 {
        if let Ok(output) = status()
            && output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("display_paused=true")
        {
            dump_started = true;
            break;
        }
        assert!(
            history
                .try_wait()
                .expect("history process status should be readable")
                .is_none(),
            "history dump finished before it could be interrupted"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(dump_started, "history dump did not become active");

    let signal = Command::new("kill")
        .args(["-INT", &history.id().to_string()])
        .status()
        .expect("SIGINT should be sent");
    assert!(signal.success(), "SIGINT command failed");

    let mut exited = false;
    for _ in 0..200 {
        if history
            .try_wait()
            .expect("history process status should be readable")
            .is_some()
        {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(exited, "history client did not exit after SIGINT");
    let output = history
        .wait_with_output()
        .expect("history client output should be collected");
    assert_eq!(output.status.code(), Some(130));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("History dump interrupted; daemon state cleaned up.")
    );
    assert_eq!(
        fs::read(&output_path).expect("existing destination should remain readable"),
        b"existing history export",
        "the interrupted dump replaced the existing destination"
    );

    let status = status().expect("post-cancellation daemon status should be readable");
    assert!(status.status.success());
    let status = String::from_utf8(status.stdout).expect("status should be UTF-8");
    assert!(status.contains("state=ready"), "{status}");
    assert!(status.contains("display_paused=false"), "{status}");

    // Exercise Drop explicitly before assertions in another test can observe
    // the process or socket.
    drop(daemon);
}

#[test]
fn device_fault_and_poll_commands_are_documented_and_guarded() {
    let top = wireview(&["--help"]);
    let top = String::from_utf8(top.stdout).expect("help should be UTF-8");
    assert!(top.contains("\n  info"));
    assert!(top.contains("identity, firmware, build, and capabilities"));
    assert!(top.contains("faults"));

    let info = wireview(&["info", "--help"]);
    assert!(info.status.success());
    let info = String::from_utf8(info.stdout).expect("help should be UTF-8");
    assert!(info.contains("identity, firmware, build, and capabilities"));
    assert!(info.contains("--json"));

    let old_device = wireview(&["device", "info"]);
    assert!(!old_device.status.success());
    let old_device = String::from_utf8(old_device.stderr).expect("error should be UTF-8");
    assert!(old_device.contains("unrecognized subcommand 'device'"));

    let faults = wireview(&["faults", "clear", "--help"]);
    assert!(faults.status.success());
    let faults = String::from_utf8(faults.stdout).expect("help should be UTF-8");
    assert!(faults.contains("active"));
    assert!(faults.contains("logged"));
    assert!(faults.contains("--yes"));

    let rejected = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "faults",
        "clear",
        "logged",
    ]);
    assert!(!rejected.status.success());
    assert_eq!(
        String::from_utf8(rejected.stderr).expect("error should be UTF-8"),
        "clearing device faults requires --yes\n"
    );

    let poll = wireview(&["debug", "poll-interval", "--help"]);
    assert!(poll.status.success());
    let poll = String::from_utf8(poll.stdout).expect("help should be UTF-8");
    assert!(poll.contains("100..=5000"));

    let pause = wireview(&["debug", "pause-display", "--help"]);
    assert!(pause.status.success());
    let pause = String::from_utf8(pause.stdout).expect("help should be UTF-8");
    assert!(pause.contains("1..=300"));
    assert!(pause.contains("diagnostics"));

    let resume = wireview(&["debug", "resume-display", "--help"]);
    assert!(resume.status.success());
    let resume = String::from_utf8(resume.stdout).expect("help should be UTF-8");
    assert!(resume.contains("history dumps remain paused"));
}

#[test]
fn configuration_commands_and_safety_are_documented() {
    let top_level = wireview(&["--help"]);
    assert!(top_level.status.success());
    let top_level = String::from_utf8(top_level.stdout).expect("help should be UTF-8");
    assert!(top_level.contains("config"));
    assert!(top_level.contains("Read or change device configuration"));

    let help = wireview(&["config", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help should be UTF-8");
    for command in ["show", "get", "set", "apply", "store", "reload", "reset"] {
        assert!(help.contains(command), "config help omitted {command:?}");
    }
    assert!(help.contains("--json"));
    assert!(help.contains("factory defaults in parentheses"));
    assert!(help.contains("Up to 32 printable ASCII characters"));
    assert!(!help.contains("32 bytes"));
    assert!(help.contains("fan.temperature_source"));
    assert!(help.contains("fan.temperature_min_c"));
    assert!(help.contains("0.0..50.0"));
    assert!(help.contains("fan.temperature_max_c"));
    assert!(help.contains("50.0..100.0"));
    assert!(help.contains("fault_thresholds.temperature_c"));
    assert!(help.contains("0.0..120.0"));
    assert!(help.contains("0..150 (55)"));
    assert!(help.contains("0..2000 (660)"));
    assert!(help.contains("0..10 (6)"));
    assert!(help.contains("chip_over_temperature"));
    assert!(help.contains("thermal_grizzly_orange"));
    assert!(help.contains("6 RGB or 8 ARGB hex characters"));
    assert!(help.contains("FFFFFF"));
    assert!(help.contains("000000"));
    assert!(help.contains("Do not use # or 0x"));
    assert!(help.contains("reload, reboot, or power loss reverts them"));
    assert!(help.contains("save them permanently"));
    assert!(!help.contains("raw_version"));
    assert!(!help.contains("\n  crc"));

    let apply_help = wireview(&["config", "apply", "--help"]);
    assert!(apply_help.status.success());
    let apply_help = String::from_utf8(apply_help.stdout).expect("help should be UTF-8");
    assert!(apply_help.contains("factory defaults in parentheses"));
    assert!(apply_help.contains("fault_thresholds"));

    let store_help = wireview(&["config", "store", "--help"]);
    assert!(store_help.status.success());
    let store_help = String::from_utf8(store_help.stdout).expect("help should be UTF-8");
    assert!(store_help.contains("--yes"));
    assert!(store_help.contains("permanently"));

    let set_help = wireview(&["config", "set", "--help"]);
    assert!(set_help.status.success());
    let set_help = String::from_utf8(set_help.stdout).expect("help should be UTF-8");
    assert!(set_help.contains("<KEY>"));
    assert!(set_help.contains("<VALUE>"));
    assert!(set_help.contains("fan.mode"));
    assert!(set_help.contains("comma-separated"));
    assert!(set_help.contains("--store"));
    assert!(set_help.contains("--yes"));

    let get_help = wireview(&["config", "get", "--help"]);
    assert!(get_help.status.success());
    let get_help = String::from_utf8(get_help.stdout).expect("help should be UTF-8");
    assert!(get_help.contains("<KEY>"));
    assert!(get_help.contains("fan.mode"));

    let reset_help = wireview(&["config", "reset", "--help"]);
    assert!(reset_help.status.success());
    let reset_help = String::from_utf8(reset_help.stdout).expect("help should be UTF-8");
    assert!(reset_help.contains("--yes"));
    assert!(reset_help.contains("firmware defaults"));
}

#[test]
fn destructive_configuration_commands_require_confirmation_before_connecting() {
    let missing_file = "/tmp/wireview-configuration-that-does-not-exist.json";
    let store = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "config",
        "store",
        missing_file,
    ]);
    assert!(!store.status.success());
    assert_eq!(
        String::from_utf8(store.stderr).expect("error should be UTF-8"),
        "permanent storage requires --yes\n"
    );

    let reset = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "config",
        "reset",
    ]);
    assert!(!reset.status.success());
    assert_eq!(
        String::from_utf8(reset.stderr).expect("error should be UTF-8"),
        "factory reset requires --yes\n"
    );

    let set = wireview(&[
        "--socket",
        "/tmp/wireviewd-definitely-not-running.sock",
        "config",
        "set",
        "friendly_name",
        "permanent",
        "--store",
    ]);
    assert!(!set.status.success());
    assert_eq!(
        String::from_utf8(set.stderr).expect("error should be UTF-8"),
        "permanent item storage requires --yes\n"
    );
}
