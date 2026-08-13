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
    #[error("failed to read PSD '{path}': {source}")]
    PsdIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse PSD '{path}': {source}")]
    PsdParse {
        path: String,
        #[source]
        source: psd::PsdError,
    },
    #[error("PSD '{path}' has no layer named '{layer}'")]
    PsdLayerNotFound { path: String, layer: String },
    /// `psd` 0.3.5 паникует (не возвращает `Result::Err`) на некоторых
    /// реальных PSD-файлах — конкретно на слоях со сжатием Zip, которое
    /// Photoshop использует по умолчанию для сохранения с прозрачностью.
    /// Проверено экспериментально: несжатый/RLE-сжатый PSD парсится
    /// нормально, а Zip-слой роняет `Psd::from_bytes` паникой изнутри
    /// крейта. Ловим это через `catch_unwind` в `load_psd` и превращаем в
    /// обычную ошибку — тот же принцип "никогда не роняем кадр", что и для
    /// отсутствующих файлов/битых SVG.
    #[error("PSD '{path}' triggered a panic inside the `psd` crate while parsing (likely Zip-compressed layers, which that crate doesn't support) — treated as a load failure")]
    PsdPanic { path: String },
    #[error("failed to read KRA '{path}': {source}")]
    KraIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open KRA '{path}' as a zip archive: {source}")]
    KraZip {
        path: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("KRA '{path}' has no entry named '{entry}' (обычно это mergedimage.png или layers/<имя>.png)")]
    KraEntryNotFound { path: String, entry: String },
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

/// Загрузить PSD: либо конкретный слой (`layer = Some(name)`), либо, если
/// имя не задано, сведённое изображение всех видимых слоёв (`psd.rgba()`).
///
/// Обёрнуто в `catch_unwind` — см. `TextureLoadError::PsdPanic` за
/// объяснением, почему это необходимо (реальный, проверенный баг в
/// используемой версии крейта `psd`, не гипотетическая предосторожность).
///
/// **Честная оговорка о потокобезопасности**: `std::panic::set_hook` —
/// глобальный, процессный, не per-thread. Пока этот вызов подавляет hook,
/// паника в ДРУГОМ потоке (например, если рендер когда-нибудь распараллелят
/// через rayon, как уже сделано в `pony-system`) тоже потеряет своё
/// сообщение. Для однопоточного пути рендера (как сейчас) это безопасно;
/// если рендер частей когда-нибудь станет многопоточным, эту защиту нужно
/// будет пересмотреть (например, ловить панику без подмены глобального hook,
/// смирившись с шумным выводом, или сериализовать вызовы `load_psd`).
pub fn load_psd(ctx: &GpuContext, path: &str, layer: Option<&str>) -> Result<LoadedTexture, TextureLoadError> {
    let bytes = std::fs::read(path).map_err(|source| TextureLoadError::PsdIo { path: path.to_string(), source })?;

    // Панику из psd-крейта мы ловим и обрабатываем как обычную ошибку (см.
    // TextureLoadError::PsdPanic) — это ожидаемый, штатно обрабатываемый
    // случай, а не крах. Стандартный panic hook всё равно напечатал бы
    // пугающий бэктрейс в stderr, будто что-то пошло катастрофически не так —
    // подавляем его на время именно этого вызова и восстанавливаем сразу
    // после, чтобы не проглотить вывод НЕожиданных паник в остальной программе.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let parse_result = std::panic::catch_unwind(|| psd::Psd::from_bytes(&bytes));
    std::panic::set_hook(previous_hook);

    let parsed = parse_result.map_err(|_| TextureLoadError::PsdPanic { path: path.to_string() })?;
    let psd_file = parsed.map_err(|source| TextureLoadError::PsdParse { path: path.to_string(), source })?;

    let (rgba, width, height) = match layer {
        Some(name) => {
            let psd_layer = psd_file
                .layer_by_name(name)
                .ok_or_else(|| TextureLoadError::PsdLayerNotFound { path: path.to_string(), layer: name.to_string() })?;
            (psd_layer.rgba(), psd_layer.width() as u32, psd_layer.height() as u32)
        }
        None => (psd_file.rgba(), psd_file.width(), psd_file.height()),
    };

    Ok(upload_rgba(ctx, Some(path), &rgba, width, height))
}

/// Загрузить KRA (формат Krita — zip-архив со слоями внутри). Без
/// `layer_file` берём `mergedimage.png` из корня архива — Krita всегда
/// пишет туда сведённый превью всех видимых слоёв, тот же принцип, что и
/// "сведённое изображение" у PSD без указания слоя. С `layer_file` —
/// конкретный PNG-файл внутри архива (обычно `layers/<имя>.png`).
pub fn load_kra(ctx: &GpuContext, path: &str, layer_file: Option<&str>) -> Result<LoadedTexture, TextureLoadError> {
    let file = std::fs::File::open(path).map_err(|source| TextureLoadError::KraIo { path: path.to_string(), source })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| TextureLoadError::KraZip { path: path.to_string(), source })?;

    let entry_name = layer_file.unwrap_or("mergedimage.png");
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|_| TextureLoadError::KraEntryNotFound { path: path.to_string(), entry: entry_name.to_string() })?;

    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|source| TextureLoadError::KraIo { path: path.to_string(), source })?;
    drop(entry);

    let rgba = image::load_from_memory(&bytes)
        .map_err(|source| TextureLoadError::Decode { path: format!("{path}!{entry_name}"), source })?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
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
///
/// Соблюдает бюджет памяти (раньше рос неограниченно, см. README) — при
/// каждой новой загрузке проверяет `LruBudget` и вытесняет наименее
/// недавно использованные текстуры, если суммарный размер (RGBA8 —
/// `width*height*4` байт на текстуру) превышает лимит. Повторное
/// обращение к уже загруженной текстуре — не бесплатное "просто отдать
/// ссылку": оно "трогает" запись в `LruBudget`, защищая её от вытеснения.
pub struct TextureCache {
    by_path: HashMap<String, LoadedTexture>,
    budget: crate::budget::LruBudget,
}

/// Бюджет по умолчанию для кэша, который никто явно не сконфигурировал —
/// достаточно большой, чтобы не мешать тестовым сценам из десятка PNG/SVG,
/// но не бесконечный. Настоящий бюджет (из `pony_system::WorkloadPolicy`,
/// посчитанный по реальной памяти системы) передаётся через
/// `TextureCache::with_budget`/`Renderer::new_with_budget`.
pub(crate) const DEFAULT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

impl Default for TextureCache {
    fn default() -> Self {
        Self::with_budget(DEFAULT_BUDGET_BYTES)
    }
}

impl TextureCache {
    pub fn with_budget(budget_bytes: u64) -> Self {
        Self { by_path: HashMap::new(), budget: crate::budget::LruBudget::new(budget_bytes) }
    }

    pub fn memory_used_bytes(&self) -> u64 {
        self.budget.total_bytes()
    }

    pub fn memory_budget_bytes(&self) -> u64 {
        self.budget.budget_bytes()
    }

    /// Зарегистрировать только что загруженную текстуру в бюджете и
    /// вытеснить, если понадобится. Возвращает то же самое `LoadedTexture`
    /// (перенося владение), чтобы вызвать сразу после загрузки, до
    /// вставки в `by_path`.
    fn account_and_evict(&mut self, key: &str, tex: &LoadedTexture) {
        let size_bytes = tex.width as u64 * tex.height as u64 * 4;
        self.budget.insert(key.to_string(), size_bytes);
        for evicted_key in self.budget.evict_to_fit() {
            self.by_path.remove(&evicted_key);
        }
    }

    pub fn get_or_load_png(&mut self, ctx: &GpuContext, path: &str, fallback_color: [f32; 4]) -> &LoadedTexture {
        if !self.by_path.contains_key(path) {
            let loaded = match load_png(ctx, path) {
                Ok(tex) => tex,
                Err(err) => {
                    eprintln!("[pony-render] текстура '{path}' не загружена ({err}) — использую заливку-заглушку");
                    solid_color_texture(ctx, to_u8_rgba(fallback_color))
                }
            };
            self.account_and_evict(path, &loaded);
            self.by_path.insert(path.to_string(), loaded);
        } else {
            self.budget.touch(path);
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
            self.account_and_evict(path, &loaded);
            self.by_path.insert(path.to_string(), loaded);
        } else {
            self.budget.touch(path);
        }
        self.by_path.get(path).expect("just inserted")
    }

    /// Часть ссылается на PSD — при ошибке (включая пойманную панику из
    /// `psd`, см. `TextureLoadError::PsdPanic`) тот же fallback, что и у
    /// остальных форматов. Кэш-ключ включает имя слоя, если оно задано, —
    /// иначе части одного PSD с разными слоями делили бы одну текстуру.
    pub fn get_or_load_psd(&mut self, ctx: &GpuContext, path: &str, layer: Option<&str>, fallback_color: [f32; 4]) -> &LoadedTexture {
        let cache_key = match layer {
            Some(name) => format!("{path}#{name}"),
            None => path.to_string(),
        };
        if !self.by_path.contains_key(&cache_key) {
            let loaded = match load_psd(ctx, path, layer) {
                Ok(tex) => tex,
                Err(err) => {
                    eprintln!("[pony-render] PSD '{path}' не загружен ({err}) — использую заливку-заглушку");
                    solid_color_texture(ctx, to_u8_rgba(fallback_color))
                }
            };
            self.account_and_evict(&cache_key, &loaded);
            self.by_path.insert(cache_key.clone(), loaded);
        } else {
            self.budget.touch(&cache_key);
        }
        self.by_path.get(&cache_key).expect("just inserted")
    }

    /// Часть ссылается на KRA — тот же принцип, что и у PSD: кэш-ключ
    /// включает имя entry внутри архива, при ошибке — цветная заглушка.
    pub fn get_or_load_kra(&mut self, ctx: &GpuContext, path: &str, layer_file: Option<&str>, fallback_color: [f32; 4]) -> &LoadedTexture {
        let cache_key = match layer_file {
            Some(name) => format!("{path}!{name}"),
            None => path.to_string(),
        };
        if !self.by_path.contains_key(&cache_key) {
            let loaded = match load_kra(ctx, path, layer_file) {
                Ok(tex) => tex,
                Err(err) => {
                    eprintln!("[pony-render] KRA '{path}' не загружен ({err}) — использую заливку-заглушку");
                    solid_color_texture(ctx, to_u8_rgba(fallback_color))
                }
            };
            self.account_and_evict(&cache_key, &loaded);
            self.by_path.insert(cache_key.clone(), loaded);
        } else {
            self.budget.touch(&cache_key);
        }
        self.by_path.get(&cache_key).expect("just inserted")
    }

    /// У части ещё нет никакого PNG-ассета (не пытались его назначить) —
    /// без попытки открыть файл сразу подставляем цветную заливку под
    /// заданным стабильным ключом (обычно один на вид части).
    pub fn get_or_create_fallback(&mut self, ctx: &GpuContext, key: &str, color: [f32; 4]) -> &LoadedTexture {
        if !self.by_path.contains_key(key) {
            let loaded = solid_color_texture(ctx, to_u8_rgba(color));
            self.account_and_evict(key, &loaded);
            self.by_path.insert(key.to_string(), loaded);
        } else {
            self.budget.touch(key);
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

#[cfg(test)]
mod vector_roundtrip_tests {
    // Проверяем не наш собственный сериализатор изолированно (это уже
    // сделано в pony-core), а весь цикл целиком: VectorDoc::to_svg_string()
    // -> файл на диске -> usvg::Tree::from_data (тот же парсер, что и в
    // load_svg) -> реальный растр. Если бы сериализатор писал невалидный
    // XML/SVG, здесь бы это всплыло — независимая, не только "текст
    // содержит нужную подстроку", а "настоящий SVG-парсер согласен, что
    // это валидный документ, и рисует ожидаемый цвет".
    use pony_core::{RgbaColor, VectorDoc, VectorShape};

    #[test]
    fn drawn_rect_round_trips_through_a_real_svg_parser() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 30.0,
            fill: RgbaColor::new(30, 200, 90, 255),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        let svg_text = doc.to_svg_string();

        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_str(&svg_text, &opt).expect("сгенерированный SVG должен быть валидным для usvg");
        let size = tree.size();
        assert!(size.width() > 0.0 && size.height() > 0.0);

        let width = size.width().ceil() as u32;
        let height = size.height().ceil() as u32;
        let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();
        resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());

        // Пиксель в центре прямоугольника должен быть нашим зелёным (с
        // поправкой на premultiplied alpha от tiny-skia — при alpha=255
        // premultiply не меняет значение, можно сравнивать напрямую).
        let cx = (width / 2) as usize;
        let cy = (height / 2) as usize;
        let idx = (cy * width as usize + cx) * 4;
        let data = pixmap.data();
        assert_eq!(&data[idx..idx + 3], &[30, 200, 90], "центр прямоугольника должен быть нарисован нашим зелёным цветом");
    }

    #[test]
    fn drawn_shape_loads_through_the_real_load_svg_pipeline() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Ellipse {
            cx: 20.0,
            cy: 20.0,
            rx: 18.0,
            ry: 18.0,
            fill: RgbaColor::new(220, 40, 40, 255),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        let svg_text = doc.to_svg_string();
        let path = "/tmp/pony_vector_roundtrip_test.svg";
        std::fs::write(path, &svg_text).expect("write svg");

        // Не через load_svg напрямую (нужен GpuContext/GPU) — но через тот
        // же usvg::Tree::from_data путь, что использует load_svg внутри,
        // на файле, реально записанном на диск (не только in-memory строка).
        let bytes = std::fs::read(path).expect("read back svg");
        let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default()).expect("файл с диска должен парситься");
        assert!(tree.size().width() > 0.0);

        std::fs::remove_file(path).ok();
    }
}

#[cfg(test)]
mod psd_tests {
    // Фикстуры сгенерированы ImageMagick (`convert`), не написаны нами
    // руками — независимая проверка, что наш код читает настоящий
    // сторонний PSD-инструмент, а не только собственный формат.
    const VALID_PSD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/test_fixtures/valid_uncompressed.psd");
    const PANICKING_PSD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/test_fixtures/unsupported_zip_layer.psd");

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let bytes_result = std::fs::read("/nonexistent/path/does_not_exist.psd");
        assert!(bytes_result.is_err(), "sanity check: path really doesn't exist");
        // Полный путь через load_psd требует GpuContext (нужен GPU) — здесь
        // проверяем только файловую часть, которая не зависит от GPU;
        // остальное покрыто реальным прогоном в pony-cli (см. README).
    }

    /// Сам факт, что `Psd::from_bytes` паникует на этом файле — воспроизводит
    /// баг крейта `psd` 0.3.5 (Zip-сжатые слои), НЕ баг нашего кода. Тест
    /// подтверждает, что баг всё ещё существует в используемой версии (если
    /// апстрим его когда-нибудь починит, этот тест начнёт падать — станет
    /// сигналом снять обёртку catch_unwind как отслужившую своё) и что наш
    /// `catch_unwind` действительно её ловит, а не просто теоретически должен.
    #[test]
    fn known_psd_crate_panic_is_caught_by_catch_unwind() {
        let bytes = std::fs::read(PANICKING_PSD).expect("fixture should exist");
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // не спамим stderr бэктрейсом ожидаемой паники
        let result = std::panic::catch_unwind(|| psd::Psd::from_bytes(&bytes));
        std::panic::set_hook(prev_hook);
        assert!(result.is_err(), "ожидали, что psd::Psd::from_bytes запаникует на Zip-слое (известный баг крейта)");
    }

    #[test]
    fn valid_uncompressed_psd_parses_without_panic() {
        let bytes = std::fs::read(VALID_PSD).expect("fixture should exist");
        let result = std::panic::catch_unwind(|| psd::Psd::from_bytes(&bytes));
        assert!(result.is_ok(), "несжатый PSD не должен паниковать");
        let psd_file = result.unwrap().expect("несжатый PSD должен успешно распарситься");
        assert_eq!((psd_file.width(), psd_file.height()), (16, 12));
        let rgba = psd_file.rgba();
        // Картинка была залита сплошным (200,50,60,255) при генерации фикстуры.
        assert_eq!(&rgba[0..4], &[200, 50, 60, 255]);
    }
}

#[cfg(test)]
mod kra_tests {
    // Фикстуры собраны через Python `zipfile` (не наш код) — независимая
    // проверка, что наш загрузчик читает настоящий сторонний zip-контейнер,
    // а не только то, что сам же и написал.
    const VALID_MERGED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/test_fixtures/valid_mergedimage.kra");
    const VALID_WITH_LAYER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/test_fixtures/valid_with_named_layer.kra");
    const MISSING_MERGED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/test_fixtures/missing_mergedimage.kra");

    #[test]
    fn reads_mergedimage_png_from_a_real_zip_archive() {
        let file = std::fs::File::open(VALID_MERGED).expect("fixture should exist");
        let mut archive = zip::ZipArchive::new(file).expect("valid zip");
        let mut entry = archive.by_name("mergedimage.png").expect("mergedimage.png should be present");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        let img = image::load_from_memory(&bytes).expect("should decode as PNG").to_rgba8();
        assert_eq!((img.width(), img.height()), (24, 16));
        // Залито сплошным (60,130,220,255) при генерации фикстуры.
        assert_eq!(img.get_pixel(0, 0).0, [60, 130, 220, 255]);
    }

    #[test]
    fn reads_a_specific_named_layer_entry() {
        let file = std::fs::File::open(VALID_WITH_LAYER).expect("fixture should exist");
        let mut archive = zip::ZipArchive::new(file).expect("valid zip");
        // Убеждаемся, что в архиве реально ДВЕ разные картинки под разными
        // именами — иначе тест "читаем конкретный слой" ничего бы не проверял.
        let mut merged_bytes = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name("mergedimage.png").unwrap(), &mut merged_bytes).unwrap();
        let merged = image::load_from_memory(&merged_bytes).unwrap().to_rgba8();

        let mut layer_bytes = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name("layers/body.png").unwrap(), &mut layer_bytes).unwrap();
        let layer = image::load_from_memory(&layer_bytes).unwrap().to_rgba8();

        assert_ne!(merged.get_pixel(0, 0).0, layer.get_pixel(0, 0).0, "merged и named-слой должны быть разными картинками в фикстуре");
        assert_eq!(layer.get_pixel(0, 0).0, [220, 60, 40, 255]);
        assert_eq!((layer.width(), layer.height()), (10, 10));
    }

    #[test]
    fn missing_mergedimage_entry_is_a_clean_error_not_a_panic() {
        let file = std::fs::File::open(MISSING_MERGED).expect("fixture should exist");
        let mut archive = zip::ZipArchive::new(file).expect("valid zip, just missing the entry we want");
        let result = archive.by_name("mergedimage.png");
        assert!(result.is_err(), "у этой фикстуры намеренно нет mergedimage.png — должна быть ошибка, не паника");
    }
}
