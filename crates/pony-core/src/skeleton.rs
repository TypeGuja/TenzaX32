//! Скелет персонажа: иерархия костей.
//! Root -> Body -> Neck -> Head -> Horn / Ears
//! Body -> Shoulder -> UpperLeg -> LowerLeg -> Hoof (x4)

use glam::{Vec2};
use serde::{Deserialize, Serialize};

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
