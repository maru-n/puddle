use anyhow::Result;

use super::command_runner::CommandRunner;

/// LVM ボリューム管理操作
pub struct VolumeManager<'a, R: CommandRunner> {
    runner: &'a R,
}

impl<'a, R: CommandRunner> VolumeManager<'a, R> {
    pub fn new(runner: &'a R) -> Self {
        Self { runner }
    }

    /// Physical Volume を作成
    pub fn pvcreate(&self, device: &str) -> Result<()> {
        self.runner.run("pvcreate", &["-f", device])?;
        Ok(())
    }

    /// Volume Group を作成
    pub fn vgcreate(&self, vg_name: &str, pvs: &[&str]) -> Result<()> {
        let mut args = vec![vg_name];
        args.extend_from_slice(pvs);
        self.runner.run("vgcreate", &args)?;
        Ok(())
    }

    /// Volume Group にPVを追加
    pub fn vgextend(&self, vg_name: &str, pv: &str) -> Result<()> {
        self.runner.run("vgextend", &[vg_name, pv])?;
        Ok(())
    }

    /// Logical Volume を作成 (VG の全空き領域を使用)
    pub fn lvcreate_full(&self, vg_name: &str, lv_name: &str) -> Result<()> {
        self.runner.run(
            "lvcreate",
            &["-l", "100%FREE", "-n", lv_name, "-y", "-Wn", "-Zn", vg_name],
        )?;
        Ok(())
    }

    /// PV 上のデータを他の PV に退避する
    pub fn pvmove(&self, pv: &str) -> Result<()> {
        self.runner.run("pvmove", &[pv])?;
        Ok(())
    }

    /// Volume Group から PV を除去する
    pub fn vgreduce(&self, vg_name: &str, pv: &str) -> Result<()> {
        self.runner.run("vgreduce", &[vg_name, pv])?;
        Ok(())
    }

    /// Physical Volume を除去
    pub fn pvremove(&self, pv: &str) -> Result<()> {
        self.runner.run("pvremove", &["-f", pv])?;
        Ok(())
    }

    /// PV の allocatable フラグを変更する (遅延割り当て用)
    pub fn pvchange_allocatable(&self, pv: &str, allocatable: bool) -> Result<()> {
        let flag = if allocatable { "y" } else { "n" };
        self.runner.run("pvchange", &["-x", flag, pv])?;
        Ok(())
    }

    /// Logical Volume を拡張 (VG の全空き領域を使用)
    ///
    /// 空き領域がない場合 (RAID1ミラー追加時など) はスキップする。
    pub fn lvextend_full(&self, lv_path: &str) -> Result<()> {
        match self.runner.run("lvextend", &["-l", "+100%FREE", lv_path]) {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("matches existing size") {
                    // 空き領域なし — 正常 (例: RAID1 ミラー追加時)
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}
