//! BLE discovery: scan for Supvan/Katasymbol BLE-only printers (E11/E12-class).
//!
//! An **LE-transport** scan, keeping devices that (a) advertise a Supvan GATT
//! service (`fee7`/`e0ff`/`ff00` — the actual BLE-print signature), (b) match
//! name `^[TGD]\d{2}`, and (c) sit in the Supvan MAC OUI `A4:93:40`. The GATT-
//! service requirement is what distinguishes a real BLE printer from a classic
//! SPP one (Serial Port `1101` only) that BlueZ may echo into discovery. The
//! advertised name (e.g. `T0182A2507162197`) is the firmware serial-name used
//! to cross-correlate USB/BT/BLE transports for one physical printer.
//!
//! Gated behind the `ble` feature; without it `list_candidates` is a stub that
//! returns nothing, so discovery wiring compiles on BlueZ-free CI.

use crate::models;

/// One BLE-attached Supvan candidate, ready to cross-correlate with USB/BT.
#[derive(Debug, Clone)]
pub struct BleCandidate {
    pub address: String,
    pub name: String,
}

/// True if a scanned device looks like a Supvan BLE printer: Supvan's OUI plus
/// the firmware serial-name shape both scanners share.
#[cfg_attr(not(feature = "ble"), allow(dead_code))]
fn is_supvan_ble(addr: &str, name: &str) -> bool {
    models::is_supvan_oui(addr) && models::is_supvan_serial_name(name)
}

#[cfg(not(feature = "ble"))]
pub async fn list_candidates() -> Vec<BleCandidate> {
    Vec::new()
}

#[cfg(feature = "ble")]
pub async fn list_candidates() -> Vec<BleCandidate> {
    match scan().await {
        Ok(found) => found,
        Err(e) => {
            log::warn!("ble_discover: LE scan failed: {e}");
            Vec::new()
        }
    }
}

#[cfg(feature = "ble")]
async fn scan() -> bluer::Result<Vec<BleCandidate>> {
    use bluer::{DiscoveryFilter, DiscoveryTransport};
    use futures_util::StreamExt;
    use std::collections::BTreeSet;
    use std::time::Duration;
    use supvan_proto::ble::chars_for_service;

    /// How long to listen for advertisements before reporting.
    const SCAN_WINDOW: Duration = Duration::from_secs(4);

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    // Restrict discovery to LE so BlueZ doesn't surface classic (BR/EDR)
    // devices — a classic SPP printer must never look like a BLE candidate.
    adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            ..Default::default()
        })
        .await?;

    // Collect addresses only. BlueZ fills Name and UUIDs in over several
    // PropertiesChanged signals after the device first appears, so a device
    // seen for the first time has neither at DeviceAdded time; reading them
    // here would silently drop it.
    let mut events = adapter.discover_devices().await?;
    let mut seen = BTreeSet::new();
    let collect = async {
        while let Some(ev) = events.next().await {
            if let bluer::AdapterEvent::DeviceAdded(addr) = ev {
                seen.insert(addr);
            }
        }
    };
    let _ = tokio::time::timeout(SCAN_WINDOW, collect).await;
    // Ends the discovery session: BlueZ stalls Connect() while a scan runs.
    drop(events);

    let mut out = Vec::new();
    for addr in seen {
        let astr = addr.to_string();
        if !models::is_supvan_oui(&astr) {
            continue;
        }
        let Ok(dev) = adapter.device(addr) else {
            continue;
        };
        let name = dev.name().await.ok().flatten().unwrap_or_default();
        // Definitive discriminator: the device must advertise a Supvan GATT
        // service (fee7/e0ff/ff00). Classic printers expose only SPP (Serial
        // Port 1101) and are excluded here even if BlueZ echoes them.
        let advertises_gatt = dev
            .uuids()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
            .iter()
            .any(|u| chars_for_service(*u).is_some());
        if advertises_gatt && is_supvan_ble(&astr, &name) {
            log::info!("ble_discover: found {name} ({astr})");
            out.push(BleCandidate {
                address: astr,
                name,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_e11_advert() {
        // The reporter's device.
        assert!(is_supvan_ble("A4:93:40:AF:B0:B5", "T0182A2507162197"));
    }

    #[test]
    fn rejects_foreign_oui() {
        assert!(!is_supvan_ble("00:11:22:33:44:55", "T0182A2507162197"));
    }

    #[test]
    fn rejects_non_printer_name() {
        assert!(!is_supvan_ble("A4:93:40:AF:B0:B5", "Some Headphones"));
        // Letter ok but missing the two digits.
        assert!(!is_supvan_ble("A4:93:40:AF:B0:B5", "TX"));
    }

    #[test]
    fn accepts_g_and_d_families() {
        assert!(is_supvan_ble("A4:93:40:AF:B0:B5", "G15Mini"));
        assert!(is_supvan_ble("A4:93:40:AF:B0:B5", "D12foo"));
        assert!(!is_supvan_ble("A4:93:40:AF:B0:B5", "X12foo"));
    }
}
