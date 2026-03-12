use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::Write;

use super::command_runner::CommandRunner;

/// 操作ステップの記録
#[derive(Debug, Clone)]
pub struct OperationStep {
    pub description: String,
    pub command: String,
    pub rollback_command: String,
}

/// 操作ログ: 各ステップとそのロールバックコマンドを記録する
///
/// Phase 1 ではログ記録のみ。自動ロールバック実行は Phase 2。
/// フォーマットは SPEC §7.3 に準拠。
#[derive(Debug)]
pub struct OperationLog {
    operation: String,
    steps: Vec<OperationStep>,
    committed: bool,
}

impl OperationLog {
    pub fn new(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            steps: Vec::new(),
            committed: false,
        }
    }

    /// 操作名を返す
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// 記録済みステップを返す
    pub fn steps(&self) -> &[OperationStep] {
        &self.steps
    }

    /// コミット済みかどうか
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// ステップを記録する
    pub fn log_step(&mut self, description: &str, command: &str, rollback_command: &str) {
        self.steps.push(OperationStep {
            description: description.to_string(),
            command: command.to_string(),
            rollback_command: rollback_command.to_string(),
        });
    }

    /// 操作を完了としてマークする
    pub fn commit(&mut self) {
        self.committed = true;
    }

    /// ロールバックコマンドを逆順で返す
    pub fn rollback_commands(&self) -> Vec<&str> {
        self.steps
            .iter()
            .rev()
            .map(|s| s.rollback_command.as_str())
            .collect()
    }

    /// ログをフォーマットされた文字列にする (SPEC §7.3 準拠)
    pub fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("BEGIN {}\n", self.operation));

        for (i, step) in self.steps.iter().enumerate() {
            out.push_str(&format!("STEP {}: {} → OK\n", i + 1, step.command));
            out.push_str(&format!("  ROLLBACK: {}\n", step.rollback_command));
        }

        if self.committed {
            out.push_str(&format!("COMMIT {}\n", self.operation));
        }

        out
    }

    /// ロールバックコマンドを逆順に実行する
    pub fn execute_rollback<R: CommandRunner>(&self, runner: &R) -> Result<()> {
        for cmd in self.rollback_commands() {
            if cmd.is_empty() {
                continue;
            }
            runner
                .run("sh", &["-c", cmd])
                .context(format!("Rollback command failed: {}", cmd))?;
        }
        Ok(())
    }

    /// ログをファイルに追記保存する
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context(format!("Failed to open operation log: {}", path))?;

        writeln!(file, "{}", self.format()).context("Failed to write operation log")?;

        Ok(())
    }
}
