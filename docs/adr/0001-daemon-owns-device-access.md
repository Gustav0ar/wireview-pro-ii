# Keep device ownership in the daemon

`wireviewd` remains the only process that opens the WireView Pro II USB serial
device. The CLI, desktop app, and shell integrations use the same typed Varlink
API because competing device owners would make reconnects, display-pause
leases, guarded flash writes, and authorization unreliable. This also keeps
clients independent of udev and serial permissions, at the cost of requiring a
running local daemon.
