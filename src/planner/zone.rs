use crate::types::*;

/// RAID レベルを選択する
///
/// ディスク数と冗長性レベルに基づいて最適な RAID レベルを決定。
fn select_raid_level(num_disks: usize, redundancy: Redundancy) -> RaidLevel {
    match redundancy {
        Redundancy::Single => match num_disks {
            0 => unreachable!("select_raid_level called with 0 disks"),
            1 => RaidLevel::Single,
            2 => RaidLevel::Raid1,
            _ => RaidLevel::Raid5,
        },
        Redundancy::Dual => match num_disks {
            0 => unreachable!("select_raid_level called with 0 disks"),
            1 => RaidLevel::Single,
            2 => RaidLevel::Raid1,
            3 => RaidLevel::Raid1, // 3台ミラー
            _ => RaidLevel::Raid6,
        },
    }
}

/// ゾーンの実効容量を計算する
fn calc_effective(zone_size: u64, num_disks: usize, raid_level: RaidLevel) -> u64 {
    match raid_level {
        RaidLevel::Single => zone_size,
        RaidLevel::Raid1 => zone_size, // ミラー: 1台分
        RaidLevel::Raid5 => zone_size * (num_disks as u64 - 1),
        RaidLevel::Raid6 => zone_size * (num_disks as u64 - 2),
    }
}

/// 警告を生成する
fn check_warnings(
    zone_index: usize,
    raid_level: RaidLevel,
    num_disks: usize,
    redundancy: Redundancy,
) -> Vec<Warning> {
    let mut warnings = Vec::new();

    if raid_level == RaidLevel::Single {
        warnings.push(Warning::NoRedundancy { zone_index });
    }

    if redundancy == Redundancy::Dual {
        match (raid_level, num_disks) {
            (RaidLevel::Raid1, 2) => {
                warnings.push(Warning::InsufficientRedundancy {
                    zone_index,
                    achieved: Redundancy::Single,
                });
            }
            (RaidLevel::Raid1, 3) => {
                // 3台ミラーはデュアル冗長達成だが RAID6 ほど効率的ではない
            }
            _ => {}
        }
    }

    warnings
}

/// ディスク容量リストからゾーン分割を計算する
///
/// SPEC §3.2 のアルゴリズムに基づく。
/// 入力はソート済みでなくてもよい（内部でソートする）。
pub fn compute_zones(capacities: &[u64], redundancy: Redundancy) -> ZonePlan {
    if capacities.is_empty() {
        return ZonePlan {
            zones: vec![],
            warnings: vec![],
            total_effective_bytes: 0,
            total_physical_bytes: 0,
        };
    }

    let mut sorted = capacities.to_vec();
    sorted.sort_unstable();

    let mut zones = Vec::new();
    let mut warnings = Vec::new();
    let mut prev_boundary: u64 = 0;
    let mut zone_index = 0;

    for i in 0..sorted.len() {
        let boundary = sorted[i];
        let zone_size = boundary - prev_boundary;

        if zone_size == 0 {
            continue; // 同一容量のディスクが続く場合スキップ
        }

        let num_disks = sorted.len() - i; // この境界以降に参加するディスク数
        let raid_level = select_raid_level(num_disks, redundancy);
        let effective = calc_effective(zone_size, num_disks, raid_level);

        warnings.extend(check_warnings(
            zone_index, raid_level, num_disks, redundancy,
        ));

        zones.push(ZoneSpec {
            index: zone_index,
            start_bytes: prev_boundary,
            size_bytes: zone_size,
            raid_level,
            num_disks,
            effective_bytes: effective,
        });

        zone_index += 1;
        prev_boundary = boundary;
    }

    let total_effective: u64 = zones.iter().map(|z| z.effective_bytes).sum();
    let total_physical: u64 = sorted.iter().sum();

    ZonePlan {
        zones,
        warnings,
        total_effective_bytes: total_effective,
        total_physical_bytes: total_physical,
    }
}
