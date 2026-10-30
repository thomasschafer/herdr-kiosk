//! herdr-kiosk: kiosk's repository and branch picker for the herdr TUI (as a plugin) and
//! the Herdr desktop app (through its `kiosk` mod). The picker itself comes from
//! `kiosk-core` and `kiosk-tui`; this crate holds the two herdr backends, the config they
//! add and how kiosk finds out which herdr started it.

pub mod config;
pub mod context;
pub mod herdr;
pub mod launch;

use kiosk_core::product::Product;

pub const PRODUCT: Product = Product {
    name: "herdr-kiosk",
    config_dir_var: Some("HERDR_PLUGIN_CONFIG_DIR"),
    state_dir_var: Some("HERDR_PLUGIN_STATE_DIR"),
    backend_name: "Herdr",
    no_backend_message: "not running inside herdr",
    setup_intro: "Pick repositories and branches without leaving herdr.",
};
