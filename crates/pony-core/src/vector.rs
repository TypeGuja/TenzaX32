//! Редактируемый векторный документ (раздел 16 ТЗ — SVG теперь не только
//! импортируется, но и рисуется/редактируется/сохраняется прямо в движке).
//! Чистая модель без GPU — сериализуется в настоящий SVG-текст, который
//! потом читает уже готовый импортёр (`pony_render::texture::load_svg`,
//! `resvg`) — то есть нарисованное здесь тут же можно использовать как
//! часть персонажа, замкнутый цикл "рисуем -> сохраняем -> используем".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `#RRGGBB` — цвет без альфы, альфа для SVG уходит отдельным атрибутом
    /// `fill-opacity`/`stroke-opacity` (0..1), не в hex-код.
    fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    fn opacity(self) -> f32 {
        self.a as f32 / 255.0
    }

    /// Линейная интерполяция между двумя цветами по каналам (включая
    /// альфу) — единственное место, где считается смешение цвета между
    /// стопами градиента (`GradientDef::sample`) и в fallback-превью GUI
    /// (`draw_vector_shape_preview`), чтобы оба места сходились в одной
    /// формуле, а не двух потенциально расходящихся копиях.
    fn lerp(self, other: RgbaColor, t: f32) -> RgbaColor {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| -> u8 {
            (a as f32 + (b as f32 - a as f32) * t)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        RgbaColor::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
            mix(self.a, other.a),
        )
    }
}

/// Раздел 60 ТЗ (Rendering: Gradients) — одна именованная остановка цвета
/// градиента. `offset` — позиция вдоль градиента в диапазоне `[0.0, 1.0]`
/// (0 — начало, 1 — конец), как атрибут `offset` у SVG `<stop>`. Порядок
/// хранения в `GradientDef::stops` НЕ обязан быть отсортирован по
/// `offset` — `GradientDef::sample` сортирует сама при семплировании
/// (пользователь может перетащить существующую остановку мимо соседней в
/// редакторе, порядок хранения — деталь реализации, не инвариант).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: RgbaColor,
}

/// Геометрия градиента в тех же (object space) координатах, что и сами
/// фигуры документа — соответствует SVG `gradientUnits="userSpaceOnUse"`
/// (не `objectBoundingBox`, у которого 0..1 масштабируется под КАЖДУЮ
/// фигуру отдельно): один `GradientDef` с абсолютными координатами
/// одинаково применяется/переиспользуется на нескольких фигурах, что
/// проще предсказать в редакторе, чем relative-координаты, зависящие от
/// габарита каждой конкретной фигуры, на которую он назначен.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GradientKind {
    /// Линейный градиент вдоль отрезка `(x1,y1)-(x2,y2)` — та же пара
    /// точек, что и у SVG `<linearGradient>` с `gradientUnits="userSpaceOnUse"`.
    Linear { x1: f32, y1: f32, x2: f32, y2: f32 },
    /// Радиальный градиент — круг с центром `(cx,cy)` и радиусом `r`.
    /// Упрощение по сравнению с полным SVG `<radialGradient>` (который
    /// поддерживает отдельный "фокус" `fx,fy` и эллиптичность через
    /// `gradientTransform`) — покрывает подавляющее большинство
    /// практических случаев (свечение, объём), полная версия — отдельная
    /// задача при первом реальном запросе на неё.
    Radial { cx: f32, cy: f32, r: f32 },
}

/// Именованное переиспользуемое определение градиента (раздел 60 ТЗ) —
/// хранится в `VectorDoc::gradients`, на него ссылаются фигуры через
/// `fill_gradient: Option<String>` (см. `VectorShape`) по имени, тем же
/// приёмом, что и `SymbolDef`/`VectorShape::Instance` (раздел 28 ТЗ) —
/// правка `GradientDef` сразу отражается на всех фигурах, ссылающихся на
/// него по имени, а не на закэшированной копии.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientDef {
    pub name: String,
    pub kind: GradientKind,
    /// Хотя бы одна остановка нужна для осмысленного градиента — пустой
    /// список — валидное (не паникующее) состояние: `sample()` тогда
    /// возвращает нейтральный серый, `average_color()` — тоже, оба
    /// безопасных фоллбека, а не паника на пустом крайнем случае.
    pub stops: Vec<GradientStop>,
}

impl GradientDef {
    pub fn new(name: impl Into<String>, kind: GradientKind, stops: Vec<GradientStop>) -> Self {
        Self {
            name: name.into(),
            kind,
            stops,
        }
    }

    /// Цвет градиента в позиции `t` (после проекции точки на его ось —
    /// см. `gradient_t_at`), с интерполяцией между двумя ближайшими
    /// остановками. Остановки сортируются по `offset` при каждом вызове
    /// (см. комментарий у `GradientStop` — порядок хранения не инвариант);
    /// для реального редактора (мало остановок, максимум единицы-десятки)
    /// это дешевле, чем поддерживать инвариант отсортированности на
    /// каждой мутации списка из GUI.
    pub fn sample(&self, t: f32) -> RgbaColor {
        if self.stops.is_empty() {
            return RgbaColor::new(128, 128, 128, 255); // безопасный нейтральный фоллбек, не паника
        }
        if self.stops.len() == 1 {
            return self.stops[0].color;
        }
        let mut sorted = self.stops.clone();
        sorted.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let t = t.clamp(0.0, 1.0);
        if t <= sorted[0].offset {
            return sorted[0].color;
        }
        if t >= sorted[sorted.len() - 1].offset {
            return sorted[sorted.len() - 1].color;
        }
        for pair in sorted.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t >= a.offset && t <= b.offset {
                let span = (b.offset - a.offset).max(1e-6);
                return a.color.lerp(b.color, (t - a.offset) / span);
            }
        }
        sorted[sorted.len() - 1].color // недостижимо при валидных offset, но безопасный фоллбек вместо паники
    }

    /// Единый плоский цвет-приближение всего градиента — используется там,
    /// где нужен ровно один `RgbaColor` (например `VectorShape::fill` как
    /// safe-фоллбек для потребителей, которые ещё не знают о градиентах, и
    /// egui-превью Ellipse на Stage, где точный per-vertex градиент по
    /// кругу не реализован — см. `pony-gui`). Простое среднее по стопам
    /// (не взвешенное по длине сегментов между ними) — достаточно для
    /// approximation, не претендует быть тем же, что и настоящий рендер.
    pub fn average_color(&self) -> RgbaColor {
        if self.stops.is_empty() {
            return RgbaColor::new(128, 128, 128, 255);
        }
        let n = self.stops.len() as u32;
        let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
        for s in &self.stops {
            r += s.color.r as u32;
            g += s.color.g as u32;
            b += s.color.b as u32;
            a += s.color.a as u32;
        }
        RgbaColor::new((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8)
    }

    fn to_svg_defs(&self) -> String {
        let mut out = String::new();
        match self.kind {
            GradientKind::Linear { x1, y1, x2, y2 } => {
                out.push_str(&format!(
                    r#"<linearGradient id="gradient_{}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" gradientUnits="userSpaceOnUse">"#,
                    self.name
                ));
            }
            GradientKind::Radial { cx, cy, r } => {
                out.push_str(&format!(
                    r#"<radialGradient id="gradient_{}" cx="{cx}" cy="{cy}" r="{r}" gradientUnits="userSpaceOnUse">"#,
                    self.name
                ));
            }
        }
        out.push('\n');
        for stop in &self.stops {
            out.push_str(&format!(
                r#"<stop offset="{}" stop-color="{}" stop-opacity="{:.3}"/>"#,
                stop.offset,
                stop.color.to_hex(),
                stop.color.opacity()
            ));
            out.push('\n');
        }
        match self.kind {
            GradientKind::Linear { .. } => out.push_str("</linearGradient>\n"),
            GradientKind::Radial { .. } => out.push_str("</radialGradient>\n"),
        }
        out
    }
}

/// Спроецировать точку `(x, y)` на ось градиента, вернуть параметр `t`
/// (НЕ зажатый в `[0,1]` — зажим делает `GradientDef::sample`, здесь чистая
/// проекция, отдельно тестируемая формула): для `Linear` — скалярная
/// проекция на отрезок `(x1,y1)-(x2,y2)`, для `Radial` — доля пройденного
/// радиуса от центра. Публичная — используется и `pony-render`/GUI-превью
/// (для честного приближения градиента per-vertex цветом в egui), и
/// потенциально финальным SVG-рендером тестами (сверка с тем, что видит
/// resvg).
pub fn gradient_t_at(kind: &GradientKind, x: f32, y: f32) -> f32 {
    match *kind {
        GradientKind::Linear { x1, y1, x2, y2 } => {
            let (dx, dy) = (x2 - x1, y2 - y1);
            let len_sq = dx * dx + dy * dy;
            if len_sq < 1e-9 {
                return 0.0; // вырожденный градиент (точка вместо отрезка) — безопасный фоллбек, не деление на 0
            }
            ((x - x1) * dx + (y - y1) * dy) / len_sq
        }
        GradientKind::Radial { cx, cy, r } => {
            if r.abs() < 1e-9 {
                return 0.0;
            }
            let dist = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            dist / r
        }
    }
}

/// Тип узла path (раздел 9 ТЗ) — управляет тем, как редактор ведёт себя
/// при перетаскивании ручек (handle) в Node Tool:
/// - `Corner` — ручки независимы, острый излом (типичный угол фигуры).
/// - `Smooth` — ручки на одной прямой через узел, но могут быть разной
///   длины (плавный переход, но с разным "напряжением" по обе стороны).
/// - `Symmetric` — ручки на одной прямой И одинаковой длины (полностью
///   симметричная кривая по обе стороны узла).
/// - `AutoSmooth` — ручки не хранятся вручную, а вычисляются автоматически
///   из соседних узлов (по Catmull-Rom-подобной схеме) — типичный "узел
///   без ручек" в большинстве векторных редакторов.
///
/// Сама модель ХРАНИТ ручки для всех типов одинаково (`in_handle`/
/// `out_handle` как абсолютные точки, не относительные смещения) — тип
/// узла влияет только на то, как редактор СИНХРОНИЗИРУЕТ ручки при
/// перетаскивании одной из них (см. `PathNode::sync_handles_after_drag`),
/// не на то, как узел сериализуется или рендерится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    Corner,
    Smooth,
    Symmetric,
    AutoSmooth,
}

/// Узел path — раздел 9 ТЗ: "position, in_handle, out_handle, node_type".
/// `in_handle`/`out_handle` — абсолютные координаты (не смещения от
/// `position`) контрольных точек входящей/исходящей кривой Безье. `None`
/// у обоих — узел соединяется с соседями прямыми линиями (`L`), не
/// кривыми (`C`) — так путь может смешивать прямые и кривые сегменты, как
/// того требует раздел 8 (поддержка и `L`, и `C`/`Q`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathNode {
    pub position: (f32, f32),
    pub in_handle: Option<(f32, f32)>,
    pub out_handle: Option<(f32, f32)>,
    pub node_type: NodeType,
}

impl PathNode {
    /// Узел-угол без ручек — узлы прямых отрезков между соседями.
    pub fn corner(position: (f32, f32)) -> Self {
        Self {
            position,
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        }
    }

    /// Симметричный узел с ручками на расстоянии `handle_len` по обе
    /// стороны вдоль направления `(dx, dy)` — удобный конструктор для
    /// программной генерации гладких кривых (например, автосглаживание).
    pub fn symmetric(position: (f32, f32), direction: (f32, f32), handle_len: f32) -> Self {
        let len = (direction.0 * direction.0 + direction.1 * direction.1)
            .sqrt()
            .max(1e-6);
        let (dx, dy) = (
            direction.0 / len * handle_len,
            direction.1 / len * handle_len,
        );
        Self {
            position,
            in_handle: Some((position.0 - dx, position.1 - dy)),
            out_handle: Some((position.0 + dx, position.1 + dy)),
            node_type: NodeType::Symmetric,
        }
    }

    /// После перетаскивания одной ручки — подтянуть вторую в соответствии
    /// с `node_type` (раздел 9: "изменение handle" должно уважать тип
    /// узла, не просто двигать одну точку независимо). Вызывается GUI
    /// сразу после того, как пользователь передвинул `in_handle` ИЛИ
    /// `out_handle` — какую из них подтягивать, определяется тем, какая
    /// STAYED (осталась той, что не двигали).
    pub fn sync_handles_after_drag(&mut self, moved_in: bool) {
        match self.node_type {
            NodeType::Corner => {} // ручки независимы — ничего не подтягиваем
            NodeType::Smooth | NodeType::Symmetric => {
                let (moved, other) = if moved_in {
                    (self.in_handle, &mut self.out_handle)
                } else {
                    (self.out_handle, &mut self.in_handle)
                };
                if let (Some(moved), Some(other_val)) = (moved, other.as_mut()) {
                    let dir = (self.position.0 - moved.0, self.position.1 - moved.1);
                    let target_len = if self.node_type == NodeType::Symmetric {
                        (dir.0 * dir.0 + dir.1 * dir.1).sqrt()
                    } else {
                        // Smooth: сохраняем ТЕКУЩУЮ длину другой ручки —
                        // только направление подстраивается под движение.
                        let odx = other_val.0 - self.position.0;
                        let ody = other_val.1 - self.position.1;
                        (odx * odx + ody * ody).sqrt()
                    };
                    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
                    other_val.0 = self.position.0 + dir.0 / len * target_len;
                    other_val.1 = self.position.1 + dir.1 / len * target_len;
                }
            }
            NodeType::AutoSmooth => {} // ручки для этого узла не хранятся вручную вообще
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VectorShape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: RgbaColor,
        stroke: RgbaColor,
        stroke_width: f32,
        /// Раздел 60 ТЗ (Gradients). `Some(name)` — заливка берётся из
        /// `VectorDoc::gradients` по имени вместо плоского `fill`, если имя
        /// резолвится (см. `resolved_fill_paint`); `fill` остаётся
        /// safe-фоллбеком (примерный плоский цвет — см. `GradientDef::
        /// average_color`) для потребителей, которые градиент ещё не
        /// умеют/не хотят учитывать, и для честного отката при битой/
        /// удалённой ссылке (тот же приём, что `clip_by`/`symbol` у масок
        /// и символов — не паника, откат на видимое поведение).
        /// `#[serde(default)]` — старые `.asset`/`.svg` без градиентов
        /// продолжают читаться как раньше.
        #[serde(default)]
        fill_gradient: Option<String>,
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        fill: RgbaColor,
        stroke: RgbaColor,
        stroke_width: f32,
        #[serde(default)]
        fill_gradient: Option<String>,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke: RgbaColor,
        stroke_width: f32,
    },
    /// Свободная линия (инструмент Pencil/Brush) — набор точек, соединённых
    /// отрезками. Не заливается (только обводка) — как и в Animate/Moho,
    /// у произвольной кривой без замыкания заливка не имеет смысла.
    Polyline {
        points: Vec<(f32, f32)>,
        stroke: RgbaColor,
        stroke_width: f32,
    },
    /// Замкнутый многоугольник (инструмент Pen — по двойному клику рядом с
    /// первой точкой — и PolyStar). В отличие от Polyline — заливается.
    Polygon {
        points: Vec<(f32, f32)>,
        fill: RgbaColor,
        stroke: RgbaColor,
        stroke_width: f32,
        #[serde(default)]
        fill_gradient: Option<String>,
    },
    /// Полноценный path с узлами Безье (разделы 8-9 ТЗ) — структурированная
    /// геометрия (`Vec<PathNode>`), не просто строка `d`. Сериализуется в
    /// честные SVG path-команды: `M` для первого узла, `C` между двумя
    /// узлами, у КАЖДОГО из которых есть хотя бы одна ручка со стороны
    /// сегмента, иначе `L` (прямая) — путь может свободно смешивать прямые
    /// и кривые участки, как того требует раздел 8. `Z` в конце, если
    /// `closed`.
    Path {
        nodes: Vec<PathNode>,
        closed: bool,
        fill: RgbaColor,
        stroke: RgbaColor,
        stroke_width: f32,
        #[serde(default)]
        fill_gradient: Option<String>,
    },
    /// Symbol Instance (раздел 28 ТЗ: "Symbol — reusable object" +
    /// раздел 95: "Symbol instance overrides"). Ссылается на переиспользуемое
    /// определение по имени (`VectorDoc::symbols`), а не копирует геометрию —
    /// правка `SymbolDef` сразу отражается на ВСЕХ инстансах, ссылающихся на
    /// неё (кроме полей, явно переопределённых через `fill_override`, как
    /// того требует раздел 95). `transform` — собственное положение/поворот/
    /// масштаб/отражение этого конкретного инстанса поверх геометрии символа
    /// (например `Eye_L`/`Eye_R` — одно определение `Eye`, два инстанса с
    /// разным `transform.0` (scale_x: -1 для зеркалирования), см. раздел 95).
    /// Хранится как плоская 2x3-матрица `(a,b,c,d,e,f)` — тот же порядок
    /// коэффициентов, что и `Transform2x3`/SVG `matrix()`, но без завязки на
    /// приватный тип (см. `symbol_instance_transform_apply`).
    Instance {
        symbol: String,
        transform: (f32, f32, f32, f32, f32, f32),
        /// `None` — использовать заливку(и) фигур символа как есть.
        /// `Some(color)` — переопределить заливку ВСЕХ заливаемых фигур
        /// символа этим цветом только для этого инстанса (раздел 95:
        /// "overrides не изменяют исходный Symbol Definition"). Точечное
        /// переопределение отдельных вложенных фигур по индексу — за
        /// пределами этой версии (см. `unsupported`-заметку в README) —
        /// целиком-символьный override уже покрывает основной практический
        /// случай (перекрасить весь переиспользуемый объект для конкретного
        /// использования, не создавая второе определение).
        fill_override: Option<RgbaColor>,
    },
}

/// Переиспользуемое определение (раздел 28 ТЗ: "Symbol Definition") —
/// именованная группа фигур, на которую могут ссылаться сколько угодно
/// `VectorShape::Instance` в этом же документе. Хранится в `VectorDoc::symbols`
/// отдельно от `shapes` (список верхнего уровня — это Z-порядок отрисовки
/// документа, а определения символов сами по себе не рисуются, пока на них
/// нет хотя бы одного инстанса — как `<symbol>` внутри `<defs>` в SVG).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDef {
    pub name: String,
    pub shapes: Vec<VectorShape>,
}

impl SymbolDef {
    /// Габарит определения в его СОБСТВЕННЫХ координатах (до применения
    /// transform инстанса) — публичный: нужен GUI, чтобы, например,
    /// посчитать центр символа при создании нового инстанса (раздел 28 ТЗ,
    /// "New Symbol" — новый инстанс обычно ставится так, чтобы его центр
    /// совпадал с точкой клика, а не левый верхний угол).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        if self.shapes.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for shape in &self.shapes {
            let (x0, y0, x1, y1) = shape.bounds();
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
        }
        (min_x, min_y, max_x, max_y)
    }
}

/// Применить плоскую 2x3-матрицу `(a,b,c,d,e,f)` инстанса к точке — та же
/// формула, что `Transform2x3::apply`, продублирована здесь намеренно:
/// `VectorShape::Instance.transform` — публичное поле (сериализуется в RON
/// как часть ассетов), а `Transform2x3` — приватный деталь реализации
/// парсера `from_svg_str`, они сознательно не связаны одним типом.
fn symbol_instance_transform_apply(t: (f32, f32, f32, f32, f32, f32), p: (f32, f32)) -> (f32, f32) {
    (t.0 * p.0 + t.2 * p.1 + t.4, t.1 * p.0 + t.3 * p.1 + t.5)
}

/// Масштаб-множитель для радиусов/ширин обводки при применении transform
/// инстанса — см. `Transform2x3::scale_factor` (та же формула, тот же повод
/// для дублирования).
fn symbol_instance_transform_scale(t: (f32, f32, f32, f32, f32, f32)) -> f32 {
    ((t.0 * t.0 + t.1 * t.1).sqrt() + (t.2 * t.2 + t.3 * t.3).sqrt()) / 2.0
}

/// Применить transform инстанса (и, если задан, `fill_override`) ко ВСЕМ
/// фигурам определения символа, возвращая их уже в "мировых" координатах
/// документа — используется и превью (GUI), и сериализацией в SVG (через
/// разворачивание `<use>` в инлайновую копию, см. `to_svg_element`), чтобы
/// оба пути рисовали инстанс идентично одной и той же логикой, не двумя
/// параллельными реализациями, которые могли бы разойтись.
///
/// Рекурсивные инстансы (символ, ссылающийся на самого себя, напрямую или
/// через цепочку) намеренно НЕ разворачиваются дальше `depth_budget` уровней
/// — это защита от бесконечной рекурсии/зависания на некорректных данных,
/// не поддерживаемая фича; `depth_budget` исчерпан — инстанс просто ничего
/// не рисует на этом уровне (безопасный отказ, не паника).
pub fn resolve_symbol_instance(
    doc: &VectorDoc,
    symbol: &str,
    transform: (f32, f32, f32, f32, f32, f32),
    fill_override: Option<RgbaColor>,
    depth_budget: u8,
) -> Vec<VectorShape> {
    let Some(def) = doc.symbols.iter().find(|s| s.name == symbol) else {
        return Vec::new(); // ссылка на несуществующий символ (например, был удалён) — безопасный отказ, не паника
    };
    if depth_budget == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(def.shapes.len());
    for shape in &def.shapes {
        match shape {
            VectorShape::Instance {
                symbol: inner_symbol,
                transform: inner_t,
                fill_override: inner_override,
            } => {
                // Вложенный инстанс — сначала выражаем ЕГО transform в
                // координатах внешнего символа (композиция матриц), потом
                // рекурсивно резолвим тем же способом с урезанным бюджетом.
                let composed = compose_instance_transforms(transform, *inner_t);
                let effective_override = fill_override.or(*inner_override);
                out.extend(resolve_symbol_instance(
                    doc,
                    inner_symbol,
                    composed,
                    effective_override,
                    depth_budget - 1,
                ));
            }
            other => {
                let mut resolved = transform_shape(other, transform);
                if let Some(color) = fill_override {
                    resolved.set_fill(color);
                }
                out.push(resolved);
            }
        }
    }
    out
}

/// Композиция `outer` (transform инстанса) со `inner` (transform фигуры/
/// вложенного инстанса ВНУТРИ символа) — тот же порядок накопления, что
/// `Transform2x3::then` в парсере (родитель применяется ПОСЛЕ ребёнка).
fn compose_instance_transforms(
    outer: (f32, f32, f32, f32, f32, f32),
    inner: (f32, f32, f32, f32, f32, f32),
) -> (f32, f32, f32, f32, f32, f32) {
    let (a1, b1, c1, d1, e1, f1) = inner;
    let (a2, b2, c2, d2, e2, f2) = outer;
    (
        a2 * a1 + c2 * b1,
        b2 * a1 + d2 * b1,
        a2 * c1 + c2 * d1,
        b2 * c1 + d2 * d1,
        a2 * e1 + c2 * f1 + e2,
        b2 * e1 + d2 * f1 + f2,
    )
}

/// Применить transform ко всем координатам фигуры, возвращая новую фигуру —
/// используется при разворачивании Symbol Instance (см. `resolve_symbol_instance`).
/// Паникует на `VectorShape::Instance` (см. `debug_assert` внутри) — вызывающая
/// сторона (`resolve_symbol_instance`) обрабатывает Instance отдельной веткой
/// ДО вызова этой функции, она никогда не должна получить Instance на вход.
fn transform_shape(shape: &VectorShape, t: (f32, f32, f32, f32, f32, f32)) -> VectorShape {
    let scale = symbol_instance_transform_scale(t);
    match shape {
        // fill_gradient намеренно НЕ переносится на трансформированную копию
        // ниже (Rect/Ellipse/Polygon/Path) — раздел 60 ТЗ (Gradients) x
        // раздел 28 ТЗ (Symbols): `fill_gradient` ссылается на `GradientDef`
        // в АБСОЛЮТНЫХ document-space координатах (см. `GradientKind` —
        // `gradientUnits="userSpaceOnUse"`), а не в локальных координатах
        // символа. Резолвинг инстанса переносит геометрию фигуры в мировые
        // координаты через `transform`, но сам `GradientDef` при этом не
        // трогается и не копируется под новым именем — оставить ссылку как
        // есть означало бы, что фигура сдвинулась/повернулась/
        // отмасштабировалась, а градиент на ней остался "приклеен" к старым
        // координатам документа, визуально разъезжаясь с формой при любом
        // transform ≠ identity. Честный выбор — не рисовать неправильно
        // совмещённый градиент молча, а откатиться на плоский цвет (уже
        // вычисленный в `fill` — см. `set_fill_gradient`, который держит
        // `fill` синхронизированным со средним цветом градиента). Полная
        // поддержка (перенос геометрии градиента вместе с transform
        // инстанса) — отдельная задача при первом реальном запросе на неё.
        VectorShape::Rect {
            x,
            y,
            w,
            h,
            fill,
            stroke,
            stroke_width,
            fill_gradient: _,
        } => {
            let (p0, p1) = (
                symbol_instance_transform_apply(t, (*x, *y)),
                symbol_instance_transform_apply(t, (x + w, y + h)),
            );
            let (x0, x1) = (p0.0.min(p1.0), p0.0.max(p1.0));
            let (y0, y1) = (p0.1.min(p1.1), p0.1.max(p1.1));
            VectorShape::Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
                fill: *fill,
                stroke: *stroke,
                stroke_width: stroke_width * scale,
                fill_gradient: None,
            }
        }
        VectorShape::Ellipse {
            cx,
            cy,
            rx,
            ry,
            fill,
            stroke,
            stroke_width,
            fill_gradient: _,
        } => {
            let center = symbol_instance_transform_apply(t, (*cx, *cy));
            VectorShape::Ellipse {
                cx: center.0,
                cy: center.1,
                rx: rx * scale,
                ry: ry * scale,
                fill: *fill,
                stroke: *stroke,
                stroke_width: stroke_width * scale,
                fill_gradient: None,
            }
        }
        VectorShape::Line {
            x1,
            y1,
            x2,
            y2,
            stroke,
            stroke_width,
        } => {
            let p1 = symbol_instance_transform_apply(t, (*x1, *y1));
            let p2 = symbol_instance_transform_apply(t, (*x2, *y2));
            VectorShape::Line {
                x1: p1.0,
                y1: p1.1,
                x2: p2.0,
                y2: p2.1,
                stroke: *stroke,
                stroke_width: stroke_width * scale,
            }
        }
        VectorShape::Polyline {
            points,
            stroke,
            stroke_width,
        } => VectorShape::Polyline {
            points: points
                .iter()
                .map(|p| symbol_instance_transform_apply(t, *p))
                .collect(),
            stroke: *stroke,
            stroke_width: stroke_width * scale,
        },
        VectorShape::Polygon {
            points,
            fill,
            stroke,
            stroke_width,
            fill_gradient: _,
        } => VectorShape::Polygon {
            points: points
                .iter()
                .map(|p| symbol_instance_transform_apply(t, *p))
                .collect(),
            fill: *fill,
            stroke: *stroke,
            stroke_width: stroke_width * scale,
            fill_gradient: None,
        },
        VectorShape::Path {
            nodes,
            closed,
            fill,
            stroke,
            stroke_width,
            fill_gradient: _,
        } => VectorShape::Path {
            nodes: nodes
                .iter()
                .map(|n| PathNode {
                    position: symbol_instance_transform_apply(t, n.position),
                    in_handle: n.in_handle.map(|h| symbol_instance_transform_apply(t, h)),
                    out_handle: n.out_handle.map(|h| symbol_instance_transform_apply(t, h)),
                    node_type: n.node_type,
                })
                .collect(),
            closed: *closed,
            fill: *fill,
            stroke: *stroke,
            stroke_width: stroke_width * scale,
            fill_gradient: None,
        },
        VectorShape::Instance { .. } => {
            debug_assert!(false, "transform_shape вызван на Instance — вызывающая сторона (resolve_symbol_instance) должна обрабатывать Instance отдельной веткой");
            shape.clone()
        }
    }
}

/// Раздел 60 ТЗ (Gradients): решает, чем красить фигуру в сериализованном
/// SVG — `url(#gradient_<name>)`, если фигура ссылается на градиент И он
/// реально существует в `VectorDoc::gradients` (`known_gradients`), иначе
/// обычный `fill`. Тот же принцип "честного отката при висячей ссылке",
/// что и у `Instance`/символов и `Part::clip_by`/масок — битая/удалённая
/// ссылка на градиент не должна привести к ссылке на несуществующий
/// `<linearGradient>`/`<radialGradient>` в сохранённом файле (браузер/resvg
/// в этом случае обычно просто не красят фигуру вообще — гораздо хуже, чем
/// видимый, пусть и не идеальный, плоский цвет-фоллбек).
fn resolved_fill_paint(
    fill: RgbaColor,
    fill_gradient: &Option<String>,
    known_gradients: &std::collections::HashSet<&str>,
) -> (String, f32) {
    if let Some(name) = fill_gradient {
        if known_gradients.contains(name.as_str()) {
            return (format!("url(#gradient_{name})"), 1.0);
        }
    }
    (fill.to_hex(), fill.opacity())
}

impl VectorShape {
    fn to_svg_element(&self, known_gradients: &std::collections::HashSet<&str>) -> String {
        match self {
            VectorShape::Rect {
                x,
                y,
                w,
                h,
                fill,
                stroke,
                stroke_width,
                fill_gradient,
            } => {
                let (fill_paint, fill_op) =
                    resolved_fill_paint(*fill, fill_gradient, known_gradients);
                format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill_paint}" fill-opacity="{fill_op:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Ellipse {
                cx,
                cy,
                rx,
                ry,
                fill,
                stroke,
                stroke_width,
                fill_gradient,
            } => {
                let (fill_paint, fill_op) =
                    resolved_fill_paint(*fill, fill_gradient, known_gradients);
                format!(
                    r#"<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="{fill_paint}" fill-opacity="{fill_op:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Line {
                x1,
                y1,
                x2,
                y2,
                stroke,
                stroke_width,
            } => format!(
                r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round"/>"#,
                stroke.to_hex(),
                stroke.opacity()
            ),
            VectorShape::Polyline {
                points,
                stroke,
                stroke_width,
            } => {
                let pts = points
                    .iter()
                    .map(|(x, y)| format!("{x},{y}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    r#"<polyline points="{pts}" fill="none" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Polygon {
                points,
                fill,
                stroke,
                stroke_width,
                fill_gradient,
            } => {
                let pts = points
                    .iter()
                    .map(|(x, y)| format!("{x},{y}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let (fill_paint, fill_op) =
                    resolved_fill_paint(*fill, fill_gradient, known_gradients);
                format!(
                    r#"<polygon points="{pts}" fill="{fill_paint}" fill-opacity="{fill_op:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linejoin="round"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Path {
                nodes,
                closed,
                fill,
                stroke,
                stroke_width,
                fill_gradient,
            } => {
                let d = path_data_string(nodes, *closed);
                let (fill_paint, fill_op) = if *closed {
                    resolved_fill_paint(*fill, fill_gradient, known_gradients)
                } else {
                    ("none".to_string(), 0.0)
                };
                format!(
                    r#"<path d="{d}" fill="{fill_paint}" fill-opacity="{fill_op:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Instance {
                symbol,
                transform,
                fill_override,
            } => {
                // Настоящий `<use>` со ссылкой `href="#symbol_<name>"` на
                // `<symbol>`, который `to_svg_string` пишет один раз в
                // `<defs>` для каждого имени из `VectorDoc::symbols` (см.
                // `VectorDoc::to_svg_string`) — resvg (уже используемый этим
                // движком для импорта/финального рендера, см. pony-render)
                // понимает `<use>` нативно, поэтому сохранённый .svg рисует
                // символ ПРАВИЛЬНО через настоящий SVG-механизм переиспользования,
                // а не через инлайн-копию. `fill_override`, если задан,
                // передаётся через CSS-каскад (`style="fill:..."` на самом
                // `<use>` перекрывает `fill` фигур внутри `<symbol>`, если
                // они сами не задают fill литералом сильнее — совпадает с
                // тем, как override резолвится в превью/`resolve_symbol_instance`,
                // см. раздел 95 ТЗ).
                let (a, b, c, d, e, f) = transform;
                let style = match fill_override {
                    Some(color) => format!(
                        r#" style="fill:{};fill-opacity:{:.3}""#,
                        color.to_hex(),
                        color.opacity()
                    ),
                    None => String::new(),
                };
                format!(
                    r##"<use href="#symbol_{symbol}" transform="matrix({a} {b} {c} {d} {e} {f})"{style}/>"##
                )
            }
        }
    }

    /// Габарит фигуры (min_x, min_y, max_x, max_y) — нужен, чтобы посчитать
    /// viewBox документа под реально нарисованное, а не гадать размер заранее.
    fn bounds(&self) -> (f32, f32, f32, f32) {
        match self {
            VectorShape::Rect { x, y, w, h, .. } => (*x, *y, x + w, y + h),
            VectorShape::Ellipse { cx, cy, rx, ry, .. } => (cx - rx, cy - ry, cx + rx, cy + ry),
            VectorShape::Line { x1, y1, x2, y2, .. } => {
                (x1.min(*x2), y1.min(*y2), x1.max(*x2), y1.max(*y2))
            }
            VectorShape::Polyline { points, .. } | VectorShape::Polygon { points, .. } => {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                for (x, y) in points {
                    min_x = min_x.min(*x);
                    min_y = min_y.min(*y);
                    max_x = max_x.max(*x);
                    max_y = max_y.max(*y);
                }
                (min_x, min_y, max_x, max_y)
            }
            VectorShape::Path { nodes, .. } => {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                // Учитываем и ручки, не только позиции узлов — кривая
                // Безье может "выпучиться" за пределы прямой между узлами,
                // и габарит только по позициям обрезал бы такую кривую.
                // Не точная геометрическая граница кривой (это отдельная,
                // более дорогая задача), но безопасный верхний предел —
                // выпуклая оболочка узлов и их ручек её гарантированно
                // содержит (свойство кривых Безье: кривая всегда лежит
                // внутри выпуклой оболочки своих контрольных точек).
                for node in nodes {
                    for pt in [Some(node.position), node.in_handle, node.out_handle]
                        .into_iter()
                        .flatten()
                    {
                        min_x = min_x.min(pt.0);
                        min_y = min_y.min(pt.1);
                        max_x = max_x.max(pt.0);
                        max_y = max_y.max(pt.1);
                    }
                }
                (min_x, min_y, max_x, max_y)
            }
            VectorShape::Instance { transform, .. } => {
                // Точный габарит инстанса требует резолвинга символа по
                // имени (`VectorDoc::symbols`), а этот метод — приватный
                // и без доступа к документу (см. `resolve_symbol_instance`
                // для точного варианта, используемого превью/hit-тестом на
                // уровне GUI, у которого доступ к документу есть). Здесь —
                // безопасное вырожденное приближение: точка переноса
                // инстанса. Единственные потребители этой ветки —
                // `contains_point` (сравнительно неважно для инстансов на
                // верхнем уровне документа — GUI использует свой собственный,
                // точный hit-тест через `resolve_symbol_instance`, см.
                // `pony-gui`) и `VectorDoc::bounds_with_padding` (viewBox
                // документа) — оба остаются в разумных пределах: реальный
                // габарит уходит из-под учёта только для документов, где
                // Instance стоит прямо в `shapes` (не внутри символа) И это
                // единственная/крайняя фигура документа, что для персонажных
                // ассетов практически не встречается (инстансы там —
                // содержимое символов, не самостоятельные элементы сцены).
                (transform.4, transform.5, transform.4, transform.5)
            }
        }
    }

    /// Попадает ли точка (в тех же SVG-координатах, что и сама фигура) в её
    /// габарит. Намеренно по bounding box, не по точной геометрии — тот же
    /// уровень точности, что и у hit-теста частей персонажа на Stage
    /// (см. `part_world_position`/`nominal_part_size` в pony-render):
    /// для инструментов PaintBucket/InkBottle, где нужно просто «попасть
    /// по примерно этой фигуре», точная геометрия — лишняя сложность.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let (min_x, min_y, max_x, max_y) = self.bounds();
        x >= min_x && x <= max_x && y >= min_y && y <= max_y
    }

    /// Задать заливку (для фигур без заливки — Line/Polyline — не действует).
    /// На `Instance` устанавливает `fill_override` этого конкретного
    /// инстанса (раздел 95 ТЗ) — не трогает `SymbolDef`, на который он
    /// ссылается, то есть остальные инстансы того же символа не меняются.
    pub fn set_fill(&mut self, color: RgbaColor) {
        match self {
            // Явный плоский цвет из color picker'а отменяет градиентную
            // заливку (раздел 60 ТЗ) — тот же принцип, что в большинстве
            // векторных редакторов: выбор сплошного цвета переключает
            // фигуру обратно на solid fill, не оставляет "мёртвую" ссылку
            // на градиент рядом с новым `fill`, которая продолжила бы
            // побеждать при рендере (см. `resolved_fill_paint`).
            VectorShape::Rect {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Ellipse {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Polygon {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Path {
                fill,
                fill_gradient,
                ..
            } => {
                *fill = color; // на Path действует только если closed — см. to_svg_element
                *fill_gradient = None;
            }
            VectorShape::Instance { fill_override, .. } => *fill_override = Some(color),
            VectorShape::Line { .. } | VectorShape::Polyline { .. } => {}
        }
    }

    /// Раздел 60 ТЗ (Gradients): назначить/снять градиентную заливку.
    /// `Some(name)` — фигура красится градиентом `name` из `VectorDoc::
    /// gradients` при рендере/сериализации (см. `resolved_fill_paint`),
    /// если он резолвится; `None` — снять градиент, вернуться к плоскому
    /// `fill`. `fallback_color` в обоих случаях становится новым видимым
    /// `fill` — единственная точка правды для всех потребителей, которые
    /// градиент не учитывают (старые сериализаторы, части превью и т.п.),
    /// и честный откат при висячей ссылке; вызывающая сторона (GUI) обычно
    /// передаёт `GradientDef::average_color()` выбранного градиента. Не
    /// действует на Line/Polyline (нет заливки вообще) и Instance — точечный
    /// градиент-override инстанса не поддержан в этой версии, тот же класс
    /// ограничения, что и per-instance stroke override (раздел 95 ТЗ).
    pub fn set_fill_gradient(&mut self, gradient_name: Option<String>, fallback_color: RgbaColor) {
        match self {
            VectorShape::Rect {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Ellipse {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Polygon {
                fill,
                fill_gradient,
                ..
            }
            | VectorShape::Path {
                fill,
                fill_gradient,
                ..
            } => {
                *fill_gradient = gradient_name;
                *fill = fallback_color;
            }
            VectorShape::Line { .. }
            | VectorShape::Polyline { .. }
            | VectorShape::Instance { .. } => {}
        }
    }

    /// Имя градиента, которым сейчас залита фигура — `None`, если фигура
    /// без заливки вообще, залита плоским цветом, или это `Instance`
    /// (градиент-override инстансов не поддержан, см. `set_fill_gradient`).
    /// Используется GUI, чтобы показать текущий выбор в панели заливки.
    pub fn fill_gradient_name(&self) -> Option<&str> {
        match self {
            VectorShape::Rect { fill_gradient, .. }
            | VectorShape::Ellipse { fill_gradient, .. }
            | VectorShape::Polygon { fill_gradient, .. }
            | VectorShape::Path { fill_gradient, .. } => fill_gradient.as_deref(),
            VectorShape::Line { .. }
            | VectorShape::Polyline { .. }
            | VectorShape::Instance { .. } => None,
        }
    }

    /// См. `VectorDoc::sync_gradient_fallback_colors` — пересчитывает `fill`
    /// из текущего `fill_gradient` через `lookup`, если он резолвится; не
    /// трогает `fill`, если `fill_gradient` пуст или ссылка висячая (не
    /// паникует — тот же честный откат, что и везде в этом модуле).
    fn sync_fill_from_gradient(&mut self, lookup: &dyn Fn(&str) -> Option<RgbaColor>) {
        if let VectorShape::Rect {
            fill,
            fill_gradient,
            ..
        }
        | VectorShape::Ellipse {
            fill,
            fill_gradient,
            ..
        }
        | VectorShape::Polygon {
            fill,
            fill_gradient,
            ..
        }
        | VectorShape::Path {
            fill,
            fill_gradient,
            ..
        } = self
        {
            if let Some(name) = fill_gradient.as_deref() {
                if let Some(color) = lookup(name) {
                    *fill = color;
                }
            }
        }
    }

    /// Задать цвет обводки — есть у всех фигур, кроме `Instance` (обводка
    /// инстанса определяется фигурами внутри `SymbolDef`, у самого
    /// инстанса нет собственной — только `fill_override`, см. раздел 95 ТЗ:
    /// override для инстанса описан именно для заливки/стиля, не заявлен
    /// отдельно для обводки; расширять до per-instance stroke override —
    /// отдельная задача при первом реальном запросе на неё).
    pub fn set_stroke(&mut self, color: RgbaColor) {
        match self {
            VectorShape::Rect { stroke, .. }
            | VectorShape::Ellipse { stroke, .. }
            | VectorShape::Line { stroke, .. }
            | VectorShape::Polyline { stroke, .. }
            | VectorShape::Polygon { stroke, .. }
            | VectorShape::Path { stroke, .. } => *stroke = color,
            VectorShape::Instance { .. } => {}
        }
    }

    /// Держатели (control points) для интерактивного редактирования формы —
    /// инструмент SubSelection: их и рисуют на Stage как перетаскиваемые
    /// маркеры, и по клику рядом с одним из них определяют, какой индекс
    /// передать в `set_control_point`. Координаты — в той же SVG-системе
    /// (Y вниз), что и сама фигура.
    ///
    /// Rect — 4 угла (порядок: top-left, top-right, bottom-left, bottom-right
    /// в SVG-координатах — "top" здесь значит меньший Y). Ellipse — центр
    /// (двигает всю фигуру) плюс две точки на краях (управляют rx/ry по
    /// отдельности, не через общий "радиус"). Line — оба конца напрямую.
    /// Polyline/Polygon — каждая точка пути напрямую. Path — позиция
    /// каждого узла, затем его `in_handle`/`out_handle`, если они есть
    /// (отсутствующие ручки не возвращаются — узел без ручек не должен
    /// давать перетаскиваемую точку, которой не существует). Порядок
    /// обхода — узел за узлом по порядку в `nodes`, ВСЕГДА один и тот же
    /// между вызовами `control_points()` и `set_control_point()` (см.
    /// `path_control_point_kind` — та же логика прохода, чтобы индексы не
    /// могли разойтись).
    pub fn control_points(&self) -> Vec<(f32, f32)> {
        match self {
            VectorShape::Rect { x, y, w, h, .. } => {
                vec![(*x, *y), (x + w, *y), (*x, y + h), (x + w, y + h)]
            }
            VectorShape::Ellipse { cx, cy, rx, ry, .. } => {
                vec![(*cx, *cy), (cx + rx, *cy), (*cx, cy + ry)]
            }
            VectorShape::Line { x1, y1, x2, y2, .. } => vec![(*x1, *y1), (*x2, *y2)],
            VectorShape::Polyline { points, .. } | VectorShape::Polygon { points, .. } => {
                points.clone()
            }
            VectorShape::Path { nodes, .. } => {
                let mut pts = Vec::new();
                for node in nodes {
                    pts.push(node.position);
                    if let Some(h) = node.in_handle {
                        pts.push(h);
                    }
                    if let Some(h) = node.out_handle {
                        pts.push(h);
                    }
                }
                pts
            }
            // Instance — один держатель (его точка переноса, transform.4/.5)
            // двигает весь инстанс целиком, как перетаскивание группы —
            // поворот/масштаб/отражение инстанса задаются не через
            // control points (тут нет мышиных ручек под них в этой
            // версии), а через GUI-панель Symbol Instance (X/Y/масштаб/
            // поворот полями ввода — см. pony-gui).
            VectorShape::Instance { transform, .. } => vec![(transform.4, transform.5)],
        }
    }

    /// Переместить держатель `index` (см. `control_points`) в новую точку.
    /// Индекс вне диапазона тихо игнорируется — вызывающая сторона (GUI)
    /// сама следит за границами по длине `control_points()`, но не паниковать
    /// на рассинхроне безопаснее, чем полагаться на то, что она не ошибётся.
    pub fn set_control_point(&mut self, index: usize, pos: (f32, f32)) {
        match self {
            VectorShape::Rect { x, y, w, h, .. } => {
                // Противоположный угол должен остаться на месте — иначе
                // перетаскивание одного угла двигало бы всю фигуру, а не
                // меняло её форму. min/max в конце — на случай, если
                // перетащить угол "через" противоположный (фигура не
                // должна вывернуться в отрицательные w/h).
                let (x0, y0, x1, y1) = (*x, *y, *x + *w, *y + *h);
                let (nx0, ny0, nx1, ny1) = match index {
                    0 => (pos.0, pos.1, x1, y1),
                    1 => (x0, pos.1, pos.0, y1),
                    2 => (pos.0, y0, x1, pos.1),
                    3 => (x0, y0, pos.0, pos.1),
                    _ => return,
                };
                *x = nx0.min(nx1);
                *y = ny0.min(ny1);
                *w = (nx1 - nx0).abs().max(1.0);
                *h = (ny1 - ny0).abs().max(1.0);
            }
            VectorShape::Ellipse { cx, cy, rx, ry, .. } => match index {
                0 => {
                    *cx = pos.0;
                    *cy = pos.1;
                }
                1 => *rx = (pos.0 - *cx).abs().max(1.0),
                2 => *ry = (pos.1 - *cy).abs().max(1.0),
                _ => {}
            },
            VectorShape::Line { x1, y1, x2, y2, .. } => match index {
                0 => {
                    *x1 = pos.0;
                    *y1 = pos.1;
                }
                1 => {
                    *x2 = pos.0;
                    *y2 = pos.1;
                }
                _ => {}
            },
            VectorShape::Polyline { points, .. } | VectorShape::Polygon { points, .. } => {
                if let Some(p) = points.get_mut(index) {
                    *p = pos;
                }
            }
            VectorShape::Path { nodes, .. } => {
                let mut i = 0;
                for node in nodes.iter_mut() {
                    if i == index {
                        // Перемещение самой позиции узла тащит обе ручки
                        // вместе с ней на ту же дельту — раздел 9
                        // разделяет "перемещение" (узла целиком) и
                        // "изменение handle" (независимая правка ручки)
                        // как разные операции; если бы позиция двигалась,
                        // а ручки оставались на месте, форма кривой возле
                        // узла ломалась бы при каждом перетаскивании.
                        let delta = (pos.0 - node.position.0, pos.1 - node.position.1);
                        node.position = pos;
                        if let Some(h) = node.in_handle.as_mut() {
                            h.0 += delta.0;
                            h.1 += delta.1;
                        }
                        if let Some(h) = node.out_handle.as_mut() {
                            h.0 += delta.0;
                            h.1 += delta.1;
                        }
                        return;
                    }
                    i += 1;
                    if node.in_handle.is_some() {
                        if i == index {
                            node.in_handle = Some(pos);
                            return;
                        }
                        i += 1;
                    }
                    if node.out_handle.is_some() {
                        if i == index {
                            node.out_handle = Some(pos);
                            return;
                        }
                        i += 1;
                    }
                }
            }
            VectorShape::Instance { transform, .. } => {
                if index == 0 {
                    transform.4 = pos.0;
                    transform.5 = pos.1;
                }
            }
        }
    }

    /// Для Path — какому узлу и какой именно точке (позиция/входящая
    /// ручка/исходящая ручка) соответствует control point с данным
    /// индексом. `None` для остальных фигур (у них нет типизированных
    /// узлов в этом смысле) и для индекса вне диапазона. Нужно GUI, чтобы
    /// после перетаскивания вызвать `PathNode::sync_handles_after_drag` с
    /// правильным аргументом (двигали именно `in_handle` или именно
    /// `out_handle` — только тогда есть смысл подтягивать вторую ручку).
    pub fn path_control_point_kind(&self, index: usize) -> Option<(usize, PathPointKind)> {
        let VectorShape::Path { nodes, .. } = self else {
            return None;
        };
        let mut i = 0;
        for (node_i, node) in nodes.iter().enumerate() {
            if i == index {
                return Some((node_i, PathPointKind::Position));
            }
            i += 1;
            if node.in_handle.is_some() {
                if i == index {
                    return Some((node_i, PathPointKind::InHandle));
                }
                i += 1;
            }
            if node.out_handle.is_some() {
                if i == index {
                    return Some((node_i, PathPointKind::OutHandle));
                }
                i += 1;
            }
        }
        None
    }
}

/// См. `VectorShape::path_control_point_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPointKind {
    Position,
    InHandle,
    OutHandle,
}

/// Документ — упорядоченный список фигур (порядок = порядок отрисовки,
/// как слои: позже добавленная фигура рисуется поверх).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorDoc {
    pub shapes: Vec<VectorShape>,
    /// Переиспользуемые определения (раздел 28 ТЗ: "Symbol — reusable
    /// object") — на них ссылаются `VectorShape::Instance` где угодно в
    /// `shapes` (или внутри других символов, см. `resolve_symbol_instance`).
    /// `#[serde(default)]` — старые документы/`.asset`-файлы без символов
    /// продолжают читаться, просто с пустым списком (обратная совместимость
    /// схемы, тот же приём, что и у `unsupported`/`ik_constraints`).
    #[serde(default)]
    pub symbols: Vec<SymbolDef>,
    /// Именованные определения градиентов (раздел 60 ТЗ: "Gradients") —
    /// на них ссылаются фигуры через `VectorShape`'s `fill_gradient:
    /// Option<String>` по имени, тем же приёмом переиспользования по
    /// ссылке, что и `symbols`/`Instance` выше — правка `GradientDef`
    /// сразу видна на всех фигурах, ссылающихся на него, не на
    /// закэшированной копии. `#[serde(default)]` — старые `.asset`-файлы
    /// без градиентов продолжают читаться.
    #[serde(default)]
    pub gradients: Vec<GradientDef>,
    /// Имена элементов, встреченных при `from_svg_str`, но пока не
    /// поддержанных этой моделью (текст и т.п. — градиенты раньше тоже
    /// попадали сюда, начиная с раздела 60 ТЗ они разбираются полноценно,
    /// см. `gradients` выше) — раздел 29 ТЗ: не молча теряются, а видны
    /// пользователю (GUI показывает список в предупреждении при открытии).
    /// Пустой для документов, не прошедших через `from_svg_str` (созданных
    /// программно/рисованием).
    #[serde(default)]
    pub unsupported: Vec<String>,
}

impl VectorDoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, shape: VectorShape) {
        self.shapes.push(shape);
    }

    /// Создать/заменить именованное определение символа (раздел 28 ТЗ —
    /// "Convert to Symbol": берётся текущий выбор фигур, они становятся
    /// содержимым нового `SymbolDef`, вызывающая сторона (GUI) сама решает,
    /// удалять ли исходные фигуры из `shapes` и добавлять ли вместо них
    /// `Instance`). Одноимённое существующее определение заменяется целиком
    /// (не сливается) — тот же принцип, что и у `add_ik_constraint`/
    /// `find_ik_constraint_mut` в `skeleton.rs`.
    /// Создать/заменить именованный градиент (раздел 60 ТЗ) — тот же
    /// принцип, что `upsert_symbol`: одноимённое существующее определение
    /// заменяется целиком (не сливается по стопам), чтобы редактирование
    /// градиента в панели (добавить/подвинуть/удалить стоп, сменить тип)
    /// было простой единственной точкой записи.
    pub fn upsert_gradient(&mut self, gradient: GradientDef) {
        if let Some(existing) = self.gradients.iter_mut().find(|g| g.name == gradient.name) {
            *existing = gradient;
        } else {
            self.gradients.push(gradient);
        }
    }

    /// Найти градиент по имени — `None`, если такого нет (висячая ссылка
    /// в `fill_gradient` или имя, которое ещё не было создано). Публичный
    /// helper, чтобы GUI не дублировал поиск по `self.gradients` вручную
    /// на каждом месте, где нужно посмотреть/показать текущий градиент
    /// выбранной фигуры.
    pub fn find_gradient(&self, name: &str) -> Option<&GradientDef> {
        self.gradients.iter().find(|g| g.name == name)
    }

    /// Удалить градиент по имени. Фигуры, ссылавшиеся на него через
    /// `fill_gradient`, НЕ обновляются автоматически (их `fill_gradient`
    /// остаётся указывать на теперь несуществующее имя) — это НЕ баг, а
    /// тот же принцип "честного отката", что и у удаления символа/маски:
    /// `resolved_fill_paint`/GUI-превью при рендере просто откатятся на
    /// плоский `fill` фигуры (который остаётся последним известным
    /// приближением цвета градиента), не запаникуют и не потеряют фигуру.
    pub fn remove_gradient(&mut self, name: &str) {
        self.gradients.retain(|g| g.name != name);
    }

    pub fn upsert_symbol(&mut self, name: impl Into<String>, shapes: Vec<VectorShape>) {
        let name = name.into();
        if let Some(existing) = self.symbols.iter_mut().find(|s| s.name == name) {
            existing.shapes = shapes;
        } else {
            self.symbols.push(SymbolDef { name, shapes });
        }
    }

    /// Раздел 95 ТЗ: "Modify → Break Apart Symbol" — разорвать связь
    /// конкретного инстанса `shapes[index]` с его `SymbolDef`, заменив его
    /// на независимую inline-копию геометрии символа (с уже применённым
    /// transform и `fill_override` этого инстанса). После вызова
    /// `shapes[index]` перестаёт существовать как единичный элемент — на
    /// его месте оказываются N отдельных фигур (N = число фигур в символе),
    /// каждая полностью независима: правка `SymbolDef` дальше на них уже не
    /// влияет, как того требует раздел 95. `false`, если по индексу не
    /// `Instance` (нечего разрывать) — тихий отказ, не паника, вызывающая
    /// сторона (GUI) сама решает, как на это реагировать (обычно — просто
    /// не показывать пункт меню для не-инстанса).
    pub fn break_apart_symbol_instance(&mut self, index: usize) -> bool {
        let Some(VectorShape::Instance {
            symbol,
            transform,
            fill_override,
        }) = self.shapes.get(index)
        else {
            return false;
        };
        let resolved = resolve_symbol_instance(self, symbol, *transform, *fill_override, 8);
        if resolved.is_empty() {
            return false;
        }
        self.shapes.splice(index..=index, resolved);
        true
    }

    /// Создать новый `VectorShape::Instance`, ссылающийся на `symbol_name`,
    /// с identity-transform, СМЕЩЁННЫЙ так, чтобы центр габарита символа
    /// оказался в точке `(at_x, at_y)` (а не левый верхний угол его
    /// внутренних координат) — GUI использует это для "поставить символ туда,
    /// куда кликнули" (раздел 28 ТЗ, New Symbol/добавление инстанса из
    /// панели символов). `None`, если `symbol_name` не существует в
    /// `self.symbols` — вызывающая сторона не должна суметь создать
    /// висячую ссылку случайно.
    pub fn new_instance_centered_at(
        &self,
        symbol_name: &str,
        at_x: f32,
        at_y: f32,
    ) -> Option<VectorShape> {
        let def = self.symbols.iter().find(|s| s.name == symbol_name)?;
        let (min_x, min_y, max_x, max_y) = def.bounds();
        let center = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
        Some(VectorShape::Instance {
            symbol: symbol_name.to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, at_x - center.0, at_y - center.1),
            fill_override: None,
        })
    }

    pub fn clear(&mut self) {
        self.shapes.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Индекс самой верхней (последней нарисованной, значит рисуется
    /// поверх остальных) фигуры, чей габарит содержит точку — для
    /// PaintBucket/InkBottle: клик должен попадать в то, что видно сверху,
    /// а не в первую фигуру, оказавшуюся под курсором в списке.
    pub fn shape_at(&self, x: f32, y: f32) -> Option<usize> {
        self.shapes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.contains_point(x, y))
            .map(|(i, _)| i)
    }

    /// Габарит всего документа: `(min_x, min_y, max_x, max_y)` в SVG-системе
    /// координат (Y вниз). Публичный, потому что редактор по нему считает,
    /// куда и какого размера поставить часть, сделанную из этого рисунка.
    /// `None` для пустого документа — размер «ничего» не определён, и
    /// придумывать за вызывающего дефолт здесь неправильно.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        if self.shapes.is_empty() {
            return None;
        }
        let (min_x, min_y, max_x, max_y) = self.bounds_with_padding(0.0);
        Some((min_x, min_y, max_x, max_y))
    }

    /// Габарит всего документа с отступом (padding) по краям — используется
    /// как viewBox, чтобы обводки на границе фигур не обрезались. Для
    /// `Instance` использует ТОЧНЫЙ (не вырожденный) габарит — резолвит
    /// символ через `resolve_symbol_instance`, в отличие от
    /// `VectorShape::bounds()`, у которого нет доступа к `self` (документу)
    /// — здесь он есть, так что нет причин мириться с приближением.
    fn bounds_with_padding(&self, padding: f32) -> (f32, f32, f32, f32) {
        if self.shapes.is_empty() {
            return (0.0, 0.0, 1.0, 1.0);
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for shape in &self.shapes {
            let (x0, y0, x1, y1) = match shape {
                VectorShape::Instance {
                    symbol,
                    transform,
                    fill_override,
                } => {
                    let resolved =
                        resolve_symbol_instance(self, symbol, *transform, *fill_override, 8);
                    if resolved.is_empty() {
                        continue;
                    }
                    let mut rx0 = f32::MAX;
                    let mut ry0 = f32::MAX;
                    let mut rx1 = f32::MIN;
                    let mut ry1 = f32::MIN;
                    for r in &resolved {
                        let (a, b, c, d) = r.bounds();
                        rx0 = rx0.min(a);
                        ry0 = ry0.min(b);
                        rx1 = rx1.max(c);
                        ry1 = ry1.max(d);
                    }
                    (rx0, ry0, rx1, ry1)
                }
                other => other.bounds(),
            };
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
        }
        if min_x > max_x || min_y > max_y {
            // Все фигуры документа были ссылками на несуществующие символы
            // (resolved пустой для каждой) — тот же безопасный дефолт, что
            // и для документа без единой фигуры.
            return (0.0, 0.0, 1.0, 1.0);
        }
        (
            min_x - padding,
            min_y - padding,
            max_x + padding,
            max_y + padding,
        )
    }

    /// Сериализовать в настоящий SVG-текст (XML) — не наш внутренний
    /// формат, читается любым SVG-инструментом, включая уже готовый
    /// `pony_render::texture::load_svg` (resvg) в этом же движке.
    ///
    /// Символы (`VectorDoc::symbols`, раздел 28 ТЗ) пишутся один раз каждый
    /// как `<symbol id="symbol_<name>">` внутри `<defs>`, а каждый
    /// `VectorShape::Instance` — как отдельный `<use href="#symbol_<name>">`
    /// (см. `VectorShape::to_svg_element`) — настоящий SVG-механизм
    /// переиспользования, который `resvg` (используется этим движком для
    /// финального рендера, см. `pony-render`) понимает нативно. Символы,
    /// вложенные в другие символы (Instance внутри `SymbolDef::shapes`),
    /// сериализуются рекурсивно той же логикой — `<symbol>` тоже может
    /// содержать `<use>`, SVG это разрешает.
    pub fn to_svg_string(&self) -> String {
        let (min_x, min_y, max_x, max_y) = self.bounds_with_padding(4.0);
        let width = (max_x - min_x).max(1.0);
        let height = (max_y - min_y).max(1.0);
        // Раздел 60 ТЗ (Gradients): какие имена градиентов реально
        // существуют в этом документе — `to_svg_element` использует это,
        // чтобы решить, писать ли `fill="url(#gradient_<name>)"` или
        // честно откатиться на плоский `fill` (см. `resolved_fill_paint`).
        let known_gradients: std::collections::HashSet<&str> =
            self.gradients.iter().map(|g| g.name.as_str()).collect();
        let mut defs = String::new();
        // Каждый `GradientDef` пишется один раз как настоящий
        // `<linearGradient>`/`<radialGradient>` в `<defs>` — resvg
        // (используется этим движком для финального рендера, см.
        // pony-render) понимает нативно, тот же принцип переиспользования
        // по ссылке, что и у `<symbol>`/`<use>` ниже.
        for gradient in &self.gradients {
            defs.push_str(&gradient.to_svg_defs());
        }
        for symbol in &self.symbols {
            defs.push_str(&format!(r#"<symbol id="symbol_{}">"#, symbol.name));
            defs.push('\n');
            for shape in &symbol.shapes {
                defs.push_str(&shape.to_svg_element(&known_gradients));
                defs.push('\n');
            }
            defs.push_str("</symbol>\n");
        }
        let mut body = String::new();
        for shape in &self.shapes {
            body.push_str(&shape.to_svg_element(&known_gradients));
            body.push('\n');
        }
        let defs_block = if defs.is_empty() {
            String::new()
        } else {
            format!("<defs>\n{defs}</defs>\n")
        };
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{min_x} {min_y} {width} {height}" width="{width}" height="{height}">
{defs_block}{body}</svg>
"#
        )
    }

    /// Разобрать SVG-текст обратно в редактируемый `VectorDoc` — обратная
    /// операция к `to_svg_string()`. Замыкает цикл "создание/редактирование/
    /// использование": часть, однажды нарисованная и сохранённая как .svg,
    /// не остаётся неизменяемой — её можно открыть заново через эту функцию
    /// (см. кнопку "Редактировать SVG" в GUI) и продолжить редактировать
    /// теми же инструментами (SubSelection и т.д.), что и при рисовании.
    ///
    /// Настоящий XML-парсер (`roxmltree`), не построчный разбор — понимает
    /// произвольную вложенность `<g>`-групп с накопленным `transform`
    /// (`translate`/`rotate`/`scale`/`matrix`, применяется к геометрии при
    /// разборе — итоговый `VectorDoc` хранит уже плоский список фигур в
    /// абсолютных координатах документа, группы как отдельная структура не
    /// сохраняются при чтении: `VectorDoc` — плоская модель), стиль через
    /// `style="fill:...;stroke:...`" (приоритетнее отдельных атрибутов —
    /// как того требует каскад SVG/CSS) и через отдельные атрибуты
    /// `fill=`/`stroke=`, с честными дефолтами для необязательных
    /// (`fill` по умолчанию `black`, `stroke` по умолчанию `none`,
    /// `stroke-width` по умолчанию `1`) — раньше их ОТСУТСТВИЕ было
    /// фатальной ошибкой разбора, хотя это совершенно обычный, валидный
    /// SVG (не у каждой фигуры есть обводка). Понимает `#RGB`/`#RRGGBB`,
    /// `rgb(r,g,b)`/`rgba(r,g,b,a)` и частые именованные цвета
    /// (`black`/`white`/`red`/`none`/`transparent`/...).
    ///
    /// Понимает теги `rect`/`ellipse`/`circle`/`line`/`polyline`/`polygon`/
    /// `path`/`g` (рекурсивно) — этого достаточно для подавляющего
    /// большинства реальных художественных SVG, экспортированных из
    /// Illustrator/Inkscape/Figma и подобных, не только для собственного
    /// вывода `to_svg_string()`. Всё ещё НЕ парсер произвольного SVG (тот
    /// — задача `resvg`/`usvg`, уже используемого для импорта и рендера):
    /// градиенты, маски, фильтры, текст, символы/`<use>` в самой
    /// РЕДАКТИРУЕМОЙ модели `VectorDoc` пока не хранятся (см. отдельные
    /// задачи в docs/tdd.md, разделы 22/28/5-6) — при встрече такого
    /// элемента фигура пропускается с честным предупреждением в списке
    /// `unsupported`, не молча портит остальной документ и не отклоняет
    /// файл целиком (раздел 29 ТЗ: "неизвестные элементы не должны
    /// немедленно уничтожаться").
    pub fn from_svg_str(text: &str) -> Result<Self, VectorParseError> {
        let xml =
            roxmltree::Document::parse(text).map_err(|e| VectorParseError::Xml(e.to_string()))?;
        let root = xml.root_element();
        if root.tag_name().name() != "svg" {
            return Err(VectorParseError::Xml(format!(
                "корневой элемент — не <svg>, а <{}>",
                root.tag_name().name()
            )));
        }
        let mut doc = VectorDoc::new();
        let mut unsupported = Vec::new();
        collect_shapes(root, Transform2x3::IDENTITY, &mut doc, &mut unsupported);
        doc.unsupported = unsupported;
        // Раздел 60 ТЗ (Gradients): при разборе фигура со ссылкой на
        // градиент (`fill="url(#gradient_...)"`) могла встретиться РАНЬШЕ
        // самого `<defs><linearGradient .../></defs>` в порядке обхода
        // XML-дерева — на тот момент `fill` фигуры был заполнен нейтральным
        // серым плейсхолдером (см. `resolve_style`), не настоящим
        // приближением цвета. Теперь, когда документ разобран целиком и
        // `doc.gradients` заполнен окончательно, один финальный проход
        // уточняет `fill` до реального среднего цвета найденного градиента
        // — независимо от порядка `<defs>` в исходном файле.
        doc.sync_gradient_fallback_colors();
        Ok(doc)
    }

    /// См. вызов в `from_svg_str` — приводит `fill` каждой фигуры со
    /// `fill_gradient: Some(name)`, резолвящимся в `self.gradients`, к
    /// среднему цвету этого градиента (`GradientDef::average_color`).
    /// Публичный — GUI вызывает то же самое после `upsert_gradient`,
    /// когда пользователь редактирует стопы уже существующего градиента
    /// (тот же принцип: `fill` — всегда лучшее известное плоское
    /// приближение на данный момент, не протухшая копия с момента
    /// создания фигуры). Рекурсивно проходит и `self.symbols[*].shapes`
    /// (символы могут содержать фигуры с градиентной заливкой так же, как
    /// и фигуры верхнего уровня).
    pub fn sync_gradient_fallback_colors(&mut self) {
        let gradients = self.gradients.clone();
        let lookup = |name: &str| {
            gradients
                .iter()
                .find(|g| g.name == name)
                .map(GradientDef::average_color)
        };
        for shape in self
            .shapes
            .iter_mut()
            .chain(self.symbols.iter_mut().flat_map(|s| s.shapes.iter_mut()))
        {
            shape.sync_fill_from_gradient(&lookup);
        }
    }
}

/// 2x3-аффинная матрица (SVG `matrix(a,b,c,d,e,f)`), достаточная для любой
/// комбинации `translate`/`rotate`/`scale`/`skewX`/`skewY`/`matrix` —
/// применяется к координатам при флаттенинге `<g transform="...">` в
/// плоский список фигур (см. `from_svg_str`, `collect_shapes`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transform2x3 {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Transform2x3 {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(&self, p: (f32, f32)) -> (f32, f32) {
        (
            self.a * p.0 + self.c * p.1 + self.e,
            self.b * p.0 + self.d * p.1 + self.f,
        )
    }

    /// `self` затем `other` — то есть результат этой матрицы дальше
    /// проходит через `other` (порядок как в SVG: transform родителя
    /// применяется ПОСЛЕ transform ребёнка при накоплении вниз по дереву,
    /// см. `collect_shapes` — `parent.then(child)`).
    fn then(&self, other: Transform2x3) -> Transform2x3 {
        Transform2x3 {
            a: other.a * self.a + other.c * self.b,
            b: other.b * self.a + other.d * self.b,
            c: other.a * self.c + other.c * self.d,
            d: other.b * self.c + other.d * self.d,
            e: other.a * self.e + other.c * self.f + other.e,
            f: other.b * self.e + other.d * self.f + other.f,
        }
    }

    /// Масштаб по X — нужен для радиусов/ширин обводки, у которых нет
    /// отдельной X/Y компоненты (упрощение: берём масштаб по X, для
    /// неравномерного масштаба это приближение, честно принятое — как и
    /// в большинстве упрощённых SVG-флаттенеров).
    fn scale_factor(&self) -> f32 {
        ((self.a * self.a + self.b * self.b).sqrt() + (self.c * self.c + self.d * self.d).sqrt())
            / 2.0
    }
}

/// Разобрать `transform="translate(10,20) rotate(45) scale(2)"` в одну
/// накопленную матрицу — SVG применяет все функции слева направо, каждая
/// следующая композируется С предыдущим результатом.
fn parse_transform_attr(s: &str) -> Transform2x3 {
    let mut result = Transform2x3::IDENTITY;
    let mut chars = s.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !c.is_ascii_alphabetic() {
            continue;
        }
        let name_start = start;
        let mut name_end = start + c.len_utf8();
        while let Some(&(_, cc)) = chars.peek() {
            if cc.is_ascii_alphabetic() {
                name_end += cc.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let name = &s[name_start..name_end];
        let Some(paren_start) = s[name_end..].find('(') else {
            continue;
        };
        let paren_start = name_end + paren_start + 1;
        let Some(paren_len) = s[paren_start..].find(')') else {
            continue;
        };
        let args_str = &s[paren_start..paren_start + paren_len];
        let args: Vec<f32> = args_str
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        let m = match name {
            "translate" => Transform2x3 {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                e: *args.first().unwrap_or(&0.0),
                f: *args.get(1).unwrap_or(&0.0),
            },
            "scale" => {
                let sx = *args.first().unwrap_or(&1.0);
                let sy = *args.get(1).unwrap_or(&sx);
                Transform2x3 {
                    a: sx,
                    b: 0.0,
                    c: 0.0,
                    d: sy,
                    e: 0.0,
                    f: 0.0,
                }
            }
            "rotate" => {
                let deg = *args.first().unwrap_or(&0.0);
                let (sin, cos) = deg.to_radians().sin_cos();
                if args.len() >= 3 {
                    // rotate(angle, cx, cy) — вращение вокруг точки: translate(cx,cy) * rotate(angle) * translate(-cx,-cy).
                    let (cx, cy) = (args[1], args[2]);
                    let rot = Transform2x3 {
                        a: cos,
                        b: sin,
                        c: -sin,
                        d: cos,
                        e: 0.0,
                        f: 0.0,
                    };
                    Transform2x3 {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 1.0,
                        e: -cx,
                        f: -cy,
                    }
                    .then(rot)
                    .then(Transform2x3 {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 1.0,
                        e: cx,
                        f: cy,
                    })
                } else {
                    Transform2x3 {
                        a: cos,
                        b: sin,
                        c: -sin,
                        d: cos,
                        e: 0.0,
                        f: 0.0,
                    }
                }
            }
            "skewX" => {
                let deg = *args.first().unwrap_or(&0.0);
                Transform2x3 {
                    a: 1.0,
                    b: 0.0,
                    c: deg.to_radians().tan(),
                    d: 1.0,
                    e: 0.0,
                    f: 0.0,
                }
            }
            "skewY" => {
                let deg = *args.first().unwrap_or(&0.0);
                Transform2x3 {
                    a: 1.0,
                    b: deg.to_radians().tan(),
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: 0.0,
                }
            }
            "matrix" if args.len() >= 6 => Transform2x3 {
                a: args[0],
                b: args[1],
                c: args[2],
                d: args[3],
                e: args[4],
                f: args[5],
            },
            _ => Transform2x3::IDENTITY,
        };
        result = result.then(m);
    }
    result
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VectorParseError {
    #[error("не удалось разобрать XML: {0}")]
    Xml(String),
}

/// Цвет из SVG-атрибута `fill`/`stroke` — понимает `#RGB`, `#RRGGBB`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)`, `none`/`transparent`, и частый набор
/// именованных CSS-цветов, которых достаточно для подавляющего
/// большинства реальных иллюстраций (raster-редакторы и большинство
/// векторных экспортёров используют либо hex, либо горстку базовых имён).
/// Неизвестное значение — честный `None`, не тихая порча в чёрный.
fn parse_color_value(raw: &str) -> Option<RgbaColor> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return Some(RgbaColor::new(0, 0, 0, 0));
    }
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(RgbaColor::new(r, g, b, 255))
            }
            3 => {
                // #RGB — каждая цифра дублируется (#f0a -> #ff00aa), стандартное CSS-сокращение.
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(RgbaColor::new(r, g, b, 255))
            }
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r: f32 = parts[0].parse().ok()?;
            let g: f32 = parts[1].parse().ok()?;
            let b: f32 = parts[2].parse().ok()?;
            let a: f32 = parts[3].parse().ok()?;
            return Some(RgbaColor::new(
                r as u8,
                g as u8,
                b as u8,
                (a.clamp(0.0, 1.0) * 255.0).round() as u8,
            ));
        }
        return None;
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let r: f32 = parts[0].parse().ok()?;
            let g: f32 = parts[1].parse().ok()?;
            let b: f32 = parts[2].parse().ok()?;
            return Some(RgbaColor::new(r as u8, g as u8, b as u8, 255));
        }
        return None;
    }
    // Частый набор именованных CSS-цветов — не полный список из спеки
    // (147 имён), а те, что реально встречаются в практике иллюстраций.
    let named = match s.to_ascii_lowercase().as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "pink" => (255, 192, 203),
        "gray" | "grey" => (128, 128, 128),
        "brown" => (165, 42, 42),
        "cyan" | "aqua" => (0, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255),
        "lime" => (0, 255, 0),
        "navy" => (0, 0, 128),
        "teal" => (0, 128, 128),
        "silver" => (192, 192, 192),
        "maroon" => (128, 0, 0),
        "olive" => (128, 128, 0),
        "gold" => (255, 215, 0),
        "indigo" => (75, 0, 130),
        "violet" => (238, 130, 238),
        "coral" => (255, 127, 80),
        "salmon" => (250, 128, 114),
        "khaki" => (240, 230, 140),
        "beige" => (245, 245, 220),
        "ivory" => (255, 255, 240),
        "lavender" => (230, 230, 250),
        "tan" => (210, 180, 140),
        _ => return None,
    };
    Some(RgbaColor::new(named.0, named.1, named.2, 255))
}

/// Атрибуты элемента с приоритетом стиля: `style="fill:...;stroke:..."`
/// перекрывает отдельные атрибуты `fill=`/`stroke=` — стандартный каскад
/// SVG/CSS (inline style сильнее presentation-атрибута того же свойства).
/// Многие внешние экспортёры (Inkscape в частности) пишут ИМЕННО через
/// `style=`, не отдельными атрибутами — без этого разбора такие файлы
/// получали бы дефолтные чёрные фигуры без обводки, даже если исходный
/// файл был цветным.
struct ResolvedStyle {
    fill: RgbaColor,
    stroke: RgbaColor,
    stroke_width: f32,
    /// Раздел 60 ТЗ (Gradients): `Some(name)`, если `fill`/`style="fill:..."`
    /// был `url(#gradient_<name>)`, а не литеральный цвет — см.
    /// `extract_gradient_ref`. `fill` при этом всё равно заполняется
    /// плейсхолдером (уточняется позже, см. `VectorDoc::sync_gradient_
    /// fallback_colors`), чтобы поле никогда не оставалось в
    /// неопределённом состоянии на момент создания фигуры.
    fill_gradient: Option<String>,
}

/// Достаёт имя градиента из значения атрибута/стиля вида
/// `url(#gradient_<name>)` (то, что пишет `GradientDef::to_svg_defs` —
/// свой собственный формат id) — `None` для любого другого значения
/// (литеральный цвет, `none`, произвольный `url(#...)` без префикса
/// `gradient_`, который эта модель не создавала и не может однозначно
/// связать с `VectorDoc::gradients`).
fn extract_gradient_ref(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix("url(#")?.strip_suffix(')')?;
    inner.strip_prefix("gradient_").map(|name| name.to_string())
}

fn resolve_style(node: roxmltree::Node) -> ResolvedStyle {
    let mut fill_raw: Option<String> = node.attribute("fill").map(String::from);
    let mut stroke_raw: Option<String> = node.attribute("stroke").map(String::from);
    let mut fill_opacity: f32 = node
        .attribute("fill-opacity")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let mut stroke_opacity: f32 = node
        .attribute("stroke-opacity")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let mut stroke_width: f32 = node
        .attribute("stroke-width")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);

    if let Some(style) = node.attribute("style") {
        for decl in style.split(';') {
            let Some((k, v)) = decl.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "fill" => fill_raw = Some(v.to_string()),
                "stroke" => stroke_raw = Some(v.to_string()),
                "fill-opacity" => fill_opacity = v.parse().unwrap_or(fill_opacity),
                "stroke-opacity" => stroke_opacity = v.parse().unwrap_or(stroke_opacity),
                "stroke-width" => stroke_width = v.parse().unwrap_or(stroke_width),
                _ => {}
            }
        }
    }

    let fill_gradient = fill_raw.as_deref().and_then(extract_gradient_ref);

    // Дефолты по SVG-спеке: fill по умолчанию чёрный (не отсутствие!),
    // stroke по умолчанию none. Раньше отсутствие любого из атрибутов
    // было фатальной ошибкой разбора всей фигуры — хотя "фигура без
    // обводки" (самый частый случай в реальных SVG) совершенно валидна.
    // Если fill оказался ссылкой на градиент (`url(#gradient_...)`, не
    // распознаётся `parse_color_value` как цвет) — временный нейтральный
    // серый плейсхолдер, уточняется до реального среднего цвета градиента
    // сразу после полного разбора документа, см. `sync_gradient_fallback_colors`
    // (на момент разбора ЭТОЙ фигуры `<defs>` с самим градиентом мог ещё
    // не встретиться в порядке обхода XML-дерева).
    let mut fill =
        fill_raw
            .as_deref()
            .and_then(parse_color_value)
            .unwrap_or(if fill_gradient.is_some() {
                RgbaColor::new(128, 128, 128, 255)
            } else {
                RgbaColor::new(0, 0, 0, 255)
            });
    let mut stroke = stroke_raw
        .as_deref()
        .and_then(parse_color_value)
        .unwrap_or(RgbaColor::new(0, 0, 0, 0));
    fill.a = (fill.a as f32 * fill_opacity.clamp(0.0, 1.0)).round() as u8;
    stroke.a = (stroke.a as f32 * stroke_opacity.clamp(0.0, 1.0)).round() as u8;

    ResolvedStyle {
        fill,
        stroke,
        stroke_width,
        fill_gradient,
    }
}

fn parse_points(s: &str) -> Vec<(f32, f32)> {
    s.split_whitespace()
        .filter_map(|pair| {
            let (xs, ys) = pair.split_once(',')?;
            Some((xs.parse().ok()?, ys.parse().ok()?))
        })
        .collect()
}

/// Рекурсивно обходит дерево XML начиная с `node`, накапливая `transform`
/// от родительских `<g>` (и от самого элемента, если у него тоже есть
/// `transform` — раздел 27 ТЗ: "SVG g является контейнером и может
/// содержать другие g на произвольной глубине"). Найденные примитивные
/// фигуры добавляются в `doc` уже в абсолютных координатах (`VectorDoc` —
/// плоская модель, группы как структура при чтении не сохраняются).
/// Неизвестные/пока не поддерживаемые элементы (текст, use, filter и
/// т.п.) добавляются в `unsupported` с именем тега — раздел 29 ТЗ:
/// "неизвестные элементы не должны немедленно уничтожаться", то есть
/// молча теряться из вида — здесь они хотя бы видны пользователю по
/// имени, даже если геометрически не редактируемы этой версией.
fn collect_shapes(
    node: roxmltree::Node,
    transform: Transform2x3,
    doc: &mut VectorDoc,
    unsupported: &mut Vec<String>,
) {
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        let local_transform = child
            .attribute("transform")
            .map(parse_transform_attr)
            .unwrap_or(Transform2x3::IDENTITY);
        let combined = transform.then(local_transform);

        match child.tag_name().name() {
            "g" | "svg" => {
                collect_shapes(child, combined, doc, unsupported);
            }
            "rect" => {
                let x: f32 = child
                    .attribute("x")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y: f32 = child
                    .attribute("y")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let w: f32 = child
                    .attribute("width")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let h: f32 = child
                    .attribute("height")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let style = resolve_style(child);
                // Rect с ненулевым transform (поворот/skew) не сводится к
                // другому Rect — приближаем нижним левым/верхним правым
                // углом после трансформации (честное упрощение: полный
                // поворот прямоугольника потребовал бы либо отдельного
                // поля rotation у Rect, либо конвертации в Path — здесь
                // сознательно оставлено как приближение по осям).
                let (p0, p1) = (combined.apply((x, y)), combined.apply((x + w, y + h)));
                let (x0, x1) = (p0.0.min(p1.0), p0.0.max(p1.0));
                let (y0, y1) = (p0.1.min(p1.1), p0.1.max(p1.1));
                doc.add(VectorShape::Rect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                    fill: style.fill,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * combined.scale_factor(),
                    fill_gradient: style.fill_gradient.clone(),
                });
            }
            "ellipse" => {
                let cx: f32 = child
                    .attribute("cx")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let cy: f32 = child
                    .attribute("cy")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let rx: f32 = child
                    .attribute("rx")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let ry: f32 = child
                    .attribute("ry")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let style = resolve_style(child);
                let center = combined.apply((cx, cy));
                let scale = combined.scale_factor();
                doc.add(VectorShape::Ellipse {
                    cx: center.0,
                    cy: center.1,
                    rx: rx * scale,
                    ry: ry * scale,
                    fill: style.fill,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * scale,
                    fill_gradient: style.fill_gradient.clone(),
                });
            }
            "circle" => {
                // <circle> — не отдельный вариант VectorShape, представляем
                // как Ellipse с rx==ry (та же геометрия, SVG сам считает
                // circle частным случаем ellipse).
                let cx: f32 = child
                    .attribute("cx")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let cy: f32 = child
                    .attribute("cy")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let r: f32 = child
                    .attribute("r")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let style = resolve_style(child);
                let center = combined.apply((cx, cy));
                let scale = combined.scale_factor();
                doc.add(VectorShape::Ellipse {
                    cx: center.0,
                    cy: center.1,
                    rx: r * scale,
                    ry: r * scale,
                    fill: style.fill,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * scale,
                    fill_gradient: style.fill_gradient.clone(),
                });
            }
            "line" => {
                let x1: f32 = child
                    .attribute("x1")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y1: f32 = child
                    .attribute("y1")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let x2: f32 = child
                    .attribute("x2")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y2: f32 = child
                    .attribute("y2")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let style = resolve_style(child);
                let p1 = combined.apply((x1, y1));
                let p2 = combined.apply((x2, y2));
                doc.add(VectorShape::Line {
                    x1: p1.0,
                    y1: p1.1,
                    x2: p2.0,
                    y2: p2.1,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * combined.scale_factor(),
                });
            }
            "polyline" => {
                let points = child
                    .attribute("points")
                    .map(parse_points)
                    .unwrap_or_default();
                let style = resolve_style(child);
                let points = points.into_iter().map(|p| combined.apply(p)).collect();
                doc.add(VectorShape::Polyline {
                    points,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * combined.scale_factor(),
                });
            }
            "polygon" => {
                let points = child
                    .attribute("points")
                    .map(parse_points)
                    .unwrap_or_default();
                let style = resolve_style(child);
                let points = points.into_iter().map(|p| combined.apply(p)).collect();
                doc.add(VectorShape::Polygon {
                    points,
                    fill: style.fill,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * combined.scale_factor(),
                    fill_gradient: style.fill_gradient.clone(),
                });
            }
            "path" => {
                let Some(d) = child.attribute("d") else {
                    unsupported.push("path (без атрибута d)".to_string());
                    continue;
                };
                let Some((nodes, closed)) = parse_path_d(d) else {
                    unsupported.push(format!(
                        "path (нераспознанная геометрия d=\"{}\")",
                        &d[..d.len().min(40)]
                    ));
                    continue;
                };
                let nodes: Vec<PathNode> = nodes
                    .into_iter()
                    .map(|n| PathNode {
                        position: combined.apply(n.position),
                        in_handle: n.in_handle.map(|h| combined.apply(h)),
                        out_handle: n.out_handle.map(|h| combined.apply(h)),
                        node_type: n.node_type,
                    })
                    .collect();
                let style = resolve_style(child);
                doc.add(VectorShape::Path {
                    nodes,
                    closed,
                    fill: style.fill,
                    stroke: style.stroke,
                    stroke_width: style.stroke_width * combined.scale_factor(),
                    fill_gradient: style.fill_gradient.clone(),
                });
            }
            "defs" => {
                // Раньше — полностью пропускался: не содержит видимой
                // геометрии САМ ПО СЕБЕ, но раздел 28 ТЗ ("Symbol Definition")
                // требует, чтобы `<symbol>` внутри всё же читался — просто
                // не как видимая геометрия документа сразу, а как отдельное
                // именованное определение (`VectorDoc::symbols`), на которое
                // может сослаться `<use>` где угодно в документе. Раздел 60
                // ТЗ (Gradients) — та же логика для `<linearGradient>`/
                // `<radialGradient>`: собственное именованное определение
                // (`VectorDoc::gradients`), на него ссылаются фигуры через
                // `fill_gradient`, а не сама геометрия документа.
                for def_child in child.children() {
                    if !def_child.is_element() {
                        continue;
                    }
                    match def_child.tag_name().name() {
                        "symbol" => collect_symbol_def(def_child, doc, unsupported),
                        "linearGradient" | "radialGradient" => {
                            collect_gradient_def(def_child, doc, unsupported)
                        }
                        _ => {
                            // Прочее содержимое `<defs>` (фильтры, паттерны и
                            // т.п.) — по-прежнему не хранится этой моделью
                            // (см. общий комментарий у `from_svg_str`), но и
                            // не добавляется в `unsupported`: `<defs>` без
                            // видимого эффекта сам по себе не то же самое,
                            // что видимый пропущенный элемент на канвасе.
                        }
                    }
                }
            }
            "use" => {
                // Раздел 28 ТЗ — `<use href="#id">` создаёт видимый инстанс
                // ранее определённого `<symbol>`. `xlink:href` — старое имя
                // того же атрибута (SVG 1.1), современный SVG 2 использует
                // просто `href`; читаем оба для совместимости с файлами из
                // старых экспортёров (Illustrator до сих пор нередко пишет
                // `xlink:href`).
                let href = child
                    .attribute("href")
                    .or_else(|| child.attribute(("http://www.w3.org/1999/xlink", "href")));
                let Some(href) = href else {
                    unsupported.push("use (без атрибута href)".to_string());
                    continue;
                };
                let symbol_name = href
                    .trim_start_matches('#')
                    .trim_start_matches("symbol_")
                    .to_string();
                // `<use x=".." y="..">` — дополнительное смещение поверх
                // transform, как отдельные атрибуты SVG (не через
                // transform="translate(...)") — складываем в ту же
                // накопленную матрицу как ещё один translate.
                let use_x: f32 = child
                    .attribute("x")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let use_y: f32 = child
                    .attribute("y")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let with_use_offset = combined.then(Transform2x3 {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: use_x,
                    f: use_y,
                });
                let fill_override =
                    child
                        .attribute("fill")
                        .and_then(parse_color_value)
                        .or_else(|| {
                            // `style="fill:...;"` на самом `<use>` — override для
                            // ЭТОГО инстанса (раздел 95 ТЗ), тот же каскад, что и
                            // resolve_style использует для обычных фигур, но здесь
                            // читаем только fill — у Instance нет отдельного stroke
                            // override (см. `VectorShape::set_stroke` на Instance).
                            let style_str = child.attribute("style")?;
                            style_str.split(';').find_map(|decl| {
                                let (prop, val) = decl.split_once(':')?;
                                if prop.trim() == "fill" {
                                    parse_color_value(val.trim())
                                } else {
                                    None
                                }
                            })
                        });
                doc.add(VectorShape::Instance {
                    symbol: symbol_name,
                    transform: (
                        with_use_offset.a,
                        with_use_offset.b,
                        with_use_offset.c,
                        with_use_offset.d,
                        with_use_offset.e,
                        with_use_offset.f,
                    ),
                    fill_override,
                });
            }
            other => {
                unsupported.push(other.to_string());
            }
        }
    }
}

/// Разобрать содержимое `<symbol id="...">` в `SymbolDef` и зарегистрировать
/// его в `doc.symbols` — использует ТОТ ЖЕ `collect_shapes`, что и обычный
/// обход документа (единая логика разбора фигур/трансформов/стилей, не
/// дублирующая копия), но в СВОЙ собственный временный `VectorDoc`, из
/// которого потом забирается только `shapes` — у символа своя, локальная
/// система координат (то, что снаружи символа ничего не транслируется на
/// него, кроме transform самого `<use>`, — стандартная SVG-семантика
/// `<symbol>`, не то же самое, что `<g>`).
///
/// `<symbol>` БЕЗ атрибута `id` — предупреждение в `unsupported` (на него
/// физически невозможно сослаться никаким `<use href="#...">`), не тихий
/// пропуск: пользователь мог забыть id при ручном редактировании SVG, и
/// молчаливая потеря содержимого была бы хуже явного предупреждения.
fn collect_symbol_def(node: roxmltree::Node, doc: &mut VectorDoc, unsupported: &mut Vec<String>) {
    let Some(id) = node.attribute("id") else {
        unsupported
            .push("symbol (без атрибута id — на него нельзя сослаться через use)".to_string());
        return;
    };
    let name = id.trim_start_matches("symbol_").to_string();
    let mut inner_doc = VectorDoc::new();
    let mut inner_unsupported = Vec::new();
    collect_shapes(
        node,
        Transform2x3::IDENTITY,
        &mut inner_doc,
        &mut inner_unsupported,
    );
    // Символы, вложенные в этот символ (Instance внутри Instance) — тоже
    // регистрируются в ТОМ ЖЕ, общем `doc.symbols` (SVG `<symbol>` внутри
    // `<symbol>` — редкость, но не запрещена спекой; `collect_shapes` уже
    // сама рекурсивно обработает вложенные `<defs>`/`<symbol>`, если они
    // окажутся внутри — здесь просто переносим уже накопленные inner_doc.symbols).
    for nested in inner_doc.symbols {
        doc.symbols.push(nested);
    }
    for name_dup in inner_unsupported {
        unsupported.push(format!("symbol '{name}': {name_dup}"));
    }
    doc.symbols.push(SymbolDef {
        name,
        shapes: inner_doc.shapes,
    });
}

/// Разобрать `<linearGradient>`/`<radialGradient>` в `GradientDef` и
/// зарегистрировать его в `doc.gradients` (раздел 60 ТЗ). Симметрично
/// `collect_symbol_def`: без `id` сослаться на градиент невозможно ни
/// одной фигурой — предупреждение в `unsupported`, не тихий пропуск.
/// Читает ТОЛЬКО `gradientUnits="userSpaceOnUse"` координаты (то, что сама
/// эта модель и пишет — см. `GradientDef::to_svg_defs`); файлы из внешних
/// редакторов, использующие `objectBoundingBox` (SVG-дефолт, ЕСЛИ атрибут
/// вообще не указан) или `gradientTransform`, читаются буквально по числам
/// координат (как если бы они уже были в userSpaceOnUse) — приближение,
/// которое может визуально разъехаться с исходным файлом для градиентов,
/// импортированных из других инструментов, честно принятое ограничение
/// (полная поддержка obj-relative координат — отдельная задача при первом
/// реальном запросе на неё), а не молчаливая потеря данных: сам факт
/// присутствия градиента и его стопы сохраняются точно в любом случае.
fn collect_gradient_def(node: roxmltree::Node, doc: &mut VectorDoc, unsupported: &mut Vec<String>) {
    let Some(id) = node.attribute("id") else {
        unsupported.push(format!(
            "{} (без атрибута id — на него нельзя сослаться через fill)",
            node.tag_name().name()
        ));
        return;
    };
    let name = id.trim_start_matches("gradient_").to_string();

    let kind = match node.tag_name().name() {
        "linearGradient" => GradientKind::Linear {
            x1: node
                .attribute("x1")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            y1: node
                .attribute("y1")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            x2: node
                .attribute("x2")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
            y2: node
                .attribute("y2")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
        },
        _ => GradientKind::Radial {
            cx: node
                .attribute("cx")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            cy: node
                .attribute("cy")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            r: node
                .attribute("r")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
        },
    };

    let mut stops = Vec::new();
    for stop_node in node
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "stop")
    {
        let offset: f32 = stop_node
            .attribute("offset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        // `stop-color`/`stop-opacity` могут быть отдельными атрибутами ИЛИ
        // внутри `style="stop-color:...;stop-opacity:..."` — тот же каскад,
        // что `resolve_style` использует для обычных фигур, но локально
        // (у `<stop>` нет `fill`/`stroke`, только эта пара свойств).
        let mut color_raw = stop_node.attribute("stop-color").map(String::from);
        let mut opacity: f32 = stop_node
            .attribute("stop-opacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        if let Some(style) = stop_node.attribute("style") {
            for decl in style.split(';') {
                let Some((k, v)) = decl.split_once(':') else {
                    continue;
                };
                let (k, v) = (k.trim(), v.trim());
                match k {
                    "stop-color" => color_raw = Some(v.to_string()),
                    "stop-opacity" => opacity = v.parse().unwrap_or(opacity),
                    _ => {}
                }
            }
        }
        let mut color = color_raw
            .as_deref()
            .and_then(parse_color_value)
            .unwrap_or(RgbaColor::new(0, 0, 0, 255));
        color.a = (color.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
        stops.push(GradientStop { offset, color });
    }

    if stops.is_empty() {
        unsupported.push(format!(
            "{} '{name}' (без стопов цвета — заливка не определена)",
            node.tag_name().name()
        ));
    }

    doc.upsert_gradient(GradientDef { name, kind, stops });
}

/// Сериализовать узлы path в SVG-команды `d` (раздел 8 ТЗ). Пишем только
/// `M`/`L`/`C`/`Z` — минимальный набор, которого достаточно, чтобы точно
/// выразить ЛЮБУЮ комбинацию прямых и кубических кривых Безье (`Q`/`S`/`T`
/// — сериализационные сокращения ДЛЯ ТЕХ ЖЕ кривых, не более выразительные
/// сами по себе; `parse_path_d` умеет их ЧИТАТЬ и конвертирует в `C`-форму
/// при разборе, так что раунд-трип через сохранение сокращённую форму не
/// сохраняет буквально, но результирующая кривая идентична).
fn path_data_string(nodes: &[PathNode], closed: bool) -> String {
    if nodes.is_empty() {
        return String::new();
    }
    let mut d = format!("M {} {}", nodes[0].position.0, nodes[0].position.1);
    for i in 1..nodes.len() {
        let prev = &nodes[i - 1];
        let cur = &nodes[i];
        match (prev.out_handle, cur.in_handle) {
            (None, None) => d.push_str(&format!(" L {} {}", cur.position.0, cur.position.1)),
            (out, inp) => {
                // Отсутствующая ручка с одной стороны сегмента — берём
                // саму точку узла как вырожденную (нулевой длины) ручку:
                // геометрически корректная кубическая кривая, которая на
                // этом конце ведёт себя как прямая линия — не нужно
                // отдельно кодировать "смешанный" сегмент третьей командой.
                let c1 = out.unwrap_or(prev.position);
                let c2 = inp.unwrap_or(cur.position);
                d.push_str(&format!(
                    " C {} {} {} {} {} {}",
                    c1.0, c1.1, c2.0, c2.1, cur.position.0, cur.position.1
                ));
            }
        }
    }
    if closed {
        // Замыкающий сегмент — от последнего узла к первому, той же логикой.
        let last = nodes.last().expect("checked non-empty above");
        let first = &nodes[0];
        match (last.out_handle, first.in_handle) {
            (None, None) => d.push_str(" Z"),
            (out, inp) => {
                let c1 = out.unwrap_or(last.position);
                let c2 = inp.unwrap_or(first.position);
                d.push_str(&format!(
                    " C {} {} {} {} {} {} Z",
                    c1.0, c1.1, c2.0, c2.1, first.position.0, first.position.1
                ));
            }
        }
    }
    d
}

/// Эллиптическая дуга (команда `A`/`a`, раздел 8 ТЗ) как последовательность
/// кубических кривых Безье — стандартный алгоритм из спецификации SVG 1.1,
/// приложение F ("Elliptical arc implementation notes"):
/// 1. Endpoint-параметризация (rx, ry, x-axis-rotation, flags, конец) —
///    конвертируется в центровую (center, углы начала/конца дуги).
/// 2. Дуга режется на сегменты не больше 90° каждый (аппроксимация одним
///    кубическим сегментом дуги больше 90° даёт заметную, легко видимую на
///    глаз погрешность формы — стандартное ограничение техники).
/// 3. Каждый сегмент дуги превращается в одну кубическую кривую по формуле
///    "magic number" (`k = 4/3 * tan(delta/4)` для длины касательной ручки)
///    — точная аппроксимация до третьего порядка, обычная практика
///    (Inkscape, resvg/lyon и большинство векторных редакторов делают то
///    же самое, дуга не хранится как отдельный примитив нигде дальше по
///    цепочке).
///
/// Возвращает `Vec<(c1, c2, end)>` — контрольные точки и конец каждого
/// кубического сегмента, в порядке от начала дуги к концу; `start` в узел
/// не входит (он уже есть как курсор ДО дуги).
fn arc_to_cubic_beziers(
    start: (f32, f32),
    rx: f32,
    ry: f32,
    x_axis_rotation_deg: f32,
    large_arc_flag: bool,
    sweep_flag: bool,
    end: (f32, f32),
) -> Vec<((f32, f32), (f32, f32), (f32, f32))> {
    // Вырожденный радиус или дуга в ту же точку — SVG-спека предписывает
    // трактовать это как прямую линию (никакой видимой дуги нет).
    if (start.0 - end.0).abs() < 1e-9 && (start.1 - end.1).abs() < 1e-9 {
        return Vec::new();
    }
    if rx.abs() < 1e-9 || ry.abs() < 1e-9 {
        return vec![(start, end, end)]; // вырожденная "дуга" = прямая; C1=start,C2=end имитирует L
    }
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    let phi = x_axis_rotation_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    // Шаг 1 (F.6.5): координаты середины хорды в повёрнутой системе координат эллипса.
    let dx2 = (start.0 - end.0) / 2.0;
    let dy2 = (start.1 - end.1) / 2.0;
    let x1p = cos_phi * dx2 + sin_phi * dy2;
    let y1p = -sin_phi * dx2 + cos_phi * dy2;

    // Шаг 2 (F.6.6): корректировка радиусов, если эллипс физически слишком
    // мал, чтобы дотянуться от start до end — SVG-спека требует
    // масштабировать rx/ry вверх пропорционально, а не отклонять path.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }

    // Шаг 3 (F.6.5): центр эллипса в повёрнутой системе координат.
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let sign = if large_arc_flag == sweep_flag {
        -1.0
    } else {
        1.0
    };
    let num = (rx2 * ry2 - rx2 * y1p2 - ry2 * x1p2).max(0.0); // .max(0.0) — защита от отрицательного из-за погрешности округления
    let denom = rx2 * y1p2 + ry2 * x1p2;
    let coef = if denom.abs() < 1e-12 {
        0.0
    } else {
        sign * (num / denom).sqrt()
    };
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * (-ry * x1p / rx);

    // Центр в исходной системе координат.
    let cx = cos_phi * cxp - sin_phi * cyp + (start.0 + end.0) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.1 + end.1) / 2.0;

    // Шаг 4 (F.6.5): углы начала и разница углов (theta1, delta_theta).
    let angle_between = |u: (f32, f32), v: (f32, f32)| -> f32 {
        let dot = u.0 * v.0 + u.1 * v.1;
        let len = ((u.0 * u.0 + u.1 * u.1) * (v.0 * v.0 + v.1 * v.1))
            .sqrt()
            .max(1e-9);
        let cross_sign = if u.0 * v.1 - u.1 * v.0 < 0.0 {
            -1.0
        } else {
            1.0
        };
        cross_sign * (dot / len).clamp(-1.0, 1.0).acos()
    };
    let v1 = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let v2 = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta1 = angle_between((1.0, 0.0), v1);
    let mut delta_theta = angle_between(v1, v2);
    if !sweep_flag && delta_theta > 0.0 {
        delta_theta -= std::f32::consts::TAU;
    } else if sweep_flag && delta_theta < 0.0 {
        delta_theta += std::f32::consts::TAU;
    }

    // Шаг 5: режем на сегменты по <=90° (PI/2) для точной кубической аппроксимации.
    let segment_count = (delta_theta.abs() / (std::f32::consts::FRAC_PI_2))
        .ceil()
        .max(1.0) as usize;
    let segment_delta = delta_theta / segment_count as f32;

    let mut result = Vec::with_capacity(segment_count);
    let mut theta = theta1;
    // Точка эллипса (до поворота/сдвига) для заданного угла.
    let ellipse_point = |t: f32| -> (f32, f32) {
        let (s, c) = t.sin_cos();
        let ex = rx * c;
        let ey = ry * s;
        (
            cx + cos_phi * ex - sin_phi * ey,
            cy + sin_phi * ex + cos_phi * ey,
        )
    };
    // Касательная (производная по t) в заданном угле, для контрольных точек.
    let ellipse_tangent = |t: f32| -> (f32, f32) {
        let (s, c) = t.sin_cos();
        let tx = -rx * s;
        let ty = ry * c;
        (cos_phi * tx - sin_phi * ty, sin_phi * tx + cos_phi * ty)
    };

    for _ in 0..segment_count {
        let theta_next = theta + segment_delta;
        let p0 = ellipse_point(theta);
        let p1 = ellipse_point(theta_next);
        // "Magic number" alpha для кубической аппроксимации дуги эллипса:
        // k = 4/3 * tan(delta/4), длина касательной ручки в единицах
        // производной по параметру t (см. арifont/de Casteljau-стандартные
        // выводы для этой аппроксимации, используется практически
        // повсеместно — Cairo, Skia, resvg).
        let alpha = (4.0 / 3.0) * (segment_delta / 4.0).tan();
        let t0 = ellipse_tangent(theta);
        let t1 = ellipse_tangent(theta_next);
        let c1 = (p0.0 + alpha * t0.0, p0.1 + alpha * t0.1);
        let c2 = (p1.0 - alpha * t1.0, p1.1 - alpha * t1.1);
        result.push((c1, c2, p1));
        theta = theta_next;
    }
    // Последняя точка должна точно совпасть с `end`, а не с точкой на
    // идеальном эллипсе (которая из-за погрешности округления параметров
    // может немного отличаться) — иначе path после дуги "телепортируется".
    if let Some(last) = result.last_mut() {
        last.2 = end;
    }
    result
}

/// Разобрать `d`-атрибут path (раздел 8 ТЗ) в узлы + флаг замкнутости.
/// Понимает `M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z/z` — то есть и
/// абсолютные, и относительные команды, включая эллиптические дуги.
/// `A`/`a` конвертируется в одну или несколько кубических кривых Безье
/// через `arc_to_cubic_beziers` (стандартный алгоритм спецификации SVG) —
/// та же причина, что и для `Q`/`T`: модель узла (раздел 9) хранит только
/// кубические ручки, единое представление для всех кривых. Это закрывает
/// самый частый практический случай, из-за которого готовые художественные
/// SVG (не только собственный вывод сериализатора) не открывались на
/// редактирование — дуги нередки в реальных иллюстрациях (скруглённые
/// формы, глаза, рты), тогда как этот редактор поначалу поддерживал их
/// только как честную ошибку импорта.
///
/// Каждая прочитанная точка становится узлом с ручками, восстановленными
/// из соответствующей кривой (`C`/`S`/`Q`/`T`) — обратная операция к
/// `path_data_string`, поэтому "нарисовали → сохранили → открыли снова"
/// не теряет форму кривой, даже если сам путь изначально пришёл ИЗВНЕ
/// (импортирован не из этого редактора) в сокращённой SVG-нотации.
fn parse_path_d(d: &str) -> Option<(Vec<PathNode>, bool)> {
    let tokens = tokenize_path_d(d);
    let mut i = 0;
    let mut nodes: Vec<PathNode> = Vec::new();
    let mut closed = false;
    let mut cursor = (0.0f32, 0.0f32);
    let mut last_cubic_c2: Option<(f32, f32)> = None; // для S/s — отражение предыдущей C2
    let mut last_quad_c: Option<(f32, f32)> = None; // для T/t — отражение предыдущей Q-ручки

    while i < tokens.len() {
        let cmd = match &tokens[i] {
            PathToken::Command(c) => *c,
            PathToken::Number(_) => return None, // число без предшествующей команды — некорректный path
        };
        i += 1;
        let relative = cmd.is_ascii_lowercase();
        let take_num = |i: &mut usize| -> Option<f32> {
            match tokens.get(*i) {
                Some(PathToken::Number(n)) => {
                    *i += 1;
                    Some(*n)
                }
                _ => None,
            }
        };
        let resolve = |p: (f32, f32), cursor: (f32, f32), relative: bool| {
            if relative {
                (cursor.0 + p.0, cursor.1 + p.1)
            } else {
                p
            }
        };

        match cmd.to_ascii_uppercase() {
            'M' => {
                let x = take_num(&mut i)?;
                let y = take_num(&mut i)?;
                let pt = resolve((x, y), cursor, relative);
                cursor = pt;
                nodes.push(PathNode::corner(pt));
                last_cubic_c2 = None;
                last_quad_c = None;
                // Дополнительные пары чисел сразу после M трактуются как
                // неявные L (стандартное поведение SVG path grammar).
                while let (Some(x), Some(y)) =
                    (take_num_peek(&tokens, i), take_num_peek(&tokens, i + 1))
                {
                    i += 2;
                    let pt = resolve((x, y), cursor, relative);
                    cursor = pt;
                    nodes.push(PathNode::corner(pt));
                }
            }
            'L' => {
                let x = take_num(&mut i)?;
                let y = take_num(&mut i)?;
                let pt = resolve((x, y), cursor, relative);
                cursor = pt;
                nodes.push(PathNode::corner(pt));
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'H' => {
                let x = take_num(&mut i)?;
                let pt = if relative {
                    (cursor.0 + x, cursor.1)
                } else {
                    (x, cursor.1)
                };
                cursor = pt;
                nodes.push(PathNode::corner(pt));
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'V' => {
                let y = take_num(&mut i)?;
                let pt = if relative {
                    (cursor.0, cursor.1 + y)
                } else {
                    (cursor.0, y)
                };
                cursor = pt;
                nodes.push(PathNode::corner(pt));
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'C' => {
                let c1 = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let c2 = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let end = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode {
                    position: end,
                    in_handle: Some(c2),
                    out_handle: None,
                    node_type: NodeType::Corner,
                });
                last_cubic_c2 = Some(c2);
                last_quad_c = None;
                cursor = end;
            }
            'S' => {
                // Smooth cubic — первая ручка есть отражение C2 ПРЕДЫДУЩЕЙ
                // кривой относительно текущего курсора (стандартное правило
                // SVG); если предыдущий сегмент не был C/S — совпадает с
                // курсором (нет "инерции" направления).
                let c1 = match last_cubic_c2 {
                    Some(prev_c2) => (2.0 * cursor.0 - prev_c2.0, 2.0 * cursor.1 - prev_c2.1),
                    None => cursor,
                };
                let c2 = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let end = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode {
                    position: end,
                    in_handle: Some(c2),
                    out_handle: None,
                    node_type: NodeType::Corner,
                });
                last_cubic_c2 = Some(c2);
                last_quad_c = None;
                cursor = end;
            }
            'Q' => {
                // Квадратичная Безье — конвертируем в кубическую (точная
                // формула: C1 = P0 + 2/3*(Q-P0), C2 = P1 + 2/3*(Q-P1)),
                // чтобы хранить ВСЕ кривые одним представлением (раздел 9:
                // модель узла — это кубические in/out ручки, не квадратичные).
                let q = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let end = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let c1 = (
                    cursor.0 + 2.0 / 3.0 * (q.0 - cursor.0),
                    cursor.1 + 2.0 / 3.0 * (q.1 - cursor.1),
                );
                let c2 = (
                    end.0 + 2.0 / 3.0 * (q.0 - end.0),
                    end.1 + 2.0 / 3.0 * (q.1 - end.1),
                );
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode {
                    position: end,
                    in_handle: Some(c2),
                    out_handle: None,
                    node_type: NodeType::Corner,
                });
                last_quad_c = Some(q);
                last_cubic_c2 = None;
                cursor = end;
            }
            'T' => {
                let q = match last_quad_c {
                    Some(prev_q) => (2.0 * cursor.0 - prev_q.0, 2.0 * cursor.1 - prev_q.1),
                    None => cursor,
                };
                let end = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);
                let c1 = (
                    cursor.0 + 2.0 / 3.0 * (q.0 - cursor.0),
                    cursor.1 + 2.0 / 3.0 * (q.1 - cursor.1),
                );
                let c2 = (
                    end.0 + 2.0 / 3.0 * (q.0 - end.0),
                    end.1 + 2.0 / 3.0 * (q.1 - end.1),
                );
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode {
                    position: end,
                    in_handle: Some(c2),
                    out_handle: None,
                    node_type: NodeType::Corner,
                });
                last_quad_c = Some(q);
                last_cubic_c2 = None;
                cursor = end;
            }
            'Z' => {
                closed = true;
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'A' => {
                let rx = take_num(&mut i)?;
                let ry = take_num(&mut i)?;
                let x_axis_rotation = take_num(&mut i)?;
                let large_arc_flag = take_num(&mut i)? != 0.0;
                let sweep_flag = take_num(&mut i)? != 0.0;
                let end = resolve((take_num(&mut i)?, take_num(&mut i)?), cursor, relative);

                let segments = arc_to_cubic_beziers(
                    cursor,
                    rx,
                    ry,
                    x_axis_rotation,
                    large_arc_flag,
                    sweep_flag,
                    end,
                );
                for (c1, c2, seg_end) in segments {
                    if let Some(prev) = nodes.last_mut() {
                        prev.out_handle = Some(c1);
                    }
                    nodes.push(PathNode {
                        position: seg_end,
                        in_handle: Some(c2),
                        out_handle: None,
                        node_type: NodeType::Corner,
                    });
                }
                last_cubic_c2 = None;
                last_quad_c = None;
                cursor = end;
            }
            _ => return None,
        }
    }
    if nodes.is_empty() {
        None
    } else {
        Some((nodes, closed))
    }
}

fn take_num_peek(tokens: &[PathToken], i: usize) -> Option<f32> {
    match tokens.get(i) {
        Some(PathToken::Number(n)) => Some(*n),
        _ => None,
    }
}

enum PathToken {
    Command(char),
    Number(f32),
}

/// Токенизация `d`: команды — отдельные буквы, числа — разделены
/// пробелами/запятыми, либо слитно (SVG допускает `10-5` как `10` и `-5`,
/// и `.5.5` как `0.5` и `0.5`) — минимальный, но настоящий разбор
/// стандартной path-грамматики, а не просто `split_whitespace`.
fn tokenize_path_d(d: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() || c == ',' {
            i += 1;
        } else if c.is_ascii_alphabetic() {
            tokens.push(PathToken::Command(c));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            let mut seen_dot = c == '.';
            while i < bytes.len() {
                let cc = bytes[i] as char;
                if cc.is_ascii_digit() {
                    i += 1;
                } else if cc == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (cc == 'e' || cc == 'E') && i + 1 < bytes.len() {
                    i += 1;
                    if i < bytes.len() && (bytes[i] as char == '-' || bytes[i] as char == '+') {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(n) = d[start..i].parse::<f32>() {
                tokens.push(PathToken::Number(n));
            }
        } else {
            i += 1; // неизвестный символ — пропускаем, не считаем фатальной ошибкой
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: RgbaColor = RgbaColor::new(255, 0, 0, 255);
    const BLUE_HALF: RgbaColor = RgbaColor::new(0, 0, 255, 128);

    #[test]
    fn empty_doc_is_empty_and_serializes_to_valid_minimal_svg() {
        let doc = VectorDoc::new();
        assert!(doc.is_empty());
        let svg = doc.to_svg_string();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn rect_serializes_with_correct_attributes() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
            fill: RED,
            stroke: RED,
            stroke_width: 2.0,
            fill_gradient: None,
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r#"<rect x="10" y="20" width="30" height="40""#));
        assert!(svg.contains(r##"fill="#ff0000""##));
    }

    #[test]
    fn alpha_becomes_opacity_not_part_of_hex_color() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Ellipse {
            cx: 0.0,
            cy: 0.0,
            rx: 5.0,
            ry: 5.0,
            fill: BLUE_HALF,
            stroke: BLUE_HALF,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        let svg = doc.to_svg_string();
        assert!(
            svg.contains(r##"fill="#0000ff""##),
            "hex should not encode alpha: {svg}"
        );
        assert!(
            svg.contains("fill-opacity=\"0.502\""),
            "alpha 128/255 should show up as opacity: {svg}"
        );
    }

    #[test]
    fn bounds_grow_to_fit_all_shapes_with_padding() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        doc.add(VectorShape::Ellipse {
            cx: 100.0,
            cy: 100.0,
            rx: 20.0,
            ry: 20.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        let (min_x, min_y, max_x, max_y) = doc.bounds_with_padding(4.0);
        assert!((min_x - (-4.0)).abs() < 1e-4);
        assert!((min_y - (-4.0)).abs() < 1e-4);
        assert!((max_x - 124.0).abs() < 1e-4, "100+20+4=124, got {max_x}");
        assert!((max_y - 124.0).abs() < 1e-4);
    }

    #[test]
    fn clear_removes_all_shapes() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RED,
            stroke_width: 1.0,
        });
        assert!(!doc.is_empty());
        doc.clear();
        assert!(doc.is_empty());
    }

    #[test]
    fn polyline_includes_all_points_in_order() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Polyline {
            points: vec![(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)],
            stroke: RED,
            stroke_width: 2.0,
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r#"points="0,0 5,5 10,0""#), "{svg}");
    }

    #[test]
    fn polygon_serializes_with_fill_unlike_polyline() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Polygon {
            points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)],
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains("<polygon"));
        assert!(svg.contains(r##"fill="#ff0000""##), "{svg}");
    }

    #[test]
    fn shape_at_finds_the_topmost_shape_under_the_point() {
        let mut doc = VectorDoc::new();
        // Два перекрывающихся прямоугольника — второй нарисован позже,
        // значит рисуется поверх и должен находиться первым.
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 20.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        doc.add(VectorShape::Rect {
            x: 5.0,
            y: 5.0,
            w: 20.0,
            h: 20.0,
            fill: BLUE_HALF,
            stroke: BLUE_HALF,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        assert_eq!(
            doc.shape_at(10.0, 10.0),
            Some(1),
            "точка внутри обоих — должна найтись верхняя (индекс 1)"
        );
        assert_eq!(doc.shape_at(2.0, 2.0), Some(0), "точка только в первой");
        assert_eq!(doc.shape_at(100.0, 100.0), None, "мимо всех фигур");
    }

    #[test]
    fn set_fill_and_set_stroke_change_the_serialized_colors() {
        let mut shape = VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 5.0,
            h: 5.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        shape.set_fill(BLUE_HALF);
        shape.set_stroke(RgbaColor::new(0, 255, 0, 255));
        let mut doc = VectorDoc::new();
        doc.add(shape);
        let svg = doc.to_svg_string();
        assert!(svg.contains(r##"fill="#0000ff""##));
        assert!(svg.contains(r##"stroke="#00ff00""##));
    }

    #[test]
    fn set_fill_is_a_no_op_on_strokeonly_shapes() {
        // Line/Polyline не заливаются — set_fill не должен паниковать и
        // не должен что-то незаметно сломать.
        let mut line = VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RED,
            stroke_width: 1.0,
        };
        line.set_fill(BLUE_HALF); // просто не должно паниковать
        let mut doc = VectorDoc::new();
        doc.add(line);
        assert!(
            !doc.to_svg_string().contains("fill-opacity"),
            "у Line вообще нет fill-атрибута в SVG"
        );
    }

    #[test]
    fn rect_control_points_are_its_four_corners() {
        let shape = VectorShape::Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        let pts = shape.control_points();
        assert_eq!(
            pts,
            vec![(10.0, 20.0), (40.0, 20.0), (10.0, 60.0), (40.0, 60.0)]
        );
    }

    #[test]
    fn dragging_a_rect_corner_keeps_the_opposite_corner_fixed() {
        let mut shape = VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        // Индекс 3 — bottom-right (x+w, y+h). Тащим его в (50, 60).
        shape.set_control_point(3, (50.0, 60.0));
        if let VectorShape::Rect { x, y, w, h, .. } = shape {
            assert_eq!(
                (x, y),
                (0.0, 0.0),
                "противоположный угол (top-left) должен остаться на месте"
            );
            assert_eq!((w, h), (50.0, 60.0));
        } else {
            panic!("должен остаться Rect");
        }
    }

    #[test]
    fn dragging_a_rect_corner_past_the_opposite_corner_does_not_go_negative() {
        let mut shape = VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        // Тащим top-left (индекс 0) ЗА противоположный угол (10,10) — в (30,30).
        shape.set_control_point(0, (30.0, 30.0));
        if let VectorShape::Rect { x, y, w, h, .. } = shape {
            assert!(
                w >= 1.0 && h >= 1.0,
                "ширина/высота не должны стать отрицательными: w={w} h={h}"
            );
            assert_eq!(
                (x, y),
                (10.0, 10.0),
                "фигура должна вывернуться, а не схлопнуться в мусор"
            );
        } else {
            panic!("должен остаться Rect");
        }
    }

    #[test]
    fn ellipse_edge_handles_control_rx_and_ry_independently() {
        let mut shape = VectorShape::Ellipse {
            cx: 0.0,
            cy: 0.0,
            rx: 5.0,
            ry: 5.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        shape.set_control_point(1, (20.0, 0.0)); // хендл rx
        if let VectorShape::Ellipse { rx, ry, .. } = &shape {
            assert!((rx - 20.0).abs() < 1e-4);
            assert!(
                (ry - 5.0).abs() < 1e-4,
                "ry не должен был измениться от хендла rx"
            );
        } else {
            panic!();
        }
    }

    #[test]
    fn ellipse_center_handle_moves_the_whole_shape() {
        let mut shape = VectorShape::Ellipse {
            cx: 0.0,
            cy: 0.0,
            rx: 5.0,
            ry: 5.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        shape.set_control_point(0, (12.0, -7.0));
        if let VectorShape::Ellipse { cx, cy, rx, ry, .. } = &shape {
            assert_eq!((*cx, *cy), (12.0, -7.0));
            assert!(
                (rx - 5.0).abs() < 1e-4 && (ry - 5.0).abs() < 1e-4,
                "радиусы не должны меняться от хендла центра"
            );
        } else {
            panic!();
        }
    }

    #[test]
    fn polygon_point_drag_changes_only_the_targeted_point() {
        let mut shape = VectorShape::Polygon {
            points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)],
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        shape.set_control_point(1, (99.0, 99.0));
        if let VectorShape::Polygon { points, .. } = &shape {
            assert_eq!(points[0], (0.0, 0.0), "точка 0 не должна была измениться");
            assert_eq!(points[1], (99.0, 99.0));
            assert_eq!(points[2], (5.0, 10.0), "точка 2 не должна была измениться");
        } else {
            panic!();
        }
    }

    #[test]
    fn out_of_range_control_point_index_is_ignored_not_a_panic() {
        let mut shape = VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RED,
            stroke_width: 1.0,
        };
        shape.set_control_point(99, (5.0, 5.0)); // не должно паниковать
        assert_eq!(
            shape.control_points(),
            vec![(0.0, 0.0), (1.0, 1.0)],
            "ничего не должно было измениться"
        );
    }

    /// Самое сильное доказательство, что парсер — реальный обратный к
    /// сериализатору, а не "вроде работает": собрать документ из ВСЕХ пяти
    /// видов фигур, записать в SVG-текст, разобрать обратно, сравнить с
    /// исходным. Не "не упало" — побитовое совпадение всех полей.
    #[test]
    fn full_round_trip_through_svg_text_preserves_every_shape_exactly() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 1.0,
            y: 2.0,
            w: 30.0,
            h: 40.0,
            fill: RgbaColor::new(200, 50, 60, 255),
            stroke: RgbaColor::new(10, 20, 30, 128),
            stroke_width: 2.5,
            fill_gradient: None,
        });
        doc.add(VectorShape::Ellipse {
            cx: -5.0,
            cy: 6.0,
            rx: 12.0,
            ry: 8.0,
            fill: RgbaColor::new(0, 255, 0, 255),
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        doc.add(VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 50.0,
            y2: -20.0,
            stroke: BLUE_HALF,
            stroke_width: 3.0,
        });
        doc.add(VectorShape::Polyline {
            points: vec![(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)],
            stroke: RED,
            stroke_width: 1.5,
        });
        doc.add(VectorShape::Polygon {
            points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)],
            fill: BLUE_HALF,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });

        let svg_text = doc.to_svg_string();
        let parsed = VectorDoc::from_svg_str(&svg_text)
            .expect("сгенерированный нами же SVG обязан разбираться без ошибок");

        assert_eq!(parsed.shapes.len(), doc.shapes.len());
        for (original, back) in doc.shapes.iter().zip(parsed.shapes.iter()) {
            assert_eq!(
                format!("{original:?}"),
                format!("{back:?}"),
                "фигура должна вернуться такой же после SVG-текста и обратно"
            );
        }
    }

    #[test]
    fn from_svg_str_now_supports_groups_recursively() {
        // Раньше <g> был фатальной ошибкой разбора всего файла — самая
        // частая причина, почему реальные (не собственного вывода)
        // художественные SVG не открывались на редактирование. Теперь
        // группа рекурсивно разбирается, её transform накапливается и
        // применяется к содержимому.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10">
<g id="wrapper"><rect x="0" y="0" width="5" height="5" fill="#ff0000" fill-opacity="1.0" stroke="#000000" stroke-opacity="1.0" stroke-width="1"/></g>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg).expect("группа должна разбираться, не отклоняться");
        assert_eq!(doc.shapes.len(), 1);
        assert!(doc.unsupported.is_empty());
    }

    #[test]
    fn from_svg_str_nested_group_transform_offsets_child_geometry() {
        // Вложенная группа с translate — координаты ребёнка должны
        // сдвинуться в абсолютные координаты документа (флаттенинг).
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
<g transform="translate(10,20)"><rect x="0" y="0" width="5" height="5" fill="#ff0000"/></g>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg).expect("должен разобраться");
        let VectorShape::Rect { x, y, .. } = &doc.shapes[0] else {
            panic!("ожидали Rect")
        };
        assert!(
            (x - 10.0).abs() < 0.01 && (y - 20.0).abs() < 0.01,
            "transform группы должен сдвинуть координаты: ({x},{y})"
        );
    }

    #[test]
    fn from_svg_str_missing_optional_attribute_defaults_instead_of_failing() {
        // Раньше отсутствие/некорректность ЛЮБОГО атрибута (даже
        // необязательного вроде stroke) было фатальной ошибкой разбора
        // всей фигуры. Теперь отсутствующие атрибуты просто дефолтятся —
        // это совершенно обычный, валидный SVG (не у каждой фигуры есть
        // явная обводка).
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
<rect x="1" y="2" width="5" height="4" fill="#ff0000"/>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg)
            .expect("отсутствие необязательных атрибутов не должно приводить к ошибке");
        assert_eq!(doc.shapes.len(), 1);
    }

    #[test]
    fn from_svg_str_style_attribute_overrides_presentation_attributes() {
        // Каскад SVG/CSS: style="fill:..." сильнее presentation-атрибута
        // fill= того же свойства — многие реальные экспортёры (Inkscape)
        // пишут именно через style=.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
<rect x="0" y="0" width="5" height="5" fill="#000000" style="fill:#00ff00;stroke:#0000ff;stroke-width:2"/>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg).expect("должен разобраться");
        let VectorShape::Rect {
            fill,
            stroke,
            stroke_width,
            ..
        } = &doc.shapes[0]
        else {
            panic!("ожидали Rect")
        };
        assert_eq!(
            *fill,
            RgbaColor::new(0, 255, 0, 255),
            "style должен перекрыть presentation-атрибут fill"
        );
        assert_eq!(*stroke, RgbaColor::new(0, 0, 255, 255));
        assert_eq!(*stroke_width, 2.0);
    }

    #[test]
    fn from_svg_str_named_colors_and_shorthand_hex_are_understood() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
<rect x="0" y="0" width="1" height="1" fill="red"/>
<rect x="0" y="0" width="1" height="1" fill="#0f0"/>
<rect x="0" y="0" width="1" height="1" fill="rgb(0,0,255)"/>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg).expect("должен разобраться");
        assert_eq!(doc.shapes.len(), 3);
        let VectorShape::Rect { fill: red, .. } = &doc.shapes[0] else {
            panic!()
        };
        let VectorShape::Rect { fill: green, .. } = &doc.shapes[1] else {
            panic!()
        };
        let VectorShape::Rect { fill: blue, .. } = &doc.shapes[2] else {
            panic!()
        };
        assert_eq!(
            *red,
            RgbaColor::new(255, 0, 0, 255),
            "именованный цвет 'red'"
        );
        assert_eq!(
            *green,
            RgbaColor::new(0, 255, 0, 255),
            "сокращённый hex #0f0"
        );
        assert_eq!(*blue, RgbaColor::new(0, 0, 255, 255), "функция rgb(...)");
    }

    #[test]
    fn from_svg_str_circle_element_becomes_ellipse_with_equal_radii() {
        // <circle> не отдельный вариант VectorShape — представляется как
        // Ellipse с rx==ry, та же геометрия, что и сам SVG считает частным случаем.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
<circle cx="5" cy="5" r="3" fill="#123456"/>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg).expect("должен разобраться");
        let VectorShape::Ellipse { cx, cy, rx, ry, .. } = &doc.shapes[0] else {
            panic!("ожидали Ellipse")
        };
        assert_eq!((*cx, *cy, *rx, *ry), (5.0, 5.0, 3.0, 3.0));
    }

    #[test]
    fn from_svg_str_unsupported_elements_are_listed_not_silently_dropped_or_fatal() {
        // Раздел 29 ТЗ: неизвестные элементы не должны немедленно
        // уничтожаться (или, в данном случае, ронять разбор всего файла) —
        // видимая геометрия документа разбирается как обычно, а
        // неподдержанный элемент (здесь — text, у модели пока нет текстовых
        // объектов) попадает в список `unsupported`, не теряется молча.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
<rect x="0" y="0" width="5" height="5" fill="#ff0000"/>
<text x="1" y="1">hello</text>
</svg>
"##;
        let doc = VectorDoc::from_svg_str(svg)
            .expect("неизвестный элемент не должен ронять разбор всего документа");
        assert_eq!(
            doc.shapes.len(),
            1,
            "видимая геометрия должна разобраться как обычно"
        );
        assert_eq!(doc.unsupported, vec!["text".to_string()]);
    }

    #[test]
    fn from_svg_str_malformed_xml_is_a_clean_error_not_a_panic() {
        let result = VectorDoc::from_svg_str("<svg><rect x=");
        assert!(
            result.is_err(),
            "битый XML должен дать понятную ошибку, не панику"
        );
    }

    // --- Path (разделы 8-9 ТЗ): узлы Безье, C/L/M-сериализация, парсинг ---

    #[test]
    fn path_data_string_emits_L_for_a_pure_corner_path() {
        let nodes = vec![
            PathNode::corner((0.0, 0.0)),
            PathNode::corner((10.0, 0.0)),
            PathNode::corner((10.0, 10.0)),
        ];
        let d = path_data_string(&nodes, false);
        assert_eq!(d, "M 0 0 L 10 0 L 10 10");
    }

    #[test]
    fn path_data_string_emits_C_when_handles_are_present() {
        let mut a = PathNode::corner((0.0, 0.0));
        a.out_handle = Some((5.0, 0.0));
        let mut b = PathNode::corner((20.0, 0.0));
        b.in_handle = Some((15.0, 0.0));
        let d = path_data_string(&[a, b], false);
        assert_eq!(d, "M 0 0 C 5 0 15 0 20 0");
    }

    #[test]
    fn path_data_string_closes_with_Z_for_corner_only_closed_path() {
        let nodes = vec![
            PathNode::corner((0.0, 0.0)),
            PathNode::corner((10.0, 0.0)),
            PathNode::corner((5.0, 10.0)),
        ];
        let d = path_data_string(&nodes, true);
        assert!(d.ends_with(" Z"), "{d}");
    }

    #[test]
    fn parse_path_d_reads_move_and_line() {
        let (nodes, closed) =
            parse_path_d("M 10 10 L 20 10 L 20 20 Z").expect("должен разобраться");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(nodes[1].position, (20.0, 10.0));
        assert_eq!(nodes[2].position, (20.0, 20.0));
        assert!(closed);
        assert!(
            nodes[0].in_handle.is_none() && nodes[0].out_handle.is_none(),
            "чистая прямая — без ручек"
        );
    }

    #[test]
    fn parse_path_d_reads_cubic_bezier_from_the_tdd_example() {
        // Ровно пример из раздела 8 присланного ТЗ.
        let (nodes, closed) = parse_path_d("M 10 10 C 50 0 100 0 150 50 C 100 100 50 100 10 10 Z")
            .expect("должен разобраться");
        assert!(closed);
        assert_eq!(
            nodes.len(),
            3,
            "M даёт первый узел, каждая C добавляет ещё один — итого 1+2=3"
        );
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(nodes[0].out_handle, Some((50.0, 0.0)));
        assert_eq!(nodes[1].position, (150.0, 50.0));
        assert_eq!(nodes[1].in_handle, Some((100.0, 0.0)));
        assert_eq!(
            nodes[2].position,
            (10.0, 10.0),
            "второй C возвращается в исходную точку (10,10)"
        );
    }

    #[test]
    fn parse_path_d_handles_relative_commands() {
        let (nodes, _) = parse_path_d("M 10 10 l 5 0 l 0 5").expect("должен разобраться");
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(
            nodes[1].position,
            (15.0, 10.0),
            "относительный l должен прибавиться к курсору"
        );
        assert_eq!(nodes[2].position, (15.0, 15.0));
    }

    #[test]
    fn parse_path_d_converts_quadratic_to_cubic_exactly() {
        // Q P0=(0,0) Q=(5,10) end=(10,0) -> C1 = 2/3*(5,10) = (3.333,6.667),
        // C2 = end + 2/3*(Q-end) = (10,0)+2/3*(-5,10) = (6.667,6.667).
        let (nodes, _) = parse_path_d("M 0 0 Q 5 10 10 0").expect("должен разобраться");
        assert_eq!(nodes.len(), 2);
        let c1 = nodes[0]
            .out_handle
            .expect("должна появиться исходящая ручка из квадратичной");
        assert!(
            (c1.0 - 3.333).abs() < 0.01 && (c1.1 - 6.667).abs() < 0.01,
            "{c1:?}"
        );
        let c2 = nodes[1].in_handle.expect("должна появиться входящая ручка");
        assert!(
            (c2.0 - 6.667).abs() < 0.01 && (c2.1 - 6.667).abs() < 0.01,
            "{c2:?}"
        );
    }

    #[test]
    fn parse_path_d_converts_a_simple_circular_arc_to_cubic_nodes() {
        // Четверть окружности радиуса 5 от (5,0) до (0,5) (стандартный
        // учебный пример дуги: rx=ry=5, без поворота, small-arc, sweep=1).
        let (nodes, _) =
            parse_path_d("M 5 0 A 5 5 0 0 1 0 5").expect("дуга должна разбираться, не отклоняться");
        assert_eq!(
            nodes.len(),
            2,
            "четверть окружности <=90° — один кубический сегмент, то есть два узла (начало+конец)"
        );
        let end = nodes[1].position;
        assert!(
            (end.0 - 0.0).abs() < 0.01 && (end.1 - 5.0).abs() < 0.01,
            "конец дуги должен быть ровно в конечной точке: {end:?}"
        );
        // Оба узла обязаны иметь ручки — иначе дуга сериализовалась бы как
        // прямая линия (L), полностью теряя форму дуги.
        assert!(
            nodes[0].out_handle.is_some(),
            "начало дуги должно получить исходящую ручку"
        );
        assert!(
            nodes[1].in_handle.is_some(),
            "конец дуги должен получить входящую ручку"
        );
    }

    #[test]
    fn parse_path_d_splits_large_arc_into_multiple_cubic_segments() {
        // Полуокружность (180°) не может быть точно аппроксимирована одним
        // кубическим сегментом (см. doc-комментарий `arc_to_cubic_beziers`:
        // максимум 90° на сегмент) — должна разбиться минимум на 2.
        let (nodes, _) =
            parse_path_d("M 10 0 A 10 10 0 1 1 -10 0").expect("большая дуга должна разбираться");
        assert!(
            nodes.len() >= 3,
            "180° дуга должна дать минимум 2 сегмента (3 узла), получили {}",
            nodes.len()
        );
        let end = nodes.last().unwrap().position;
        assert!(
            (end.0 - (-10.0)).abs() < 0.05 && (end.1 - 0.0).abs() < 0.05,
            "конец должен быть в (-10,0): {end:?}"
        );
    }

    #[test]
    fn parse_path_d_arc_with_zero_radius_degenerates_to_a_line_not_a_panic() {
        // SVG-спека: нулевой радиус — трактуется как прямая линия, не как
        // ошибка и не как NaN/паника.
        let result = parse_path_d("M 0 0 A 0 0 0 0 1 10 10");
        assert!(
            result.is_some(),
            "нулевой радиус не должен приводить к отказу разбора"
        );
        let (nodes, _) = result.unwrap();
        let end = nodes.last().unwrap().position;
        assert!((end.0 - 10.0).abs() < 0.01 && (end.1 - 10.0).abs() < 0.01);
    }

    #[test]
    fn parse_path_d_arc_to_same_point_produces_no_extra_segment() {
        // Дуга из точки в саму себя не рисует ничего (SVG-спека) — не
        // должна ни паниковать, ни добавлять вырожденный узел NaN-геометрии.
        let (nodes, _) =
            parse_path_d("M 5 5 A 3 3 0 0 1 5 5").expect("должен разобраться без паники");
        assert_eq!(nodes.len(), 1, "дуга в ту же точку не добавляет узлов");
    }

    #[test]
    fn parse_path_d_relative_arc_resolves_against_cursor() {
        // Относительная дуга `a` — конечная точка считается от текущего
        // курсора, как и остальные относительные команды (l/c/q/...).
        let (nodes, _) =
            parse_path_d("M 10 10 a 5 5 0 0 1 5 5").expect("относительная дуга должна разбираться");
        let end = nodes.last().unwrap().position;
        assert!(
            (end.0 - 15.0).abs() < 0.01 && (end.1 - 15.0).abs() < 0.01,
            "конец относительной дуги: {end:?}"
        );
    }

    #[test]
    fn parse_path_d_rotated_ellipse_arc_reaches_exact_endpoint() {
        // Эллиптическая (rx != ry) дуга с поворотом x-axis-rotation — самый
        // общий случай команды A. Проверяем главное инвариантное свойство:
        // независимо от параметризации, путь обязан закончиться ТОЧНО в
        // заявленной конечной точке (не "около эллипса").
        let (nodes, _) = parse_path_d("M 0 0 A 20 10 45 0 1 40 20")
            .expect("повёрнутый эллипс должен разбираться");
        let end = nodes.last().unwrap().position;
        assert!(
            (end.0 - 40.0).abs() < 0.01 && (end.1 - 20.0).abs() < 0.01,
            "конечная точка должна быть точной даже для повёрнутого эллипса: {end:?}"
        );
    }

    #[test]
    fn parse_path_d_arc_with_radius_too_small_scales_up_per_svg_spec() {
        // Радиус физически недостаточен, чтобы дотянуться от start до end
        // по кратчайшей дуге — SVG-спека (шаг F.6.6) требует масштабировать
        // rx/ry вверх пропорционально, а НЕ отклонять path как невалидный.
        let result = parse_path_d("M 0 0 A 1 1 0 0 1 100 100"); // радиус 1, но нужно дотянуться на 100
        assert!(
            result.is_some(),
            "слишком маленький радиус должен масштабироваться, а не отклонять path"
        );
        let (nodes, _) = result.unwrap();
        let end = nodes.last().unwrap().position;
        assert!(
            (end.0 - 100.0).abs() < 0.5 && (end.1 - 100.0).abs() < 0.5,
            "конечная точка должна быть достигнута даже после масштабирования радиуса: {end:?}"
        );
    }

    #[test]
    fn full_round_trip_of_a_path_with_an_arc_via_svg_text() {
        // Полный практический сценарий: путь с дугой сериализуется (уже
        // как C — сериализатор всегда пишет только M/L/C/Z, см.
        // `path_data_string`), читается заново, и получившаяся геометрия
        // визуально идентична исходной дуге (сверяем конечную точку и
        // наличие ручек, не точное числовое совпадение параметров дуги,
        // которых после конвертации в Безье уже не существует).
        let (nodes, closed) =
            parse_path_d("M 0 10 A 10 10 0 0 1 10 0 L 10 -10").expect("должен разобраться");
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Path {
            nodes,
            closed,
            fill: RgbaColor::new(0, 0, 0, 0),
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        let svg_text = doc.to_svg_string();
        let reparsed = VectorDoc::from_svg_str(&svg_text)
            .expect("сериализованная дуга-как-безье должна читаться обратно");
        assert_eq!(reparsed.shapes.len(), 1);
    }

    /// Регрессия на реальную жалобу пользователя ("редактирование SVG
    /// работает не на все файлы") — типичный практический паттерн: круг,
    /// нарисованный как path из двух полуокружностей (`A...A...Z`), а не
    /// через примитив `<circle>` — распространённый способ в экспорте из
    /// внешних векторных редакторов (Illustrator, Inkscape и др.), который
    /// раньше ЛЮБОЙ такой файл делал полностью нередактируемым в этом
    /// движке (парсер отклонял весь path целиком при первой же встреченной
    /// команде `A`, честной ошибкой, но всё равно отказом открыть файл).
    #[test]
    fn real_world_circle_drawn_as_two_arcs_now_opens_for_editing() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\">\n  <path d=\"M 10 50 A 40 40 0 1 1 90 50 A 40 40 0 1 1 10 50 Z\" fill=\"#ffcc00\" stroke=\"#333333\" stroke-width=\"2\"/>\n</svg>";
        let doc = VectorDoc::from_svg_str(svg)
            .expect("реальный SVG с двумя дугами теперь должен разбираться, не отклоняться");
        assert_eq!(doc.shapes.len(), 1);
        let VectorShape::Path { nodes, closed, .. } = &doc.shapes[0] else {
            panic!("ожидали Path")
        };
        assert!(*closed);
        assert!(
            nodes.len() >= 4,
            "ожидали несколько узлов от двух дуг по 180° каждая, получили {}",
            nodes.len()
        );
        for n in nodes {
            assert!(
                n.position.0.is_finite() && n.position.1.is_finite(),
                "узел не должен содержать NaN/inf"
            );
        }
    }

    #[test]
    fn full_round_trip_of_a_real_mixed_curve_and_line_path() {
        // Смешанный путь: прямой сегмент + кривой — раздел 8 явно требует
        // поддержки смешивания L и C в одном path.
        let mut doc = VectorDoc::new();
        let mut n0 = PathNode::corner((0.0, 0.0));
        n0.out_handle = Some((10.0, -5.0));
        let mut n1 = PathNode::corner((30.0, 0.0));
        n1.in_handle = Some((20.0, -5.0));
        let n2 = PathNode::corner((50.0, 0.0)); // прямой сегмент от n1 к n2 — без ручек
        doc.add(VectorShape::Path {
            nodes: vec![n0, n1, n2],
            closed: false,
            fill: RgbaColor::new(0, 0, 0, 0),
            stroke: RED,
            stroke_width: 2.0,
            fill_gradient: None,
        });

        let svg_text = doc.to_svg_string();
        let parsed = VectorDoc::from_svg_str(&svg_text).expect("наш же вывод должен разбираться");
        assert_eq!(parsed.shapes.len(), 1);
        let VectorShape::Path { nodes, closed, .. } = &parsed.shapes[0] else {
            panic!("ожидали Path")
        };
        assert!(!closed);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].position, (0.0, 0.0));
        assert_eq!(nodes[2].position, (50.0, 0.0));
        assert!(
            nodes[1].in_handle.is_some(),
            "кривой сегмент должен сохранить ручку"
        );
    }

    #[test]
    fn parses_the_actual_demo_pony_mouth_svg() {
        // Ровно то, что лежит в assets/pony_svg/mouth.svg — реальный
        // художественный SVG с квадратичной кривой, не наш собственный
        // вывод. Это и есть тот файл, который раньше редактор не мог
        // открыть на редактирование (see README).
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 60 24">
  <path d="M5,8 Q30,22 55,8" fill="none" stroke="#4d1a1a" stroke-width="4" stroke-linecap="round"/>
</svg>
"##;
        let doc =
            VectorDoc::from_svg_str(svg).expect("реальный демо-ассет должен теперь разбираться");
        assert_eq!(doc.shapes.len(), 1);
        let VectorShape::Path { nodes, .. } = &doc.shapes[0] else {
            panic!("ожидали Path")
        };
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].position, (5.0, 8.0));
        assert_eq!(nodes[1].position, (55.0, 8.0));
    }

    #[test]
    fn set_control_point_on_path_position_drags_handles_along_with_it() {
        let mut node = PathNode::corner((10.0, 10.0));
        node.in_handle = Some((5.0, 10.0));
        node.out_handle = Some((15.0, 10.0));
        let mut shape = VectorShape::Path {
            nodes: vec![node],
            closed: false,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        // control_points()[0] — позиция узла (единственный узел, значит index 0).
        shape.set_control_point(0, (20.0, 20.0));
        let VectorShape::Path { nodes, .. } = &shape else {
            panic!()
        };
        assert_eq!(nodes[0].position, (20.0, 20.0));
        // Ручки должны были сдвинуться на ту же дельту (+10,+10).
        assert_eq!(nodes[0].in_handle, Some((15.0, 20.0)));
        assert_eq!(nodes[0].out_handle, Some((25.0, 20.0)));
    }

    #[test]
    fn path_control_point_kind_identifies_position_and_handles_correctly() {
        let mut n0 = PathNode::corner((0.0, 0.0));
        n0.out_handle = Some((5.0, 0.0));
        let mut n1 = PathNode::corner((20.0, 0.0));
        n1.in_handle = Some((15.0, 0.0));
        let shape = VectorShape::Path {
            nodes: vec![n0, n1],
            closed: false,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        };
        // Порядок обхода: n0.position(0), n0.out_handle(1), n1.position(2), n1.in_handle(3).
        assert_eq!(
            shape.path_control_point_kind(0),
            Some((0, PathPointKind::Position))
        );
        assert_eq!(
            shape.path_control_point_kind(1),
            Some((0, PathPointKind::OutHandle))
        );
        assert_eq!(
            shape.path_control_point_kind(2),
            Some((1, PathPointKind::Position))
        );
        assert_eq!(
            shape.path_control_point_kind(3),
            Some((1, PathPointKind::InHandle))
        );
        assert_eq!(shape.path_control_point_kind(99), None);
    }

    #[test]
    fn sync_handles_after_drag_keeps_symmetric_node_handles_opposite_and_equal_length() {
        let mut node = PathNode::symmetric((10.0, 10.0), (1.0, 0.0), 5.0);
        // Двигаем out_handle в новое место, подальше и в другом направлении.
        node.out_handle = Some((10.0, 20.0));
        node.sync_handles_after_drag(false); // false = двигали out_handle
        let in_h = node.in_handle.expect("ручка должна остаться");
        // Symmetric: in_handle должна оказаться точно напротив на ТОЙ ЖЕ длине (10 единиц).
        let dx = in_h.0 - node.position.0;
        let dy = in_h.1 - node.position.1;
        let len = (dx * dx + dy * dy).sqrt();
        assert!(
            (len - 10.0).abs() < 0.01,
            "длина должна остаться той же: {len}"
        );
        // Направление противоположно движению out_handle (0,10) от узла -> in должна быть в (0,-10) направлении.
        assert!(dx.abs() < 0.01 && dy < 0.0, "{in_h:?}");
    }

    #[test]
    fn sync_handles_after_drag_does_nothing_for_corner_nodes() {
        let mut node = PathNode::corner((10.0, 10.0));
        node.in_handle = Some((5.0, 10.0));
        node.out_handle = Some((15.0, 10.0));
        node.out_handle = Some((30.0, 30.0)); // "перетащили" ручку в произвольное место
        node.sync_handles_after_drag(false);
        assert_eq!(
            node.in_handle,
            Some((5.0, 10.0)),
            "corner-узел не должен трогать вторую ручку вообще"
        );
    }

    // ------------------------------------------------------------------
    // Symbols / Instances (раздел 28 + 95 ТЗ) — Задача 4 списка "Adobe Animate".
    // ------------------------------------------------------------------

    fn eye_symbol_def() -> SymbolDef {
        SymbolDef {
            name: "eye".to_string(),
            shapes: vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 5.0,
                ry: 3.0,
                fill: RgbaColor::new(0, 0, 0, 255),
                stroke: RgbaColor::new(0, 0, 0, 0),
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        }
    }

    #[test]
    fn resolve_symbol_instance_applies_translation() {
        let mut doc = VectorDoc::new();
        doc.symbols.push(eye_symbol_def());
        let resolved =
            resolve_symbol_instance(&doc, "eye", (1.0, 0.0, 0.0, 1.0, 100.0, 50.0), None, 8);
        assert_eq!(resolved.len(), 1);
        let VectorShape::Ellipse { cx, cy, .. } = resolved[0] else {
            panic!("expected ellipse")
        };
        assert!(
            (cx - 100.0).abs() < 0.001 && (cy - 50.0).abs() < 0.001,
            "должен сдвинуться в точку транcформа: {cx},{cy}"
        );
    }

    #[test]
    fn resolve_symbol_instance_mirrors_via_negative_scale_x_for_left_right_pair() {
        // Раздел 28 ТЗ пример: Eye -> Eye_L/Eye_R через один SymbolDef.
        let mut doc = VectorDoc::new();
        doc.symbols.push(SymbolDef {
            name: "eye".to_string(),
            shapes: vec![VectorShape::Ellipse {
                cx: 3.0,
                cy: 0.0,
                rx: 5.0,
                ry: 3.0,
                fill: RgbaColor::new(0, 0, 0, 255),
                stroke: RgbaColor::new(0, 0, 0, 0),
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        });
        let right =
            resolve_symbol_instance(&doc, "eye", (1.0, 0.0, 0.0, 1.0, 100.0, 50.0), None, 8);
        let left =
            resolve_symbol_instance(&doc, "eye", (-1.0, 0.0, 0.0, 1.0, 100.0, 50.0), None, 8); // scale_x: -1 — зеркалирование
        let VectorShape::Ellipse { cx: rx, .. } = right[0] else {
            panic!()
        };
        let VectorShape::Ellipse { cx: lx, .. } = left[0] else {
            panic!()
        };
        // Правый глаз смещён center.x=3 -> 103 (100+3); зеркальный левый -> 97 (100-3).
        assert!((rx - 103.0).abs() < 0.001, "{rx}");
        assert!((lx - 97.0).abs() < 0.001, "{lx}");
    }

    #[test]
    fn resolve_symbol_instance_fill_override_recolors_without_touching_definition() {
        let mut doc = VectorDoc::new();
        doc.symbols.push(eye_symbol_def());
        let green = RgbaColor::new(0, 255, 0, 255);
        let resolved =
            resolve_symbol_instance(&doc, "eye", (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), Some(green), 8);
        let VectorShape::Ellipse { fill, .. } = resolved[0] else {
            panic!()
        };
        assert_eq!(fill, green);
        // Определение символа не изменилось — override затрагивает только этот резолв.
        assert_eq!(
            doc.symbols[0].shapes[0].bounds(),
            eye_symbol_def().shapes[0].bounds()
        );
        let VectorShape::Ellipse { fill: def_fill, .. } = doc.symbols[0].shapes[0] else {
            panic!()
        };
        assert_eq!(
            def_fill,
            RgbaColor::new(0, 0, 0, 255),
            "SymbolDef должен остаться нетронутым"
        );
    }

    #[test]
    fn resolve_symbol_instance_missing_symbol_returns_empty_not_panic() {
        let doc = VectorDoc::new();
        let resolved = resolve_symbol_instance(
            &doc,
            "does_not_exist",
            (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            None,
            8,
        );
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_symbol_instance_editing_definition_affects_all_instances() {
        // Раздел 28 ТЗ: "Обе копии ссылаются на один Symbol Definition" —
        // правка определения должна быть видна через resolve на ОБОИХ инстансах.
        let mut doc = VectorDoc::new();
        doc.symbols.push(eye_symbol_def());
        let before = resolve_symbol_instance(&doc, "eye", (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), None, 8);
        let VectorShape::Ellipse { rx: rx_before, .. } = before[0] else {
            panic!()
        };
        assert_eq!(rx_before, 5.0);

        doc.symbols[0].shapes[0].set_control_point(1, (8.0, 0.0)); // тянем rx-держатель дальше

        let after = resolve_symbol_instance(&doc, "eye", (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), None, 8);
        let VectorShape::Ellipse { rx: rx_after, .. } = after[0] else {
            panic!()
        };
        assert_eq!(
            rx_after, 8.0,
            "правка SymbolDef должна отражаться на инстансе"
        );
    }

    #[test]
    fn nested_instance_composes_transforms_correctly() {
        // symbol "dot" — точка в начале координат. symbol "pair" — два
        // инстанса "dot" со своими смещениями. Инстанс "pair" сдвинут ещё
        // на (100,100) — итоговые позиции точек должны сложить оба сдвига.
        let mut doc = VectorDoc::new();
        doc.symbols.push(SymbolDef {
            name: "dot".to_string(),
            shapes: vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 1.0,
                ry: 1.0,
                fill: RgbaColor::new(0, 0, 0, 255),
                stroke: RgbaColor::new(0, 0, 0, 0),
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        });
        doc.symbols.push(SymbolDef {
            name: "pair".to_string(),
            shapes: vec![
                VectorShape::Instance {
                    symbol: "dot".to_string(),
                    transform: (1.0, 0.0, 0.0, 1.0, -10.0, 0.0),
                    fill_override: None,
                },
                VectorShape::Instance {
                    symbol: "dot".to_string(),
                    transform: (1.0, 0.0, 0.0, 1.0, 10.0, 0.0),
                    fill_override: None,
                },
            ],
        });
        let resolved =
            resolve_symbol_instance(&doc, "pair", (1.0, 0.0, 0.0, 1.0, 100.0, 100.0), None, 8);
        assert_eq!(resolved.len(), 2);
        let mut xs: Vec<f32> = resolved
            .iter()
            .map(|s| {
                let VectorShape::Ellipse { cx, .. } = s else {
                    panic!()
                };
                *cx
            })
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((xs[0] - 90.0).abs() < 0.001, "{xs:?}"); // 100 - 10
        assert!((xs[1] - 110.0).abs() < 0.001, "{xs:?}"); // 100 + 10
    }

    #[test]
    fn recursive_self_referencing_symbol_terminates_instead_of_hanging() {
        // Символ, инстанс которого ссылается сам на себя — некорректные
        // данные (например, битый .asset после ручного редактирования RON),
        // но resolve_symbol_instance должен безопасно оборваться по
        // depth_budget, а не зациклиться/переполнить стек.
        let mut doc = VectorDoc::new();
        doc.symbols.push(SymbolDef {
            name: "loop".to_string(),
            shapes: vec![VectorShape::Instance {
                symbol: "loop".to_string(),
                transform: (1.0, 0.0, 0.0, 1.0, 1.0, 1.0),
                fill_override: None,
            }],
        });
        let resolved =
            resolve_symbol_instance(&doc, "loop", (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), None, 8);
        // Каждый уровень рекурсии производит 0 конкретных фигур (только
        // дальнейшую Instance-ветку) — итог должен быть пустым, не паникой/зависанием.
        assert!(resolved.is_empty());
    }

    #[test]
    fn break_apart_symbol_instance_produces_independent_copy() {
        let mut doc = VectorDoc::new();
        doc.symbols.push(eye_symbol_def());
        doc.add(VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 50.0, 50.0),
            fill_override: None,
        });
        assert_eq!(doc.shapes.len(), 1);

        let ok = doc.break_apart_symbol_instance(0);
        assert!(ok);
        assert_eq!(doc.shapes.len(), 1); // eye_symbol_def имеет ровно 1 фигуру
        let VectorShape::Ellipse { cx, cy, .. } = doc.shapes[0] else {
            panic!("expected inlined ellipse, got {:?}", doc.shapes[0])
        };
        assert!((cx - 50.0).abs() < 0.001 && (cy - 50.0).abs() < 0.001);

        // Дальнейшая правка SymbolDef НЕ должна влиять на разорванную копию.
        doc.symbols[0].shapes[0].set_control_point(1, (99.0, 0.0));
        let VectorShape::Ellipse { rx, .. } = doc.shapes[0] else {
            panic!()
        };
        assert_eq!(
            rx, 5.0,
            "разорванная копия должна остаться независимой от SymbolDef"
        );
    }

    #[test]
    fn break_apart_on_non_instance_index_is_a_safe_no_op() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            fill: RED,
            stroke: RED,
            stroke_width: 1.0,
            fill_gradient: None,
        });
        assert!(!doc.break_apart_symbol_instance(0));
        assert_eq!(doc.shapes.len(), 1);
    }

    #[test]
    fn upsert_symbol_replaces_existing_definition_by_name() {
        let mut doc = VectorDoc::new();
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 1.0,
                ry: 1.0,
                fill: RED,
                stroke: RED,
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        );
        assert_eq!(doc.symbols.len(), 1);
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 9.0,
                ry: 9.0,
                fill: RED,
                stroke: RED,
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        );
        assert_eq!(
            doc.symbols.len(),
            1,
            "то же имя — замена, не второе определение"
        );
        let VectorShape::Ellipse { rx, .. } = doc.symbols[0].shapes[0] else {
            panic!()
        };
        assert_eq!(rx, 9.0);
    }

    #[test]
    fn new_instance_centered_at_offsets_by_symbol_bounds_center() {
        let mut doc = VectorDoc::new();
        // Символ с габаритом от (0,0) до (10,10) -> центр (5,5).
        doc.upsert_symbol(
            "box",
            vec![VectorShape::Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                fill: RED,
                stroke: RED,
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        );
        let inst = doc
            .new_instance_centered_at("box", 200.0, 300.0)
            .expect("symbol exists");
        let VectorShape::Instance { transform, .. } = inst else {
            panic!()
        };
        // Смещение должно быть таким, чтобы центр (5,5) символа оказался в (200,300):
        // применяем resolve и проверяем итоговый центр фигуры.
        let resolved = resolve_symbol_instance(&doc, "box", transform, None, 8);
        let VectorShape::Rect { x, y, w, h, .. } = resolved[0] else {
            panic!()
        };
        let center = (x + w / 2.0, y + h / 2.0);
        assert!(
            (center.0 - 200.0).abs() < 0.001 && (center.1 - 300.0).abs() < 0.001,
            "{center:?}"
        );
    }

    #[test]
    fn new_instance_centered_at_unknown_symbol_returns_none() {
        let doc = VectorDoc::new();
        assert!(doc.new_instance_centered_at("nope", 0.0, 0.0).is_none());
    }

    #[test]
    fn to_svg_string_writes_symbol_defs_and_use_referencing_them() {
        let mut doc = VectorDoc::new();
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 5.0,
                ry: 3.0,
                fill: RED,
                stroke: RED,
                stroke_width: 1.0,
                fill_gradient: None,
            }],
        );
        doc.add(VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 50.0, 50.0),
            fill_override: None,
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains("<defs>"), "{svg}");
        assert!(svg.contains(r#"<symbol id="symbol_eye">"#), "{svg}");
        assert!(svg.contains(r##"href="#symbol_eye""##), "{svg}");
        assert!(svg.contains("matrix(1 0 0 1 50 50)"), "{svg}");
    }

    #[test]
    fn to_svg_string_use_with_fill_override_writes_style_fill() {
        let mut doc = VectorDoc::new();
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 5.0,
                ry: 3.0,
                fill: RED,
                stroke: RED,
                stroke_width: 1.0,
                fill_gradient: None,
            }],
        );
        let green = RgbaColor::new(0, 255, 0, 255);
        doc.add(VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            fill_override: Some(green),
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains("style=\"fill:#00ff00"), "{svg}");
    }

    #[test]
    fn from_svg_str_round_trips_symbol_and_use() {
        let mut doc = VectorDoc::new();
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 5.0,
                ry: 3.0,
                fill: RgbaColor::new(10, 20, 30, 255),
                stroke: RgbaColor::new(0, 0, 0, 0),
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        );
        doc.add(VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 42.0, 17.0),
            fill_override: None,
        });
        let svg = doc.to_svg_string();

        let reparsed = VectorDoc::from_svg_str(&svg).expect("должен разобраться обратно");
        assert_eq!(
            reparsed.symbols.len(),
            1,
            "unsupported: {:?}",
            reparsed.unsupported
        );
        assert_eq!(reparsed.symbols[0].name, "eye");
        assert_eq!(reparsed.shapes.len(), 1);
        let VectorShape::Instance {
            symbol, transform, ..
        } = &reparsed.shapes[0]
        else {
            panic!("expected Instance, got {:?}", reparsed.shapes[0])
        };
        assert_eq!(symbol, "eye");
        assert!(
            (transform.4 - 42.0).abs() < 0.01 && (transform.5 - 17.0).abs() < 0.01,
            "{transform:?}"
        );

        // Резолвится в ту же геометрию, что и до сохранения.
        let resolved = resolve_symbol_instance(&reparsed, "eye", *transform, None, 8);
        let VectorShape::Ellipse { cx, cy, fill, .. } = resolved[0] else {
            panic!()
        };
        assert!((cx - 42.0).abs() < 0.01 && (cy - 17.0).abs() < 0.01);
        assert_eq!(fill, RgbaColor::new(10, 20, 30, 255));
    }

    #[test]
    fn from_svg_str_use_without_href_is_reported_not_silently_dropped() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><use/></svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("malformed use should not be fatal");
        assert!(
            doc.unsupported.iter().any(|s| s.contains("use")),
            "{:?}",
            doc.unsupported
        );
    }

    #[test]
    fn from_svg_str_symbol_without_id_is_reported_not_silently_dropped() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><symbol><rect width="1" height="1"/></symbol></defs></svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("malformed symbol should not be fatal");
        assert!(
            doc.unsupported.iter().any(|s| s.contains("symbol")),
            "{:?}",
            doc.unsupported
        );
        assert!(doc.symbols.is_empty());
    }

    #[test]
    fn set_fill_on_instance_sets_override_without_touching_symbol_def() {
        let mut doc = VectorDoc::new();
        doc.upsert_symbol(
            "eye",
            vec![VectorShape::Ellipse {
                cx: 0.0,
                cy: 0.0,
                rx: 1.0,
                ry: 1.0,
                fill: RED,
                stroke: RED,
                stroke_width: 0.0,
                fill_gradient: None,
            }],
        );
        let mut inst = doc.new_instance_centered_at("eye", 0.0, 0.0).unwrap();
        let blue = RgbaColor::new(0, 0, 255, 255);
        inst.set_fill(blue);
        let VectorShape::Instance { fill_override, .. } = inst else {
            panic!()
        };
        assert_eq!(fill_override, Some(blue));
    }

    #[test]
    fn control_points_on_instance_is_single_origin_point_and_is_draggable() {
        let mut shape = VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 10.0, 20.0),
            fill_override: None,
        };
        assert_eq!(shape.control_points(), vec![(10.0, 20.0)]);
        shape.set_control_point(0, (99.0, 88.0));
        let VectorShape::Instance { transform, .. } = shape else {
            panic!()
        };
        assert!((transform.4 - 99.0).abs() < 0.001 && (transform.5 - 88.0).abs() < 0.001);
    }

    // ---- Раздел 60 ТЗ: Gradients ----------------------------------------

    #[test]
    fn gradient_sample_interpolates_linearly_between_two_stops() {
        let g = GradientDef::new(
            "g1",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(200, 100, 50, 255),
                },
            ],
        );
        let mid = g.sample(0.5);
        assert_eq!(mid, RgbaColor::new(100, 50, 25, 255));
        assert_eq!(g.sample(0.0), RgbaColor::new(0, 0, 0, 255));
        assert_eq!(g.sample(1.0), RgbaColor::new(200, 100, 50, 255));
    }

    #[test]
    fn gradient_sample_clamps_t_outside_0_1_range() {
        let g = GradientDef::new(
            "g1",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(10, 20, 30, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(200, 210, 220, 255),
                },
            ],
        );
        assert_eq!(g.sample(-5.0), RgbaColor::new(10, 20, 30, 255));
        assert_eq!(g.sample(5.0), RgbaColor::new(200, 210, 220, 255));
    }

    #[test]
    fn gradient_sample_uses_two_nearest_stops_among_three() {
        let g = GradientDef::new(
            "g1",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
                GradientStop {
                    offset: 0.5,
                    color: RgbaColor::new(100, 100, 100, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
            ],
        );
        // Ровно в середине между вторым и третьим стопом (0.75) — должен
        // интерполировать между 100 и 0, а не "перепрыгнуть" через средний
        // стоп к первому/последнему.
        assert_eq!(g.sample(0.75), RgbaColor::new(50, 50, 50, 255));
    }

    #[test]
    fn gradient_sample_unsorted_stops_gives_same_result_as_sorted() {
        // Раздел 60 ТЗ: порядок хранения стопов — не инвариант, sample()
        // обязана сортировать сама (см. doc-comment у GradientStop).
        let sorted = GradientDef::new(
            "g1",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(255, 255, 255, 255),
                },
            ],
        );
        let unsorted = GradientDef::new(
            "g1",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(255, 255, 255, 255),
                },
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
            ],
        );
        assert_eq!(sorted.sample(0.3), unsorted.sample(0.3));
    }

    #[test]
    fn gradient_sample_with_empty_stops_returns_neutral_gray_not_panic() {
        let g = GradientDef::new(
            "empty",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![],
        );
        assert_eq!(g.sample(0.5), RgbaColor::new(128, 128, 128, 255));
        assert_eq!(g.average_color(), RgbaColor::new(128, 128, 128, 255));
    }

    #[test]
    fn gradient_sample_with_single_stop_returns_that_stop_everywhere() {
        let g = GradientDef::new(
            "one",
            GradientKind::Radial {
                cx: 0.0,
                cy: 0.0,
                r: 1.0,
            },
            vec![GradientStop {
                offset: 0.5,
                color: RgbaColor::new(1, 2, 3, 4),
            }],
        );
        assert_eq!(g.sample(0.0), RgbaColor::new(1, 2, 3, 4));
        assert_eq!(g.sample(1.0), RgbaColor::new(1, 2, 3, 4));
    }

    #[test]
    fn gradient_average_color_is_unweighted_mean_of_stops() {
        let g = GradientDef::new(
            "avg",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0, 0, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(100, 200, 250, 255),
                },
            ],
        );
        assert_eq!(g.average_color(), RgbaColor::new(50, 100, 125, 255));
    }

    #[test]
    fn gradient_t_at_linear_projects_along_axis_unclamped() {
        let kind = GradientKind::Linear {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 0.0,
        };
        assert!((gradient_t_at(&kind, 0.0, 0.0) - 0.0).abs() < 1e-5);
        assert!((gradient_t_at(&kind, 5.0, 0.0) - 0.5).abs() < 1e-5);
        assert!((gradient_t_at(&kind, 10.0, 0.0) - 1.0).abs() < 1e-5);
        // За пределами отрезка — НЕ зажато (это делает sample, не эта
        // функция), значение продолжает линейную проекцию.
        assert!((gradient_t_at(&kind, 20.0, 0.0) - 2.0).abs() < 1e-5);
        assert!(gradient_t_at(&kind, -10.0, 0.0) < 0.0);
    }

    #[test]
    fn gradient_t_at_linear_perpendicular_offset_does_not_shift_t() {
        // Точка, сдвинутая перпендикулярно оси градиента, должна давать тот
        // же t, что и её проекция на саму ось — скалярная проекция, а не
        // расстояние по прямой до одной из точек.
        let kind = GradientKind::Linear {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 0.0,
        };
        assert!((gradient_t_at(&kind, 5.0, 0.0) - gradient_t_at(&kind, 5.0, 100.0)).abs() < 1e-4);
    }

    #[test]
    fn gradient_t_at_linear_degenerate_zero_length_axis_returns_zero_not_nan() {
        let kind = GradientKind::Linear {
            x1: 3.0,
            y1: 3.0,
            x2: 3.0,
            y2: 3.0,
        };
        let t = gradient_t_at(&kind, 100.0, 100.0);
        assert_eq!(t, 0.0);
        assert!(!t.is_nan());
    }

    #[test]
    fn gradient_t_at_radial_is_fraction_of_radius_from_center() {
        let kind = GradientKind::Radial {
            cx: 0.0,
            cy: 0.0,
            r: 10.0,
        };
        assert!((gradient_t_at(&kind, 0.0, 0.0) - 0.0).abs() < 1e-5);
        assert!((gradient_t_at(&kind, 10.0, 0.0) - 1.0).abs() < 1e-5);
        assert!((gradient_t_at(&kind, 0.0, 5.0) - 0.5).abs() < 1e-5);
        // За пределами круга — не зажато, продолжает расти.
        assert!((gradient_t_at(&kind, 20.0, 0.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn gradient_t_at_radial_degenerate_zero_radius_returns_zero_not_nan() {
        let kind = GradientKind::Radial {
            cx: 0.0,
            cy: 0.0,
            r: 0.0,
        };
        let t = gradient_t_at(&kind, 5.0, 5.0);
        assert_eq!(t, 0.0);
        assert!(!t.is_nan());
    }

    #[test]
    fn vector_doc_upsert_gradient_replaces_same_name_not_duplicates() {
        let mut doc = VectorDoc::new();
        doc.upsert_gradient(GradientDef::new(
            "g",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![GradientStop {
                offset: 0.0,
                color: RgbaColor::new(1, 1, 1, 255),
            }],
        ));
        doc.upsert_gradient(GradientDef::new(
            "g",
            GradientKind::Radial {
                cx: 0.0,
                cy: 0.0,
                r: 1.0,
            },
            vec![GradientStop {
                offset: 0.0,
                color: RgbaColor::new(2, 2, 2, 255),
            }],
        ));
        assert_eq!(doc.gradients.len(), 1);
        let g = doc.find_gradient("g").expect("gradient should exist");
        assert!(matches!(g.kind, GradientKind::Radial { .. }));
        assert_eq!(g.stops[0].color, RgbaColor::new(2, 2, 2, 255));
    }

    #[test]
    fn vector_doc_find_gradient_returns_none_for_unknown_name() {
        let doc = VectorDoc::new();
        assert!(doc.find_gradient("does_not_exist").is_none());
    }

    #[test]
    fn vector_doc_remove_gradient_drops_it_but_shape_reference_survives_as_dangling() {
        let mut doc = VectorDoc::new();
        doc.upsert_gradient(GradientDef::new(
            "g",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
            },
            vec![GradientStop {
                offset: 0.0,
                color: RgbaColor::new(9, 9, 9, 255),
            }],
        ));
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: Some("g".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        doc.remove_gradient("g");
        assert!(doc.find_gradient("g").is_none());
        // Честный откат: ссылка на удалённый градиент остаётся на фигуре
        // (не паникует, не удаляет фигуру), но резолвиться в SVG в реальный
        // paint уже не будет — см. gradient_to_svg_falls_back_to_flat_fill_when_dangling.
        let VectorShape::Rect { fill_gradient, .. } = &doc.shapes[0] else {
            panic!()
        };
        assert_eq!(fill_gradient.as_deref(), Some("g"));
    }

    #[test]
    fn set_fill_gradient_sets_name_and_fallback_color_together() {
        let mut shape = VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        shape.set_fill_gradient(Some("sunset".to_string()), RgbaColor::new(7, 8, 9, 255));
        assert_eq!(shape.fill_gradient_name(), Some("sunset"));
        let VectorShape::Rect { fill, .. } = &shape else {
            panic!()
        };
        assert_eq!(*fill, RgbaColor::new(7, 8, 9, 255));
    }

    #[test]
    fn set_fill_gradient_none_clears_gradient_and_sets_flat_fill() {
        let mut shape = VectorShape::Ellipse {
            cx: 0.0,
            cy: 0.0,
            rx: 1.0,
            ry: 1.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: Some("old".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        shape.set_fill_gradient(None, RgbaColor::new(1, 2, 3, 255));
        assert_eq!(shape.fill_gradient_name(), None);
        let VectorShape::Ellipse { fill, .. } = &shape else {
            panic!()
        };
        assert_eq!(*fill, RgbaColor::new(1, 2, 3, 255));
    }

    #[test]
    fn set_fill_gradient_is_a_no_op_on_line_and_instance() {
        let mut line = VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RgbaColor::new(0, 0, 0, 255),
            stroke_width: 1.0,
        };
        line.set_fill_gradient(Some("g".to_string()), RgbaColor::new(9, 9, 9, 255));
        assert_eq!(line.fill_gradient_name(), None);

        let mut instance = VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            fill_override: None,
        };
        instance.set_fill_gradient(Some("g".to_string()), RgbaColor::new(9, 9, 9, 255));
        assert_eq!(instance.fill_gradient_name(), None);
    }

    #[test]
    fn set_fill_clears_any_previously_set_gradient() {
        let mut shape = VectorShape::Polygon {
            points: vec![(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)],
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: Some("g".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        shape.set_fill(RgbaColor::new(5, 6, 7, 255));
        assert_eq!(shape.fill_gradient_name(), None);
    }

    #[test]
    fn gradient_to_svg_string_writes_linear_defs_with_stops() {
        let mut doc = VectorDoc::new();
        doc.upsert_gradient(GradientDef::new(
            "sunset",
            GradientKind::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 0.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(255, 0, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0, 0, 255, 128),
                },
            ],
        ));
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: Some("sunset".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r#"<linearGradient id="gradient_sunset""#));
        assert!(svg.contains(r#"gradientUnits="userSpaceOnUse""#));
        assert!(svg.contains(r#"fill="url(#gradient_sunset)""#));
        assert!(svg.contains("stop-color="));
    }

    #[test]
    fn gradient_to_svg_falls_back_to_flat_fill_when_dangling() {
        // Раздел 60 ТЗ: ссылка на несуществующий градиент — честный откат
        // на плоский fill в сериализованном SVG, а не url() в никуда.
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: RgbaColor::new(11, 22, 33, 255),
            fill_gradient: Some("does_not_exist".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        let svg = doc.to_svg_string();
        assert!(!svg.contains("url(#gradient_does_not_exist)"));
        assert!(svg.contains(&RgbaColor::new(11, 22, 33, 255).to_hex()));
    }

    #[test]
    fn gradient_svg_round_trip_through_a_real_parser_preserves_kind_and_stops() {
        let mut doc = VectorDoc::new();
        doc.upsert_gradient(GradientDef::new(
            "sunset",
            GradientKind::Radial {
                cx: 5.0,
                cy: 5.0,
                r: 4.0,
            },
            vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(255, 200, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(255, 0, 0, 200),
                },
            ],
        ));
        doc.add(VectorShape::Ellipse {
            cx: 5.0,
            cy: 5.0,
            rx: 5.0,
            ry: 5.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: Some("sunset".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        });
        let svg = doc.to_svg_string();

        // pony-core сам не тянет usvg/resvg/tiny-skia (эти зависимости —
        // только у pony-render, который их реально растеризует) — здесь
        // раунд-трип проверяется через собственный `from_svg_str`, тем же
        // приёмом, что и другие round-trip тесты в этом модуле (символы,
        // группы, маски). Независимая проверка через настоящий resvg-рендер
        // тех же данных — см.
        // `pony_render::texture::vector_roundtrip_tests::drawn_gradient_ellipse_renders_the_correct_stop_color_through_a_real_svg_parser`.
        let parsed = VectorDoc::from_svg_str(&svg).expect("parse back");
        assert_eq!(parsed.gradients.len(), 1);
        let g = &parsed.gradients[0];
        assert_eq!(g.name, "sunset");
        assert!(matches!(g.kind, GradientKind::Radial { .. }));
        assert_eq!(g.stops.len(), 2);
        let VectorShape::Ellipse { fill_gradient, .. } = &parsed.shapes[0] else {
            panic!()
        };
        assert_eq!(fill_gradient.as_deref(), Some("sunset"));
    }

    #[test]
    fn gradient_svg_parse_is_order_independent_when_defs_appear_after_shape_reference() {
        // Раздел 60 ТЗ: SVG не гарантирует, что <defs> с градиентом стоит
        // раньше фигуры, которая на него ссылается — from_svg_str должна
        // пересчитать честный fallback-цвет фигуры уже после полного
        // разбора документа (см. sync_gradient_fallback_colors), а не в
        // порядке обхода узлов.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <rect x="0" y="0" width="10" height="10" fill="url(#gradient_late)"/>
            <defs>
                <linearGradient id="gradient_late" x1="0" y1="0" x2="10" y2="0" gradientUnits="userSpaceOnUse">
                    <stop offset="0" stop-color="#102030"/>
                    <stop offset="1" stop-color="#a0b0c0"/>
                </linearGradient>
            </defs>
        </svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("parse");
        assert_eq!(doc.gradients.len(), 1);
        let VectorShape::Rect { fill, fill_gradient, .. } = &doc.shapes[0] else {
            panic!()
        };
        assert_eq!(fill_gradient.as_deref(), Some("late"));
        // fill должен стать средним цветом градиента (не остаться серым
        // плейсхолдером) благодаря пост-обработке после парсинга.
        assert_eq!(*fill, doc.gradients[0].average_color());
    }

    #[test]
    fn gradient_svg_parse_stop_opacity_and_style_cascade_are_honored() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs>
                <linearGradient id="gradient_g" x1="0" y1="0" x2="10" y2="0" gradientUnits="userSpaceOnUse">
                    <stop offset="0" stop-color="#ff0000" stop-opacity="0.5"/>
                    <stop offset="1" style="stop-color:#00ff00;stop-opacity:1"/>
                </linearGradient>
            </defs>
            <rect x="0" y="0" width="10" height="10" fill="url(#gradient_g)"/>
        </svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("parse");
        let g = doc.find_gradient("g").expect("gradient should be parsed");
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].color.r, 255);
        assert_eq!(g.stops[0].color.a, 128); // 255 * 0.5, округлено
        assert_eq!(g.stops[1].color.g, 255);
        assert_eq!(g.stops[1].color.a, 255);
    }

    #[test]
    fn gradient_svg_parse_without_id_is_reported_not_silently_dropped() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs>
                <linearGradient x1="0" y1="0" x2="10" y2="0">
                    <stop offset="0" stop-color="#ff0000"/>
                </linearGradient>
            </defs>
        </svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("parse");
        assert_eq!(doc.gradients.len(), 0);
        assert!(doc.unsupported.iter().any(|u| u.contains("linearGradient")));
    }

    #[test]
    fn gradient_svg_parse_without_stops_is_reported_not_silently_dropped() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs>
                <linearGradient id="gradient_empty" x1="0" y1="0" x2="10" y2="0"/>
            </defs>
        </svg>"##;
        let doc = VectorDoc::from_svg_str(svg).expect("parse");
        assert_eq!(doc.gradients.len(), 1);
        assert!(doc
            .unsupported
            .iter()
            .any(|u| u.contains("empty") && u.contains("стопов")));
    }

    #[test]
    fn transform_shape_strips_gradient_reference_but_keeps_flat_fill() {
        // См. комментарий у transform_shape: градиенты в абсолютном
        // document-space визуально "отвязались" бы от трансформированной
        // геометрии символьного инстанса, поэтому transform_shape намеренно
        // роняет ссылку на градиент, но сохраняет последний известный
        // плоский fill как честное приближение.
        let shape = VectorShape::Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            fill: RgbaColor::new(42, 42, 42, 255),
            fill_gradient: Some("g".to_string()),
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        let transformed = transform_shape(&shape, (1.0, 0.0, 0.0, 1.0, 5.0, 5.0));
        let VectorShape::Rect { fill, fill_gradient, .. } = &transformed else {
            panic!()
        };
        assert_eq!(fill_gradient, &None);
        assert_eq!(*fill, RgbaColor::new(42, 42, 42, 255));
    }

    #[test]
    fn extract_gradient_ref_recognizes_our_own_url_format_and_rejects_others() {
        assert_eq!(
            extract_gradient_ref("url(#gradient_sunset)"),
            Some("sunset".to_string())
        );
        assert_eq!(extract_gradient_ref("#ff0000"), None);
        assert_eq!(extract_gradient_ref("none"), None);
        assert_eq!(extract_gradient_ref("url(#other_thing)"), None);
    }
}
