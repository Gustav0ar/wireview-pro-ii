use std::time::Duration;

use tokio::time::timeout;
use wireviewd::{
    ConnectionState, DeviceError, DeviceEvent, DisconnectReason, HostEvent, MockBackend, Screen,
    config::{DeviceSettings, NvmOperation},
    history::{FLASH_LENGTH, FLASH_SECTOR_SIZE, parse_history},
    spawn_manager,
};

async fn wait_for_state(
    receiver: &mut tokio::sync::watch::Receiver<wireviewd::DaemonState>,
    predicate: impl Fn(&ConnectionState) -> bool,
) -> wireviewd::DaemonState {
    timeout(Duration::from_secs(2), async {
        loop {
            let state = receiver.borrow().clone();
            if predicate(&state.connection) {
                return state;
            }
            receiver.changed().await.expect("manager remains alive");
        }
    })
    .await
    .expect("state transition timed out")
}

#[tokio::test]
async fn store_accepts_firmware_updated_crc_when_editable_settings_match() {
    let (backend, control) = MockBackend::new();
    control.change_crc_on_store();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut requested = original.clone();
    requested.friendly_name = "CRC verification".into();
    let stored = manager
        .apply_configuration(requested.clone(), true)
        .await
        .unwrap();

    assert_ne!(stored.crc, requested.crc);
    assert_eq!(
        DeviceSettings::from_configuration(&stored),
        DeviceSettings::from_configuration(&requested)
    );
    assert_eq!(manager.reload_configuration().await.unwrap(), stored);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn physical_or_vm_detach_marks_state_stale_and_keeps_manager_alive() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    let mut events = manager.subscribe_events();

    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    let connected = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;
    assert_eq!(connected.session_id, 1);
    assert!(control.is_connected());
    assert!(!connected.telemetry.unwrap().stale);

    manager
        .observe(HostEvent::Candidates(Vec::new()))
        .await
        .unwrap();
    let absent = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Absent { .. })
    })
    .await;
    assert_eq!(
        absent.connection,
        ConnectionState::Absent {
            reason: DisconnectReason::RemovedFromHost
        }
    );
    assert!(absent.connected_port.is_none());
    assert!(absent.telemetry.unwrap().stale);
    assert!(!control.is_connected());
    assert_eq!(
        manager.set_screen(Screen::Main).await,
        Err(DeviceError::NotConnected)
    );

    let mut disconnects = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DeviceEvent::Disconnected { .. }) {
            disconnects += 1;
        }
    }
    assert_eq!(disconnects, 1);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn device_reboot_preserves_configuration_and_reconnects_as_a_new_session() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 1 })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    manager.reboot_device().await.unwrap();

    let reset = wait_for_state(&mut state_rx, |state| {
        matches!(
            state,
            ConnectionState::Absent {
                reason: DisconnectReason::DeviceReboot
            }
        )
    })
    .await;
    assert_eq!(reset.session_id, 1);
    assert_eq!(
        reset.last_disconnect_reason,
        Some(DisconnectReason::DeviceReboot)
    );
    assert!(reset.telemetry.unwrap().stale);
    assert_eq!(control.device_reboots(), 1);
    assert!(!control.is_connected());

    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 2 })
    })
    .await;
    assert_eq!(manager.read_configuration().await.unwrap(), original);
    assert_eq!(control.device_reboots(), 1);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn temporary_and_stored_configuration_follow_device_nvm_semantics() {
    let (backend, _) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    let candidates = vec!["/dev/ttyACM0".into()];
    manager
        .observe(HostEvent::Candidates(candidates.clone()))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 1 })
    })
    .await;

    let factory_defaults = manager.read_configuration().await.unwrap();
    let mut temporary = factory_defaults.clone();
    temporary.friendly_name = "temporary".into();
    manager
        .apply_configuration(temporary.clone(), false)
        .await
        .unwrap();
    assert_eq!(manager.read_configuration().await.unwrap(), temporary);

    manager.reboot_device().await.unwrap();
    manager
        .observe(HostEvent::Candidates(candidates.clone()))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 2 })
    })
    .await;
    assert_eq!(
        manager.read_configuration().await.unwrap(),
        factory_defaults
    );

    let mut permanent = factory_defaults.clone();
    permanent.friendly_name = "permanent".into();
    let stored = manager.apply_configuration(permanent, true).await.unwrap();
    manager.reboot_device().await.unwrap();
    manager
        .observe(HostEvent::Candidates(candidates))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 3 })
    })
    .await;
    assert_eq!(manager.read_configuration().await.unwrap(), stored);

    let mut second_temporary = stored.clone();
    second_temporary.friendly_name = "discard me".into();
    manager
        .apply_configuration(second_temporary, false)
        .await
        .unwrap();
    assert_eq!(manager.reload_configuration().await.unwrap(), stored);
    assert_eq!(
        manager.reset_configuration().await.unwrap(),
        factory_defaults
    );

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn vm_to_host_reattach_renumbers_port_and_starts_new_session() {
    let (backend, _) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();

    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 1 })
    })
    .await;

    manager
        .observe(HostEvent::Candidates(Vec::new()))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Absent { .. })
    })
    .await;

    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM2".into()]))
        .await
        .unwrap();
    let reattached = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 2 })
    })
    .await;
    assert_eq!(reattached.connected_port.as_deref(), Some("/dev/ttyACM2"));
    assert_eq!(reattached.telemetry.unwrap().session_id, 2);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn duplicate_devices_are_ambiguous_and_never_opened() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec![
            "/dev/ttyACM1".into(),
            "/dev/ttyACM0".into(),
            "/dev/ttyACM1".into(),
        ]))
        .await
        .unwrap();
    let state = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::AmbiguousDevice { .. })
    })
    .await;
    assert_eq!(
        state.connection,
        ConnectionState::AmbiguousDevice {
            candidates: vec!["/dev/ttyACM0".into(), "/dev/ttyACM1".into()]
        }
    );
    assert!(!control.is_connected());
    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn serial_hangup_uses_same_idempotent_disconnect_path() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_millis(10));
    let mut state_rx = manager.subscribe_state();
    let mut events = manager.subscribe_events();

    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;
    control.fail_next_read();
    let absent = wait_for_state(&mut state_rx, |state| {
        matches!(
            state,
            ConnectionState::Absent {
                reason: DisconnectReason::SerialHangup
            }
        )
    })
    .await;
    assert!(absent.telemetry.unwrap().stale);

    manager
        .observe(HostEvent::Candidates(Vec::new()))
        .await
        .unwrap();
    tokio::task::yield_now().await;
    let mut disconnects = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DeviceEvent::Disconnected { .. }) {
            disconnects += 1;
        }
    }
    assert_eq!(disconnects, 1);
    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn screen_command_is_serialized_through_manager() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;
    manager.set_screen(Screen::Current).await.unwrap();
    manager.set_screen(Screen::Temp).await.unwrap();
    assert_eq!(control.screens(), vec![Screen::Current, Screen::Temp]);
    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn device_history_reads_are_serialized_through_manager() {
    let (backend, _) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let bytes = manager
        .read_history_chunk(0, FLASH_SECTOR_SIZE)
        .await
        .unwrap();
    let parsed = parse_history(&bytes);
    assert!(parsed.end_found);
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].device_time_ms, 42);
    assert!(matches!(
        manager.state().connection,
        ConnectionState::Ready { .. }
    ));

    assert!(matches!(
        manager.read_history_chunk(FLASH_LENGTH, 1).await,
        Err(DeviceError::InvalidArgument(_))
    ));
    assert!(matches!(
        manager.state().connection,
        ConnectionState::Ready { .. }
    ));
    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn configuration_apply_reload_store_and_reset_are_serialized_through_manager() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut temporary = original.clone();
    temporary.backlight_percent = 73;
    assert_eq!(
        manager
            .apply_configuration(temporary.clone(), false)
            .await
            .unwrap(),
        temporary
    );
    assert_eq!(control.configuration(), Some(temporary));
    assert!(
        control.screens().is_empty(),
        "configuration changes must not override the current or fault screen"
    );
    assert!(control.nvm_operations().is_empty());

    assert_eq!(
        manager.reload_configuration().await.unwrap(),
        original.clone()
    );
    assert!(control.screens().is_empty());
    assert_eq!(control.nvm_operations(), vec![NvmOperation::Reload]);

    let mut permanent = original.clone();
    permanent.logging_interval_seconds = 30;
    assert_eq!(
        manager
            .apply_configuration(permanent.clone(), true)
            .await
            .unwrap(),
        permanent
    );
    assert_eq!(
        control.nvm_operations(),
        vec![NvmOperation::Reload, NvmOperation::Store]
    );
    assert_eq!(
        manager.reload_configuration().await.unwrap(),
        permanent.clone()
    );

    assert_eq!(manager.reset_configuration().await.unwrap(), original);
    assert_eq!(
        control.nvm_operations(),
        vec![
            NvmOperation::Reload,
            NvmOperation::Store,
            NvmOperation::Reload,
            NvmOperation::Reset
        ]
    );

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn configuration_writes_reject_stale_snapshots_without_mutating_the_device() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut stale = original.clone();
    stale.crc ^= 1;
    stale.backlight_percent = 50;
    assert!(matches!(
        manager.apply_configuration(stale, false).await,
        Err(DeviceError::InvalidArgument(message))
            if message.contains("configuration is stale")
    ));
    assert_eq!(control.configuration(), Some(original));
    assert!(control.nvm_operations().is_empty());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn bulk_configuration_revision_is_checked_inside_the_manager_transaction() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    let ready = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let revision = wireviewd::manager::configuration_revision(ready.session_id, &original).unwrap();
    let changed = manager
        .set_configuration_item("friendly_name".into(), "newer writer".into(), false)
        .await
        .unwrap();
    let mut stale_candidate = original;
    stale_candidate.backlight_percent = 50;

    assert!(matches!(
        manager
            .apply_configuration_if_revision(stale_candidate, false, revision)
            .await,
        Err(DeviceError::RevisionConflict(message))
            if message.contains("configuration changed")
    ));
    assert_eq!(control.configuration(), Some(changed));
    assert!(control.nvm_operations().is_empty());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn failed_configuration_write_is_rolled_back_without_display_commands() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut candidate = original.clone();
    candidate.backlight_percent = 75;
    control.fail_next_configuration_write();
    assert!(matches!(
        manager.apply_configuration(candidate, false).await,
        Err(DeviceError::FailedAndRolledBack(message))
            if message.contains("synthetic configuration write failure")
    ));
    assert_eq!(manager.read_configuration().await.unwrap(), original);
    assert!(control.screens().is_empty());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn failed_persistent_store_restores_active_and_saved_configuration() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut candidate = original.clone();
    candidate.logging_interval_seconds = 30;
    control.fail_next_nvm_operation();
    assert!(matches!(
        manager.apply_configuration(candidate, true).await,
        Err(DeviceError::FailedAndRolledBack(message))
            if message.contains("synthetic NVM operation failure")
    ));
    assert_eq!(manager.read_configuration().await.unwrap(), original);
    assert_eq!(manager.reload_configuration().await.unwrap(), original);
    assert!(control.screens().is_empty());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn rollback_failure_is_reported_distinctly() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut candidate = original;
    candidate.backlight_percent = 75;
    control.fail_configuration_writes(2);
    assert!(matches!(
        manager.apply_configuration(candidate, false).await,
        Err(DeviceError::RollbackFailed { .. })
    ));

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn history_dump_pauses_once_and_is_bound_to_its_session() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let dump = manager.begin_history_dump().await.unwrap();
    assert_eq!(dump.session_id, 1);
    assert_eq!(control.history_pause_depth(), 1);
    manager
        .read_history_dump_chunk(dump.id, 0, 64)
        .await
        .unwrap();
    manager
        .read_history_dump_chunk(dump.id, 64, 64)
        .await
        .unwrap();
    assert_eq!(control.history_pause_depth(), 1);
    manager.end_history_dump(dump.id).await.unwrap();
    assert_eq!(control.history_pause_depth(), 0);
    assert!(matches!(
        manager.read_history_dump_chunk(dump.id, 0, 1).await,
        Err(DeviceError::InvalidArgument(_))
    ));

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn debug_display_pause_is_bounded_and_does_not_break_history_ownership() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    assert!(matches!(
        manager.pause_display(99).await,
        Err(DeviceError::InvalidArgument(_))
    ));
    assert_eq!(control.history_pause_depth(), 0);

    let debug_pause = manager.pause_display(500).await.unwrap();
    assert!(debug_pause.paused);
    assert!(debug_pause.debug_lease_active);
    assert_eq!(control.history_pause_depth(), 1);

    let dump = manager.begin_history_dump().await.unwrap();
    let overlap = manager.display_pause_state().await.unwrap();
    assert!(overlap.debug_lease_active);
    assert!(overlap.history_dump_active);
    assert_eq!(control.history_pause_depth(), 1);

    let history_only = manager.resume_display().await.unwrap();
    assert!(history_only.paused);
    assert!(!history_only.debug_lease_active);
    assert!(history_only.history_dump_active);
    assert_eq!(control.history_pause_depth(), 1);

    manager.end_history_dump(dump.id).await.unwrap();
    assert!(!manager.display_pause_state().await.unwrap().paused);
    assert_eq!(control.history_pause_depth(), 0);

    manager.pause_display(100).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!manager.display_pause_state().await.unwrap().paused);
    assert_eq!(control.history_pause_depth(), 0);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn device_info_fault_clear_and_poll_interval_are_daemon_validated() {
    let (backend, control) = MockBackend::new();
    control.set_fault_masks(0x8004, 0x0024);
    let (manager, task) = spawn_manager(backend, Duration::from_millis(500));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let info = manager.read_device_info().await.unwrap();
    assert_eq!(info.build_string, "mock-build");
    assert_eq!(manager.poll_interval_ms().await.unwrap(), 500);
    assert_eq!(manager.set_poll_interval_ms(100).await.unwrap(), 100);
    assert!(matches!(
        manager.set_poll_interval_ms(99).await,
        Err(DeviceError::InvalidArgument(_))
    ));
    assert!(matches!(
        manager.clear_faults(0x8000, 0).await,
        Err(DeviceError::InvalidArgument(_))
    ));
    let telemetry = manager.clear_faults(0x0004, 0x0020).await.unwrap();
    assert_eq!(telemetry.active_fault_mask, 0x8000);
    assert_eq!(telemetry.logged_fault_mask, 0x0004);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn repeated_transient_poll_failures_mark_stale_and_disconnect() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_millis(100));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;
    control.fail_transient_telemetry_reads(3);
    let absent = wait_for_state(&mut state_rx, |state| {
        matches!(
            state,
            ConnectionState::Absent {
                reason: DisconnectReason::PresenceCheckFailed
            }
        )
    })
    .await;
    assert!(absent.telemetry.unwrap().stale);
    assert!(!control.is_connected());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn individual_setting_changes_are_atomic_validated_and_optionally_stored() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    assert_eq!(
        manager
            .read_configuration_item("friendly_name".into())
            .await
            .unwrap(),
        serde_json::to_string(&original.friendly_name).unwrap()
    );
    assert!(matches!(
        manager
            .read_configuration_item("unknown".into())
            .await,
        Err(DeviceError::InvalidArgument(message))
            if message.contains("unknown configuration key")
    ));
    assert_eq!(control.configuration(), Some(original.clone()));
    assert!(matches!(
        manager
            .set_configuration_item("backlight_percent".into(), "101".into(), false)
            .await,
        Err(DeviceError::InvalidArgument(_))
    ));
    assert_eq!(control.configuration(), Some(original.clone()));
    assert!(control.nvm_operations().is_empty());

    let temporary = manager
        .set_configuration_item("friendly_name".into(), "temporary".into(), false)
        .await
        .unwrap();
    assert_eq!(temporary.friendly_name, "temporary");
    assert_eq!(
        manager
            .read_configuration_item("friendly_name".into())
            .await
            .unwrap(),
        "\"temporary\""
    );
    assert!(control.nvm_operations().is_empty());
    assert_eq!(manager.reload_configuration().await.unwrap(), original);

    let permanent = manager
        .set_configuration_item("friendly_name".into(), "permanent".into(), true)
        .await
        .unwrap();
    assert_eq!(permanent.friendly_name, "permanent");
    assert_eq!(
        control.nvm_operations(),
        vec![NvmOperation::Reload, NvmOperation::Store]
    );
    assert_eq!(manager.reload_configuration().await.unwrap(), permanent);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn daemon_rejects_invalid_configuration_before_the_backend_is_mutated() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut invalid = original.clone();
    invalid.backlight_percent = 101;
    assert!(matches!(
        manager.apply_configuration(invalid, false).await,
        Err(DeviceError::InvalidArgument(message))
            if message.contains("backlight_percent")
    ));
    assert_eq!(control.configuration(), Some(original));
    assert!(control.nvm_operations().is_empty());

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn invalid_saved_configuration_is_rolled_back_without_leaving_device_unsafe() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    manager
        .observe(HostEvent::Candidates(vec!["/dev/ttyACM0".into()]))
        .await
        .unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { .. })
    })
    .await;

    let original = manager.read_configuration().await.unwrap();
    let mut invalid_saved = original.clone();
    invalid_saved.raw_version = u8::MAX;
    control.set_saved_configuration(invalid_saved);

    assert!(matches!(
        manager.reload_configuration().await,
        Err(DeviceError::InvalidArgument(message))
            if message.contains("previous active settings were restored")
    ));
    assert_eq!(control.configuration(), Some(original));
    assert_eq!(control.nvm_operations(), vec![NvmOperation::Reload]);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn unchanged_candidate_reconnects_after_transient_serial_loss() {
    let (backend, control) = MockBackend::new();
    let (manager, task) = spawn_manager(backend, Duration::from_millis(10));
    let mut state_rx = manager.subscribe_state();
    let candidates = HostEvent::Candidates(vec!["/dev/ttyACM0".into()]);

    manager.observe(candidates.clone()).await.unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 1 })
    })
    .await;
    control.fail_next_read();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Absent { .. })
    })
    .await;

    manager.observe(candidates).await.unwrap();
    let state = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 2 })
    })
    .await;
    assert_eq!(state.connected_port.as_deref(), Some("/dev/ttyACM0"));

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn failed_connections_are_backed_off_before_retrying() {
    let (backend, control) = MockBackend::new();
    control.fail_next_connect();
    let (manager, task) = spawn_manager(backend, Duration::from_secs(60));
    let mut state_rx = manager.subscribe_state();
    let candidates = HostEvent::Candidates(vec!["/dev/ttyACM0".into()]);

    manager.observe(candidates.clone()).await.unwrap();
    let recovering = wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Recovering { attempt: 1, .. })
    })
    .await;
    assert!(matches!(
        recovering.connection,
        ConnectionState::Recovering { attempt: 1, .. }
    ));
    assert_eq!(control.connection_attempts(), 1);

    manager.observe(candidates.clone()).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(control.connection_attempts(), 1);

    tokio::time::sleep(Duration::from_millis(275)).await;
    manager.observe(candidates).await.unwrap();
    wait_for_state(&mut state_rx, |state| {
        matches!(state, ConnectionState::Ready { session_id: 1 })
    })
    .await;
    assert_eq!(control.connection_attempts(), 2);

    manager.observe(HostEvent::Shutdown).await.unwrap();
    task.await.unwrap();
}
