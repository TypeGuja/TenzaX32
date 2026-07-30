//! Проигрыватель анимаций: держит "какая анимация играет и с какого
//! момента" и на каждый тик применяет её дорожки к скелету/морфам
//! персонажа. Раньше `Track::sample()` можно было вызвать только вручную
//! на конкретное время (см. тесты в `animation.rs`) — этот модуль превращает
//! это в реальный проигрыватель с течением времени, паузой, циклами.
//!
//! Применение — АБСОЛЮТНОЕ (каждый тик перезаписывает значение канала),
//! как в большинстве скелетных аниматоров. Это значит: если скрипт через
//! `pony.Move()` сдвинул кость `Root`, а потом запустилась анимация,
//! у которой есть дорожка на `Root.PositionX`, — анимация с этого кадра
//! перезапишет сдвиг скрипта. Ожидаемое поведение, не баг: анимация задаёт
//! позу целиком, а не добавляется поверх текущей позы (аддитивные слои —
//! отдельная, более сложная модель, которой здесь нет).

use crate::animation::{AnimTarget, BoneChannel};
use crate::character::Character;

#[derive(Debug, Clone, Default)]
pub struct AnimationPlayer {
    current: Option<String>,
    time: f32,
}

impl AnimationPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Начать (или перезапустить) анимацию с указанным именем. Ничего не
    /// проверяет здесь — если у персонажа нет такой анимации, `advance`/
    /// `apply` просто ничего не будают делать (см. `is_valid`).
    pub fn play(&mut self, name: impl Into<String>) {
        self.current = Some(name.into());
        self.time = 0.0;
    }

    pub fn stop(&mut self) {
        self.current = None;
        self.time = 0.0;
    }

    pub fn current_name(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    /// Существует ли у персонажа анимация с текущим именем — полезно,
    /// чтобы отличить "доиграла до конца" от "такой анимации нет вовсе".
    pub fn is_valid(&self, character: &Character) -> bool {
        self.current.as_ref().is_some_and(|name| character.animations.contains_key(name))
    }

    /// Доиграла ли текущая незацикленная анимация до конца. Для
    /// зацикленных или отсутствующих анимаций — всегда false/true
    /// соответственно (нет анимации — значит и играть нечему, "закончена").
    pub fn is_finished(&self, character: &Character) -> bool {
        match self.current.as_ref().and_then(|name| character.animations.get(name)) {
            Some(anim) => !anim.looping && self.time >= anim.duration,
            None => true,
        }
    }

    /// Продвинуть время на `dt` секунд. Зацикленные анимации оборачиваются
    /// по модулю длительности; незацикленные — останавливаются на
    /// последнем кадре (не откатываются и не выходят за пределы).
    pub fn advance(&mut self, character: &Character, dt: f32) {
        let Some(anim) = self.current.as_ref().and_then(|name| character.animations.get(name)) else {
            return;
        };
        if anim.duration <= 0.0 {
            return;
        }
        self.time += dt;
        if anim.looping {
            self.time %= anim.duration;
        } else {
            self.time = self.time.min(anim.duration);
        }
    }

    /// Применить текущий кадр к скелету/морфам персонажа. Дорожки на
    /// камеру (`AnimTarget::Camera`) здесь пропускаются — камера не часть
    /// `Character`, её должен обновлять вызывающий код отдельно (см.
    /// `apply_camera` ниже, если понадобится).
    pub fn apply(&self, character: &mut Character) {
        let Some(anim) = self.current.as_ref().and_then(|name| character.animations.get(name).cloned()) else {
            return;
        };
        for track in &anim.tracks {
            let Some(value) = track.sample(self.time) else { continue };
            match &track.target {
                AnimTarget::Bone { id, channel } => {
                    if let Some(bone) = character.skeleton.bones.iter_mut().find(|b| &b.id == id) {
                        match channel {
                            BoneChannel::PositionX => bone.local_transform.position.x = value,
                            BoneChannel::PositionY => bone.local_transform.position.y = value,
                            BoneChannel::Rotation => bone.local_transform.rotation = value,
                            BoneChannel::ScaleX => bone.local_transform.scale.x = value,
                            BoneChannel::ScaleY => bone.local_transform.scale.y = value,
                        }
                    }
                }
                AnimTarget::Morph { name } => character.default_morph.set(name.clone(), value),
                AnimTarget::EyeParam { channel } => match channel.as_str() {
                    "radius" => character.default_morph.eyes.radius = value,
                    "height" => character.default_morph.eyes.height = value,
                    "pupil" => character.default_morph.eyes.pupil = value,
                    "rotation" => character.default_morph.eyes.rotation = value,
                    _ => {}
                },
                AnimTarget::Camera { .. } => {
                    // Камера — не часть Character, применяется отдельно.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{AnimValue, Animation, Interpolation, Keyframe, Track};
    use crate::skeleton::{Bone, Transform2D};

    fn character_with_bob_animation(looping: bool) -> Character {
        let mut character = Character::new("PlayerTestPony");
        character.skeleton.add_bone(Bone {
            id: "Head".into(),
            parent: None,
            local_transform: Transform2D::default(),
            length: 1.0,
        });
        character.add_animation(Animation {
            name: "Bob".into(),
            duration: 2.0,
            looping,
            tracks: vec![Track {
                target: AnimTarget::Bone { id: "Head".into(), channel: BoneChannel::PositionY },
                keyframes: vec![
                    Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                    Keyframe { time: 1.0, value: AnimValue::Float(-4.0), interpolation: Interpolation::Linear },
                    Keyframe { time: 2.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                ],
            }],
        });
        character
    }

    #[test]
    fn apply_moves_bone_according_to_elapsed_time() {
        let mut character = character_with_bob_animation(true);
        let mut player = AnimationPlayer::new();
        player.play("Bob");

        player.advance(&character, 1.0); // t=1.0 -> нижняя точка (-4.0)
        player.apply(&mut character);
        let head_y = character.skeleton.find("Head").unwrap().local_transform.position.y;
        assert!((head_y - (-4.0)).abs() < 1e-4, "got {head_y}");
    }

    #[test]
    fn looping_animation_wraps_time_around() {
        let character = character_with_bob_animation(true);
        let mut player = AnimationPlayer::new();
        player.play("Bob");
        player.advance(&character, 2.5); // длительность 2.0 -> должно обернуться на 0.5
        assert!((player.time() - 0.5).abs() < 1e-4, "got {}", player.time());
    }

    #[test]
    fn non_looping_animation_clamps_and_reports_finished() {
        let character = character_with_bob_animation(false);
        let mut player = AnimationPlayer::new();
        player.play("Bob");
        player.advance(&character, 10.0); // намного больше длительности
        assert!((player.time() - 2.0).abs() < 1e-4, "должно остановиться на последнем кадре");
        assert!(player.is_finished(&character));
    }

    #[test]
    fn missing_animation_is_valid_false_and_does_not_panic() {
        let character = character_with_bob_animation(true);
        let mut player = AnimationPlayer::new();
        player.play("DoesNotExist");
        assert!(!player.is_valid(&character));
        player.advance(&character, 1.0); // не должно паниковать
        player.apply(&mut character.clone()); // тоже не должно паниковать
    }
}
