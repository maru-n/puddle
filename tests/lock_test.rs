use puddle::lock::PuddleLock;

#[test]
fn test_lock_acquire_and_release() {
    let tmp_dir = std::env::temp_dir().join("puddle-test-lock");
    std::fs::create_dir_all(&tmp_dir).ok();
    let lock_path = tmp_dir.join("puddle.lock");

    {
        let lock = PuddleLock::acquire(lock_path.to_str().unwrap());
        assert!(lock.is_ok(), "should acquire lock: {:?}", lock.err());
    } // lock dropped here → released

    // ロック解放後に再取得できる
    let lock2 = PuddleLock::acquire(lock_path.to_str().unwrap());
    assert!(
        lock2.is_ok(),
        "should re-acquire lock after release: {:?}",
        lock2.err()
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_lock_contention() {
    let tmp_dir = std::env::temp_dir().join("puddle-test-lock-contention");
    std::fs::create_dir_all(&tmp_dir).ok();
    let lock_path = tmp_dir.join("puddle.lock");

    let _lock1 = PuddleLock::acquire(lock_path.to_str().unwrap()).unwrap();

    // 同じロックファイルに対する2つ目のロックは失敗すべき
    let lock2 = PuddleLock::try_acquire(lock_path.to_str().unwrap());
    assert!(
        lock2.is_err(),
        "second lock should fail while first is held"
    );
    let err_msg = lock2.unwrap_err().to_string();
    assert!(
        err_msg.contains("another puddle process"),
        "error should mention another process, got: {}",
        err_msg
    );

    std::fs::remove_dir_all(&tmp_dir).ok();
}
