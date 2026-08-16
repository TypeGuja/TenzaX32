//! Boolean-геометрия (раздел 44 ТЗ, меню Modify -> "Combine Paths"):
//! Union/Difference/Intersection/XOR/Divide между двумя произвольными
//! фигурами документа.
//!
//! Реализовано через `i_overlay` — настоящий проверенный сканлайн-движок
//! полигонального оверлея (в духе Vatti/Martinez-Rueda), а не собственная
//! наивная реализация "на коленке": именно так этот класс задач обычно и
//! решается (см. README, "Крупнейшие настоящие пробелы" — там же раньше
//! стояла заметка "обычно решается через полигональную триангуляцию или
//! библиотеку вроде lyon/boolean-ops").
//!
//! Сам boolean-движок работает с прямыми отрезками (полигонами), не с
//! кривыми Безье — поэтому перед любой операцией фигура сначала
//! ФЛЭТТЕНИТСЯ в полигон (`flatten_shape_to_contour`): `Rect`/`Polygon` —
//! точно, `Ellipse` — 32-сегментная аппроксимация (тот же приём, что и
//! в GUI-превью, см. `pony-gui`), закрытый `Path` — кубические сегменты
//! семплируются по параметру `t` (та же формула Безье, что и в
//! `arc_to_cubic_beziers`/рендере, флэттенится с тем же разрешением, что
//! и рисование в GUI). Результат Boolean-операции — тоже полигон
//! (`VectorShape::Polygon`), не кривая: обратное преобразование "полигон
//! -> гладкий Path" не является целью этой задачи (в большинстве
//! реальных векторных редакторов Boolean-результат на кривых тоже даёт
//! полигональную, не идеально гладкую без дополнительного постсглаживания,
//! границу).
//!
//! Честное ограничение: `VectorShape::Polygon` умеет хранить только ОДИН
//! замкнутый контур (нет встроенной поддержки дырок/составных путей с
//! `fill-rule: evenodd`, тот же класс ограничения, что и у `Path` в этом
//! модуле). Если boolean-результат содержит "дырку" (например XOR двух
//! концентрических фигур или Divide, создающее кольцо) — внутренние
//! контуры отбрасываются, остаётся только внешняя граница каждого куска,
//! и это явно репортится вызывающей стороне через `dropped_holes`
//! (честный откат, не тихая потеря геометрии, тот же принцип, что и
//! `unsupported` при парсинге SVG).

use crate::vector::{PathNode, RgbaColor, VectorShape};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Сколько прямых сегментов на один кубический Безье-сегмент пути при
/// флэттенинге в полигон — тот же порядок точности, что и у 32-сегментной
/// аппроксимации эллипса (не научная точность, а "выглядит гладко на
/// типичном масштабе персонажа этого движка").
const BEZIER_FLATTEN_SEGMENTS: usize = 16;
/// Сколько сегментов у полигональной аппроксимации `Ellipse` при
/// флэттенинге — совпадает с константой в `pony-gui` (32-сегментный fan),
/// чтобы boolean-результат визуально не отличался разрешением контура от
/// того, что пользователь видит на превью.
const ELLIPSE_FLATTEN_SEGMENTS: usize = 32;

/// Какую boolean-операцию выполнить между двумя фигурами — `front`
/// считается "верхней" (в Z-порядке холста), `back` — "нижней"; порядок
/// важен для `Difference` (вычитает `back` из `front`) и `Divide` (стиль
/// куска перекрытия берётся от `front`, как в Illustrator/Animate, где
/// верхний объект "красит" зону перекрытия).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    Union,
    Difference,
    Intersection,
    Xor,
    Divide,
}

/// Один результирующий кусок boolean-операции — достаточно данных, чтобы
/// вызывающая сторона (GUI) собрала из него `VectorShape::Polygon`.
#[derive(Debug, Clone, PartialEq)]
pub struct BooleanPiece {
    /// Точки внешнего контура куска, в document-space координатах.
    pub points: Vec<(f32, f32)>,
    pub fill: RgbaColor,
    pub stroke: RgbaColor,
    pub stroke_width: f32,
}

/// Итог boolean-операции — ноль, один или несколько кусков (Divide и XOR
/// естественно производят больше одного куска; Union/Intersection обычно
/// один, но тоже могут дать несколько несвязных кусков, если исходные
/// фигуры не пересекались вообще, а операция всё равно даёт результат
/// — например Union двух непересекающихся фигур).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BooleanResult {
    pub pieces: Vec<BooleanPiece>,
    /// Сколько внутренних контуров (дырок) было отброшено при сведении
    /// результата к одноконтурным `VectorShape::Polygon` — 0, если
    /// результат был полностью без дырок (типичный случай для
    /// непересекающихся дырками фигур). Честно репортится, не молчаливая
    /// потеря геометрии.
    pub dropped_holes: usize,
}

/// Превратить фигуру в замкнутый полигональный контур в document-space
/// координатах — общий первый шаг перед любой boolean-операцией.
/// `None`, если у фигуры нет замкнутой заливаемой области вообще: `Line`/
/// `Polyline` (нет заливки), `Instance` (символ-инстанс — boolean между
/// инстансами не поддержан в этой версии, тот же класс ограничения, что
/// и градиент-override инстансов), незамкнутый `Path` (та же логика, что
/// у `to_svg_element`/`set_fill` — заливка Path действует только если
/// `closed`), вырожденный `Polygon` с менее чем 3 точками.
pub fn flatten_shape_to_contour(shape: &VectorShape) -> Option<Vec<(f32, f32)>> {
    match shape {
        VectorShape::Rect { x, y, w, h, .. } => Some(vec![
            (*x, *y),
            (*x + *w, *y),
            (*x + *w, *y + *h),
            (*x, *y + *h),
        ]),
        VectorShape::Ellipse { cx, cy, rx, ry, .. } => {
            let n = ELLIPSE_FLATTEN_SEGMENTS;
            Some(
                (0..n)
                    .map(|i| {
                        let t = (i as f32 / n as f32) * std::f32::consts::TAU;
                        (*cx + *rx * t.cos(), *cy + *ry * t.sin())
                    })
                    .collect(),
            )
        }
        VectorShape::Polygon { points, .. } => {
            if points.len() < 3 {
                return None;
            }
            Some(points.clone())
        }
        VectorShape::Path { nodes, closed, .. } => {
            if !*closed || nodes.len() < 2 {
                return None;
            }
            Some(flatten_path_nodes(nodes))
        }
        VectorShape::Line { .. } | VectorShape::Polyline { .. } | VectorShape::Instance { .. } => {
            None
        }
    }
}

/// Флэттенинг замкнутого списка узлов пути в полигон — та же логика
/// выбора "прямая линия vs. кубическая кривая" для каждого сегмента, что
/// и `path_data_string` (сериализация в SVG `d=`): сегмент — прямая,
/// только если у ОБОИХ узлов нет ручки со своей стороны; если хотя бы
/// одна ручка есть, недостающая берётся как вырожденная (в самой точке
/// узла), и сегмент семплируется как кубическая кривая. Замыкающий
/// сегмент от последнего узла к первому обрабатывается так же (path
/// здесь всегда закрыт — проверено вызывающей стороной).
fn flatten_path_nodes(nodes: &[PathNode]) -> Vec<(f32, f32)> {
    let n = nodes.len();
    let mut out = Vec::with_capacity(n * BEZIER_FLATTEN_SEGMENTS);
    for i in 0..n {
        let a = &nodes[i];
        let b = &nodes[(i + 1) % n];
        out.push(a.position);
        match (a.out_handle, b.in_handle) {
            (None, None) => {} // прямая — конец сегмента добавится на следующей итерации (или замкнётся сам)
            (out_h, in_h) => {
                let c1 = out_h.unwrap_or(a.position);
                let c2 = in_h.unwrap_or(b.position);
                for s in 1..BEZIER_FLATTEN_SEGMENTS {
                    let t = s as f32 / BEZIER_FLATTEN_SEGMENTS as f32;
                    out.push(cubic_bezier_point(a.position, c1, c2, b.position, t));
                }
            }
        }
    }
    out
}

fn cubic_bezier_point(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

fn to_f64_contour(points: &[(f32, f32)]) -> Vec<[f64; 2]> {
    points.iter().map(|(x, y)| [*x as f64, *y as f64]).collect()
}

/// Прогнать один `i_overlay::OverlayRule` между двумя уже-флэттенутыми
/// контурами — возвращает внешние контуры всех получившихся кусков (в
/// `(f32,f32)`) плюс сколько внутренних контуров (дырок) при этом
/// отброшено. Общий внутренний helper для `boolean_op`, не публичный —
/// снаружи модуля есть смысл только в целой операции (`BooleanOp`), не в
/// сыром `OverlayRule`.
fn run_overlay(
    subj: &[(f32, f32)],
    clip: &[(f32, f32)],
    rule: OverlayRule,
) -> (Vec<Vec<(f32, f32)>>, usize) {
    let subj64 = to_f64_contour(subj);
    let clip64 = to_f64_contour(clip);
    let shapes = subj64.overlay(&clip64, rule, FillRule::NonZero);
    let mut pieces = Vec::new();
    let mut dropped_holes = 0usize;
    for shape in shapes {
        if shape.is_empty() {
            continue;
        }
        dropped_holes += shape.len().saturating_sub(1);
        let outer = &shape[0];
        if outer.len() >= 3 {
            pieces.push(outer.iter().map(|p| (p[0] as f32, p[1] as f32)).collect());
        }
    }
    (pieces, dropped_holes)
}

/// Выполнить boolean-операцию `op` между `front` (верхняя фигура) и
/// `back` (нижняя фигура) — `None`, если хотя бы одна из фигур не
/// флэттенится в полигон (см. `flatten_shape_to_contour`), иначе
/// `Some(BooleanResult)` — результат может быть и пустым (`pieces` — не
/// паника, честный "операция ничего не дала", например `Intersection`
/// двух непересекающихся фигур).
pub fn boolean_op(front: &VectorShape, back: &VectorShape, op: BooleanOp) -> Option<BooleanResult> {
    let front_points = flatten_shape_to_contour(front)?;
    let back_points = flatten_shape_to_contour(back)?;
    let (front_fill, front_stroke, front_stroke_width) = shape_style(front)?;
    let (back_fill, back_stroke, back_stroke_width) = shape_style(back)?;

    let mut result = BooleanResult::default();
    let push_pieces = |result: &mut BooleanResult,
                        pieces: Vec<Vec<(f32, f32)>>,
                        dropped: usize,
                        fill: RgbaColor,
                        stroke: RgbaColor,
                        stroke_width: f32| {
        result.dropped_holes += dropped;
        for points in pieces {
            result.pieces.push(BooleanPiece {
                points,
                fill,
                stroke,
                stroke_width,
            });
        }
    };

    match op {
        BooleanOp::Union => {
            let (pieces, dropped) = run_overlay(&front_points, &back_points, OverlayRule::Union);
            push_pieces(
                &mut result,
                pieces,
                dropped,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
        }
        BooleanOp::Intersection => {
            let (pieces, dropped) =
                run_overlay(&front_points, &back_points, OverlayRule::Intersect);
            push_pieces(
                &mut result,
                pieces,
                dropped,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
        }
        BooleanOp::Difference => {
            let (pieces, dropped) =
                run_overlay(&front_points, &back_points, OverlayRule::Difference);
            push_pieces(
                &mut result,
                pieces,
                dropped,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
        }
        BooleanOp::Xor => {
            // XOR = (front без back) объединено с (back без front) — каждая
            // половина сохраняет стиль своей исходной фигуры, честнее, чем
            // красить весь результат одним плоским цветом.
            let (front_only, d1) =
                run_overlay(&front_points, &back_points, OverlayRule::Difference);
            let (back_only, d2) =
                run_overlay(&back_points, &front_points, OverlayRule::Difference);
            push_pieces(
                &mut result,
                front_only,
                d1,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
            push_pieces(
                &mut result,
                back_only,
                d2,
                back_fill,
                back_stroke,
                back_stroke_width,
            );
        }
        BooleanOp::Divide => {
            // Divide (Illustrator/Animate): режет обе фигуры на все
            // непересекающиеся куски — "только front", "только back" и
            // "перекрытие" (которое красится стилем ВЕРХНЕЙ фигуры — она
            // визуально перекрывает нижнюю в этой зоне на исходном холсте).
            let (front_only, d1) =
                run_overlay(&front_points, &back_points, OverlayRule::Difference);
            let (back_only, d2) =
                run_overlay(&back_points, &front_points, OverlayRule::Difference);
            let (overlap, d3) =
                run_overlay(&front_points, &back_points, OverlayRule::Intersect);
            push_pieces(
                &mut result,
                front_only,
                d1,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
            push_pieces(
                &mut result,
                back_only,
                d2,
                back_fill,
                back_stroke,
                back_stroke_width,
            );
            push_pieces(
                &mut result,
                overlap,
                d3,
                front_fill,
                front_stroke,
                front_stroke_width,
            );
        }
    }
    Some(result)
}

/// Достать (fill, stroke, stroke_width) фигуры для стилизации boolean-
/// результата — `None` для фигур без этих полей (`Line`/`Instance`, хотя
/// на практике `flatten_shape_to_contour` их уже отсеял раньше по вызову
/// `boolean_op`; эта функция отдельно проверяет то же самое на случай,
/// если её вызовут напрямую).
fn shape_style(shape: &VectorShape) -> Option<(RgbaColor, RgbaColor, f32)> {
    match shape {
        VectorShape::Rect {
            fill,
            stroke,
            stroke_width,
            ..
        }
        | VectorShape::Ellipse {
            fill,
            stroke,
            stroke_width,
            ..
        }
        | VectorShape::Polygon {
            fill,
            stroke,
            stroke_width,
            ..
        }
        | VectorShape::Path {
            fill,
            stroke,
            stroke_width,
            ..
        } => Some((*fill, *stroke, *stroke_width)),
        VectorShape::Line { .. } | VectorShape::Polyline { .. } | VectorShape::Instance { .. } => {
            None
        }
    }
}

/// Собрать `BooleanPiece` обратно в полноценный `VectorShape::Polygon` —
/// удобный конструктор для вызывающей стороны (GUI), чтобы не дублировать
/// список полей на каждом месте использования.
pub fn piece_to_shape(piece: &BooleanPiece) -> VectorShape {
    VectorShape::Polygon {
        points: piece.points.clone(),
        fill: piece.fill,
        fill_gradient: None,
        stroke: piece.stroke,
        stroke_width: piece.stroke_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::PathNode;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> VectorShape {
        VectorShape::Rect {
            x,
            y,
            w,
            h,
            fill: RgbaColor::new(200, 50, 50, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        }
    }

    fn rect_with_fill(x: f32, y: f32, w: f32, h: f32, fill: RgbaColor) -> VectorShape {
        VectorShape::Rect {
            x,
            y,
            w,
            h,
            fill,
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        }
    }

    #[test]
    fn flatten_rect_gives_four_corners_in_order() {
        let r = rect(0.0, 0.0, 10.0, 5.0);
        let pts = flatten_shape_to_contour(&r).expect("rect flattens");
        assert_eq!(pts, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)]);
    }

    #[test]
    fn flatten_ellipse_produces_32_points_on_the_ellipse() {
        let e = VectorShape::Ellipse {
            cx: 10.0,
            cy: 10.0,
            rx: 5.0,
            ry: 3.0,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        let pts = flatten_shape_to_contour(&e).expect("ellipse flattens");
        assert_eq!(pts.len(), 32);
        for (x, y) in &pts {
            let nx = (x - 10.0) / 5.0;
            let ny = (y - 10.0) / 3.0;
            assert!((nx * nx + ny * ny - 1.0).abs() < 1e-4, "точка должна лежать на эллипсе");
        }
    }

    #[test]
    fn flatten_polygon_with_fewer_than_3_points_returns_none() {
        let p = VectorShape::Polygon {
            points: vec![(0.0, 0.0), (1.0, 1.0)],
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        assert!(flatten_shape_to_contour(&p).is_none());
    }

    #[test]
    fn flatten_open_path_returns_none_closed_path_returns_points() {
        let nodes = vec![
            PathNode::corner((0.0, 0.0)),
            PathNode::corner((10.0, 0.0)),
            PathNode::corner((10.0, 10.0)),
        ];
        let open = VectorShape::Path {
            nodes: nodes.clone(),
            closed: false,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        assert!(flatten_shape_to_contour(&open).is_none());

        let closed = VectorShape::Path {
            nodes,
            closed: true,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        let pts = flatten_shape_to_contour(&closed).expect("closed path flattens");
        // Только corner-узлы (без ручек) — сегменты прямые, по одной точке
        // на узел, без промежуточных Безье-сэмплов.
        assert_eq!(pts, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    }

    #[test]
    fn flatten_path_with_handles_samples_a_curve_between_nodes() {
        let nodes = vec![
            PathNode::symmetric((0.0, 0.0), (1.0, 0.0), 3.0),
            PathNode::symmetric((10.0, 0.0), (1.0, 0.0), 3.0),
        ];
        let closed = VectorShape::Path {
            nodes,
            closed: true,
            fill: RgbaColor::new(0, 0, 0, 255),
            fill_gradient: None,
            stroke: RgbaColor::new(0, 0, 0, 0),
            stroke_width: 0.0,
        };
        let pts = flatten_shape_to_contour(&closed).expect("flattens");
        // 2 узла, у обоих сегментов (туда и обратно) есть ручки — каждый
        // сегмент даёт узел + (BEZIER_FLATTEN_SEGMENTS - 1) промежуточных
        // точек кривой.
        assert_eq!(pts.len(), 2 * BEZIER_FLATTEN_SEGMENTS);
    }

    #[test]
    fn line_polyline_and_instance_do_not_flatten() {
        let line = VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RgbaColor::new(0, 0, 0, 255),
            stroke_width: 1.0,
        };
        assert!(flatten_shape_to_contour(&line).is_none());

        let polyline = VectorShape::Polyline {
            points: vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)],
            stroke: RgbaColor::new(0, 0, 0, 255),
            stroke_width: 1.0,
        };
        assert!(flatten_shape_to_contour(&polyline).is_none());

        let instance = VectorShape::Instance {
            symbol: "eye".to_string(),
            transform: (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            fill_override: None,
        };
        assert!(flatten_shape_to_contour(&instance).is_none());
    }

    #[test]
    fn union_of_two_overlapping_rects_gives_one_bigger_piece() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_op(&a, &b, BooleanOp::Union).expect("both flatten");
        assert_eq!(result.pieces.len(), 1);
        assert_eq!(result.dropped_holes, 0);
        // Площадь объединения двух пересекающихся 10x10 квадратов со
        // сдвигом (5,5) — 100 + 100 - 25 (площадь пересечения) = 175.
        let area = polygon_area(&result.pieces[0].points);
        assert!((area - 175.0).abs() < 1.0, "площадь объединения ≈175, получили {area}");
    }

    #[test]
    fn intersection_of_two_overlapping_rects_gives_the_overlap_square() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = boolean_op(&a, &b, BooleanOp::Intersection).expect("both flatten");
        assert_eq!(result.pieces.len(), 1);
        let area = polygon_area(&result.pieces[0].points);
        assert!((area - 25.0).abs() < 0.5, "площадь пересечения = 25 (5x5), получили {area}");
    }

    #[test]
    fn intersection_of_non_overlapping_rects_is_empty_not_panic() {
        let a = rect(0.0, 0.0, 5.0, 5.0);
        let b = rect(100.0, 100.0, 5.0, 5.0);
        let result = boolean_op(&a, &b, BooleanOp::Intersection).expect("both flatten");
        assert!(result.pieces.is_empty());
        assert_eq!(result.dropped_holes, 0);
    }

    #[test]
    fn difference_removes_the_overlapping_area_from_front() {
        let front = rect(0.0, 0.0, 10.0, 10.0);
        let back = rect(5.0, 0.0, 10.0, 10.0);
        let result = boolean_op(&front, &back, BooleanOp::Difference).expect("both flatten");
        assert_eq!(result.pieces.len(), 1);
        let area = polygon_area(&result.pieces[0].points);
        assert!((area - 50.0).abs() < 0.5, "front минус перекрытие = 5x10 = 50, получили {area}");
    }

    #[test]
    fn difference_is_not_symmetric_front_minus_back_differs_from_back_minus_front() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 0.0, 20.0, 10.0);
        let a_minus_b = boolean_op(&a, &b, BooleanOp::Difference).unwrap();
        let b_minus_a = boolean_op(&b, &a, BooleanOp::Difference).unwrap();
        let area_a_minus_b: f32 = a_minus_b.pieces.iter().map(|p| polygon_area(&p.points)).sum();
        let area_b_minus_a: f32 = b_minus_a.pieces.iter().map(|p| polygon_area(&p.points)).sum();
        assert!((area_a_minus_b - 50.0).abs() < 0.5); // (0..5) x (0..10)
        assert!((area_b_minus_a - 150.0).abs() < 0.5); // (10..25) x (0..10)
    }

    #[test]
    fn xor_produces_two_pieces_styled_by_their_own_source_shape() {
        let front = rect_with_fill(0.0, 0.0, 10.0, 10.0, RgbaColor::new(255, 0, 0, 255));
        let back = rect_with_fill(5.0, 5.0, 10.0, 10.0, RgbaColor::new(0, 255, 0, 255));
        let result = boolean_op(&front, &back, BooleanOp::Xor).expect("both flatten");
        assert_eq!(result.pieces.len(), 2);
        let red_piece = result
            .pieces
            .iter()
            .find(|p| p.fill == RgbaColor::new(255, 0, 0, 255))
            .expect("front-only piece keeps front's color");
        let green_piece = result
            .pieces
            .iter()
            .find(|p| p.fill == RgbaColor::new(0, 255, 0, 255))
            .expect("back-only piece keeps back's color");
        assert!((polygon_area(&red_piece.points) - 75.0).abs() < 1.0);
        assert!((polygon_area(&green_piece.points) - 75.0).abs() < 1.0);
    }

    #[test]
    fn divide_produces_three_pieces_and_overlap_takes_front_style() {
        let front = rect_with_fill(0.0, 0.0, 10.0, 10.0, RgbaColor::new(255, 0, 0, 255));
        let back = rect_with_fill(5.0, 5.0, 10.0, 10.0, RgbaColor::new(0, 255, 0, 255));
        let result = boolean_op(&front, &back, BooleanOp::Divide).expect("both flatten");
        assert_eq!(result.pieces.len(), 3);
        let total_area: f32 = result.pieces.iter().map(|p| polygon_area(&p.points)).sum();
        // 75 (front-only) + 75 (back-only) + 25 (overlap) = 175, та же
        // суммарная площадь, что и Union — Divide просто режет ту же
        // область на непересекающиеся куски, не теряя и не добавляя её.
        assert!((total_area - 175.0).abs() < 1.0);
        let overlap_pieces: Vec<_> = result
            .pieces
            .iter()
            .filter(|p| (polygon_area(&p.points) - 25.0).abs() < 1.0)
            .collect();
        assert_eq!(overlap_pieces.len(), 1);
        assert_eq!(overlap_pieces[0].fill, RgbaColor::new(255, 0, 0, 255));
    }

    #[test]
    fn boolean_op_returns_none_when_either_shape_does_not_flatten() {
        let rect_shape = rect(0.0, 0.0, 10.0, 10.0);
        let line = VectorShape::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            stroke: RgbaColor::new(0, 0, 0, 255),
            stroke_width: 1.0,
        };
        assert!(boolean_op(&rect_shape, &line, BooleanOp::Union).is_none());
        assert!(boolean_op(&line, &rect_shape, BooleanOp::Union).is_none());
    }

    #[test]
    fn piece_to_shape_builds_a_polygon_with_no_gradient() {
        let piece = BooleanPiece {
            points: vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
            fill: RgbaColor::new(9, 9, 9, 255),
            stroke: RgbaColor::new(1, 1, 1, 255),
            stroke_width: 2.0,
        };
        let shape = piece_to_shape(&piece);
        let VectorShape::Polygon {
            points,
            fill,
            fill_gradient,
            stroke,
            stroke_width,
        } = shape
        else {
            panic!("expected Polygon")
        };
        assert_eq!(points, piece.points);
        assert_eq!(fill, piece.fill);
        assert_eq!(fill_gradient, None);
        assert_eq!(stroke, piece.stroke);
        assert_eq!(stroke_width, piece.stroke_width);
    }

    /// Площадь простого (не самопересекающегося) полигона по формуле
    /// шнурков (shoelace) — независимый способ сверить boolean-результат
    /// с ожидаемой геометрией, не завязанный на конкретный порядок точек,
    /// который отдаёт `i_overlay`.
    fn polygon_area(points: &[(f32, f32)]) -> f32 {
        if points.len() < 3 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..points.len() {
            let (x0, y0) = points[i];
            let (x1, y1) = points[(i + 1) % points.len()];
            sum += x0 * y1 - x1 * y0;
        }
        (sum / 2.0).abs()
    }
}
