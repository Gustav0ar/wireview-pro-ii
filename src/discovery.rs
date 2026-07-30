use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::domain::DeviceError;
use crate::manager::{HostEvent, ManagerHandle};

pub const VENDOR_ID: &str = "0483";
pub const PRODUCT_ID: &str = "5740";

pub fn spawn_discovery(
    manager: ManagerHandle,
    interval: Duration,
) -> JoinHandle<Result<(), DeviceError>> {
    tokio::spawn(async move {
        loop {
            let candidates = tokio::task::spawn_blocking(scan_matching_ttys)
                .await
                .map_err(|error| DeviceError::Transport(error.to_string()))?
                .map_err(|error| DeviceError::Transport(error.to_string()))?;
            // Repeat the current candidate set so the manager can reconnect
            // after a transient serial hangup even when udev identity did not
            // change.
            manager
                .observe(HostEvent::Candidates(candidates.into_iter().collect()))
                .await?;
            tokio::time::sleep(interval).await;
        }
    })
}

pub fn scan_matching_ttys() -> std::io::Result<BTreeSet<String>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("tty")?;
    let mut result = BTreeSet::new();
    for device in enumerator.scan_devices()? {
        let Some(node) = device.devnode() else {
            continue;
        };
        if usb_ids_match(&device) {
            result.insert(node.to_string_lossy().into_owned());
        }
    }
    Ok(result)
}

fn usb_ids_match(device: &udev::Device) -> bool {
    let mut current = Some(device.clone());
    while let Some(node) = current {
        let vendor = node
            .attribute_value("idVendor")
            .map(|value| value.to_string_lossy().to_ascii_lowercase());
        let product = node
            .attribute_value("idProduct")
            .map(|value| value.to_string_lossy().to_ascii_lowercase());
        if vendor.as_deref() == Some(VENDOR_ID) && product.as_deref() == Some(PRODUCT_ID) {
            return true;
        }
        current = node.parent();
    }
    false
}

#[must_use]
pub fn mock_port() -> PathBuf {
    PathBuf::from("/dev/wireview-mock")
}
