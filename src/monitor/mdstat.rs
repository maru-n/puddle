/// /proc/mdstat の各アレイの状態
#[derive(Debug, Clone)]
pub struct MdArrayStatus {
    pub name: String,
    pub level: String,
    pub num_devices: usize,
    pub active_devices: usize,
    pub recovery_percent: Option<f64>,
}

impl MdArrayStatus {
    /// アレイが正常 (全デバイスアクティブ) かどうか
    pub fn is_clean(&self) -> bool {
        self.num_devices == self.active_devices && self.recovery_percent.is_none()
    }
}

/// /proc/mdstat の内容をパースする
pub fn parse_mdstat(content: &str) -> Vec<MdArrayStatus> {
    let mut arrays = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // "md0 : active raid5 sdd2[2] sdc2[1] sdb2[0]" のような行を探す
        if line.starts_with("md") && line.contains(" : active ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let name = parts[0].to_string();

            // RAID レベルは "active" の次
            let level = parts
                .iter()
                .position(|&p| p == "active")
                .and_then(|pos| parts.get(pos + 1))
                .unwrap_or(&"unknown")
                .to_string();

            // [N/M] の形式を探す (後続行にある)
            let mut num_devices = 0;
            let mut active_devices = 0;
            let mut recovery_percent = None;

            // 次の数行を調べる
            for scan_line in lines.iter().skip(i).take(4) {
                // [3/2] [UU_] のようなパターン
                if let Some(bracket_start) = scan_line.find('[') {
                    let after = &scan_line[bracket_start + 1..];
                    if let Some(slash) = after.find('/') {
                        if let Some(bracket_end) = after.find(']') {
                            if slash < bracket_end {
                                if let Ok(n) = after[..slash].parse::<usize>() {
                                    if let Ok(a) = after[slash + 1..bracket_end].parse::<usize>() {
                                        num_devices = n;
                                        active_devices = a;
                                    }
                                }
                            }
                        }
                    }
                }

                // recovery = 22.3%
                if scan_line.contains("recovery =") || scan_line.contains("resync =") {
                    if let Some(pct_pos) = scan_line.find('%') {
                        let before_pct = &scan_line[..pct_pos];
                        let num_start = before_pct.rfind('=').unwrap_or(0) + 1;
                        let pct_str = before_pct[num_start..].trim();
                        if let Ok(pct) = pct_str.parse::<f64>() {
                            recovery_percent = Some(pct);
                        }
                    }
                }
            }

            arrays.push(MdArrayStatus {
                name,
                level,
                num_devices,
                active_devices,
                recovery_percent,
            });
        }

        i += 1;
    }

    arrays
}
