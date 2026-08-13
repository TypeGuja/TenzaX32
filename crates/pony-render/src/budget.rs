//! LRU-бюджет памяти для `TextureCache` (см. `pony_system::WorkloadPolicy::
//! memory_budget_bytes`, который считает бюджет — этот модуль его
//! соблюдает). Раньше `TextureCache` рос неограниченно; теперь при
//! превышении бюджета вытесняются наименее недавно использованные
//! текстуры (LRU), пока сумма не впишется в лимит.
//!
//! Разделено намеренно: сама бухгалтерия (какие ключи вытеснить) — чистая
//! структура без GPU, полностью тестируемая; `TextureCache` (в `texture.rs`)
//! использует её, чтобы решить, какие реальные `wgpu::Texture` удалить.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct LruBudget {
    /// От наименее до наиболее недавно использованного. Простой Vec, не
    /// связный список/индексированная куча — для реалистичного числа
    /// одновременно загруженных текстур (десятки-сотни, не миллионы)
    /// линейный поиск при `touch` дешевле, чем кажется, и код проще.
    order: Vec<String>,
    sizes: HashMap<String, u64>,
    total: u64,
    budget: u64,
}

impl LruBudget {
    pub fn new(budget_bytes: u64) -> Self {
        Self { budget: budget_bytes, ..Default::default() }
    }

    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget
    }

    pub fn contains(&self, key: &str) -> bool {
        self.sizes.contains_key(key)
    }

    /// Отметить ключ как только что использованный — двигает его в конец
    /// порядка (самый "свежий", вытесняется последним).
    pub fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos);
            self.order.push(k);
        }
    }

    /// Добавить новый ключ с размером в байтах (обычно `width*height*4`
    /// для RGBA8). Если ключ уже был — сначала убирает старую запись (на
    /// случай, если размер текстуры под тем же путём изменился).
    pub fn insert(&mut self, key: String, size_bytes: u64) {
        self.remove(&key);
        self.total += size_bytes;
        self.sizes.insert(key.clone(), size_bytes);
        self.order.push(key);
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(size) = self.sizes.remove(key) {
            self.total -= size;
            self.order.retain(|k| k != key);
        }
    }

    /// Вытеснить наименее недавно использованные ключи, пока `total` не
    /// впишется в `budget`. Возвращает вытесненные ключи — вызывающая
    /// сторона (`TextureCache`) должна удалить по ним реальные текстуры.
    /// Никогда не вытесняет ПОСЛЕДНИЙ оставшийся ключ, даже если он один
    /// превышает весь бюджет — единственная огромная текстура не должна
    /// уйти в бесконечный цикл "загрузить-вытеснить-загрузить" каждый кадр.
    pub fn evict_to_fit(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.total > self.budget && self.order.len() > 1 {
            let victim = self.order.remove(0);
            if let Some(size) = self.sizes.remove(&victim) {
                self.total -= size;
            }
            evicted.push(victim);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_under_budget_evicts_nothing() {
        let mut lru = LruBudget::new(1000);
        lru.insert("a".into(), 300);
        lru.insert("b".into(), 300);
        assert_eq!(lru.evict_to_fit(), Vec::<String>::new());
        assert_eq!(lru.total_bytes(), 600);
    }

    #[test]
    fn evicts_oldest_first_when_over_budget() {
        let mut lru = LruBudget::new(500);
        lru.insert("a".into(), 300);
        lru.insert("b".into(), 300); // total=600 > 500
        let evicted = lru.evict_to_fit();
        assert_eq!(evicted, vec!["a".to_string()], "должен вытеснить самый старый (a), не b");
        assert_eq!(lru.total_bytes(), 300);
        assert!(!lru.contains("a"));
        assert!(lru.contains("b"));
    }

    #[test]
    fn touch_protects_recently_used_key_from_eviction() {
        let mut lru = LruBudget::new(500);
        lru.insert("a".into(), 300);
        lru.insert("b".into(), 300);
        lru.touch("a"); // "a" теперь самый свежий, "b" — самый старый
        let evicted = lru.evict_to_fit();
        assert_eq!(evicted, vec!["b".to_string()], "после touch('a') вытеснить должны b, не a");
        assert!(lru.contains("a"));
    }

    #[test]
    fn never_evicts_the_last_remaining_key_even_over_budget() {
        let mut lru = LruBudget::new(100);
        lru.insert("huge".into(), 10_000); // одна текстура больше всего бюджета
        let evicted = lru.evict_to_fit();
        assert!(evicted.is_empty(), "единственный ключ не должен вытесняться сам из себя");
        assert!(lru.contains("huge"));
    }

    #[test]
    fn evicts_multiple_keys_if_needed_to_fit() {
        let mut lru = LruBudget::new(150);
        lru.insert("a".into(), 100);
        lru.insert("b".into(), 100);
        lru.insert("c".into(), 100); // total=300 > 150, вытеснить нужно и a, и b (100 всё ещё > 150? нет — 100<=150 после вытеснения обоих)
        let evicted = lru.evict_to_fit();
        assert_eq!(evicted, vec!["a".to_string(), "b".to_string()], "должен вытеснить и a, и b, чтобы влезть в бюджет 150");
        assert_eq!(lru.total_bytes(), 100);
    }

    #[test]
    fn reinserting_existing_key_updates_size_without_duplicating() {
        let mut lru = LruBudget::new(1000);
        lru.insert("a".into(), 100);
        lru.insert("a".into(), 400); // тот же путь, но текстура оказалась больше
        assert_eq!(lru.total_bytes(), 400, "не должно суммировать 100+400");
        assert_eq!(lru.order.len(), 1);
    }
}
