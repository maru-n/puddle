use puddle::executor::rollback::OperationLog;

#[test]
fn test_new_operation_log() {
    let log = OperationLog::new("add_disk /dev/sdc");
    assert_eq!(log.operation(), "add_disk /dev/sdc");
    assert!(log.steps().is_empty());
    assert!(!log.is_committed());
}

#[test]
fn test_log_step() {
    let mut log = OperationLog::new("add_disk /dev/sdc");

    log.log_step(
        "Create partitions on /dev/sdc",
        "sgdisk -n 1:0:+16M -t 1:8300 /dev/sdc",
        "sgdisk --zap-all /dev/sdc",
    );

    assert_eq!(log.steps().len(), 1);
    assert_eq!(log.steps()[0].description, "Create partitions on /dev/sdc");
    assert_eq!(
        log.steps()[0].command,
        "sgdisk -n 1:0:+16M -t 1:8300 /dev/sdc"
    );
    assert_eq!(log.steps()[0].rollback_command, "sgdisk --zap-all /dev/sdc");
}

#[test]
fn test_log_multiple_steps() {
    let mut log = OperationLog::new("add_disk /dev/sdc");

    log.log_step(
        "Partition disk",
        "sgdisk /dev/sdc",
        "sgdisk --zap-all /dev/sdc",
    );
    log.log_step(
        "Add to RAID",
        "mdadm --add /dev/md/puddle-z0 /dev/sdc2",
        "mdadm --fail --remove /dev/md/puddle-z0 /dev/sdc2",
    );
    log.log_step(
        "Create PV",
        "pvcreate /dev/md/puddle-z1",
        "pvremove /dev/md/puddle-z1",
    );

    assert_eq!(log.steps().len(), 3);
    // ステップは追加順
    assert_eq!(log.steps()[0].description, "Partition disk");
    assert_eq!(log.steps()[2].description, "Create PV");
}

#[test]
fn test_commit() {
    let mut log = OperationLog::new("add_disk /dev/sdc");
    log.log_step("Step 1", "cmd1", "rollback1");
    log.commit();
    assert!(log.is_committed());
}

#[test]
fn test_rollback_commands_in_reverse_order() {
    let mut log = OperationLog::new("add_disk /dev/sdc");
    log.log_step("Step A", "cmdA", "rollbackA");
    log.log_step("Step B", "cmdB", "rollbackB");
    log.log_step("Step C", "cmdC", "rollbackC");

    let rollbacks = log.rollback_commands();
    assert_eq!(rollbacks, vec!["rollbackC", "rollbackB", "rollbackA"]);
}

#[test]
fn test_format_log() {
    let mut log = OperationLog::new("add_disk /dev/sdc");
    log.log_step(
        "Partition disk",
        "sgdisk /dev/sdc",
        "sgdisk --zap-all /dev/sdc",
    );
    log.log_step("Add to RAID", "mdadm --add", "mdadm --fail --remove");
    log.commit();

    let formatted = log.format();

    assert!(formatted.contains("BEGIN add_disk /dev/sdc"));
    assert!(formatted.contains("STEP 1: sgdisk /dev/sdc"));
    assert!(formatted.contains("ROLLBACK: sgdisk --zap-all /dev/sdc"));
    assert!(formatted.contains("STEP 2: mdadm --add"));
    assert!(formatted.contains("COMMIT add_disk /dev/sdc"));
}

#[test]
fn test_format_log_without_commit() {
    let mut log = OperationLog::new("add_disk /dev/sdc");
    log.log_step("Step 1", "cmd1", "rollback1");

    let formatted = log.format();

    assert!(formatted.contains("BEGIN add_disk /dev/sdc"));
    assert!(formatted.contains("STEP 1: cmd1"));
    assert!(
        !formatted.contains("COMMIT"),
        "uncommitted log should not have COMMIT"
    );
}

#[test]
fn test_save_to_file() {
    let mut log = OperationLog::new("add_disk /dev/sdc");
    log.log_step(
        "Partition disk",
        "sgdisk /dev/sdc",
        "sgdisk --zap-all /dev/sdc",
    );
    log.commit();

    let tmp_dir = std::env::temp_dir().join("puddle-test-rollback");
    std::fs::create_dir_all(&tmp_dir).ok();
    let log_path = tmp_dir.join("operations.log");

    let result = log.save_to_file(log_path.to_str().unwrap());
    assert!(result.is_ok(), "save_to_file failed: {:?}", result);

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("BEGIN add_disk /dev/sdc"));
    assert!(content.contains("COMMIT add_disk /dev/sdc"));

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_save_appends_to_existing_file() {
    let tmp_dir = std::env::temp_dir().join("puddle-test-rollback-append");
    std::fs::create_dir_all(&tmp_dir).ok();
    let log_path = tmp_dir.join("operations.log");

    // 1回目
    let mut log1 = OperationLog::new("init /dev/sdb");
    log1.log_step("Wipe", "sgdisk --zap-all", "");
    log1.commit();
    log1.save_to_file(log_path.to_str().unwrap()).unwrap();

    // 2回目
    let mut log2 = OperationLog::new("add_disk /dev/sdc");
    log2.log_step("Partition", "sgdisk /dev/sdc", "sgdisk --zap-all /dev/sdc");
    log2.commit();
    log2.save_to_file(log_path.to_str().unwrap()).unwrap();

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("BEGIN init /dev/sdb"));
    assert!(content.contains("BEGIN add_disk /dev/sdc"));

    std::fs::remove_dir_all(&tmp_dir).ok();
}
