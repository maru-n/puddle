use puddle::monitor::mdstat::parse_mdstat;
use puddle::monitor::smart::parse_smart_json;

// ── smartctl JSON parsing ──

const SMART_JSON_OK: &str = r#"{
  "device": {"name": "/dev/sdb", "type": "sat"},
  "model_name": "Samsung SSD 870 EVO 2TB",
  "serial_number": "S1234",
  "smart_status": {"passed": true},
  "temperature": {"current": 34},
  "ata_smart_attributes": {
    "table": [
      {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 0}},
      {"id": 241, "name": "Total_LBAs_Written", "raw": {"value": 25000000000}}
    ]
  }
}"#;

#[test]
fn test_parse_smart_json_ok() {
    let info = parse_smart_json(SMART_JSON_OK).unwrap();
    assert_eq!(info.model, "Samsung SSD 870 EVO 2TB");
    assert!(info.passed);
    assert_eq!(info.temperature_celsius, Some(34));
    assert_eq!(info.reallocated_sectors, Some(0));
}

const SMART_JSON_DEGRADED: &str = r#"{
  "device": {"name": "/dev/sdc", "type": "sat"},
  "model_name": "WD Red 4TB",
  "serial_number": "WD5678",
  "smart_status": {"passed": false},
  "temperature": {"current": 42},
  "ata_smart_attributes": {
    "table": [
      {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 8}}
    ]
  }
}"#;

#[test]
fn test_parse_smart_json_degraded() {
    let info = parse_smart_json(SMART_JSON_DEGRADED).unwrap();
    assert!(!info.passed);
    assert_eq!(info.temperature_celsius, Some(42));
    assert_eq!(info.reallocated_sectors, Some(8));
}

#[test]
fn test_parse_smart_json_invalid() {
    let result = parse_smart_json("not json");
    assert!(result.is_err());
}

// ── /proc/mdstat parsing ──

const MDSTAT_CLEAN: &str = r#"Personalities : [raid1] [raid5] [raid6]
md0 : active raid5 sdd2[2] sdc2[1] sdb2[0]
      524032 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]

md1 : active raid1 sdd3[1] sdc3[0]
      262080 blocks super 1.2 [2/2] [UU]

unused devices: <none>
"#;

#[test]
fn test_parse_mdstat_clean() {
    let arrays = parse_mdstat(MDSTAT_CLEAN);
    assert_eq!(arrays.len(), 2);

    assert_eq!(arrays[0].name, "md0");
    assert_eq!(arrays[0].level, "raid5");
    assert_eq!(arrays[0].num_devices, 3);
    assert_eq!(arrays[0].active_devices, 3);
    assert!(arrays[0].is_clean());

    assert_eq!(arrays[1].name, "md1");
    assert_eq!(arrays[1].level, "raid1");
    assert_eq!(arrays[1].num_devices, 2);
    assert_eq!(arrays[1].active_devices, 2);
    assert!(arrays[1].is_clean());
}

const MDSTAT_DEGRADED: &str = r#"Personalities : [raid5]
md0 : active raid5 sdc2[1] sdb2[0]
      524032 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [UU_]

unused devices: <none>
"#;

#[test]
fn test_parse_mdstat_degraded() {
    let arrays = parse_mdstat(MDSTAT_DEGRADED);
    assert_eq!(arrays.len(), 1);
    assert_eq!(arrays[0].num_devices, 3);
    assert_eq!(arrays[0].active_devices, 2);
    assert!(!arrays[0].is_clean());
}

const MDSTAT_REBUILDING: &str = r#"Personalities : [raid5]
md0 : active raid5 sdd2[3] sdc2[1] sdb2[0]
      524032 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [UU_]
      [====>................]  recovery = 22.3% (58816/262016) finish=0.1min speed=26408K/sec

unused devices: <none>
"#;

#[test]
fn test_parse_mdstat_rebuilding() {
    let arrays = parse_mdstat(MDSTAT_REBUILDING);
    assert_eq!(arrays.len(), 1);
    assert!(!arrays[0].is_clean());
    assert!(arrays[0].recovery_percent.is_some());
}

const MDSTAT_EMPTY: &str = r#"Personalities :
unused devices: <none>
"#;

#[test]
fn test_parse_mdstat_empty() {
    let arrays = parse_mdstat(MDSTAT_EMPTY);
    assert!(arrays.is_empty());
}
