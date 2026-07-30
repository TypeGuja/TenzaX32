//! Сколько памяти реально свободно сейчас — не "сколько всего в системе",
//! а "сколько можно безопасно забрать под кэш ассетов/меши/текстуры".

use sysinfo::System;

#[derive(Debug, Clone, Copy)]
pub struct MemoryProfile {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl MemoryProfile {
    pub fn detect(sys: &System) -> Self {
        // sysinfo (0.30+) отдаёт значения в байтах.
        Self {
            total_bytes: sys.total_memory(),
            available_bytes: sys.available_memory(),
        }
    }
}
