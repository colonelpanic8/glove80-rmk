use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod keycodes;
mod keymap;
mod lighting;
mod rynk_client;
mod rynk_hid;
mod rynk_keycode;
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
    /// Read and edit the live keymap.
    Keymap {
        #[command(subcommand)]
        command: keymap::KeymapCommand,
    },
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
        Command::Lighting { command } => lighting::run(&selector(&cli), command),
        Command::Keymap { command } => keymap::run(&selector(&cli), command),
        Command::Version => version::run(&selector(&cli)),
        Command::Bootloader { options } => {
            lighting::run_bootloader(&selector(&cli), options.peripheral, options.yes)
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
