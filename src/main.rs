use clap::{Parser, Subcommand};
use puddle::cli::commands;
use puddle::daemon;
use puddle::executor::command_runner::RealCommandRunner;
use puddle::lock::PuddleLock;
use puddle::metadata::pool_config::PoolConfig;
use puddle::types::Redundancy;

const META_DIR: &str = "/var/lib/puddle";
const LOCK_FILE: &str = "/var/lib/puddle/puddle.lock";

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
        /// Redundancy level: "single" (default) or "dual" (RAID6, requires 4+ disks)
        #[arg(long, default_value = "single")]
        redundancy: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
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
    /// Show disk health (SMART) and RAID sync status
    Health,
    /// Replace a failed disk with a new one
    Replace {
        /// Old device to replace (e.g. /dev/sdb)
        old_device: String,
        /// New device to use (e.g. /dev/sde)
        new_device: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Replace a disk with a larger one and recalculate zones
    Upgrade {
        /// Old device to replace (e.g. /dev/sdb)
        old_device: String,
        /// New (larger) device (e.g. /dev/sde)
        new_device: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Remove a disk from the pool (data will be evacuated first)
    Remove {
        /// Block device to remove (e.g. /dev/sdb)
        device: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Destroy the pool and remove all RAID/LVM structures
    Destroy {
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Run continuous monitoring daemon (SMART + RAID)
    Monitor {
        /// Run once and exit (no loop)
        #[arg(long)]
        once: bool,
        /// SMART polling interval in seconds
        #[arg(long, default_value = "60")]
        interval: u64,
    },
    /// Generate systemd unit file for puddled
    GenerateSystemd {
        /// Path to the puddle binary
        #[arg(long, default_value = "/usr/local/bin/puddle")]
        exec_path: String,
    },
    /// Configure webhook notification URL
    Notify {
        /// Webhook URL to POST alerts to
        #[arg(long)]
        webhook: String,
        /// Test the webhook by sending a test notification
        #[arg(long)]
        test: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    // 排他ロック取得 (ディレクトリが存在しない場合は作成)
    std::fs::create_dir_all(META_DIR).ok();
    let _lock = match PuddleLock::try_acquire(LOCK_FILE) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let runner = RealCommandRunner;

    let result = match cli.command {
        Commands::Init {
            device,
            mkfs,
            mount,
            redundancy,
            yes,
        } => {
            let redundancy = match redundancy.as_str() {
                "single" => Redundancy::Single,
                "dual" => Redundancy::Dual,
                _ => {
                    eprintln!(
                        "Error: Invalid redundancy level '{}'. Use 'single' or 'dual'.",
                        redundancy
                    );
                    std::process::exit(1);
                }
            };
            if !yes {
                println!(
                    "WARNING: This will destroy all data on {} and initialize a new pool.",
                    device
                );
                if redundancy == Redundancy::Dual {
                    println!("  Redundancy: Dual (RAID6, requires 4+ disks for full protection)");
                }
                if !confirm("Proceed?") {
                    println!("Aborted.");
                    return;
                }
            }
            let fs = mkfs.as_deref();
            let mp = mount.as_deref();
            match commands::init_with_redundancy(&runner, &device, fs, mp, META_DIR, redundancy) {
                Ok(config) => {
                    print_init_result(&config);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Add { device, yes } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    eprintln!("Run 'puddle init <device>' first.");
                    std::process::exit(1);
                }
            };
            let existing = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };

            // プレビュー表示 + 確認プロンプト
            match commands::preview_add(&runner, &device, &existing) {
                Ok(preview) => {
                    print_add_preview(&preview);
                    if !yes && !confirm("Proceed?") {
                        println!("Aborted.");
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("Error: Failed to preview: {:#}", e);
                    std::process::exit(1);
                }
            }

            match commands::add(&runner, &device, &existing, META_DIR) {
                Ok(config) => {
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
            let config = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };
            print_status(&config);
            Ok(())
        }
        Commands::Health => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let config = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };
            print_health(&runner, &config);
            Ok(())
        }
        Commands::Replace {
            old_device,
            new_device,
            yes,
        } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let existing = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };

            if !yes {
                println!(
                    "Replacing {} with {} in pool '{}'.",
                    old_device, new_device, existing.pool.name
                );
                if !confirm("Proceed?") {
                    println!("Aborted.");
                    return;
                }
            }

            match commands::replace(&runner, &old_device, &new_device, &existing, META_DIR) {
                Ok(_config) => {
                    println!("Disk replaced successfully. RAID rebuild started.");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Upgrade {
            old_device,
            new_device,
            yes,
        } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let existing = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };

            if !yes {
                println!(
                    "Upgrading {} -> {} in pool '{}'.",
                    old_device, new_device, existing.pool.name
                );
                println!("Zone layout will be recalculated after rebuild.");
                if !confirm("Proceed?") {
                    println!("Aborted.");
                    return;
                }
            }

            match commands::upgrade(&runner, &old_device, &new_device, &existing, META_DIR) {
                Ok(_config) => {
                    println!("Disk upgraded successfully. RAID rebuild started.");
                    println!("Zone layout will be recalculated after rebuild completes.");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Remove { device, yes } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let existing = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };

            if !yes {
                println!(
                    "Removing {} from pool '{}'. Data will be evacuated first.",
                    device, existing.pool.name
                );
                println!(
                    "Remaining disks: {}",
                    existing.disks.len().saturating_sub(1)
                );
                if !confirm("Proceed?") {
                    println!("Aborted.");
                    return;
                }
            }

            match commands::remove(&runner, &device, &existing, META_DIR) {
                Ok(config) => {
                    println!("Disk {} removed successfully.", device);
                    println!("  Remaining disks: {}", config.disks.len());
                    println!("  Zones: {}", config.zones.len());
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Destroy { yes } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let config = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    eprintln!("The pool configuration may be corrupted.");
                    std::process::exit(1);
                }
            };

            if !yes {
                println!(
                    "WARNING: This will destroy pool '{}' and ALL data on it.",
                    config.pool.name
                );
                println!("  Disks: {}", config.disks.len());
                println!("  Zones: {}", config.zones.len());
                if !confirm("Are you sure?") {
                    println!("Aborted.");
                    return;
                }
            }

            match commands::destroy(&runner, &config) {
                Ok(()) => {
                    println!("Pool '{}' destroyed.", config.pool.name);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Commands::Monitor { once, interval } => {
            let meta_path = format!("{}/pool.toml", META_DIR);
            let toml_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: No existing pool found at {}: {}", meta_path, e);
                    std::process::exit(1);
                }
            };
            let config = match PoolConfig::from_toml(&toml_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to parse pool config: {}", e);
                    std::process::exit(1);
                }
            };

            let devices: Vec<String> = config.disks.iter().map(|d| d.device_id.clone()).collect();

            // webhook URL があれば読み込む
            let notify_path = format!("{}/notify.conf", META_DIR);
            let webhook_url = std::fs::read_to_string(&notify_path).ok();

            if once {
                // 1回だけポーリングして終了
                let events = daemon::poll_once(&runner, &devices);
                for event in &events {
                    println!("{}", daemon::format_event(event));
                }
                // 警告があれば webhook 通知
                if let Some(ref url) = webhook_url {
                    if let Err(e) = daemon::send_webhook(&runner, url, &events) {
                        eprintln!("Webhook notification failed: {:#}", e);
                    }
                }
                let has_warnings = events.iter().any(daemon::is_warning);
                if has_warnings {
                    std::process::exit(2); // 警告ありで終了コード 2
                }
                Ok(())
            } else {
                // 継続監視ループ
                let poll_interval = std::time::Duration::from_secs(interval);
                println!(
                    "puddled: monitoring {} disks (interval: {}s)",
                    devices.len(),
                    interval
                );
                loop {
                    let events = daemon::poll_once(&runner, &devices);
                    for event in &events {
                        if daemon::is_warning(event) {
                            println!("{}", daemon::format_event(event));
                        }
                    }
                    // 警告があれば webhook 通知
                    if let Some(ref url) = webhook_url {
                        if let Err(e) = daemon::send_webhook(&runner, url, &events) {
                            eprintln!("Webhook notification failed: {:#}", e);
                        }
                    }
                    std::thread::sleep(poll_interval);
                }
            }
        }
        Commands::GenerateSystemd { exec_path } => {
            let unit = daemon::generate_systemd_unit(&format!("{} monitor", exec_path));
            println!("{}", unit);
            Ok(())
        }
        Commands::Notify { webhook, test } => {
            // webhook URL を設定ファイルに保存
            let notify_path = format!("{}/notify.conf", META_DIR);
            if let Err(e) = std::fs::write(&notify_path, &webhook) {
                eprintln!("Failed to save webhook URL to {}: {}", notify_path, e);
                std::process::exit(1);
            }
            println!("Webhook URL saved: {}", webhook);

            if test {
                // テスト通知を送信
                let test_events = vec![daemon::DaemonEvent::SmartWarning {
                    device: "test".to_string(),
                    message: "This is a test notification from puddle".to_string(),
                }];
                match daemon::send_webhook(&runner, &webhook, &test_events) {
                    Ok(()) => println!("Test notification sent successfully."),
                    Err(e) => eprintln!("Failed to send test notification: {:#}", e),
                }
            }
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

fn print_add_preview(preview: &commands::AddPreview) {
    use puddle::planner::capacity::format_bytes;

    println!("Planning zone layout...\n");

    println!("  Current layout:");
    for zone in &preview.current_zones {
        let redundancy_mark = if zone.raid_level == puddle::types::RaidLevel::Single {
            " -> no redundancy"
        } else {
            ""
        };
        println!(
            "    Zone {}: {:?} ({} disks, {}){}",
            zone.index,
            zone.raid_level,
            zone.participating_disk_uuids.len(),
            format_bytes(zone.size_bytes),
            redundancy_mark,
        );
    }
    println!();

    println!("  New layout:");
    for zone in &preview.new_zones {
        let redundancy_mark = if zone.raid_level == puddle::types::RaidLevel::Single {
            " -> no redundancy"
        } else {
            ""
        };
        println!(
            "    Zone {}: {:?} ({} disks, {}){}",
            zone.index,
            zone.raid_level,
            zone.num_disks,
            format_bytes(zone.size_bytes),
            redundancy_mark,
        );
    }
    println!();

    let diff = preview.new_effective_bytes as i64 - preview.current_effective_bytes as i64;
    let sign = if diff >= 0 { "+" } else { "" };
    println!(
        "  Effective capacity: {} -> {} ({}{})",
        format_bytes(preview.current_effective_bytes),
        format_bytes(preview.new_effective_bytes),
        sign,
        format_bytes(diff.unsigned_abs()),
    );
    println!();
}

/// ユーザーに確認を求める
fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{} [Y/n] ", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let trimmed = input.trim().to_lowercase();
    trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
}

fn print_health(runner: &puddle::executor::command_runner::RealCommandRunner, config: &PoolConfig) {
    use puddle::executor::command_runner::CommandRunner;
    use puddle::monitor::mdstat::parse_mdstat;
    use puddle::monitor::smart::parse_smart_json;

    println!("SMART Status:");
    for disk in &config.disks {
        match runner.run("smartctl", &["-j", &disk.device_id]) {
            Ok(json) => match parse_smart_json(&json) {
                Ok(info) => {
                    let status = if info.passed { "OK" } else { "WARN" };
                    let temp = info
                        .temperature_celsius
                        .map(|t| format!("{}C", t))
                        .unwrap_or_else(|| "N/A".to_string());
                    let realloc = info
                        .reallocated_sectors
                        .map(|r| format!("{}", r))
                        .unwrap_or_else(|| "N/A".to_string());
                    println!(
                        "  #{} {} {} (Temp: {}, Reallocated: {})",
                        disk.seq, info.model, status, temp, realloc
                    );
                }
                Err(_) => {
                    println!("  #{} {} [SMART parse error]", disk.seq, disk.device_id);
                }
            },
            Err(_) => {
                println!("  #{} {} [smartctl unavailable]", disk.seq, disk.device_id);
            }
        }
    }
    println!();

    // RAID Sync status from /proc/mdstat
    println!("RAID Sync:");
    match std::fs::read_to_string("/proc/mdstat") {
        Ok(mdstat) => {
            let arrays = parse_mdstat(&mdstat);
            for zone in &config.zones {
                let md_name = zone.md_device.rsplit('/').next().unwrap_or(&zone.md_device);
                let status = arrays.iter().find(|a| a.name == md_name);
                match status {
                    Some(arr) => {
                        let state = if arr.is_clean() {
                            "clean".to_string()
                        } else if let Some(pct) = arr.recovery_percent {
                            format!("rebuilding {:.1}%", pct)
                        } else {
                            format!("degraded [{}/{}]", arr.active_devices, arr.num_devices)
                        };
                        println!("  Zone {} ({:?}): {}", zone.index, zone.raid_level, state);
                    }
                    None => {
                        println!("  Zone {} ({:?}): not found", zone.index, zone.raid_level);
                    }
                }
            }
        }
        Err(_) => {
            println!("  /proc/mdstat not available");
        }
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
