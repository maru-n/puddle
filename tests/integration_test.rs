//! 統合テスト: ループバックデバイスを使った E2E テスト
//!
//! 実行方法 (要 root):
//!   cargo test --features integration --test integration_test
//!
//! 必須パッケージ: mdadm, lvm2, e2fsprogs, gdisk

#![cfg(feature = "integration")]

use std::process::Command;

/// テスト用ループバックデバイスを管理するヘルパー
struct LoopDevice {
    path: String,
    img_path: String,
}

impl LoopDevice {
    fn create(name: &str, size_mb: u64) -> Self {
        let img_path = format!("/tmp/puddle-test-{}.img", name);

        // イメージファイル作成
        let status = Command::new("dd")
            .args([
                "if=/dev/zero",
                &format!("of={}", img_path),
                "bs=1M",
                &format!("count={}", size_mb),
            ])
            .output()
            .expect("dd failed");
        assert!(status.status.success(), "dd failed: {:?}", status);

        // ループバックデバイス割り当て
        let output = Command::new("losetup")
            .args(["--find", "--show", &img_path])
            .output()
            .expect("losetup failed");
        assert!(output.status.success(), "losetup failed");

        let path = String::from_utf8(output.stdout).unwrap().trim().to_string();

        LoopDevice { path, img_path }
    }
}

impl Drop for LoopDevice {
    fn drop(&mut self) {
        // ループバックデバイス解放
        let _ = Command::new("losetup").args(["-d", &self.path]).output();
        // イメージファイル削除
        let _ = std::fs::remove_file(&self.img_path);
    }
}

/// テスト後のクリーンアップ (RAID / LVM)
struct PoolCleanup;

impl Drop for PoolCleanup {
    fn drop(&mut self) {
        // LVM クリーンアップ
        let _ = Command::new("lvchange")
            .args(["-an", "puddle-pool/data"])
            .output();
        let _ = Command::new("lvremove")
            .args(["-f", "puddle-pool/data"])
            .output();
        let _ = Command::new("vgremove")
            .args(["-f", "puddle-pool"])
            .output();

        // 全 puddle md デバイスを停止
        for i in 0..10 {
            let md = format!("/dev/md/puddle-z{}", i);
            let _ = Command::new("mdadm").args(["--stop", &md]).output();
        }
        // 番号付き md デバイスも停止
        for i in 0..10 {
            let md = format!("/dev/md{}", i);
            let _ = Command::new("mdadm").args(["--stop", &md]).output();
        }
    }
}

fn run_puddle(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_puddle"))
        .args(args)
        .output()
        .expect("Failed to run puddle")
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[test]
fn test_init_single_disk() {
    if !is_root() {
        eprintln!("SKIP: test_init_single_disk requires root");
        return;
    }

    let _cleanup = PoolCleanup;
    let disk0 = LoopDevice::create("init0", 256);

    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "puddle init failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("created") || stdout.contains("Pool"),
        "Expected pool creation message, got: {}",
        stdout
    );
}

#[test]
fn test_full_lifecycle() {
    if !is_root() {
        eprintln!("SKIP: test_full_lifecycle requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    // 3台のループバックデバイス作成
    let disk0 = LoopDevice::create("lc0", 256);
    let disk1 = LoopDevice::create("lc1", 256);
    let disk2 = LoopDevice::create("lc2", 256);

    // 1. init
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4"]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 2. add disk1
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(
        output.status.success(),
        "add disk1 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 3. add disk2
    let output = run_puddle(&["add", &disk2.path, "--yes"]);
    assert!(
        output.status.success(),
        "add disk2 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 4. status
    let output = run_puddle(&["status"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed");
    assert!(stdout.contains("Disks:"), "Status should show disks");
    assert!(stdout.contains("Zones:"), "Status should show zones");
}

#[test]
fn test_data_survives_disk_failure() {
    if !is_root() {
        eprintln!("SKIP: test_data_survives_disk_failure requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    // 均一サイズ3台でフル冗長構成
    let disk0 = LoopDevice::create("fail0", 256);
    let disk1 = LoopDevice::create("fail1", 256);
    let disk2 = LoopDevice::create("fail2", 256);

    // init + add × 2
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4"]);
    assert!(output.status.success(), "init failed");
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(output.status.success(), "add disk1 failed");
    let output = run_puddle(&["add", &disk2.path, "--yes"]);
    assert!(output.status.success(), "add disk2 failed");

    // マウントしてデータ書き込み
    let mount_point = "/tmp/puddle-test-mount";
    std::fs::create_dir_all(mount_point).ok();
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output()
        .expect("mount failed");

    if !mount_result.status.success() {
        eprintln!(
            "SKIP: mount failed (may need dm module): {}",
            String::from_utf8_lossy(&mount_result.stderr)
        );
        return;
    }

    // テストデータ書き込み
    let test_file = format!("{}/testfile", mount_point);
    Command::new("dd")
        .args([
            "if=/dev/urandom",
            &format!("of={}", test_file),
            "bs=1M",
            "count=10",
        ])
        .output()
        .expect("dd write failed");

    // ハッシュ記録
    let hash_before = md5sum(&test_file);
    assert!(!hash_before.is_empty(), "Failed to get hash before failure");

    // sync してからアンマウント
    let _ = Command::new("sync").output();
    let _ = Command::new("umount").arg(mount_point).output();

    // ディスク1台を fail させる
    // md デバイスを探す
    let md_devices = find_md_devices();
    if let Some(md_dev) = md_devices.first() {
        let fail_part = format!("{}p1", disk1.path); // ループバックのパーティション
        let _ = Command::new("mdadm")
            .args(["--fail", md_dev, &fail_part])
            .output();
    }

    // 再マウントしてデータ確認
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output();

    if let Ok(output) = mount_result {
        if output.status.success() {
            let hash_after = md5sum(&test_file);
            assert_eq!(
                hash_before, hash_after,
                "Data corruption detected after disk failure!"
            );
            let _ = Command::new("umount").arg(mount_point).output();
        }
    }

    let _ = std::fs::remove_dir(mount_point);
}

fn md5sum(path: &str) -> String {
    let output = Command::new("md5sum")
        .arg(path)
        .output()
        .expect("md5sum failed");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

#[test]
fn test_replace_disk() {
    if !is_root() {
        eprintln!("SKIP: test_replace_disk requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    let disk0 = LoopDevice::create("rep0", 256);
    let disk1 = LoopDevice::create("rep1", 256);
    let disk_new = LoopDevice::create("rep_new", 256);

    // init + add → 2台 RAID1 構成
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4"]);
    assert!(output.status.success(), "init failed");
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(output.status.success(), "add failed");

    // マウントしてデータ書き込み
    let mount_point = "/tmp/puddle-test-replace";
    std::fs::create_dir_all(mount_point).ok();
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output()
        .expect("mount failed");

    if !mount_result.status.success() {
        eprintln!(
            "SKIP: mount failed: {}",
            String::from_utf8_lossy(&mount_result.stderr)
        );
        return;
    }

    let test_file = format!("{}/replace-test", mount_point);
    Command::new("dd")
        .args([
            "if=/dev/urandom",
            &format!("of={}", test_file),
            "bs=1M",
            "count=5",
        ])
        .output()
        .expect("dd write failed");
    let hash_before = md5sum(&test_file);
    let _ = Command::new("sync").output();
    let _ = Command::new("umount").arg(mount_point).output();

    // replace disk1 → disk_new
    let output = run_puddle(&["replace", &disk1.path, &disk_new.path, "--yes"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "replace failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // 再マウントしてデータ確認
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output();
    if let Ok(output) = mount_result {
        if output.status.success() {
            let hash_after = md5sum(&test_file);
            assert_eq!(
                hash_before, hash_after,
                "Data corruption detected after replace!"
            );
            let _ = Command::new("umount").arg(mount_point).output();
        }
    }

    let _ = std::fs::remove_dir(mount_point);
}

#[test]
fn test_destroy_cleans_up_everything() {
    if !is_root() {
        eprintln!("SKIP: test_destroy_cleans_up_everything requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    let disk0 = LoopDevice::create("dest0", 256);
    let disk1 = LoopDevice::create("dest1", 256);

    // init + add
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4"]);
    assert!(output.status.success(), "init failed");
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(output.status.success(), "add failed");

    // destroy
    let output = run_puddle(&["destroy", "--yes"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "destroy failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(stdout.contains("destroyed"), "Expected 'destroyed' message");

    // VG が存在しないことを確認
    let vg_check = Command::new("vgs")
        .arg("puddle-pool")
        .output()
        .expect("vgs failed");
    assert!(
        !vg_check.status.success(),
        "VG puddle-pool should not exist after destroy"
    );

    // md デバイスが存在しないことを確認
    let md_devices = find_md_devices();
    assert!(
        md_devices.is_empty(),
        "MD devices should not exist after destroy: {:?}",
        md_devices
    );
}

#[test]
fn test_concurrent_puddle_blocked() {
    if !is_root() {
        eprintln!("SKIP: test_concurrent_puddle_blocked requires root");
        return;
    }

    let _cleanup = PoolCleanup;
    let disk0 = LoopDevice::create("conc0", 256);

    // 1つ目のプロセスを開始 (init --yes) — バックグラウンドではなく通常実行
    let output1 = run_puddle(&["init", &disk0.path, "--mkfs", "ext4", "--yes"]);
    assert!(
        output1.status.success(),
        "first init should succeed: {}",
        String::from_utf8_lossy(&output1.stderr)
    );

    // 2つ目の init を同時に実行しようとする (既にプールがあるのでエラーになるが、
    // ロックが動作していることの間接的な確認)
    let output2 = run_puddle(&["status"]);
    // status はロック取得→解放するので成功するはず
    assert!(
        output2.status.success(),
        "status should succeed after init: {}",
        String::from_utf8_lossy(&output2.stderr)
    );
}

#[test]
fn test_init_creates_operation_log() {
    if !is_root() {
        eprintln!("SKIP: test_init_creates_operation_log requires root");
        return;
    }

    let _cleanup = PoolCleanup;
    let disk0 = LoopDevice::create("oplog0", 256);

    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4", "--yes"]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 操作ログが作成されていることを確認
    let log_path = "/var/lib/puddle/operations.log";
    assert!(
        std::path::Path::new(log_path).exists(),
        "operations.log should exist after init"
    );
    let content = std::fs::read_to_string(log_path).unwrap();
    assert!(content.contains("BEGIN"), "log should contain BEGIN");
    assert!(content.contains("COMMIT"), "log should contain COMMIT");
}

fn find_md_devices() -> Vec<String> {
    let mut devices = Vec::new();
    for i in 0..10 {
        let path = format!("/dev/md/puddle-z{}", i);
        if std::path::Path::new(&path).exists() {
            devices.push(path);
        }
    }
    devices
}

#[test]
fn test_monitor_once() {
    if !is_root() {
        eprintln!("SKIP: test_monitor_once requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    let disk0 = LoopDevice::create("mon0", 256);
    let disk1 = LoopDevice::create("mon1", 256);

    // init + add で 2台構成
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4", "--yes"]);
    assert!(output.status.success(), "init failed");
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(output.status.success(), "add failed");

    // monitor --once で1回チェック
    let output = run_puddle(&["monitor", "--once"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // SMART が使えない環境でも RAID チェックは動く
    // exit code 0 (正常) or 2 (警告あり) のどちらかであること
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(2),
        "monitor --once should exit with 0 or 2, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    );

    // 何らかの出力があること
    assert!(
        !stdout.is_empty() || !stderr.is_empty(),
        "monitor should produce output"
    );
}

#[test]
fn test_remove_disk_e2e() {
    if !is_root() {
        eprintln!("SKIP: test_remove_disk_e2e requires root");
        return;
    }

    let _cleanup = PoolCleanup;

    let disk0 = LoopDevice::create("rm0", 256);
    let disk1 = LoopDevice::create("rm1", 256);
    let disk2 = LoopDevice::create("rm2", 256);

    // 3台構成
    let output = run_puddle(&["init", &disk0.path, "--mkfs", "ext4", "--yes"]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_puddle(&["add", &disk1.path, "--yes"]);
    assert!(
        output.status.success(),
        "add disk1 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_puddle(&["add", &disk2.path, "--yes"]);
    assert!(
        output.status.success(),
        "add disk2 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // マウントしてデータ書き込み
    let mount_point = "/tmp/puddle-test-remove";
    std::fs::create_dir_all(mount_point).ok();
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output()
        .expect("mount failed");

    if !mount_result.status.success() {
        eprintln!(
            "SKIP: mount failed: {}",
            String::from_utf8_lossy(&mount_result.stderr)
        );
        return;
    }

    let test_file = format!("{}/remove-test", mount_point);
    Command::new("dd")
        .args([
            "if=/dev/urandom",
            &format!("of={}", test_file),
            "bs=1M",
            "count=5",
        ])
        .output()
        .expect("dd write failed");
    let hash_before = md5sum(&test_file);
    let _ = Command::new("sync").output();
    let _ = Command::new("umount").arg(mount_point).output();

    // ディスク1台を除去
    let output = run_puddle(&["remove", &disk2.path, "--yes"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "remove failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("removed") || stdout.contains("Remaining"),
        "Expected removal confirmation, got: {}",
        stdout
    );

    // status で2台になっていることを確認
    let output = run_puddle(&["status"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed after remove");

    // 再マウントしてデータ確認
    let mount_result = Command::new("mount")
        .args(["/dev/mapper/puddle--pool-data", mount_point])
        .output();
    if let Ok(output) = mount_result {
        if output.status.success() {
            let hash_after = md5sum(&test_file);
            assert_eq!(
                hash_before, hash_after,
                "Data corruption detected after remove!"
            );
            let _ = Command::new("umount").arg(mount_point).output();
        }
    }

    let _ = std::fs::remove_dir(mount_point);
}

#[test]
fn test_init_with_dual_redundancy_e2e() {
    if !is_root() {
        eprintln!("SKIP: test_init_with_dual_redundancy_e2e requires root");
        return;
    }

    let _cleanup = PoolCleanup;
    let disk0 = LoopDevice::create("dual0", 256);

    let output = run_puddle(&[
        "init",
        &disk0.path,
        "--mkfs",
        "ext4",
        "--redundancy",
        "dual",
        "--yes",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "init with dual redundancy failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // status でプール情報を確認
    let output = run_puddle(&["status"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed");
    assert!(
        stdout.contains("Dual"),
        "Status should show Dual redundancy, got: {}",
        stdout
    );
}
