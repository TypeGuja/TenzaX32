//! Скелет персонажа: иерархия костей.
//! Root -> Body -> Neck -> Head -> Horn / Ears
//! Body -> Shoulder -> UpperLeg -> LowerLeg -> Hoof (x4)

use glam::{Vec2};
use serde::{Deserialize, Serialize};

use crate::ik::{solve_two_bone_ik, IkConstraint};

pub type BoneId = String;

/// Локальная трансформация кости относительно родителя.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Transform2D {
    pub position: Vec2,
    /// В радианах.
    pub rotation: f32,
    pub scale: Vec2,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            rotation: 0.0,
            scale: Vec2::ONE,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bone {
    pub id: BoneId,
    pub parent: Option<BoneId>,
    pub local_transform: Transform2D,
    /// Длина кости, используется для IK и для пересчёта поворота (2.5D).
    pub length: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Раздел 41 ТЗ: Two Bone IK-констрейнты (Hip->Knee->Hoof + target).
    /// `#[serde(default)]` — старые `.asset`-файлы без IK продолжают
    /// загружаться, просто без констрейнтов (тот же принцип, что уже
    /// применён к `Character::facing_yaw`).
    #[serde(default)]
    pub ik_constraints: Vec<IkConstraint>,
}

impl Skeleton {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_bone(&mut self, bone: Bone) -> &mut Self {
        self.bones.push(bone);
        self
    }

    pub fn find(&self, id: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    /// Мировая трансформация кости — обход вверх по цепочке родителей.
    /// Пока наивная реализация (без кэша), для реального рендера
    /// потребуется топологическая сортировка и кэш world-transform.
    ///
    /// Это ЧИСТЫЙ FK (forward kinematics) — не учитывает IK-констрейнты
    /// (раздел 41 ТЗ). Для персонажей с IK (например, нога с целью-копытом)
    /// используйте `world_transform_with_ik`, которая сначала прогоняет IK
    /// и только потом считает world_transform по уже скорректированным
    /// локальным углам. Оставлен отдельным методом, а не заменён — большая
    /// часть кода (рендер частей, hit-тесты, редактор скелета) не должна
    /// каждый раз платить за прогон IK-солвера, когда констрейнтов нет.
    pub fn world_transform(&self, id: &str) -> Option<Transform2D> {
        let bone = self.find(id)?;
        match &bone.parent {
            None => Some(bone.local_transform),
            Some(parent_id) => {
                let parent_world = self.world_transform(parent_id)?;
                Some(compose(parent_world, bone.local_transform))
            }
        }
    }

    /// То же самое, что `world_transform`, но сначала применяет все
    /// включённые IK-констрейнты (раздел 41 ТЗ) поверх обычной FK-позы.
    ///
    /// Для каждого `IkConstraint` с `enabled == true`: решает two-bone IK
    /// аналитически (`solve_two_bone_ik`) по МИРОВОЙ позиции `root` (уже
    /// посчитанной обычным FK — так IK-цепочки могут висеть на анимированном
    /// родителе, например нога, прикреплённая к качающемуся телу) и
    /// подставляет результат как ЛОКАЛЬНЫЕ углы `root`/`mid` в копию
    /// скелета, по которой уже считается world_transform для всех костей.
    /// `weight < 1.0` — линейная интерполяция между исходным (FK) и
    /// IK-углом, не жёсткое переключение (раздел 94: "constraints... weight
    /// (0..1, для смешивания с базовой анимацией)").
    ///
    /// Наивная реализация: каждый вызов заново решает все констрейнты и
    /// строит временную копию `Skeleton` — годится для интерактивного
    /// редактора (десятки костей, не тысячи), но не для сцены с сотнями
    /// одновременно анимируемых IK-цепочек без кэширования.
    pub fn world_transform_with_ik(&self, id: &str) -> Option<Transform2D> {
        if self.ik_constraints.iter().all(|c| !c.enabled) {
            // Быстрый путь: нет активных констрейнтов — не платим за копию.
            return self.world_transform(id);
        }

        let mut resolved = self.clone();
        for constraint in &self.ik_constraints {
            if !constraint.enabled {
                continue;
            }
            // Констрейнт ссылается на несуществующие кости — пропускаем, не паникуем
            // (тот же принцип, что и у "мёртвых" ссылок на удалённые кости в анимациях).
            if self.find(&constraint.root).is_none() || self.find(&constraint.mid).is_none() || self.find(&constraint.tip).is_none() {
                continue;
            }
            let Some(root_world) = self.world_transform(&constraint.root) else { continue };
            let Some(mid_world) = self.world_transform(&constraint.mid) else { continue };
            let Some(tip_world) = self.world_transform(&constraint.tip) else { continue };

            // Длина звена берётся из РЕАЛЬНОГО геометрического расстояния
            // между мировыми позициями костей, а не из декларативного поля
            // `Bone::length` — в этой кодовой базе `length` не гарантированно
            // совпадает с фактическим расстоянием `position` дочерней кости
            // от родителя (в дефолтном скелете пони, например, `UpperLegFL.
            // length == 12.0`, но реальное расстояние Shoulder->UpperLeg —
            // 7.0: `length`, судя по всему, используется только 2.5D-поворотом
            // и пока не синхронизировано с `local_transform.position`).
            // IK обязан тянуться на РЕАЛЬНУЮ длину кости, иначе tip не
            // достигает target даже при достижимой цели (подтверждено
            // регрессионным тестом на настоящем скелете пони, не только на
            // синтетической тестовой цепочке).
            let len_root = (mid_world.position - root_world.position).length();
            let len_mid = (tip_world.position - mid_world.position).length();

            // `solve_two_bone_ik` считает угол 0 как направление вдоль +X —
            // но кости в этом скелете обычно направлены как угодно (в
            // дефолтном пони-скелете нога идёт вдоль -Y). Нужен "rest
            // angle" — направление кости в её ТЕКУЩЕЙ (до-IK) позе — чтобы
            // компенсировать эту разницу перед тем, как записать угол в
            // local_transform.rotation. Без этой компенсации нога сгибалась
            // не в ту сторону и не туда, куда нужно (см. регрессионный тест
            // `ik_constraint_moves_hoof_to_target`, который поймал это
            // именно так — числом, не на глаз: Hoof оказывался ДАЛЬШЕ от
            // target после применения IK, чем до).
            let root_rest_dir = (mid_world.position - root_world.position).angle_to(Vec2::X) * -1.0;
            let mid_rest_dir = (tip_world.position - mid_world.position).angle_to(Vec2::X) * -1.0;

            let ik = solve_two_bone_ik(root_world.position, len_root, len_mid, constraint.target, constraint.pole_target);

            if let Some(root_mut) = resolved.bones.iter_mut().find(|b| b.id == constraint.root) {
                // ik.root_world_angle — угол НАПРАВЛЕНИЯ кости в мировых
                // координатах (0 = вдоль +X). Компенсируем rest-направление
                // и текущий мировой угол родителя, чтобы получить итоговый
                // ЛОКАЛЬНЫЙ rotation, который при composed с родителем даст
                // направление ровно ik.root_world_angle.
                let parent_angle = root_mut
                    .parent
                    .clone()
                    .and_then(|p| self.world_transform(&p))
                    .map(|w| w.rotation)
                    .unwrap_or(0.0);
                let target_local_angle = ik.root_world_angle - root_rest_dir - parent_angle + root_mut.local_transform.rotation;
                let blended = lerp_angle(root_mut.local_transform.rotation, target_local_angle, constraint.weight.clamp(0.0, 1.0));
                root_mut.local_transform.rotation = blended;
            }
            if let Some(mid_mut) = resolved.bones.iter_mut().find(|b| b.id == constraint.mid) {
                // mid_local_angle из солвера — это угол ЦЕПОЧКИ mid относительно
                // root (в мировых координатах, считая root_world_angle=0 базой),
                // то есть уже "локальный" угол сгиба колена. Компенсируем
                // rest-направление кости mid относительно root таким же образом.
                let mid_rest_relative = mid_rest_dir - root_rest_dir;
                let target_local_angle = ik.mid_local_angle - mid_rest_relative + mid_mut.local_transform.rotation;
                let blended = lerp_angle(mid_mut.local_transform.rotation, target_local_angle, constraint.weight.clamp(0.0, 1.0));
                mid_mut.local_transform.rotation = blended;
            }
        }
        resolved.world_transform(id)
    }

    pub fn add_ik_constraint(&mut self, constraint: IkConstraint) -> &mut Self {
        self.ik_constraints.push(constraint);
        self
    }

    pub fn remove_ik_constraint(&mut self, id: &str) -> bool {
        let before = self.ik_constraints.len();
        self.ik_constraints.retain(|c| c.id != id);
        self.ik_constraints.len() != before
    }

    pub fn find_ik_constraint_mut(&mut self, id: &str) -> Option<&mut IkConstraint> {
        self.ik_constraints.iter_mut().find(|c| c.id == id)
    }

    /// Является ли `id` потомком `ancestor_id` (или самой этой костью) —
    /// нужно, чтобы не дать репривязку родителя создать цикл в иерархии.
    pub fn is_descendant_of(&self, id: &str, ancestor_id: &str) -> bool {
        let mut current = Some(id.to_string());
        while let Some(cur) = current {
            if cur == ancestor_id {
                return true;
            }
            current = self.find(&cur).and_then(|b| b.parent.clone());
        }
        false
    }

    /// Переродить кость на нового родителя. Отказывает (возвращает `false`,
    /// ничего не меняя), если новый родитель не существует, совпадает с
    /// самой костью, или является её потомком — иначе иерархия зациклится
    /// и `world_transform` уйдёт в бесконечную рекурсию.
    pub fn reparent(&mut self, id: &str, new_parent: &str) -> bool {
        if id == new_parent || self.find(new_parent).is_none() || self.is_descendant_of(new_parent, id) {
            return false;
        }
        match self.bones.iter_mut().find(|b| b.id == id) {
            Some(bone) => {
                bone.parent = Some(new_parent.to_string());
                true
            }
            None => false,
        }
    }

    /// Удалить кость и всё её поддерево (потомков — рекурсивно, а не
    /// только прямых детей). Возвращает id всех удалённых костей — что
    /// делать с частями персонажа/дорожками анимации, которые на них
    /// ссылались, решает вызывающая сторона на уровне `Character` (сам
    /// `Skeleton` ничего не знает про части и анимации).
    pub fn remove_subtree(&mut self, id: &str) -> Vec<BoneId> {
        let mut to_remove = vec![id.to_string()];
        let mut i = 0;
        while i < to_remove.len() {
            let current = to_remove[i].clone();
            for b in &self.bones {
                if b.parent.as_deref() == Some(current.as_str()) {
                    to_remove.push(b.id.clone());
                }
            }
            i += 1;
        }
        self.bones.retain(|b| !to_remove.contains(&b.id));
        to_remove
    }

    /// Переименовать кость: меняет её собственный id и все ссылки `parent`
    /// у прямых детей. Отказывает, если новое имя уже занято или старого
    /// не существует. Ссылки на кость из `Part::bone`/`AnimTarget::Bone`
    /// (в `Character`) — забота вызывающей стороны, `Skeleton` их не видит.
    pub fn rename_bone(&mut self, old_id: &str, new_id: &str) -> bool {
        if old_id == new_id || self.find(new_id).is_some() || self.find(old_id).is_none() {
            return false;
        }
        for b in &mut self.bones {
            if b.id == old_id {
                b.id = new_id.to_string();
            }
            if b.parent.as_deref() == Some(old_id) {
                b.parent = Some(new_id.to_string());
            }
        }
        true
    }
}

/// Линейная интерполяция угла по кратчайшему пути (не наивный `lerp` по
/// значениям в радианах, который дал бы неверный результат при переходе
/// через границу ±π — например, lerp(3.0, -3.0, 0.5) должен пройти ЧЕРЕЗ
/// π, а не через 0.0). Используется для `IkConstraint::weight` — смешивание
/// IK-угла с исходной FK-позой (раздел 94: constraints применяются с весом).
fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    let mut diff = (to - from) % std::f32::consts::TAU;
    if diff > std::f32::consts::PI {
        diff -= std::f32::consts::TAU;
    } else if diff < -std::f32::consts::PI {
        diff += std::f32::consts::TAU;
    }
    from + diff * t
}

fn compose(parent: Transform2D, local: Transform2D) -> Transform2D {
    let (sin, cos) = parent.rotation.sin_cos();
    let rotated = Vec2::new(
        local.position.x * cos - local.position.y * sin,
        local.position.x * sin + local.position.y * cos,
    );
    Transform2D {
        position: parent.position + rotated * parent.scale,
        rotation: parent.rotation + local.rotation,
        scale: parent.scale * local.scale,
    }
}

/// Стандартный скелет пони-персонажа по мотивам ТЗ (раздел 6).
pub fn default_pony_skeleton() -> Skeleton {
    let mut sk = Skeleton::new();
    sk.add_bone(Bone {
        id: "Root".into(),
        parent: None,
        local_transform: Transform2D::default(),
        length: 0.0,
    })
    .add_bone(Bone {
        id: "Body".into(),
        parent: Some("Root".into()),
        local_transform: Transform2D { position: Vec2::new(0.0, 0.0), ..Default::default() },
        length: 40.0,
    })
    .add_bone(Bone {
        id: "Neck".into(),
        parent: Some("Body".into()),
        local_transform: Transform2D { position: Vec2::new(16.0, 8.0), ..Default::default() },
        length: 10.0,
    })
    .add_bone(Bone {
        id: "Head".into(),
        parent: Some("Neck".into()),
        local_transform: Transform2D { position: Vec2::new(7.0, 9.0), ..Default::default() },
        length: 15.0,
    })
    .add_bone(Bone {
        id: "Horn".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D { position: Vec2::new(2.0, 9.0), ..Default::default() },
        length: 5.0,
    })
    .add_bone(Bone {
        id: "EarL".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D { position: Vec2::new(-3.0, 8.0), ..Default::default() },
        length: 4.0,
    })
    .add_bone(Bone {
        id: "EarR".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D { position: Vec2::new(4.0, 8.0), ..Default::default() },
        length: 4.0,
    });

    // Плечи/бёдра разнесены по X (перед/зад) и слегка по Y, дальше нога
    // идёт вниз звеньями. Раньше ВСЕ кости имели нулевое смещение, из-за
    // чего весь скелет схлопывался в одну точку: все части персонажа
    // рисовались друг на друге, и он выглядел бесформенным пятном.
    for (leg, shoulder_x, shoulder_y) in [("FL", 12.0, -6.0), ("FR", 15.0, -6.0), ("BL", -12.0, -6.0), ("BR", -15.0, -6.0)] {
        sk.add_bone(Bone {
            id: format!("Shoulder{leg}"),
            parent: Some("Body".into()),
            local_transform: Transform2D { position: Vec2::new(shoulder_x, shoulder_y), ..Default::default() },
            length: 5.0,
        })
        .add_bone(Bone {
            id: format!("UpperLeg{leg}"),
            parent: Some(format!("Shoulder{leg}")),
            local_transform: Transform2D { position: Vec2::new(0.0, -7.0), ..Default::default() },
            length: 12.0,
        })
        .add_bone(Bone {
            id: format!("LowerLeg{leg}"),
            parent: Some(format!("UpperLeg{leg}")),
            local_transform: Transform2D { position: Vec2::new(0.0, -9.0), ..Default::default() },
            length: 12.0,
        })
        .add_bone(Bone {
            id: format!("Hoof{leg}"),
            parent: Some(format!("LowerLeg{leg}")),
            local_transform: Transform2D { position: Vec2::new(0.0, -8.0), ..Default::default() },
            length: 3.0,
        });
    }

    sk
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Регрессия на реальную находку: раньше у ВСЕХ костей скелета было
    /// нулевое локальное смещение, поэтому все кости оказывались в одной
    /// точке, все части персонажа рисовались друг на друге, и он выглядел
    /// бесформенным пятном. Скелет обязан иметь реальную протяжённость.
    #[test]
    fn default_skeleton_is_spread_out_not_collapsed_to_a_point() {
        let sk = default_pony_skeleton();
        let mut min = Vec2::splat(f32::MAX);
        let mut max = Vec2::splat(f32::MIN);
        for bone in &sk.bones {
            let w = sk.world_transform(&bone.id).expect("кость должна разрешаться");
            min = min.min(w.position);
            max = max.max(w.position);
        }
        let extent = max - min;
        assert!(extent.x > 20.0, "скелет должен быть протяжённым по X, получили {extent:?}");
        assert!(extent.y > 20.0, "скелет должен быть протяжённым по Y, получили {extent:?}");
    }

    #[test]
    fn head_is_above_body_and_hooves_are_below() {
        let sk = default_pony_skeleton();
        let body = sk.world_transform("Body").unwrap().position;
        let head = sk.world_transform("Head").unwrap().position;
        let hoof = sk.world_transform("HoofFL").unwrap().position;
        assert!(head.y > body.y, "голова должна быть выше тела: голова {head:?}, тело {body:?}");
        assert!(hoof.y < body.y, "копыто должно быть ниже тела: копыто {hoof:?}, тело {body:?}");
    }

    #[test]
    fn front_and_back_legs_are_on_opposite_sides() {
        let sk = default_pony_skeleton();
        let front = sk.world_transform("ShoulderFL").unwrap().position;
        let back = sk.world_transform("ShoulderBL").unwrap().position;
        assert!(front.x > 0.0 && back.x < 0.0, "перед и зад должны быть по разные стороны: {front:?} vs {back:?}");
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;

    fn chain() -> Skeleton {
        // Root -> A -> B -> C, плюс D как отдельный ребёнок Root.
        let mut sk = Skeleton::new();
        sk.add_bone(Bone { id: "Root".into(), parent: None, local_transform: Transform2D::default(), length: 0.0 })
            .add_bone(Bone { id: "A".into(), parent: Some("Root".into()), local_transform: Transform2D::default(), length: 1.0 })
            .add_bone(Bone { id: "B".into(), parent: Some("A".into()), local_transform: Transform2D::default(), length: 1.0 })
            .add_bone(Bone { id: "C".into(), parent: Some("B".into()), local_transform: Transform2D::default(), length: 1.0 })
            .add_bone(Bone { id: "D".into(), parent: Some("Root".into()), local_transform: Transform2D::default(), length: 1.0 });
        sk
    }

    #[test]
    fn is_descendant_of_walks_up_the_chain() {
        let sk = chain();
        assert!(sk.is_descendant_of("C", "A"));
        assert!(sk.is_descendant_of("C", "Root"));
        assert!(sk.is_descendant_of("C", "C"), "кость считается потомком самой себя для целей проверки цикла");
        assert!(!sk.is_descendant_of("A", "C"), "A не потомок C");
        assert!(!sk.is_descendant_of("D", "A"), "D и A в разных ветках");
    }

    #[test]
    fn reparent_rejects_cycles() {
        let mut sk = chain();
        assert!(!sk.reparent("A", "C"), "A -> C создал бы цикл (C уже потомок A)");
        assert!(!sk.reparent("A", "A"), "кость не может быть родителем самой себе");
        assert!(!sk.reparent("A", "NoSuchBone"), "несуществующий родитель — отказ");
        // Ничего не изменилось после всех отказов.
        assert_eq!(sk.find("A").unwrap().parent.as_deref(), Some("Root"));
    }

    #[test]
    fn reparent_moves_the_bone_to_a_valid_new_parent() {
        let mut sk = chain();
        assert!(sk.reparent("C", "D"));
        assert_eq!(sk.find("C").unwrap().parent.as_deref(), Some("D"));
    }

    #[test]
    fn remove_subtree_removes_the_bone_and_all_descendants_only() {
        let mut sk = chain();
        let removed = sk.remove_subtree("A");
        let mut removed_sorted = removed.clone();
        removed_sorted.sort();
        assert_eq!(removed_sorted, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
        assert!(sk.find("A").is_none() && sk.find("B").is_none() && sk.find("C").is_none());
        assert!(sk.find("Root").is_some() && sk.find("D").is_some(), "не-потомки должны остаться");
    }

    #[test]
    fn rename_bone_updates_own_id_and_childrens_parent_refs() {
        let mut sk = chain();
        assert!(sk.rename_bone("A", "Arm"));
        assert!(sk.find("A").is_none());
        assert_eq!(sk.find("Arm").unwrap().parent.as_deref(), Some("Root"));
        assert_eq!(sk.find("B").unwrap().parent.as_deref(), Some("Arm"), "ребёнок B должен ссылаться на новое имя");
    }

    #[test]
    fn rename_bone_rejects_name_collision_and_missing_source() {
        let mut sk = chain();
        assert!(!sk.rename_bone("A", "B"), "имя 'B' уже занято");
        assert!(!sk.rename_bone("NoSuchBone", "X"));
        assert!(sk.find("A").is_some(), "ничего не должно было измениться");
    }
}

#[cfg(test)]
mod ik_integration_tests {
    use super::*;

    /// Простая двухзвенная нога: Hip(0,0) -> Knee (длина 12) -> Hoof (длина 8),
    /// без поворота — прямая вниз по Y, как в примере ТЗ (раздел 41).
    fn leg_skeleton() -> Skeleton {
        let mut sk = Skeleton::new();
        sk.add_bone(Bone { id: "Root".into(), parent: None, local_transform: Transform2D::default(), length: 0.0 })
            .add_bone(Bone {
                id: "Hip".into(),
                parent: Some("Root".into()),
                local_transform: Transform2D { position: Vec2::new(0.0, 0.0), ..Default::default() },
                length: 12.0,
            })
            .add_bone(Bone {
                id: "Knee".into(),
                parent: Some("Hip".into()),
                local_transform: Transform2D { position: Vec2::new(0.0, -12.0), ..Default::default() },
                length: 8.0,
            })
            .add_bone(Bone {
                id: "Hoof".into(),
                parent: Some("Knee".into()),
                local_transform: Transform2D { position: Vec2::new(0.0, -8.0), ..Default::default() },
                length: 3.0,
            });
        sk
    }

    /// Без IK-констрейнтов `world_transform_with_ik` обязана давать РОВНО
    /// тот же результат, что и обычный `world_transform` — быстрый путь
    /// (см. `if self.ik_constraints.iter().all(|c| !c.enabled)`) не должен
    /// незаметно менять поведение персонажей без IK (регрессия для всего
    /// уже существующего контента).
    #[test]
    fn without_constraints_ik_transform_matches_plain_fk() {
        let sk = leg_skeleton();
        let plain = sk.world_transform("Hoof").unwrap();
        let with_ik = sk.world_transform_with_ik("Hoof").unwrap();
        assert!((plain.position - with_ik.position).length() < 1e-4);
    }

    /// Главная проверка: Hoof реально дотягивается до target (раздел 41 —
    /// "перемещение копыта автоматически изменяет положение колена и бедра").
    #[test]
    fn ik_constraint_moves_hoof_to_target() {
        let mut sk = leg_skeleton();
        let mut ik = crate::ik::IkConstraint::new("leg_ik", "Hip", "Knee", "Hoof");
        // Target в стороне и чуть вперёд от прямой вытянутой позиции —
        // достижимо (12+8=20 максимум, цель на расстоянии ~14.14).
        ik.target = Vec2::new(10.0, -10.0);
        sk.add_ik_constraint(ik);

        let hoof_world = sk.world_transform_with_ik("Hoof").unwrap();
        // Hoof — не Knee, у него своя длина 3.0 от Knee, поэтому не бьёт
        // ТОЧНО в target (IK решает только root+mid, tip продолжает по
        // своей исходной локальной трансформации от mid) — но Knee должен
        // сдвинуться в сторону target гораздо ближе, чем прямая нога вниз.
        let plain_hoof = sk.world_transform("Hoof").unwrap();
        let dist_before = (plain_hoof.position - ik_target_for_test()).length();
        let dist_after = (hoof_world.position - ik_target_for_test()).length();
        assert!(dist_after < dist_before, "IK должен приблизить Hoof к target: было {dist_before}, стало {dist_after}");
    }

    fn ik_target_for_test() -> Vec2 {
        Vec2::new(10.0, -10.0)
    }

    /// Отключённый (`enabled = false`) констрейнт не должен влиять на позу —
    /// раздел 94 требует явного `enabled`, не просто "констрейнт есть -
    /// констрейнт применяется всегда".
    #[test]
    fn disabled_constraint_is_ignored() {
        let mut sk = leg_skeleton();
        let mut ik = crate::ik::IkConstraint::new("leg_ik", "Hip", "Knee", "Hoof");
        ik.target = Vec2::new(10.0, -10.0);
        ik.enabled = false;
        sk.add_ik_constraint(ik);

        let plain = sk.world_transform("Hoof").unwrap();
        let with_disabled_ik = sk.world_transform_with_ik("Hoof").unwrap();
        assert!(
            (plain.position - with_disabled_ik.position).length() < 1e-4,
            "выключенный констрейнт не должен менять позу"
        );
    }

    /// weight=0.0 должен давать позу, неотличимую от чистого FK (раздел 94:
    /// "weight (0..1, для смешивания с базовой анимацией)").
    #[test]
    fn zero_weight_constraint_matches_fk() {
        let mut sk = leg_skeleton();
        let mut ik = crate::ik::IkConstraint::new("leg_ik", "Hip", "Knee", "Hoof");
        ik.target = Vec2::new(10.0, -10.0);
        ik.weight = 0.0;
        sk.add_ik_constraint(ik);

        let plain = sk.world_transform("Hoof").unwrap();
        let with_zero_weight = sk.world_transform_with_ik("Hoof").unwrap();
        assert!(
            (plain.position - with_zero_weight.position).length() < 1e-3,
            "weight=0 должен быть неотличим от FK: plain={plain:?}, ik={with_zero_weight:?}"
        );
    }

    /// Констрейнт, ссылающийся на несуществующую кость, не должен паниковать —
    /// та же дисциплина, что и у "мёртвых" ссылок на удалённые кости в анимациях.
    #[test]
    fn constraint_with_missing_bone_does_not_panic() {
        let mut sk = leg_skeleton();
        let ik = crate::ik::IkConstraint::new("bad_ik", "Hip", "NoSuchBone", "Hoof");
        sk.add_ik_constraint(ik);
        let result = sk.world_transform_with_ik("Hoof");
        assert!(result.is_some(), "должен вернуть валидный результат, проигнорировав битый констрейнт");
    }

    /// add/remove/find для управления констрейнтами из GUI.
    #[test]
    fn ik_constraint_management() {
        let mut sk = leg_skeleton();
        sk.add_ik_constraint(crate::ik::IkConstraint::new("leg_ik", "Hip", "Knee", "Hoof"));
        assert!(sk.find_ik_constraint_mut("leg_ik").is_some());
        assert!(sk.remove_ik_constraint("leg_ik"));
        assert!(sk.find_ik_constraint_mut("leg_ik").is_none());
        assert!(!sk.remove_ik_constraint("leg_ik"), "повторное удаление — false, не паника");
    }

    /// Регрессия на реальную находку: длина звена ДОЛЖНА браться из
    /// геометрического расстояния между костями (`world_transform`), а не
    /// из декларативного поля `Bone::length` — на настоящем скелете пони
    /// (`default_pony_skeleton`) они не совпадают (`UpperLegFL.length ==
    /// 12.0`, но фактическое расстояние Shoulder->UpperLeg — 7.0). При
    /// первой версии солвера, использовавшей `Bone::length` напрямую, нога
    /// не дотягивалась до достижимой цели (расстояние до target — 2.74
    /// units при желаемом < 0.1) — поймано именно на реальной геометрии
    /// персонажа, не на упрощённой синтетической цепочке из `leg_skeleton()`.
    #[test]
    fn ik_reaches_target_on_the_real_default_pony_skeleton() {
        let mut sk = default_pony_skeleton();
        let hip_pos = sk.world_transform("ShoulderFL").unwrap().position;
        let upper_pos = sk.world_transform("UpperLegFL").unwrap().position;
        let lower_pos = sk.world_transform("LowerLegFL").unwrap().position;
        let max_reach = (upper_pos - hip_pos).length() + (lower_pos - upper_pos).length();

        // Достижимая цель (60% максимального вытягивания по диагонали).
        let target = hip_pos + Vec2::new(max_reach * 0.6, -max_reach * 0.5);

        let mut ik = crate::ik::IkConstraint::new("front_leg", "ShoulderFL", "UpperLegFL", "LowerLegFL");
        ik.target = target;
        sk.add_ik_constraint(ik);

        let lower_leg_world = sk.world_transform_with_ik("LowerLegFL").unwrap();
        let dist = (lower_leg_world.position - target).length();
        assert!(dist < 0.1, "LowerLeg должен почти точно попасть в достижимый target на реальном скелете пони, расстояние: {dist}");
    }
}




