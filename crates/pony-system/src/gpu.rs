//! Список всех *физических* GPU в системе (интегрированных и дискретных)
//! плюс грубая эвристика их относительной "мощности" для распределения
//! нагрузки между несколькими видеокартами.
//!
//! ВАЖНО: `wgpu::Instance::enumerate_adapters` отдаёт одну запись на
//! КАЖДУЮ пару (физический GPU × graphics backend, который его видит) —
//! не на физическую карту. Одна и та же дискретная видеокарта на Windows
//! обычно доступна и через Vulkan, и через DirectX12, и через OpenGL —
//! то есть будет и три отдельных "адаптера" на один и тот же чип. Плюс
//! Windows почти всегда добавляет отдельный программный WARP-фоллбек
//! (`Microsoft Basic Render Driver`), который физическим GPU не является.
//! Наивный список поэтому завышает число видеокарт — здесь мы схлопываем
//! записи по PCI (vendor, device) id, который стабилен вне зависимости
//! от backend'а, и оставляем один (самый быстрый) backend на устройство.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceType {
    Discrete,
    Integrated,
    Virtual,
    /// Программный растеризатор (llvmpipe/lavapipe/WARP и т.п.) — считается
    /// доступным GPU-слотом, но с низким весом.
    Cpu,
    Other,
}

impl GpuDeviceType {
    /// Эвристический коэффициент мощности. Не претендует на точность —
    /// это стартовая точка для распределения работы; в перспективе стоит
    /// заменить на реальный бенчмарк-прогон при первом запуске.
    pub fn relative_weight(self) -> f32 {
        match self {
            GpuDeviceType::Discrete => 1.0,
            GpuDeviceType::Integrated => 0.4,
            GpuDeviceType::Virtual => 0.2,
            GpuDeviceType::Cpu => 0.1,
            GpuDeviceType::Other => 0.3,
        }
    }

    /// Порядок для стабильной сортировки вывода (дискретные — вперёд).
    fn sort_rank(self) -> u8 {
        match self {
            GpuDeviceType::Discrete => 0,
            GpuDeviceType::Integrated => 1,
            GpuDeviceType::Virtual => 2,
            GpuDeviceType::Other => 3,
            GpuDeviceType::Cpu => 4,
        }
    }
}

impl From<wgpu::DeviceType> for GpuDeviceType {
    fn from(dt: wgpu::DeviceType) -> Self {
        match dt {
            wgpu::DeviceType::DiscreteGpu => GpuDeviceType::Discrete,
            wgpu::DeviceType::IntegratedGpu => GpuDeviceType::Integrated,
            wgpu::DeviceType::VirtualGpu => GpuDeviceType::Virtual,
            wgpu::DeviceType::Cpu => GpuDeviceType::Cpu,
            wgpu::DeviceType::Other => GpuDeviceType::Other,
        }
    }
}

/// Приоритет backend'а при выборе, какую из нескольких API-проекций
/// одного и того же физического GPU оставить. Ниже число — выше приоритет.
/// Vulkan/Metal — нативные низкоуровневые API на своих платформах;
/// Dx12 — тоже низкоуровневый, оставляем вторым по приоритету; GL — legacy-путь
/// с наибольшими накладными расходами внутри wgpu.
fn backend_priority(backend: wgpu::Backend) -> u8 {
    match backend {
        wgpu::Backend::Vulkan | wgpu::Backend::Metal => 0,
        wgpu::Backend::Dx12 => 1,
        wgpu::Backend::Gl => 2,
        wgpu::Backend::BrowserWebGpu => 3,
        wgpu::Backend::Empty => 4,
    }
}

#[derive(Debug, Clone)]
pub struct GpuAdapterInfo {
    /// Индекс в ДЕДУПЛИЦИРОВАННОМ списке — ссылается на физический GPU,
    /// не на конкретную пару (GPU, backend).
    pub index: usize,
    pub name: String,
    pub backend: String,
    pub device_type: GpuDeviceType,
    /// PCI vendor/device id — используются для повторного поиска
    /// нужного `wgpu::Adapter` на этапе реальной инициализации в
    /// pony-render (там снова придётся enumerate_adapters, но уже
    /// сопоставляя по этим id + backend, а не по позиции в списке).
    pub vendor: u32,
    pub device: u32,
}

/// Перечислить все физические GPU, видимые хоть через один backend
/// (Vulkan/Metal/DX12/GL), схлопнув дубликаты одного и того же чипа под
/// разными API. На системах без GPU (или без драйверов) вернёт пустой
/// список — это нормальный, ожидаемый случай, а не ошибка.
pub fn enumerate_gpus() -> Vec<GpuAdapterInfo> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    let raw_infos: Vec<wgpu::AdapterInfo> = instance
        .enumerate_adapters(wgpu::Backends::all())
        .into_iter()
        .map(|adapter| adapter.get_info())
        .collect();

    // Схлопываем по (vendor, device) — это ID физического чипа, общий
    // для всех backend'ов, которые его видят.
    let mut by_hardware: HashMap<(u32, u32), wgpu::AdapterInfo> = HashMap::new();
    for info in raw_infos {
        let key = (info.vendor, info.device);
        let should_replace = match by_hardware.get(&key) {
            Some(existing) => backend_priority(info.backend) < backend_priority(existing.backend),
            None => true,
        };
        if should_replace {
            by_hardware.insert(key, info);
        }
    }

    let mut deduped: Vec<wgpu::AdapterInfo> = by_hardware.into_values().collect();
    // HashMap не гарантирует порядок — сортируем детерминированно
    // (дискретные вперёд, затем по имени), чтобы вывод и индексы не
    // прыгали между запусками на одной и той же машине.
    deduped.sort_by(|a, b| {
        let rank_a = GpuDeviceType::from(a.device_type).sort_rank();
        let rank_b = GpuDeviceType::from(b.device_type).sort_rank();
        rank_a.cmp(&rank_b).then_with(|| a.name.cmp(&b.name))
    });

    deduped
        .into_iter()
        .enumerate()
        .map(|(index, info)| GpuAdapterInfo {
            index,
            name: info.name,
            backend: format!("{:?}", info.backend),
            device_type: GpuDeviceType::from(info.device_type),
            vendor: info.vendor,
            device: info.device,
        })
        .collect()
}
