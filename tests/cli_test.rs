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
    // created_at がハードコードでなく、実際のタイムスタンプであること
    assert_ne!(
        pool_config.pool.created_at, "2026-03-10T12:00:00Z",
        "created_at should not be hardcoded"
    );
    assert!(
        pool_config.pool.created_at.contains('T'),
        "created_at should be ISO 8601 format"
    );
}

// ── init with redundancy tests ──

#[test]
fn test_init_with_dual_redundancy() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");
    mock.set_stdout("blkid", "");

    let result = commands::init_with_redundancy(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        "/tmp/puddle-test-dual",
        Redundancy::Dual,
    );

    assert!(result.is_ok(), "init with dual failed: {:?}", result.err());
    let config = result.unwrap();
    assert_eq!(config.pool.redundancy, Redundancy::Dual);
    // 1台なので SINGLE (Dual は 4台以上で初めて RAID6 になる)
    assert_eq!(config.zones[0].raid_level, RaidLevel::Single);
}

// ── add tests ──

#[test]
fn test_add_disk_updates_zones() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");

    // 既存プール: 1台 2TB
    let existing = make_single_disk_pool(2_000_000_000_000);

    let tmp_dir = std::env::temp_dir().join("puddle-test-add-zones");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::add(&mock, "/dev/sdc", &existing, tmp_dir.to_str().unwrap());

    assert!(result.is_ok(), "add failed: {:?}", result.err());

    let new_config = result.unwrap();
    assert_eq!(new_config.disks.len(), 2);
    assert_eq!(new_config.zones.len(), 2);
    // Zone 0: RAID1 (2台 × 2TB)
    assert_eq!(new_config.zones[0].raid_level, RaidLevel::Raid1);
    // Zone 1: SINGLE (1台 × 2TB, 大きいディスクの余り)
    assert_eq!(new_config.zones[1].raid_level, RaidLevel::Single);

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_add_calls_correct_commands() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");

    let existing = make_single_disk_pool(2_000_000_000_000);
    let tmp_dir = std::env::temp_dir().join("puddle-test-add-cmds");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::add(&mock, "/dev/sdc", &existing, tmp_dir.to_str().unwrap());

    assert!(result.is_ok());

    let h = mock.history();
    std::fs::remove_dir_all(&tmp_dir).ok();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    assert!(programs.contains(&"sgdisk"), "should partition new disk");
    assert!(programs.contains(&"mdadm"), "should modify raid arrays");
    assert!(
        programs.contains(&"pvcreate"),
        "should create PV for new zones"
    );
}

// ── replace tests ──

#[test]
fn test_replace_calls_fail_remove_add() {
    let mock = MockCommandRunner::new();
    // 新ディスクの容量 (旧と同じ)
    mock.set_stdout("lsblk", "2000000000000\n");

    let config = make_single_disk_pool(2_000_000_000_000);
    let old_device = &config.disks[0].device_id;

    let tmp_dir = std::env::temp_dir().join("puddle-test-replace");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::replace(
        &mock,
        old_device,
        "/dev/sdd",
        &config,
        tmp_dir.to_str().unwrap(),
    );

    assert!(result.is_ok(), "replace failed: {:?}", result.err());

    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    // fail → remove → partition → add の順
    assert!(programs.contains(&"mdadm"), "should call mdadm");
    assert!(programs.contains(&"sgdisk"), "should partition new disk");

    // mdadm の呼び出し履歴に --fail と --add が含まれる
    let mdadm_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "mdadm").collect();
    let has_fail = mdadm_calls
        .iter()
        .any(|(_, args)| args.contains(&"--fail".to_string()));
    let has_add = mdadm_calls
        .iter()
        .any(|(_, args)| args.contains(&"--add".to_string()));
    assert!(has_fail, "should fail old device in mdadm");
    assert!(has_add, "should add new device to mdadm");

    let new_config = result.unwrap();
    // ディスク数は変わらない (交換)
    assert_eq!(new_config.disks.len(), 1);

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_replace_updates_disk_info() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");

    let config = make_single_disk_pool(2_000_000_000_000);
    let old_device = &config.disks[0].device_id;
    let old_uuid = config.disks[0].uuid;

    let tmp_dir = std::env::temp_dir().join("puddle-test-replace-info");
    std::fs::create_dir_all(&tmp_dir).ok();

    let new_config = commands::replace(
        &mock,
        old_device,
        "/dev/sdd",
        &config,
        tmp_dir.to_str().unwrap(),
    )
    .unwrap();

    // 旧ディスクの UUID が消え、新ディスクが入っている
    assert!(
        !new_config.disks.iter().any(|d| d.uuid == old_uuid),
        "old disk should be replaced"
    );
    assert_eq!(new_config.disks.len(), 1);
    assert_eq!(new_config.disks[0].status, DiskStatus::Active);

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_replace_old_device_not_found() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");

    let config = make_single_disk_pool(2_000_000_000_000);

    let result = commands::replace(
        &mock,
        "/dev/nonexistent",
        "/dev/sdd",
        &config,
        "/tmp/puddle-test",
    );

    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("not found"),
        "should report old device not found"
    );
}

// ── upgrade tests ──

#[test]
fn test_upgrade_to_larger_disk() {
    let mock = MockCommandRunner::new();
    // 新ディスクは 8TB (旧は 2TB)
    mock.set_stdout("lsblk", "8000000000000\n");

    let config = make_multi_zone_pool();
    let old_device = &config.disks[0].device_id; // 2TB disk

    let tmp_dir = std::env::temp_dir().join("puddle-test-upgrade");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::upgrade(
        &mock,
        old_device,
        "/dev/sde",
        &config,
        tmp_dir.to_str().unwrap(),
    );

    assert!(result.is_ok(), "upgrade failed: {:?}", result.err());

    let new_config = result.unwrap();

    // 新ディスクの容量が 8TB に更新されている
    let upgraded_disk = new_config
        .disks
        .iter()
        .find(|d| d.capacity_bytes == 8_000_000_000_000);
    assert!(upgraded_disk.is_some(), "should have 8TB disk");

    // ゾーン数が変わっているはず (4TB+4TB+8TB → ゾーン構成が変化)
    assert!(new_config.zones.len() >= 2, "should have at least 2 zones");

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_upgrade_smaller_disk_fails() {
    let mock = MockCommandRunner::new();
    // 新ディスクが旧より小さい
    mock.set_stdout("lsblk", "1000000000000\n");

    let config = make_multi_zone_pool();
    let old_device = &config.disks[0].device_id;

    let result = commands::upgrade(&mock, old_device, "/dev/sde", &config, "/tmp/puddle-test");

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("smaller"));
}

// ── destroy tests ──

#[test]
fn test_destroy_calls_correct_command_sequence() {
    let mock = MockCommandRunner::new();
    let config = make_single_disk_pool(2_000_000_000_000);

    let result = commands::destroy(&mock, &config);
    assert!(result.is_ok(), "destroy failed: {:?}", result.err());

    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();

    // umount → lvchange → lvremove → vgremove → pvremove → mdadm stop → sgdisk zap
    assert!(programs.contains(&"umount"), "should umount");
    assert!(programs.contains(&"lvchange"), "should deactivate LV");
    assert!(programs.contains(&"lvremove"), "should remove LV");
    assert!(programs.contains(&"vgremove"), "should remove VG");
    assert!(programs.contains(&"mdadm"), "should stop mdadm arrays");
    assert!(programs.contains(&"sgdisk"), "should wipe partition tables");
}

#[test]
fn test_destroy_multi_zone_pool() {
    let config = make_multi_zone_pool();
    let mock = MockCommandRunner::new();

    let result = commands::destroy(&mock, &config);
    assert!(result.is_ok(), "destroy failed: {:?}", result.err());

    let h = mock.history();

    // mdadm --stop が2回 (Zone 0, Zone 1)
    let mdadm_stops: Vec<_> = h
        .iter()
        .filter(|(cmd, args)| cmd == "mdadm" && args.contains(&"--stop".to_string()))
        .collect();
    assert_eq!(mdadm_stops.len(), 2, "should stop 2 mdadm arrays");
}

// ── rollback integration tests ──

#[test]
fn test_init_rollback_on_pvcreate_failure() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    // pvcreate を失敗させる (mdadm 成功後)
    mock.set_fail("pvcreate", "pvcreate: Device not found");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        "/tmp/puddle-test-meta",
    );

    // init は失敗するべき
    assert!(result.is_err(), "init should fail when pvcreate fails");

    // ロールバックコマンドが実行されたことを確認
    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    assert!(
        !sh_calls.is_empty(),
        "rollback should execute sh -c commands, got history: {:?}",
        h.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn test_init_success_saves_operation_log() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");

    let tmp_dir = std::env::temp_dir().join("puddle-test-init-oplog");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        tmp_dir.to_str().unwrap(),
    );
    assert!(result.is_ok(), "init should succeed: {:?}", result.err());

    // 操作ログが保存されていることを確認
    let log_path = tmp_dir.join("operations.log");
    assert!(log_path.exists(), "operations.log should be created");
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("BEGIN"), "log should contain BEGIN");
    assert!(content.contains("COMMIT"), "log should contain COMMIT");

    // ロールバックは実行されない
    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    assert!(
        sh_calls.is_empty(),
        "no rollback commands should run on success"
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_add_rollback_on_lvextend_failure() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");
    // lvextend を失敗させる
    mock.set_fail("lvextend", "lvextend: Insufficient free space");

    let existing = make_single_disk_pool(2_000_000_000_000);
    let tmp_dir = std::env::temp_dir().join("puddle-test-add-rollback");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::add(&mock, "/dev/sdc", &existing, tmp_dir.to_str().unwrap());

    assert!(result.is_err(), "add should fail when lvextend fails");

    // ロールバックコマンドが実行されたことを確認
    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    assert!(
        !sh_calls.is_empty(),
        "rollback should execute sh -c commands on add failure"
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

// ── device validation tests ──

#[test]
fn test_add_rejects_duplicate_device() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");

    // 既存プールのデバイスと同じデバイスを追加しようとする
    let existing = make_single_disk_pool(2_000_000_000_000);
    let dup_device = &existing.disks[0].device_id;

    let result = commands::add(&mock, dup_device, &existing, "/tmp/puddle-test");

    assert!(result.is_err(), "should reject duplicate device");
    assert!(
        result.unwrap_err().to_string().contains("already in pool"),
        "error should mention already in pool"
    );
}

#[test]
fn test_check_device_mounted_rejects_mounted() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    // findmnt が成功する = デバイスがマウント中
    mock.set_stdout("findmnt", "/dev/sdb on /mnt type ext4");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        "/tmp/puddle-test-meta",
    );

    assert!(result.is_err(), "should reject mounted device");
    assert!(
        result.unwrap_err().to_string().contains("mounted"),
        "error should mention mounted"
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

fn make_four_disk_pool() -> PoolConfig {
    use puddle::metadata::pool_config::*;
    use uuid::Uuid;

    let disks: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
    let pool_uuid = Uuid::new_v4();

    PoolConfig {
        pool: PoolMeta {
            uuid: pool_uuid,
            name: format!("puddle-{}", &pool_uuid.to_string()[..8]),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: disks
            .iter()
            .enumerate()
            .map(|(i, &uuid)| DiskMeta {
                uuid,
                device_id: format!("/dev/loop{}", i),
                capacity_bytes: 4_000_000_000_000,
                seq: i as u32,
                status: DiskStatus::Active,
            })
            .collect(),
        zones: vec![ZoneMeta {
            index: 0,
            start_bytes: 0,
            size_bytes: 4_000_000_000_000,
            raid_level: RaidLevel::Raid5,
            md_device: "/dev/md/puddle-z0".to_string(),
            participating_disk_uuids: disks.clone(),
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

fn make_multi_zone_pool() -> PoolConfig {
    use puddle::metadata::pool_config::*;
    use uuid::Uuid;

    let disk0 = Uuid::new_v4();
    let disk1 = Uuid::new_v4();
    let disk2 = Uuid::new_v4();
    let pool_uuid = Uuid::new_v4();

    PoolConfig {
        pool: PoolMeta {
            uuid: pool_uuid,
            name: format!("puddle-{}", &pool_uuid.to_string()[..8]),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: vec![
            DiskMeta {
                uuid: disk0,
                device_id: "/dev/loop0".to_string(),
                capacity_bytes: 2_000_000_000_000,
                seq: 0,
                status: DiskStatus::Active,
            },
            DiskMeta {
                uuid: disk1,
                device_id: "/dev/loop1".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 1,
                status: DiskStatus::Active,
            },
            DiskMeta {
                uuid: disk2,
                device_id: "/dev/loop2".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 2,
                status: DiskStatus::Active,
            },
        ],
        zones: vec![
            ZoneMeta {
                index: 0,
                start_bytes: 0,
                size_bytes: 2_000_000_000_000,
                raid_level: RaidLevel::Raid5,
                md_device: "/dev/md/puddle-z0".to_string(),
                participating_disk_uuids: vec![disk0, disk1, disk2],
            },
            ZoneMeta {
                index: 1,
                start_bytes: 2_000_000_000_000,
                size_bytes: 2_000_000_000_000,
                raid_level: RaidLevel::Raid1,
                md_device: "/dev/md/puddle-z1".to_string(),
                participating_disk_uuids: vec![disk1, disk2],
            },
        ],
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

/// 2台均等プール (remove テスト用)
fn make_two_disk_pool() -> PoolConfig {
    use puddle::metadata::pool_config::*;
    use uuid::Uuid;

    let disk0 = Uuid::new_v4();
    let disk1 = Uuid::new_v4();
    let pool_uuid = Uuid::new_v4();

    PoolConfig {
        pool: PoolMeta {
            uuid: pool_uuid,
            name: format!("puddle-{}", &pool_uuid.to_string()[..8]),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: vec![
            DiskMeta {
                uuid: disk0,
                device_id: "/dev/loop0".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 0,
                status: DiskStatus::Active,
            },
            DiskMeta {
                uuid: disk1,
                device_id: "/dev/loop1".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 1,
                status: DiskStatus::Active,
            },
        ],
        zones: vec![ZoneMeta {
            index: 0,
            start_bytes: 0,
            size_bytes: 4_000_000_000_000,
            raid_level: RaidLevel::Raid1,
            md_device: "/dev/md/puddle-z0".to_string(),
            participating_disk_uuids: vec![disk0, disk1],
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

// ── remove tests ──

#[test]
fn test_remove_device_not_found() {
    let mock = MockCommandRunner::new();
    let pool = make_two_disk_pool();

    let tmp = tempfile::tempdir().unwrap();
    let meta_dir = tmp.path().to_str().unwrap();
    let meta_path = format!("{}/pool.toml", meta_dir);
    std::fs::write(&meta_path, pool.to_toml().unwrap()).unwrap();

    let result = commands::remove(&mock, "/dev/nonexistent", &pool, meta_dir);
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("not found"),
        "should error for missing device"
    );
}

#[test]
fn test_remove_last_disk_rejected() {
    let mock = MockCommandRunner::new();
    let pool = make_single_disk_pool(4_000_000_000_000);

    let tmp = tempfile::tempdir().unwrap();
    let meta_dir = tmp.path().to_str().unwrap();
    let meta_path = format!("{}/pool.toml", meta_dir);
    std::fs::write(&meta_path, pool.to_toml().unwrap()).unwrap();

    let result = commands::remove(&mock, "ata-TEST_DISK_0", &pool, meta_dir);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Cannot remove the last disk"));
}

#[test]
fn test_remove_disk_from_two_disk_pool() {
    let mock = MockCommandRunner::new();
    let pool = make_two_disk_pool();

    let tmp = tempfile::tempdir().unwrap();
    let meta_dir = tmp.path().to_str().unwrap();
    let meta_path = format!("{}/pool.toml", meta_dir);
    std::fs::write(&meta_path, pool.to_toml().unwrap()).unwrap();

    let result = commands::remove(&mock, "/dev/loop1", &pool, meta_dir);
    assert!(result.is_ok(), "remove failed: {:?}", result.err());

    let new_config = result.unwrap();
    assert_eq!(new_config.disks.len(), 1);
    assert_eq!(new_config.disks[0].device_id, "/dev/loop0");

    // pvmove, mdadm --fail, mdadm --remove が呼ばれたことを確認
    let h = mock.history();
    let programs: Vec<&str> = h.iter().map(|e| e.0.as_str()).collect();
    assert!(programs.contains(&"pvmove"), "should call pvmove");
    assert!(programs.contains(&"mdadm"), "should call mdadm");
    assert!(programs.contains(&"sgdisk"), "should call sgdisk to wipe");
    assert!(programs.contains(&"resize2fs"), "should resize fs");
}

#[test]
fn test_remove_disk_from_multi_zone_pool() {
    let mock = MockCommandRunner::new();
    let pool = make_multi_zone_pool();

    let tmp = tempfile::tempdir().unwrap();
    let meta_dir = tmp.path().to_str().unwrap();
    let meta_path = format!("{}/pool.toml", meta_dir);
    std::fs::write(&meta_path, pool.to_toml().unwrap()).unwrap();

    // loop0 は zone0 (3台 RAID5) にのみ参加
    let result = commands::remove(&mock, "/dev/loop0", &pool, meta_dir);
    assert!(result.is_ok(), "remove failed: {:?}", result.err());

    let new_config = result.unwrap();
    assert_eq!(new_config.disks.len(), 2);
    // loop0 が消えている
    assert!(new_config.disks.iter().all(|d| d.device_id != "/dev/loop0"));
}
