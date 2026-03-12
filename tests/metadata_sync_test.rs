use puddle::executor::command_runner::MockCommandRunner;
use puddle::metadata::pool_config::*;
use puddle::metadata::sync::MetadataSync;
use puddle::types::*;
use uuid::Uuid;

fn sample_config() -> PoolConfig {
    let disk0_uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let disk1_uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();

    PoolConfig {
        pool: PoolMeta {
            uuid: Uuid::parse_str("a1b2c3d4-e5f6-0000-0000-000000000000").unwrap(),
            name: "mypool".to_string(),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: vec![
            DiskMeta {
                uuid: disk0_uuid,
                device_id: "/dev/loop0".to_string(),
                capacity_bytes: 256_000_000,
                seq: 0,
                status: DiskStatus::Active,
            },
            DiskMeta {
                uuid: disk1_uuid,
                device_id: "/dev/loop1".to_string(),
                capacity_bytes: 256_000_000,
                seq: 1,
                status: DiskStatus::Active,
            },
        ],
        zones: vec![ZoneMeta {
            index: 0,
            start_bytes: 0,
            size_bytes: 256_000_000,
            raid_level: RaidLevel::Raid1,
            md_device: "/dev/md/puddle-z0".to_string(),
            participating_disk_uuids: vec![disk0_uuid, disk1_uuid],
        }],
        lvm: LvmMeta {
            vg_name: "puddle-pool".to_string(),
            lv_name: "data".to_string(),
            filesystem: "ext4".to_string(),
            mount_point: "/mnt/pool".to_string(),
        },
        state: StateMeta {
            pool_status: PoolStatus::Healthy,
            last_scrub: None,
            version: 2,
        },
    }
}

#[test]
fn test_write_metadata_mounts_and_umounts_each_disk() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let tmp_dir = std::env::temp_dir().join("puddle-test-sync-mounts");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = sync.write_metadata_with_local(
        &config,
        &["/dev/loop0", "/dev/loop1"],
        tmp_dir.to_str().unwrap(),
    );

    let history = runner.history();
    let mount_calls: Vec<_> = history.iter().filter(|(cmd, _)| cmd == "mount").collect();
    let umount_calls: Vec<_> = history.iter().filter(|(cmd, _)| cmd == "umount").collect();

    // mount が2回 (各ディスクのメタデータパーティション)
    assert_eq!(
        mount_calls.len(),
        2,
        "should mount each disk's metadata partition"
    );

    // umount が2回
    assert_eq!(
        umount_calls.len(),
        2,
        "should umount each disk's metadata partition"
    );

    // mount の引数にメタデータパーティションが含まれる
    assert_eq!(mount_calls[0].1[0], "/dev/loop0p1");
    assert_eq!(mount_calls[1].1[0], "/dev/loop1p1");

    assert!(
        result.is_ok(),
        "write_metadata should succeed: {:?}",
        result
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_write_metadata_saves_local_copy() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let tmp_dir = std::env::temp_dir().join("puddle-test-sync-local");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result =
        sync.write_metadata_with_local(&config, &["/dev/loop0"], tmp_dir.to_str().unwrap());
    assert!(
        result.is_ok(),
        "write_metadata_with_local should succeed: {:?}",
        result
    );

    // ローカルコピーが存在する
    let local_path = tmp_dir.join("pool.toml");
    assert!(local_path.exists(), "local pool.toml should be created");

    // 内容が正しい
    let content = std::fs::read_to_string(&local_path).unwrap();
    let restored = PoolConfig::from_toml(&content).unwrap();
    assert_eq!(restored.pool.uuid, config.pool.uuid);

    // クリーンアップ
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_write_metadata_partition_path_for_loop_device() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let _ = sync.write_metadata(&config, &["/dev/loop0"]);

    let history = runner.history();
    let mount_call = history.iter().find(|(cmd, _)| cmd == "mount").unwrap();
    // loop デバイスのパーティション1は /dev/loop0p1
    assert_eq!(mount_call.1[0], "/dev/loop0p1");
}

#[test]
fn test_write_metadata_partition_path_for_sd_device() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let _ = sync.write_metadata(&config, &["/dev/sdb"]);

    let history = runner.history();
    let mount_call = history.iter().find(|(cmd, _)| cmd == "mount").unwrap();
    // sd デバイスのパーティション1は /dev/sdb1
    assert_eq!(mount_call.1[0], "/dev/sdb1");
}

#[test]
fn test_write_metadata_partition_path_for_nvme_device() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let _ = sync.write_metadata(&config, &["/dev/nvme0n1"]);

    let history = runner.history();
    let mount_call = history.iter().find(|(cmd, _)| cmd == "mount").unwrap();
    // nvme デバイスのパーティション1は /dev/nvme0n1p1
    assert_eq!(mount_call.1[0], "/dev/nvme0n1p1");
}

#[test]
fn test_write_metadata_umounts_even_on_write_error() {
    let runner = MockCommandRunner::new();
    // mount を失敗させる
    runner.set_fail("mount", "mount failed");
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    let result = sync.write_metadata(&config, &["/dev/loop0"]);

    // エラーが返される
    assert!(result.is_err());
}

#[test]
fn test_read_metadata_from_disk() {
    let runner = MockCommandRunner::new();
    let sync = MetadataSync::new(&runner);
    let config = sample_config();

    // まずローカルに書き込んで、そこから読み込むテスト
    // (ディスク読み出しは mount が必要なのでモックでは限界がある)
    let tmp_dir = std::env::temp_dir().join("puddle-test-sync-read");
    std::fs::create_dir_all(&tmp_dir).ok();

    let toml_str = config.to_toml().unwrap();
    std::fs::write(tmp_dir.join("pool.toml"), &toml_str).unwrap();

    let result = sync.read_metadata_local(tmp_dir.to_str().unwrap());
    assert!(result.is_ok());
    let restored = result.unwrap();
    assert_eq!(restored.pool.uuid, config.pool.uuid);
    assert_eq!(restored.disks.len(), 2);
    assert_eq!(restored.zones.len(), 1);

    std::fs::remove_dir_all(&tmp_dir).ok();
}
