use anyhow::Result;

use super::command_runner::CommandRunner;
use crate::types::RaidLevel;

/// mdadm RAID 操作
pub struct RaidManager<'a, R: CommandRunner> {
    runner: &'a R,
}

impl<'a, R: CommandRunner> RaidManager<'a, R> {
    pub fn new(runner: &'a R) -> Self {
        Self { runner }
    }

    /// RAID アレイを新規作成
    pub fn create_array(&self, md_device: &str, level: RaidLevel, devices: &[&str]) -> Result<()> {
        let level_str = match level {
            RaidLevel::Single => "1", // SINGLE は mdadm 的には RAID1 --force で1台
            RaidLevel::Raid1 => "1",
            RaidLevel::Raid5 => "5",
            RaidLevel::Raid6 => "6",
        };

        let raid_devices = devices.len().to_string();

        let mut args = vec![
            "--create",
            md_device,
            "--level",
            level_str,
            "--raid-devices",
            &raid_devices,
            "--metadata=1.2",
        ];

        // 1台構成は --force が必要
        if devices.len() == 1 {
            args.push("--force");
        }

        args.extend_from_slice(devices);

        self.runner.run("mdadm", &args)?;
        Ok(())
    }

    /// 既存アレイにデバイスを追加
    pub fn add_device(&self, md_device: &str, device: &str) -> Result<()> {
        self.runner.run("mdadm", &["--add", md_device, device])?;
        Ok(())
    }

    /// アレイのデバイス数を拡張 (--grow)
    pub fn grow(&self, md_device: &str, new_raid_devices: usize) -> Result<()> {
        let count = new_raid_devices.to_string();
        self.runner
            .run("mdadm", &["--grow", md_device, "--raid-devices", &count])?;
        Ok(())
    }

    /// アレイの RAID レベルを変更 (--grow --level)
    pub fn grow_level(
        &self,
        md_device: &str,
        new_level: RaidLevel,
        new_raid_devices: usize,
    ) -> Result<()> {
        let level_str = match new_level {
            RaidLevel::Single => "1",
            RaidLevel::Raid1 => "1",
            RaidLevel::Raid5 => "5",
            RaidLevel::Raid6 => "6",
        };
        let count = new_raid_devices.to_string();
        self.runner.run(
            "mdadm",
            &[
                "--grow",
                md_device,
                "--level",
                level_str,
                "--raid-devices",
                &count,
            ],
        )?;
        Ok(())
    }

    /// アレイ内のデバイスを fail 状態にする
    pub fn fail_device(&self, md_device: &str, device: &str) -> Result<()> {
        self.runner.run("mdadm", &["--fail", md_device, device])?;
        Ok(())
    }

    /// アレイからデバイスを除去する (fail 状態のデバイスのみ)
    pub fn remove_device(&self, md_device: &str, device: &str) -> Result<()> {
        self.runner.run("mdadm", &["--remove", md_device, device])?;
        Ok(())
    }

    /// アレイを停止
    pub fn stop(&self, md_device: &str) -> Result<()> {
        self.runner.run("mdadm", &["--stop", md_device])?;
        Ok(())
    }

    /// アレイを組み立て
    pub fn assemble(&self, md_device: &str, devices: &[&str]) -> Result<()> {
        let mut args = vec!["--assemble", md_device];
        args.extend_from_slice(devices);
        self.runner.run("mdadm", &args)?;
        Ok(())
    }
}
