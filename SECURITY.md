# Security policy

Please report vulnerabilities privately through the repository's GitHub
Security Advisory page. Do not include device serial numbers, firmware images,
or other private hardware data in a public issue.

The supported branch is the latest tagged release. Security fixes may restrict
device functionality when protocol safety or authorization is uncertain.

`wireviewd` runs as a dedicated unprivileged user. All Varlink operations,
including configuration changes, are available to local users through its Unix
socket. The `wireview` system group grants only the daemon access to the USB
device; clients never open the device directly. This means every local process
that can connect to the socket is trusted to use the normal device API.

Configuration validation, stale-revision checks, readback verification, and
rollback are enforced by the daemon rather than only by the CLI. Operations
with potentially disruptive effects also require confirmation in the Varlink
request, so bypassing the CLI does not bypass that safety boundary. Device
reboot, persistent configuration store, firmware-default reset, and selective
fault clearing are serialized with normal device access. Persistent
configuration-NVM mutations are additionally spaced by at least one second;
recovery rollback may bypass that wait so it can restore the prior state.

Arbitrary serial commands, calibration writes, bootloader entry, and firmware
updates are not exposed. Those operations remain disabled until their backup,
identity, image-authenticity, power-loss, and recovery requirements can be
verified on recoverable hardware.
