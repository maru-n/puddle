use puddle::cli::commands;
use puddle::executor::command_runner::MockCommandRunner;
use puddle::metadata::pool_config::PoolConfig;
use puddle::types::*;

// ── init tests ──

#[test]
fn test_init_calls_correct_command_sequence() {
    let mock = MockCommandRunner::new();
    // lsblk でディスク容量を返す
    mock.set_stdout("lsblk", "2000000000000\n");
    // blkid でパーティション未検出を返す (空文字 = パーティションなし)
    mock.set_stdout("blkid", "");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None, // mount_point
        "/tmp/puddle-test-meta",
    );

    assert!(result.is_ok(), "init failed: {:?}", result.err());

    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    // 期待する実行順序:
    // 1. lsblk (容量取得)
    // 2. blkid (既存パーティションチェック)
    // 3. sgdisk --zap-all (GPT 初期化)
    // 4. sgdisk (metadata パーティション作成)
    // 5. sgdisk (zone パーティション作成)
    // 6. partprobe (テーブル再読み込み)
    // 7. mkfs.ext4 (metadata パーティションのフォーマット)
    // 8. mdadm --create (RAID アレイ作成)
    // 9. pvcreate
    // 10. vgcreate
    // 11. lvcreate
    // 12. mkfs.ext4 (データボリュームのフォーマット)
    assert!(programs.contains(&"lsblk"), "should call lsblk");
    assert!(programs.contains(&"sgdisk"), "should call sgdisk");
    assert!(programs.contains(&"mdadm"), "should call mdadm");
    assert!(programs.contains(&"pvcreate"), "should call pvcreate");
    assert!(programs.contains(&"vgcreate"), "should call vgcreate");
    assert!(programs.contains(&"lvcreate"), "should call lvcreate");
    assert!(programs.contains(&"mkfs.ext4"), "should call mkfs.ext4");
}

#[test]
fn test_init_without_mkfs() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        None, // no mkfs
        None,
        "/tmp/puddle-test-meta",
    );

    assert!(result.is_ok());

    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    // mkfs.ext4 は metadata パーティション用の1回のみ
    let mkfs_count = programs.iter().filter(|&&p| p == "mkfs.ext4").count();
    assert_eq!(mkfs_count, 1, "should only mkfs for metadata partition");
}

#[test]
fn test_init_produces_valid_pool_config() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");
    mock.set_stdout("blkid", "");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        "/tmp/puddle-test-meta",
    );

    let pool_config = result.unwrap();

    assert_eq!(pool_config.disks.len(), 1);
    assert_eq!(pool_config.disks[0].capacity_bytes, 4_000_000_000_000);
    assert_eq!(pool_config.disks[0].status, DiskStatus::Active);
    assert_eq!(pool_config.zones.len(), 1);
    assert_eq!(pool_config.zones[0].raid_level, RaidLevel::Single);
    assert_eq!(pool_config.state.pool_status, PoolStatus::Healthy);
    assert_eq!(pool_config.pool.redundancy, Redundancy::Single);
}

// ── add tests ──

#[test]
fn test_add_disk_updates_zones() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");

    // 既存プール: 1台 2TB
    let existing = make_single_disk_pool(2_000_000_000_000);

    let result = commands::add(&mock, "/dev/sdc", &existing);

    assert!(result.is_ok(), "add failed: {:?}", result.err());

    let new_config = result.unwrap();
    assert_eq!(new_config.disks.len(), 2);
    assert_eq!(new_config.zones.len(), 2);
    // Zone 0: RAID1 (2台 × 2TB)
    assert_eq!(new_config.zones[0].raid_level, RaidLevel::Raid1);
    // Zone 1: SINGLE (1台 × 2TB, 大きいディスクの余り)
    assert_eq!(new_config.zones[1].raid_level, RaidLevel::Single);
}

#[test]
fn test_add_calls_correct_commands() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");

    let existing = make_single_disk_pool(2_000_000_000_000);
    let result = commands::add(&mock, "/dev/sdc", &existing);

    assert!(result.is_ok());

    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    assert!(programs.contains(&"sgdisk"), "should partition new disk");
    assert!(programs.contains(&"mdadm"), "should modify raid arrays");
    assert!(
        programs.contains(&"pvcreate"),
        "should create PV for new zones"
    );
}

// ── helpers ──

fn make_single_disk_pool(capacity: u64) -> PoolConfig {
    use puddle::metadata::pool_config::*;
    use uuid::Uuid;

    let disk_uuid = Uuid::new_v4();
    let pool_uuid = Uuid::new_v4();

    PoolConfig {
        pool: PoolMeta {
            uuid: pool_uuid,
            name: format!("puddle-{}", &pool_uuid.to_string()[..8]),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: vec![DiskMeta {
            uuid: disk_uuid,
            device_id: "ata-TEST_DISK_0".to_string(),
            capacity_bytes: capacity,
            seq: 0,
            status: DiskStatus::Active,
        }],
        zones: vec![ZoneMeta {
            index: 0,
            start_bytes: 0,
            size_bytes: capacity,
            raid_level: RaidLevel::Single,
            md_device: "/dev/md/puddle-z0".to_string(),
            participating_disk_uuids: vec![disk_uuid],
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
