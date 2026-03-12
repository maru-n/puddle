use anyhow::{bail, Context, Result};
use uuid::Uuid;

use crate::executor::command_runner::CommandRunner;
use crate::executor::filesystem::FilesystemManager;
use crate::executor::lvm::VolumeManager;
use crate::executor::mdadm::RaidManager;
use crate::executor::partition::PartitionManager;
use crate::metadata::pool_config::*;
use crate::metadata::sync::MetadataSync;
use crate::planner::zone::compute_zones;
use crate::types::*;

/// デバイスの容量をバイト単位で取得する
fn get_device_capacity<R: CommandRunner>(runner: &R, device: &str) -> Result<u64> {
    let output = runner.run("lsblk", &["-bno", "SIZE", device])?;
    let capacity: u64 = output
        .trim()
        .parse()
        .context("Failed to parse device capacity")?;
    Ok(capacity)
}

/// デバイスに既存パーティションがあるか確認する
fn check_existing_partitions<R: CommandRunner>(runner: &R, device: &str) -> Result<bool> {
    let result = runner.run("blkid", &[device]);
    match result {
        Ok(output) => Ok(!output.trim().is_empty()),
        Err(_) => Ok(false), // blkid がエラー = パーティションなし
    }
}

/// デバイスの by-id パスを取得する (取得できなければデバイスパスをそのまま返す)
fn get_device_id<R: CommandRunner>(runner: &R, device: &str) -> String {
    // テスト環境では失敗するので、デバイスパスをフォールバック
    if let Ok(output) = runner.run("ls", &["-la", "/dev/disk/by-id/"]) {
        // デバイス名を by-id から探す
        let dev_name = device.rsplit('/').next().unwrap_or(device);
        for line in output.lines() {
            if line.contains(dev_name) && !line.contains("-part") {
                if let Some(id) = line.split_whitespace().nth(8) {
                    return id.to_string();
                }
            }
        }
    }
    device.to_string()
}

/// ゾーンの md デバイスパスを生成する
fn md_device_name(zone_index: usize) -> String {
    format!("/dev/md/puddle-z{}", zone_index)
}

/// パーティションデバイスパスを生成する (例: /dev/sdb → /dev/sdb2)
fn partition_path(device: &str, part_num: usize) -> String {
    // nvme デバイスは p 付き (例: /dev/nvme0n1p2)
    if device.contains("nvme") || device.contains("loop") {
        format!("{}p{}", device, part_num)
    } else {
        format!("{}{}", device, part_num)
    }
}

/// LV の device-mapper パスを生成する
fn lv_path(vg_name: &str, lv_name: &str) -> String {
    let escaped_vg = vg_name.replace('-', "--");
    format!("/dev/mapper/{}-{}", escaped_vg, lv_name)
}

// ────────────────────────────────────────────
// puddle init
// ────────────────────────────────────────────

/// プールを初期化する
///
/// meta_dir: メタデータ(pool.toml)保存先ディレクトリパス
pub fn init<R: CommandRunner>(
    runner: &R,
    device: &str,
    fs_type: Option<&str>,
    mount_point: Option<&str>,
    meta_dir: &str,
) -> Result<PoolConfig> {
    // 1. デバイス情報取得
    let capacity = get_device_capacity(runner, device)?;
    let has_partitions = check_existing_partitions(runner, device)?;
    if has_partitions {
        bail!(
            "Device {} already has partitions. Use --force to override.",
            device
        );
    }

    // 2. ゾーン計算
    let plan = compute_zones(&[capacity], Redundancy::Single);

    // 3. パーティション作成
    let pm = PartitionManager::new(runner);
    pm.wipe(device)?;
    pm.create_metadata_partition(device)?;
    pm.create_zone_partitions(device, &plan.zones)?;
    pm.reload_table(device)?;

    // 4. メタデータパーティションをフォーマット
    let meta_part = partition_path(device, 1);
    let fm = FilesystemManager::new(runner);
    fm.mkfs(&meta_part, "ext4")?;

    // 5. RAID アレイ作成
    let rm = RaidManager::new(runner);
    let pool_uuid = Uuid::new_v4();
    let disk_uuid = Uuid::new_v4();
    let device_id = get_device_id(runner, device);

    let mut zone_metas = Vec::new();
    for zone in &plan.zones {
        let md_dev = md_device_name(zone.index);
        let zone_part = partition_path(device, zone.index + 2);
        rm.create_array(&md_dev, zone.raid_level, &[&zone_part])?;

        zone_metas.push(ZoneMeta {
            index: zone.index,
            start_bytes: zone.start_bytes,
            size_bytes: zone.size_bytes,
            raid_level: zone.raid_level,
            md_device: md_dev,
            participating_disk_uuids: vec![disk_uuid],
        });
    }

    // 6. LVM セットアップ
    let vm = VolumeManager::new(runner);
    let vg_name = "puddle-pool";
    let lv_name = "data";

    let md_devices: Vec<String> = plan.zones.iter().map(|z| md_device_name(z.index)).collect();

    for md_dev in &md_devices {
        vm.pvcreate(md_dev)?;
    }

    let pv_refs: Vec<&str> = md_devices.iter().map(|s| s.as_str()).collect();
    vm.vgcreate(vg_name, &pv_refs)?;
    vm.lvcreate_full(vg_name, lv_name)?;

    // 7. データ FS 作成
    let data_lv = lv_path(vg_name, lv_name);
    let filesystem = fs_type.unwrap_or("ext4");
    if fs_type.is_some() {
        fm.mkfs(&data_lv, filesystem)?;
    }

    // 8. マウント (指定時)
    let mp = mount_point.unwrap_or("/mnt/pool");
    if mount_point.is_some() {
        fm.mount(&data_lv, mp)?;
    }

    // 9. PoolConfig 生成
    let config = PoolConfig {
        pool: PoolMeta {
            uuid: pool_uuid,
            name: format!("puddle-{}", &pool_uuid.to_string()[..8]),
            created_at: chrono_now(),
            redundancy: Redundancy::Single,
        },
        disks: vec![DiskMeta {
            uuid: disk_uuid,
            device_id,
            capacity_bytes: capacity,
            seq: 0,
            status: DiskStatus::Active,
        }],
        zones: zone_metas,
        lvm: LvmMeta {
            vg_name: vg_name.to_string(),
            lv_name: lv_name.to_string(),
            filesystem: filesystem.to_string(),
            mount_point: mp.to_string(),
        },
        state: StateMeta {
            pool_status: PoolStatus::Healthy,
            last_scrub: None,
            version: 2,
        },
    };

    // 10. メタデータ保存 (ディスク + ローカルキャッシュ)
    let ms = MetadataSync::new(runner);
    ms.write_metadata_with_local(&config, &[device], meta_dir)?;

    Ok(config)
}

// ────────────────────────────────────────────
// puddle add
// ────────────────────────────────────────────

/// ディスク追加のプレビュー情報
pub struct AddPreview {
    pub current_zones: Vec<ZoneMeta>,
    pub new_zones: Vec<crate::types::ZoneSpec>,
    pub current_effective_bytes: u64,
    pub new_effective_bytes: u64,
    pub new_disk_capacity_bytes: u64,
}

/// ディスク追加のプレビューを生成する (実行はしない)
pub fn preview_add<R: CommandRunner>(
    runner: &R,
    device: &str,
    existing: &PoolConfig,
) -> Result<AddPreview> {
    let capacity = get_device_capacity(runner, device)?;

    let mut all_capacities: Vec<u64> = existing.disks.iter().map(|d| d.capacity_bytes).collect();
    let current_plan = compute_zones(&all_capacities, existing.pool.redundancy);

    all_capacities.push(capacity);
    let new_plan = compute_zones(&all_capacities, existing.pool.redundancy);

    Ok(AddPreview {
        current_zones: existing.zones.clone(),
        new_zones: new_plan.zones,
        current_effective_bytes: current_plan.total_effective_bytes,
        new_effective_bytes: new_plan.total_effective_bytes,
        new_disk_capacity_bytes: capacity,
    })
}

/// ディスクを既存プールに追加する
///
/// meta_dir: メタデータ(pool.toml)保存先ディレクトリパス
pub fn add<R: CommandRunner>(
    runner: &R,
    device: &str,
    existing: &PoolConfig,
    meta_dir: &str,
) -> Result<PoolConfig> {
    // 1. 新ディスクの容量取得
    let capacity = get_device_capacity(runner, device)?;
    let device_id = get_device_id(runner, device);
    let new_disk_uuid = Uuid::new_v4();

    // 2. 新しいディスク構成でゾーン再計算
    let mut all_capacities: Vec<u64> = existing.disks.iter().map(|d| d.capacity_bytes).collect();
    all_capacities.push(capacity);
    let new_plan = compute_zones(&all_capacities, existing.pool.redundancy);

    // 3. 新ディスクにパーティション作成
    let pm = PartitionManager::new(runner);
    pm.wipe(device)?;
    pm.create_metadata_partition(device)?;
    pm.create_zone_partitions(device, &new_plan.zones)?;
    pm.reload_table(device)?;

    // 4. メタデータパーティションをフォーマット
    let meta_part = partition_path(device, 1);
    let fm = FilesystemManager::new(runner);
    fm.mkfs(&meta_part, "ext4")?;

    // 5. RAID アレイの更新
    let rm = RaidManager::new(runner);
    let vm = VolumeManager::new(runner);
    let mut new_zone_metas = Vec::new();

    for new_zone in &new_plan.zones {
        let md_dev = md_device_name(new_zone.index);
        let zone_part = partition_path(device, new_zone.index + 2);

        let old_zone = existing.zones.iter().find(|z| z.index == new_zone.index);

        match old_zone {
            Some(oz) => {
                // 既存ゾーン: デバイス追加 + grow
                rm.add_device(&md_dev, &zone_part)?;

                if oz.raid_level != new_zone.raid_level {
                    // RAID レベル変更 (例: RAID1 → RAID5)
                    rm.grow_level(&md_dev, new_zone.raid_level, new_zone.num_disks)?;
                } else if new_zone.num_disks > oz.participating_disk_uuids.len() {
                    rm.grow(&md_dev, new_zone.num_disks)?;
                }

                let mut disk_uuids = oz.participating_disk_uuids.clone();
                disk_uuids.push(new_disk_uuid);
                new_zone_metas.push(ZoneMeta {
                    index: oz.index,
                    start_bytes: new_zone.start_bytes,
                    size_bytes: new_zone.size_bytes,
                    raid_level: new_zone.raid_level,
                    md_device: md_dev,
                    participating_disk_uuids: disk_uuids,
                });
            }
            None => {
                // 新規ゾーン: アレイ作成 + LVM 拡張
                rm.create_array(&md_dev, new_zone.raid_level, &[&zone_part])?;
                vm.pvcreate(&md_dev)?;
                vm.vgextend(&existing.lvm.vg_name, &md_dev)?;

                new_zone_metas.push(ZoneMeta {
                    index: new_zone.index,
                    start_bytes: new_zone.start_bytes,
                    size_bytes: new_zone.size_bytes,
                    raid_level: new_zone.raid_level,
                    md_device: md_dev,
                    participating_disk_uuids: vec![new_disk_uuid],
                });
            }
        }
    }

    // 6. LV 拡張 + FS リサイズ
    let data_lv = lv_path(&existing.lvm.vg_name, &existing.lvm.lv_name);
    vm.lvextend_full(&data_lv)?;
    fm.resize(&data_lv, &existing.lvm.filesystem)?;

    // 7. 新しい PoolConfig を生成
    let mut new_disks = existing.disks.clone();
    let next_seq = new_disks.iter().map(|d| d.seq).max().unwrap_or(0) + 1;
    new_disks.push(DiskMeta {
        uuid: new_disk_uuid,
        device_id,
        capacity_bytes: capacity,
        seq: next_seq,
        status: DiskStatus::Active,
    });

    let new_config = PoolConfig {
        pool: existing.pool.clone(),
        disks: new_disks,
        zones: new_zone_metas,
        lvm: existing.lvm.clone(),
        state: existing.state.clone(),
    };

    // 8. メタデータ保存 (新ディスク + ローカルキャッシュ)
    // Phase 1 では新ディスクのパスのみに書き込む
    // (既存ディスクのデバイスパスは PoolConfig に保持されていないため)
    let ms = MetadataSync::new(runner);
    ms.write_metadata_with_local(&new_config, &[device], meta_dir)?;

    Ok(new_config)
}

// ────────────────────────────────────────────
// puddle replace
// ────────────────────────────────────────────

/// 障害ディスクを同容量以上の新ディスクに交換する
///
/// 旧ディスクを全 mdadm アレイで fail → remove し、
/// 新ディスクにパーティションを作成して各アレイに add する。
pub fn replace<R: CommandRunner>(
    runner: &R,
    old_device: &str,
    new_device: &str,
    existing: &PoolConfig,
    meta_dir: &str,
) -> Result<PoolConfig> {
    // 1. 旧ディスクを特定
    let old_disk = existing
        .disks
        .iter()
        .find(|d| d.device_id == old_device)
        .ok_or_else(|| anyhow::anyhow!("Device {} not found in pool", old_device))?;

    let old_disk_uuid = old_disk.uuid;
    let old_seq = old_disk.seq;

    // 2. 新ディスクの容量確認
    let new_capacity = get_device_capacity(runner, new_device)?;
    if new_capacity < old_disk.capacity_bytes {
        bail!(
            "New device is smaller ({}) than old device ({})",
            new_capacity,
            old_disk.capacity_bytes
        );
    }

    // 3. 旧ディスクを全 mdadm アレイで fail → remove
    let rm = RaidManager::new(runner);
    for zone in &existing.zones {
        if zone.participating_disk_uuids.contains(&old_disk_uuid) {
            let old_part = partition_path(old_device, zone.index + 2);
            rm.fail_device(&zone.md_device, &old_part).ok();
            rm.remove_device(&zone.md_device, &old_part).ok();
        }
    }

    // 4. 新ディスクにパーティション作成 (旧ディスクと同じゾーン構成)
    let pm = PartitionManager::new(runner);
    pm.wipe(new_device)?;
    pm.create_metadata_partition(new_device)?;

    // 旧ディスクが参加していたゾーンのパーティションを作成
    let participating_zones: Vec<&ZoneMeta> = existing
        .zones
        .iter()
        .filter(|z| z.participating_disk_uuids.contains(&old_disk_uuid))
        .collect();

    let zone_specs: Vec<ZoneSpec> = participating_zones
        .iter()
        .map(|z| ZoneSpec {
            index: z.index,
            start_bytes: z.start_bytes,
            size_bytes: z.size_bytes,
            raid_level: z.raid_level,
            num_disks: z.participating_disk_uuids.len(),
            effective_bytes: 0, // replace では使わない
        })
        .collect();
    pm.create_zone_partitions(new_device, &zone_specs)?;
    pm.reload_table(new_device)?;

    // 5. メタデータパーティションをフォーマット
    let meta_part = partition_path(new_device, 1);
    let fm = FilesystemManager::new(runner);
    fm.mkfs(&meta_part, "ext4")?;

    // 6. 各 mdadm アレイに新デバイスを追加 (リビルド開始)
    for zone in &participating_zones {
        let new_part = partition_path(new_device, zone.index + 2);
        rm.add_device(&zone.md_device, &new_part)?;
    }

    // 7. メタデータ更新
    let new_disk_uuid = Uuid::new_v4();
    let new_device_id = get_device_id(runner, new_device);

    let new_disks: Vec<DiskMeta> = existing
        .disks
        .iter()
        .map(|d| {
            if d.uuid == old_disk_uuid {
                DiskMeta {
                    uuid: new_disk_uuid,
                    device_id: new_device_id.clone(),
                    capacity_bytes: new_capacity,
                    seq: old_seq,
                    status: DiskStatus::Active,
                }
            } else {
                d.clone()
            }
        })
        .collect();

    let new_zones: Vec<ZoneMeta> = existing
        .zones
        .iter()
        .map(|z| {
            let mut uuids = z.participating_disk_uuids.clone();
            if let Some(pos) = uuids.iter().position(|u| *u == old_disk_uuid) {
                uuids[pos] = new_disk_uuid;
            }
            ZoneMeta {
                participating_disk_uuids: uuids,
                ..z.clone()
            }
        })
        .collect();

    let new_config = PoolConfig {
        pool: existing.pool.clone(),
        disks: new_disks,
        zones: new_zones,
        lvm: existing.lvm.clone(),
        state: existing.state.clone(),
    };

    let ms = MetadataSync::new(runner);
    ms.write_metadata_with_local(&new_config, &[new_device], meta_dir)?;

    Ok(new_config)
}

// ────────────────────────────────────────────
// puddle upgrade
// ────────────────────────────────────────────

/// 容量アップグレード交換
///
/// replace と同様にリビルドを行った後、
/// 新容量でゾーンを再計算し、新ゾーン分のアレイ作成 + LVM 拡張を行う。
pub fn upgrade<R: CommandRunner>(
    runner: &R,
    old_device: &str,
    new_device: &str,
    existing: &PoolConfig,
    meta_dir: &str,
) -> Result<PoolConfig> {
    // 1. 旧ディスクを特定
    let old_disk = existing
        .disks
        .iter()
        .find(|d| d.device_id == old_device)
        .ok_or_else(|| anyhow::anyhow!("Device {} not found in pool", old_device))?;

    let old_disk_uuid = old_disk.uuid;
    let old_seq = old_disk.seq;

    // 2. 新ディスク容量確認
    let new_capacity = get_device_capacity(runner, new_device)?;
    if new_capacity < old_disk.capacity_bytes {
        bail!(
            "New device is smaller ({}) than old device ({})",
            new_capacity,
            old_disk.capacity_bytes
        );
    }

    // 3. 旧ディスクを全 mdadm アレイで fail → remove
    let rm = RaidManager::new(runner);
    for zone in &existing.zones {
        if zone.participating_disk_uuids.contains(&old_disk_uuid) {
            let old_part = partition_path(old_device, zone.index + 2);
            rm.fail_device(&zone.md_device, &old_part).ok();
            rm.remove_device(&zone.md_device, &old_part).ok();
        }
    }

    // 4. 新容量でゾーン再計算
    let new_disk_uuid = Uuid::new_v4();
    let new_device_id = get_device_id(runner, new_device);

    let new_disks: Vec<DiskMeta> = existing
        .disks
        .iter()
        .map(|d| {
            if d.uuid == old_disk_uuid {
                DiskMeta {
                    uuid: new_disk_uuid,
                    device_id: new_device_id.clone(),
                    capacity_bytes: new_capacity,
                    seq: old_seq,
                    status: DiskStatus::Active,
                }
            } else {
                d.clone()
            }
        })
        .collect();

    let all_capacities: Vec<u64> = new_disks.iter().map(|d| d.capacity_bytes).collect();
    let new_plan = compute_zones(&all_capacities, existing.pool.redundancy);

    // 5. 新ディスクにパーティション作成 (新ゾーン構成で)
    let pm = PartitionManager::new(runner);
    pm.wipe(new_device)?;
    pm.create_metadata_partition(new_device)?;
    pm.create_zone_partitions(new_device, &new_plan.zones)?;
    pm.reload_table(new_device)?;

    // 6. メタデータパーティションをフォーマット
    let meta_part = partition_path(new_device, 1);
    let fm = FilesystemManager::new(runner);
    fm.mkfs(&meta_part, "ext4")?;

    // 7. 各 mdadm アレイの更新
    let vm = VolumeManager::new(runner);
    let mut new_zone_metas = Vec::new();

    for new_zone in &new_plan.zones {
        let md_dev = md_device_name(new_zone.index);
        let new_part = partition_path(new_device, new_zone.index + 2);

        let old_zone = existing.zones.iter().find(|z| z.index == new_zone.index);

        match old_zone {
            Some(oz) => {
                // 既存ゾーン: 新デバイスを追加 (リビルド)
                rm.add_device(&md_dev, &new_part)?;

                // RAID レベルや台数変更が必要な場合
                if oz.raid_level != new_zone.raid_level {
                    rm.grow_level(&md_dev, new_zone.raid_level, new_zone.num_disks)?;
                } else if new_zone.num_disks != oz.participating_disk_uuids.len() {
                    rm.grow(&md_dev, new_zone.num_disks)?;
                }

                let mut uuids = oz.participating_disk_uuids.clone();
                if let Some(pos) = uuids.iter().position(|u| *u == old_disk_uuid) {
                    uuids[pos] = new_disk_uuid;
                }
                new_zone_metas.push(ZoneMeta {
                    index: oz.index,
                    start_bytes: new_zone.start_bytes,
                    size_bytes: new_zone.size_bytes,
                    raid_level: new_zone.raid_level,
                    md_device: md_dev,
                    participating_disk_uuids: uuids,
                });
            }
            None => {
                // 新規ゾーン (容量増分)
                rm.create_array(&md_dev, new_zone.raid_level, &[&new_part])?;
                vm.pvcreate(&md_dev)?;
                vm.vgextend(&existing.lvm.vg_name, &md_dev)?;

                new_zone_metas.push(ZoneMeta {
                    index: new_zone.index,
                    start_bytes: new_zone.start_bytes,
                    size_bytes: new_zone.size_bytes,
                    raid_level: new_zone.raid_level,
                    md_device: md_dev,
                    participating_disk_uuids: vec![new_disk_uuid],
                });
            }
        }
    }

    // 8. LV 拡張 + FS リサイズ
    let data_lv = lv_path(&existing.lvm.vg_name, &existing.lvm.lv_name);
    vm.lvextend_full(&data_lv)?;
    fm.resize(&data_lv, &existing.lvm.filesystem)?;

    // 9. 新しい PoolConfig
    let new_config = PoolConfig {
        pool: existing.pool.clone(),
        disks: new_disks,
        zones: new_zone_metas,
        lvm: existing.lvm.clone(),
        state: existing.state.clone(),
    };

    let ms = MetadataSync::new(runner);
    ms.write_metadata_with_local(&new_config, &[new_device], meta_dir)?;

    Ok(new_config)
}

// ────────────────────────────────────────────
// puddle destroy
// ────────────────────────────────────────────

/// プールを破棄する
///
/// LVM → mdadm → パーティションの順に削除する。
/// データは完全に失われる。
pub fn destroy<R: CommandRunner>(runner: &R, config: &PoolConfig) -> Result<()> {
    let fm = FilesystemManager::new(runner);
    let rm = RaidManager::new(runner);

    // 1. アンマウント (マウント中の場合のみ)
    let _ = fm.umount(&config.lvm.mount_point);

    // 2. LVM 削除
    let data_lv = lv_path(&config.lvm.vg_name, &config.lvm.lv_name);
    runner.run("lvchange", &["-an", &data_lv]).ok();
    runner.run("lvremove", &["-f", &data_lv]).ok();
    runner.run("vgremove", &["-f", &config.lvm.vg_name]).ok();

    // 3. PV 削除 + mdadm アレイ停止
    for zone in &config.zones {
        runner.run("pvremove", &["-f", &zone.md_device]).ok();
        rm.stop(&zone.md_device).ok();
    }

    // 4. パーティションテーブル消去 (全ディスク)
    let pm = PartitionManager::new(runner);
    for disk in &config.disks {
        pm.wipe(&disk.device_id).ok();
    }

    // 5. ローカルメタデータ削除
    let _ = std::fs::remove_file("/var/lib/puddle/pool.toml");

    Ok(())
}

/// ISO 8601 形式の現在時刻を返す
fn chrono_now() -> String {
    // chrono クレートを使わず、date コマンドで取得
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}
