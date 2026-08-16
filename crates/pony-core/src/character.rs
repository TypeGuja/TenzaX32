//! Персонаж (раздел 4 ТЗ): Name, Version, Parts, Skeleton, Morphs,
//! Animations, Physics, Metadata — всё, что раньше было бы пятью тысячами
//! PNG, теперь одно описание.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::animation::Animation;
use crate::group::GroupTree;
use crate::morph::MorphState;
use crate::part::Part;
use crate::skeleton::Skeleton;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub author: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhysicsConfig {
    /// Кости, которые качаются пассивно (грива, хвост) — по имени кости
    /// и коэффициенту "мягкости" (0 = жёсткая, 1 = максимально свободная).
    pub soft_bones: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub version: String,
    pub parts: HashMap<String, Part>,
    /// Группы (раздел 27 ТЗ, `<g>`) — организационная иерархия ПОВЕРХ
    /// `parts` (см. `crate::group` и `Part::group`), не влияет на
    /// `skeleton`/деформацию. `#[serde(default)]` — старые `.asset`-файлы
    /// без групп продолжают загружаться (пустое дерево — все части
    /// верхнего уровня, поведение идентично тому, что было до этого поля).
    #[serde(default)]
    pub groups: GroupTree,
    pub skeleton: Skeleton,
    /// Морфинг хранится как дефолтное состояние; во время анимации
    /// поверх него применяются дорожки типа AnimTarget::Morph.
    pub default_morph: MorphState,
    pub animations: HashMap<String, Animation>,
    pub physics: PhysicsConfig,
    pub metadata: Metadata,
    /// Автоматический поворот (раздел 8 ТЗ), радианы, 0 = анфас. См.
    /// `orientation::apply_yaw_2_5d` — сама математика пересчёта живёт
    /// не здесь (это данные персонажа, а не поведение рендера).
    /// `#[serde(default)]` — старые `.asset`-файлы без этого поля не
    /// перестанут загружаться, просто получат анфас по умолчанию.
    #[serde(default)]
    pub facing_yaw: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ron serialize error: {0}")]
    RonSer(#[from] ron::Error),
    #[error("ron deserialize error: {0}")]
    RonDe(#[from] ron::error::SpannedError),
}

impl Character {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "0.1.0".into(),
            parts: HashMap::new(),
            groups: GroupTree::new(),
            skeleton: Skeleton::new(),
            default_morph: MorphState::default(),
            animations: HashMap::new(),
            physics: PhysicsConfig::default(),
            metadata: Metadata::default(),
            facing_yaw: 0.0,
        }
    }

    pub fn add_part(&mut self, part: Part) -> &mut Self {
        self.parts.insert(part.id.clone(), part);
        self
    }

    pub fn add_animation(&mut self, anim: Animation) -> &mut Self {
        self.animations.insert(anim.name.clone(), anim);
        self
    }

    /// Раздел 44/56 ТЗ ("Group" в меню Modify) — сгруппировать уже
    /// существующие части: создаёт новую группу с именем `name` и
    /// прописывает её как `group` у каждой части из `part_ids`
    /// (существующей группы у этих частей, если была, при этом теряется —
    /// часть не может быть одновременно в двух группах верхнего уровня,
    /// как и обычная группировка объектов в векторных редакторах).
    /// Возвращает id новой группы. Части, которых нет в `self.parts`, тихо
    /// пропускаются (не паникует на устаревшую ссылку).
    pub fn group_parts(&mut self, name: impl Into<String>, part_ids: &[impl AsRef<str>]) -> crate::group::GroupId {
        let group_id = format!("group_{}", self.groups.groups.len() + 1);
        self.groups.add(crate::group::PartGroup::new(group_id.clone(), name));
        for id in part_ids {
            if let Some(part) = self.parts.get_mut(id.as_ref()) {
                part.group = Some(group_id.clone());
            }
        }
        group_id
    }

    /// Раздел 27/44 ТЗ ("Ungroup") — удалить группу `group_id` (и все
    /// вложенные в неё, рекурсивно — см. `GroupTree::remove_subtree`),
    /// освобождая её содержимое: у всех частей, ссылавшихся на удалённые
    /// группы, `group` сбрасывается в `None` (части сами НЕ удаляются —
    /// "разгруппировать" не значит "удалить содержимое"). Части, чья
    /// группа была ВЛОЖЕНА в удаляемую (внук, не прямой член), поднимаются
    /// на верхний уровень целиком вместе со своей уже-удалённой группой —
    /// осознанное упрощение: частичный "подъём на один уровень" (как в
    /// некоторых редакторах, где Ungroup вложенной группы поднимает её
    /// содержимое в родительскую, а не сразу на самый верх) не реализован
    /// в этом проходе, при первом реальном запросе на именно такое
    /// поведение — легко добавить отдельным методом.
    pub fn ungroup(&mut self, group_id: &str) {
        let removed = self.groups.remove_subtree(group_id);
        for part in self.parts.values_mut() {
            if let Some(g) = &part.group {
                if removed.contains(g) {
                    part.group = None;
                }
            }
        }
    }

    /// Скрыта ли часть `part_id` ЭФФЕКТИВНО — либо у самой части выставлен
    /// `Part`-уровневый флаг (в этой версии модели такого флага у `Part`
    /// нет, скрытие частей реализовано на уровне GUI-state `hidden_layers`,
    /// см. `pony-gui`), либо она состоит в группе, которая скрыта (сама
    /// или любой из её родителей по цепочке, см. `GroupTree::
    /// is_hidden_including_ancestors`). Раздел 27: скрытие родительской
    /// группы должно скрывать всё вложенное дерево целиком.
    pub fn is_part_hidden_by_group(&self, part_id: &str) -> bool {
        let Some(part) = self.parts.get(part_id) else { return false };
        match &part.group {
            Some(g) => self.groups.is_hidden_including_ancestors(g),
            None => false,
        }
    }

    /// Раздел 60 ТЗ (Masks/Clipping): к какой части реально нужно применить
    /// как маску для `part_id`, если это безопасно — `None`, если у части
    /// нет `clip_by`, ссылка ведёт на несуществующую часть, часть пытается
    /// замаскировать САМА СЕБЯ, или маскирующая цепочка зациклена (A
    /// маскируется B, которая маскируется A — только ОДИН уровень маски на
    /// часть поддержан в этом проходе: маска не может сама иметь маску, это
    /// самый частый практический случай и полностью исключает возможность
    /// цикла в принципе — не нужен обход графа на каждый кадр рендера).
    pub fn resolve_clip_mask<'a>(&'a self, part_id: &str) -> Option<&'a Part> {
        let part = self.parts.get(part_id)?;
        let mask_id = part.clip_by.as_ref()?;
        if mask_id == part_id {
            return None; // часть не может маскировать сама себя
        }
        let mask = self.parts.get(mask_id)?;
        if mask.clip_by.is_some() {
            return None; // маска маски не поддержана в этом проходе — см. коммент выше
        }
        Some(mask)
    }

    /// Сохранить как `name.asset` (человекочитаемый RON).
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AssetError> {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(8);
        let s = ron::ser::to_string_pretty(self, pretty)?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AssetError> {
        let s = std::fs::read_to_string(path)?;
        let character: Character = ron::from_str(&s)?;
        Ok(character)
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::part::{PartKind, PartSource};

    fn png_part(id: &str) -> Part {
        Part::new(id, PartKind::Custom, PartSource::Png { path: String::new() })
    }

    #[test]
    fn group_parts_creates_group_and_assigns_it_to_each_part() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        c.add_part(png_part("eye_r"));
        let group_id = c.group_parts("Eyes", &["eye_l", "eye_r"]);
        assert_eq!(c.parts["eye_l"].group.as_deref(), Some(group_id.as_str()));
        assert_eq!(c.parts["eye_r"].group.as_deref(), Some(group_id.as_str()));
        assert_eq!(c.groups.find(&group_id).unwrap().name, "Eyes");
    }

    #[test]
    fn group_parts_silently_skips_unknown_part_ids() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        // "ghost" не существует — не должно паниковать.
        let group_id = c.group_parts("Eyes", &["eye_l", "ghost"]);
        assert_eq!(c.parts["eye_l"].group.as_deref(), Some(group_id.as_str()));
    }

    #[test]
    fn ungroup_clears_group_field_on_member_parts_without_deleting_them() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        c.add_part(png_part("eye_r"));
        let group_id = c.group_parts("Eyes", &["eye_l", "eye_r"]);
        c.ungroup(&group_id);
        assert!(c.parts.contains_key("eye_l"), "части не должны удаляться при Ungroup");
        assert!(c.parts.contains_key("eye_r"));
        assert_eq!(c.parts["eye_l"].group, None);
        assert_eq!(c.parts["eye_r"].group, None);
        assert!(c.groups.find(&group_id).is_none());
    }

    #[test]
    fn ungroup_of_parent_also_frees_parts_in_nested_child_group() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        let parent_id = c.group_parts("Head", &[] as &[&str]);
        let child_id = c.group_parts("Eyes", &["eye_l"]);
        c.groups.reparent(&child_id, Some(&parent_id));

        c.ungroup(&parent_id);
        assert!(c.groups.find(&parent_id).is_none());
        assert!(c.groups.find(&child_id).is_none(), "вложенная группа тоже должна быть удалена");
        assert_eq!(c.parts["eye_l"].group, None, "часть из вложенной группы тоже должна освободиться");
    }

    #[test]
    fn is_part_hidden_by_group_reflects_own_and_ancestor_hidden_flags() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        let group_id = c.group_parts("Eyes", &["eye_l"]);
        assert!(!c.is_part_hidden_by_group("eye_l"));

        c.groups.find_mut(&group_id).unwrap().hidden = true;
        assert!(c.is_part_hidden_by_group("eye_l"));
    }

    #[test]
    fn is_part_hidden_by_group_false_for_ungrouped_or_unknown_part() {
        let mut c = Character::new("test");
        c.add_part(png_part("solo"));
        assert!(!c.is_part_hidden_by_group("solo"), "часть без группы никогда не скрыта группой");
        assert!(!c.is_part_hidden_by_group("nonexistent"), "неизвестная часть — false, не паника");
    }

    #[test]
    fn resolve_clip_mask_finds_the_masking_part() {
        let mut c = Character::new("test");
        c.add_part(png_part("content"));
        c.add_part(png_part("mask_shape"));
        c.parts.get_mut("content").unwrap().clip_by = Some("mask_shape".to_string());
        let mask = c.resolve_clip_mask("content").expect("should find the mask part");
        assert_eq!(mask.id, "mask_shape");
    }

    #[test]
    fn resolve_clip_mask_none_when_no_clip_by_set() {
        let mut c = Character::new("test");
        c.add_part(png_part("content"));
        assert!(c.resolve_clip_mask("content").is_none());
    }

    #[test]
    fn resolve_clip_mask_none_for_dangling_reference() {
        let mut c = Character::new("test");
        c.add_part(png_part("content"));
        c.parts.get_mut("content").unwrap().clip_by = Some("ghost".to_string());
        assert!(c.resolve_clip_mask("content").is_none(), "ссылка на несуществующую часть — None, не паника");
    }

    #[test]
    fn resolve_clip_mask_none_for_self_reference() {
        let mut c = Character::new("test");
        c.add_part(png_part("content"));
        c.parts.get_mut("content").unwrap().clip_by = Some("content".to_string());
        assert!(c.resolve_clip_mask("content").is_none(), "часть не может маскировать сама себя");
    }

    #[test]
    fn resolve_clip_mask_none_when_mask_chain_would_cycle() {
        let mut c = Character::new("test");
        c.add_part(png_part("a"));
        c.add_part(png_part("b"));
        c.parts.get_mut("a").unwrap().clip_by = Some("b".to_string());
        c.parts.get_mut("b").unwrap().clip_by = Some("a".to_string()); // A маскируется B, B маскируется A — цикл
        assert!(c.resolve_clip_mask("a").is_none(), "цепочка масок глубже одного уровня не поддержана — безопасный отказ, не зависание");
        assert!(c.resolve_clip_mask("b").is_none());
    }

    #[test]
    fn resolve_clip_mask_none_for_unknown_part() {
        let c = Character::new("test");
        assert!(c.resolve_clip_mask("nonexistent").is_none());
    }

    #[test]
    fn character_with_groups_round_trips_through_ron() {
        let mut c = Character::new("test");
        c.add_part(png_part("eye_l"));
        let group_id = c.group_parts("Eyes", &["eye_l"]);
        c.groups.find_mut(&group_id).unwrap().hidden = true;

        let ron_text = ron::ser::to_string_pretty(&c, ron::ser::PrettyConfig::new().depth_limit(8)).expect("serialize");
        let reloaded: Character = ron::from_str(&ron_text).expect("deserialize");
        assert_eq!(reloaded.parts["eye_l"].group.as_deref(), Some(group_id.as_str()));
        assert!(reloaded.groups.find(&group_id).unwrap().hidden);
    }

    /// Удаляет поле верхнего уровня `field_name: (...)` или `field_name: [...]`
    /// из pretty-printed RON-текста, учитывая вложенные скобки (не наивный
    /// построчный фильтр — значение поля почти всегда занимает НЕСКОЛЬКО
    /// строк у `ron::ser::to_string_pretty`, простой `lines().filter(...)`
    /// снял бы только первую строку и оставил бы висящий "хвост" тела,
    /// ломающий синтаксис остального документа).
    fn remove_ron_field_by_brace_balance(text: &str, field_name: &str) -> String {
        let marker = format!("{field_name}:");
        let start = text.find(&marker).unwrap_or_else(|| panic!("field '{field_name}' not found in RON text — test assumption is stale"));
        // Ищем открывающую скобку значения поля (первую `(`/`[`/`{` после ":").
        let after_colon = &text[start + marker.len()..];
        let open_offset = after_colon.find(['(', '[', '{']).expect("field value should start with an opening bracket");
        let open_pos = start + marker.len() + open_offset;
        let open_char = text.as_bytes()[open_pos] as char;
        let close_char = match open_char {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            _ => unreachable!(),
        };
        let mut depth = 0i32;
        let mut end_pos = None;
        for (i, ch) in text[open_pos..].char_indices() {
            if ch == open_char {
                depth += 1;
            } else if ch == close_char {
                depth -= 1;
                if depth == 0 {
                    end_pos = Some(open_pos + i + ch.len_utf8());
                    break;
                }
            }
        }
        let end_pos = end_pos.expect("unbalanced brackets — test assumption is stale");
        // Дальше в тексте после значения обычно идёт `,` и перевод строки —
        // включаем и запятую, чтобы не оставить висящую лишнюю запятую
        // перед следующим полем.
        let mut real_end = end_pos;
        if text[real_end..].trim_start().starts_with(',') {
            real_end += text[real_end..].find(',').unwrap() + 1;
        }
        format!("{}{}", &text[..start], &text[real_end..])
    }

    #[test]
    fn old_asset_ron_without_groups_field_still_loads_via_serde_default() {
        // Симулирует .asset, сохранённый ДО появления поля `groups` —
        // раздел про обратную совместимость схемы (тот же приём, что и у
        // ik_constraints/unsupported/symbols в vector.rs).
        let mut c = Character::new("test");
        c.add_part(png_part("solo"));
        let ron_text = ron::ser::to_string_pretty(&c, ron::ser::PrettyConfig::new().depth_limit(8)).expect("serialize");
        let without_groups = remove_ron_field_by_brace_balance(&ron_text, "groups");
        assert!(!without_groups.contains("groups:"), "поле должно быть реально удалено, не просто закомментировано");

        let reloaded: Result<Character, _> = ron::from_str(&without_groups);
        assert!(reloaded.is_ok(), "должно грузиться даже без поля groups (старые .asset): {reloaded:?}\n---\n{without_groups}");
        assert!(reloaded.unwrap().groups.groups.is_empty(), "отсутствующее поле должно стать пустым деревом групп через #[serde(default)]");
    }
}
