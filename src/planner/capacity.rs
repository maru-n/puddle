use crate::types::ZonePlan;

/// 容量情報のサマリ (表示用)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacitySummary {
    pub physical_bytes: u64,
    pub usable_bytes: u64,
    pub overhead_bytes: u64,
}

/// ZonePlan から容量サマリを計算する
pub fn summarize(plan: &ZonePlan) -> CapacitySummary {
    let overhead = plan.total_physical_bytes - plan.total_effective_bytes;
    CapacitySummary {
        physical_bytes: plan.total_physical_bytes,
        usable_bytes: plan.total_effective_bytes,
        overhead_bytes: overhead,
    }
}

/// バイト数を人間可読な文字列に変換する
pub fn format_bytes(bytes: u64) -> String {
    const TB: u64 = 1_000_000_000_000;
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;

    if bytes >= TB {
        let whole = bytes / TB;
        let frac = (bytes % TB) / (TB / 10);
        format!("{}.{} TB", whole, frac)
    } else if bytes >= GB {
        let whole = bytes / GB;
        let frac = (bytes % GB) / (GB / 10);
        format!("{}.{} GB", whole, frac)
    } else {
        let whole = bytes / MB;
        format!("{} MB", whole)
    }
}
