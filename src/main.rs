use clap::{Parser, Subcommand};
use puddle::cli::commands;
use puddle::executor::command_runner::RealCommandRunner;
use puddle::metadata::pool_config::PoolConfig;

const META_DIR: &str = "/var/lib/puddle";

#[derive(Parser)]
#[command(name = "puddle", version, about = "Heterogeneous disk pool manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new storage pool with a single disk
    Init {
        /// Block device to initialize (e.g. /dev/sdb)
        device: String,
        /// Filesystem type to create on the data volume
        #[arg(long)]
        mkfs: Option<String>,
        /// Mount point for the data volume
        #[arg(long)]
        mount: Option<String>,
    },
    /// Add a disk to an existing pool
    Add {
        /// Block device to add (e.g. /dev/sdc)
        device: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Show pool status
    Status,
}

fn main() {
    let cli = Cli::parse();
    let runner = RealCommandRunner;

    let result = match cli.command {
        Commands::Init {
            device,
            mkfs,
            mount,
        } => {
            let fs = mkfs.as_deref();
            let mp = mount.as_deref();
            match commands::init(&runner, &device, fs, mp, META_DIR) {
                Ok(config) => {
                    print_init_result(&config);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Add { device, yes: _ } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    eprintln!("Run 'puddle init <device>' first.");
                    std::process::exit(1);
                }
            };
            let existing = PoolConfig::from_toml(&toml_str).unwrap();
            match commands::add(&runner, &device, &existing) {
                Ok(config) => {
                    // メタデータ保存
                    let new_toml = config.to_toml().unwrap();
                    std::fs::write(&meta_path, new_toml).unwrap();
                    print_add_result(&config);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Status => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let config = PoolConfig::from_toml(&toml_str).unwrap();
            print_status(&config);
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

fn print_init_result(config: &PoolConfig) {
    println!("Pool '{}' created.", config.pool.name);
    if config.disks.len() == 1 {
        println!("WARNING: 1-disk configuration has no redundancy.");
        println!("  Add a disk: puddle add <device>");
    }
}

fn print_add_result(config: &PoolConfig) {
    println!("Disk added successfully.");
    println!("  Disks: {}", config.disks.len());
    println!("  Zones: {}", config.zones.len());
    for zone in &config.zones {
        println!(
            "    Zone {}: {:?} ({} disks)",
            zone.index,
            zone.raid_level,
            zone.participating_disk_uuids.len()
        );
    }
}

fn print_status(config: &PoolConfig) {
    use puddle::planner::capacity::format_bytes;

    println!("Pool: {}", config.pool.name);
    println!("State: {:?}", config.state.pool_status);
    println!("Redundancy: {:?}", config.pool.redundancy);
    println!();

    println!("Disks:");
    for disk in &config.disks {
        println!(
            "  #{} {} {} [{}]",
            disk.seq,
            disk.device_id,
            format_bytes(disk.capacity_bytes),
            match disk.status {
                puddle::types::DiskStatus::Active => "ACTIVE",
                puddle::types::DiskStatus::Failed => "FAILED",
                puddle::types::DiskStatus::Removing => "REMOVING",
            }
        );
    }
    println!();

    println!("Zones:");
    for zone in &config.zones {
        println!(
            "  Zone {} {:?} {} disks x {} {}",
            zone.index,
            zone.raid_level,
            zone.participating_disk_uuids.len(),
            format_bytes(zone.size_bytes),
            zone.md_device,
        );
    }
    println!();

    let total_physical: u64 = config.disks.iter().map(|d| d.capacity_bytes).sum();
    println!("Capacity:");
    println!("  Physical: {}", format_bytes(total_physical));
    println!();

    println!(
        "Mount: {} ({})",
        config.lvm.mount_point, config.lvm.filesystem
    );
}
