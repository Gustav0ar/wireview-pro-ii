# WireView Pro II

This context describes the measurements, state, and guarded operations exposed
for one Thermal Grizzly WireView Pro II power monitor.

## Hardware and measurements

**Device**:
A supported WireView Pro II attached to the Linux host.
_Avoid_: Meter, controller, peripheral

**Power conductor**:
One of the six 12 V paths measured independently by the device.
_Avoid_: Pin, rail, wire when referring to one measured path

**Telemetry sample**:
One coherent reading of voltage, current, power, temperature, fan, and fault
state from the device.
_Avoid_: Snapshot, measurement packet

**Device session**:
One continuous attachment of a device, ending when it disconnects or is
re-enumerated.
_Avoid_: Connection, run, instance

## Configuration

**Active configuration**:
The settings currently used by the device, including temporary changes.
_Avoid_: Current config, live config

**Stored configuration**:
The settings retained in device nonvolatile memory and loaded after reboot or
power loss.
_Avoid_: Saved config, permanent config

**Factory configuration**:
The firmware-defined settings written by a factory reset.
_Avoid_: Default config, original config

## Protection and storage

**Active fault**:
A protection condition currently asserted by the device.
_Avoid_: Current error, live alarm

**Recorded fault**:
A protection condition retained by the device after the triggering condition
may have ended.
_Avoid_: Logged error, fault history

**History dump**:
A leased read of the complete device logging region while display updates are
temporarily paused.
_Avoid_: Log download, history export

**Theme asset slot**:
One fixed, named RGB565 bitmap region used by the device display.
_Avoid_: Flash address, image file, theme
