use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

// The keycode tables and the VIA/`KeyAction` bridge are part of the pure
// configuration model, so the CLI reaches them through their old module paths.
use glove80_config::{keycodes, rynk_keycode};

mod config;
mod connection;
mod keymap;
mod lighting;
mod rynk_client;
mod rynk_hid;
mod transport;
mod version;

#[derive(Parser)]
#[command(about = "Control Glove80 keymaps, lighting, firmware, and bootloaders over Rynk")]
struct Cli {
    /// Device to use: a /dev/hidraw* path, older Rynk /dev/ttyACM* path, or BLE address.
    #[arg(long, global = true)]
    device: Option<PathBuf>,

    /// Require the USB Rynk transport.
    #[arg(long, global = true, conflicts_with = "ble")]
    usb: bool,

    /// Require the BLE Rynk transport. Auto-selection prefers USB.
    #[arg(long, global = true)]
    ble: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate, compare, pull, or apply managed runtime configuration.
    Config {
        #[command(subcommand)]
        command: config::ConfigCommand,
    },
    /// Reboot either half into its UF2 bootloader through Rynk.
    Bootloader {
        #[command(flatten)]
        options: lighting::BootloaderArgs,
    },
    /// Control topology-aware RMK lighting.
    Lighting {
        #[command(subcommand)]
        command: lighting::LightingCommand,
    },
    /// Read and drive the transports and BLE slots.
    Connection {
        #[command(subcommand)]
        command: connection::ConnectionCommand,
    },
    /// Read and edit the live keymap.
    Keymap {
        #[command(subcommand)]
        command: keymap::KeymapCommand,
    },
    /// Show whether host maintenance operations are currently allowed.
    Maintenance,
    /// Show the CLI, firmware, RMK, and Rynk versions.
    Version,
}

fn selector(cli: &Cli) -> transport::Selector {
    let preference = if cli.usb {
        transport::Preference::Usb
    } else if cli.ble {
        transport::Preference::Ble
    } else {
        transport::Preference::Auto
    };
    transport::Selector {
        preference,
        device: cli
            .device
            .as_ref()
            .map(|device| device.to_string_lossy().into_owned()),
    }
}

fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Config { command } => config::run(&selector(&cli), command),
        Command::Connection { command } => connection::run(&selector(&cli), command),
        Command::Lighting { command } => lighting::run(&selector(&cli), command),
        Command::Keymap { command } => keymap::run(&selector(&cli), command),
        Command::Maintenance => rynk_client::run_maintenance(&selector(&cli)),
        Command::Version => version::run(&selector(&cli)),
        Command::Bootloader { options } => {
            lighting::run_bootloader(&selector(&cli), options.peripheral, options.yes)
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        if error.downcast_ref::<config::DiffFound>().is_none() {
            eprintln!("error: {error:#}");
        }
        std::process::exit(1);
    }
}
