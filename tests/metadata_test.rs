use puddle::metadata::pool_config::PoolConfig;
use puddle::types::*;
use uuid::Uuid;

fn sample_config() -> PoolConfig {
    let disk0_uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let disk1_uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
    let disk2_uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();

    PoolConfig {
        pool: puddle::metadata::pool_config::PoolMeta {
            uuid: Uuid::parse_str("a1b2c3d4-e5f6-0000-0000-000000000000").unwrap(),
            name: "mypool".to_string(),
            created_at: "2026-03-10T12:00:00Z".to_string(),
            redundancy: Redundancy::Single,
        },
        disks: vec![
            puddle::metadata::pool_config::DiskMeta {
                uuid: disk0_uuid,
                device_id: "ata-Samsung_SSD_870_EVO_2TB_S1234".to_string(),
                capacity_bytes: 2_000_000_000_000,
                seq: 0,
                status: DiskStatus::Active,
            },
            puddle::metadata::pool_config::DiskMeta {
                uuid: disk1_uuid,
                device_id: "ata-WDC_WD40EFRX_1234".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 1,
                status: DiskStatus::Active,
            },
            puddle::metadata::pool_config::DiskMeta {
                uuid: disk2_uuid,
                device_id: "ata-WDC_WD40EFRX_5678".to_string(),
                capacity_bytes: 4_000_000_000_000,
                seq: 2,
                status: DiskStatus::Active,
            },
        ],
        zones: vec![
            puddle::metadata::pool_config::ZoneMeta {
                index: 0,
                start_bytes: 0,
                size_bytes: 2_000_000_000_000,
                raid_level: RaidLevel::Raid5,
                md_device: "/dev/md/puddle-z0".to_string(),
                participating_disk_uuids: vec![disk0_uuid, disk1_uuid, disk2_uuid],
                allocatable: true,
            },
            puddle::metadata::pool_config::ZoneMeta {
                index: 1,
                start_bytes: 2_000_000_000_000,
                size_bytes: 2_000_000_000_000,
                raid_level: RaidLevel::Raid1,
                md_device: "/dev/md/puddle-z1".to_string(),
                participating_disk_uuids: vec![disk1_uuid, disk2_uuid],
                allocatable: true,
            },
        ],
        lvm: puddle::metadata::pool_config::LvmMeta {
            vg_name: "puddle-pool".to_string(),
            lv_name: "data".to_string(),
            filesystem: "ext4".to_string(),
            mount_point: "/mnt/pool".to_string(),
        },
        state: puddle::metadata::pool_config::StateMeta {
            pool_status: PoolStatus::Healthy,
            last_scrub: Some("2026-03-08T03:00:00Z".to_string()),
            version: 2,
        },
    }
}

#[test]
fn test_serialize_deserialize_roundtrip() {
    let config = sample_config();
    let toml_str = config.to_toml().expect("serialize failed");
    let restored = PoolConfig::from_toml(&toml_str).expect("deserialize failed");

    assert_eq!(config.pool.uuid, restored.pool.uuid);
    assert_eq!(config.pool.name, restored.pool.name);
    assert_eq!(config.pool.redundancy, restored.pool.redundancy);
    assert_eq!(config.disks.len(), restored.disks.len());
    assert_eq!(config.zones.len(), restored.zones.len());
    assert_eq!(config.lvm.vg_name, restored.lvm.vg_name);
    assert_eq!(config.state.pool_status, restored.state.pool_status);
}

#[test]
fn test_serialize_contains_expected_keys() {
    let config = sample_config();
    let toml_str = config.to_toml().expect("serialize failed");

    assert!(toml_str.contains("[pool]"));
    assert!(toml_str.contains("[[disks]]"));
    assert!(toml_str.contains("[[zones]]"));
    assert!(toml_str.contains("[lvm]"));
    assert!(toml_str.contains("[state]"));
    assert!(toml_str.contains("mypool"));
    assert!(toml_str.contains("raid5"));
    assert!(toml_str.contains("raid1"));
}

#[test]
fn test_deserialize_disk_status() {
    let config = sample_config();
    let toml_str = config.to_toml().unwrap();
    let restored = PoolConfig::from_toml(&toml_str).unwrap();

    for disk in &restored.disks {
        assert_eq!(disk.status, DiskStatus::Active);
    }
}

#[test]
fn test_deserialize_invalid_toml_returns_error() {
    let result = PoolConfig::from_toml("this is not valid toml {{{{");
    assert!(result.is_err());
}

#[test]
fn test_deserialize_missing_field_returns_error() {
    let incomplete = r#"
[pool]
uuid = "a1b2c3d4-e5f6-0000-0000-000000000000"
name = "mypool"
"#;
    let result = PoolConfig::from_toml(incomplete);
    assert!(result.is_err());
}

// ── Step 28: allocatable フィールド ──

#[test]
fn test_allocatable_roundtrip() {
    let mut config = sample_config();
    config.zones[1].allocatable = false;

    let toml_str = config.to_toml().unwrap();
    assert!(toml_str.contains("allocatable = false"));

    let restored = PoolConfig::from_toml(&toml_str).unwrap();
    assert!(restored.zones[0].allocatable);
    assert!(!restored.zones[1].allocatable);
}

#[test]
fn test_allocatable_backward_compat() {
    // 既存 pool.toml に allocatable フィールドがない場合、true として読み込まれる
    let toml_without_allocatable = r#"
[pool]
uuid = "a1b2c3d4-e5f6-0000-0000-000000000000"
name = "mypool"
created_at = "2026-03-10T12:00:00Z"
redundancy = "single"

[[disks]]
uuid = "00000000-0000-0000-0000-000000000001"
device_id = "ata-Samsung_870"
capacity_bytes = 2000000000000
seq = 0
status = "active"

[[zones]]
index = 0
start_bytes = 0
size_bytes = 2000000000000
raid_level = "single"
md_device = "/dev/md/puddle-z0"
participating_disk_uuids = ["00000000-0000-0000-0000-000000000001"]

[lvm]
vg_name = "puddle-pool"
lv_name = "data"
filesystem = "ext4"
mount_point = "/mnt/pool"

[state]
pool_status = "healthy"
version = 2
"#;
    let config = PoolConfig::from_toml(toml_without_allocatable).unwrap();
    assert!(config.zones[0].allocatable);
}

#[test]
fn test_is_redundant() {
    assert!(!puddle::types::RaidLevel::Single.is_redundant());
    assert!(puddle::types::RaidLevel::Raid1.is_redundant());
    assert!(puddle::types::RaidLevel::Raid5.is_redundant());
    assert!(puddle::types::RaidLevel::Raid6.is_redundant());
}
