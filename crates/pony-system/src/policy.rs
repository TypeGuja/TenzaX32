//! Политика использования ресурсов: сколько потоков поднимать, сколько
//! памяти брать под кэш, как раскидать работу по видеокартам —
//! всё вычисляется ИЗ профиля системы, а не захардкожено.
//!
//! Принцип: забирать реально доступное, но не всё подчистую —
//! система должна оставаться отзывчивой (ОС, другие приложения,
//! сам рендер-луп на CPU-стороне).

use crate::profile::SystemProfile;

#[derive(Debug, Clone)]
pub enum GpuAssignment {
    /// GPU не найден (или недоступен) — работаем в CPU-only режиме.
    None,
    /// Один адаптер — очевидный случай, весь рендер туда.
    Single(usize),
    /// Несколько адаптеров: индекс + нормализованная доля нагрузки
    /// (сумма весов = 1.0), пропорциональная эвристике "мощности" типа GPU.
    Multi(Vec<(usize, f32)>),
}

#[derive(Debug, Clone)]
pub struct WorkloadPolicy {
    /// Сколько потоков отдать rayon-пулу под CPU-работу (скелет,
    /// морфы, частицы, физика волос/гривы).
    pub worker_threads: usize,
    /// Сколько байт можно занять под кэш ассетов/текстур/мешей.
    pub memory_budget_bytes: u64,
    pub gpu_assignment: GpuAssignment,
}

/// Доля доступной памяти, которую можно занять под кэш движка.
/// Оставшееся — запас для ОС и остальных программ пользователя.
const MEMORY_BUDGET_FRACTION: f64 = 0.75;

/// На однопоточных/двухпоточных системах не пытаемся отобрать
/// последний доступный поток — иначе UI/render-луп начнёт тормозить.
const MIN_WORKER_THREADS: usize = 1;

impl WorkloadPolicy {
    pub fn from_profile(profile: &SystemProfile) -> Self {
        // Один логический поток резервируем под основной цикл
        // (событие/рендер-луп); всё остальное — rayon-пулу.
        let worker_threads = profile
            .cpu
            .logical_cores
            .saturating_sub(1)
            .max(MIN_WORKER_THREADS);

        let memory_budget_bytes =
            (profile.memory.available_bytes as f64 * MEMORY_BUDGET_FRACTION) as u64;

        let gpu_assignment = match profile.gpus.len() {
            0 => GpuAssignment::None,
            1 => GpuAssignment::Single(0),
            _ => {
                let total_weight: f32 = profile
                    .gpus
                    .iter()
                    .map(|g| g.device_type.relative_weight())
                    .sum();
                let weighted = profile
                    .gpus
                    .iter()
                    .enumerate()
                    .map(|(i, g)| (i, g.device_type.relative_weight() / total_weight))
                    .collect();
                GpuAssignment::Multi(weighted)
            }
        };

        Self {
            worker_threads,
            memory_budget_bytes,
            gpu_assignment,
        }
    }
}

/// Построить глобальный (или отдельный) rayon-пул размером под систему.
/// Используем именованный пул вместо `rayon::ThreadPoolBuilder::build_global`,
/// чтобы можно было держать несколько политик (например, пересчитать
/// на лету при смене питания ноутбука) без паники "global pool already set".
pub fn build_thread_pool(policy: &WorkloadPolicy) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(policy.worker_threads)
        .thread_name(|i| format!("pony-worker-{i}"))
        .build()
        .expect("failed to build rayon thread pool")
}
