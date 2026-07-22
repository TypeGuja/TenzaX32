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
        local_transform: Transform2D::default(),
        length: 40.0,
    })
    .add_bone(Bone {
        id: "Neck".into(),
        parent: Some("Body".into()),
        local_transform: Transform2D::default(),
        length: 10.0,
    })
    .add_bone(Bone {
        id: "Head".into(),
        parent: Some("Neck".into()),
        local_transform: Transform2D::default(),
        length: 15.0,
    })
    .add_bone(Bone {
        id: "Horn".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D::default(),
        length: 5.0,
    })
    .add_bone(Bone {
        id: "EarL".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D::default(),
        length: 4.0,
    })
    .add_bone(Bone {
        id: "EarR".into(),
        parent: Some("Head".into()),
        local_transform: Transform2D::default(),
        length: 4.0,
    });

    for leg in ["FL", "FR", "BL", "BR"] {
        sk.add_bone(Bone {
            id: format!("Shoulder{leg}"),
            parent: Some("Body".into()),
            local_transform: Transform2D::default(),
            length: 5.0,
        })
        .add_bone(Bone {
            id: format!("UpperLeg{leg}"),
            parent: Some(format!("Shoulder{leg}")),
            local_transform: Transform2D::default(),
            length: 12.0,
        })
        .add_bone(Bone {
            id: format!("LowerLeg{leg}"),
            parent: Some(format!("UpperLeg{leg}")),
            local_transform: Transform2D::default(),
            length: 12.0,
        })
        .add_bone(Bone {
            id: format!("Hoof{leg}"),
            parent: Some(format!("LowerLeg{leg}")),
            local_transform: Transform2D::default(),
            length: 3.0,
        });
    }

    sk
}
