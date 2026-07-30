//! pony-system: определяет, сколько CPU-потоков, памяти и GPU реально
//! доступно на текущей машине, и превращает это в конкретную политику
//! использования ресурсов — от одного слабого ноутбука до рабочей
//! станции с несколькими видеокартами.

pub mod cpu;
pub mod gpu;
pub mod memory;
pub mod policy;
pub mod profile;

pub use cpu::CpuProfile;
pub use gpu::{GpuAdapterInfo, GpuDeviceType};
pub use memory::MemoryProfile;
pub use policy::{build_thread_pool, GpuAssignment, WorkloadPolicy};
pub use profile::SystemProfile;
