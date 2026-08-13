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
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var t_diffuse: texture_2d<f32>;
@group(0) @binding(2) var s_diffuse: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.transform * vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    return vec4<f32>(tex.rgb * u.light.rgb, tex.a);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    transform: [[f32; 4]; 4],
    light: [f32; 4],
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
            multisample: wgpu::MultisampleState::default(),
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
        let projection = compute_projection(width, height, camera, time);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pony-frame-encoder"),
        });

        let mut parts: Vec<_> = character.parts.values().collect();
        parts.sort_by_key(|p| p.layer);

        // Bind group'ы (и лежащие за ними uniform-буферы/текстуры) держим
        // в векторе снаружи render pass'а: RenderPass заимствует их на
        // весь свой срок жизни, а pass должен быть уничтожен раньше.
        let bind_groups: Vec<wgpu::BindGroup> = parts
            .iter()
            .map(|part| {
                let world = part
                    .bone
                    .as_ref()
                    .and_then(|b| character.skeleton.world_transform(b))
                    .unwrap_or_default();
                let (_, fallback_color) = nominal_size_and_fallback_color(part.kind);
                let size = part_render_size(part);

                // Позицию части считаем общей функцией (её же зовёт GUI для
                // hit-теста и перетаскивания) — чтобы «где нарисовано» и «где
                // ловится клик» не могли разъехаться.
                let part_pos = part_world_position(part, &world);

                let depth_z = -(part.layer as f32) * DEPTH_PER_LAYER;
                let (yawed_x, foreshorten) = pony_core::apply_yaw_2_5d(part_pos.x, depth_z, character.facing_yaw);

                // Доводка pony.Look() (раздел 7 ТЗ): "прицел взгляда" — это
                // поворот глаза через морфинг (EyeParams.rotation), а не
                // всей кости головы. Применяется только к частям вида Eyes —
                // остальные части кости Head (уши, рог, морда) не должны
                // поворачиваться вслед за взглядом.
                let eye_rotation = if part.kind == PartKind::Eyes { character.default_morph.eyes.rotation } else { 0.0 };

                let model = Mat4::from_scale_rotation_translation(
                    glam::Vec3::new(world.scale.x * size.x * foreshorten, world.scale.y * size.y, 1.0),
                    glam::Quat::from_rotation_z(world.rotation + eye_rotation),
                    glam::Vec3::new(yawed_x, part_pos.y, 0.0),
                );
                // Свет считаем по итоговой (уже повёрнутой 2.5D) позиции —
                // часть, ушедшая параллаксом в сторону, освещается там, где
                // она реально оказалась на экране, а не в исходных мировых
                // координатах кости.
                let light_rgb = pony_core::lighting::shade_at(glam::Vec2::new(yawed_x, part_pos.y), lighting);
                let uniforms = Uniforms {
                    transform: (projection * model).to_cols_array_2d(),
                    light: [light_rgb[0], light_rgb[1], light_rgb[2], 1.0],
                };
                let uniform_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pony-part-uniforms"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

                let tex_view = match asset_path(part) {
                    Some(AssetPath::Png(path)) => &self.textures.get_or_load_png(ctx, path, fallback_color).view,
                    Some(AssetPath::Svg(path)) => &self.textures.get_or_load_svg(ctx, path, fallback_color).view,
                    Some(AssetPath::Psd(path, layer)) => &self.textures.get_or_load_psd(ctx, path, layer, fallback_color).view,
                    Some(AssetPath::Kra(path, layer_file)) => &self.textures.get_or_load_kra(ctx, path, layer_file, fallback_color).view,
                    None => &self.textures.get_or_create_fallback(ctx, fallback_key(part.kind), fallback_color).view,
                };

                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pony-part-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
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
                let tex_view = &self.textures.get_or_create_fallback(ctx, particle_fallback_key(emitter.kind), fallback_color).view;
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
                        let uniforms = Uniforms { transform: (projection * model).to_cols_array_2d(), light: [1.0, 1.0, 1.0, 1.0] };
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
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex_view) },
                                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
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
                    view: &view,
                    resolve_target: None,
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
        let projection = compute_projection(width, height, camera, time);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pony-particle-encoder"),
        });

        let fallback_color = emitter.kind.base_color();
        let tex_view = &self.textures.get_or_create_fallback(ctx, particle_fallback_key(emitter.kind), fallback_color).view;

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
                // нейтральный множитель [1,1,1,1].
                let uniforms = Uniforms { transform: (projection * model).to_cols_array_2d(), light: [1.0, 1.0, 1.0, 1.0] };
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
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            })
            .collect();

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pony-particle-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
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
