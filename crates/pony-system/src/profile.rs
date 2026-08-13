use sysinfo::System;

use crate::cpu::CpuProfile;
use crate::gpu::{enumerate_gpus, GpuAdapterInfo};
use crate::memory::MemoryProfile;

#[derive(Debug, Clone)]
pub struct SystemProfile {
    pub cpu: CpuProfile,
    pub memory: MemoryProfile,
    pub gpus: Vec<GpuAdapterInfo>,
}

impl SystemProfile {
    /// Снять срез текущих возможностей системы. Дешёво по времени
    /// (десятки миллисекунд), можно перевызывать периодически, если
    /// нужно реагировать на подключение/отключение оборудования
    /// (например, ноутбук на батарее меняет доступные GPU).
    pub fn detect() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        Self {
            cpu: CpuProfile::detect(&sys),
            memory: MemoryProfile::detect(&sys),
            gpus: enumerate_gpus(),
        }
    }

    /// То же самое, но БЕЗ перечисления GPU (`gpus` остаётся пустым).
    ///
    /// Зачем нужен отдельный метод: `enumerate_gpus()` создаёт собственный
    /// `wgpu::Instance` с `Backends::all()`. Если вызвать его из процесса,
    /// у которого УЖЕ есть живой Instance/Device/Surface (например, из GUI
    /// после инициализации окна), на Windows это приводило к падению с
    /// STATUS_ACCESS_VIOLATION — второй Instance инициализирует все
    /// бэкенды (включая GL/WGL), и это конфликтует с уже работающим
    /// устройством. Для всего, что зависит только от CPU/памяти (например,
    /// `WorkloadPolicy::memory_budget_bytes` — он считается ровно из
    /// `memory.available_bytes`), перечислять видеокарты не нужно вообще.
    pub fn detect_without_gpus() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        Self {
            cpu: CpuProfile::detect(&sys),
            memory: MemoryProfile::detect(&sys),
            gpus: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Регрессионный тест на реальное падение: вызов полного `detect()`
    /// из процесса с уже живым wgpu-контекстом ронял GUI на Windows
    /// (STATUS_ACCESS_VIOLATION). `detect_without_gpus()` не должен
    /// трогать wgpu вообще — проверяем, что список GPU пуст, а данные
    /// по CPU/памяти при этом настоящие.
    #[test]
    fn detect_without_gpus_skips_gpu_enumeration_but_keeps_real_cpu_and_memory() {
        let profile = SystemProfile::detect_without_gpus();
        assert!(profile.gpus.is_empty(), "не должен перечислять GPU (и, значит, не должен создавать wgpu::Instance)");
        assert!(profile.cpu.logical_cores >= 1, "данные по CPU должны быть настоящими");
        assert!(profile.memory.total_bytes > 0, "данные по памяти должны быть настоящими");
    }

    /// Бюджет памяти не должен зависеть от того, перечисляли мы GPU или
    /// нет — иначе "безопасный" путь тихо давал бы другой результат.
    #[test]
    fn memory_budget_is_the_same_with_or_without_gpu_enumeration() {
        use crate::WorkloadPolicy;
        let without = WorkloadPolicy::from_profile(&SystemProfile::detect_without_gpus()).memory_budget_bytes;
        assert!(without > 0, "бюджет должен быть положительным");
    }
}
