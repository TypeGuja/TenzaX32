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
        Self { position, in_handle: None, out_handle: None, node_type: NodeType::Corner }
    }

    /// Симметричный узел с ручками на расстоянии `handle_len` по обе
    /// стороны вдоль направления `(dx, dy)` — удобный конструктор для
    /// программной генерации гладких кривых (например, автосглаживание).
    pub fn symmetric(position: (f32, f32), direction: (f32, f32), handle_len: f32) -> Self {
        let len = (direction.0 * direction.0 + direction.1 * direction.1).sqrt().max(1e-6);
        let (dx, dy) = (direction.0 / len * handle_len, direction.1 / len * handle_len);
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
                let (moved, other) = if moved_in { (self.in_handle, &mut self.out_handle) } else { (self.out_handle, &mut self.in_handle) };
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
    Rect { x: f32, y: f32, w: f32, h: f32, fill: RgbaColor, stroke: RgbaColor, stroke_width: f32 },
    Ellipse { cx: f32, cy: f32, rx: f32, ry: f32, fill: RgbaColor, stroke: RgbaColor, stroke_width: f32 },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, stroke: RgbaColor, stroke_width: f32 },
    /// Свободная линия (инструмент Pencil/Brush) — набор точек, соединённых
    /// отрезками. Не заливается (только обводка) — как и в Animate/Moho,
    /// у произвольной кривой без замыкания заливка не имеет смысла.
    Polyline { points: Vec<(f32, f32)>, stroke: RgbaColor, stroke_width: f32 },
    /// Замкнутый многоугольник (инструмент Pen — по двойному клику рядом с
    /// первой точкой — и PolyStar). В отличие от Polyline — заливается.
    Polygon { points: Vec<(f32, f32)>, fill: RgbaColor, stroke: RgbaColor, stroke_width: f32 },
    /// Полноценный path с узлами Безье (разделы 8-9 ТЗ) — структурированная
    /// геометрия (`Vec<PathNode>`), не просто строка `d`. Сериализуется в
    /// честные SVG path-команды: `M` для первого узла, `C` между двумя
    /// узлами, у КАЖДОГО из которых есть хотя бы одна ручка со стороны
    /// сегмента, иначе `L` (прямая) — путь может свободно смешивать прямые
    /// и кривые участки, как того требует раздел 8. `Z` в конце, если
    /// `closed`.
    Path { nodes: Vec<PathNode>, closed: bool, fill: RgbaColor, stroke: RgbaColor, stroke_width: f32 },
}

impl VectorShape {
    fn to_svg_element(&self) -> String {
        match self {
            VectorShape::Rect { x, y, w, h, fill, stroke, stroke_width } => format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{}" fill-opacity="{:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}"/>"#,
                fill.to_hex(),
                fill.opacity(),
                stroke.to_hex(),
                stroke.opacity()
            ),
            VectorShape::Ellipse { cx, cy, rx, ry, fill, stroke, stroke_width } => format!(
                r#"<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="{}" fill-opacity="{:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}"/>"#,
                fill.to_hex(),
                fill.opacity(),
                stroke.to_hex(),
                stroke.opacity()
            ),
            VectorShape::Line { x1, y1, x2, y2, stroke, stroke_width } => format!(
                r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round"/>"#,
                stroke.to_hex(),
                stroke.opacity()
            ),
            VectorShape::Polyline { points, stroke, stroke_width } => {
                let pts = points.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ");
                format!(
                    r#"<polyline points="{pts}" fill="none" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Polygon { points, fill, stroke, stroke_width } => {
                let pts = points.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ");
                format!(
                    r#"<polygon points="{pts}" fill="{}" fill-opacity="{:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linejoin="round"/>"#,
                    fill.to_hex(),
                    fill.opacity(),
                    stroke.to_hex(),
                    stroke.opacity()
                )
            }
            VectorShape::Path { nodes, closed, fill, stroke, stroke_width } => {
                let d = path_data_string(nodes, *closed);
                let (fill_hex, fill_op) = if *closed { (fill.to_hex(), fill.opacity()) } else { ("none".to_string(), 0.0) };
                format!(
                    r#"<path d="{d}" fill="{fill_hex}" fill-opacity="{fill_op:.3}" stroke="{}" stroke-opacity="{:.3}" stroke-width="{stroke_width}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    stroke.to_hex(),
                    stroke.opacity()
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
            VectorShape::Line { x1, y1, x2, y2, .. } => (x1.min(*x2), y1.min(*y2), x1.max(*x2), y1.max(*y2)),
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
                    for pt in [Some(node.position), node.in_handle, node.out_handle].into_iter().flatten() {
                        min_x = min_x.min(pt.0);
                        min_y = min_y.min(pt.1);
                        max_x = max_x.max(pt.0);
                        max_y = max_y.max(pt.1);
                    }
                }
                (min_x, min_y, max_x, max_y)
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
    pub fn set_fill(&mut self, color: RgbaColor) {
        match self {
            VectorShape::Rect { fill, .. } | VectorShape::Ellipse { fill, .. } | VectorShape::Polygon { fill, .. } => *fill = color,
            VectorShape::Path { fill, .. } => *fill = color, // действует только если closed — см. to_svg_element
            VectorShape::Line { .. } | VectorShape::Polyline { .. } => {}
        }
    }

    /// Задать цвет обводки — есть у всех фигур.
    pub fn set_stroke(&mut self, color: RgbaColor) {
        match self {
            VectorShape::Rect { stroke, .. }
            | VectorShape::Ellipse { stroke, .. }
            | VectorShape::Line { stroke, .. }
            | VectorShape::Polyline { stroke, .. }
            | VectorShape::Polygon { stroke, .. }
            | VectorShape::Path { stroke, .. } => *stroke = color,
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
            VectorShape::Rect { x, y, w, h, .. } => vec![(*x, *y), (x + w, *y), (*x, y + h), (x + w, y + h)],
            VectorShape::Ellipse { cx, cy, rx, ry, .. } => vec![(*cx, *cy), (cx + rx, *cy), (*cx, cy + ry)],
            VectorShape::Line { x1, y1, x2, y2, .. } => vec![(*x1, *y1), (*x2, *y2)],
            VectorShape::Polyline { points, .. } | VectorShape::Polygon { points, .. } => points.clone(),
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
        let VectorShape::Path { nodes, .. } = self else { return None };
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
}

impl VectorDoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, shape: VectorShape) {
        self.shapes.push(shape);
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
        self.shapes.iter().enumerate().rev().find(|(_, s)| s.contains_point(x, y)).map(|(i, _)| i)
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
    /// как viewBox, чтобы обводки на границе фигур не обрезались.
    fn bounds_with_padding(&self, padding: f32) -> (f32, f32, f32, f32) {
        if self.shapes.is_empty() {
            return (0.0, 0.0, 1.0, 1.0);
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
        (min_x - padding, min_y - padding, max_x + padding, max_y + padding)
    }

    /// Сериализовать в настоящий SVG-текст (XML) — не наш внутренний
    /// формат, читается любым SVG-инструментом, включая уже готовый
    /// `pony_render::texture::load_svg` (resvg) в этом же движке.
    pub fn to_svg_string(&self) -> String {
        let (min_x, min_y, max_x, max_y) = self.bounds_with_padding(4.0);
        let width = (max_x - min_x).max(1.0);
        let height = (max_y - min_y).max(1.0);
        let mut body = String::new();
        for shape in &self.shapes {
            body.push_str(&shape.to_svg_element());
            body.push('\n');
        }
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{min_x} {min_y} {width} {height}" width="{width}" height="{height}">
{body}</svg>
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
    /// Честная оговорка: это НЕ парсер произвольного SVG (тот — задача
    /// `resvg`/`usvg`, уже используемого для импорта и рендера). Понимает
    /// ровно тот набор тегов и атрибутов, которые сам же `to_svg_string()`
    /// и пишет — `rect`/`ellipse`/`line`/`polyline`/`polygon` с нашими
    /// атрибутами. SVG из внешнего редактора с более сложной разметкой
    /// (группы, трансформации, пути `<path>`, CSS-классы) этой функцией
    /// не откроется — вернётся `UnsupportedTag`/`BadLine`, а не тихая порча
    /// данных. Сильнее всего это доказывает round-trip тест ниже: любой
    /// документ, собранный `VectorDoc`, после `to_svg_string()` ->
    /// `from_svg_str()` возвращается ровно тем же (с точностью плавающей
    /// точки), не только "не падает".
    pub fn from_svg_str(text: &str) -> Result<Self, VectorParseError> {
        let mut doc = VectorDoc::new();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("<svg") || line.starts_with("</svg") || line.starts_with("<?xml") {
                continue;
            }
            doc.add(parse_shape_line(line)?);
        }
        Ok(doc)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VectorParseError {
    #[error("не удалось разобрать строку SVG: '{0}'")]
    BadLine(String),
    #[error("неизвестный или неподдерживаемый тег SVG: '{0}' (этот парсер понимает только то, что сам же to_svg_string() и пишет)")]
    UnsupportedTag(String),
    #[error("у тега '{tag}' не хватает или некорректен атрибут '{attr}'")]
    BadAttribute { tag: String, attr: String },
}

fn parse_attrs(s: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if key_start == i || i >= bytes.len() || bytes[i] != b'=' {
            break;
        }
        let key = &s[key_start..i];
        i += 1; // '='
        if i >= bytes.len() || bytes[i] != b'"' {
            break;
        }
        i += 1; // открывающая кавычка
        let val_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        map.insert(key.to_string(), s[val_start..i].to_string());
        i += 1; // закрывающая кавычка
    }
    map
}

fn parse_color(hex: &str, opacity: &str) -> Option<RgbaColor> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    let op: f32 = opacity.parse().ok()?;
    Some(RgbaColor::new(r, g, b, (op.clamp(0.0, 1.0) * 255.0).round() as u8))
}

fn parse_points(s: &str) -> Vec<(f32, f32)> {
    s.split_whitespace()
        .filter_map(|pair| {
            let (xs, ys) = pair.split_once(',')?;
            Some((xs.parse().ok()?, ys.parse().ok()?))
        })
        .collect()
}

fn parse_shape_line(line: &str) -> Result<VectorShape, VectorParseError> {
    let trimmed = line.trim_end_matches("/>").trim_end_matches('>').trim();
    let tag_end = trimmed.find(char::is_whitespace).ok_or_else(|| VectorParseError::BadLine(line.to_string()))?;
    if !trimmed.starts_with('<') {
        return Err(VectorParseError::BadLine(line.to_string()));
    }
    let tag = &trimmed[1..tag_end];
    let attrs = parse_attrs(&trimmed[tag_end..]);
    let bad = |attr: &str| VectorParseError::BadAttribute { tag: tag.to_string(), attr: attr.to_string() };
    let get_f32 = |k: &str| -> Result<f32, VectorParseError> { attrs.get(k).ok_or_else(|| bad(k))?.parse().map_err(|_| bad(k)) };
    let get_color = |fill_k: &str, op_k: &str| -> Result<RgbaColor, VectorParseError> {
        let hex = attrs.get(fill_k).ok_or_else(|| bad(fill_k))?;
        let op = attrs.get(op_k).map(String::as_str).unwrap_or("1");
        parse_color(hex, op).ok_or_else(|| bad(fill_k))
    };

    match tag {
        "rect" => Ok(VectorShape::Rect {
            x: get_f32("x")?,
            y: get_f32("y")?,
            w: get_f32("width")?,
            h: get_f32("height")?,
            fill: get_color("fill", "fill-opacity")?,
            stroke: get_color("stroke", "stroke-opacity")?,
            stroke_width: get_f32("stroke-width")?,
        }),
        "ellipse" => Ok(VectorShape::Ellipse {
            cx: get_f32("cx")?,
            cy: get_f32("cy")?,
            rx: get_f32("rx")?,
            ry: get_f32("ry")?,
            fill: get_color("fill", "fill-opacity")?,
            stroke: get_color("stroke", "stroke-opacity")?,
            stroke_width: get_f32("stroke-width")?,
        }),
        "line" => Ok(VectorShape::Line {
            x1: get_f32("x1")?,
            y1: get_f32("y1")?,
            x2: get_f32("x2")?,
            y2: get_f32("y2")?,
            stroke: get_color("stroke", "stroke-opacity")?,
            stroke_width: get_f32("stroke-width")?,
        }),
        "polyline" => Ok(VectorShape::Polyline {
            points: parse_points(attrs.get("points").ok_or_else(|| bad("points"))?),
            stroke: get_color("stroke", "stroke-opacity")?,
            stroke_width: get_f32("stroke-width")?,
        }),
        "polygon" => Ok(VectorShape::Polygon {
            points: parse_points(attrs.get("points").ok_or_else(|| bad("points"))?),
            fill: get_color("fill", "fill-opacity")?,
            stroke: get_color("stroke", "stroke-opacity")?,
            stroke_width: get_f32("stroke-width")?,
        }),
        "path" => {
            let d = attrs.get("d").ok_or_else(|| bad("d"))?;
            let (nodes, closed) = parse_path_d(d).ok_or_else(|| bad("d"))?;
            let fill_attr = attrs.get("fill").map(String::as_str).unwrap_or("none");
            let fill = if fill_attr == "none" {
                RgbaColor::new(0, 0, 0, 0)
            } else {
                get_color("fill", "fill-opacity").unwrap_or(RgbaColor::new(0, 0, 0, 255))
            };
            Ok(VectorShape::Path {
                nodes,
                closed,
                fill,
                stroke: get_color("stroke", "stroke-opacity")?,
                stroke_width: get_f32("stroke-width")?,
            })
        }
        other => Err(VectorParseError::UnsupportedTag(other.to_string())),
    }
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
                d.push_str(&format!(" C {} {} {} {} {} {}", c1.0, c1.1, c2.0, c2.1, cur.position.0, cur.position.1));
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
                d.push_str(&format!(" C {} {} {} {} {} {} Z", c1.0, c1.1, c2.0, c2.1, first.position.0, first.position.1));
            }
        }
    }
    d
}

/// Разобрать `d`-атрибут path (раздел 8 ТЗ) в узлы + флаг замкнутости.
/// Понимает `M/m L/l H/h V/v C/c S/s Q/q T/t Z/z` — то есть и абсолютные,
/// и относительные команды. `A/a` (дуги) НЕ поддерживаются — это
/// осознанная, задокументированная граница (эллиптические дуги требуют
/// отдельной параметризации, не сводящейся к кубической Безье без
/// приближения) — путь с `A` возвращает `None`, вызывающая сторона это
/// превращает в понятную ошибку, не в тихую порчу геометрии.
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
        let resolve = |p: (f32, f32), cursor: (f32, f32), relative: bool| if relative { (cursor.0 + p.0, cursor.1 + p.1) } else { p };

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
                while let (Some(x), Some(y)) = (take_num_peek(&tokens, i), take_num_peek(&tokens, i + 1)) {
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
                let pt = if relative { (cursor.0 + x, cursor.1) } else { (x, cursor.1) };
                cursor = pt;
                nodes.push(PathNode::corner(pt));
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'V' => {
                let y = take_num(&mut i)?;
                let pt = if relative { (cursor.0, cursor.1 + y) } else { (cursor.0, y) };
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
                nodes.push(PathNode { position: end, in_handle: Some(c2), out_handle: None, node_type: NodeType::Corner });
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
                nodes.push(PathNode { position: end, in_handle: Some(c2), out_handle: None, node_type: NodeType::Corner });
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
                let c1 = (cursor.0 + 2.0 / 3.0 * (q.0 - cursor.0), cursor.1 + 2.0 / 3.0 * (q.1 - cursor.1));
                let c2 = (end.0 + 2.0 / 3.0 * (q.0 - end.0), end.1 + 2.0 / 3.0 * (q.1 - end.1));
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode { position: end, in_handle: Some(c2), out_handle: None, node_type: NodeType::Corner });
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
                let c1 = (cursor.0 + 2.0 / 3.0 * (q.0 - cursor.0), cursor.1 + 2.0 / 3.0 * (q.1 - cursor.1));
                let c2 = (end.0 + 2.0 / 3.0 * (q.0 - end.0), end.1 + 2.0 / 3.0 * (q.1 - end.1));
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(c1);
                }
                nodes.push(PathNode { position: end, in_handle: Some(c2), out_handle: None, node_type: NodeType::Corner });
                last_quad_c = Some(q);
                last_cubic_c2 = None;
                cursor = end;
            }
            'Z' => {
                closed = true;
                last_cubic_c2 = None;
                last_quad_c = None;
            }
            'A' => return None, // дуги не поддерживаются — см. пояснение в doc-комментарии функции
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
        doc.add(VectorShape::Rect { x: 10.0, y: 20.0, w: 30.0, h: 40.0, fill: RED, stroke: RED, stroke_width: 2.0 });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r#"<rect x="10" y="20" width="30" height="40""#));
        assert!(svg.contains(r##"fill="#ff0000""##));
    }

    #[test]
    fn alpha_becomes_opacity_not_part_of_hex_color() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Ellipse { cx: 0.0, cy: 0.0, rx: 5.0, ry: 5.0, fill: BLUE_HALF, stroke: BLUE_HALF, stroke_width: 1.0 });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r##"fill="#0000ff""##), "hex should not encode alpha: {svg}");
        assert!(svg.contains("fill-opacity=\"0.502\""), "alpha 128/255 should show up as opacity: {svg}");
    }

    #[test]
    fn bounds_grow_to_fit_all_shapes_with_padding() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0, fill: RED, stroke: RED, stroke_width: 1.0 });
        doc.add(VectorShape::Ellipse { cx: 100.0, cy: 100.0, rx: 20.0, ry: 20.0, fill: RED, stroke: RED, stroke_width: 1.0 });
        let (min_x, min_y, max_x, max_y) = doc.bounds_with_padding(4.0);
        assert!((min_x - (-4.0)).abs() < 1e-4);
        assert!((min_y - (-4.0)).abs() < 1e-4);
        assert!((max_x - 124.0).abs() < 1e-4, "100+20+4=124, got {max_x}");
        assert!((max_y - 124.0).abs() < 1e-4);
    }

    #[test]
    fn clear_removes_all_shapes() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Line { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, stroke: RED, stroke_width: 1.0 });
        assert!(!doc.is_empty());
        doc.clear();
        assert!(doc.is_empty());
    }

    #[test]
    fn polyline_includes_all_points_in_order() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Polyline { points: vec![(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)], stroke: RED, stroke_width: 2.0 });
        let svg = doc.to_svg_string();
        assert!(svg.contains(r#"points="0,0 5,5 10,0""#), "{svg}");
    }

    #[test]
    fn polygon_serializes_with_fill_unlike_polyline() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Polygon { points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)], fill: RED, stroke: RED, stroke_width: 1.0 });
        let svg = doc.to_svg_string();
        assert!(svg.contains("<polygon"));
        assert!(svg.contains(r##"fill="#ff0000""##), "{svg}");
    }

    #[test]
    fn shape_at_finds_the_topmost_shape_under_the_point() {
        let mut doc = VectorDoc::new();
        // Два перекрывающихся прямоугольника — второй нарисован позже,
        // значит рисуется поверх и должен находиться первым.
        doc.add(VectorShape::Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0, fill: RED, stroke: RED, stroke_width: 1.0 });
        doc.add(VectorShape::Rect { x: 5.0, y: 5.0, w: 20.0, h: 20.0, fill: BLUE_HALF, stroke: BLUE_HALF, stroke_width: 1.0 });
        assert_eq!(doc.shape_at(10.0, 10.0), Some(1), "точка внутри обоих — должна найтись верхняя (индекс 1)");
        assert_eq!(doc.shape_at(2.0, 2.0), Some(0), "точка только в первой");
        assert_eq!(doc.shape_at(100.0, 100.0), None, "мимо всех фигур");
    }

    #[test]
    fn set_fill_and_set_stroke_change_the_serialized_colors() {
        let mut shape = VectorShape::Rect { x: 0.0, y: 0.0, w: 5.0, h: 5.0, fill: RED, stroke: RED, stroke_width: 1.0 };
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
        let mut line = VectorShape::Line { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, stroke: RED, stroke_width: 1.0 };
        line.set_fill(BLUE_HALF); // просто не должно паниковать
        let mut doc = VectorDoc::new();
        doc.add(line);
        assert!(!doc.to_svg_string().contains("fill-opacity"), "у Line вообще нет fill-атрибута в SVG");
    }

    #[test]
    fn rect_control_points_are_its_four_corners() {
        let shape = VectorShape::Rect { x: 10.0, y: 20.0, w: 30.0, h: 40.0, fill: RED, stroke: RED, stroke_width: 1.0 };
        let pts = shape.control_points();
        assert_eq!(pts, vec![(10.0, 20.0), (40.0, 20.0), (10.0, 60.0), (40.0, 60.0)]);
    }

    #[test]
    fn dragging_a_rect_corner_keeps_the_opposite_corner_fixed() {
        let mut shape = VectorShape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0, fill: RED, stroke: RED, stroke_width: 1.0 };
        // Индекс 3 — bottom-right (x+w, y+h). Тащим его в (50, 60).
        shape.set_control_point(3, (50.0, 60.0));
        if let VectorShape::Rect { x, y, w, h, .. } = shape {
            assert_eq!((x, y), (0.0, 0.0), "противоположный угол (top-left) должен остаться на месте");
            assert_eq!((w, h), (50.0, 60.0));
        } else {
            panic!("должен остаться Rect");
        }
    }

    #[test]
    fn dragging_a_rect_corner_past_the_opposite_corner_does_not_go_negative() {
        let mut shape = VectorShape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0, fill: RED, stroke: RED, stroke_width: 1.0 };
        // Тащим top-left (индекс 0) ЗА противоположный угол (10,10) — в (30,30).
        shape.set_control_point(0, (30.0, 30.0));
        if let VectorShape::Rect { x, y, w, h, .. } = shape {
            assert!(w >= 1.0 && h >= 1.0, "ширина/высота не должны стать отрицательными: w={w} h={h}");
            assert_eq!((x, y), (10.0, 10.0), "фигура должна вывернуться, а не схлопнуться в мусор");
        } else {
            panic!("должен остаться Rect");
        }
    }

    #[test]
    fn ellipse_edge_handles_control_rx_and_ry_independently() {
        let mut shape = VectorShape::Ellipse { cx: 0.0, cy: 0.0, rx: 5.0, ry: 5.0, fill: RED, stroke: RED, stroke_width: 1.0 };
        shape.set_control_point(1, (20.0, 0.0)); // хендл rx
        if let VectorShape::Ellipse { rx, ry, .. } = &shape {
            assert!((rx - 20.0).abs() < 1e-4);
            assert!((ry - 5.0).abs() < 1e-4, "ry не должен был измениться от хендла rx");
        } else {
            panic!();
        }
    }

    #[test]
    fn ellipse_center_handle_moves_the_whole_shape() {
        let mut shape = VectorShape::Ellipse { cx: 0.0, cy: 0.0, rx: 5.0, ry: 5.0, fill: RED, stroke: RED, stroke_width: 1.0 };
        shape.set_control_point(0, (12.0, -7.0));
        if let VectorShape::Ellipse { cx, cy, rx, ry, .. } = &shape {
            assert_eq!((*cx, *cy), (12.0, -7.0));
            assert!((rx - 5.0).abs() < 1e-4 && (ry - 5.0).abs() < 1e-4, "радиусы не должны меняться от хендла центра");
        } else {
            panic!();
        }
    }

    #[test]
    fn polygon_point_drag_changes_only_the_targeted_point() {
        let mut shape = VectorShape::Polygon { points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)], fill: RED, stroke: RED, stroke_width: 1.0 };
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
        let mut shape = VectorShape::Line { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, stroke: RED, stroke_width: 1.0 };
        shape.set_control_point(99, (5.0, 5.0)); // не должно паниковать
        assert_eq!(shape.control_points(), vec![(0.0, 0.0), (1.0, 1.0)], "ничего не должно было измениться");
    }

    /// Самое сильное доказательство, что парсер — реальный обратный к
    /// сериализатору, а не "вроде работает": собрать документ из ВСЕХ пяти
    /// видов фигур, записать в SVG-текст, разобрать обратно, сравнить с
    /// исходным. Не "не упало" — побитовое совпадение всех полей.
    #[test]
    fn full_round_trip_through_svg_text_preserves_every_shape_exactly() {
        let mut doc = VectorDoc::new();
        doc.add(VectorShape::Rect { x: 1.0, y: 2.0, w: 30.0, h: 40.0, fill: RgbaColor::new(200, 50, 60, 255), stroke: RgbaColor::new(10, 20, 30, 128), stroke_width: 2.5 });
        doc.add(VectorShape::Ellipse { cx: -5.0, cy: 6.0, rx: 12.0, ry: 8.0, fill: RgbaColor::new(0, 255, 0, 255), stroke: RED, stroke_width: 1.0 });
        doc.add(VectorShape::Line { x1: 0.0, y1: 0.0, x2: 50.0, y2: -20.0, stroke: BLUE_HALF, stroke_width: 3.0 });
        doc.add(VectorShape::Polyline { points: vec![(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)], stroke: RED, stroke_width: 1.5 });
        doc.add(VectorShape::Polygon { points: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)], fill: BLUE_HALF, stroke: RED, stroke_width: 1.0 });

        let svg_text = doc.to_svg_string();
        let parsed = VectorDoc::from_svg_str(&svg_text).expect("сгенерированный нами же SVG обязан разбираться без ошибок");

        assert_eq!(parsed.shapes.len(), doc.shapes.len());
        for (original, back) in doc.shapes.iter().zip(parsed.shapes.iter()) {
            assert_eq!(format!("{original:?}"), format!("{back:?}"), "фигура должна вернуться такой же после SVG-текста и обратно");
        }
    }

    #[test]
    fn from_svg_str_rejects_unsupported_tags_instead_of_silently_dropping_them() {
        // <path> теперь ПОДДЕРЖИВАЕТСЯ (см. Path-тесты ниже) — берём
        // тег, который заведомо остаётся неподдержанным (группы,
        // градиенты, фильтры и т.д. — вне области этого редактора).
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10">
<g id="wrapper"><rect x="0" y="0" width="5" height="5" fill="#ff0000" fill-opacity="1.0" stroke="#000000" stroke-opacity="1.0" stroke-width="1"/></g>
</svg>
"##;
        let result = VectorDoc::from_svg_str(svg);
        assert_eq!(result, Err(VectorParseError::UnsupportedTag("g".to_string())));
    }

    #[test]
    fn from_svg_str_reports_which_attribute_is_missing_or_bad() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10">
<rect x="1" y="2" width="not_a_number" height="4" fill="#ff0000" fill-opacity="1.0" stroke="#000000" stroke-opacity="1.0" stroke-width="1"/>
</svg>
"##;
        let result = VectorDoc::from_svg_str(svg);
        assert_eq!(result, Err(VectorParseError::BadAttribute { tag: "rect".to_string(), attr: "width".to_string() }));
    }

    // --- Path (разделы 8-9 ТЗ): узлы Безье, C/L/M-сериализация, парсинг ---

    #[test]
    fn path_data_string_emits_L_for_a_pure_corner_path() {
        let nodes = vec![PathNode::corner((0.0, 0.0)), PathNode::corner((10.0, 0.0)), PathNode::corner((10.0, 10.0))];
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
        let nodes = vec![PathNode::corner((0.0, 0.0)), PathNode::corner((10.0, 0.0)), PathNode::corner((5.0, 10.0))];
        let d = path_data_string(&nodes, true);
        assert!(d.ends_with(" Z"), "{d}");
    }

    #[test]
    fn parse_path_d_reads_move_and_line() {
        let (nodes, closed) = parse_path_d("M 10 10 L 20 10 L 20 20 Z").expect("должен разобраться");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(nodes[1].position, (20.0, 10.0));
        assert_eq!(nodes[2].position, (20.0, 20.0));
        assert!(closed);
        assert!(nodes[0].in_handle.is_none() && nodes[0].out_handle.is_none(), "чистая прямая — без ручек");
    }

    #[test]
    fn parse_path_d_reads_cubic_bezier_from_the_tdd_example() {
        // Ровно пример из раздела 8 присланного ТЗ.
        let (nodes, closed) = parse_path_d("M 10 10 C 50 0 100 0 150 50 C 100 100 50 100 10 10 Z").expect("должен разобраться");
        assert!(closed);
        assert_eq!(nodes.len(), 3, "M даёт первый узел, каждая C добавляет ещё один — итого 1+2=3");
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(nodes[0].out_handle, Some((50.0, 0.0)));
        assert_eq!(nodes[1].position, (150.0, 50.0));
        assert_eq!(nodes[1].in_handle, Some((100.0, 0.0)));
        assert_eq!(nodes[2].position, (10.0, 10.0), "второй C возвращается в исходную точку (10,10)");
    }

    #[test]
    fn parse_path_d_handles_relative_commands() {
        let (nodes, _) = parse_path_d("M 10 10 l 5 0 l 0 5").expect("должен разобраться");
        assert_eq!(nodes[0].position, (10.0, 10.0));
        assert_eq!(nodes[1].position, (15.0, 10.0), "относительный l должен прибавиться к курсору");
        assert_eq!(nodes[2].position, (15.0, 15.0));
    }

    #[test]
    fn parse_path_d_converts_quadratic_to_cubic_exactly() {
        // Q P0=(0,0) Q=(5,10) end=(10,0) -> C1 = 2/3*(5,10) = (3.333,6.667),
        // C2 = end + 2/3*(Q-end) = (10,0)+2/3*(-5,10) = (6.667,6.667).
        let (nodes, _) = parse_path_d("M 0 0 Q 5 10 10 0").expect("должен разобраться");
        assert_eq!(nodes.len(), 2);
        let c1 = nodes[0].out_handle.expect("должна появиться исходящая ручка из квадратичной");
        assert!((c1.0 - 3.333).abs() < 0.01 && (c1.1 - 6.667).abs() < 0.01, "{c1:?}");
        let c2 = nodes[1].in_handle.expect("должна появиться входящая ручка");
        assert!((c2.0 - 6.667).abs() < 0.01 && (c2.1 - 6.667).abs() < 0.01, "{c2:?}");
    }

    #[test]
    fn parse_path_d_rejects_arcs_honestly() {
        // Дуги (A/a) осознанно не поддерживаются — должны дать None, не
        // тихо портить геометрию подстановкой чего попало.
        assert!(parse_path_d("M 0 0 A 5 5 0 0 1 10 10").is_none());
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
        doc.add(VectorShape::Path { nodes: vec![n0, n1, n2], closed: false, fill: RgbaColor::new(0, 0, 0, 0), stroke: RED, stroke_width: 2.0 });

        let svg_text = doc.to_svg_string();
        let parsed = VectorDoc::from_svg_str(&svg_text).expect("наш же вывод должен разбираться");
        assert_eq!(parsed.shapes.len(), 1);
        let VectorShape::Path { nodes, closed, .. } = &parsed.shapes[0] else { panic!("ожидали Path") };
        assert!(!closed);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].position, (0.0, 0.0));
        assert_eq!(nodes[2].position, (50.0, 0.0));
        assert!(nodes[1].in_handle.is_some(), "кривой сегмент должен сохранить ручку");
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
        let doc = VectorDoc::from_svg_str(svg).expect("реальный демо-ассет должен теперь разбираться");
        assert_eq!(doc.shapes.len(), 1);
        let VectorShape::Path { nodes, .. } = &doc.shapes[0] else { panic!("ожидали Path") };
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].position, (5.0, 8.0));
        assert_eq!(nodes[1].position, (55.0, 8.0));
    }

    #[test]
    fn set_control_point_on_path_position_drags_handles_along_with_it() {
        let mut node = PathNode::corner((10.0, 10.0));
        node.in_handle = Some((5.0, 10.0));
        node.out_handle = Some((15.0, 10.0));
        let mut shape = VectorShape::Path { nodes: vec![node], closed: false, fill: RED, stroke: RED, stroke_width: 1.0 };
        // control_points()[0] — позиция узла (единственный узел, значит index 0).
        shape.set_control_point(0, (20.0, 20.0));
        let VectorShape::Path { nodes, .. } = &shape else { panic!() };
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
        let shape = VectorShape::Path { nodes: vec![n0, n1], closed: false, fill: RED, stroke: RED, stroke_width: 1.0 };
        // Порядок обхода: n0.position(0), n0.out_handle(1), n1.position(2), n1.in_handle(3).
        assert_eq!(shape.path_control_point_kind(0), Some((0, PathPointKind::Position)));
        assert_eq!(shape.path_control_point_kind(1), Some((0, PathPointKind::OutHandle)));
        assert_eq!(shape.path_control_point_kind(2), Some((1, PathPointKind::Position)));
        assert_eq!(shape.path_control_point_kind(3), Some((1, PathPointKind::InHandle)));
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
        assert!((len - 10.0).abs() < 0.01, "длина должна остаться той же: {len}");
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
        assert_eq!(node.in_handle, Some((5.0, 10.0)), "corner-узел не должен трогать вторую ручку вообще");
    }
}
