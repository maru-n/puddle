use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::*;

/// プールのメタ情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolMeta {
    pub uuid: Uuid,
    pub name: String,
    pub created_at: String,
    pub redundancy: Redundancy,
}

/// ディスクのメタ情報 (メタデータ TOML 用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMeta {
    pub uuid: Uuid,
    pub device_id: String,
    pub capacity_bytes: u64,
    pub seq: u32,
    pub status: DiskStatus,
}

/// ゾーンのメタ情報 (メタデータ TOML 用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMeta {
    pub index: usize,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub raid_level: RaidLevel,
    pub md_device: String,
    pub participating_disk_uuids: Vec<Uuid>,
    /// LVM 割り当て可能かどうか (非冗長ゾーンは false で遅延割り当て)
    #[serde(default = "default_allocatable")]
    pub allocatable: bool,
}

fn default_allocatable() -> bool {
    true
}

/// LVM のメタ情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LvmMeta {
    pub vg_name: String,
    pub lv_name: String,
    pub filesystem: String,
    pub mount_point: String,
}

/// プール状態のメタ情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMeta {
    pub pool_status: PoolStatus,
    pub last_scrub: Option<String>,
    pub version: u32,
}

/// プール全体の設定 (メタデータ TOML のルート)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub pool: PoolMeta,
    pub disks: Vec<DiskMeta>,
    pub zones: Vec<ZoneMeta>,
    pub lvm: LvmMeta,
    pub state: StateMeta,
}

impl PoolConfig {
    /// TOML 文字列にシリアライズ
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("Failed to serialize PoolConfig to TOML")
    }

    /// TOML 文字列からデシリアライズ
    pub fn from_toml(s: &str) -> Result<Self> {
        toml::from_str(s).context("Failed to deserialize PoolConfig from TOML")
    }
}
