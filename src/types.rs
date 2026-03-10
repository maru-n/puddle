use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// RAID レベル
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RaidLevel {
    Single,
    Raid1,
    Raid5,
    Raid6,
}

impl RaidLevel {
    /// パリティ/ミラーに使われるディスク数
    pub fn parity_count(self) -> u64 {
        match self {
            RaidLevel::Single => 0,
            RaidLevel::Raid1 => 0, // RAID1 は特殊: 実効 = 1台分
            RaidLevel::Raid5 => 1,
            RaidLevel::Raid6 => 2,
        }
    }
}

/// 冗長性レベル
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Redundancy {
    Single,
    Dual,
}

/// ディスクの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskStatus {
    Active,
    Failed,
    Removing,
}

/// プールの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PoolStatus {
    Healthy,
    Degraded,
    Critical,
}

/// ゾーンやディスク構成に対する警告
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// このゾーンに冗長性がない (SINGLE)
    NoRedundancy { zone_index: usize },
    /// デュアル冗長が要求されたが達成できない
    InsufficientRedundancy {
        zone_index: usize,
        achieved: Redundancy,
    },
}

/// ディスク情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub uuid: Uuid,
    pub device_id: String,
    pub capacity_bytes: u64,
    pub seq: u32,
    pub status: DiskStatus,
}

/// ゾーン仕様 (planner の出力)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneSpec {
    pub index: usize,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub raid_level: RaidLevel,
    pub num_disks: usize,
    pub effective_bytes: u64,
}

/// ゾーン分割の計算結果
#[derive(Debug, Clone)]
pub struct ZonePlan {
    pub zones: Vec<ZoneSpec>,
    pub warnings: Vec<Warning>,
    pub total_effective_bytes: u64,
    pub total_physical_bytes: u64,
}
