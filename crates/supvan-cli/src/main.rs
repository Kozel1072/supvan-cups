//! `supvan-cli` — a diagnostic tool for talking to a Supvan printer directly,
//! bypassing the IPP/CUPS stack. Connect over Bluetooth (an address) or USB HID
//! (a `/dev/hidrawN` path) and run a subcommand: `probe` (device/status/material/
//! version), `material` (loaded label + RFID + remaining count), `test-print`
//! (a built-in pattern), `feed` (advance one label), `provision` (inject a
//! synthetic material record for stock the printer can't read a tag from),
//! `heat-sweep` (walk heat time against density to calibrate unknown stock), or
//! `discover` (scan for Supvan Bluetooth devices).

use std::error::Error;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use supvan_proto::bitmap::PRINTHEAD_WIDTH_MM;
use supvan_proto::printer::Printer;
use supvan_proto::rfid::{RfidMaterial, heat_presets};
use supvan_proto::status::{DEFAULT_LABEL_GAP_MM, DEFAULT_LABEL_HEIGHT_MM, MaterialInfo};

type CliResult = Result<(), Box<dyn Error>>;

#[derive(Parser)]
#[command(name = "supvan-cli", about = "Supvan T50 Pro printer tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Probe printer: check device, status, material, version info
    Probe {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
    },
    /// Query and print label material info
    Material {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
    },
    /// Send a test print pattern
    TestPrint {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
        /// Print density (0-15)
        #[arg(short, long, default_value_t = 4)]
        density: u8,
    },
    /// Feed/advance one blank label (PAPER_SKIP)
    Feed {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
    },
    /// Write a synthetic label-material record, for stock whose RFID tag the
    /// printer can't read (third-party or foreign-brand rolls)
    Provision {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
        /// Label width across the printhead, mm
        #[arg(long, default_value_t = 40)]
        width: u8,
        /// Label length along the feed direction, mm
        #[arg(long, default_value_t = 30)]
        length: u8,
        /// Inter-label gap, mm
        #[arg(long, default_value_t = DEFAULT_LABEL_GAP_MM)]
        gap: u8,
        /// Heat times as `heat5:heat40`; defaults to the vendor's standard profile
        #[arg(long, value_parser = parse_heat_pair)]
        heat: Option<(u16, u16)>,
        /// Labels remaining to report on the roll
        #[arg(long, default_value_t = 480)]
        count: u32,
        /// Material type discriminant (1 = die-cut, 0 = continuous)
        #[arg(long, default_value_t = 1)]
        mat_type: u8,
    },
    /// Sweep heat time against density, printing one calibration strip per heat
    /// profile. For two-colour thermal stock, this finds the energy at which the
    /// colour flips.
    HeatSweep {
        /// Bluetooth address or /dev/hidrawN path
        target: String,
        /// Label width across the printhead, mm
        #[arg(long, default_value_t = 40)]
        width: u8,
        /// Label length along the feed direction, mm
        #[arg(long, default_value_t = 30)]
        length: u8,
        /// Inter-label gap, mm
        #[arg(long, default_value_t = DEFAULT_LABEL_GAP_MM)]
        gap: u8,
        /// Heat profiles to walk, each `heat5:heat40`. Repeatable; defaults to
        /// the three the vendor ships.
        #[arg(long = "heat", value_parser = parse_heat_pair)]
        heats: Vec<(u16, u16)>,
        /// Densities to lay down the strip, top to bottom
        #[arg(long, value_delimiter = ',', default_values_t = [0u8, 2, 4, 6, 8, 10, 12, 15])]
        densities: Vec<u8>,
    },
    /// Scan for Supvan Bluetooth devices (via BlueZ D-Bus)
    Discover,
}

fn connect(target: &str) -> Result<Printer, Box<dyn Error>> {
    if target.starts_with("/dev/hidraw") {
        eprintln!("Opening USB HID {target}...");
    } else {
        eprintln!("Connecting to {target} (Bluetooth)...");
    }
    let printer = Printer::open_target(target)?;
    eprintln!("Connected.");
    Ok(printer)
}

async fn cmd_probe(target: &str) -> CliResult {
    let printer = connect(target)?;

    if printer.check_device().await? {
        eprintln!("Device: OK");
    } else {
        return Err("device check: no response".into());
    }

    if let Some(status) = printer.query_status().await? {
        eprintln!("Status:");
        eprintln!("  printing:     {}", status.printing);
        eprintln!("  device_busy:  {}", status.device_busy);
        eprintln!("  buf_full:     {}", status.buf_full);
        eprintln!("  low_battery:  {}", status.low_battery);
        eprintln!("  cover_open:   {}", status.cover_open);
        eprintln!("  print_count:  {}", status.print_count);
        if let Some(errs) = status.error_description() {
            eprintln!("  ERRORS:       {errs}");
        }
    }

    if let Some(name) = printer.read_device_name().await? {
        eprintln!("Device name: {name}");
    }
    if let Some(fw) = printer.read_firmware_version().await? {
        eprintln!("Firmware:    {fw}");
    }
    if let Some(ver) = printer.read_version().await? {
        eprintln!("Protocol:    {ver}");
    }

    if let Some(mat) = printer.query_material().await? {
        eprintln!("Material:");
        eprintln!("  Label:     {}mm x {}mm", mat.width_mm, mat.height_mm);
        eprintln!("  Type:      {}", mat.label_type);
        eprintln!("  Gap:       {}mm", mat.gap_mm);
        eprintln!("  SN:        {}", mat.sn);
        eprintln!("  UUID:      {}", mat.uuid);
        eprintln!("  Code:      {}", mat.code);
        if let Some(remaining) = mat.remaining {
            eprintln!("  Remaining: {remaining} labels");
        }
        if let Some(ref dev_sn) = mat.device_sn {
            eprintln!("  Device SN: {dev_sn}");
        }
    }
    Ok(())
}

async fn cmd_material(target: &str) -> CliResult {
    let printer = connect(target)?;

    if !printer.check_device().await? {
        return Err("device not responding".into());
    }

    let mat = printer
        .query_material()
        .await?
        .ok_or("no material info (label not installed?)")?;

    println!(
        "Label:     {}mm x {}mm  (type={}, gap={}mm)",
        mat.width_mm, mat.height_mm, mat.label_type, mat.gap_mm
    );
    println!("Label SN:  {}", mat.sn);
    println!("RFID UID:  {}", mat.uuid);
    println!("RFID code: {}", mat.code);
    match mat.remaining {
        Some(r) => println!("Remaining: {r} labels"),
        None => println!("Remaining: (not reported)"),
    }
    match mat.device_sn {
        Some(s) => println!("Device SN: {s}"),
        None => println!("Device SN: (not in this response)"),
    }
    Ok(())
}

async fn cmd_test_print(target: &str, density: u8) -> CliResult {
    let printer = connect(target)?;

    // Query material to get label dimensions, falling back to printhead-width
    // defaults if no label is installed.
    let mat = match printer.query_material().await? {
        Some(m) => m,
        None => {
            eprintln!(
                "No material info, using defaults ({PRINTHEAD_WIDTH_MM}mm x {DEFAULT_LABEL_HEIGHT_MM}mm)"
            );
            MaterialInfo {
                width_mm: PRINTHEAD_WIDTH_MM as u8,
                height_mm: DEFAULT_LABEL_HEIGHT_MM,
                gap_mm: DEFAULT_LABEL_GAP_MM,
                ..Default::default()
            }
        }
    };

    eprintln!(
        "Printing test pattern on {}mm x {}mm label...",
        mat.width_mm, mat.height_mm
    );
    printer.test_print(&mat, density).await?;
    eprintln!("Done.");
    Ok(())
}

async fn cmd_feed(target: &str) -> CliResult {
    let printer = connect(target)?;
    printer.paper_skip().await?;
    eprintln!("Fed one label.");
    Ok(())
}

/// Parse a `heat5:heat40` pair, e.g. `1700:1200`.
fn parse_heat_pair(s: &str) -> Result<(u16, u16), String> {
    let (h5, h40) = s
        .split_once(':')
        .ok_or_else(|| format!("expected `heat5:heat40`, got `{s}`"))?;
    Ok((
        h5.parse().map_err(|_| format!("bad heat5 `{h5}`"))?,
        h40.parse().map_err(|_| format!("bad heat40 `{h40}`"))?,
    ))
}

fn build_material(
    width: u8,
    length: u8,
    gap: u8,
    heat: (u16, u16),
    count: u32,
    mat_type: u8,
) -> RfidMaterial {
    RfidMaterial {
        width_mm: width,
        length_mm: length,
        gap_mm: gap,
        heat_time_5: heat.0,
        heat_time_40: heat.1,
        remaining: count,
        mat_type,
        ..Default::default()
    }
}

/// The printer commits a written record asynchronously: for a short window
/// after the bulk write it still serves a half-updated one (observed on a T50M
/// Pro as a UUID of `00001000000000` between the old all-zero value and the new
/// one). Poll rather than trusting the first read.
const PROVISION_SETTLE_POLLS: usize = 10;
const PROVISION_SETTLE_INTERVAL: Duration = Duration::from_millis(200);

/// Write the record, then read the material back so the caller can see whether
/// the printer actually took it — the synthetic UUID echoing back is the tell.
async fn provision(printer: &Printer, mat: &RfidMaterial) -> Result<(), Box<dyn Error>> {
    printer.set_rfid_data(&mat.encode()).await?;

    let expected = hex_upper(&mat.uuid_bytes());
    let mut last = None;
    for _ in 0..PROVISION_SETTLE_POLLS {
        tokio::time::sleep(PROVISION_SETTLE_INTERVAL).await;
        let read_back = printer.query_material().await?;
        if let Some(ref m) = read_back
            && m.uuid == expected
        {
            eprintln!(
                "Provisioned: {}mm x {}mm, gap {}mm, heat {}/{}, UUID {}",
                m.width_mm, m.height_mm, m.gap_mm, mat.heat_time_5, mat.heat_time_40, m.uuid
            );
            return Ok(());
        }
        last = read_back;
    }

    match last {
        Some(m) => eprintln!(
            "Warning: printer still reports UUID {} (wrote {expected}) — record not taken",
            m.uuid
        ),
        None => eprintln!("Warning: no material info after write"),
    }
    Ok(())
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

async fn cmd_provision(
    target: &str,
    width: u8,
    length: u8,
    gap: u8,
    heat: Option<(u16, u16)>,
    count: u32,
    mat_type: u8,
) -> CliResult {
    let printer = connect(target)?;
    let heat = heat.unwrap_or(heat_presets::STANDARD);
    let mat = build_material(width, length, gap, heat, count, mat_type);
    provision(&printer, &mat).await
}

async fn cmd_heat_sweep(
    target: &str,
    width: u8,
    length: u8,
    gap: u8,
    heats: Vec<(u16, u16)>,
    densities: Vec<u8>,
) -> CliResult {
    let heats = if heats.is_empty() {
        vec![
            heat_presets::STANDARD,
            heat_presets::BLACK_MARK,
            heat_presets::CARDSTOCK,
        ]
    } else {
        heats
    };

    let printer = connect(target)?;
    eprintln!(
        "Sweeping {} heat profiles x {} densities on {width}mm x {length}mm labels.",
        heats.len(),
        densities.len()
    );
    eprintln!("Bands run top to bottom in the order printed; annotate each strip as it comes out.");

    for (i, heat) in heats.iter().enumerate() {
        let mat = build_material(width, length, gap, *heat, 480, 1);
        eprintln!(
            "\nStrip {}/{}: heat5={} heat40={}, densities {:?}",
            i + 1,
            heats.len(),
            heat.0,
            heat.1,
            densities
        );
        provision(&printer, &mat).await?;
        printer
            .print_swatch_ladder(width as u32, length as u32, &densities)
            .await?;
    }

    eprintln!("\nSweep complete: {} strips.", heats.len());
    Ok(())
}

fn cmd_discover() {
    eprintln!("Scanning for Supvan devices...");
    eprintln!("(For full D-Bus discovery, use the CUPS backend with 0 args)");
    eprintln!();
    eprintln!("Manual discovery:");
    eprintln!("  bluetoothctl devices | grep -i 'T0117\\|T50\\|Supvan\\|Katasymbol'");
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Probe { target } => cmd_probe(&target).await,
        Command::Material { target } => cmd_material(&target).await,
        Command::TestPrint { target, density } => cmd_test_print(&target, density).await,
        Command::Feed { target } => cmd_feed(&target).await,
        Command::Provision {
            target,
            width,
            length,
            gap,
            heat,
            count,
            mat_type,
        } => cmd_provision(&target, width, length, gap, heat, count, mat_type).await,
        Command::HeatSweep {
            target,
            width,
            length,
            gap,
            heats,
            densities,
        } => cmd_heat_sweep(&target, width, length, gap, heats, densities).await,
        Command::Discover => {
            cmd_discover();
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parse_probe_with_target() {
        let cli = Cli::try_parse_from(["supvan-cli", "probe", "/dev/hidraw3"]).unwrap();
        match cli.command {
            Command::Probe { target } => assert_eq!(target, "/dev/hidraw3"),
            _ => panic!("expected Probe"),
        }
    }

    #[test]
    fn probe_requires_target() {
        // `target` is a required positional now (no hardcoded default).
        assert!(Cli::try_parse_from(["supvan-cli", "probe"]).is_err());
    }

    #[test]
    fn parse_test_print_density() {
        let cli = Cli::try_parse_from([
            "supvan-cli",
            "test-print",
            "AA:BB:CC:DD:EE:FF",
            "--density",
            "7",
        ])
        .unwrap();
        match cli.command {
            Command::TestPrint { target, density } => {
                assert_eq!(target, "AA:BB:CC:DD:EE:FF");
                assert_eq!(density, 7);
            }
            _ => panic!("expected TestPrint"),
        }
    }

    #[test]
    fn parse_feed_with_target() {
        let cli = Cli::try_parse_from(["supvan-cli", "feed", "/dev/hidraw3"]).unwrap();
        match cli.command {
            Command::Feed { target } => assert_eq!(target, "/dev/hidraw3"),
            _ => panic!("expected Feed"),
        }
    }

    #[test]
    fn parse_discover() {
        let cli = Cli::try_parse_from(["supvan-cli", "discover"]).unwrap();
        assert!(matches!(cli.command, Command::Discover));
    }

    #[test]
    fn parse_provision_with_heat() {
        let cli = Cli::try_parse_from([
            "supvan-cli",
            "provision",
            "/dev/hidraw11",
            "--width",
            "50",
            "--length",
            "30",
            "--heat",
            "1900:1400",
        ])
        .unwrap();
        let Command::Provision {
            width,
            length,
            heat,
            ..
        } = cli.command
        else {
            panic!("expected Provision");
        };
        assert_eq!((width, length), (50, 30));
        assert_eq!(heat, Some((1900, 1400)));
    }

    #[test]
    fn heat_pair_needs_a_colon() {
        assert!(super::parse_heat_pair("1700").is_err());
        assert!(super::parse_heat_pair("1700:abc").is_err());
        assert_eq!(super::parse_heat_pair("1700:1200"), Ok((1700, 1200)));
    }

    #[test]
    fn parse_heat_sweep_repeats_heat_and_splits_densities() {
        let cli = Cli::try_parse_from([
            "supvan-cli",
            "heat-sweep",
            "/dev/hidraw11",
            "--heat",
            "1700:1200",
            "--heat",
            "2500:2000",
            "--densities",
            "0,4,8,15",
        ])
        .unwrap();
        let Command::HeatSweep {
            heats, densities, ..
        } = cli.command
        else {
            panic!("expected HeatSweep");
        };
        assert_eq!(heats, vec![(1700, 1200), (2500, 2000)]);
        assert_eq!(densities, vec![0, 4, 8, 15]);
    }

    /// Omitting --heat is what selects the three vendor presets at run time, so
    /// the parsed value must stay empty rather than picking up a clap default.
    #[test]
    fn heat_sweep_defaults_to_no_explicit_heats() {
        let cli = Cli::try_parse_from(["supvan-cli", "heat-sweep", "/dev/hidraw11"]).unwrap();
        let Command::HeatSweep {
            heats, densities, ..
        } = cli.command
        else {
            panic!("expected HeatSweep");
        };
        assert!(heats.is_empty());
        assert_eq!(densities, vec![0, 2, 4, 6, 8, 10, 12, 15]);
    }
}
