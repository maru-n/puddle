use anyhow::Result;

use super::command_runner::CommandRunner;
use crate::types::ZoneSpec;

const METADATA_PARTITION_SIZE_MB: u64 = 16;

/// sgdisk パーティション操作
pub struct PartitionManager<'a, R: CommandRunner> {
    runner: &'a R,
}

impl<'a, R: CommandRunner> PartitionManager<'a, R> {
    pub fn new(runner: &'a R) -> Self {
        Self { runner }
    }

    /// GPT パーティションテーブルを初期化
    pub fn wipe(&self, device: &str) -> Result<()> {
        self.runner.run("sgdisk", &["--zap-all", device])?;
        Ok(())
    }

    /// メタデータパーティション (16MB, partition 1) を作成
    pub fn create_metadata_partition(&self, device: &str) -> Result<()> {
        let size_arg = format!("+{}M", METADATA_PARTITION_SIZE_MB);
        self.runner.run(
            "sgdisk",
            &["-n", &format!("1:0:{}", size_arg), "-t", "1:8300", device],
        )?;
        Ok(())
    }

    /// ゾーン用パーティション群を作成 (partition 2, 3, ...)
    pub fn create_zone_partitions(&self, device: &str, zones: &[ZoneSpec]) -> Result<()> {
        for (i, _zone) in zones.iter().enumerate() {
            let part_num = i + 2; // partition 1 はメタデータ

            // 最後のゾーンは残り全てを使う
            let size_spec = if i == zones.len() - 1 {
                format!("{}:0:0", part_num)
            } else {
                let size_mb = _zone.size_bytes / 1_000_000;
                format!("{}:0:+{}M", part_num, size_mb)
            };

            let type_arg = format!("{}:fd00", part_num);
            self.runner
                .run("sgdisk", &["-n", &size_spec, "-t", &type_arg, device])?;
        }
        Ok(())
    }

    /// カーネルにパーティションテーブルの再読み込みを通知
    ///
    /// 複数の方法を試し、全て失敗しても致命的エラーにはしない。
    /// sgdisk 自体がカーネル通知を行うため、ここでの失敗は通常問題にならない。
    pub fn reload_table(&self, device: &str) -> Result<()> {
        if self.runner.run("partprobe", &[device]).is_ok() {
            return Ok(());
        }
        if self.runner.run("partx", &["--update", device]).is_ok() {
            return Ok(());
        }
        if self.runner.run("blockdev", &["--rereadpt", device]).is_ok() {
            return Ok(());
        }
        // 全て失敗しても続行（sgdisk が既にカーネルに通知済みの場合が多い）
        eprintln!(
            "Warning: could not reload partition table for {}. Continuing anyway.",
            device
        );
        Ok(())
    }
}
