//! Inverse Kinematics (раздел 41 ТЗ): Two Bone IK.
//!
//! Крупнейший из отмеченных в README пробелов ("риггинг без инверсной
//! кинематики, только прямая") — раздел 41 ТЗ требует именно классический
//! "Two Bone IK" (Hip -> Knee -> Hoof, копыто как target) плюс pole target
//! для управления направлением сгиба колена. `Chain IK` (произвольная длина
//! цепочки, раздел 41) сознательно НЕ реализован в этом проходе — two-bone
//! покрывает главный практический случай (нога/рука пони) и его one-shot
//! аналитическое решение не требует итеративного солвера (FABRIK/CCD),
//! который был бы нужен для цепочки произвольной длины.
//!
//! Чистая математика, без GPU и без побочных эффектов — принимает мировые
//! позиции и возвращает углы, которые вызывающая сторона (`Skeleton::
//! world_transform_with_ik`) применяет поверх обычной FK-иерархии. Ничего
//! не мутирует напрямую — тот же принцип, что уже используется в
//! `orientation::apply_yaw_2_5d` и `Camera::shake_offset` (чистая функция
//! от входа, кто вызывает — тот и решает, что делать с результатом).

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::skeleton::BoneId;

/// IK-констрейнт на цепочку из двух костей (раздел 41 ТЗ: "Two Bone IK").
///
/// Пример из ТЗ:
/// ```text
/// Hip
///  │
/// Knee
///  │
/// Hoof ← Target
/// ```
/// `root` = Hip, `mid` = Knee, `tip` = Hoof. `target` — мировая точка, куда
/// должен дотянуться `tip`. `pole_target` (опционально) — раздел 41:
/// "Pole Target" — управляет тем, В КАКУЮ сторону сгибается `mid`, когда
/// решений для угла колена математически два (стандартная неоднозначность
/// two-bone IK: колено может согнуться вперёд или назад и всё равно
/// дотянуться до цели).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IkConstraint {
    pub id: String,
    pub root: BoneId,
    pub mid: BoneId,
    pub tip: BoneId,
    pub target: Vec2,
    pub pole_target: Option<Vec2>,
    /// Раздел 94 (constraints вообще): смешивание с базовой FK-анимацией.
    /// 1.0 — целиком IK, 0.0 — целиком обычная FK-поза (константа игнорируется).
    pub weight: f32,
    pub enabled: bool,
}

impl IkConstraint {
    pub fn new(id: impl Into<String>, root: impl Into<String>, mid: impl Into<String>, tip: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            root: root.into(),
            mid: mid.into(),
            tip: tip.into(),
            target: Vec2::ZERO,
            pole_target: None,
            weight: 1.0,
            enabled: true,
        }
    }
}

/// Результат решения two-bone IK — мировые углы поворота `root` и `mid`,
/// которые нужно применить, чтобы `tip` (на расстоянии `len_root + len_mid`
/// по прямой цепочке) дотянулся до `target` максимально близко.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwoBoneIkResult {
    pub root_world_angle: f32,
    pub mid_local_angle: f32,
}

/// Аналитическое решение Two Bone IK законом косинусов — классический
/// подход (тот же, что в Unity Animation Rigging / Spine / DragonBones),
/// без итераций, точное решение за одну оценку.
///
/// `root_pos` — мировая позиция корневой кости (Hip), `len_root` — длина
/// кости root->mid (Hip->Knee), `len_mid` — длина кости mid->tip
/// (Knee->Hoof), `target` — мировая точка, куда тянется `tip`.
/// `pole` — опциональная точка, определяющая сторону сгиба; без неё
/// сгиб направлен в сторону текущего положения (эвристика — "не дальше
/// той стороны, где цель").
///
/// Если `target` дальше, чем `len_root + len_mid` (цель физически
/// недостижима), нога честно вытягивается в прямую линию к цели, а не
/// схлопывается или улетает в NaN.
pub fn solve_two_bone_ik(root_pos: Vec2, len_root: f32, len_mid: f32, target: Vec2, pole: Option<Vec2>) -> TwoBoneIkResult {
    let to_target = target - root_pos;
    let dist = to_target.length().max(1e-5);

    // Недостижимая цель — вытягиваем цепочку в прямую линию к ней.
    let max_reach = len_root + len_mid;
    let clamped_dist = dist.min(max_reach - 1e-4).max((len_root - len_mid).abs() + 1e-4);

    // Закон косинусов: угол в root между направлением на target и направлением на mid.
    let cos_root_angle = ((len_root * len_root + clamped_dist * clamped_dist - len_mid * len_mid)
        / (2.0 * len_root * clamped_dist))
        .clamp(-1.0, 1.0);
    let root_angle_offset = cos_root_angle.acos();

    // Угол в mid (внутренний угол треугольника Hip-Knee-Hoof).
    let cos_mid_angle = ((len_root * len_root + len_mid * len_mid - clamped_dist * clamped_dist)
        / (2.0 * len_root * len_mid))
        .clamp(-1.0, 1.0);
    let mid_interior_angle = cos_mid_angle.acos();

    let to_target_angle = to_target.y.atan2(to_target.x);

    // Сторона сгиба: по умолчанию колено гнётся "вверх" (против часовой),
    // pole_target даёт точный контроль (раздел 41: "Pole Target").
    let bend_sign = match pole {
        Some(pole_pos) => {
            let to_pole = pole_pos - root_pos;
            // Знак псевдоскалярного произведения — по какую сторону от
            // линии root->target лежит pole.
            let cross = to_target.x * to_pole.y - to_target.y * to_pole.x;
            if cross >= 0.0 { 1.0 } else { -1.0 }
        }
        None => 1.0,
    };

    let root_world_angle = to_target_angle + bend_sign * root_angle_offset;
    // mid_local_angle — поворот КОСТИ mid относительно root (локальный, не
    // мировой): PI - внутренний угол, т.к. кость идёт "назад" от направления
    // на target при полностью выпрямленной ноге (internal angle = PI).
    let mid_local_angle = bend_sign * (std::f32::consts::PI - mid_interior_angle) * -1.0;

    TwoBoneIkResult { root_world_angle, mid_local_angle }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Полностью выпрямленная нога: target ровно на расстоянии len_root+len_mid.
    /// mid_local_angle должен быть ~0 (кость mid продолжает root по прямой).
    #[test]
    fn fully_extended_leg_has_near_zero_mid_angle() {
        let root_pos = Vec2::new(0.0, 0.0);
        let target = Vec2::new(20.0, 0.0); // ровно len_root + len_mid по прямой вправо
        let result = solve_two_bone_ik(root_pos, 12.0, 8.0, target, None);
        assert!(result.mid_local_angle.abs() < 0.01, "нога должна быть почти прямой, получили {}", result.mid_local_angle);
        assert!((result.root_world_angle - 0.0).abs() < 0.01, "root должен смотреть точно на target: {}", result.root_world_angle);
    }

    /// Цель ближе максимальной длины — колено должно согнуться (не ноль).
    #[test]
    fn bent_leg_has_nonzero_mid_angle_when_target_is_closer_than_full_reach() {
        let root_pos = Vec2::new(0.0, 0.0);
        let target = Vec2::new(10.0, 0.0); // короче 12+8=20
        let result = solve_two_bone_ik(root_pos, 12.0, 8.0, target, None);
        assert!(result.mid_local_angle.abs() > 0.1, "колено должно заметно согнуться: {}", result.mid_local_angle);
    }

    /// Недостижимая цель (дальше len_root+len_mid) не должна давать NaN —
    /// нога просто вытягивается максимально прямо в сторону цели.
    #[test]
    fn unreachable_target_does_not_produce_nan() {
        let root_pos = Vec2::new(0.0, 0.0);
        let target = Vec2::new(1000.0, 0.0); // недостижимо для длины 12+8=20
        let result = solve_two_bone_ik(root_pos, 12.0, 8.0, target, None);
        assert!(result.root_world_angle.is_finite(), "root_world_angle не должен быть NaN/inf");
        assert!(result.mid_local_angle.is_finite(), "mid_local_angle не должен быть NaN/inf");
        // Вытянута практически прямо (недостижимая цель = максимальное вытягивание).
        assert!(result.mid_local_angle.abs() < 0.05, "недостижимая цель должна вытягивать ногу прямо: {}", result.mid_local_angle);
    }

    /// Pole target на разных сторонах линии root->target должен давать
    /// противоположный знак сгиба (нога гнётся вперёд ИЛИ назад, не всегда
    /// в одну сторону) — иначе Pole Target был бы декорацией, не реальным
    /// управлением, как того требует раздел 41.
    #[test]
    fn pole_target_controls_bend_direction() {
        let root_pos = Vec2::new(0.0, 0.0);
        let target = Vec2::new(10.0, 0.0);
        let pole_above = Vec2::new(5.0, 100.0);
        let pole_below = Vec2::new(5.0, -100.0);

        let result_above = solve_two_bone_ik(root_pos, 12.0, 8.0, target, Some(pole_above));
        let result_below = solve_two_bone_ik(root_pos, 12.0, 8.0, target, Some(pole_below));

        assert!(
            result_above.mid_local_angle.signum() != result_below.mid_local_angle.signum(),
            "pole по разные стороны должен давать противоположный сгиб: above={}, below={}",
            result_above.mid_local_angle,
            result_below.mid_local_angle
        );
    }

    /// Цель точно в позиции root (вырожденный случай, dist -> 0) не должна
    /// паниковать или давать NaN — защита через `.max(1e-5)`.
    #[test]
    fn target_at_root_position_does_not_panic_or_nan() {
        let root_pos = Vec2::new(3.0, 4.0);
        let target = root_pos; // совпадает с root
        let result = solve_two_bone_ik(root_pos, 12.0, 8.0, target, None);
        assert!(result.root_world_angle.is_finite());
        assert!(result.mid_local_angle.is_finite());
    }

    /// Симметрия: цель зеркально по X должна давать зеркальный root_world_angle
    /// (косвенно проверяет, что atan2 используется правильно, не перепутаны оси).
    #[test]
    fn mirrored_target_gives_mirrored_root_angle() {
        let root_pos = Vec2::ZERO;
        let target_right = Vec2::new(10.0, 5.0);
        let target_left = Vec2::new(-10.0, 5.0);
        let r_right = solve_two_bone_ik(root_pos, 12.0, 8.0, target_right, None);
        let r_left = solve_two_bone_ik(root_pos, 12.0, 8.0, target_left, None);
        // Не точное зеркало из-за фиксированного bend_sign=1, но направление
        // "смотрит примерно туда, куда цель" должно сохраняться по обе стороны.
        assert!(r_right.root_world_angle.cos() > 0.0, "цель справа -> root смотрит вправо");
        assert!(r_left.root_world_angle.cos() < 0.0, "цель слева -> root смотрит влево");
    }
}
