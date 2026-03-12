use anyhow::{Context, Result};
use serde::Deserialize;

/// SMART 情報
#[derive(Debug, Clone)]
pub struct SmartInfo {
    pub model: String,
    pub passed: bool,
    pub temperature_celsius: Option<i64>,
    pub reallocated_sectors: Option<u64>,
}

// serde 用の内部構造体
#[derive(Deserialize)]
struct SmartJson {
    model_name: Option<String>,
    smart_status: Option<SmartStatus>,
    temperature: Option<Temperature>,
    ata_smart_attributes: Option<AtaSmartAttributes>,
}

#[derive(Deserialize)]
struct SmartStatus {
    passed: bool,
}

#[derive(Deserialize)]
struct Temperature {
    current: Option<i64>,
}

#[derive(Deserialize)]
struct AtaSmartAttributes {
    table: Vec<AtaAttribute>,
}

#[derive(Deserialize)]
struct AtaAttribute {
    id: u32,
    #[allow(dead_code)]
    name: String,
    raw: AtaRawValue,
}

#[derive(Deserialize)]
struct AtaRawValue {
    value: u64,
}

/// `smartctl -j` の JSON 出力をパースする
pub fn parse_smart_json(json_str: &str) -> Result<SmartInfo> {
    let parsed: SmartJson =
        serde_json::from_str(json_str).context("Failed to parse smartctl JSON")?;

    let reallocated = parsed.ata_smart_attributes.as_ref().and_then(|attrs| {
        attrs
            .table
            .iter()
            .find(|a| a.id == 5) // Reallocated_Sector_Ct
            .map(|a| a.raw.value)
    });

    Ok(SmartInfo {
        model: parsed.model_name.unwrap_or_default(),
        passed: parsed.smart_status.map(|s| s.passed).unwrap_or(true),
        temperature_celsius: parsed.temperature.and_then(|t| t.current),
        reallocated_sectors: reallocated,
    })
}
