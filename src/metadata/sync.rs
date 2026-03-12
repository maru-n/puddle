use anyhow::{Context, Result};

use super::pool_config::PoolConfig;
use crate::executor::command_runner::CommandRunner;
use crate::executor::filesystem::FilesystemManager;

const METADATA_FILENAME: &str = "puddle.toml";

/// メタデータパーティションのパスを生成する (partition 1)
fn metadata_partition_path(device: &str) -> String {
    if device.contains("nvme") || device.contains("loop") {
        format!("{}p1", device)
    } else {
        format!("{}1", device)
    }
}

/// メタデータの読み書きを担当する
///
/// 各ディスクのメタデータパーティション (partition 1, ext4) を
/// 一時ディレクトリにマウントし、puddle.toml を読み書きする。
/// 全ディスクに同一内容をレプリケーションする。
pub struct MetadataSync<'a, R: CommandRunner> {
    runner: &'a R,
}

impl<'a, R: CommandRunner> MetadataSync<'a, R> {
    pub fn new(runner: &'a R) -> Self {
        Self { runner }
    }

    /// 全ディスクのメタデータパーティションに PoolConfig を書き込む
    ///
    /// 加えてローカルキャッシュ (/var/lib/puddle/pool.toml) にも保存する。
    pub fn write_metadata(&self, config: &PoolConfig, disk_devices: &[&str]) -> Result<()> {
        self.write_metadata_with_local(config, disk_devices, "/var/lib/puddle")
    }

    /// 全ディスクのメタデータパーティション + 指定ディレクトリにローカルコピーを書き込む
    pub fn write_metadata_with_local(
        &self,
        config: &PoolConfig,
        disk_devices: &[&str],
        local_dir: &str,
    ) -> Result<()> {
        let toml_str = config.to_toml()?;

        // 各ディスクのメタデータパーティションに書き込む
        for device in disk_devices {
            self.write_to_disk(&toml_str, device)?;
        }

        // ローカルコピーを保存
        std::fs::create_dir_all(local_dir).ok();
        let local_path = format!("{}/pool.toml", local_dir);
        std::fs::write(local_path, &toml_str).context("Failed to write local metadata copy")?;

        Ok(())
    }

    /// 1台のディスクのメタデータパーティションに書き込む
    fn write_to_disk(&self, toml_str: &str, device: &str) -> Result<()> {
        let meta_part = metadata_partition_path(device);
        let fm = FilesystemManager::new(self.runner);

        // 一時マウントポイントを作成
        let tmp_dir =
            tempfile::tempdir().context("Failed to create temp dir for metadata mount")?;
        let mount_point = tmp_dir.path().to_str().unwrap();

        // マウント → 書き込み → アンマウント
        fm.mount(&meta_part, mount_point)?;

        let write_result = std::fs::write(tmp_dir.path().join(METADATA_FILENAME), toml_str);

        // アンマウントは書き込み成功/失敗に関わらず実行
        let umount_result = fm.umount(mount_point);

        // 書き込みエラーを先に返す
        write_result.context(format!("Failed to write metadata to {} partition", device))?;

        // アンマウントエラーを返す
        umount_result?;

        Ok(())
    }

    /// 1台のディスクのメタデータパーティションから PoolConfig を読み出す
    pub fn read_metadata(&self, device: &str) -> Result<PoolConfig> {
        let meta_part = metadata_partition_path(device);
        let fm = FilesystemManager::new(self.runner);

        let tmp_dir =
            tempfile::tempdir().context("Failed to create temp dir for metadata mount")?;
        let mount_point = tmp_dir.path().to_str().unwrap();

        fm.mount(&meta_part, mount_point)?;

        let toml_path = tmp_dir.path().join(METADATA_FILENAME);
        let read_result = std::fs::read_to_string(toml_path);

        // アンマウントは読み取り成功/失敗に関わらず実行
        let umount_result = fm.umount(mount_point);

        let toml_str =
            read_result.context(format!("Failed to read metadata from {} partition", device))?;

        umount_result?;

        PoolConfig::from_toml(&toml_str)
    }

    /// ローカルキャッシュから PoolConfig を読み出す
    pub fn read_metadata_local(&self, local_dir: &str) -> Result<PoolConfig> {
        let local_path = format!("{}/pool.toml", local_dir);
        let toml_str = std::fs::read_to_string(&local_path)
            .context(format!("Failed to read local metadata from {}", local_path))?;
        PoolConfig::from_toml(&toml_str)
    }
}
