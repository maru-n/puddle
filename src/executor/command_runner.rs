use anyhow::{bail, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;

/// 外部コマンド実行の抽象化
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String>;
}

/// 実際にコマンドを実行する実装
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to execute {}: {}", program, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{} failed (exit {}): {}", program, output.status, stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// テスト用モックランナー
pub struct MockCommandRunner {
    history: RefCell<Vec<(String, Vec<String>)>>,
    failures: RefCell<HashMap<String, String>>,
    stdout_map: RefCell<HashMap<String, String>>,
}

impl Default for MockCommandRunner {
    fn default() -> Self {
        Self {
            history: RefCell::new(Vec::new()),
            failures: RefCell::new(HashMap::new()),
            stdout_map: RefCell::new(HashMap::new()),
        }
    }
}

impl MockCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定コマンドを失敗させる
    pub fn set_fail(&self, program: &str, message: &str) {
        self.failures
            .borrow_mut()
            .insert(program.to_string(), message.to_string());
    }

    /// 指定コマンドの stdout を設定する
    pub fn set_stdout(&self, program: &str, stdout: &str) {
        self.stdout_map
            .borrow_mut()
            .insert(program.to_string(), stdout.to_string());
    }

    /// 実行履歴を取得する
    pub fn history(&self) -> Vec<(String, Vec<String>)> {
        self.history.borrow().clone()
    }
}

impl CommandRunner for MockCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        self.history.borrow_mut().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));

        if let Some(msg) = self.failures.borrow().get(program) {
            bail!("{}", msg);
        }

        if let Some(stdout) = self.stdout_map.borrow().get(program) {
            return Ok(stdout.clone());
        }

        Ok(String::new())
    }
}
