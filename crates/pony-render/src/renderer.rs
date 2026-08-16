//! Render pass: рисует части персонажа как текстурированные квады,
//! трансформированные world_transform их костей. Текстура части — реальный
//! импортированный PNG (раздел 16 ТЗ), либо, если ассета ещё нет, плоская
//! заливка нужного цвета — пайплайн один и тот же в обоих случаях
//! (см. `texture::TextureCache`).

use glam::Mat4;
use pony_core::character::Character;
use pony_core::part::PartKind;
use wgpu::util::DeviceExt;

use crate::texture::TextureCache;
use crate::GpuContext;

/// Условная глубина части для 2.5D-поворота (раздел 8 ТЗ) — выводим из
/// `layer`, а не храним отдельным полем: части с более высоким layer
/// рисуются поверх, что в этом движке используется и как грубая эвристика
/// "ближе к камере". Не физически точно (слой отрисовки — это про порядок
/// рисования, не про реальную глубину), но даёт согласованный, тестируемый
/// параллакс без новых полей в модели персонажа. Публичная — чтобы
/// hit-тестинг в GUI (клик мышью по частям на Stage) считал по тем же
/// координатам, что и сам рендер, а не дублировал magic number отдельно.
pub const DEPTH_PER_LAYER: f32 = 8.0;

const SHADER_SRC: &str = r#"
struct Uniforms {
    transform: mat4x4<f32>,
    // rgb — множитель освещения (см. pony_core::lighting::shade_at), a не
    // используется (оставлено для выравнивания 16 байт, стандартная практика
    // в WGSL uniform-буферах).
    light: vec4<f32>,
    // Раздел 60 ТЗ (Masks/Clipping). world_to_mask_local переводит МИРОВУЮ
    // позицию текущего фрагмента (не UV этой части — маска обычно имеет
    // другой transform, чем маскируемая часть) в локальное [-0.5..0.5]
    // пространство КВАДА части-маски — тот же трюк, что обратная матрица
    // модели для семплинга чужой геометрии в общем шейдере, без отдельного
    // прохода/стенсил-буфера. has_mask == 0.0 — маска не задана, альфа не
    // трогается (быстрый путь для абсолютного большинства частей, у
    // которых clip_by пуст — везде дальше просто умножение на 1.0, ветвления
    // на GPU нет вообще, дешевле, чем if в шейдере).
    world_to_mask_local: mat4x4<f32>,
    // has_mask живёт в .x — vec4 вместо голого f32, чтобы избежать
    // расхождения выравнивания между WGSL (std140-подобные правила:
    // скаляр после mat4x4 всё равно требует последующий vec3/vec4 на
    // границе 16 байт для ЗАВЕРШЕНИЯ структуры) и `#[repr(C)]` в Rust
    // (который padding по правилам WGSL сам не вставляет) — раньше здесь
    // была пара `has_mask: f32` + `_pad: vec3<f32>`, которая давала 160
    // байт на Rust-стороне против 176 ожидаемых WGSL (проверено РЕАЛЬНЫМ
    // headless GPU прогоном — wgpu отказал с "Buffer is bound with size
    // 160 where the shader expects 176", не гипотетическая, а поймана
    // валидацией). vec4<f32> с обеих сторон — оба поля по 16 байт, размеры
    // совпадают дословно, никакой неявной подгонки выравнивания не нужно.
    has_mask_and_pad: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var t_diffuse: texture_2d<f32>;
@group(0) @binding(2) var s_diffuse: sampler;
@group(0) @binding(3) var t_mask: texture_2d<f32>;
@group(0) @binding(4) var s_mask: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_pos: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    let world = vec4<f32>(position, 0.0, 1.0);
    out.pos = u.transform * world;
    out.uv = uv;
    out.world_pos = world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    // in.world_pos здесь — это МИРОВАЯ позиция вершины квада ЭТОЙ части
    // (см. vs_main: world до умножения на u.transform, которое включает и
    // проекцию, и model — но world_pos интерполируется ДО проекции, то есть
    // это координаты в системе, где model этой части уже "снят" делением на
    // scale/rotate этой части... фактически world_pos — координаты в
    // ЛОКАЛЬНОМ пространстве квада [-0.5..0.5] этой части, ДО её собственного
    // model-преобразования (см. vs_main: world = vec4(position,0,1), а
    // position — вершина QUAD, ещё не тронутая u.transform). Чтобы получить
    // РЕАЛЬНУЮ мировую позицию фрагмента, world_to_mask_local строится на
    // CPU уже с учётом ПОЛНОЙ цепочки: model этой части (переводит квад в
    // мир) СОСТАВЛЕНО с обратной model части-маски (переводит мир в
    // локальный квад маски) — см. renderer.rs, mask_transform_uniform().
    let mask_local = u.world_to_mask_local * in.world_pos;
    // Квад в UV: [-0.5..0.5] -> [0..1], с тем же переворотом Y, что и у
    // основной геометрии (см. QUAD: локальный +Y -> v=0).
    let mask_uv = vec2<f32>(mask_local.x + 0.5, 0.5 - mask_local.y);
    let mask_tex = textureSample(t_mask, s_mask, mask_uv);
    // Вне квада маски (mask_uv за пределами [0,1]) — альфа маски трактуется
    // как 0 (полностью прозрачно), не оборачивается/не растягивается: то,
    // что физически не попадает под фигуру маски, не должно быть видно —
    // стандартная семантика клип-маски ограниченного размера, не бесконечной.
    let in_mask_bounds = step(0.0, mask_uv.x) * step(mask_uv.x, 1.0) * step(0.0, mask_uv.y) * step(mask_uv.y, 1.0);
    let mask_alpha = mix(1.0, mask_tex.a * in_mask_bounds, u.has_mask_and_pad.x);
    return vec4<f32>(tex.rgb * u.light.rgb, tex.a * mask_alpha);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    transform: [[f32; 4]; 4],
    light: [f32; 4],
    world_to_mask_local: [[f32; 4]; 4],
    // .x = has_mask (1.0/0.0), .yzw не используются (padding до 16 байт —
    // см. подробный комментарий в SHADER_SRC про то, почему не голый f32).
    has_mask_and_pad: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

// Единичный квад (-0.5..0.5), масштабируется под номинальный размер части.
// UV: локальный +Y (верх спрайта) -> v=0 (верх текстуры) — обычное
// соответствие "верх картинки — верх части".
const QUAD: [Vertex; 6] = [
    Vertex { pos: [-0.5, -0.5], uv: [0.0, 1.0] },
    Vertex { pos: [0.5, -0.5], uv: [1.0, 1.0] },
    Vertex { pos: [0.5, 0.5], uv: [1.0, 0.0] },
    Vertex { pos: [-0.5, -0.5], uv: [0.0, 1.0] },
    Vertex { pos: [0.5, 0.5], uv: [1.0, 0.0] },
    Vertex { pos: [-0.5, 0.5], uv: [0.0, 0.0] },
];

/// Номинальный размер части и цвет-заглушка на случай, если для неё ещё
/// нет реального PNG-ассета (тогда рисуется плоская заливка этим цветом).
/// Мировая позиция части с учётом её смещения относительно кости
/// (`Part::pivot`), повёрнутого и отмасштабированного трансформом кости.
///
/// Публичная и используется И рендером, И GUI (hit-test клика, перетаскивание
/// части) — намеренно одна функция на всех: раньше GUI считал позицию своей
/// копией формулы, и стоило поменять рендер, как выделение начинало
/// промахиваться мимо того, что нарисовано.
pub fn part_world_position(part: &pony_core::part::Part, bone_world: &pony_core::skeleton::Transform2D) -> glam::Vec2 {
    let (bone_sin, bone_cos) = bone_world.rotation.sin_cos();
    let off_x = part.pivot.x * bone_world.scale.x;
    let off_y = part.pivot.y * bone_world.scale.y;
    glam::Vec2::new(
        bone_world.position.x + (off_x * bone_cos - off_y * bone_sin),
        bone_world.position.y + (off_x * bone_sin + off_y * bone_cos),
    )
}

/// Обратное к `part_world_position`: какой `pivot` нужен части, чтобы она
/// оказалась в заданной мировой точке. Нужно для перетаскивания части мышью —
/// курсор даёт мировую точку, а хранится смещение относительно кости.
pub fn pivot_for_world_position(target: glam::Vec2, bone_world: &pony_core::skeleton::Transform2D) -> glam::Vec2 {
    let (bone_sin, bone_cos) = bone_world.rotation.sin_cos();
    let dx = target.x - bone_world.position.x;
    let dy = target.y - bone_world.position.y;
    // Обратный поворот, затем обратный масштаб.
    let unrot_x = dx * bone_cos + dy * bone_sin;
    let unrot_y = -dx * bone_sin + dy * bone_cos;
    glam::Vec2::new(
        unrot_x / if bone_world.scale.x.abs() < 1e-6 { 1.0 } else { bone_world.scale.x },
        unrot_y / if bone_world.scale.y.abs() < 1e-6 { 1.0 } else { bone_world.scale.y },
    )
}

/// Итоговый размер части на сцене: явный `Part::size`, если задан, иначе
/// размер по умолчанию для её вида.
pub fn part_render_size(part: &pony_core::part::Part) -> glam::Vec2 {
    part.size.unwrap_or_else(|| nominal_size_and_fallback_color(part.kind).0)
}

/// Номинальный размер части (см. `nominal_size_and_fallback_color`) без
/// цвета-заглушки — публичная версия для того, кому нужен только размер.
pub fn nominal_part_size(kind: PartKind) -> glam::Vec2 {
    nominal_size_and_fallback_color(kind).0
}

fn nominal_size_and_fallback_color(kind: PartKind) -> (glam::Vec2, [f32; 4]) {
    match kind {
        PartKind::Body => (glam::Vec2::new(50.0, 34.0), [0.85, 0.55, 0.75, 1.0]),
        PartKind::Head => (glam::Vec2::new(24.0, 22.0), [0.85, 0.55, 0.75, 1.0]),
        PartKind::ManeFront | PartKind::ManeBack => (glam::Vec2::new(20.0, 16.0), [0.4, 0.2, 0.5, 1.0]),
        PartKind::Tail => (glam::Vec2::new(10.0, 24.0), [0.4, 0.2, 0.5, 1.0]),
        PartKind::Eyes => (glam::Vec2::new(5.0, 6.0), [0.1, 0.1, 0.15, 1.0]),
        PartKind::Mouth => (glam::Vec2::new(6.0, 2.0), [0.3, 0.1, 0.1, 1.0]),
        PartKind::Ear => (glam::Vec2::new(6.0, 8.0), [0.85, 0.55, 0.75, 1.0]),
        PartKind::Wing => (glam::Vec2::new(18.0, 12.0), [0.95, 0.95, 0.9, 1.0]),
        PartKind::Horn => (glam::Vec2::new(4.0, 10.0), [0.95, 0.9, 0.6, 1.0]),
        PartKind::LegFL | PartKind::LegFR | PartKind::LegBL | PartKind::LegBR => {
            (glam::Vec2::new(7.0, 26.0), [0.85, 0.55, 0.75, 1.0])
        }
        PartKind::Custom => (glam::Vec2::new(10.0, 10.0), [0.6, 0.6, 0.6, 1.0]),
    }
}

/// Модельная (object-to-world, БЕЗ проекции) матрица части — квад
/// [-0.5..0.5] переводится в её итоговое положение/поворот/масштаб на
/// сцене, с учётом кости (`world_transform_with_ik`), pivot, 2.5D-параллакса
/// от `facing_yaw` и поворота глаза (pony.Look()). Вынесена из
/// `render_character` в отдельную функцию — раздел 60 ТЗ (Masks/Clipping)
/// требует знать ПОЛНУЮ model-матрицу части, которая используется как
/// маска, до того как дошла очередь рендерить саму маскируемую часть
/// (порядок обхода `parts` — по `layer`, не гарантирует такой порядок) —
/// теперь у обоих потребителей (обычный рендер части и её использование
/// как маски для другой части) одна и та же формула, а не две
/// потенциально расходящиеся копии.
fn part_model_matrix(part: &pony_core::part::Part, character: &Character) -> Mat4 {
    // `world_transform_with_ik` (не голый `world_transform`) — раздел 41
    // ТЗ: IK-констрейнты должны реально влиять на рендер. Без активных
    // констрейнтов это тот же путь и та же цена, что и раньше.
    let world = part.bone.as_ref().and_then(|b| character.skeleton.world_transform_with_ik(b)).unwrap_or_default();
    let size = part_render_size(part);
    let part_pos = part_world_position(part, &world);

    let depth_z = -(part.layer as f32) * DEPTH_PER_LAYER;
    let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);

    // Доводка pony.Look() (раздел 7 ТЗ): поворот глаза через морфинг, не
    // всей кости головы — применяется только к частям вида Eyes.
    let eye_rotation = if part.kind == PartKind::Eyes { character.default_morph.eyes.rotation } else { 0.0 };

    Mat4::from_scale_rotation_translation(
        glam::Vec3::new(world.scale.x * size.x * foreshorten, world.scale.y * size.y, 1.0),
        glam::Quat::from_rotation_z(world.rotation + eye_rotation),
        glam::Vec3::new(yawed_x, part_pos.y, 0.0),
    )
}

/// Какой загрузчик текстуры использовать для источника части.
/// Mesh пока не поддержан как источник текстуры — для него, как и для
/// отсутствующего файла, используется цветная заглушка.
enum AssetPath<'a> {
    Png(&'a str),
    Svg(&'a str),
    Psd(&'a str, Option<&'a str>),
    Kra(&'a str, Option<&'a str>),
}

fn asset_path(part: &pony_core::part::Part) -> Option<AssetPath<'_>> {
    match &part.source {
        pony_core::part::PartSource::Png { path } => Some(AssetPath::Png(path.as_str())),
        pony_core::part::PartSource::Vector { path } => Some(AssetPath::Svg(path.as_str())),
        pony_core::part::PartSource::Psd { path, layer } => Some(AssetPath::Psd(path.as_str(), layer.as_deref())),
        pony_core::part::PartSource::Kra { path, layer_file } => Some(AssetPath::Kra(path.as_str(), layer_file.as_deref())),
        pony_core::part::PartSource::Mesh { .. } => None,
    }
}

pub struct FrameOutput {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub rendered_on: String,
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    textures: TextureCache,
}

impl Renderer {
    /// Использует бюджет памяти по умолчанию (`texture::DEFAULT_BUDGET_BYTES`)
    /// — для настоящего бюджета из `pony_system::WorkloadPolicy` (посчитан по
    /// фактической памяти системы, не захардкожен) используй `new_with_budget`.
    pub fn new(ctx: &GpuContext) -> Self {
        Self::new_with_budget(ctx, crate::texture::DEFAULT_BUDGET_BYTES)
    }

    /// То же самое, но с явным бюджетом памяти для `TextureCache` (в байтах)
    /// — при превышении наименее недавно использованные текстуры вытесняются
    /// (см. `pony_render::LruBudget`). Обычно сюда передают
    /// `WorkloadPolicy::memory_budget_bytes`, посчитанный из реальной
    /// доступной памяти машины, а не произвольную константу.
    pub fn new_with_budget(ctx: &GpuContext, texture_budget_bytes: u64) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pony-part-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let bind_group_layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pony-part-bind-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // VERTEX для transform, FRAGMENT для light — оба поля
                    // теперь в одном uniform-буфере (см. SHADER_SRC).
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Раздел 60 ТЗ (Masks/Clipping) — вторая текстура/сэмплер
                // для клип-маски. Присутствуют в layout ВСЕГДА (не опционально
                // по binding count) — bind group должен точно соответствовать
                // layout; части без маски получают заглушку 1x1 непрозрачный
                // белый пиксель (см. `mask_fallback_view` ниже), шейдер же
                // просто умножает на 1.0 через `has_mask == 0.0`.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pony-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pony-part-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x2 },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 2]>() as u64,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            // 4x MSAA — без этого края частей (особенно повёрнутых,
            // не выровненных по осям — колышущийся хвост, наклонённая
            // голова) рисуются "лесенкой", по одному ровному пикселю на
            // край, и именно это выглядит как "пиксельная графика", а не
            // сам факт растрового рендера. Требует рендерить в отдельную
            // мультисемпл-текстуру и резолвить её в обычную (см.
            // MSAA_SAMPLES и create_msaa_texture ниже) — цена: одна лишняя
            // текстура на кадр, для сцены в пару сотен пикселей это дёшево.
            multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
        });

        let vertex_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pony-quad-vertices"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pony-part-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            pipeline,
            bind_group_layout,
            vertex_buffer,
            sampler,
            textures: TextureCache::with_budget(texture_budget_bytes),
        }
    }

    /// Отрисовать персонажа в offscreen-текстуру заданного размера и
    /// вернуть сырые пиксели. `&mut self` — потому что рендер лениво
    /// догружает и кэширует текстуры частей при первом обращении.
    /// `camera`/`time` — `time` — любой монотонно растущий счётчик секунд,
    /// используется только для детерминированного дрожания от тряски
    /// (`Camera::shake_offset`); он не должен совпадать с `AnimationPlayer`'s
    /// временем персонажа (это разные независимые часы — камера может
    /// трястись, даже когда анимация персонажа на паузе).
    pub fn render_character(
        &mut self,
        ctx: &GpuContext,
        character: &Character,
        width: u32,
        height: u32,
        camera: &pony_core::Camera,
        time: f32,
        lighting: &pony_core::Lighting,
        particles: Option<&pony_core::ParticleEmitter>,
    ) -> FrameOutput {
        let texture = create_frame_texture(ctx, width, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa_texture = create_msaa_texture(ctx, width, height);
        let msaa_view = msaa_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let projection = compute_projection(width, height, camera, time);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pony-frame-encoder"),
        });

        let mut parts: Vec<_> = character.parts.values().collect();
        parts.sort_by_key(|p| p.layer);

        // Раздел 60 ТЗ (Masks/Clipping): модельная матрица КАЖДОЙ части
        // считается заранее, отдельным проходом — маскируемая часть должна
        // знать полную model-матрицу своей части-маски (см. `resolve_clip_
        // mask`), а порядок обхода `parts` (по layer) не гарантирует, что
        // маска обработана раньше того, что она маскирует. Тот же расчёт
        // model, что и раньше — вынесен в отдельную функцию `part_model_
        // matrix`, чтобы не дублировать формулу между этим проходом и
        // основным циклом бинд-групп ниже.
        let models: std::collections::HashMap<&str, Mat4> =
            parts.iter().map(|part| (part.id.as_str(), part_model_matrix(part, character))).collect();

        // Заглушка маски для частей без `clip_by` — сплошной непрозрачный
        // белый пиксель 1x1: в шейдере `has_mask == 0.0` для них всё равно
        // не даёт этой текстуре повлиять на результат (см. SHADER_SRC), но
        // валидный bind group ОБЯЗАН заполнить все binding'и layout'а — не
        // может быть "пустого" слота.
        let mask_fallback_view = self.textures.get_or_create_fallback(ctx, "mask_none_fallback", [1.0, 1.0, 1.0, 1.0]).view.clone();

        // Bind group'ы (и лежащие за ними uniform-буферы/текстуры) держим
        // в векторе снаружи render pass'а: RenderPass заимствует их на
        // весь срок жизни, а pass должен быть уничтожен раньше.
        let bind_groups: Vec<wgpu::BindGroup> = parts
            .iter()
            .map(|part| {
                let model = models[part.id.as_str()];
                let (_, fallback_color) = nominal_size_and_fallback_color(part.kind);

                // Свет считаем по итоговой мировой позиции части (уже с
                // 2.5D-параллаксом) — берём напрямую из уже посчитанной
                // model-матрицы (её последний столбец — позиция), не
                // пересчитываем world/part_pos второй раз.
                let world_pos_from_model = glam::Vec2::new(model.w_axis.x, model.w_axis.y);
                let light_rgb = pony_core::lighting::shade_at(world_pos_from_model, lighting);

                // Часть-маска для ЭТОЙ части (если задана и безопасна — см.
                // `Character::resolve_clip_mask`, отсекает самоссылки и
                // цепочки масок глубже одного уровня). world_to_mask_local
                // переводит мировую позицию фрагмента (полученную из
                // ЛОКАЛЬНЫХ координат квада this части через model this
                // части — см. комментарий в SHADER_SRC про world_pos) в
                // локальные координаты квада МАСКИ: composed = model_this
                // (квад -> мир), затем inverse(model_mask) (мир -> квад маски).
                let (has_mask, world_to_mask_local, mask_tex_view) = match character.resolve_clip_mask(&part.id) {
                    Some(mask_part) => {
                        let mask_model = models[mask_part.id.as_str()];
                        let world_to_mask_local = mask_model.inverse() * model;
                        let (_, mask_fallback_color) = nominal_size_and_fallback_color(mask_part.kind);
                        let view = match asset_path(mask_part) {
                            Some(AssetPath::Png(path)) => self.textures.get_or_load_png(ctx, path, mask_fallback_color).view.clone(),
                            Some(AssetPath::Svg(path)) => self.textures.get_or_load_svg(ctx, path, mask_fallback_color).view.clone(),
                            Some(AssetPath::Psd(path, layer)) => self.textures.get_or_load_psd(ctx, path, layer, mask_fallback_color).view.clone(),
                            Some(AssetPath::Kra(path, layer_file)) => self.textures.get_or_load_kra(ctx, path, layer_file, mask_fallback_color).view.clone(),
                            None => self.textures.get_or_create_fallback(ctx, fallback_key(mask_part.kind), mask_fallback_color).view.clone(),
                        };
                        (1.0f32, world_to_mask_local, view)
                    }
                    None => (0.0f32, Mat4::IDENTITY, mask_fallback_view.clone()),
                };

                let uniforms = Uniforms {
                    transform: (projection * model).to_cols_array_2d(),
                    light: [light_rgb[0], light_rgb[1], light_rgb[2], 1.0],
                    world_to_mask_local: world_to_mask_local.to_cols_array_2d(),
                    has_mask_and_pad: [has_mask, 0.0, 0.0, 0.0],
                };
                let uniform_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pony-part-uniforms"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

                let tex_view = match asset_path(part) {
                    Some(AssetPath::Png(path)) => self.textures.get_or_load_png(ctx, path, fallback_color).view.clone(),
                    Some(AssetPath::Svg(path)) => self.textures.get_or_load_svg(ctx, path, fallback_color).view.clone(),
                    Some(AssetPath::Psd(path, layer)) => self.textures.get_or_load_psd(ctx, path, layer, fallback_color).view.clone(),
                    Some(AssetPath::Kra(path, layer_file)) => self.textures.get_or_load_kra(ctx, path, layer_file, fallback_color).view.clone(),
                    None => self.textures.get_or_create_fallback(ctx, fallback_key(part.kind), fallback_color).view.clone(),
                };

                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pony-part-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&mask_tex_view) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            })
            .collect();

        // Частицы (раздел 13 ТЗ), если передан эмиттер, рисуются в ТОМ ЖЕ
        // проходе поверх частей персонажа — не отдельным кадром, как в
        // headless render_particles ниже (та версия существует отдельно для
        // headless-демо в pony-cli, где сцена — только частицы). Здесь их
        // нужно реально скомпоновать с персонажем (например, снег поверх
        // пони), поэтому используем общую текстуру кадра и общий pass.
        let particle_bind_groups: Vec<wgpu::BindGroup> = match particles {
            Some(emitter) if !emitter.particles.is_empty() => {
                let fallback_color = emitter.kind.base_color();
                let tex_view = self.textures.get_or_create_fallback(ctx, particle_fallback_key(emitter.kind), fallback_color).view.clone();
                // Частицы не поддерживают маски (раздел 60 — masks относятся
                // к частям персонажа, не к частицам) — тот же fallback
                // "непрозрачный белый", что и у частей без `clip_by`,
                // держит layout/pipeline общими без ветвления.
                let particle_mask_view = mask_fallback_view.clone();
                emitter
                    .particles
                    .iter()
                    .map(|p| {
                        let size = p.current_size();
                        let model = Mat4::from_scale_rotation_translation(
                            glam::Vec3::new(size, size, 1.0),
                            glam::Quat::IDENTITY,
                            glam::Vec3::new(p.position.x, p.position.y, 0.0),
                        );
                        // Частицы не затемняются освещением сцены — см. то же
                        // решение и обоснование в headless render_particles.
                        let uniforms = Uniforms {
                            transform: (projection * model).to_cols_array_2d(),
                            light: [1.0, 1.0, 1.0, 1.0],
                            world_to_mask_local: Mat4::IDENTITY.to_cols_array_2d(),
                            has_mask_and_pad: [0.0, 0.0, 0.0, 0.0],
                        };
                        let uniform_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("pony-particle-uniforms"),
                            contents: bytemuck::bytes_of(&uniforms),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });
                        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("pony-particle-bind-group"),
                            layout: &self.bind_group_layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex_view) },
                                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&particle_mask_view) },
                                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            ],
                        })
                    })
                    .collect()
            }
            _ => Vec::new(),
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pony-frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    // Рисуем в мультисемпл-текстуру, GPU сам сводит
                    // MSAA_SAMPLES сэмплов в `view` (обычную, читаемую
                    // обратно на CPU) через resolve_target при завершении
                    // прохода — не нужно резолвить вручную.
                    view: &msaa_view,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.53, g: 0.81, b: 0.92, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            for bind_group in &bind_groups {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..QUAD.len() as u32, 0..1);
            }
            for bind_group in &particle_bind_groups {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..QUAD.len() as u32, 0..1);
            }
        }

        ctx.queue.submit(Some(encoder.finish()));
        let rgba = read_back_rgba(ctx, &texture, width, height);
        FrameOutput { width, height, rgba, rendered_on: ctx.info.name.clone() }
    }

    /// Отрисовать частицы эмиттера (раздел 13 ТЗ) в свою offscreen-текстуру.
    /// Переиспользует тот же пайплайн/шейдер, что и `render_character` —
    /// частица рисуется как цветной квад (текстура-заглушка нужного цвета
    /// из `TextureCache`, см. модуль `texture`), сжимающийся до нуля к концу
    /// жизни (`Particle::size_factor`) вместо альфа-затухания — см.
    /// `pony_core::particles` за объяснением этого упрощения.
    /// Сколько байт видеопамяти сейчас занято закэшированными текстурами
    /// частей — для диагностики/демо бюджета памяти (раздел про
    /// `WorkloadPolicy::memory_budget_bytes` в README).
    pub fn texture_memory_used_bytes(&self) -> u64 {
        self.textures.memory_used_bytes()
    }

    pub fn texture_memory_budget_bytes(&self) -> u64 {
        self.textures.memory_budget_bytes()
    }

    /// См. `TextureCache::invalidate` — нужен после перезаписи ассета на
    /// диске (например, повторного сохранения отредактированного SVG),
    /// иначе персонаж продолжит рисоваться со старой текстурой из кэша.
    pub fn invalidate_texture(&mut self, key: &str) {
        self.textures.invalidate(key);
    }

    pub fn render_particles(
        &mut self,
        ctx: &GpuContext,
        emitter: &pony_core::ParticleEmitter,
        width: u32,
        height: u32,
        camera: &pony_core::Camera,
        time: f32,
    ) -> FrameOutput {
        let texture = create_frame_texture(ctx, width, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa_texture = create_msaa_texture(ctx, width, height);
        let msaa_view = msaa_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let projection = compute_projection(width, height, camera, time);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pony-particle-encoder"),
        });

        let fallback_color = emitter.kind.base_color();
        let tex_view = self.textures.get_or_create_fallback(ctx, particle_fallback_key(emitter.kind), fallback_color).view.clone();
        let mask_fallback_view = self.textures.get_or_create_fallback(ctx, "mask_none_fallback", [1.0, 1.0, 1.0, 1.0]).view.clone();

        let bind_groups: Vec<wgpu::BindGroup> = emitter
            .particles
            .iter()
            .map(|p| {
                let size = p.current_size();
                let model = Mat4::from_scale_rotation_translation(
                    glam::Vec3::new(size, size, 1.0),
                    glam::Quat::IDENTITY,
                    glam::Vec3::new(p.position.x, p.position.y, 0.0),
                );
                // Частицы не затемняются освещением сцены — они сами
                // источники света (искры, магия) или слишком мелкие/быстрые,
                // чтобы правдоподобно считать per-particle затенение —
                // нейтральный множитель [1,1,1,1]. Маска не поддержана для
                // частиц (см. то же решение в render_character выше).
                let uniforms = Uniforms {
                    transform: (projection * model).to_cols_array_2d(),
                    light: [1.0, 1.0, 1.0, 1.0],
                    world_to_mask_local: Mat4::IDENTITY.to_cols_array_2d(),
                    has_mask_and_pad: [0.0, 0.0, 0.0, 0.0],
                };
                let uniform_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pony-particle-uniforms"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pony-particle-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&mask_fallback_view) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            })
            .collect();

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pony-particle-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        // Тёмный нейтральный фон (не небо render_character) —
                        // отдельная сцена только для частиц, чтобы их было
                        // легко отличить/проверить на скриншоте.
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.08, g: 0.08, b: 0.1, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            for bind_group in &bind_groups {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..QUAD.len() as u32, 0..1);
            }
        }

        ctx.queue.submit(Some(encoder.finish()));
        let rgba = read_back_rgba(ctx, &texture, width, height);
        FrameOutput { width, height, rgba, rendered_on: ctx.info.name.clone() }
    }
}

/// Число сэмплов MSAA. 4 — стандартный практичный выбор (заметно
/// сглаживает края, не 8x/16x-цена по памяти и производительности,
/// которая для сцены в пару сотен пикселей всё равно избыточна).
const MSAA_SAMPLES: u32 = 4;

fn create_frame_texture(ctx: &GpuContext, width: u32, height: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pony-frame-target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Мультисемпл-текстура для рендера (не читается назад напрямую — только
/// как источник для `resolve_target` в `RenderPassColorAttachment`, куда
/// GPU сам сводит `MSAA_SAMPLES` сэмплов в обычный пиксель). Не может
/// иметь `COPY_SRC`/`TEXTURE_BINDING` — мультисемпл-текстуры так не
/// читаются, отсюда и resolve в отдельную обычную текстуру.
fn create_msaa_texture(ctx: &GpuContext, width: u32, height: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pony-frame-target-msaa"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: MSAA_SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// Камера: сначала сдвигаем мир на -(position + тряска) (камера "смотрит
/// туда, куда мы её подвинули"), затем крутим на -rotation, затем
/// масштабируем на zoom (больший zoom -> позиции дальше от центра кадра ->
/// меньше мира помещается в кадр -> ощущение приближения), и только потом —
/// обычная ортографическая проекция. Порядок важен: `scale * rotate * translate`.
fn compute_projection(width: u32, height: u32, camera: &pony_core::Camera, time: f32) -> Mat4 {
    let half_w = width as f32 / 2.0;
    let half_h = height as f32 / 2.0;
    let base_projection = glam::camera::rh::proj::directx::orthographic(-half_w, half_w, -half_h, half_h, -1.0, 1.0);

    let shake = camera.shake_offset(time);
    let translate = Mat4::from_translation(glam::Vec3::new(-(camera.position.x + shake.x), -(camera.position.y + shake.y), 0.0));
    let rotate = Mat4::from_rotation_z(-camera.rotation);
    let zoom = camera.zoom.max(0.0001);
    let scale = Mat4::from_scale(glam::Vec3::new(zoom, zoom, 1.0));
    base_projection * scale * rotate * translate
}

fn read_back_rgba(ctx: &GpuContext, texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> {
    let unpadded_bytes_per_row = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = (unpadded_bytes_per_row + align - 1) / align * align;

    let output_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pony-frame-readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pony-readback-encoder") });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer {
            buffer: &output_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(encoder.finish()));

    let slice = output_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().expect("failed to map readback buffer");

    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height as usize {
        let start = row * padded_bytes_per_row as usize;
        let end = start + unpadded_bytes_per_row as usize;
        rgba.extend_from_slice(&data[start..end]);
    }
    drop(data);
    output_buffer.unmap();
    rgba
}

/// Ключ в кэше текстур для заглушек без реального ассета — один общий
/// 1x1 на каждый вид части (а не на каждую часть отдельно), чтобы, скажем,
/// два уха без PNG делили одну и ту же текстуру-заглушку.
fn fallback_key(kind: PartKind) -> &'static str {
    match kind {
        PartKind::Body => "__fallback_body",
        PartKind::Head => "__fallback_head",
        PartKind::ManeFront => "__fallback_mane_front",
        PartKind::ManeBack => "__fallback_mane_back",
        PartKind::Tail => "__fallback_tail",
        PartKind::Eyes => "__fallback_eyes",
        PartKind::Mouth => "__fallback_mouth",
        PartKind::Ear => "__fallback_ear",
        PartKind::Wing => "__fallback_wing",
        PartKind::Horn => "__fallback_horn",
        PartKind::LegFL => "__fallback_leg_fl",
        PartKind::LegFR => "__fallback_leg_fr",
        PartKind::LegBL => "__fallback_leg_bl",
        PartKind::LegBR => "__fallback_leg_br",
        PartKind::Custom => "__fallback_custom",
    }
}

/// Аналог `fallback_key`, но для частиц — один цвет-заглушка на вид
/// частицы (Dust/Snow/Rain/...), не на каждую частицу отдельно.
fn particle_fallback_key(kind: pony_core::ParticleKind) -> &'static str {
    match kind {
        pony_core::ParticleKind::Dust => "__particle_dust",
        pony_core::ParticleKind::Snow => "__particle_snow",
        pony_core::ParticleKind::Rain => "__particle_rain",
        pony_core::ParticleKind::Spark => "__particle_spark",
        pony_core::ParticleKind::Magic => "__particle_magic",
        pony_core::ParticleKind::Smoke => "__particle_smoke",
        pony_core::ParticleKind::Cloud => "__particle_cloud",
    }
}

#[cfg(test)]
mod part_placement_tests {
    use super::*;
    use pony_core::part::{Part, PartKind, PartSource};
    use pony_core::skeleton::Transform2D;

    fn test_part(pivot: glam::Vec2) -> Part {
        Part::new("p", PartKind::Custom, PartSource::Png { path: String::new() }).with_pivot(pivot)
    }

    #[test]
    fn zero_pivot_puts_the_part_exactly_on_its_bone() {
        let bone = Transform2D { position: glam::Vec2::new(10.0, -5.0), rotation: 0.7, scale: glam::Vec2::new(2.0, 2.0) };
        let pos = part_world_position(&test_part(glam::Vec2::ZERO), &bone);
        assert!((pos - bone.position).length() < 1e-5, "без смещения часть должна быть ровно на кости, got {pos:?}");
    }

    #[test]
    fn pivot_offsets_the_part_from_its_bone() {
        let bone = Transform2D { position: glam::Vec2::ZERO, rotation: 0.0, scale: glam::Vec2::ONE };
        let pos = part_world_position(&test_part(glam::Vec2::new(30.0, 12.0)), &bone);
        assert!((pos - glam::Vec2::new(30.0, 12.0)).length() < 1e-5, "got {pos:?}");
    }

    #[test]
    fn pivot_rotates_with_the_bone() {
        // Кость повёрнута на 90°: смещение (10,0) должно уехать в (0,10).
        let bone = Transform2D {
            position: glam::Vec2::ZERO,
            rotation: std::f32::consts::FRAC_PI_2,
            scale: glam::Vec2::ONE,
        };
        let pos = part_world_position(&test_part(glam::Vec2::new(10.0, 0.0)), &bone);
        assert!(pos.x.abs() < 1e-4 && (pos.y - 10.0).abs() < 1e-4, "смещение должно повернуться вместе с костью, got {pos:?}");
    }

    #[test]
    fn pivot_scales_with_the_bone() {
        let bone = Transform2D { position: glam::Vec2::ZERO, rotation: 0.0, scale: glam::Vec2::new(3.0, 1.0) };
        let pos = part_world_position(&test_part(glam::Vec2::new(10.0, 10.0)), &bone);
        assert!((pos.x - 30.0).abs() < 1e-4 && (pos.y - 10.0).abs() < 1e-4, "got {pos:?}");
    }

    #[test]
    fn pivot_for_world_position_is_the_exact_inverse() {
        // Ключевое свойство для перетаскивания мышью: положили часть в точку
        // под курсором -> она обязана оказаться ровно там, при любом
        // повороте/масштабе кости.
        for (rot, sx, sy) in [(0.0f32, 1.0f32, 1.0f32), (0.9, 2.0, 0.5), (-2.3, 1.5, 3.0)] {
            let bone = Transform2D {
                position: glam::Vec2::new(-7.0, 4.0),
                rotation: rot,
                scale: glam::Vec2::new(sx, sy),
            };
            let target = glam::Vec2::new(42.0, -13.0);
            let pivot = pivot_for_world_position(target, &bone);
            let back = part_world_position(&test_part(pivot), &bone);
            assert!((back - target).length() < 1e-3, "rot={rot} scale=({sx},{sy}): ожидали {target:?}, получили {back:?}");
        }
    }

    #[test]
    fn explicit_size_overrides_the_kind_default() {
        let mut part = test_part(glam::Vec2::ZERO);
        let default_size = part_render_size(&part);
        assert_eq!(default_size, nominal_part_size(PartKind::Custom));
        part.size = Some(glam::Vec2::new(123.0, 45.0));
        assert_eq!(part_render_size(&part), glam::Vec2::new(123.0, 45.0));
    }
}

/// Раздел 60 ТЗ (Masks/Clipping) — сквозная проверка на РЕАЛЬНОМ GPU: не
/// просто "шейдер компилируется", а честный рендер двух кадров и сравнение
/// конкретных пикселей. Требует настоящий графический адаптер (даже
/// программный, например llvmpipe, — этого достаточно), поэтому каждый тест
/// сам проверяет доступность адаптера в начале и корректно завершается
/// (не падает, не паникует), если адаптера нет — это НЕ заглушка теста под
/// видом рабочей проверки: собственно масштабирование/сравнение пикселей
/// внутри выполняется по-настоящему, просто окружения без GPU (например,
/// некоторые CI-контейнеры) не должны ломать `cargo test` из-за отсутствия
/// оборудования, а не из-за бага в самой маскировке.
#[cfg(test)]
mod mask_gpu_tests {
    use super::*;
    use pony_core::part::{Part, PartKind, PartSource};
    use pony_core::skeleton::{Bone, Transform2D};
    use pony_core::{Camera, Character, Lighting};

    /// Пытается поднять реальный GPU-контекст (см. `GpuContext` в lib.rs).
    /// `None`, если в этом окружении вообще нет подходящего адаптера —
    /// единственная причина, по которой тест ниже пропускается, а не падает.
    fn try_make_gpu_context() -> Option<crate::GpuContext> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await?;
            let raw_info = adapter.get_info();
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await.ok()?;
            Some(crate::GpuContext {
                info: pony_system::gpu::GpuAdapterInfo {
                    index: 0,
                    name: raw_info.name.clone(),
                    backend: format!("{:?}", raw_info.backend),
                    device_type: pony_system::gpu::GpuDeviceType::Other,
                    vendor: raw_info.vendor,
                    device: raw_info.device,
                },
                weight: 1.0,
                device,
                queue,
            })
        })
    }

    fn pixel_at(frame: &crate::FrameOutput, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y * frame.width + x) * 4) as usize;
        [frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]]
    }

    #[test]
    fn clip_by_hides_the_part_outside_the_mask_shape() {
        let Some(ctx) = try_make_gpu_context() else {
            eprintln!("clip_by_hides_the_part_outside_the_mask_shape: нет GPU-адаптера в этом окружении — тест пропущен (не провален)");
            return;
        };
        let mut renderer = Renderer::new(&ctx);

        // content — часть-фон во весь номинальный размер (Body), mask_shape —
        // часть-маска ровно вдвое ýже content и сдвинутая влево так, чтобы
        // покрывать только его левую половину. Обе висят на одной корневой
        // кости с identity-трансформом, так что экранные координаты
        // предсказуемы из номинальных размеров без лишней арифметики.
        let mut character = Character::new("MaskGpuTest");
        character.skeleton.add_bone(Bone {
            id: "Root".into(),
            parent: None,
            local_transform: Transform2D { position: glam::Vec2::ZERO, rotation: 0.0, scale: glam::Vec2::ONE },
            length: 1.0,
        });
        character.add_part(Part::new("content", PartKind::Body, PartSource::Png { path: "__test_content.png".into() }).with_bone("Root"));
        let mut mask_part = Part::new("mask_shape", PartKind::Custom, PartSource::Png { path: "__test_mask.png".into() }).with_bone("Root");
        mask_part.size = Some(glam::Vec2::new(25.0, 34.0));
        mask_part.pivot = glam::Vec2::new(-12.5, 0.0);
        character.add_part(mask_part);
        character.parts.get_mut("content").unwrap().clip_by = Some("mask_shape".to_string());

        let camera = Camera::default();
        let lighting = Lighting::default();
        let (width, height) = (200u32, 150u32);
        let masked = renderer.render_character(&ctx, &character, width, height, &camera, 0.0, &lighting, None);

        let mut unmasked_character = character.clone();
        unmasked_character.parts.get_mut("content").unwrap().clip_by = None;
        unmasked_character.parts.remove("mask_shape");
        let unmasked = renderer.render_character(&ctx, &unmasked_character, width, height, &camera, 0.0, &lighting, None);

        // content — 50 единиц шириной, центрирован в мировых (0,0), кадр
        // 200px шириной при zoom 1:1 -> квад на экране занимает x=[75,125].
        // mask_shape покрывает его левую половину, x=[75,100]. x=85 — внутри
        // квада И внутри маски (должно остаться видно), x=115 — внутри
        // квада, но вне маски (должно быть обрезано).
        let cy = height / 2;
        let (left_x, right_x) = (85u32, 115u32);
        let bg = pixel_at(&masked, 2, 2);

        let left_masked = pixel_at(&masked, left_x, cy);
        let right_masked = pixel_at(&masked, right_x, cy);
        let right_unmasked = pixel_at(&unmasked, right_x, cy);

        assert!(left_masked[3] > 200 && left_masked != bg, "левая половина content должна остаться видимой под маской, got {left_masked:?}");
        assert!(right_masked[3] < 50 || right_masked == bg, "правая половина content должна быть обрезана маской, got {right_masked:?}");
        assert!(
            right_unmasked[3] > 200 && right_unmasked != bg,
            "без маски та же точка справа обязана быть видна — иначе тест ничего не доказывает про именно маску, got {right_unmasked:?}"
        );
    }

    #[test]
    fn clip_by_pointing_at_unknown_part_id_renders_unclipped() {
        // `Character::resolve_clip_mask` уже проверяет это на уровне данных
        // (см. character.rs), но здесь — сквозная проверка, что рендер
        // реально ведёт себя так же: битая/устаревшая ссылка на маску не
        // должна ронять кадр и не должна ничего скрывать (deleted mask part
        // не должен внезапно сделать весь объект невидимым).
        let Some(ctx) = try_make_gpu_context() else {
            eprintln!("clip_by_pointing_at_unknown_part_id_renders_unclipped: нет GPU-адаптера — тест пропущен");
            return;
        };
        let mut renderer = Renderer::new(&ctx);

        let mut character = Character::new("MaskDanglingTest");
        character.skeleton.add_bone(Bone {
            id: "Root".into(),
            parent: None,
            local_transform: Transform2D { position: glam::Vec2::ZERO, rotation: 0.0, scale: glam::Vec2::ONE },
            length: 1.0,
        });
        let mut part = Part::new("content", PartKind::Body, PartSource::Png { path: "__test_content.png".into() }).with_bone("Root");
        part.clip_by = Some("does_not_exist".into());
        character.add_part(part);

        let camera = Camera::default();
        let lighting = Lighting::default();
        let (width, height) = (200u32, 150u32);
        let frame = renderer.render_character(&ctx, &character, width, height, &camera, 0.0, &lighting, None);

        let bg = pixel_at(&frame, 2, 2);
        let center = pixel_at(&frame, width / 2, height / 2);
        assert!(center[3] > 200 && center != bg, "битая ссылка clip_by не должна скрывать часть, got {center:?}");
    }
}
