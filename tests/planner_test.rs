use puddle::planner::capacity::{format_bytes, summarize};
use puddle::planner::diff::{compute_replan, ZoneChange};
use puddle::planner::zone::compute_zones;
use puddle::types::*;

/// ヘルパー: TB をバイトに変換
fn tb(n: u64) -> u64 {
    n * 1_000_000_000_000
}

#[test]
fn test_three_equal_disks() {
    let capacities = vec![tb(4), tb(4), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid5);
    assert_eq!(plan.zones[0].num_disks, 3);
    assert_eq!(plan.zones[0].effective_bytes, tb(8));
    assert_eq!(plan.total_effective_bytes, tb(8));
    assert!(plan.warnings.is_empty());
}

#[test]
fn test_mixed_disks_2_4_4() {
    let capacities = vec![tb(2), tb(4), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 2);

    // Zone 0: 3台 × 2TB, RAID5
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid5);
    assert_eq!(plan.zones[0].num_disks, 3);
    assert_eq!(plan.zones[0].size_bytes, tb(2));
    assert_eq!(plan.zones[0].effective_bytes, tb(4));

    // Zone 1: 2台 × 2TB, RAID1
    assert_eq!(plan.zones[1].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[1].num_disks, 2);
    assert_eq!(plan.zones[1].size_bytes, tb(2));
    assert_eq!(plan.zones[1].effective_bytes, tb(2));

    assert_eq!(plan.total_effective_bytes, tb(6));
    assert!(plan.warnings.is_empty());
}

#[test]
fn test_single_disk_warns_no_redundancy() {
    let capacities = vec![tb(2)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Single);
    assert_eq!(plan.zones[0].effective_bytes, tb(2));
    assert_eq!(plan.warnings.len(), 1);
    assert!(matches!(
        &plan.warnings[0],
        Warning::NoRedundancy { zone_index: 0 }
    ));
}

#[test]
fn test_two_equal_disks() {
    let capacities = vec![tb(4), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[0].num_disks, 2);
    assert_eq!(plan.zones[0].effective_bytes, tb(4));
    assert!(plan.warnings.is_empty());
}

#[test]
fn test_two_different_disks() {
    let capacities = vec![tb(2), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 2);

    // Zone 0: 2台 × 2TB, RAID1
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[0].effective_bytes, tb(2));

    // Zone 1: 1台 × 2TB, SINGLE (冗長なし)
    assert_eq!(plan.zones[1].raid_level, RaidLevel::Single);
    assert_eq!(plan.zones[1].effective_bytes, tb(2));

    assert_eq!(plan.total_effective_bytes, tb(4));
    // Zone 1 に冗長性なし警告
    assert_eq!(plan.warnings.len(), 1);
}

#[test]
fn test_four_disks_with_duplicate_capacities() {
    // 同一容量が連続するケース: skip 処理の確認
    let capacities = vec![tb(2), tb(2), tb(4), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert_eq!(plan.zones.len(), 2);

    // Zone 0: 4台 × 2TB, RAID5
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid5);
    assert_eq!(plan.zones[0].num_disks, 4);
    assert_eq!(plan.zones[0].effective_bytes, tb(6)); // (4-1) × 2TB

    // Zone 1: 2台 × 2TB, RAID1
    assert_eq!(plan.zones[1].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[1].num_disks, 2);
    assert_eq!(plan.zones[1].effective_bytes, tb(2));

    assert_eq!(plan.total_effective_bytes, tb(8));
}

#[test]
fn test_empty_disks_returns_empty_plan() {
    let capacities: Vec<u64> = vec![];
    let plan = compute_zones(&capacities, Redundancy::Single);

    assert!(plan.zones.is_empty());
    assert_eq!(plan.total_effective_bytes, 0);
}

#[test]
fn test_unsorted_input_is_handled() {
    // compute_zones はソート済みを前提とするが、未ソートでも正しく動くべき
    let capacities = vec![tb(4), tb(2), tb(4)];
    let plan = compute_zones(&capacities, Redundancy::Single);

    // ソートされた [2, 4, 4] と同じ結果になるべき
    assert_eq!(plan.zones.len(), 2);
    assert_eq!(plan.total_effective_bytes, tb(6));
}

#[test]
fn test_gradual_expansion_1_to_2_to_3() {
    // 1台
    let plan1 = compute_zones(&vec![tb(2)], Redundancy::Single);
    assert_eq!(plan1.zones[0].raid_level, RaidLevel::Single);

    // 2台
    let plan2 = compute_zones(&vec![tb(2), tb(4)], Redundancy::Single);
    assert_eq!(plan2.zones[0].raid_level, RaidLevel::Raid1);
    assert_eq!(plan2.zones[1].raid_level, RaidLevel::Single);

    // 3台
    let plan3 = compute_zones(&vec![tb(2), tb(4), tb(4)], Redundancy::Single);
    assert_eq!(plan3.zones[0].raid_level, RaidLevel::Raid5);
    assert_eq!(plan3.zones[1].raid_level, RaidLevel::Raid1);
}

// ── capacity tests ──

#[test]
fn test_capacity_summary() {
    let plan = compute_zones(&vec![tb(2), tb(4), tb(4)], Redundancy::Single);
    let summary = summarize(&plan);

    assert_eq!(summary.physical_bytes, tb(10));
    assert_eq!(summary.usable_bytes, tb(6));
    assert_eq!(summary.overhead_bytes, tb(4));
}

#[test]
fn test_format_bytes_tb() {
    assert_eq!(format_bytes(4_000_000_000_000), "4.0 TB");
    assert_eq!(format_bytes(2_500_000_000_000), "2.5 TB");
}

#[test]
fn test_format_bytes_gb() {
    assert_eq!(format_bytes(500_000_000_000), "500.0 GB");
}

#[test]
fn test_format_bytes_mb() {
    assert_eq!(format_bytes(16_000_000), "16 MB");
}

// ── diff tests ──

#[test]
fn test_replan_add_second_disk() {
    let before = vec![tb(2)];
    let after = vec![tb(2), tb(4)];
    let diff = compute_replan(&before, &after, Redundancy::Single);

    // Zone 0: SINGLE → RAID1 (昇格)
    assert!(diff.changes[0].is_upgrade());
    // Zone 1: 新規追加 (SINGLE)
    assert!(matches!(&diff.changes[1], ZoneChange::Added(_)));
    assert_eq!(diff.capacity_delta, tb(2) as i64);
}

#[test]
fn test_replan_add_third_disk() {
    let before = vec![tb(2), tb(4)];
    let after = vec![tb(2), tb(4), tb(4)];
    let diff = compute_replan(&before, &after, Redundancy::Single);

    // Zone 0: RAID1 → RAID5
    assert!(diff.changes[0].is_upgrade());
    if let ZoneChange::RaidUpgrade {
        old_level,
        new_level,
        ..
    } = &diff.changes[0]
    {
        assert_eq!(*old_level, RaidLevel::Raid1);
        assert_eq!(*new_level, RaidLevel::Raid5);
    }

    // Zone 1: SINGLE → RAID1
    assert!(diff.changes[1].is_upgrade());

    assert_eq!(diff.old_effective_bytes, tb(4));
    assert_eq!(diff.new_effective_bytes, tb(6));
    assert_eq!(diff.capacity_delta, tb(2) as i64);
}

// ── Dual Redundancy (RAID6) tests ──

#[test]
fn test_dual_single_disk_warns() {
    let plan = compute_zones(&[tb(4)], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Single);
    assert!(plan
        .warnings
        .iter()
        .any(|w| matches!(w, Warning::NoRedundancy { .. })));
}

#[test]
fn test_dual_two_disks_raid1_with_warning() {
    let plan = compute_zones(&[tb(4), tb(4)], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid1);
    // デュアル冗長不達成の警告
    assert!(plan.warnings.iter().any(|w| matches!(
        w,
        Warning::InsufficientRedundancy {
            achieved: Redundancy::Single,
            ..
        }
    )));
}

#[test]
fn test_dual_three_disks_raid1_mirror() {
    let plan = compute_zones(&[tb(4), tb(4), tb(4)], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[0].num_disks, 3);
    // 3台ミラーなので実効容量 = 1台分
    assert_eq!(plan.zones[0].effective_bytes, tb(4));
}

#[test]
fn test_dual_four_disks_raid6() {
    let plan = compute_zones(&[tb(4); 4], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid6);
    assert_eq!(plan.zones[0].num_disks, 4);
    // RAID6: 実効容量 = (4 - 2) × 4TB = 8TB
    assert_eq!(plan.zones[0].effective_bytes, tb(8));
    assert!(plan.warnings.is_empty());
}

#[test]
fn test_dual_five_disks_raid6() {
    let plan = compute_zones(&[tb(2); 5], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 1);
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid6);
    assert_eq!(plan.zones[0].num_disks, 5);
    // RAID6: (5 - 2) × 2TB = 6TB
    assert_eq!(plan.zones[0].effective_bytes, tb(6));
}

#[test]
fn test_dual_mixed_disks_zones() {
    // 2TB + 4TB × 3 = Zone0 (4台 RAID6 × 2TB) + Zone1 (3台 RAID1 × 2TB)
    let plan = compute_zones(&[tb(2), tb(4), tb(4), tb(4)], Redundancy::Dual);
    assert_eq!(plan.zones.len(), 2);

    // Zone 0: 4台 × 2TB, RAID6
    assert_eq!(plan.zones[0].raid_level, RaidLevel::Raid6);
    assert_eq!(plan.zones[0].num_disks, 4);
    assert_eq!(plan.zones[0].effective_bytes, tb(4)); // (4-2) × 2TB

    // Zone 1: 3台 × 2TB, RAID1 (3台ミラー)
    assert_eq!(plan.zones[1].raid_level, RaidLevel::Raid1);
    assert_eq!(plan.zones[1].num_disks, 3);
    assert_eq!(plan.zones[1].effective_bytes, tb(2)); // ミラー = 1台分
}

#[test]
fn test_dual_effective_less_than_single() {
    // デュアル冗長は常にシングル冗長より実効容量が小さい (4台以上)
    let caps = vec![tb(4); 5];
    let single = compute_zones(&caps, Redundancy::Single);
    let dual = compute_zones(&caps, Redundancy::Dual);

    assert!(
        dual.total_effective_bytes < single.total_effective_bytes,
        "dual {} should be less than single {}",
        dual.total_effective_bytes,
        single.total_effective_bytes
    );
}

#[test]
fn test_dual_gradual_expansion() {
    // 1台→2台→3台→4台の段階的拡張 (Dual)
    let plan1 = compute_zones(&[tb(4)], Redundancy::Dual);
    assert_eq!(plan1.zones[0].raid_level, RaidLevel::Single);

    let plan2 = compute_zones(&[tb(4); 2], Redundancy::Dual);
    assert_eq!(plan2.zones[0].raid_level, RaidLevel::Raid1);

    let plan3 = compute_zones(&[tb(4); 3], Redundancy::Dual);
    assert_eq!(plan3.zones[0].raid_level, RaidLevel::Raid1); // 3台ミラー

    let plan4 = compute_zones(&[tb(4); 4], Redundancy::Dual);
    assert_eq!(plan4.zones[0].raid_level, RaidLevel::Raid6); // ここで RAID6 に
}

#[test]
fn test_replan_no_change() {
    let disks = vec![tb(4), tb(4), tb(4)];
    let diff = compute_replan(&disks, &disks, Redundancy::Single);

    assert_eq!(diff.changes.len(), 1);
    assert!(matches!(&diff.changes[0], ZoneChange::Unchanged(_)));
    assert_eq!(diff.capacity_delta, 0);
}
