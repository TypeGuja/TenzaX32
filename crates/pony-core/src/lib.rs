//! pony-core: описание персонажа (скелет/части/морфы/анимации),
//! не хранит ни одного растрового кадра.

pub mod animation;
pub mod boolean;
pub mod camera;
pub mod character;
pub mod group;
pub mod ik;
pub mod lighting;
pub mod morph;
pub mod orientation;
pub mod part;
pub mod particles;
pub mod player;
pub mod skeleton;
pub mod vector;

pub use boolean::{boolean_op, flatten_shape_to_contour, piece_to_shape, BooleanOp, BooleanPiece, BooleanResult};
pub use camera::Camera;
pub use character::{AssetError, Character};
pub use group::{GroupId, GroupTree, PartGroup};
pub use ik::{solve_two_bone_ik, IkConstraint, TwoBoneIkResult};
pub use lighting::{AmbientLight, Lighting, PointLight, SunLight};
pub use orientation::apply_yaw_2_5d;
pub use particles::{Particle, ParticleEmitter, ParticleKind};
pub use player::AnimationPlayer;
pub use vector::{
    gradient_t_at, resolve_symbol_instance, GradientDef, GradientKind, GradientStop, NodeType,
    PathNode, PathPointKind, RgbaColor, SymbolDef, VectorDoc, VectorParseError, VectorShape,
};
