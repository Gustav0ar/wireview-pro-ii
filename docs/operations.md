# Operations

## Service management

Packages install both the daemon and CLI. Enable socket activation once:

```bash
sudo systemctl enable --now wireviewd.socket
systemctl status wireviewd.socket
```

The socket starts `wireviewd.service` on the first CLI request. Useful checks:

```bash
wireview status
systemctl status wireviewd.service wireviewd.socket
journalctl -u wireviewd.service
```

All local users may connect to the Varlink socket. Only the dedicated daemon
account opens the USB serial device. There is no client group-membership or
polkit requirement, so any local process can invoke validated device writes.

## USB removal and VM passthrough

The daemon treats physical removal and USB reassignment to a VM as a normal
disconnect. CLI reads report that no device is connected, active history and
display-pause leases are cleaned up, and the daemon continues scanning. When
the device returns to the host, it is validated again and receives a new
session identifier.

Do not let more than one host application access the device at the same time.
Detach it from the host before assigning it to a VM, and detach it from the VM
before returning it.

## Troubleshooting

Start with:

```bash
wireview status
wireview info
wireview faults
journalctl -u wireviewd.service -n 100 --no-pager
```

If the display is blank or the controller is unresponsive but the daemon can
still communicate:

```bash
wireview debug reboot-device --yes
```

This preserves saved configuration. Use `wireview debug factory-reset --yes`
only when you intend to permanently replace configuration with firmware
defaults.

If USB access fails, confirm the device appears as `0483:5740`, reload the
packaged udev rules, and reconnect it:

```bash
lsusb -d 0483:5740
sudo udevadm control --reload-rules
```

## Removal

```bash
# Debian or Ubuntu
sudo apt remove wireviewd

# Fedora
sudo dnf remove wireviewd

# Arch Linux
sudo pacman -R wireviewd
```

Removal leaves the package-owned system user and group in place so their
numeric identities are not accidentally reused.
