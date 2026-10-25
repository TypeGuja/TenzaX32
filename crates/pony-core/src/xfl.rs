//! Импорт Adobe Animate: `.fla` (ZIP с XFL внутри), распакованная папка
//! XFL или её `.xfl`-файл.
//!
//! XFL — это XML: `DOMDocument.xml` (главная временная шкала) и
//! `LIBRARY/*.xml` (символы). Графика векторная: `DOMShape` хранит стили
//! заливки/обводки и рёбра (`<Edge edges="!x y|x y[cx cy x y ...">`) в
//! твипах (1/20 px), у каждого ребра — стиль заливки слева (`fillStyle0`)
//! и справа (`fillStyle1`). Замкнутые контуры заливок собираются из этих
//! рёбер — так же, как это делает сам Animate при публикации.
//!
//! Здесь документ разбирается в модель (символы, слои, кадры, элементы),
//! а любой кадр любого символа переводится в SVG — дальше его рисует тот
//! же resvg, что и остальные векторные части движка. `import_to_character`
//! раскладывает кадр символа на части персонажа: каждый слой (или группа
//! "маска + замаскированные слои") — отдельная SVG-часть со своей костью в
//! точке трансформации Animate, так что в редакторе части двигаются и
//! вращаются вокруг тех же точек, что и у аниматора в Animate.
//!
//! Что поддерживается: символы (graphic/movie clip/button) с firstFrame и
//! режимами loop/play once/single frame, группы, вложенность любой
//! глубины, маски (через `clipPath`), классические motion-твины (линейно),
//! цветовые преобразования (alpha/tint/brightness/advanced), сплошные и
//! градиентные (linear/radial) заливки, обводки. Что нет: текст, растровые
//! заливки и вставленные картинки (заменяются серым), shape-твины, фильтры,
//! режимы смешивания, 3D.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const XFL_NS: &str = "http://ns.adobe.com/xfl/2008/";
/// Предел вложенности символов — защита от циклических ссылок в битом файле.
const MAX_DEPTH: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum XflError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an Adobe Animate document: {0}")]
    NotXfl(String),
    #[error("broken archive: {0}")]
    Zip(String),
    #[error("xml error in {file}: {message}")]
    Xml { file: String, message: String },
    #[error("symbol not found: {0}")]
    NoSymbol(String),
}

// ---------------------------------------------------------------- модель

/// Аффинная матрица Animate: x' = a*x + c*y + tx, y' = b*x + d*y + ty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }

    /// `self * child`: сначала child, потом self.
    pub fn then(&self, child: &Matrix) -> Matrix {
        Matrix {
            a: self.a * child.a + self.c * child.b,
            b: self.b * child.a + self.d * child.b,
            c: self.a * child.c + self.c * child.d,
            d: self.b * child.c + self.d * child.d,
            tx: self.a * child.tx + self.c * child.ty + self.tx,
            ty: self.b * child.tx + self.d * child.ty + self.ty,
        }
    }

    fn lerp(&self, o: &Matrix, t: f32) -> Matrix {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Matrix { a: l(self.a, o.a), b: l(self.b, o.b), c: l(self.c, o.c), d: l(self.d, o.d), tx: l(self.tx, o.tx), ty: l(self.ty, o.ty) }
    }

    fn svg(&self) -> String {
        format!("matrix({} {} {} {} {} {})", f(self.a), f(self.b), f(self.c), f(self.d), f(self.tx), f(self.ty))
    }
}

/// Цветовое преобразование Animate (multiplier + offset на канал).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTransform {
    pub mul: [f32; 4],
    pub add: [f32; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        Self { mul: [1.0; 4], add: [0.0; 4] }
    }
}

impl ColorTransform {
    /// `self` применяется ПОСЛЕ `inner` (родитель поверх ребёнка).
    fn then(&self, inner: &ColorTransform) -> ColorTransform {
        let mut out = ColorTransform::default();
        for i in 0..4 {
            out.mul[i] = self.mul[i] * inner.mul[i];
            out.add[i] = self.mul[i] * inner.add[i] + self.add[i];
        }
        out
    }

    fn apply(&self, c: [f32; 4]) -> [f32; 4] {
        let mut o = [0.0; 4];
        for i in 0..3 {
            o[i] = (c[i] * self.mul[i] + self.add[i]).clamp(0.0, 255.0);
        }
        o[3] = (c[3] * self.mul[3] + self.add[3] / 255.0).clamp(0.0, 1.0);
        o
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub ratio: f32,
    /// r, g, b в 0..255, a в 0..1.
    pub color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid([f32; 4]),
    Linear { matrix: Matrix, stops: Vec<GradientStop>, spread: String },
    Radial { matrix: Matrix, stops: Vec<GradientStop>, spread: String, focal: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub paint: Paint,
    pub weight: f32,
    pub caps: String,
    pub joints: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub from: (f32, f32),
    pub ctrl: Option<(f32, f32)>,
    pub to: (f32, f32),
}

impl Segment {
    fn reversed(&self) -> Segment {
        Segment { from: self.to, ctrl: self.ctrl, to: self.from }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub fill0: u32,
    pub fill1: u32,
    pub stroke: u32,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    pub matrix: Matrix,
    pub fills: HashMap<u32, Paint>,
    pub strokes: HashMap<u32, StrokeStyle>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    Loop,
    PlayOnce,
    SingleFrame,
    LoopReverse,
    PlayOnceReverse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInstance {
    pub library_item: String,
    pub matrix: Matrix,
    /// Точка трансформации (вокруг неё вращает Free Transform в Animate),
    /// в координатах самого символа.
    pub transformation_point: (f32, f32),
    pub first_frame: usize,
    pub loop_mode: LoopMode,
    /// Movie clip / button показываются первым кадром (их время в Animate
    /// идёт независимо от родителя).
    pub independent_time: bool,
    pub color: ColorTransform,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Shape(Shape),
    Group(Vec<Element>),
    Symbol(SymbolInstance),
    /// Текст, растровая картинка и прочее, что не переводится в векторы.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub index: usize,
    pub duration: usize,
    pub label: Option<String>,
    pub motion_tween: bool,
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Normal,
    Guide,
    Mask,
    Folder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    pub kind: LayerKind,
    pub parent: Option<usize>,
    pub visible: bool,
    pub frames: Vec<Frame>,
}

/// Временная шкала. Слои — в порядке XFL: ПЕРВЫЙ слой — ВЕРХНИЙ.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub name: String,
    pub layers: Vec<Layer>,
}

impl Timeline {
    pub fn frame_count(&self) -> usize {
        self.layers.iter().flat_map(|l| l.frames.iter().map(|f| f.index + f.duration)).max().unwrap_or(1).max(1)
    }

    /// Метки кадров (`name` у ключевого кадра) — в мультяшных паппетах ими
    /// подписаны ракурсы и позы ("Turn180", "Sit", ...).
    pub fn frame_labels(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .layers
            .iter()
            .flat_map(|l| l.frames.iter().filter_map(|f| f.label.clone().map(|n| (f.index, n))))
            .filter(|(_, n)| !n.is_empty() && n != "sync")
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn layer_is_masked_by(&self, li: usize) -> Option<usize> {
        let p = self.layers[li].parent?;
        (self.layers.get(p)?.kind == LayerKind::Mask).then_some(p)
    }

    /// Не попадает в картинку: направляющий слой (или слой в папке-
    /// направляющей) — всегда; слой, скрытый глазком (сам или его папка), —
    /// только если `include_hidden == false`. Animate по умолчанию
    /// ПУБЛИКУЕТ скрытые слои (Publish Settings → «Include hidden layers»,
    /// `<InvisibleLayer>1`): художники прячут слои для удобства работы, а в
    /// готовом мультфильме они видны. Раньше импорт их пропускал, и у Луны
    /// пропадала тёмно-синяя заливка и звёзды гривы — оставался только контур.
    fn layer_hidden(&self, li: usize, include_hidden: bool) -> bool {
        let mut cur = Some(li);
        let mut guard = 0;
        while let Some(i) = cur {
            let l = &self.layers[i];
            if l.kind == LayerKind::Guide || (!l.visible && !include_hidden) {
                return true;
            }
            cur = l.parent;
            guard += 1;
            if guard > self.layers.len() {
                break;
            }
        }
        false
    }

    fn frame_at(&self, li: usize, frame: usize) -> Option<(usize, &Frame)> {
        let frames = &self.layers[li].frames;
        let i = frames.iter().position(|f| f.index <= frame && frame < f.index + f.duration)?;
        Some((i, &frames[i]))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct XflDocument {
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub frame_rate: f32,
    pub background: [f32; 4],
    pub main: Timeline,
    pub symbols: HashMap<String, Timeline>,
    /// Что не удалось разобрать (битые символы и т.п.) — честно сообщаем
    /// пользователю, а не молча теряем.
    pub warnings: Vec<String>,
    /// Рисовать слои, скрытые глазком (см. `Timeline::layer_hidden`).
    /// Берётся из Publish Settings документа (`<InvisibleLayer>`), по
    /// умолчанию — да, как публикует Animate. Можно переключить в окне импорта.
    pub include_hidden_layers: bool,
}

// ------------------------------------------------------------ контейнер

/// Файлы документа: путь внутри XFL -> содержимое.
type FileMap = HashMap<String, Vec<u8>>;

/// Читает `.fla` как ZIP по ЛОКАЛЬНЫМ заголовкам. Animate пишет
/// central directory с ошибкой ("reported length of central directory is
/// 54 bytes too long") — стандартные ZIP-библиотеки такие файлы не
/// открывают, а локальные заголовки у него корректные.
fn read_fla(bytes: &[u8]) -> Result<FileMap, XflError> {
    let mut files = FileMap::new();
    let mut pos = 0usize;
    let u16_at = |p: usize| -> u16 { u16::from_le_bytes([bytes[p], bytes[p + 1]]) };
    let u32_at = |p: usize| -> u32 { u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]) };
    while pos + 30 <= bytes.len() {
        if &bytes[pos..pos + 4] != b"PK\x03\x04" {
            // central directory или мусор — локальные записи кончились
            break;
        }
        let flags = u16_at(pos + 6);
        let method = u16_at(pos + 8);
        let mut comp_size = u32_at(pos + 18) as usize;
        let name_len = u16_at(pos + 26) as usize;
        let extra_len = u16_at(pos + 28) as usize;
        let name_start = pos + 30;
        let data_start = name_start + name_len + extra_len;
        if data_start > bytes.len() {
            return Err(XflError::Zip("truncated local header".into()));
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).replace('\\', "/");
        if flags & 0x08 != 0 && comp_size == 0 {
            // Размер в data descriptor после данных: ищем следующую запись.
            let next = find_sub(&bytes[data_start..], b"PK\x03\x04").map(|o| data_start + o).unwrap_or(bytes.len());
            comp_size = next.saturating_sub(data_start + 16);
        }
        let data_end = (data_start + comp_size).min(bytes.len());
        let raw = &bytes[data_start..data_end];
        if !name.ends_with('/') {
            let data = match method {
                0 => raw.to_vec(),
                8 => miniz_oxide::inflate::decompress_to_vec(raw).map_err(|e| XflError::Zip(format!("{name}: inflate failed: {e:?}")))?,
                m => return Err(XflError::Zip(format!("{name}: unsupported compression method {m}"))),
            };
            files.insert(name, data);
        }
        pos = data_end;
        if flags & 0x08 != 0 {
            // data descriptor (необязательная сигнатура + crc + 2 размера)
            if bytes.get(pos..pos + 4) == Some(b"PK\x07\x08") {
                pos += 16;
            } else {
                pos += 12;
            }
        }
    }
    if files.is_empty() {
        return Err(XflError::NotXfl("no files in archive".into()));
    }
    Ok(files)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_dir_recursive(root: &Path, dir: &Path, out: &mut FileMap) -> Result<(), XflError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            read_dir_recursive(root, &p, out)?;
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xml")) {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            out.insert(rel, std::fs::read(&p)?);
        }
    }
    Ok(())
}

impl XflDocument {
    /// Открыть `.fla`, `.xfl` или папку XFL.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, XflError> {
        let path = path.as_ref();
        let files = if path.is_dir() {
            let mut files = FileMap::new();
            read_dir_recursive(path, path, &mut files)?;
            files
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xfl")) {
            let dir = path.parent().unwrap_or(Path::new("."));
            let mut files = FileMap::new();
            read_dir_recursive(dir, dir, &mut files)?;
            files
        } else {
            read_fla(&std::fs::read(path)?)?
        };
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("document").to_string();
        Self::from_files(name, &files)
    }

    fn from_files(name: String, files: &FileMap) -> Result<Self, XflError> {
        let dom = files.get("DOMDocument.xml").ok_or_else(|| XflError::NotXfl("DOMDocument.xml is missing".into()))?;
        let text = decode_xml(dom);
        let xml = roxmltree::Document::parse(&text).map_err(|e| XflError::Xml { file: "DOMDocument.xml".into(), message: e.to_string() })?;
        let root = xml.root_element();
        let attr_f = |n: &str, d: f32| root.attribute(n).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        let mut doc = XflDocument {
            name,
            width: attr_f("width", 550.0),
            height: attr_f("height", 400.0),
            frame_rate: attr_f("frameRate", 24.0),
            background: root.attribute("backgroundColor").map(|c| parse_color(c, 1.0)).unwrap_or([255.0, 255.0, 255.0, 1.0]),
            main: Timeline { name: "Scene 1".into(), layers: Vec::new() },
            symbols: HashMap::new(),
            warnings: Vec::new(),
            include_hidden_layers: files
                .get("PublishSettings.xml")
                .map(|b| !decode_xml(b).contains("<InvisibleLayer>0</InvisibleLayer>"))
                .unwrap_or(true),
        };
        if let Some(tl) = child(root, "timelines").and_then(|t| child(t, "DOMTimeline")) {
            doc.main = parse_timeline(tl);
        }
        let mut lib: Vec<(&String, &Vec<u8>)> = files.iter().filter(|(k, _)| k.starts_with("LIBRARY/") && k.ends_with(".xml")).collect();
        lib.sort_by(|a, b| a.0.cmp(b.0));
        for (file, bytes) in lib {
            let text = decode_xml(bytes);
            let xml = match roxmltree::Document::parse(&text) {
                Ok(x) => x,
                Err(e) => {
                    doc.warnings.push(format!("{file}: {e}"));
                    continue;
                }
            };
            let item = xml.root_element();
            if item.tag_name().name() != "DOMSymbolItem" {
                continue;
            }
            let Some(name) = item.attribute("name") else { continue };
            if let Some(tl) = child(item, "timeline").and_then(|t| child(t, "DOMTimeline")) {
                doc.symbols.insert(name.to_string(), parse_timeline(tl));
            }
        }
        Ok(doc)
    }

    /// Имена символов библиотеки, по алфавиту.
    pub fn symbol_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.symbols.keys().map(|s| s.as_str()).collect();
        v.sort_by_key(|s| s.to_lowercase());
        v
    }

    /// `None` — главная временная шкала документа.
    pub fn timeline(&self, symbol: Option<&str>) -> Option<&Timeline> {
        match symbol {
            None => Some(&self.main),
            Some(n) => self.symbols.get(n),
        }
    }
}

fn decode_xml(bytes: &[u8]) -> String {
    // UTF-16 с BOM встречается в старых XFL; иначе UTF-8 (с BOM или без).
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&u);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&u);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

// ------------------------------------------------------------- парсинг

fn is(node: roxmltree::Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace().map_or(true, |ns| ns == XFL_NS)
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| is(*c, name))
}

fn children<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a {
    node.children().filter(move |c| is(*c, name))
}

fn fattr(node: roxmltree::Node, name: &str, default: f32) -> f32 {
    node.attribute(name).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(default)
}

fn parse_timeline(tl: roxmltree::Node) -> Timeline {
    let mut layers = Vec::new();
    if let Some(ls) = child(tl, "layers") {
        for l in children(ls, "DOMLayer") {
            let kind = match l.attribute("layerType") {
                Some("guide") => LayerKind::Guide,
                Some("mask") => LayerKind::Mask,
                Some("folder") => LayerKind::Folder,
                _ => LayerKind::Normal,
            };
            let mut frames = Vec::new();
            if let Some(fs) = child(l, "frames") {
                for fr in children(fs, "DOMFrame") {
                    let elements = child(fr, "elements").map(parse_elements).unwrap_or_default();
                    frames.push(Frame {
                        index: fattr(fr, "index", 0.0) as usize,
                        duration: (fattr(fr, "duration", 1.0) as usize).max(1),
                        label: fr.attribute("name").map(str::to_string),
                        motion_tween: fr.attribute("tweenType") == Some("motion"),
                        elements,
                    });
                }
            }
            layers.push(Layer {
                name: l.attribute("name").unwrap_or("Layer").to_string(),
                kind,
                parent: l.attribute("parentLayerIndex").and_then(|v| v.parse().ok()),
                visible: l.attribute("visible") != Some("false"),
                frames,
            });
        }
    }
    Timeline { name: tl.attribute("name").unwrap_or("").to_string(), layers }
}

fn parse_elements(node: roxmltree::Node) -> Vec<Element> {
    node.children().filter(|c| c.is_element()).map(parse_element).collect()
}

fn parse_matrix(node: roxmltree::Node) -> Matrix {
    child(node, "matrix")
        .and_then(|m| child(m, "Matrix"))
        .map(|m| Matrix {
            a: fattr(m, "a", 1.0),
            b: fattr(m, "b", 0.0),
            c: fattr(m, "c", 0.0),
            d: fattr(m, "d", 1.0),
            tx: fattr(m, "tx", 0.0),
            ty: fattr(m, "ty", 0.0),
        })
        .unwrap_or_default()
}

fn parse_element(node: roxmltree::Node) -> Element {
    match node.tag_name().name() {
        "DOMShape" => Element::Shape(parse_shape(node)),
        "DOMGroup" => Element::Group(child(node, "members").map(parse_elements).unwrap_or_default()),
        "DOMSymbolInstance" => {
            let tp = child(node, "transformationPoint").and_then(|t| child(t, "Point")).map(|p| (fattr(p, "x", 0.0), fattr(p, "y", 0.0))).unwrap_or((0.0, 0.0));
            let loop_mode = match node.attribute("loop") {
                Some("play once") => LoopMode::PlayOnce,
                Some("single frame") => LoopMode::SingleFrame,
                Some("loop reverse") => LoopMode::LoopReverse,
                Some("play once reverse") => LoopMode::PlayOnceReverse,
                _ => LoopMode::Loop,
            };
            let independent = !matches!(node.attribute("symbolType"), Some("graphic"));
            Element::Symbol(SymbolInstance {
                library_item: node.attribute("libraryItemName").unwrap_or("").to_string(),
                matrix: parse_matrix(node),
                transformation_point: tp,
                first_frame: fattr(node, "firstFrame", 0.0) as usize,
                loop_mode,
                independent_time: independent,
                color: child(node, "color").and_then(|c| child(c, "Color")).map(parse_color_transform).unwrap_or_default(),
                visible: node.attribute("isVisible") != Some("false"),
            })
        }
        other => Element::Unsupported(other.to_string()),
    }
}

fn parse_color_transform(c: roxmltree::Node) -> ColorTransform {
    let mut ct = ColorTransform::default();
    if let Some(b) = c.attribute("brightness").and_then(|v| v.parse::<f32>().ok()) {
        if b > 0.0 {
            ct.mul = [1.0 - b, 1.0 - b, 1.0 - b, 1.0];
            ct.add = [255.0 * b, 255.0 * b, 255.0 * b, 0.0];
        } else {
            ct.mul = [1.0 + b, 1.0 + b, 1.0 + b, 1.0];
        }
    }
    if let Some(tc) = c.attribute("tintColor") {
        let tm = fattr(c, "tintMultiplier", 0.0);
        let col = parse_color(tc, 1.0);
        ct.mul = [1.0 - tm, 1.0 - tm, 1.0 - tm, 1.0];
        ct.add = [col[0] * tm, col[1] * tm, col[2] * tm, 0.0];
    }
    for (i, ch) in ["red", "green", "blue", "alpha"].iter().enumerate() {
        if let Some(m) = c.attribute(format!("{ch}Multiplier").as_str()).and_then(|v| v.parse::<f32>().ok()) {
            ct.mul[i] = m;
        }
        if let Some(o) = c.attribute(format!("{ch}Offset").as_str()).and_then(|v| v.parse::<f32>().ok()) {
            ct.add[i] = o;
        }
    }
    ct
}

/// "#RRGGBB" + alpha -> [r, g, b (0..255), a (0..1)].
fn parse_color(s: &str, alpha: f32) -> [f32; 4] {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    [((v >> 16) & 0xFF) as f32, ((v >> 8) & 0xFF) as f32, (v & 0xFF) as f32, alpha]
}

fn parse_paint(node: roxmltree::Node) -> Paint {
    if let Some(s) = child(node, "SolidColor") {
        return Paint::Solid(parse_color(s.attribute("color").unwrap_or("#000000"), fattr(s, "alpha", 1.0)));
    }
    let grad = |g: roxmltree::Node| -> (Matrix, Vec<GradientStop>, String) {
        let stops = children(g, "GradientEntry")
            .map(|e| GradientStop { ratio: fattr(e, "ratio", 0.0), color: parse_color(e.attribute("color").unwrap_or("#000000"), fattr(e, "alpha", 1.0)) })
            .collect();
        (parse_matrix(g), stops, g.attribute("spreadMethod").unwrap_or("pad").to_string())
    };
    if let Some(g) = child(node, "LinearGradient") {
        let (matrix, stops, spread) = grad(g);
        return Paint::Linear { matrix, stops, spread };
    }
    if let Some(g) = child(node, "RadialGradient") {
        let (matrix, stops, spread) = grad(g);
        return Paint::Radial { matrix, stops, spread, focal: fattr(g, "focalPointRatio", 0.0) };
    }
    // BitmapFill и неизвестное: нейтральный серый вместо пропажи формы.
    Paint::Solid([128.0, 128.0, 128.0, 1.0])
}

fn parse_shape(node: roxmltree::Node) -> Shape {
    let mut shape = Shape { matrix: parse_matrix(node), ..Default::default() };
    if let Some(fills) = child(node, "fills") {
        for fs in children(fills, "FillStyle") {
            shape.fills.insert(fattr(fs, "index", 0.0) as u32, parse_paint(fs));
        }
    }
    if let Some(strokes) = child(node, "strokes") {
        for ss in children(strokes, "StrokeStyle") {
            let idx = fattr(ss, "index", 0.0) as u32;
            if let Some(st) = ss.children().find(|c| c.is_element()) {
                let paint = child(st, "fill").map(parse_paint).unwrap_or(Paint::Solid([0.0, 0.0, 0.0, 1.0]));
                shape.strokes.insert(
                    idx,
                    StrokeStyle {
                        paint,
                        weight: fattr(st, "weight", 1.0),
                        caps: st.attribute("caps").unwrap_or("round").to_string(),
                        joints: st.attribute("joints").unwrap_or("round").to_string(),
                    },
                );
            }
        }
    }
    if let Some(edges) = child(node, "edges") {
        for e in children(edges, "Edge") {
            let Some(d) = e.attribute("edges") else { continue };
            shape.edges.push(Edge {
                fill0: fattr(e, "fillStyle0", 0.0) as u32,
                fill1: fattr(e, "fillStyle1", 0.0) as u32,
                stroke: fattr(e, "strokeStyle", 0.0) as u32,
                segments: parse_edge_data(d),
            });
        }
    }
    shape
}

/// Число в рёбрах XFL: десятичное в твипах или `#XXXXXX.YY` — 24.8
/// fixed point в hex (дополнительный код), тоже в твипах.
fn parse_edge_number(tok: &str) -> Option<f32> {
    if let Some(h) = tok.strip_prefix('#') {
        let (int_part, frac_part) = h.split_once('.').unwrap_or((h, "0"));
        let int_v = i64::from_str_radix(int_part, 16).ok()?;
        let bits = int_part.len() * 4;
        let int_v = if bits > 0 && bits < 64 && int_v >= (1i64 << (bits - 1)) && bits >= 24 { int_v - (1i64 << bits) } else { int_v };
        let frac_v = i64::from_str_radix(&format!("{:0<2}", &frac_part[..frac_part.len().min(2)]), 16).ok()? as f32 / 256.0;
        Some(int_v as f32 + frac_v)
    } else {
        tok.parse::<f32>().ok()
    }
}

/// Разбор атрибута `edges`: `!` moveTo, `|`/`/` lineTo, `[`/`]` quadTo
/// (cx cy x y), `S<n>` — служебные пометки выделения (пропускаем).
/// Координаты переводятся из твипов в пиксели.
pub fn parse_edge_data(d: &str) -> Vec<Segment> {
    let mut segs = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut cmd = '!';
    let mut nums: Vec<f32> = Vec::with_capacity(4);
    let bytes: Vec<char> = d.chars().collect();
    let mut i = 0;
    let flush = |cmd: char, nums: &mut Vec<f32>, cur: &mut (f32, f32), segs: &mut Vec<Segment>| {
        match cmd {
            '!' if nums.len() >= 2 => {
                *cur = (nums[0] / 20.0, nums[1] / 20.0);
                nums.clear();
            }
            '|' | '/' if nums.len() >= 2 => {
                let to = (nums[0] / 20.0, nums[1] / 20.0);
                segs.push(Segment { from: *cur, ctrl: None, to });
                *cur = to;
                nums.clear();
            }
            '[' | ']' if nums.len() >= 4 => {
                let c = (nums[0] / 20.0, nums[1] / 20.0);
                let to = (nums[2] / 20.0, nums[3] / 20.0);
                segs.push(Segment { from: *cur, ctrl: Some(c), to });
                *cur = to;
                nums.clear();
            }
            _ => {}
        }
    };
    while i < bytes.len() {
        let ch = bytes[i];
        match ch {
            '!' | '|' | '/' | '[' | ']' => {
                nums.clear();
                cmd = ch;
                i += 1;
            }
            'S' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            c if c == '-' || c == '#' || c == '.' || c.is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == '.' || (bytes[start] != '#' && (bytes[i] == 'e' || bytes[i] == 'E'))) {
                    // У десятичных чисел hex-буквы (кроме экспоненты) не встречаются.
                    if bytes[start] != '#' && bytes[i].is_ascii_alphabetic() && bytes[i] != 'e' && bytes[i] != 'E' {
                        break;
                    }
                    i += 1;
                }
                let tok: String = bytes[start..i].iter().collect();
                if let Some(v) = parse_edge_number(&tok) {
                    nums.push(v);
                    flush(cmd, &mut nums, &mut cur, &mut segs);
                }
            }
            _ => i += 1,
        }
    }
    segs
}

// ------------------------------------------------------------ рендер SVG

fn f(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

#[derive(Default)]
struct SvgOut {
    defs: String,
    body: String,
    next_id: usize,
    bbox: Option<(f32, f32, f32, f32)>,
    /// Внутри `clipPath` пишутся только геометрии, без заливок/обводок.
    clip_mode: bool,
    unsupported: usize,
}

impl SvgOut {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn grow(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.bbox = Some(match self.bbox {
            Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
            None => (x, y, x, y),
        });
    }

    fn rgba(c: [f32; 4]) -> (String, f32) {
        (format!("#{:02x}{:02x}{:02x}", c[0].round() as u8, c[1].round() as u8, c[2].round() as u8), c[3])
    }

    /// Вернуть значение атрибута fill/stroke (цвет или url градиента) и
    /// непрозрачность, с учётом цветового преобразования.
    fn paint(&mut self, paint: &Paint, ct: &ColorTransform) -> (String, f32) {
        match paint {
            Paint::Solid(c) => Self::rgba(ct.apply(*c)),
            Paint::Linear { matrix, stops, spread } | Paint::Radial { matrix, stops, spread, .. } => {
                let id = self.id("g");
                let spread = match spread.as_str() {
                    "reflect" => "reflect",
                    "repeat" => "repeat",
                    _ => "pad",
                };
                // Градиентный бокс Animate: -819.2..819.2 px (16384 твипа),
                // растянутый матрицей градиента.
                let head = match paint {
                    Paint::Linear { .. } => format!(r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="-819.2" y1="0" x2="819.2" y2="0" spreadMethod="{spread}" gradientTransform="{}">"#, matrix.svg()),
                    Paint::Radial { focal, .. } => format!(
                        r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="819.2" fx="{}" fy="0" spreadMethod="{spread}" gradientTransform="{}">"#,
                        f(819.2 * focal),
                        matrix.svg()
                    ),
                    Paint::Solid(_) => unreachable!(),
                };
                self.defs.push_str(&head);
                for s in stops {
                    let (col, a) = Self::rgba(ct.apply(s.color));
                    let _ = write!(self.defs, r#"<stop offset="{}" stop-color="{col}" stop-opacity="{}"/>"#, f(s.ratio), f(a));
                }
                self.defs.push_str(if matches!(paint, Paint::Linear { .. }) { "</linearGradient>" } else { "</radialGradient>" });
                (format!("url(#{id})"), 1.0)
            }
        }
    }
}

fn path_data(loops: &[Vec<Segment>]) -> String {
    let mut d = String::new();
    for lp in loops {
        let Some(first) = lp.first() else { continue };
        let _ = write!(d, "M{} {}", f(first.from.0), f(first.from.1));
        let mut cur = first.from;
        for s in lp {
            if (s.from.0 - cur.0).abs() > 1e-3 || (s.from.1 - cur.1).abs() > 1e-3 {
                let _ = write!(d, "M{} {}", f(s.from.0), f(s.from.1));
            }
            match s.ctrl {
                Some(c) => {
                    let _ = write!(d, "Q{} {} {} {}", f(c.0), f(c.1), f(s.to.0), f(s.to.1));
                }
                None => {
                    let _ = write!(d, "L{} {}", f(s.to.0), f(s.to.1));
                }
            }
            cur = s.to;
        }
    }
    d
}

/// Собрать замкнутые контуры заливки из направленных отрезков: каждый
/// отрезок продолжается тем, что начинается в его конце.
fn link_loops(mut segs: Vec<Segment>) -> Vec<Vec<Segment>> {
    let key = |p: (f32, f32)| ((p.0 * 100.0).round() as i64, (p.1 * 100.0).round() as i64);
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        by_start.entry(key(s.from)).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut lp = vec![segs[start]];
        let origin = key(segs[start].from);
        let mut end = key(segs[start].to);
        while end != origin {
            let next = by_start.get(&end).and_then(|c| c.iter().copied().find(|&i| !used[i]));
            match next {
                Some(i) => {
                    used[i] = true;
                    lp.push(segs[i]);
                    end = key(segs[i].to);
                }
                None => break, // незамкнутый контур — SVG замкнёт заливку сам
            }
        }
        loops.push(lp);
    }
    segs.clear();
    loops
}

fn emit_shape(out: &mut SvgOut, shape: &Shape, m: &Matrix, ct: &ColorTransform) {
    let m = m.then(&shape.matrix);
    let transform = m.svg();
    for e in &shape.edges {
        for s in &e.segments {
            for p in [Some(s.from), s.ctrl, Some(s.to)].into_iter().flatten() {
                let (x, y) = m.apply(p.0, p.1);
                out.grow(x, y);
            }
        }
    }
    // Заливки: слева от ребра — fillStyle0 (отрезок разворачиваем), справа — fillStyle1.
    let mut fill_ids: Vec<u32> = shape.fills.keys().copied().collect();
    fill_ids.sort_unstable();
    for fid in fill_ids {
        let mut segs = Vec::new();
        for e in &shape.edges {
            if e.fill1 == fid {
                segs.extend(e.segments.iter().copied());
            }
            if e.fill0 == fid {
                segs.extend(e.segments.iter().rev().map(Segment::reversed));
            }
        }
        if segs.is_empty() {
            continue;
        }
        let d = path_data(&link_loops(segs));
        if out.clip_mode {
            let _ = write!(out.body, r#"<path transform="{transform}" clip-rule="evenodd" d="{d}"/>"#);
        } else {
            let (paint, opacity) = out.paint(&shape.fills[&fid], ct);
            if opacity <= 0.0 {
                continue;
            }
            let op = if opacity < 1.0 { format!(r#" fill-opacity="{}""#, f(opacity)) } else { String::new() };
            let _ = write!(out.body, r#"<path transform="{transform}" fill="{paint}"{op} fill-rule="evenodd" d="{d}"/>"#);
        }
    }
    if out.clip_mode {
        return;
    }
    let mut stroke_ids: Vec<u32> = shape.strokes.keys().copied().collect();
    stroke_ids.sort_unstable();
    for sid in stroke_ids {
        let segs: Vec<Segment> = shape.edges.iter().filter(|e| e.stroke == sid).flat_map(|e| e.segments.iter().copied()).collect();
        if segs.is_empty() {
            continue;
        }
        let st = &shape.strokes[&sid];
        let (paint, opacity) = out.paint(&st.paint, ct);
        if opacity <= 0.0 {
            continue;
        }
        let cap = match st.caps.as_str() {
            "none" => "butt",
            "square" => "square",
            _ => "round",
        };
        let join = match st.joints.as_str() {
            "miter" => "miter",
            "bevel" => "bevel",
            _ => "round",
        };
        let op = if opacity < 1.0 { format!(r#" stroke-opacity="{}""#, f(opacity)) } else { String::new() };
        let d = path_data(&[segs]);
        let _ = write!(
            out.body,
            r#"<path transform="{transform}" fill="none" stroke="{paint}"{op} stroke-width="{}" stroke-linecap="{cap}" stroke-linejoin="{join}" d="{d}"/>"#,
            f(st.weight.max(0.05))
        );
    }
}

impl XflDocument {
    fn symbol_frame(&self, inst: &SymbolInstance, parent_offset: usize) -> usize {
        let n = self.symbols.get(&inst.library_item).map(|t| t.frame_count()).unwrap_or(1).max(1);
        if inst.independent_time {
            return 0;
        }
        let ff = inst.first_frame.min(n - 1);
        match inst.loop_mode {
            LoopMode::SingleFrame => ff,
            LoopMode::Loop => (ff + parent_offset) % n,
            LoopMode::PlayOnce => (ff + parent_offset).min(n - 1),
            LoopMode::LoopReverse => (ff + n * (parent_offset / n + 1) - parent_offset % n) % n,
            LoopMode::PlayOnceReverse => ff.saturating_sub(parent_offset),
        }
    }

    fn emit_element(&self, out: &mut SvgOut, el: &Element, m: &Matrix, ct: &ColorTransform, frame_offset: usize, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match el {
            Element::Shape(s) => emit_shape(out, s, m, ct),
            Element::Group(members) => {
                for e in members {
                    self.emit_element(out, e, m, ct, frame_offset, depth + 1);
                }
            }
            Element::Symbol(inst) => {
                if !inst.visible {
                    return;
                }
                let Some(tl) = self.symbols.get(&inst.library_item) else { return };
                let frame = self.symbol_frame(inst, frame_offset);
                self.emit_timeline(out, tl, frame, &m.then(&inst.matrix), &ct.then(&inst.color), depth + 1);
            }
            Element::Unsupported(_) => out.unsupported += 1,
        }
    }

    /// Элементы слоя на кадре (с интерполяцией классического motion-твина).
    fn layer_elements(&self, tl: &Timeline, li: usize, frame: usize) -> Option<(Vec<Element>, usize)> {
        let (fi, fr) = tl.frame_at(li, frame)?;
        let offset = frame - fr.index;
        if !fr.motion_tween || offset == 0 {
            return Some((fr.elements.clone(), offset));
        }
        let Some(next) = tl.layers[li].frames.get(fi + 1) else { return Some((fr.elements.clone(), offset)) };
        let t = offset as f32 / fr.duration as f32;
        let mut els = fr.elements.clone();
        for (a, b) in els.iter_mut().zip(next.elements.iter()) {
            if let (Element::Symbol(sa), Element::Symbol(sb)) = (a, b) {
                if sa.library_item == sb.library_item {
                    sa.matrix = sa.matrix.lerp(&sb.matrix, t);
                    for i in 0..4 {
                        sa.color.mul[i] += (sb.color.mul[i] - sa.color.mul[i]) * t;
                        sa.color.add[i] += (sb.color.add[i] - sa.color.add[i]) * t;
                    }
                }
            }
        }
        Some((els, offset))
    }

    fn emit_layer(&self, out: &mut SvgOut, tl: &Timeline, li: usize, frame: usize, m: &Matrix, ct: &ColorTransform, depth: usize) {
        if let Some((els, offset)) = self.layer_elements(tl, li, frame) {
            for el in &els {
                self.emit_element(out, el, m, ct, offset, depth);
            }
        }
    }

    /// Слой-маска и его замаскированные слои: содержимое в `<g clip-path>`.
    fn emit_mask_group(&self, out: &mut SvgOut, tl: &Timeline, mask: usize, frame: usize, m: &Matrix, ct: &ColorTransform, depth: usize) {
        let clip_id = out.id("c");
        let mut clip = SvgOut { clip_mode: true, next_id: out.next_id, ..Default::default() };
        self.emit_layer(&mut clip, tl, mask, frame, m, ct, depth);
        out.next_id = clip.next_id;
        out.defs.push_str(&clip.defs);
        let _ = write!(out.defs, r#"<clipPath id="{clip_id}">{}</clipPath>"#, clip.body);
        let _ = write!(out.body, r#"<g clip-path="url(#{clip_id})">"#);
        // Габарит замаскированного содержимого считаем отдельно и режем
        // габаритом маски: под маской часто лежит что-то огромное (длинная
        // анимированная грива у Селестии), и без обрезки часть получалась
        // в разы больше видимого.
        let outer_bbox = out.bbox.take();
        // Слои снизу вверх (в XFL первый — верхний).
        for li in (0..tl.layers.len()).rev() {
            if tl.layer_is_masked_by(li) == Some(mask) && !tl.layer_hidden(li, self.include_hidden_layers) {
                self.emit_layer(out, tl, li, frame, m, ct, depth);
            }
        }
        out.body.push_str("</g>");
        let clipped = match (out.bbox, clip.bbox) {
            (Some(a), Some(c)) => {
                let r = (a.0.max(c.0), a.1.max(c.1), a.2.min(c.2), a.3.min(c.3));
                (r.0 < r.2 && r.1 < r.3).then_some(r)
            }
            _ => None,
        };
        out.bbox = outer_bbox;
        if let Some((x0, y0, x1, y1)) = clipped {
            out.grow(x0, y0);
            out.grow(x1, y1);
        }
    }

    fn emit_timeline(&self, out: &mut SvgOut, tl: &Timeline, frame: usize, m: &Matrix, ct: &ColorTransform, depth: usize) {
        for unit in render_units(tl, self.include_hidden_layers) {
            self.emit_unit(out, tl, unit, frame, m, ct, depth);
        }
    }

    fn emit_unit(&self, out: &mut SvgOut, tl: &Timeline, unit: usize, frame: usize, m: &Matrix, ct: &ColorTransform, depth: usize) {
        if tl.layers[unit].kind == LayerKind::Mask {
            self.emit_mask_group(out, tl, unit, frame, m, ct, depth);
        } else {
            self.emit_layer(out, tl, unit, frame, m, ct, depth);
        }
    }
}

/// Порядок отрисовки слоёв временной шкалы снизу вверх. Замаскированные
/// слои в список не входят — их рисует слой-маска вместе с собой.
fn render_units(tl: &Timeline, include_hidden: bool) -> Vec<usize> {
    (0..tl.layers.len())
        .rev()
        .filter(|&li| {
            let l = &tl.layers[li];
            l.kind != LayerKind::Folder && !tl.layer_hidden(li, include_hidden) && tl.layer_is_masked_by(li).is_none()
        })
        .collect()
}

/// Готовый SVG и его габарит (в координатах символа, px, Y вниз).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedSvg {
    pub svg: String,
    pub bbox: (f32, f32, f32, f32),
    /// Сколько элементов пропущено (текст, растровые картинки).
    pub unsupported: usize,
}

fn finish_svg(out: SvgOut, pad: f32) -> Option<RenderedSvg> {
    let (x0, y0, x1, y1) = out.bbox?;
    let (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}" height="{}"><defs>{}</defs>{}</svg>"#,
        f(x0),
        f(y0),
        f(w),
        f(h),
        f(w),
        f(h),
        out.defs,
        out.body
    );
    Some(RenderedSvg { svg, bbox: (x0, y0, x0 + w, y0 + h), unsupported: out.unsupported })
}

impl XflDocument {
    /// Весь кадр символа (или главной шкалы при `None`) одним SVG.
    pub fn render_svg(&self, symbol: Option<&str>, frame: usize) -> Option<RenderedSvg> {
        let tl = self.timeline(symbol)?;
        let mut out = SvgOut::default();
        self.emit_timeline(&mut out, tl, frame, &Matrix::IDENTITY, &ColorTransform::default(), 0);
        finish_svg(out, 2.0)
    }
}

// ------------------------------------------------------------- импорт

#[derive(Debug, Clone, PartialEq)]
pub struct ImportOptions {
    /// Мировых единиц на пиксель Animate.
    pub scale: f32,
    /// Раскрывать вложенные символы-контейнеры (имя начинается с `~`, как
    /// `~RD_Head` у паппетов MLP) на отдельные части: глаза, рот, брови
    /// становятся самостоятельными слоями, а не частью головы.
    pub expand_containers: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self { scale: 1.0, expand_containers: false }
    }
}

/// Одна будущая часть персонажа.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPart {
    pub id: String,
    pub rendered: RenderedSvg,
    /// Точка трансформации Animate (вокруг неё вращается часть), px.
    pub origin: (f32, f32),
    /// Раскрытый контейнер, внутри которого лежит часть (`None` — верхний уровень).
    pub container: Option<String>,
}

/// Раскрытый символ-контейнер (`~RD_Head`): станет костью-родителем для
/// костей своих частей — повернул голову, и глаза/рот/грива поехали с ней.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedContainer {
    pub id: String,
    pub origin: (f32, f32),
    pub parent: Option<String>,
}

/// Результат разбора кадра: части снизу вверх + контейнеры.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedRig {
    pub parts: Vec<ImportedPart>,
    pub containers: Vec<ImportedContainer>,
}

impl XflDocument {
    /// Разложить кадр символа на части: по одной на слой (маска +
    /// замаскированные — одна часть). Порядок — снизу вверх.
    pub fn import_parts(&self, symbol: Option<&str>, frame: usize, opts: &ImportOptions) -> Result<ImportedRig, XflError> {
        let tl = self.timeline(symbol).ok_or_else(|| XflError::NoSymbol(symbol.unwrap_or("").to_string()))?;
        let mut rig = ImportedRig::default();
        let mut used: HashMap<String, usize> = HashMap::new();
        self.collect_parts(tl, frame, &Matrix::IDENTITY, &ColorTransform::default(), "", None, opts, &mut rig, &mut used, 0);
        Ok(rig)
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_parts(
        &self,
        tl: &Timeline,
        frame: usize,
        m: &Matrix,
        ct: &ColorTransform,
        prefix: &str,
        container: Option<&str>,
        opts: &ImportOptions,
        rig: &mut ImportedRig,
        used: &mut HashMap<String, usize>,
        depth: usize,
    ) {
        for unit in render_units(tl, self.include_hidden_layers) {
            let layer = &tl.layers[unit];
            let base = format!("{prefix}{}", sanitize_id(&layer.name));
            // Раскрытие контейнера: обычный слой с одним символом "~...".
            if opts.expand_containers && layer.kind == LayerKind::Normal && depth < MAX_DEPTH {
                if let Some((els, offset)) = self.layer_elements(tl, unit, frame) {
                    if let [Element::Symbol(inst)] = els.as_slice() {
                        if inst.visible && inst.library_item.rsplit('/').next().is_some_and(|n| n.starts_with('~')) {
                            if let Some(sub) = self.symbols.get(&inst.library_item) {
                                let sub_frame = self.symbol_frame(inst, offset);
                                let inst_m = m.then(&inst.matrix);
                                let origin = inst_m.apply(inst.transformation_point.0, inst.transformation_point.1);
                                rig.containers.push(ImportedContainer { id: base.clone(), origin, parent: container.map(str::to_string) });
                                self.collect_parts(sub, sub_frame, &inst_m, &ct.then(&inst.color), &format!("{base}."), Some(&base), opts, rig, used, depth + 1);
                                continue;
                            }
                        }
                    }
                }
            }
            let mut out = SvgOut::default();
            self.emit_unit(&mut out, tl, unit, frame, m, ct, depth);
            let Some(rendered) = finish_svg(out, 1.0) else { continue };
            // Точка вращения: точка трансформации символа, если слой —
            // один символ; иначе центр габарита.
            let origin = self
                .layer_elements(tl, unit, frame)
                .and_then(|(els, _)| match els.as_slice() {
                    [Element::Symbol(inst)] => {
                        let (x, y) = m.then(&inst.matrix).apply(inst.transformation_point.0, inst.transformation_point.1);
                        Some((x, y))
                    }
                    _ => None,
                })
                .unwrap_or(((rendered.bbox.0 + rendered.bbox.2) / 2.0, (rendered.bbox.1 + rendered.bbox.3) / 2.0));
            let n = used.entry(base.clone()).or_insert(0);
            *n += 1;
            let id = if *n == 1 { base } else { format!("{base}_{n}") };
            rig.parts.push(ImportedPart { id, rendered, origin, container: container.map(str::to_string) });
        }
    }
}

fn sanitize_id(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "layer".into()
    } else {
        s
    }
}

/// Что получилось при импорте — для статус-бара.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub parts: usize,
    pub unsupported: usize,
    pub out_dir: PathBuf,
}

/// Импортировать кадр символа как персонажа: SVG частей пишутся в
/// `out_dir`, каждая часть висит на своей кости в точке трансформации
/// Animate (кости — дети `Root`), координаты переводятся в мировые
/// (Y вверх) с масштабом `opts.scale`.
pub fn import_to_character(
    doc: &XflDocument,
    symbol: Option<&str>,
    frame: usize,
    opts: &ImportOptions,
    out_dir: &Path,
) -> Result<(crate::Character, ImportReport), XflError> {
    use crate::part::{Part, PartKind, PartSource};
    use crate::skeleton::{Bone, Transform2D};

    let rig = doc.import_parts(symbol, frame, opts)?;
    let parts = &rig.parts;
    std::fs::create_dir_all(out_dir)?;
    let name = symbol.unwrap_or(&doc.name).rsplit('/').next().unwrap_or("Imported").trim_start_matches('~').to_string();
    let mut character = crate::Character::new(format!("{name} @{frame}"));
    character.skeleton.add_bone(Bone { id: "Root".into(), parent: None, local_transform: Transform2D::default(), length: 0.0 });
    let s = opts.scale;
    let to_world = |p: (f32, f32)| glam::Vec2::new(p.0 * s, -p.1 * s);
    // Кости контейнеров (родитель раньше ребёнка — так их и собирали).
    let mut container_world: HashMap<String, glam::Vec2> = HashMap::new();
    for c in &rig.containers {
        let world = to_world(c.origin);
        let parent_world = c.parent.as_ref().and_then(|p| container_world.get(p)).copied().unwrap_or(glam::Vec2::ZERO);
        character.skeleton.add_bone(Bone {
            id: format!("b.{}", c.id),
            parent: Some(c.parent.as_ref().map(|p| format!("b.{p}")).unwrap_or_else(|| "Root".into())),
            local_transform: Transform2D { position: world - parent_world, ..Default::default() },
            length: 0.0,
        });
        container_world.insert(c.id.clone(), world);
    }
    let mut unsupported = 0;
    for (i, p) in parts.iter().enumerate() {
        unsupported += p.rendered.unsupported;
        let file = out_dir.join(format!("{}.svg", p.id));
        std::fs::write(&file, &p.rendered.svg)?;
        let (x0, y0, x1, y1) = p.rendered.bbox;
        let center = glam::Vec2::new((x0 + x1) / 2.0 * s, -(y0 + y1) / 2.0 * s);
        let origin = to_world(p.origin);
        let bone_id = format!("b.{}", p.id);
        let parent_world = p.container.as_ref().and_then(|c| container_world.get(c)).copied().unwrap_or(glam::Vec2::ZERO);
        character.skeleton.add_bone(Bone {
            id: bone_id.clone(),
            parent: Some(p.container.as_ref().map(|c| format!("b.{c}")).unwrap_or_else(|| "Root".into())),
            local_transform: Transform2D { position: origin - parent_world, ..Default::default() },
            length: 0.0,
        });
        let lower = p.id.to_lowercase();
        // Глаза и рот — первыми: у раскрытого контейнера головы их id
        // "HEAD.EYE_mask", "HEAD.MOUTH", и они должны реагировать на
        // выражения (Blink/Smile), а не считаться головой.
        let leaf = lower.rsplit('.').next().unwrap_or(&lower);
        let kind = if leaf.contains("eye") && !leaf.contains("brow") && !leaf.contains("lash") {
            PartKind::Eyes
        } else if leaf.contains("mouth") {
            PartKind::Mouth
        } else if lower.contains("head") {
            PartKind::Head
        } else if lower.contains("tail") {
            PartKind::Tail
        } else if lower.contains("wing") {
            PartKind::Wing
        } else if lower.contains("ear") {
            PartKind::Ear
        } else if lower.contains("torso") || lower.contains("body") {
            PartKind::Body
        } else {
            PartKind::Custom
        };
        character.add_part(
            Part::new(&p.id, kind, PartSource::Vector { path: file.to_string_lossy().replace('\\', "/") })
                .with_bone(&bone_id)
                .with_pivot(center - origin)
                .with_size(glam::Vec2::new((x1 - x0) * s, (y1 - y0) * s))
                .with_layer(i as i32),
        );
    }
    Ok((character, ImportReport { parts: parts.len(), unsupported, out_dir: out_dir.to_path_buf() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_edge_numbers_are_signed_fixed_point() {
        assert_eq!(parse_edge_number("#126.78"), Some(0x126 as f32 + 0x78 as f32 / 256.0));
        // 0xFFFB7B = -1157 в 24-битном дополнительном коде
        let v = parse_edge_number("#FFFB7B.92").unwrap();
        assert!((v - (-1157.0 + 0x92 as f32 / 256.0)).abs() < 1e-3, "{v}");
        assert_eq!(parse_edge_number("-12.5"), Some(-12.5));
    }

    #[test]
    fn edge_data_parses_moves_lines_quads_and_skips_selection_marks() {
        let segs = parse_edge_data("!0 0|200 0[400 0 400 200!400 200S2|0 200");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment { from: (0.0, 0.0), ctrl: None, to: (10.0, 0.0) });
        assert_eq!(segs[1].ctrl, Some((20.0, 0.0)));
        assert_eq!(segs[1].to, (20.0, 10.0));
        assert_eq!(segs[2], Segment { from: (20.0, 10.0), ctrl: None, to: (0.0, 10.0) });
    }

    #[test]
    fn fill_loops_are_assembled_from_left_and_right_edges() {
        // Квадрат: две стороны несут заливку справа, две — слева.
        let shape = Shape {
            fills: [(1u32, Paint::Solid([255.0, 0.0, 0.0, 1.0]))].into_iter().collect(),
            edges: vec![
                Edge { fill0: 0, fill1: 1, stroke: 0, segments: parse_edge_data("!0 0|200 0|200 200") },
                Edge { fill0: 1, fill1: 0, stroke: 0, segments: parse_edge_data("!0 0|0 200|200 200") },
            ],
            ..Default::default()
        };
        let mut out = SvgOut::default();
        emit_shape(&mut out, &shape, &Matrix::IDENTITY, &ColorTransform::default());
        assert!(out.body.contains("fill=\"#ff0000\""));
        // Один замкнутый контур из четырёх отрезков: M + 4 команды.
        assert_eq!(out.body.matches('M').count(), 1, "{}", out.body);
        assert_eq!(out.body.matches('L').count(), 4, "{}", out.body);
        assert_eq!(out.bbox, Some((0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn color_transforms_compose_and_apply() {
        let tint = ColorTransform { mul: [0.5, 0.5, 0.5, 1.0], add: [127.5, 0.0, 0.0, 0.0] };
        let half_alpha = ColorTransform { mul: [1.0, 1.0, 1.0, 0.5], add: [0.0; 4] };
        let c = half_alpha.then(&tint).apply([0.0, 200.0, 100.0, 1.0]);
        assert_eq!(c, [127.5, 100.0, 50.0, 0.5]);
    }

    fn sample_doc() -> XflDocument {
        let dom = br##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="100" height="80" frameRate="12">
          <timelines><DOMTimeline name="Scene 1"><layers>
            <DOMLayer name="pony"><frames><DOMFrame index="0" duration="3"><elements>
              <DOMSymbolInstance libraryItemName="~Pony" symbolType="graphic" loop="loop">
                <matrix><Matrix tx="50" ty="40"/></matrix>
                <transformationPoint><Point x="5" y="5"/></transformationPoint>
              </DOMSymbolInstance>
            </elements></DOMFrame></frames></DOMLayer>
          </layers></DOMTimeline></timelines></DOMDocument>"##;
        let pony = br##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="~Pony" symbolType="graphic"><timeline><DOMTimeline name="~Pony"><layers>
            <DOMLayer name="Guide" layerType="guide"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#00FF00"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer>
            <DOMLayer name="Eye mask" layerType="mask"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#FFFFFF"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|100 0|100 100|0 100|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer>
            <DOMLayer name="Pupil" parentLayerIndex="1"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#000000"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer>
            <DOMLayer name="Body"><frames>
              <DOMFrame index="0" name="Front"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#E781B3"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame>
              <DOMFrame index="1" name="Side"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#E781B3"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|400 0|400 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame>
            </frames></DOMLayer>
          </layers></DOMTimeline></timeline></DOMSymbolItem>"##;
        let mut files = FileMap::new();
        files.insert("DOMDocument.xml".into(), dom.to_vec());
        files.insert("LIBRARY/~Pony.xml".into(), pony.to_vec());
        XflDocument::from_files("test".into(), &files).unwrap()
    }

    #[test]
    fn document_parses_symbols_labels_and_frame_counts() {
        let doc = sample_doc();
        assert_eq!(doc.symbol_names(), vec!["~Pony"]);
        let tl = doc.timeline(Some("~Pony")).unwrap();
        assert_eq!(tl.frame_count(), 2);
        assert_eq!(tl.frame_labels(), vec![(0, "Front".to_string()), (1, "Side".to_string())]);
        assert_eq!(doc.main.frame_count(), 3);
    }

    #[test]
    fn hidden_layers_are_published_like_animate_does_unless_turned_off() {
        let dom = br##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/"><timelines><DOMTimeline name="S"><layers>
            <DOMLayer name="Fill" visible="false"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#214EC5"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer>
            <DOMLayer name="Ref" layerType="guide"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><SolidColor color="#00FF00"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer>
        </layers></DOMTimeline></timelines></DOMDocument>"##;
        let mut files = FileMap::new();
        files.insert("DOMDocument.xml".into(), dom.to_vec());
        let mut doc = XflDocument::from_files("t".into(), &files).unwrap();
        assert!(doc.include_hidden_layers, "по умолчанию Animate публикует скрытые слои");
        let svg = doc.render_svg(None, 0).unwrap().svg;
        assert!(svg.contains("#214ec5"), "скрытый слой должен рисоваться");
        assert!(!svg.contains("#00ff00"), "направляющий — никогда");
        doc.include_hidden_layers = false;
        assert!(doc.render_svg(None, 0).is_none(), "с выключенной опцией скрытый слой пропадает");

        files.insert("PublishSettings.xml".into(), b"<flash_profiles><PublishFlashProperties><InvisibleLayer>0</InvisibleLayer></PublishFlashProperties></flash_profiles>".to_vec());
        assert!(!XflDocument::from_files("t".into(), &files).unwrap().include_hidden_layers, "настройка документа уважается");
    }

    #[test]
    fn render_skips_guides_and_clips_masked_layers() {
        let doc = sample_doc();
        let r = doc.render_svg(Some("~Pony"), 0).unwrap();
        assert!(!r.svg.contains("#00ff00"), "guide layer must not be rendered");
        assert!(r.svg.contains("<clipPath"), "mask must become a clipPath");
        assert!(r.svg.contains("clip-path=\"url(#"));
        // Body на кадре 1 шире, чем на кадре 0.
        let r1 = doc.render_svg(Some("~Pony"), 1).unwrap();
        assert!(r1.bbox.2 > r.bbox.2);
    }

    #[test]
    fn nested_symbols_follow_loop_mode_and_matrix() {
        let doc = sample_doc();
        // Главная шкала, кадр 1: "loop" -> кадр 1 символа (широкое тело), сдвиг 50/40.
        let r = doc.render_svg(None, 1).unwrap();
        assert!((r.bbox.0 - 48.0).abs() < 0.01 && (r.bbox.2 - 72.0).abs() < 0.01, "{:?}", r.bbox);
    }

    #[test]
    fn import_makes_one_part_per_layer_with_bones_at_transformation_points() {
        let doc = sample_doc();
        let rig = doc.import_parts(None, 0, &ImportOptions::default()).unwrap();
        assert_eq!(rig.parts.len(), 1);
        assert!(rig.containers.is_empty());
        assert_eq!(rig.parts[0].origin, (55.0, 45.0));
        let expanded = doc.import_parts(None, 0, &ImportOptions { expand_containers: true, ..Default::default() }).unwrap();
        assert_eq!(expanded.containers, vec![ImportedContainer { id: "pony".into(), origin: (55.0, 45.0), parent: None }]);
        assert!(expanded.parts.iter().all(|p| p.container.as_deref() == Some("pony")));
        let ids: Vec<&str> = expanded.parts.iter().map(|p| p.id.as_str()).collect();
        // Снизу вверх: Body, затем маска с замаскированным зрачком — одна часть.
        assert_eq!(ids, vec!["pony.Body", "pony.Eye_mask"]);

        let dir = std::env::temp_dir().join(format!("pony_xfl_test_{}", std::process::id()));
        let (character, report) = import_to_character(&doc, None, 0, &ImportOptions { scale: 2.0, expand_containers: true }, &dir).unwrap();
        assert_eq!(report.parts, 2);
        let body = &character.parts["pony.Body"];
        let bone = character.skeleton.world_transform(body.bone.as_deref().unwrap()).unwrap();
        // Кость части — центр её габарита (у слоя из фигур нет точки
        // трансформации): тело 0..10 px внутри символа + сдвиг 50/40.
        assert_eq!(bone.position, glam::Vec2::new(110.0, -90.0));
        // ...и она — ребёнок кости контейнера (55,45) -> мир (110,-90).
        assert_eq!(character.skeleton.find(body.bone.as_deref().unwrap()).unwrap().parent.as_deref(), Some("b.pony"));
        assert_eq!(character.skeleton.world_transform("b.pony").unwrap().position, glam::Vec2::new(110.0, -90.0));
        assert!(std::path::Path::new(match &body.source {
            crate::part::PartSource::Vector { path } => path,
            _ => panic!(),
        })
        .exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fla_zip_is_read_by_local_headers() {
        // Минимальный ZIP (stored) с битым central directory.
        let mut z = Vec::new();
        let name = b"DOMDocument.xml";
        let data = br#"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/"/>"#;
        z.extend_from_slice(b"PK\x03\x04");
        z.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(name);
        z.extend_from_slice(data);
        z.extend_from_slice(b"PK\x01\x02garbage");
        let files = read_fla(&z).unwrap();
        assert_eq!(files["DOMDocument.xml"], data.to_vec());
    }
}
