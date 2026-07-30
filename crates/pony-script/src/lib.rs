//! Скриптовый слой (раздел 15 ТЗ). Скрипт на rhai описывает намерение
//! через `pony.*`/`camera.*` вызовы (см. `engine.rs`), а эта функция
//! применяет получившиеся команды к реальному состоянию персонажа/камеры.

pub mod commands;
pub mod engine;

pub use commands::Command;
pub use engine::{ScriptEngine, ScriptError};

use pony_core::{AnimationPlayer, Camera, Character};

/// Применить последовательность команд к персонажу, камере и проигрывателю
/// анимаций. `Move`/`CameraMove` и т.п. — накопительные (сдвиг/множитель),
/// а не присваивание абсолютного значения, поэтому порядок команд имеет
/// значение и результат зависит от того, что уже было применено раньше.
///
/// `pony.Walk()` не двигает кости сам — он запускает анимацию "Walk" через
/// `player`, если она есть у персонажа (см. `AnimationPlayer` в pony-core).
/// Дальше именно проигрыватель на каждый кадр применяет её дорожки.
pub fn apply_commands(character: &mut Character, camera: &mut Camera, player: &mut AnimationPlayer, commands: &[Command]) {
    for cmd in commands {
        match cmd {
            Command::Move { dx, dy } => {
                if let Some(root) = character.skeleton.bones.iter_mut().find(|b| b.id == "Root") {
                    root.local_transform.position.x += dx;
                    root.local_transform.position.y += dy;
                }
            }
            Command::Look { x, y } => {
                if let Some(head) = character.skeleton.bones.iter_mut().find(|b| b.id == "Head") {
                    head.local_transform.rotation = y.atan2(*x);
                }
            }
            Command::Blink => character.default_morph.set("Blink", 1.0),
            Command::Smile { amount } => character.default_morph.set("Smile", *amount),
            Command::Walk => {
                if character.animations.contains_key("Walk") {
                    player.play("Walk");
                } else {
                    eprintln!("[pony-script] pony.Walk() вызван, но у персонажа нет анимации \"Walk\"");
                }
            }
            Command::CameraMove { dx, dy } => camera.move_by(*dx, *dy),
            Command::CameraRotate { radians } => camera.rotate_by(*radians),
            Command::CameraZoom { factor } => camera.zoom_by(*factor),
            Command::CameraShake { intensity } => camera.shake(*intensity),
            Command::CameraDepth { value } => camera.depth = *value,
            Command::CameraBlur { value } => camera.blur = *value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pony_core::skeleton::default_pony_skeleton;

    fn test_character() -> Character {
        let mut c = Character::new("ScriptTestPony");
        c.skeleton = default_pony_skeleton();
        c
    }

    #[test]
    fn move_and_smile_and_camera_zoom() {
        let engine = ScriptEngine::new();
        let commands = engine
            .run(
                r#"
                pony.Move(5.0, -2.0);
                pony.Smile(0.75);
                pony.Blink();
                camera.Zoom(2.0);
                camera.Shake(0.3);
            "#,
            )
            .expect("script should run");

        assert_eq!(
            commands,
            vec![
                Command::Move { dx: 5.0, dy: -2.0 },
                Command::Smile { amount: 0.75 },
                Command::Blink,
                Command::CameraZoom { factor: 2.0 },
                Command::CameraShake { intensity: 0.3 },
            ]
        );

        let mut character = test_character();
        let mut camera = Camera::default();
        let mut player = AnimationPlayer::new();
        apply_commands(&mut character, &mut camera, &mut player, &commands);

        let root = character.skeleton.find("Root").unwrap();
        assert_eq!(root.local_transform.position.x, 5.0);
        assert_eq!(root.local_transform.position.y, -2.0);
        assert_eq!(character.default_morph.get("Smile"), 0.75);
        assert_eq!(character.default_morph.get("Blink"), 1.0);
        assert_eq!(camera.zoom, 2.0); // старт 1.0 * 2.0
        assert_eq!(camera.shake_intensity, 0.3);
    }

    #[test]
    fn commands_are_cumulative_and_order_dependent() {
        let engine = ScriptEngine::new();
        let commands = engine
            .run("pony.Move(1.0, 0.0); pony.Move(1.0, 0.0); pony.Move(1.0, 0.0);")
            .unwrap();

        let mut character = test_character();
        let mut camera = Camera::default();
        let mut player = AnimationPlayer::new();
        apply_commands(&mut character, &mut camera, &mut player, &commands);

        let root = character.skeleton.find("Root").unwrap();
        assert_eq!(root.local_transform.position.x, 3.0, "три сдвига по 1.0 должны накопиться");
    }

    #[test]
    fn camera_zoom_is_multiplicative_not_absolute() {
        let engine = ScriptEngine::new();
        let commands = engine.run("camera.Zoom(2.0); camera.Zoom(3.0);").unwrap();

        let mut character = test_character();
        let mut camera = Camera::default();
        let mut player = AnimationPlayer::new();
        apply_commands(&mut character, &mut camera, &mut player, &commands);

        assert_eq!(camera.zoom, 6.0, "1.0 * 2.0 * 3.0");
    }

    #[test]
    fn walk_starts_playback_when_animation_exists() {
        use pony_core::animation::{AnimTarget, AnimValue, Animation, BoneChannel, Interpolation, Keyframe, Track};

        let mut character = test_character();
        character.add_animation(Animation {
            name: "Walk".into(),
            duration: 1.0,
            looping: true,
            tracks: vec![Track {
                target: AnimTarget::Bone { id: "Root".into(), channel: BoneChannel::PositionX },
                keyframes: vec![
                    Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                    Keyframe { time: 1.0, value: AnimValue::Float(10.0), interpolation: Interpolation::Linear },
                ],
            }],
        });

        let engine = ScriptEngine::new();
        let commands = engine.run("pony.Walk();").unwrap();

        let mut camera = Camera::default();
        let mut player = AnimationPlayer::new();
        apply_commands(&mut character, &mut camera, &mut player, &commands);

        assert_eq!(player.current_name(), Some("Walk"));
        assert!(player.is_valid(&character));
    }

    #[test]
    fn walk_without_matching_animation_leaves_player_idle() {
        let character = test_character(); // без анимации "Walk"
        let engine = ScriptEngine::new();
        let commands = engine.run("pony.Walk();").unwrap();

        let mut character = character;
        let mut camera = Camera::default();
        let mut player = AnimationPlayer::new();
        apply_commands(&mut character, &mut camera, &mut player, &commands);

        assert_eq!(player.current_name(), None, "не должен запускать несуществующую анимацию");
    }

    #[test]
    fn invalid_script_returns_error_not_panic() {
        let engine = ScriptEngine::new();
        let result = engine.run("pony.Move(1.0);"); // не хватает аргумента
        assert!(result.is_err());
    }
}
