use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) const DEFAULT_GPU_TEXTURE_CAPACITY: usize = 512;

#[derive(Debug)]
pub(super) struct TextureLru {
    capacity: usize,
    clock: u64,
    last_used: HashMap<PathBuf, u64>,
}

impl TextureLru {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            clock: 0,
            last_used: HashMap::new(),
        }
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(super) fn register(&mut self, path: &Path) {
        self.touch(path);
    }

    pub(super) fn touch(&mut self, path: &Path) {
        self.clock = self.clock.saturating_add(1);
        self.last_used.insert(path.to_path_buf(), self.clock);
    }

    pub(super) fn remove(&mut self, path: &Path) {
        self.last_used.remove(path);
    }

    pub(super) fn clear(&mut self) {
        self.last_used.clear();
    }

    pub(super) fn eviction_victims(
        &mut self,
        residents: &[PathBuf],
        protected: &HashSet<PathBuf>,
    ) -> Vec<PathBuf> {
        let resident_set: HashSet<&PathBuf> = residents.iter().collect();
        self.last_used
            .retain(|path, _| resident_set.contains(path));
        for path in residents {
            self.last_used.entry(path.clone()).or_insert(0);
        }

        let over_capacity = residents.len().saturating_sub(self.capacity);
        if over_capacity == 0 {
            return Vec::new();
        }

        let mut candidates: Vec<(u64, PathBuf)> = residents
            .iter()
            .filter(|path| !protected.contains(*path))
            .map(|path| (*self.last_used.get(path).unwrap_or(&0), path.clone()))
            .collect();
        candidates.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let victims: Vec<PathBuf> = candidates
            .into_iter()
            .take(over_capacity)
            .map(|(_, path)| path)
            .collect();
        for path in &victims {
            self.last_used.remove(path);
        }
        victims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn least_recently_used_texture_is_evicted_first() {
        let mut lru = TextureLru::new(2);
        let resident = paths(&["a.jpg", "b.jpg", "c.jpg"]);
        for path in &resident {
            lru.register(path);
        }

        let victims = lru.eviction_victims(&resident, &HashSet::new());
        assert_eq!(victims, vec![PathBuf::from("a.jpg")]);
    }

    #[test]
    fn touching_texture_refreshes_its_lru_position() {
        let mut lru = TextureLru::new(2);
        let resident = paths(&["a.jpg", "b.jpg", "c.jpg"]);
        lru.register(&resident[0]);
        lru.register(&resident[1]);
        lru.touch(&resident[0]);
        lru.register(&resident[2]);

        let victims = lru.eviction_victims(&resident, &HashSet::new());
        assert_eq!(victims, vec![PathBuf::from("b.jpg")]);
    }

    #[test]
    fn protected_query_texture_is_kept_when_possible() {
        let mut lru = TextureLru::new(2);
        let resident = paths(&["query.jpg", "old.jpg", "new.jpg"]);
        for path in &resident {
            lru.register(path);
        }
        let protected = HashSet::from([PathBuf::from("query.jpg")]);

        let victims = lru.eviction_victims(&resident, &protected);
        assert_eq!(victims, vec![PathBuf::from("old.jpg")]);
        assert!(!victims.contains(&PathBuf::from("query.jpg")));
    }

    #[test]
    fn no_eviction_occurs_at_or_below_capacity() {
        let mut lru = TextureLru::new(3);
        let resident = paths(&["a.jpg", "b.jpg", "c.jpg"]);
        for path in &resident {
            lru.register(path);
        }

        assert!(lru
            .eviction_victims(&resident, &HashSet::new())
            .is_empty());
    }
}
