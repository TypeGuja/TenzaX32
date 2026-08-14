//! pony-core: описание персонажа (скелет/части/морфы/анимации),
//! не хранит ни одного растрового кадра.

pub mod animation;
pub mod camera;
pub mod character;
pub mod lighting;
pub mod morph;
pub mod orientation;
pub mod part;
pub mod particles;
pub mod player;
pub mod skeleton;
pub mod vector;

pub use camera::Camera;
pub use character::{AssetError, Character};
pub use lighting::{AmbientLight, Lighting, PointLight, SunLight};
pub use orientation::apply_yaw_2_5d;
pub use particles::{Particle, ParticleEmitter, ParticleKind};
pub use player::AnimationPlayer;
pub use vector::{NodeType, PathNode, PathPointKind, RgbaColor, VectorDoc, VectorParseError, VectorShape};
