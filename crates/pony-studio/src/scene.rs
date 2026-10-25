//! Что и где рисовать на Stage — одна и та же математика для веб-холста
//! (JSON-список для JS, который строит из него SVG-DOM) и для экспорта
//! GIF/спрайт-листа и пипетки (SVG-строка, которую растеризует resvg на CPU).
//!
//! Геометрия частей — `pony_render::part_model_matrix`, ровно та же, что у
//! GPU-рендера: единичный квад [-0.5..0.5] (локальный +Y — верх картинки)
//! переводится в мир (Y вверх). Свет — множитель цвета в линейном
//! пространстве (как в шейдере: текстура sRGB, умножение после выборки),
//! поэтому фильтр feColorMatrix оставлен в linearRGB по умолчанию. Маска —
//! альфа другой части (`mask-type="alpha"`), как `t_mask` в шейдере.

use std::collections::HashMap;
use std::fmt::Write as _;

use base64::Engine as _;
use pony_core::part::{Part, PartSource};
use pony_core::Character;

/// Цвет фона Stage — тот же, которым GPU-рендер очищал кадр.
pub const SKY: [u8; 3] = [135, 207, 235];

/// 2D-аффинное преобразование в порядке SVG `matrix(a b c d e f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    #[allow(dead_code)]
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(x: f32, y: f32) -> Affine {
        Affine([x, 0.0, 0.0, y, 0.0, 0.0])
    }
    pub fn rotate(rad: f32) -> Affine {
        let (s, c) = rad.sin_cos();
        Affine([c, s, -s, c, 0.0, 0.0])
    }
    /// `self * rhs`: сначала `rhs`, потом `self`.
    pub fn mul(&self, rhs: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = rhs.0;
        Affine([a * a2 + c * b2, b * a2 + d * b2, a * c2 + c * d2, b * c2 + d * d2, a * e2 + c * f2 + e, b * e2 + d * f2 + f])
    }
    pub fn inverse(&self) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        let det = if det.abs() < 1e-12 { 1e-12 } else { det };
        Affine([d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det])
    }
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }
    pub fn from_mat4(m: &glam::Mat4) -> Affine {
        Affine([m.x_axis.x, m.x_axis.y, m.y_axis.x, m.y_axis.y, m.w_axis.x, m.w_axis.y])
    }
    fn svg(&self) -> String {
        let [a, b, c, d, e, f] = self.0;
        format!("matrix({a} {b} {c} {d} {e} {f})")
    }
}

/// Откуда брать картинку части.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Asset {
    Svg(String),
    Png(String),
    Psd(String, Option<String>),
    Kra(String, Option<String>),
}

impl Asset {
    pub fn of(part: &Part) -> Option<Asset> {
        let a = match &part.source {
            PartSource::Png { path } => Asset::Png(path.clone()),
            PartSource::Vector { path } => Asset::Svg(path.clone()),
            PartSource::Psd { path, layer } => Asset::Psd(path.clone(), layer.clone()),
            PartSource::Kra { path, layer_file } => Asset::Kra(path.clone(), layer_file.clone()),
            PartSource::Mesh { .. } => return None,
        };
        // Пустой слой (Insert > Пустой слой) и потерянный файл — заглушка
        // цветом, как у GPU-рендера, а не битая картинка.
        (!a.path().is_empty() && std::path::Path::new(a.path()).is_file()).then_some(a)
    }

    pub fn path(&self) -> &str {
        match self {
            Asset::Svg(p) | Asset::Png(p) | Asset::Psd(p, _) | Asset::Kra(p, _) => p,
        }
    }

    /// URL для веб-панели (см. `serve_file` в main.rs). `v` — время изменения
    /// файла: перезаписанный SVG (правка части, повторный импорт) получает
    /// новый URL, и браузер не показывает старую картинку из кэша.
    pub fn url(&self) -> String {
        let v = std::fs::metadata(self.path())
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut url = format!("f?p={}&v={v}", percent_encode(self.path()));
        match self {
            Asset::Psd(_, Some(layer)) => url.push_str(&format!("&layer={}", percent_encode(layer))),
            Asset::Kra(_, Some(lf)) => url.push_str(&format!("&lf={}", percent_encode(lf))),
            _ => {}
        }
        url
    }
}

/// Одна часть в порядке отрисовки.
#[derive(Clone, Debug)]
pub struct DrawItem {
    pub id: String,
    pub asset: Option<Asset>,
    /// sRGB 0..255 — цвет заглушки, если ассета нет.
    pub fallback: [u8; 4],
    /// Квад [-0.5..0.5] -> мир.
    pub model: Affine,
    /// Множитель света, `None` — нейтральный (1,1,1).
    pub light: Option<[f32; 3]>,
    pub mask: Option<MaskItem>,
}

#[derive(Clone, Debug)]
pub struct MaskItem {
    pub asset: Option<Asset>,
    pub fallback: [u8; 4],
    pub model: Affine,
}

#[derive(Clone, Debug)]
pub struct ParticleItem {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: [u8; 4],
}

fn to_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Список отрисовки персонажа: части по (layer, id) — тот же порядок, что у
/// GPU-рендера и таймлайна.
pub fn draw_list(character: &Character, lighting: &pony_core::Lighting) -> Vec<DrawItem> {
    let mut parts: Vec<&Part> = character.parts.values().collect();
    parts.sort_by(|a, b| a.layer.cmp(&b.layer).then_with(|| a.id.cmp(&b.id)));
    let models: HashMap<&str, Affine> = parts.iter().map(|p| (p.id.as_str(), Affine::from_mat4(&pony_render::part_model_matrix(p, character)))).collect();
    parts
        .iter()
        .map(|part| {
            let model = models[part.id.as_str()];
            let light = pony_core::lighting::shade_at(glam::Vec2::new(model.0[4], model.0[5]), lighting);
            let mask = character.resolve_clip_mask(&part.id).map(|m| MaskItem {
                asset: Asset::of(m),
                fallback: to_u8(pony_render::part_fallback_color(m.kind)),
                model: models[m.id.as_str()],
            });
            DrawItem {
                id: part.id.clone(),
                asset: Asset::of(part),
                fallback: to_u8(pony_render::part_fallback_color(part.kind)),
                model,
                light: (light != [1.0, 1.0, 1.0]).then_some(light),
                mask,
            }
        })
        .collect()
}

pub fn particle_list(emitter: &pony_core::ParticleEmitter) -> Vec<ParticleItem> {
    let color = to_u8(emitter.kind.base_color());
    emitter.particles.iter().map(|p| ParticleItem { x: p.position.x, y: p.position.y, size: p.current_size(), color }).collect()
}

/// Мир (Y вверх) -> пиксели кадра `width`x`height` (Y вниз) через камеру —
/// та же цепочка, что `compute_projection` у GPU-рендера.
pub fn camera_view(camera: &pony_core::Camera, time: f32, width: f32, height: f32) -> Affine {
    let shake = camera.shake_offset(time);
    Affine::translate(width / 2.0, height / 2.0)
        .mul(&Affine::scale(1.0, -1.0))
        .mul(&Affine::scale(camera.zoom.max(0.0001), camera.zoom.max(0.0001)))
        .mul(&Affine::rotate(-camera.rotation))
        .mul(&Affine::translate(-(camera.position.x + shake.x), -(camera.position.y + shake.y)))
}

/// Картинка части внутри квада: верх картинки — локальный +Y.
pub const IMAGE_FLIP: Affine = Affine([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);

/// Data-URI ассетов для автономной SVG-строки (resvg не ходит по нашему
/// протоколу). Кэш живёт весь экспорт — каждый файл читается один раз.
#[derive(Default)]
pub struct DataUris(HashMap<Asset, Option<String>>);

impl DataUris {
    fn get(&mut self, asset: &Asset) -> Option<String> {
        self.0
            .entry(asset.clone())
            .or_insert_with(|| {
                let (mime, bytes) = asset_bytes(asset).ok()?;
                Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
            })
            .clone()
    }
}

/// Байты ассета в формате, который понимает браузер/resvg: SVG и PNG как
/// есть, PSD/KRA — сведённые в PNG.
pub fn asset_bytes(asset: &Asset) -> Result<(&'static str, Vec<u8>), String> {
    match asset {
        Asset::Svg(p) => std::fs::read(p).map(|b| ("image/svg+xml", b)).map_err(|e| e.to_string()),
        Asset::Png(p) => std::fs::read(p).map(|b| ("image/png", b)).map_err(|e| e.to_string()),
        Asset::Psd(p, layer) => {
            let (rgba, w, h) = pony_render::decode_psd_rgba(p, layer.as_deref()).map_err(|e| e.to_string())?;
            encode_png(&rgba, w, h).map(|b| ("image/png", b))
        }
        Asset::Kra(p, lf) => {
            let (rgba, w, h) = pony_render::decode_kra_rgba(p, lf.as_deref()).map_err(|e| e.to_string())?;
            encode_png(&rgba, w, h).map(|b| ("image/png", b))
        }
    }
}

pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, w, h, image::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

use image::ImageEncoder as _;

fn rgba_css(c: [u8; 4]) -> String {
    format!("rgba({},{},{},{})", c[0], c[1], c[2], c[3] as f32 / 255.0)
}

/// Автономная SVG-строка кадра `width`x`height`: фон, части, частицы.
pub fn scene_svg(items: &[DrawItem], particles: &[ParticleItem], view: &Affine, width: u32, height: u32, uris: &mut DataUris) -> String {
    let mut defs = String::new();
    let mut body = String::new();
    let quad = |out: &mut String, asset: &Option<Asset>, fallback: [u8; 4], model: &Affine, uris: &mut DataUris| {
        let t = model.mul(&IMAGE_FLIP).svg();
        match asset.as_ref().and_then(|a| uris.get(a)) {
            Some(uri) => {
                let _ = write!(out, r#"<image href="{uri}" x="-0.5" y="-0.5" width="1" height="1" preserveAspectRatio="none" transform="{t}"/>"#);
            }
            None => {
                let _ = write!(out, r#"<rect x="-0.5" y="-0.5" width="1" height="1" fill="{}" transform="{t}"/>"#, rgba_css(fallback));
            }
        }
    };
    for (i, item) in items.iter().enumerate() {
        let mut attrs = String::new();
        if let Some(mask) = &item.mask {
            let mut inner = String::new();
            quad(&mut inner, &mask.asset, mask.fallback, &mask.model, uris);
            let _ = write!(defs, r#"<mask id="m{i}" mask-type="alpha" maskUnits="userSpaceOnUse" x="-100000" y="-100000" width="200000" height="200000">{inner}</mask>"#);
            let _ = write!(attrs, r#" mask="url(#m{i})""#);
        }
        if let Some([r, g, b]) = item.light {
            let _ = write!(defs, r#"<filter id="l{i}"><feColorMatrix type="matrix" values="{r} 0 0 0 0 0 {g} 0 0 0 0 0 {b} 0 0 0 0 0 1 0"/></filter>"#);
            let _ = write!(attrs, r#" filter="url(#l{i})""#);
        }
        let _ = write!(body, "<g{attrs}>");
        quad(&mut body, &item.asset, item.fallback, &item.model, uris);
        body.push_str("</g>");
    }
    for p in particles {
        let _ = write!(body, r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#, p.x - p.size / 2.0, p.y - p.size / 2.0, p.size, p.size, rgba_css(p.color));
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><defs>{defs}</defs><rect width="{width}" height="{height}" fill="rgb({},{},{})"/><g transform="{}">{body}</g></svg>"#,
        SKY[0],
        SKY[1],
        SKY[2],
        view.svg()
    )
}

/// Растеризовать SVG-строку кадра в RGBA (straight alpha).
pub fn rasterize(svg: &str, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1)).ok_or("пустой кадр")?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Ok(pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect())
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(()) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_mul_applies_right_operand_first() {
        let m = Affine::translate(10.0, 0.0).mul(&Affine::scale(2.0, 2.0));
        assert_eq!(m.apply(1.0, 1.0), (12.0, 2.0));
        let inv = m.inverse().apply(12.0, 2.0);
        assert!((inv.0 - 1.0).abs() < 1e-5 && (inv.1 - 1.0).abs() < 1e-5);
        let r = Affine::rotate(std::f32::consts::FRAC_PI_2).apply(1.0, 0.0);
        assert!((r.0).abs() < 1e-6 && (r.1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn percent_round_trip_keeps_unicode_and_spaces() {
        let p = "C:\\Пони/my file+1.svg";
        assert_eq!(percent_decode(&percent_encode(p)), p);
    }

    #[test]
    fn camera_view_centers_origin_and_flips_y() {
        let v = camera_view(&pony_core::Camera::default(), 0.0, 960.0, 720.0);
        assert_eq!(v.apply(0.0, 0.0), (480.0, 360.0));
        assert_eq!(v.apply(10.0, 10.0), (490.0, 350.0));
    }

    #[test]
    fn fallback_part_renders_its_color_on_sky() {
        let mut c = Character::new("t");
        c.skeleton = pony_core::skeleton::default_pony_skeleton();
        c.add_part(Part::new("b", pony_core::part::PartKind::Body, PartSource::Png { path: String::new() }).with_bone("Body"));
        let items = draw_list(&c, &pony_core::Lighting::default());
        assert_eq!(items.len(), 1);
        assert!(items[0].asset.is_none());
        let body = c.skeleton.world_transform_with_ik("Body").unwrap().position;
        let view = camera_view(&pony_core::Camera::default(), 0.0, 200.0, 200.0);
        let svg = scene_svg(&items, &[], &view, 200, 200, &mut DataUris::default());
        let rgba = rasterize(&svg, 200, 200).unwrap();
        let (px, py) = view.apply(body.x, body.y);
        let i = ((py as usize) * 200 + px as usize) * 4;
        assert_eq!(&rgba[i..i + 3], &items[0].fallback[..3], "в центре части — её цвет-заглушка");
        assert_eq!(&rgba[0..3], &SKY, "в углу — небо");
    }
}
