//! pony-core: описание персонажа (скелет/части/морфы/анимации),
//! не хранит ни одного растрового кадра.

pub mod animation;
pub mod character;
pub mod morph;
pub mod part;
pub mod skeleton;

pub use character::{AssetError, Character};
