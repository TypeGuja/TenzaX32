//! Импорт текстур (раздел 16 ТЗ, PNG-часть). SVG/PSD/KRA/GLTF/OBJ — пока
//! нет, это следующий шаг (SVG потребует растеризатор вроде `resvg`).
//!
//! Ключевая идея: пайплайн рендера ВСЕГДА сэмплит текстуру — никогда не
//! ветвится в шейдере на "текстура есть / текстуры нет". Если реального
//! PNG-файла нет или он не читается, подставляется процедурная заливка
//! 1x1 нужным цветом (см. `solid_color_texture`) — тот же путь рендера,
//! просто с других данных. Так части без готового арта остаются видимыми
//! и на своих местах, а не пропадают/не роняют кадр.

use std::collections::HashMap;

use crate::GpuContext;

pub struct LoadedTexture {
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum TextureLoadError {
    #[error("failed to decode image '{path}': {source}")]
    Decode {
        path: String,
        #[source]
        source: image::ImageError,
    },
    #[error("failed to read SVG '{path}': {source}")]
    SvgIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse SVG '{path}': {source}")]
    SvgParse {
        path: String,
        #[source]
        source: usvg::Error,
    },
    #[error("SVG '{path}' has zero/invalid size, can't rasterize")]
    SvgInvalidSize { path: String },
}

/// Загрузить PNG (или любой формат, который поддерживает decoder `image`
/// при включённых фичах — сейчас только `png`) в GPU-текстуру.
pub fn load_png(ctx: &GpuContext, path: &str) -> Result<LoadedTexture, TextureLoadError> {
    let rgba = image::open(path)
        .map_err(|source| TextureLoadError::Decode { path: path.to_string(), source })?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(upload_rgba(ctx, Some(path), &rgba, width, height))
}

/// Во сколько раз растеризовать SVG плотнее его собственного viewBox —
/// SVG обычно описывается в мелких "логических" единицах (100x100 и т.п.),
/// растеризация 1:1 дала бы мыльную текстуру на реальном размере части.
const SVG_UPSCALE: f32 = 4.0;
/// Не даём разъехаться в гигантскую текстуру на SVG с большим viewBox.
const SVG_MAX_DIM: u32 = 1024;

/// Растеризовать SVG (raster-once, дальше — обычная текстура) через
/// resvg/usvg/tiny-skia. Каждая часть — отдельный `.svg` файл; текст,
/// внешние шрифты и анимации SVG не поддерживаются (не нужны для
/// плоских частей персонажа).
pub fn load_svg(ctx: &GpuContext, path: &str) -> Result<LoadedTexture, TextureLoadError> {
    let data = std::fs::read(path).map_err(|source| TextureLoadError::SvgIo { path: path.to_string(), source })?;
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(&data, &opt)
        .map_err(|source| TextureLoadError::SvgParse { path: path.to_string(), source })?;

    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err(TextureLoadError::SvgInvalidSize { path: path.to_string() });
    }

    let width = ((size.width() * SVG_UPSCALE).ceil() as u32).clamp(1, SVG_MAX_DIM);
    let height = ((size.height() * SVG_UPSCALE).ceil() as u32).clamp(1, SVG_MAX_DIM);
    let scale_x = width as f32 / size.width();
    let scale_y = height as f32 / size.height();

    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| TextureLoadError::SvgInvalidSize { path: path.to_string() })?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale_x, scale_y), &mut pixmap.as_mut());

    // tiny-skia хранит цвет с предумноженной альфой (premultiplied) — наш
    // формат текстуры и блендинг ожидают обычную (straight) альфу, иначе
    // полупрозрачные края будут отрисованы темнее, чем нужно.
    let mut rgba = pixmap.data().to_vec();
    unpremultiply(&mut rgba);

    Ok(upload_rgba(ctx, Some(path), &rgba, width, height))
}

fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 || a == 255 {
            continue; // полностью прозрачный/непрозрачный — делить не на что/незачем
        }
        let a_f = a as f32;
        for c in 0..3 {
            px[c] = ((px[c] as f32) * 255.0 / a_f).round().min(255.0) as u8;
        }
    }
}

/// Процедурная заливка 1x1 — используется, когда реального ассета нет.
/// Не паникует, не пропускает часть — просто рисует её плоским цветом.
pub fn solid_color_texture(ctx: &GpuContext, rgba: [u8; 4]) -> LoadedTexture {
    upload_rgba(ctx, Some("solid-color-fallback"), &rgba, 1, 1)
}

fn upload_rgba(ctx: &GpuContext, label: Option<&str>, data: &[u8], width: u32, height: u32) -> LoadedTexture {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    LoadedTexture { view, width, height }
}

/// Кэш загруженных текстур по пути к файлу — части, ссылающиеся на один
/// и тот же ассет (например, оба уха на один and тот же PNG), не грузят
/// и не аплоадят его на GPU повторно.
#[derive(Default)]
pub struct TextureCache {
    by_path: HashMap<String, LoadedTexture>,
}

impl TextureCache {
    /// Часть ссылается на реальный путь к PNG — пробуем загрузить, при
    /// неудаче логируем причину и подставляем цветную заглушку.
    pub fn get_or_load_png(&mut self, ctx: &GpuContext, path: &str, fallback_color: [f32; 4]) -> &LoadedTexture {
        if !self.by_path.contains_key(path) {
            let loaded = match load_png(ctx, path) {
                Ok(tex) => tex,
                Err(err) => {
                    eprintln!("[pony-render] текстура '{path}' не загружена ({err}) — использую заливку-заглушку");
                    solid_color_texture(ctx, to_u8_rgba(fallback_color))
                }
            };
            self.by_path.insert(path.to_string(), loaded);
        }
        self.by_path.get(path).expect("just inserted")
    }

    /// Часть ссылается на SVG — растеризуем один раз (см. `load_svg`) и
    /// дальше кэшируем как обычную текстуру; при ошибке — тот же fallback.
    pub fn get_or_load_svg(&mut self, ctx: &GpuContext, path: &str, fallback_color: [f32; 4]) -> &LoadedTexture {
        if !self.by_path.contains_key(path) {
            let loaded = match load_svg(ctx, path) {
                Ok(tex) => tex,
                Err(err) => {
                    eprintln!("[pony-render] SVG '{path}' не загружен ({err}) — использую заливку-заглушку");
                    solid_color_texture(ctx, to_u8_rgba(fallback_color))
                }
            };
            self.by_path.insert(path.to_string(), loaded);
        }
        self.by_path.get(path).expect("just inserted")
    }

    /// У части ещё нет никакого PNG-ассета (не пытались его назначить) —
    /// без попытки открыть файл сразу подставляем цветную заливку под
    /// заданным стабильным ключом (обычно один на вид части).
    pub fn get_or_create_fallback(&mut self, ctx: &GpuContext, key: &str, color: [f32; 4]) -> &LoadedTexture {
        if !self.by_path.contains_key(key) {
            let loaded = solid_color_texture(ctx, to_u8_rgba(color));
            self.by_path.insert(key.to_string(), loaded);
        }
        self.by_path.get(key).expect("just inserted")
    }
}

fn to_u8_rgba(color: [f32; 4]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}
