//! Редактируемый векторный документ (раздел 16 ТЗ — SVG теперь не только
//! импортируется, но и рисуется/редактируется/сохраняется прямо в движке).
//! Чистая модель без GPU — сериализуется в настоящий SVG-текст, который
//! потом читает уже готовый импортёр (`pony_render::texture::load_svg`,
//! `resvg`) — то есть нарисованное здесь тут же можно использовать как
//! часть персонажа, замкнутый цикл "рисуем -> сохраняем -> используем".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            | VectorShape::Polygon { stroke, .. } => *stroke = color,
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
    /// Polyline/Polygon — каждая точка пути напрямую.
    pub fn control_points(&self) -> Vec<(f32, f32)> {
        match self {
            VectorShape::Rect { x, y, w, h, .. } => vec![(*x, *y), (x + w, *y), (*x, y + h), (x + w, y + h)],
            VectorShape::Ellipse { cx, cy, rx, ry, .. } => vec![(*cx, *cy), (cx + rx, *cy), (*cx, cy + ry)],
            VectorShape::Line { x1, y1, x2, y2, .. } => vec![(*x1, *y1), (*x2, *y2)],
            VectorShape::Polyline { points, .. } | VectorShape::Polygon { points, .. } => points.clone(),
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
        }
    }
}

/// Документ — упорядоченный список фигур (порядок = порядок отрисовки,
/// как слои: позже добавленная фигура рисуется поверх).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}
