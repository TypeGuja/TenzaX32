//! Рендер-слой поверх нескольких GPU одновременно. Идея: под каждый
//! адаптер, который политика (`pony_system::WorkloadPolicy`) решила
//! задействовать, поднимаем отдельные `wgpu::Device`/`wgpu::Queue`
//! (у wgpu нет "одного устройства на несколько GPU" — так это и
//! делается в реальных мульти-GPU движках: N независимых контекстов
//! + ручное распределение работы между ними).
//!
//! Если GPU не найден вообще — `RenderCluster` будет пустым, и вызывающий
//! код должен переключиться на CPU-путь (например, программный
//! растеризатор или упрощённый рендер через egui/canvas). Это
//! осознанный сценарий, а не ошибка: движок обязан запускаться и на
//! машине без видеокарты.

use pony_system::{GpuAdapterInfo, GpuAssignment, SystemProfile};

pub mod budget;
pub mod export;
pub mod renderer;
pub mod texture;
pub use budget::LruBudget;
pub use export::{export_gif, export_spritesheet, ExportError, SpriteSheetLayout};
pub use renderer::{nominal_part_size, part_render_size, part_world_position, pivot_for_world_position, FrameOutput, Renderer, DEPTH_PER_LAYER};
pub use texture::{LoadedTexture, TextureCache, TextureLoadError};

#[derive(Debug, thiserror::Error)]
pub enum RenderInitError {
    #[error("failed to request GPU device for adapter '{adapter_name}': {source}")]
    DeviceRequest {
        adapter_name: String,
        #[source]
        source: wgpu::RequestDeviceError,
    },
}

/// Один активный GPU-контекст: устройство + очередь + информация об
/// адаптере + доля нагрузки, которую этот GPU должен нести относительно
/// остальных (при мульти-GPU).
pub struct GpuContext {
    pub info: GpuAdapterInfo,
    pub weight: f32,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Набор из 0..N активных GPU-контекстов, готовых делить между собой
/// рендер-нагрузку.
#[derive(Default)]
pub struct RenderCluster {
    pub contexts: Vec<GpuContext>,
}

impl RenderCluster {
    /// Поднять контексты под все GPU, которые политика решила
    /// задействовать. `GpuAssignment::None` даёт пустой кластер —
    /// это валидный CPU-only результат, не паникуем и не считаем ошибкой.
    pub async fn initialize(
        profile: &SystemProfile,
        assignment: &GpuAssignment,
    ) -> Result<Self, RenderInitError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let raw_adapters = instance.enumerate_adapters(wgpu::Backends::all());

        let targets: Vec<(usize, f32)> = match assignment {
            GpuAssignment::None => Vec::new(),
            GpuAssignment::Single(idx) => vec![(*idx, 1.0)],
            GpuAssignment::Multi(weighted) => weighted.clone(),
        };

        let mut contexts = Vec::with_capacity(targets.len());
        for (idx, weight) in targets {
            let Some(info) = profile.gpus.get(idx) else {
                // Профиль устарел (GPU отключили между detect() и initialize())
                // — пропускаем этот слот вместо падения всего кластера.
                continue;
            };
            // pony-system дедуплицирует физические GPU по (vendor, device),
            // но здесь нам нужен конкретный wgpu::Adapter для выбранного
            // backend'а — снова ищем его среди сырых адаптеров по тем же
            // (vendor, device) + backend, а не по позиции в списке (позиции
            // между "физическим" списком pony-system и "сырым" списком
            // enumerate_adapters не совпадают, там разное число элементов).
            let Some(adapter) = raw_adapters.iter().find(|a| {
                let ai = a.get_info();
                ai.vendor == info.vendor
                    && ai.device == info.device
                    && format!("{:?}", ai.backend) == info.backend
            }) else {
                continue;
            };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|source| RenderInitError::DeviceRequest {
                    adapter_name: info.name.clone(),
                    source,
                })?;
            contexts.push(GpuContext {
                info: info.clone(),
                weight,
                device,
                queue,
            });
        }

        Ok(Self { contexts })
    }

    pub fn is_cpu_only(&self) -> bool {
        self.contexts.is_empty()
    }

    /// Выбрать контекст для очередного юнита работы (кадр, персонаж,
    /// сцена — единица дробления решается на уровне вызывающего кода).
    /// Взвешенный round-robin: пропорционально раскидывает работу по
    /// GPU согласно их относительной мощности, без внешних состояний
    /// (детерминированно от job_index).
    pub fn pick(&self, job_index: usize) -> Option<&GpuContext> {
        if self.contexts.is_empty() {
            return None;
        }
        if self.contexts.len() == 1 {
            return self.contexts.first();
        }

        let weights: Vec<f32> = self.contexts.iter().map(|c| c.weight).collect();
        let idx = weighted_pick_index(&weights, job_index);
        self.contexts.get(idx)
    }
}

/// Чистая функция без побочных эффектов: по вектору весов и индексу job'а
/// возвращает, какой контекст должен его обработать. Вынесена отдельно,
/// чтобы её можно было протестировать без реального GPU (см. тесты ниже) —
/// в этом контейнере физически нет второй видеокарты, поэтому саму
/// пропорциональность распределения проверяем математически, а не "на глаз".
fn weighted_pick_index(weights: &[f32], job_index: usize) -> usize {
    if weights.len() <= 1 {
        return 0;
    }
    let total_weight: f32 = weights.iter().sum();
    // Низкодискрепантная последовательность (золотое сечение) — по мере
    // роста job_index доля попаданий в каждый контекст сходится к его
    // весу быстрее и равномернее, чем при обычном (job_index % N).
    let scaled = (job_index as f32 * 0.6180339887) % 1.0 * total_weight;
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        acc += w;
        if scaled <= acc {
            return i;
        }
    }
    weights.len() - 1
}

#[cfg(test)]
mod tests {
    use super::weighted_pick_index;

    #[test]
    fn single_context_always_zero() {
        for job in 0..10 {
            assert_eq!(weighted_pick_index(&[1.0], job), 0);
        }
    }

    #[test]
    fn distributes_proportionally_to_weights() {
        // Дискретная GPU (вес 0.7) должна получать заметно больше job'ов,
        // чем встроенная (вес 0.3), пропорционально весам.
        let weights = [0.7_f32, 0.3];
        let n = 10_000;
        let mut counts = [0usize; 2];
        for job in 0..n {
            counts[weighted_pick_index(&weights, job)] += 1;
        }
        let ratio0 = counts[0] as f32 / n as f32;
        let ratio1 = counts[1] as f32 / n as f32;
        assert!((ratio0 - 0.7).abs() < 0.02, "got {ratio0}, expected ~0.7");
        assert!((ratio1 - 0.3).abs() < 0.02, "got {ratio1}, expected ~0.3");
    }

    #[test]
    fn three_way_split_sums_to_all_jobs() {
        let weights = [0.5_f32, 0.3, 0.2];
        let n = 6_000;
        let mut counts = [0usize; 3];
        for job in 0..n {
            counts[weighted_pick_index(&weights, job)] += 1;
        }
        assert_eq!(counts.iter().sum::<usize>(), n);
        for (i, expected) in weights.iter().enumerate() {
            let ratio = counts[i] as f32 / n as f32;
            assert!((ratio - expected).abs() < 0.02, "context {i}: got {ratio}, expected ~{expected}");
        }
    }
}
