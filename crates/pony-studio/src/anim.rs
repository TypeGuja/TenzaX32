//! Ключевые кадры и анимации — перенесено из старого egui-интерфейса
//! (`pony-gui`) без изменений поведения, вместе с тестами.

use pony_core::animation::{AnimTarget, AnimValue, Animation, BoneChannel, Interpolation, Keyframe, Track};
use pony_core::{AnimationPlayer, Character};

pub const FPS: f32 = 24.0;

pub const CHANNELS: [BoneChannel; 5] = [BoneChannel::PositionX, BoneChannel::PositionY, BoneChannel::Rotation, BoneChannel::ScaleX, BoneChannel::ScaleY];

pub fn channel_key(channel: BoneChannel) -> &'static str {
    match channel {
        BoneChannel::PositionX => "x",
        BoneChannel::PositionY => "y",
        BoneChannel::Rotation => "rot",
        BoneChannel::ScaleX => "sx",
        BoneChannel::ScaleY => "sy",
    }
}

pub fn channel_label(channel: BoneChannel) -> &'static str {
    match channel {
        BoneChannel::PositionX => "Position X",
        BoneChannel::PositionY => "Position Y",
        BoneChannel::Rotation => "Rotation",
        BoneChannel::ScaleX => "Scale X",
        BoneChannel::ScaleY => "Scale Y",
    }
}

pub fn channel_from_key(key: &str) -> Option<BoneChannel> {
    CHANNELS.into_iter().find(|c| channel_key(*c) == key)
}

/// Первое свободное имя вида `base`, `base_2`, `base_3`...
pub fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|c| !taken(c)).expect("бесконечный ряд кандидатов")
}

/// Имя для новой анимации, не занятое у персонажа: "Animation", "Animation 2", ...
pub fn unique_animation_name(character: &Character) -> String {
    let mut n = 1;
    loop {
        let name = if n == 1 { "Animation".to_string() } else { format!("Animation {n}") };
        if !character.animations.contains_key(&name) {
            return name;
        }
        n += 1;
    }
}

/// Имя текущей анимации; если играть нечего — создаёт пустую и делает
/// текущей (иначе ключевые кадры молча не ставились бы).
pub fn ensure_animation(character: &mut Character, player: &mut AnimationPlayer) -> String {
    if let Some(name) = player.current_name().filter(|n| character.animations.contains_key(*n)) {
        return name.to_string();
    }
    let name = unique_animation_name(character);
    character.add_animation(Animation { name: name.clone(), duration: 1.0, looping: true, tracks: Vec::new() });
    player.play(&name);
    name
}

/// Анимация, с которой открывать персонажа: "Walk", если есть, иначе первая
/// по алфавиту. Плеер на кадре 0, без воспроизведения.
pub fn pick_default_animation(character: &Character, player: &mut AnimationPlayer) {
    *player = AnimationPlayer::new();
    let name = if character.animations.contains_key("Walk") {
        Some("Walk".to_string())
    } else {
        let mut names: Vec<&String> = character.animations.keys().collect();
        names.sort();
        names.first().map(|n| n.to_string())
    };
    if let Some(name) = name {
        player.play(name);
    }
}

/// Вставить (или заменить) ключ канала кости в момент `time`.
pub fn insert_keyframe(anim: &mut Animation, bone_id: &str, channel: BoneChannel, time: f32, value: f32) {
    let target_matches = |t: &AnimTarget| matches!(t, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel);
    let track = match anim.tracks.iter_mut().position(|t| target_matches(&t.target)) {
        Some(i) => &mut anim.tracks[i],
        None => {
            anim.tracks.push(Track { target: AnimTarget::Bone { id: bone_id.to_string(), channel }, keyframes: Vec::new() });
            anim.tracks.last_mut().expect("just pushed")
        }
    };
    let kf = Keyframe { time, value: AnimValue::Float(value), interpolation: Interpolation::Linear };
    match track.keyframes.iter().position(|k| (k.time - time).abs() < 1e-4) {
        Some(i) => track.keyframes[i] = kf,
        None => {
            track.keyframes.push(kf);
            track.keyframes.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    if time > anim.duration {
        anim.duration = time;
    }
}

/// Убрать ключ (с точностью до полукадра); возвращает его значение.
pub fn remove_keyframe(anim: &mut Animation, bone_id: &str, channel: BoneChannel, time: f32) -> Option<f32> {
    let target_matches = |t: &AnimTarget| matches!(t, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel);
    let track = anim.tracks.iter_mut().find(|t| target_matches(&t.target))?;
    let idx = track.keyframes.iter().position(|k| (k.time - time).abs() < 0.5 / FPS)?;
    let kf = track.keyframes.remove(idx);
    match kf.value {
        AnimValue::Float(v) => Some(v),
        _ => Some(0.0),
    }
}

/// Есть ли уже ключ на этом кадре у дорожки канала кости.
pub fn has_keyframe_at(anim: &Animation, bone_id: &str, channel: BoneChannel, time: f32) -> bool {
    anim.tracks
        .iter()
        .filter(|t| matches!(&t.target, AnimTarget::Bone { id, channel: c } if id == bone_id && *c == channel))
        .any(|t| t.keyframes.iter().any(|k| (k.time - time).abs() < 0.5 / FPS))
}

/// Текущее значение канала кости — им затравливается новый ключ.
pub fn bone_channel_value(bone: &pony_core::skeleton::Bone, channel: BoneChannel) -> f32 {
    match channel {
        BoneChannel::PositionX => bone.local_transform.position.x,
        BoneChannel::PositionY => bone.local_transform.position.y,
        BoneChannel::Rotation => bone.local_transform.rotation,
        BoneChannel::ScaleX => bone.local_transform.scale.x,
        BoneChannel::ScaleY => bone.local_transform.scale.y,
    }
}

/// Префикс строки таймлайна для кости без собственной части.
pub const BONE_ROW_PREFIX: &str = "кость: ";

/// Кость, к которой относятся ключи строки таймлайна.
pub fn row_bone(character: &Character, label: &str) -> Option<String> {
    match character.parts.get(label) {
        Some(part) => part.bone.clone(),
        None => label.strip_prefix(BONE_ROW_PREFIX).map(str::to_string),
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Row {
    pub label: String,
    /// "part" | "bone" | "misc"
    pub kind: &'static str,
    pub frames: Vec<i64>,
}

/// Строки таймлайна для выбранного канала: слои (части) по (layer, id),
/// затем кости без своих частей (если анимированы или выбраны), затем
/// "Прочее" — дорожки камеры/морфов и ключи костей без строки.
pub fn timeline_rows(character: &Character, anim: Option<&Animation>, channel: BoneChannel, selected_bone: Option<&str>) -> Vec<Row> {
    let tracks: &[Track] = anim.map(|a| a.tracks.as_slice()).unwrap_or(&[]);
    let mut layer_ids: Vec<String> = character.parts.keys().cloned().collect();
    layer_ids.sort_by_key(|id| (character.parts[id].layer, id.clone()));
    let mut rows: Vec<Row> = layer_ids.into_iter().map(|label| Row { label, kind: "part", frames: Vec::new() }).collect();

    let used_bones: std::collections::HashSet<&str> = character.parts.values().filter_map(|p| p.bone.as_deref()).collect();
    let animated_bones: std::collections::HashSet<&str> = tracks
        .iter()
        .filter_map(|t| match &t.target {
            AnimTarget::Bone { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    for bone in &character.skeleton.bones {
        if bone.parent.is_some() && !used_bones.contains(bone.id.as_str()) && (animated_bones.contains(bone.id.as_str()) || selected_bone == Some(bone.id.as_str())) {
            rows.push(Row { label: format!("{BONE_ROW_PREFIX}{}", bone.id), kind: "bone", frames: Vec::new() });
        }
    }

    let mut misc: Vec<i64> = Vec::new();
    for track in tracks {
        let bone_id = match &track.target {
            AnimTarget::Bone { id, channel: c } if *c == channel => Some(id.as_str()),
            AnimTarget::Bone { .. } => continue,
            _ => None,
        };
        let frames: Vec<i64> = track.keyframes.iter().map(|kf| (kf.time * FPS).round() as i64).collect();
        match bone_id {
            Some(bid) => {
                let mut any = false;
                for row in rows.iter_mut() {
                    if row_bone(character, &row.label).as_deref() == Some(bid) {
                        row.frames.extend(frames.iter().copied());
                        any = true;
                    }
                }
                if !any {
                    misc.extend(frames);
                }
            }
            None => misc.extend(frames),
        }
    }
    if !misc.is_empty() {
        rows.push(Row { label: "(Прочее: камера/морфы)".into(), kind: "misc", frames: misc });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_anim() -> Animation {
        Animation { name: "Test".into(), duration: 1.0, looping: false, tracks: Vec::new() }
    }

    #[test]
    fn insert_then_remove_keyframe_round_trips_the_value() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 12.5);
        assert_eq!(anim.tracks.len(), 1);
        assert_eq!(remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5), Some(12.5));
        assert!(anim.tracks[0].keyframes.is_empty());
    }

    #[test]
    fn remove_keyframe_at_wrong_time_returns_none_and_does_not_touch_others() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 12.5);
        assert_eq!(remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.9), None);
        assert_eq!(anim.tracks[0].keyframes.len(), 1);
    }

    #[test]
    fn dragging_a_keyframe_to_a_new_time_is_remove_then_insert() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.2, 7.0);
        let value = remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.2).unwrap();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.8, value);
        assert_eq!(anim.tracks[0].keyframes.len(), 1);
        assert!((anim.tracks[0].keyframes[0].time - 0.8).abs() < 1e-4);
    }

    #[test]
    fn insert_keyframe_extends_duration_when_keyframe_is_past_the_end() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 2.5, 0.0);
        assert!((anim.duration - 2.5).abs() < 1e-4);
    }

    #[test]
    fn remove_keyframe_finds_key_placed_between_frames() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.43, 3.0);
        assert_eq!(remove_keyframe(&mut anim, "Head", BoneChannel::PositionY, 10.0 / FPS), Some(3.0));
    }

    #[test]
    fn has_keyframe_at_only_sees_the_same_bone_and_channel() {
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.5, 1.0);
        assert!(has_keyframe_at(&anim, "Head", BoneChannel::PositionY, 0.5));
        assert!(!has_keyframe_at(&anim, "Head", BoneChannel::Rotation, 0.5));
        assert!(!has_keyframe_at(&anim, "Body", BoneChannel::PositionY, 0.5));
    }

    #[test]
    fn rows_show_bone_tracks_on_every_part_of_that_bone_and_misc_for_the_rest() {
        let mut c = Character::new("t");
        c.skeleton = pony_core::skeleton::default_pony_skeleton();
        let src = || pony_core::part::PartSource::Png { path: String::new() };
        c.add_part(pony_core::part::Part::new("body", pony_core::part::PartKind::Body, src()).with_bone("Body"));
        c.add_part(pony_core::part::Part::new("tail", pony_core::part::PartKind::Tail, src()).with_bone("Body").with_layer(-1));
        let mut anim = test_anim();
        insert_keyframe(&mut anim, "Body", BoneChannel::PositionY, 0.5, 1.0);
        insert_keyframe(&mut anim, "Head", BoneChannel::PositionY, 0.25, 1.0);
        let rows = timeline_rows(&c, Some(&anim), BoneChannel::PositionY, None);
        assert_eq!(rows[0].label, "tail", "сначала нижний слой");
        assert_eq!(rows[0].frames, vec![12]);
        assert_eq!(rows[1].frames, vec![12]);
        assert!(rows.iter().any(|r| r.label == "кость: Head" && r.frames == vec![6]), "у кости без части — своя строка");
        assert!(timeline_rows(&c, Some(&anim), BoneChannel::Rotation, None).iter().all(|r| r.frames.is_empty()));
    }
}
