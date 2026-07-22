use pony_core::animation::{AnimTarget, AnimValue, Animation, BoneChannel, Interpolation, Keyframe, Track};
use pony_core::part::{Part, PartKind, PartSource};
use pony_core::skeleton::default_pony_skeleton;
use pony_core::Character;

fn main() {
    let mut character = Character::new("SamplePony");
    character.skeleton = default_pony_skeleton();

    character
        .add_part(
            Part::new("body", PartKind::Body, PartSource::Vector { path: "assets/body.svg".into() })
                .with_bone("Body")
                .with_layer(0),
        )
        .add_part(
            Part::new("head", PartKind::Head, PartSource::Vector { path: "assets/head.svg".into() })
                .with_bone("Head")
                .with_layer(1),
        )
        .add_part(
            Part::new("eye_l", PartKind::Eyes, PartSource::Vector { path: "assets/eye.svg".into() })
                .with_bone("Head")
                .with_layer(2),
        );

    // Простая анимация "Blink" — вращение века (тут через морф) плюс
    // покачивание головы, чтобы показать работу дорожек/ключей.
    let blink = Animation {
        name: "Blink".into(),
        duration: 0.4,
        looping: false,
        tracks: vec![Track {
            target: AnimTarget::Morph { name: "Blink".into() },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.15, value: AnimValue::Float(1.0), interpolation: Interpolation::Linear },
                Keyframe { time: 0.4, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    };

    let idle_head_bob = Animation {
        name: "Idle".into(),
        duration: 2.0,
        looping: true,
        tracks: vec![Track {
            target: AnimTarget::Bone { id: "Head".into(), channel: BoneChannel::PositionY },
            keyframes: vec![
                Keyframe { time: 0.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
                Keyframe { time: 1.0, value: AnimValue::Float(-2.0), interpolation: Interpolation::Linear },
                Keyframe { time: 2.0, value: AnimValue::Float(0.0), interpolation: Interpolation::Linear },
            ],
        }],
    };

    character.add_animation(blink).add_animation(idle_head_bob);

    let out_path = "sample_pony.asset";
    character.save_to_file(out_path).expect("failed to save asset");
    println!("Saved character to {out_path}");

    let loaded = Character::load_from_file(out_path).expect("failed to load asset");
    println!(
        "Loaded '{}' v{}: {} parts, {} bones, {} animations",
        loaded.name,
        loaded.version,
        loaded.parts.len(),
        loaded.skeleton.bones.len(),
        loaded.animations.len()
    );

    // Пример вычисления мировой трансформации кости и сэмплинга анимации.
    if let Some(world_head) = loaded.skeleton.world_transform("Head") {
        println!("Head world position: {:?}", world_head.position);
    }
    if let Some(idle) = loaded.animations.get("Idle") {
        if let Some(track) = idle.tracks.first() {
            println!("Idle head-bob at t=0.5s: {:?}", track.sample(0.5));
        }
    }
}
