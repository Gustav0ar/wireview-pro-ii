# Build the desktop app with Slint and Rust

The Linux desktop client uses Slint for the declarative UI and Rust for every
runtime component. This keeps the application native, type-safe, and aligned
with the daemon without introducing a browser runtime, while accepting a
smaller widget ecosystem and an explicit Slint attribution requirement.
