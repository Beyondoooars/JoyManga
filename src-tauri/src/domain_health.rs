//! 线路健康记录：按域名累计连续失败次数与最近成功延迟。
//!
//! 目的：把"刚被墙/超时的线路"暂时排到候选列表末尾，避免每次请求都先撞它一轮。
//! 只影响**排序**，不删除候选；冷却结束后自动恢复，配置里的首选线路也始终保留。
//!
//! 与 `last_success_domain` 的分工：后者只记一条"最近成功"用于优先复用；
//! 这里记录每条线路的健康度，供 `api_domain_candidates` 排序使用。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// 连续失败达到该次数后进入冷却。
const FAILURE_THRESHOLD: u32 = 2;
/// 冷却时长；期间该线路排到所有健康线路之后。
const COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default)]
struct Health {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
    /// 最近一次成功时的耗时，用于同分排序（快的优先）
    last_success_latency: Option<Duration>,
    last_success: Option<Instant>,
}

impl Health {
    fn in_cooldown(&self, now: Instant) -> bool {
        self.consecutive_failures >= FAILURE_THRESHOLD
            && self
                .last_failure
                .is_some_and(|at| now.duration_since(at) < COOLDOWN)
    }

    /// 排序键：越小越优先。
    /// 冷却中的线路排最后；其余按"最近是否成功 → 延迟 → 连续失败次数"排序。
    fn rank(&self, now: Instant) -> (u8, u128, u32) {
        if self.in_cooldown(now) {
            return (2, u128::MAX, u32::MAX);
        }
        // 从未用过的线路排在已成功线路之后，但优于冷却中的
        let latency = self
            .last_success_latency
            .map_or(u128::MAX / 2, |value| value.as_millis());
        let tier = if self.last_success.is_some() { 0 } else { 1 };
        (tier, latency, self.consecutive_failures)
    }
}

#[derive(Default)]
pub struct DomainHealthStore {
    entries: RwLock<HashMap<String, Health>>,
}

impl DomainHealthStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_success(&self, domain: &str, latency: Duration) {
        let mut entries = self.entries.write();
        let entry = entries.entry(domain.to_string()).or_default();
        entry.consecutive_failures = 0;
        entry.last_success = Some(Instant::now());
        entry.last_success_latency = Some(latency);
    }

    pub fn record_failure(&self, domain: &str) {
        let mut entries = self.entries.write();
        let entry = entries.entry(domain.to_string()).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        entry.last_failure = Some(Instant::now());
    }

    /// 按健康度重排候选：健康线路在前（快者优先），冷却线路在后。
    /// 保持同分线路的原有相对顺序（稳定排序），避免打乱用户配置的首选。
    pub fn sort_candidates(&self, candidates: &mut [String]) {
        let now = Instant::now();
        let entries = self.entries.read();
        let rank_of = |domain: &str| -> (u8, u128, u32) {
            entries
                .get(domain)
                .map_or((1, u128::MAX / 2, 0), |health| health.rank(now))
        };
        let mut ranked: Vec<(usize, (u8, u128, u32), String)> = candidates
            .iter()
            .enumerate()
            .map(|(index, domain)| (index, rank_of(domain), domain.clone()))
            .collect();
        // 先按索引稳定排序，再按健康度排序，保证同分时保持原顺序
        ranked.sort_by_key(|(index, _, _)| *index);
        ranked.sort_by_key(|(_, rank, _)| *rank);
        for (slot, (_, _, domain)) in candidates.iter_mut().zip(ranked) {
            *slot = domain;
        }
    }

    /// 该线路当前是否处于冷却（供诊断日志/设置页展示）。
    pub fn is_cooling_down(&self, domain: &str) -> bool {
        let now = Instant::now();
        self.entries
            .read()
            .get(domain)
            .is_some_and(|health| health.in_cooldown(now))
    }

    pub fn failure_count(&self, domain: &str) -> u32 {
        self.entries
            .read()
            .get(domain)
            .map_or(0, |health| health.consecutive_failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_domain_moves_behind_healthy_one() {
        let store = DomainHealthStore::new();
        store.record_success("good.example", Duration::from_millis(80));
        store.record_failure("bad.example");
        store.record_failure("bad.example");

        let mut candidates = vec!["bad.example".to_string(), "good.example".to_string()];
        store.sort_candidates(&mut candidates);
        assert_eq!(candidates, vec!["good.example", "bad.example"]);
        assert!(store.is_cooling_down("bad.example"));
    }

    #[test]
    fn single_failure_does_not_cool_down_yet() {
        let store = DomainHealthStore::new();
        store.record_failure("flaky.example");
        assert!(!store.is_cooling_down("flaky.example"));
        assert_eq!(store.failure_count("flaky.example"), 1);
    }

    #[test]
    fn success_clears_cooldown() {
        let store = DomainHealthStore::new();
        store.record_failure("recover.example");
        store.record_failure("recover.example");
        assert!(store.is_cooling_down("recover.example"));

        store.record_success("recover.example", Duration::from_millis(120));
        assert!(!store.is_cooling_down("recover.example"));
        assert_eq!(store.failure_count("recover.example"), 0);
    }

    #[test]
    fn unknown_domains_keep_relative_order() {
        let store = DomainHealthStore::new();
        let mut candidates = vec!["a.example".to_string(), "b.example".to_string()];
        store.sort_candidates(&mut candidates);
        assert_eq!(candidates, vec!["a.example", "b.example"]);
    }

    #[test]
    fn faster_success_ranks_first() {
        let store = DomainHealthStore::new();
        store.record_success("slow.example", Duration::from_millis(400));
        store.record_success("fast.example", Duration::from_millis(60));

        let mut candidates = vec!["slow.example".to_string(), "fast.example".to_string()];
        store.sort_candidates(&mut candidates);
        assert_eq!(candidates, vec!["fast.example", "slow.example"]);
    }
}
