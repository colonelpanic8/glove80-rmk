//! `version` verb: report the CLI, Rynk protocol, application-defined firmware
//! identity, and structured RMK version without conflating their semantics.

use anyhow::Result;
use rynk::rmk_types::protocol::rynk::{BuildInfo, DeviceInfo, ProtocolVersion};

use crate::transport::Selector;

const CLI_GIT_HASH: &str = env!("GLOVE80_GIT_HASH");
const CLI_GIT_DIRTY: bool = {
    let s = env!("GLOVE80_GIT_DIRTY").as_bytes();
    s.len() == 1 && s[0] == b'1'
};

pub(crate) fn render(protocol: ProtocolVersion, device: &DeviceInfo, build: &BuildInfo) -> String {
    let cli_dirty = if CLI_GIT_DIRTY { "-dirty" } else { "" };
    format!(
        "glove80-control v{} ({CLI_GIT_HASH}{cli_dirty})\n\
         Rynk protocol: v{}.{}\n\
         firmware: {}\n\
         RMK: v{}.{}.{}\n\
         device: {} {} (USB {:04x}:{:04x})\n\
         serial: {}\n",
        env!("CARGO_PKG_VERSION"),
        protocol.major,
        protocol.minor,
        build.label,
        device.rmk_version.major,
        device.rmk_version.minor,
        device.rmk_version.patch,
        device.manufacturer,
        device.product_name,
        device.vendor_id,
        device.product_id,
        device.serial_number,
    )
}

pub fn run(selector: &Selector) -> Result<()> {
    crate::rynk_client::run_version(selector)
}

#[cfg(test)]
mod tests {
    use rynk::rmk_types::protocol::rynk::FirmwareVersion;

    use super::*;

    #[test]
    fn keeps_protocol_application_and_rmk_versions_distinct() {
        let build = BuildInfo {
            label: "glove80-rmk v0.1.0 (205266c5) / RMK v0.8.2"
                .try_into()
                .unwrap(),
        };
        let device = DeviceInfo {
            rmk_version: FirmwareVersion {
                major: 0,
                minor: 8,
                patch: 2,
            },
            vendor_id: 0x16c0,
            product_id: 0x27db,
            manufacturer: "MoErgo".try_into().unwrap(),
            product_name: "Glove80".try_into().unwrap(),
            serial_number: "rynk:glove80".try_into().unwrap(),
        };

        let report = render(ProtocolVersion { major: 0, minor: 3 }, &device, &build);
        assert!(report.contains("Rynk protocol: v0.3"));
        assert!(report.contains("firmware: glove80-rmk v0.1.0"));
        assert!(report.contains("RMK: v0.8.2"));
        assert!(report.contains("MoErgo Glove80 (USB 16c0:27db)"));
    }
}
