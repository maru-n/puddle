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
    nth_failures: RefCell<HashMap<String, (usize, String)>>,
    call_counts: RefCell<HashMap<String, usize>>,
    stdout_map: RefCell<HashMap<String, String>>,
}

impl Default for MockCommandRunner {
    fn default() -> Self {
        Self {
            history: RefCell::new(Vec::new()),
            failures: RefCell::new(HashMap::new()),
            nth_failures: RefCell::new(HashMap::new()),
            call_counts: RefCell::new(HashMap::new()),
            stdout_map: RefCell::new(HashMap::new()),
        }
    }
}

impl MockCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定コマンドを常に失敗させる
    pub fn set_fail(&self, program: &str, message: &str) {
        self.failures
            .borrow_mut()
            .insert(program.to_string(), message.to_string());
    }

    /// 指定コマンドの N 回目の呼び出しで失敗させる (1-indexed)
    pub fn set_fail_on_nth(&self, program: &str, n: usize, message: &str) {
        self.nth_failures
            .borrow_mut()
            .insert(program.to_string(), (n, message.to_string()));
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

    /// 指定コマンドの呼び出し回数を取得する
    pub fn call_count(&self, program: &str) -> usize {
        self.call_counts.borrow().get(program).copied().unwrap_or(0)
    }
}

impl CommandRunner for MockCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        self.history.borrow_mut().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));

        // 呼び出し回数を更新
        let count = {
            let mut counts = self.call_counts.borrow_mut();
            let count = counts.entry(program.to_string()).or_insert(0);
            *count += 1;
            *count
        };

        // 常時失敗
        if let Some(msg) = self.failures.borrow().get(program) {
            bail!("{}", msg);
        }

        // N 回目の呼び出しで失敗
        if let Some((n, msg)) = self.nth_failures.borrow().get(program) {
            if count == *n {
                bail!("{}", msg);
            }
        }

        if let Some(stdout) = self.stdout_map.borrow().get(program) {
            return Ok(stdout.clone());
        }

        Ok(String::new())
    }
}
