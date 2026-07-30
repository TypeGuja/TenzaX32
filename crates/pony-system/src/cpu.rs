//! Сколько потоков реально даёт система — логических и физических.
//! Физические важны для эвристик (гипертрединг даёт логические
//! ядра, но не удваивает реальную вычислительную мощность).

use sysinfo::System;

#[derive(Debug, Clone, Copy)]
pub struct CpuProfile {
    pub logical_cores: usize,
    pub physical_cores: usize,
}

impl CpuProfile {
    pub fn detect(sys: &System) -> Self {
        let logical_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let physical_cores = sys.physical_core_count().unwrap_or(logical_cores).max(1);
        Self {
            logical_cores,
            physical_cores,
        }
    }
}
