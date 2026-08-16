//! Groups (раздел 27 ТЗ): "SVG `<g>` должен соответствовать группе Scene
//! Graph... группа может содержать paths/shapes/nested groups/symbols/
//! images... `g` является контейнером и может содержать другие `g` на
//! произвольной глубине".
//!
//! Задача 5 списка "Adobe Animate": "Groups как иерархия поверх костей" —
//! ключевое слово "поверх": группа — это ОРГАНИЗАЦИОННАЯ структура для
//! `Part`ов (как папка слоёв в Timeline Animate — выделить/скрыть/
//! переместить сразу несколько частей как единое целое), полностью
//! ОТДЕЛЬНАЯ от `Skeleton`/`Bone` иерархии, которая управляет тем, как
//! части ДЕФОРМИРУЮТСЯ при анимации. Одна и та же группа частей ("Голова")
//! может состоять из частей, прикреплённых к разным костям (глаза к
//! `Head`, уши к `Head`, чёлка к `ManeFront`) — группировка касается
//! организации СЦЕНЫ (что редактор считает "одним объектом" при
//! выделении/Group/Ungroup), не влияет на то, как считается world
//! transform (для этого по-прежнему используется `Skeleton`).

use serde::{Deserialize, Serialize};

pub type GroupId = String;

/// Одна группа (раздел 27 ТЗ, `<g>`) — именованный узел дерева групп.
/// Сама по себе НЕ хранит список частей внутри себя (в отличие от
/// `Skeleton::Bone`, который тоже не хранит детей у себя — тот же приём:
/// принадлежность идёт от ребёнка к родителю, не наоборот, см. `Part::group`
/// и `PartGroup::parent`), чтобы не пришлось синхронизировать два списка
/// (member ids здесь и `group` у `Part`) при каждой правке.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartGroup {
    pub id: GroupId,
    pub name: String,
    /// `None` — группа верхнего уровня. `Some(id)` — вложена в другую
    /// группу (раздел 27: "g является контейнером и может содержать
    /// другие g на произвольной глубине").
    pub parent: Option<GroupId>,
    /// Свёрнута ли группа в панели слоёв — чисто UI-состояние, но хранится
    /// в модели (не в GUI), т.к. должно переживать сохранение/загрузку
    /// `.asset`, как и остальной вид Timeline (та же логика, что у
    /// `Part::layer`).
    #[serde(default)]
    pub collapsed: bool,
    /// Скрыта ли вся группа целиком (раздел 44: "Layers panel" — Hide) —
    /// НЕ то же самое, что скрытие отдельных частей внутри неё; рендер
    /// части должен учитывать оба флага (своей группы и всех родительских
    /// групп по цепочке — см. `Character::is_part_effectively_hidden`).
    #[serde(default)]
    pub hidden: bool,
}

impl PartGroup {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), parent: None, collapsed: false, hidden: false }
    }

    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }
}

/// Контейнер дерева групп — используется как поле `Character::groups`
/// (`Vec<PartGroup>` было бы достаточно для хранения, но операции
/// reparent/remove/rename с защитой от циклов повторяли бы `Skeleton`
/// почти дословно, так что вынесены в собственный тип с той же формой
/// API, что и у `Skeleton` — тот же паттерн, знакомый по костям).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupTree {
    pub groups: Vec<PartGroup>,
}

impl GroupTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn find(&self, id: &str) -> Option<&PartGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut PartGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn add(&mut self, group: PartGroup) {
        self.groups.push(group);
    }

    /// Является ли `id` потомком `ancestor_id` (или самой этой группой) —
    /// защита от цикла при `reparent`, дословно тот же алгоритм, что
    /// `Skeleton::is_descendant_of`.
    pub fn is_descendant_of(&self, id: &str, ancestor_id: &str) -> bool {
        let mut current = Some(id.to_string());
        while let Some(cur) = current {
            if cur == ancestor_id {
                return true;
            }
            current = self.find(&cur).and_then(|g| g.parent.clone());
        }
        false
    }

    /// Переродить группу на нового родителя (или сделать верхнеуровневой,
    /// если `new_parent` — `None`). Отказывает при попытке создать цикл.
    pub fn reparent(&mut self, id: &str, new_parent: Option<&str>) -> bool {
        if let Some(np) = new_parent {
            if id == np || self.find(np).is_none() || self.is_descendant_of(np, id) {
                return false;
            }
        }
        match self.find_mut(id) {
            Some(group) => {
                group.parent = new_parent.map(|s| s.to_string());
                true
            }
            None => false,
        }
    }

    /// Удалить группу и все вложенные (рекурсивно) — возвращает id всех
    /// удалённых групп, чтобы вызывающая сторона (`Character::remove_
    /// group`) знала, у каких `Part`ов нужно сбросить `group` в `None`
    /// (сами части НЕ удаляются вместе с группой — раздел 27/44:
    /// "Ungroup"/удаление группы освобождает содержимое, не уничтожает
    /// его, это отдельная операция от удаления частей).
    pub fn remove_subtree(&mut self, id: &str) -> Vec<GroupId> {
        let mut to_remove = vec![id.to_string()];
        let mut i = 0;
        while i < to_remove.len() {
            let current = to_remove[i].clone();
            for g in &self.groups {
                if g.parent.as_deref() == Some(current.as_str()) {
                    to_remove.push(g.id.clone());
                }
            }
            i += 1;
        }
        self.groups.retain(|g| !to_remove.contains(&g.id));
        to_remove
    }

    /// Переименовать группу: свой id и ссылки `parent` у прямых детей.
    pub fn rename(&mut self, old_id: &str, new_id: &str) -> bool {
        if old_id == new_id || self.find(new_id).is_some() || self.find(old_id).is_none() {
            return false;
        }
        for g in &mut self.groups {
            if g.id == old_id {
                g.id = new_id.to_string();
            }
            if g.parent.as_deref() == Some(old_id) {
                g.parent = Some(new_id.to_string());
            }
        }
        true
    }

    /// Скрыта ли группа `id` целиком — учитывает ВСЮ цепочку родителей
    /// (раздел 27: вложенные группы произвольной глубины), не только сам
    /// флаг `hidden` этой группы: скрытая родительская группа скрывает
    /// всё вложенное дерево, как в любом редакторе со слоями-папками.
    pub fn is_hidden_including_ancestors(&self, id: &str) -> bool {
        let mut current = Some(id.to_string());
        while let Some(cur) = current {
            match self.find(&cur) {
                Some(g) if g.hidden => return true,
                Some(g) => current = g.parent.clone(),
                None => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_find_group() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("head_group", "Голова"));
        assert!(tree.find("head_group").is_some());
        assert!(tree.find("nonexistent").is_none());
    }

    #[test]
    fn is_descendant_of_walks_up_the_chain() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B").with_parent("a"));
        tree.add(PartGroup::new("c", "C").with_parent("b"));
        assert!(tree.is_descendant_of("c", "a"));
        assert!(tree.is_descendant_of("c", "b"));
        assert!(tree.is_descendant_of("c", "c")); // сама себе потомок — граничный случай, как у Skeleton
        assert!(!tree.is_descendant_of("a", "c"));
    }

    #[test]
    fn reparent_rejects_cycles() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B").with_parent("a"));
        assert!(!tree.reparent("a", Some("b")), "нельзя сделать родителя потомком своего же потомка");
        assert!(!tree.reparent("a", Some("a")), "нельзя быть своим собственным родителем");
        assert_eq!(tree.find("a").unwrap().parent, None, "неудачный reparent не должен менять состояние");
    }

    #[test]
    fn reparent_to_none_makes_top_level() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B").with_parent("a"));
        assert!(tree.reparent("b", None));
        assert_eq!(tree.find("b").unwrap().parent, None);
    }

    #[test]
    fn remove_subtree_removes_group_and_all_nested() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B").with_parent("a"));
        tree.add(PartGroup::new("c", "C").with_parent("b"));
        tree.add(PartGroup::new("sibling", "Sibling")); // не должна быть затронута
        let removed = tree.remove_subtree("a");
        assert_eq!(removed.len(), 3);
        assert!(tree.find("a").is_none() && tree.find("b").is_none() && tree.find("c").is_none());
        assert!(tree.find("sibling").is_some());
    }

    #[test]
    fn rename_updates_own_id_and_childrens_parent_refs() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B").with_parent("a"));
        assert!(tree.rename("a", "a2"));
        assert!(tree.find("a").is_none());
        assert_eq!(tree.find("a2").unwrap().name, "A");
        assert_eq!(tree.find("b").unwrap().parent.as_deref(), Some("a2"));
    }

    #[test]
    fn rename_rejects_collision_and_missing_source() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        tree.add(PartGroup::new("b", "B"));
        assert!(!tree.rename("a", "b"), "имя занято");
        assert!(!tree.rename("nonexistent", "c"), "исходной группы нет");
    }

    #[test]
    fn hidden_ancestor_hides_the_whole_subtree() {
        let mut tree = GroupTree::new();
        tree.add(PartGroup::new("a", "A"));
        let mut b = PartGroup::new("b", "B").with_parent("a");
        b.hidden = false;
        tree.add(b);
        assert!(!tree.is_hidden_including_ancestors("b"));

        tree.find_mut("a").unwrap().hidden = true;
        assert!(tree.is_hidden_including_ancestors("b"), "скрытая родительская группа должна скрывать вложенную");
        assert!(tree.is_hidden_including_ancestors("a"));
    }

    #[test]
    fn is_hidden_on_unknown_group_is_false_not_panic() {
        let tree = GroupTree::new();
        assert!(!tree.is_hidden_including_ancestors("nope"));
    }
}
