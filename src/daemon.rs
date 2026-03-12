use anyhow::Result;

use crate::executor::command_runner::CommandRunner;
use crate::monitor::mdstat::parse_mdstat;
use crate::monitor::smart::parse_smart_json;

/// デーモンが検知するイベント
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEvent {
    /// SMART 異常検知
    SmartWarning { device: String, message: String },
    /// RAID degraded 検知
    RaidDegraded {
        array: String,
        active: usize,
        total: usize,
    },
    /// RAID リビルド中
    RaidRebuilding { array: String, percent: f64 },
    /// SMART チェック正常完了
    SmartOk { device: String },
    /// RAID チェック正常完了
    RaidOk { array: String },
}

/// 1台のディスクの SMART をチェックし、異常があればイベントを返す
pub fn check_smart<R: CommandRunner>(runner: &R, device: &str) -> Result<Vec<DaemonEvent>> {
    let output = runner.run("smartctl", &["-j", device])?;
    let info = parse_smart_json(&output)?;
    let mut events = Vec::new();

    if !info.passed {
        events.push(DaemonEvent::SmartWarning {
            device: device.to_string(),
            message: "SMART overall-health self-assessment test result: FAILED".to_string(),
        });
    }

    if let Some(sectors) = info.reallocated_sectors {
        if sectors > 0 {
            events.push(DaemonEvent::SmartWarning {
                device: device.to_string(),
                message: format!("Reallocated sector count: {}", sectors),
            });
        }
    }

    if let Some(temp) = info.temperature_celsius {
        if temp >= 55 {
            events.push(DaemonEvent::SmartWarning {
                device: device.to_string(),
                message: format!("High temperature: {}°C", temp),
            });
        }
    }

    if events.is_empty() {
        events.push(DaemonEvent::SmartOk {
            device: device.to_string(),
        });
    }

    Ok(events)
}

/// /proc/mdstat をチェックし、異常があればイベントを返す
pub fn check_mdstat<R: CommandRunner>(runner: &R) -> Result<Vec<DaemonEvent>> {
    let content = runner.run("cat", &["/proc/mdstat"])?;
    let arrays = parse_mdstat(&content);
    let mut events = Vec::new();

    for array in &arrays {
        if let Some(pct) = array.recovery_percent {
            events.push(DaemonEvent::RaidRebuilding {
                array: array.name.clone(),
                percent: pct,
            });
        } else if !array.is_clean() {
            events.push(DaemonEvent::RaidDegraded {
                array: array.name.clone(),
                active: array.active_devices,
                total: array.num_devices,
            });
        } else {
            events.push(DaemonEvent::RaidOk {
                array: array.name.clone(),
            });
        }
    }

    Ok(events)
}

/// 全デバイスの1回分のポーリングを実行し、イベントを収集する
pub fn poll_once<R: CommandRunner>(runner: &R, devices: &[String]) -> Vec<DaemonEvent> {
    let mut events = Vec::new();

    // SMART チェック
    for device in devices {
        match check_smart(runner, device) {
            Ok(evts) => events.extend(evts),
            Err(e) => {
                events.push(DaemonEvent::SmartWarning {
                    device: device.clone(),
                    message: format!("Failed to read SMART data: {}", e),
                });
            }
        }
    }

    // mdstat チェック
    match check_mdstat(runner) {
        Ok(evts) => events.extend(evts),
        Err(e) => {
            eprintln!("WARNING: Failed to read /proc/mdstat: {}", e);
        }
    }

    events
}

/// イベントのフォーマット (ログ出力用)
pub fn format_event(event: &DaemonEvent) -> String {
    match event {
        DaemonEvent::SmartWarning { device, message } => {
            format!("[WARN] SMART {}: {}", device, message)
        }
        DaemonEvent::RaidDegraded {
            array,
            active,
            total,
        } => {
            format!(
                "[WARN] RAID {} degraded: {}/{} active",
                array, active, total
            )
        }
        DaemonEvent::RaidRebuilding { array, percent } => {
            format!("[INFO] RAID {} rebuilding: {:.1}%", array, percent)
        }
        DaemonEvent::SmartOk { device } => {
            format!("[OK] SMART {}: healthy", device)
        }
        DaemonEvent::RaidOk { array } => {
            format!("[OK] RAID {}: clean", array)
        }
    }
}

/// イベントが警告レベルかどうか
pub fn is_warning(event: &DaemonEvent) -> bool {
    matches!(
        event,
        DaemonEvent::SmartWarning { .. } | DaemonEvent::RaidDegraded { .. }
    )
}

/// webhook で警告イベントを通知する
///
/// curl を使って HTTP POST を送信する。JSON ペイロードには
/// イベントの種類と詳細を含む。
pub fn send_webhook<R: CommandRunner>(
    runner: &R,
    webhook_url: &str,
    events: &[DaemonEvent],
) -> Result<()> {
    let warnings: Vec<_> = events.iter().filter(|e| is_warning(e)).collect();
    if warnings.is_empty() {
        return Ok(());
    }

    let messages: Vec<String> = warnings.iter().map(|e| format_event(e)).collect();
    let payload = format!(
        r#"{{"source":"puddle","level":"warning","count":{},"messages":{}}}"#,
        messages.len(),
        serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string()),
    );

    runner.run(
        "curl",
        &[
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload,
            "--max-time",
            "10",
            webhook_url,
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::command_runner::MockCommandRunner;

    fn smart_json_ok() -> &'static str {
        r#"{
            "model_name": "WDC WD40EFRX",
            "smart_status": {"passed": true},
            "temperature": {"current": 35},
            "ata_smart_attributes": {
                "table": [
                    {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 0}}
                ]
            }
        }"#
    }

    fn smart_json_failing() -> &'static str {
        r#"{
            "model_name": "WDC WD40EFRX",
            "smart_status": {"passed": false},
            "temperature": {"current": 60},
            "ata_smart_attributes": {
                "table": [
                    {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 42}}
                ]
            }
        }"#
    }

    fn mdstat_clean() -> &'static str {
        "Personalities : [raid1] [raid5] \nmd0 : active raid5 sdd2[2] sdc2[1] sdb2[0]\n      4194304 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]\n\nunused devices: <none>\n"
    }

    fn mdstat_degraded() -> &'static str {
        "Personalities : [raid1] [raid5] \nmd0 : active raid5 sdc2[1] sdb2[0]\n      4194304 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [UU_]\n\nunused devices: <none>\n"
    }

    fn mdstat_rebuilding() -> &'static str {
        "Personalities : [raid1] [raid5] \nmd0 : active raid5 sdd2[2] sdc2[1] sdb2[0]\n      4194304 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/2] [UU_]\n      [=====>...............]  recovery = 28.3% (595008/2097152) finish=1.2min speed=200000K/sec\n\nunused devices: <none>\n"
    }

    #[test]
    fn test_check_smart_healthy_disk() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("smartctl", smart_json_ok());
        let events = check_smart(&runner, "/dev/sda").unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], DaemonEvent::SmartOk { device } if device == "/dev/sda"));
    }

    #[test]
    fn test_check_smart_failing_disk() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("smartctl", smart_json_failing());
        let events = check_smart(&runner, "/dev/sdb").unwrap();

        // 3 warnings: FAILED, reallocated sectors, high temp
        assert_eq!(events.len(), 3);
        assert!(events
            .iter()
            .all(|e| matches!(e, DaemonEvent::SmartWarning { .. })));
    }

    #[test]
    fn test_check_mdstat_clean() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("cat", mdstat_clean());
        let events = check_mdstat(&runner).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], DaemonEvent::RaidOk { array } if array == "md0"));
    }

    #[test]
    fn test_check_mdstat_degraded() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("cat", mdstat_degraded());
        let events = check_mdstat(&runner).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DaemonEvent::RaidDegraded { array, active: 2, total: 3 } if array == "md0"
        ));
    }

    #[test]
    fn test_check_mdstat_rebuilding() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("cat", mdstat_rebuilding());
        let events = check_mdstat(&runner).unwrap();
        assert_eq!(events.len(), 1);
        if let DaemonEvent::RaidRebuilding { array, percent } = &events[0] {
            assert_eq!(array, "md0");
            assert!((percent - 28.3).abs() < 0.1);
        } else {
            panic!("Expected RaidRebuilding event");
        }
    }

    #[test]
    fn test_poll_once_combines_events() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("smartctl", smart_json_ok());
        runner.set_stdout("cat", mdstat_clean());

        let devices = vec!["/dev/sda".to_string()];
        let events = poll_once(&runner, &devices);

        assert_eq!(events.len(), 2); // 1 SMART ok + 1 RAID ok
    }

    #[test]
    fn test_poll_once_smart_failure_becomes_warning() {
        let runner = MockCommandRunner::new();
        runner.set_fail("smartctl", "command not found");
        runner.set_stdout("cat", mdstat_clean());

        let devices = vec!["/dev/sda".to_string()];
        let events = poll_once(&runner, &devices);

        assert!(events
            .iter()
            .any(|e| matches!(e, DaemonEvent::SmartWarning { .. })));
    }

    #[test]
    fn test_format_event_warning() {
        let event = DaemonEvent::SmartWarning {
            device: "/dev/sda".to_string(),
            message: "FAILED".to_string(),
        };
        assert_eq!(format_event(&event), "[WARN] SMART /dev/sda: FAILED");
    }

    #[test]
    fn test_format_event_degraded() {
        let event = DaemonEvent::RaidDegraded {
            array: "md0".to_string(),
            active: 2,
            total: 3,
        };
        assert_eq!(format_event(&event), "[WARN] RAID md0 degraded: 2/3 active");
    }

    #[test]
    fn test_format_event_rebuilding() {
        let event = DaemonEvent::RaidRebuilding {
            array: "md0".to_string(),
            percent: 45.6,
        };
        assert_eq!(format_event(&event), "[INFO] RAID md0 rebuilding: 45.6%");
    }

    #[test]
    fn test_is_warning() {
        assert!(is_warning(&DaemonEvent::SmartWarning {
            device: "x".into(),
            message: "y".into(),
        }));
        assert!(is_warning(&DaemonEvent::RaidDegraded {
            array: "md0".into(),
            active: 1,
            total: 3,
        }));
        assert!(!is_warning(&DaemonEvent::SmartOk { device: "x".into() }));
        assert!(!is_warning(&DaemonEvent::RaidOk {
            array: "md0".into(),
        }));
        assert!(!is_warning(&DaemonEvent::RaidRebuilding {
            array: "md0".into(),
            percent: 50.0,
        }));
    }

    #[test]
    fn test_send_webhook_with_warnings() {
        let runner = MockCommandRunner::new();
        runner.set_stdout("curl", ""); // curl 成功

        let events = vec![
            DaemonEvent::SmartWarning {
                device: "/dev/sda".to_string(),
                message: "FAILED".to_string(),
            },
            DaemonEvent::SmartOk {
                device: "/dev/sdb".to_string(),
            },
        ];

        send_webhook(&runner, "http://example.com/hook", &events).unwrap();

        // curl が呼ばれたことを確認
        let history = runner.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].0, "curl");
        // URL が引数に含まれる
        assert!(history[0]
            .1
            .contains(&"http://example.com/hook".to_string()));
        // JSON ペイロードが含まれる
        let payload_arg = history[0].1.iter().find(|a| a.contains("puddle")).unwrap();
        assert!(payload_arg.contains("warning"));
        assert!(payload_arg.contains("FAILED"));
    }

    #[test]
    fn test_send_webhook_no_warnings_skips() {
        let runner = MockCommandRunner::new();

        let events = vec![DaemonEvent::SmartOk {
            device: "/dev/sda".to_string(),
        }];

        send_webhook(&runner, "http://example.com/hook", &events).unwrap();

        // 警告なし → curl は呼ばれない
        assert!(runner.history().is_empty());
    }

    #[test]
    fn test_send_webhook_failure_propagates() {
        let runner = MockCommandRunner::new();
        runner.set_fail("curl", "connection refused");

        let events = vec![DaemonEvent::RaidDegraded {
            array: "md0".to_string(),
            active: 1,
            total: 3,
        }];

        let result = send_webhook(&runner, "http://example.com/hook", &events);
        assert!(result.is_err());
    }
}
