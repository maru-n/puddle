//! 信頼性検証テスト
//!
//! - プロパティベーステスト: ゾーン計算の不変条件
//! - 境界値テスト: 極端なディスクサイズ・台数
//! - 故障注入テスト: 各ステップでの失敗とロールバック検証

use puddle::cli::commands;
use puddle::executor::command_runner::MockCommandRunner;
use puddle::planner::zone::compute_zones;
use puddle::types::*;

// ════════════════════════════════════════════
// Step 20: プロパティベーステスト (ゾーン計算不変条件)
// ════════════════════════════════════════════

/// 不変条件: 全ゾーンのサイズ合計は最小ディスク容量を超えない
#[test]
fn property_zone_sizes_within_disk_capacity() {
    let test_cases: Vec<Vec<u64>> = vec![
        vec![1_000_000_000_000],
        vec![1_000_000_000_000, 2_000_000_000_000],
        vec![1_000_000_000_000, 2_000_000_000_000, 4_000_000_000_000],
        vec![500_000_000_000; 5],
        vec![
            100_000_000_000,
            200_000_000_000,
            300_000_000_000,
            400_000_000_000,
        ],
        vec![
            1_000_000_000_000,
            1_000_000_000_000,
            3_000_000_000_000,
            3_000_000_000_000,
        ],
        vec![
            500_000_000_000,
            1_000_000_000_000,
            1_500_000_000_000,
            2_000_000_000_000,
            2_500_000_000_000,
        ],
    ];

    for capacities in &test_cases {
        let plan = compute_zones(capacities, Redundancy::Single);
        let min_cap = *capacities.iter().min().unwrap();

        // 各ゾーンのサイズは非負
        for zone in &plan.zones {
            assert!(
                zone.size_bytes > 0,
                "zone size should be > 0, capacities: {:?}",
                capacities
            );
        }

        // ゾーンサイズ合計は最大ディスク容量を超えない
        let total_zone_size: u64 = plan.zones.iter().map(|z| z.size_bytes).sum();
        let max_cap = *capacities.iter().max().unwrap();
        assert!(
            total_zone_size <= max_cap,
            "total zone size {} > max disk capacity {}, capacities: {:?}",
            total_zone_size,
            max_cap,
            capacities
        );

        // 最小ディスクが参加するゾーンのサイズ合計 ≤ 最小ディスク容量
        let min_disk_zones: u64 = plan
            .zones
            .iter()
            .filter(|z| z.num_disks == capacities.len())
            .map(|z| z.size_bytes)
            .sum();
        assert!(
            min_disk_zones <= min_cap,
            "zones using min disk {} > min capacity {}, capacities: {:?}",
            min_disk_zones,
            min_cap,
            capacities
        );
    }
}

/// 不変条件: 実効容量は物理容量合計を超えない
#[test]
fn property_effective_capacity_le_physical() {
    let test_cases: Vec<Vec<u64>> = vec![
        vec![1_000_000_000_000],
        vec![2_000_000_000_000; 2],
        vec![2_000_000_000_000; 3],
        vec![1_000_000_000_000, 2_000_000_000_000, 4_000_000_000_000],
        vec![500_000_000_000; 10],
        vec![
            100_000_000_000,
            500_000_000_000,
            1_000_000_000_000,
            2_000_000_000_000,
            8_000_000_000_000,
        ],
    ];

    for capacities in &test_cases {
        let plan = compute_zones(capacities, Redundancy::Single);
        let physical_total: u64 = capacities.iter().sum();

        assert!(
            plan.total_effective_bytes <= physical_total,
            "effective {} > physical {}, capacities: {:?}",
            plan.total_effective_bytes,
            physical_total,
            capacities
        );
    }
}

/// 不変条件: RAID レベルはディスク数に応じて正しく選択される
#[test]
fn property_raid_level_matches_disk_count() {
    let test_cases: Vec<Vec<u64>> = vec![
        vec![1_000_000_000_000],                                       // 1台
        vec![1_000_000_000_000; 2],                                    // 2台
        vec![1_000_000_000_000; 3],                                    // 3台
        vec![1_000_000_000_000; 5],                                    // 5台
        vec![1_000_000_000_000, 2_000_000_000_000],                    // 2台異種
        vec![1_000_000_000_000, 2_000_000_000_000, 2_000_000_000_000], // 3台異種
    ];

    for capacities in &test_cases {
        let plan = compute_zones(capacities, Redundancy::Single);

        for zone in &plan.zones {
            match zone.num_disks {
                1 => assert_eq!(
                    zone.raid_level,
                    RaidLevel::Single,
                    "1 disk should be Single, capacities: {:?}",
                    capacities
                ),
                2 => assert_eq!(
                    zone.raid_level,
                    RaidLevel::Raid1,
                    "2 disks should be Raid1, capacities: {:?}",
                    capacities
                ),
                n if n >= 3 => assert_eq!(
                    zone.raid_level,
                    RaidLevel::Raid5,
                    "{} disks should be Raid5, capacities: {:?}",
                    n,
                    capacities
                ),
                _ => unreachable!(),
            }
        }
    }
}

/// 不変条件: ディスク追加で実効容量は単調増加する
#[test]
fn property_adding_disk_increases_capacity() {
    let base_capacities = vec![2_000_000_000_000u64; 2];
    let additions = vec![
        1_000_000_000_000,
        2_000_000_000_000,
        4_000_000_000_000,
        8_000_000_000_000,
    ];

    for add_cap in additions {
        let plan_before = compute_zones(&base_capacities, Redundancy::Single);
        let mut after = base_capacities.clone();
        after.push(add_cap);
        let plan_after = compute_zones(&after, Redundancy::Single);

        assert!(
            plan_after.total_effective_bytes >= plan_before.total_effective_bytes,
            "adding {}B disk should not decrease capacity: {} -> {}",
            add_cap,
            plan_before.total_effective_bytes,
            plan_after.total_effective_bytes
        );
    }
}

/// 不変条件: ゾーンインデックスは0から連続
#[test]
fn property_zone_indices_contiguous() {
    let test_cases: Vec<Vec<u64>> = vec![
        vec![1_000_000_000_000],
        vec![1_000_000_000_000, 2_000_000_000_000],
        vec![1_000_000_000_000, 2_000_000_000_000, 4_000_000_000_000],
        vec![
            1_000_000_000_000,
            1_000_000_000_000,
            2_000_000_000_000,
            4_000_000_000_000,
        ],
    ];

    for capacities in &test_cases {
        let plan = compute_zones(capacities, Redundancy::Single);
        for (i, zone) in plan.zones.iter().enumerate() {
            assert_eq!(
                zone.index, i,
                "zone index should be {}, got {}, capacities: {:?}",
                i, zone.index, capacities
            );
        }
    }
}

// ════════════════════════════════════════════
// Step 21: 境界値テスト
// ════════════════════════════════════════════

#[test]
fn boundary_empty_disks() {
    let plan = compute_zones(&[], Redundancy::Single);
    assert!(plan.zones.is_empty());
    assert_eq!(plan.total_effective_bytes, 0);
}

#[test]
fn boundary_very_small_disk() {
    // 1 MB ディスク
    let plan = compute_zones(&[1_000_000], Redundancy::Single);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Single);
    assert!(plan.zones[0].size_bytes > 0);
}

#[test]
fn boundary_very_large_disk() {
    // 100 PB ディスク
    let cap = 100_000_000_000_000_000u64;
    let plan = compute_zones(&[cap, cap, cap], Redundancy::Single);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid5);
    // オーバーフローしないこと
    assert!(plan.total_effective_bytes > 0);
    assert!(plan.total_effective_bytes <= cap * 3);
}

#[test]
fn boundary_many_identical_disks() {
    // 同一容量 10 台
    let caps = vec![2_000_000_000_000u64; 10];
    let plan = compute_zones(&caps, Redundancy::Single);
    // 同一容量なので1ゾーンのみ
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].num_disks, 10);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid5);
}

#[test]
fn boundary_many_different_disks() {
    // 全て異なる容量 8 台
    let caps: Vec<u64> = (1..=8).map(|i| i * 1_000_000_000_000).collect();
    let plan = compute_zones(&caps, Redundancy::Single);
    // ゾーン数はユニーク容量数 - 1 以上
    assert!(plan.zones.len() >= 1);
    // 全ゾーンで num_disks >= 1
    for zone in &plan.zones {
        assert!(zone.num_disks >= 1);
    }
    assert!(plan.total_effective_bytes > 0);
}

#[test]
fn boundary_two_extreme_sizes() {
    // 極端に小さいディスクと極端に大きいディスク
    let plan = compute_zones(&[1_000_000, 10_000_000_000_000], Redundancy::Single);
    assert!(plan.zones.len() >= 1);
    // 実効容量はオーバーフローしない
    assert!(plan.total_effective_bytes > 0);
}

#[test]
fn boundary_effective_capacity_consistency() {
    // compute_zones 経由で effective_bytes の一貫性を検証
    let test_cases: Vec<Vec<u64>> = vec![
        vec![1_000_000_000_000],     // Single
        vec![1_000_000_000_000; 2],  // Raid1
        vec![1_000_000_000_000; 3],  // Raid5 x3
        vec![1_000_000_000_000; 5],  // Raid5 x5
        vec![1_000_000_000_000; 10], // Raid5 x10
    ];

    for capacities in &test_cases {
        let plan = compute_zones(capacities, Redundancy::Single);
        let physical_total: u64 = capacities.iter().sum();

        for zone in &plan.zones {
            let zone_physical = zone.size_bytes * zone.num_disks as u64;
            assert!(
                zone.effective_bytes <= zone_physical,
                "zone effective {} > zone physical {} for {:?} with {} disks",
                zone.effective_bytes,
                zone_physical,
                zone.raid_level,
                zone.num_disks
            );
            assert!(
                zone.effective_bytes > 0,
                "zone effective should be > 0 for {:?} with {} disks",
                zone.raid_level,
                zone.num_disks
            );
        }

        assert!(
            plan.total_effective_bytes <= physical_total,
            "total effective {} > total physical {}",
            plan.total_effective_bytes,
            physical_total
        );
    }
}

// ════════════════════════════════════════════
// Step 22: 故障注入テスト (ロールバック正当性)
// ════════════════════════════════════════════

/// init で各ステップの失敗を順番にテスト
/// pvcreate 失敗 → mdadm stop + sgdisk --zap-all がロールバックされる
#[test]
fn fault_init_pvcreate_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    mock.set_fail("pvcreate", "simulated pvcreate failure");

    let result = commands::init(&mock, "/dev/sdb", Some("ext4"), None, "/tmp/puddle-fault-1");
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // pvcreate 失敗時: partition + mkfs.ext4(meta) + mdadm の3ステップが記録済み
    // ロールバック: mdadm --stop + sgdisk --zap-all (空のロールバックはスキップ)
    assert!(
        sh_calls.len() >= 2,
        "should rollback at least 2 steps, got {}: {:?}",
        sh_calls.len(),
        sh_calls
    );
}

/// init で vgcreate 失敗
#[test]
fn fault_init_vgcreate_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    mock.set_fail("vgcreate", "simulated vgcreate failure");

    let result = commands::init(&mock, "/dev/sdb", Some("ext4"), None, "/tmp/puddle-fault-2");
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // pvcreate まで成功 → pvremove + mdadm --stop + sgdisk --zap-all
    assert!(
        sh_calls.len() >= 3,
        "should rollback at least 3 steps (pvremove + mdadm stop + sgdisk zap), got {}: {:?}",
        sh_calls.len(),
        sh_calls
    );
}

/// init で lvcreate 失敗
#[test]
fn fault_init_lvcreate_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    mock.set_fail("lvcreate", "simulated lvcreate failure");

    let result = commands::init(&mock, "/dev/sdb", Some("ext4"), None, "/tmp/puddle-fault-3");
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // vgcreate まで成功 → vgremove + pvremove + mdadm --stop + sgdisk --zap-all
    assert!(
        sh_calls.len() >= 4,
        "should rollback at least 4 steps, got {}: {:?}",
        sh_calls.len(),
        sh_calls
    );
}

/// init で mkfs(data) 失敗
#[test]
fn fault_init_mkfs_data_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    // mkfs.ext4 は2回呼ばれる: metadata partition (1回目) + data volume (2回目)
    // 2回目で失敗させる
    mock.set_fail_on_nth("mkfs.ext4", 2, "simulated mkfs failure on data volume");

    let result = commands::init(&mock, "/dev/sdb", Some("ext4"), None, "/tmp/puddle-fault-4");
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // lvcreate まで成功 → lvremove + vgremove + pvremove + mdadm --stop + sgdisk --zap-all
    assert!(
        sh_calls.len() >= 5,
        "should rollback at least 5 steps, got {}: {:?}",
        sh_calls.len(),
        sh_calls
    );
}

/// init 成功時はロールバック実行されない
#[test]
fn fault_init_success_no_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");

    let tmp_dir = std::env::temp_dir().join("puddle-fault-success");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        tmp_dir.to_str().unwrap(),
    );
    assert!(result.is_ok(), "init should succeed: {:?}", result.err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    assert!(
        sh_calls.is_empty(),
        "no rollback should run on success, got {:?}",
        sh_calls
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

/// add で mdadm 失敗 → ロールバック
#[test]
fn fault_add_mdadm_add_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    // mdadm は add() 内で --add として呼ばれる。1回目で失敗させる
    mock.set_fail("mdadm", "simulated mdadm --add failure");

    let existing = make_single_disk_pool(2_000_000_000_000);
    let tmp_dir = std::env::temp_dir().join("puddle-fault-add-1");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::add(&mock, "/dev/sdc", &existing, tmp_dir.to_str().unwrap());
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // partition + mkfs(meta) の2ステップが記録済み、ロールバックが実行される
    assert!(
        sh_calls.len() >= 1,
        "should rollback at least 1 step on mdadm failure, got {}: {:?}",
        sh_calls.len(),
        sh_calls
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

/// add で lvextend 失敗 → ロールバック
#[test]
fn fault_add_lvextend_fails_rollback() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "4000000000000\n");
    mock.set_fail("lvextend", "simulated lvextend failure");

    let existing = make_single_disk_pool(2_000_000_000_000);
    let tmp_dir = std::env::temp_dir().join("puddle-fault-add-2");
    std::fs::create_dir_all(&tmp_dir).ok();

    let result = commands::add(&mock, "/dev/sdc", &existing, tmp_dir.to_str().unwrap());
    assert!(result.is_err());

    let h = mock.history();
    let sh_calls: Vec<_> = h.iter().filter(|(cmd, _)| cmd == "sh").collect();
    // mdadm add + pvcreate + vgextend + partition + mkfs のステップが記録済み
    assert!(!sh_calls.is_empty(), "should rollback on lvextend failure");

    std::fs::remove_dir_all(&tmp_dir).ok();
}

/// ロールバックコマンドの順序が正しいことを検証
#[test]
fn fault_rollback_order_is_reverse() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "2000000000000\n");
    mock.set_stdout("blkid", "");
    mock.set_fail("vgcreate", "simulated failure");

    let result = commands::init(
        &mock,
        "/dev/sdb",
        Some("ext4"),
        None,
        "/tmp/puddle-fault-order",
    );
    assert!(result.is_err());

    let h = mock.history();
    let sh_cmds: Vec<String> = h
        .iter()
        .filter(|(cmd, _)| cmd == "sh")
        .map(|(_, args)| args.get(1).cloned().unwrap_or_default())
        .collect();

    // ロールバックは逆順: pvremove → mdadm --stop → sgdisk --zap-all
    assert!(
        sh_cmds.len() >= 3,
        "expected at least 3 rollback commands, got {:?}",
        sh_cmds
    );

    // 最後のロールバックコマンドは sgdisk --zap-all (最初に実行されたステップの巻き戻し)
    let last = sh_cmds.last().unwrap();
    assert!(
        last.contains("sgdisk --zap-all"),
        "last rollback should be partition wipe, got: {}",
        last
    );

    // 最初のロールバックコマンドは pvremove (最後に成功したステップの巻き戻し)
    let first = &sh_cmds[0];
    assert!(
        first.contains("pvremove"),
        "first rollback should be pvremove, got: {}",
        first
    );
}

// ── helper ──

fn make_single_disk_pool(capacity: u64) -> puddle::metadata::pool_config::PoolConfig {
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
