use anyhow::{bail, Result};

use super::command_runner::CommandRunner;

/// ファイルシステム操作
pub struct FilesystemManager<'a, R: CommandRunner> {
    runner: &'a R,
}

impl<'a, R: CommandRunner> FilesystemManager<'a, R> {
    pub fn new(runner: &'a R) -> Self {
        Self { runner }
    }

    /// ファイルシステムを作成
    pub fn mkfs(&self, device: &str, fs_type: &str) -> Result<()> {
        match fs_type {
            "ext4" => {
                self.runner.run("mkfs.ext4", &["-F", device])?;
            }
            "xfs" => {
                self.runner.run("mkfs.xfs", &["-f", device])?;
            }
            _ => bail!("Unsupported filesystem type: {}", fs_type),
        }
        Ok(())
    }

    /// ファイルシステムをリサイズ
    pub fn resize(&self, device: &str, fs_type: &str) -> Result<()> {
        match fs_type {
            "ext4" => {
                self.runner.run("resize2fs", &[device])?;
            }
            "xfs" => {
                // xfs_growfs はマウントポイントが必要
                self.runner.run("xfs_growfs", &[device])?;
            }
            _ => bail!("Unsupported filesystem type for resize: {}", fs_type),
        }
        Ok(())
    }

    /// マウント
    pub fn mount(&self, device: &str, mount_point: &str) -> Result<()> {
        self.runner.run("mount", &[device, mount_point])?;
        Ok(())
    }

    /// アンマウント
    pub fn umount(&self, mount_point: &str) -> Result<()> {
        self.runner.run("umount", &[mount_point])?;
        Ok(())
    }
}
