use puddle::executor::command_runner::{CommandRunner, MockCommandRunner, RealCommandRunner};
use puddle::executor::filesystem::FilesystemManager;
use puddle::executor::lvm::VolumeManager;
use puddle::executor::mdadm::RaidManager;
use puddle::executor::partition::PartitionManager;
use puddle::types::*;

#[test]
fn test_mock_runner_records_commands() {
    let mock = MockCommandRunner::new();

    mock.run("sgdisk", &["--zap-all", "/dev/sdb"]).unwrap();
    mock.run("mdadm", &["--create", "/dev/md/puddle-z0"])
        .unwrap();

    let history = mock.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].0, "sgdisk");
    assert_eq!(history[0].1, vec!["--zap-all", "/dev/sdb"]);
    assert_eq!(history[1].0, "mdadm");
}

#[test]
fn test_mock_runner_can_simulate_failure() {
    let mock = MockCommandRunner::new();
    mock.set_fail("sgdisk", "simulated sgdisk failure");

    let result = mock.run("sgdisk", &["--zap-all", "/dev/sdb"]);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("simulated sgdisk failure"));
}

#[test]
fn test_mock_runner_can_set_stdout() {
    let mock = MockCommandRunner::new();
    mock.set_stdout("lsblk", "sdb  2000000000000\n");

    let output = mock.run("lsblk", &["--bytes", "/dev/sdb"]).unwrap();
    assert_eq!(output, "sdb  2000000000000\n");
}

#[test]
fn test_real_runner_runs_echo() {
    let runner = RealCommandRunner;
    let output = runner.run("echo", &["hello"]).unwrap();
    assert_eq!(output.trim(), "hello");
}

// ── partition tests ──

#[test]
fn test_partition_wipe() {
    let mock = MockCommandRunner::new();
    let pm = PartitionManager::new(&mock);

    pm.wipe("/dev/sdb").unwrap();

    let h = mock.history();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].0, "sgdisk");
    assert!(h[0].1.contains(&"--zap-all".to_string()));
    assert!(h[0].1.contains(&"/dev/sdb".to_string()));
}

#[test]
fn test_partition_create_metadata() {
    let mock = MockCommandRunner::new();
    let pm = PartitionManager::new(&mock);

    pm.create_metadata_partition("/dev/sdb").unwrap();

    let h = mock.history();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].0, "sgdisk");
}

#[test]
fn test_partition_create_zones() {
    let mock = MockCommandRunner::new();
    let pm = PartitionManager::new(&mock);

    let zones = vec![
        ZoneSpec {
            index: 0,
            start_bytes: 0,
            size_bytes: 2_000_000_000_000,
            raid_level: RaidLevel::Raid5,
            num_disks: 3,
            effective_bytes: 4_000_000_000_000,
        },
        ZoneSpec {
            index: 1,
            start_bytes: 2_000_000_000_000,
            size_bytes: 2_000_000_000_000,
            raid_level: RaidLevel::Raid1,
            num_disks: 2,
            effective_bytes: 2_000_000_000_000,
        },
    ];

    pm.create_zone_partitions("/dev/sdb", &zones).unwrap();

    let h = mock.history();
    assert_eq!(h.len(), 2); // 2 zones → 2 sgdisk calls
}

// ── mdadm tests ──

#[test]
fn test_mdadm_create_raid5() {
    let mock = MockCommandRunner::new();
    let rm = RaidManager::new(&mock);

    rm.create_array(
        "/dev/md/puddle-z0",
        RaidLevel::Raid5,
        &["/dev/sdb2", "/dev/sdc2", "/dev/sdd2"],
    )
    .unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "mdadm");
    assert!(h[0].1.contains(&"--create".to_string()));
    assert!(h[0].1.contains(&"5".to_string())); // level
    assert!(h[0].1.contains(&"3".to_string())); // raid-devices
    assert!(!h[0].1.contains(&"--force".to_string())); // no force for 3 devices
}

#[test]
fn test_mdadm_create_single_requires_force() {
    let mock = MockCommandRunner::new();
    let rm = RaidManager::new(&mock);

    rm.create_array("/dev/md/puddle-z0", RaidLevel::Single, &["/dev/sdb2"])
        .unwrap();

    let h = mock.history();
    assert!(h[0].1.contains(&"--force".to_string()));
}

#[test]
fn test_mdadm_grow_level() {
    let mock = MockCommandRunner::new();
    let rm = RaidManager::new(&mock);

    rm.grow_level("/dev/md/puddle-z0", RaidLevel::Raid5, 3)
        .unwrap();

    let h = mock.history();
    assert!(h[0].1.contains(&"--grow".to_string()));
    assert!(h[0].1.contains(&"--level".to_string()));
    assert!(h[0].1.contains(&"5".to_string()));
}

#[test]
fn test_mdadm_fail_device() {
    let mock = MockCommandRunner::new();
    let rm = RaidManager::new(&mock);

    rm.fail_device("/dev/md/puddle-z0", "/dev/sdb2").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "mdadm");
    assert!(h[0].1.contains(&"--fail".to_string()));
    assert!(h[0].1.contains(&"/dev/md/puddle-z0".to_string()));
    assert!(h[0].1.contains(&"/dev/sdb2".to_string()));
}

#[test]
fn test_mdadm_remove_device() {
    let mock = MockCommandRunner::new();
    let rm = RaidManager::new(&mock);

    rm.remove_device("/dev/md/puddle-z0", "/dev/sdb2").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "mdadm");
    assert!(h[0].1.contains(&"--remove".to_string()));
    assert!(h[0].1.contains(&"/dev/sdb2".to_string()));
}

// ── lvm tests ──

#[test]
fn test_lvm_full_flow() {
    let mock = MockCommandRunner::new();
    let vm = VolumeManager::new(&mock);

    vm.pvcreate("/dev/md/puddle-z0").unwrap();
    vm.vgcreate("puddle-pool", &["/dev/md/puddle-z0"]).unwrap();
    vm.lvcreate_full("puddle-pool", "data").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "pvcreate");
    assert_eq!(h[1].0, "vgcreate");
    assert_eq!(h[2].0, "lvcreate");
}

#[test]
fn test_lvm_extend_flow() {
    let mock = MockCommandRunner::new();
    let vm = VolumeManager::new(&mock);

    vm.pvcreate("/dev/md/puddle-z1").unwrap();
    vm.vgextend("puddle-pool", "/dev/md/puddle-z1").unwrap();
    vm.lvextend_full("/dev/mapper/puddle--pool-data").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "pvcreate");
    assert_eq!(h[1].0, "vgextend");
    assert_eq!(h[2].0, "lvextend");
}

// ── filesystem tests ──

#[test]
fn test_fs_mkfs_ext4() {
    let mock = MockCommandRunner::new();
    let fm = FilesystemManager::new(&mock);

    fm.mkfs("/dev/mapper/puddle--pool-data", "ext4").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "mkfs.ext4");
}

#[test]
fn test_fs_mkfs_unsupported() {
    let mock = MockCommandRunner::new();
    let fm = FilesystemManager::new(&mock);

    let result = fm.mkfs("/dev/mapper/puddle--pool-data", "ntfs");
    assert!(result.is_err());
}

#[test]
fn test_fs_resize_ext4() {
    let mock = MockCommandRunner::new();
    let fm = FilesystemManager::new(&mock);

    fm.resize("/dev/mapper/puddle--pool-data", "ext4").unwrap();

    let h = mock.history();
    assert_eq!(h[0].0, "resize2fs");
}
