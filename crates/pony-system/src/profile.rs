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
}
