// Rust port of keyd. See LICENSE for the original project's attribution.
pub mod appmapper;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod dbg;
pub mod device;
pub mod evloop;
pub mod ini;
pub mod ipc;
pub mod ipc_server;
pub mod keyboard;
pub mod keys;
pub mod linux;
pub mod log;
pub mod macros;
pub mod monitor;
pub mod runtime;
pub mod string;
pub mod unicode;
pub mod vkbd;

pub const BUILD_VERSION: &str = env!("KEYD_BUILD_VERSION");
pub const CONFIG_DIR: &str = env!("KEYD_CONFIG_DIR");
pub const DATA_DIR: &str = env!("KEYD_DATA_DIR");
pub const SOCKET_PATH: &str = env!("KEYD_SOCKET_PATH");

#[cfg(keyd_backend = "uinput")]
pub const BACKEND: vkbd::BackendKind = vkbd::BackendKind::Uinput;
#[cfg(keyd_backend = "stdout")]
pub const BACKEND: vkbd::BackendKind = vkbd::BackendKind::Stdout;
#[cfg(keyd_backend = "usb-gadget")]
pub const BACKEND: vkbd::BackendKind = vkbd::BackendKind::UsbGadget;
