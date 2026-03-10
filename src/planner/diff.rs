use crate::types::*;

/// ゾーンの変更種別
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneChange {
    /// 新しいゾーンが追加される
    Added(ZoneSpec),
    /// 既存ゾーンの RAID レベルが変わる (昇格 or 降格)
    RaidUpgrade {
        zone_index: usize,
        old_level: RaidLevel,
        new_level: RaidLevel,
        old_num_disks: usize,
        new_num_disks: usize,
    },
    /// ゾーンが削除される
    Removed(ZoneSpec),
    /// 変更なし
    Unchanged(ZoneSpec),
}

impl ZoneChange {
    pub fn is_upgrade(&self) -> bool {
        matches!(self, ZoneChange::RaidUpgrade { .. })
    }
}

/// リプラン差分
#[derive(Debug, Clone)]
pub struct ReplanDiff {
    pub changes: Vec<ZoneChange>,
    pub old_effective_bytes: u64,
    pub new_effective_bytes: u64,
    pub capacity_delta: i64,
}

/// 2つのディスク構成間のリプラン差分を計算する
pub fn compute_replan(
    old_capacities: &[u64],
    new_capacities: &[u64],
    redundancy: Redundancy,
) -> ReplanDiff {
    use super::zone::compute_zones;

    let old_plan = compute_zones(old_capacities, redundancy);
    let new_plan = compute_zones(new_capacities, redundancy);

    let max_zones = old_plan.zones.len().max(new_plan.zones.len());
    let mut changes = Vec::new();

    for i in 0..max_zones {
        let old_zone = old_plan.zones.get(i);
        let new_zone = new_plan.zones.get(i);

        match (old_zone, new_zone) {
            (None, Some(nz)) => {
                changes.push(ZoneChange::Added(nz.clone()));
            }
            (Some(oz), None) => {
                changes.push(ZoneChange::Removed(oz.clone()));
            }
            (Some(oz), Some(nz)) => {
                if oz.raid_level != nz.raid_level || oz.num_disks != nz.num_disks {
                    changes.push(ZoneChange::RaidUpgrade {
                        zone_index: i,
                        old_level: oz.raid_level,
                        new_level: nz.raid_level,
                        old_num_disks: oz.num_disks,
                        new_num_disks: nz.num_disks,
                    });
                } else if oz == nz {
                    changes.push(ZoneChange::Unchanged(nz.clone()));
                } else {
                    // サイズ変更等
                    changes.push(ZoneChange::RaidUpgrade {
                        zone_index: i,
                        old_level: oz.raid_level,
                        new_level: nz.raid_level,
                        old_num_disks: oz.num_disks,
                        new_num_disks: nz.num_disks,
                    });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    let capacity_delta =
        new_plan.total_effective_bytes as i64 - old_plan.total_effective_bytes as i64;

    ReplanDiff {
        changes,
        old_effective_bytes: old_plan.total_effective_bytes,
        new_effective_bytes: new_plan.total_effective_bytes,
        capacity_delta,
    }
}
