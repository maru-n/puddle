use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

/// プロセス間排他ロック (flock ベース)
///
/// Drop 時に自動解放される。
#[derive(Debug)]
pub struct PuddleLock {
    _file: File,
}

impl PuddleLock {
    /// ブロッキングでロックを取得する
    pub fn acquire(path: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .context(format!("Failed to open lock file: {}", path))?;

        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            bail!(
                "Failed to acquire lock: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(Self { _file: file })
    }

    /// ノンブロッキングでロック取得を試みる
    pub fn try_acquire(path: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .context(format!("Failed to open lock file: {}", path))?;

        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            bail!(
                "another puddle process is already running (lock file: {})",
                path
            );
        }

        Ok(Self { _file: file })
    }
}
