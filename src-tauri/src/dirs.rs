//! A frecency store of the folders the terminals work in, like zoxide: a
//! folder used more often and more recently ranks higher, so the folder
//! palette and the `z` command can jump to it fast. It is rebuilt from what
//! the terminals report and kept in a small JSON file next to the app's data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Most folders kept; past this the lowest-ranked are dropped.
const MAX_DIRS: usize = 500;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How much a folder's age counts against its visit count, as zoxide weighs
/// it: fresh folders win, old ones fade but never vanish.
fn frecency(rank: f64, last: u64, now: u64) -> f64 {
    let age = now.saturating_sub(last);
    let factor = if age < 3_600 {
        4.0
    } else if age < 86_400 {
        2.0
    } else if age < 604_800 {
        0.5
    } else {
        0.25
    };
    rank * factor
}

/// Whether `terms` all appear in `haystack`, in order (zoxide-style matching).
/// No terms match everything, so an empty query lists the most frecent folders.
fn matches(haystack: &str, terms: &[String]) -> bool {
    let mut from = 0;
    for term in terms {
        match haystack[from..].find(term.as_str()) {
            Some(index) => from += index + term.len(),
            None => return false,
        }
    }
    true
}

fn terms_of(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// The path as a stable key: forward slashes, no trailing separator.
fn key_of(dir: &Path) -> String {
    let text = dir.to_string_lossy().replace('\\', "/");
    text.trim_end_matches('/').to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirEntry {
    rank: f64,
    /// Unix epoch seconds of the last visit.
    last: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Data {
    entries: HashMap<String, DirEntry>,
}

pub struct DirStore {
    path: PathBuf,
    data: Mutex<Data>,
    dirty: AtomicBool,
}

impl DirStore {
    /// Loads the store from `path`, or starts empty when it is missing or
    /// unreadable (a lost ranking is no loss worth failing a launch over).
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, data: Mutex::new(data), dirty: AtomicBool::new(false) }
    }

    /// Records a visit to `dir`: one more point and the time now. A folder
    /// seen for the first time past [`MAX_DIRS`] evicts the lowest-ranked one.
    pub fn add(&self, dir: &Path) {
        let key = key_of(dir);
        if key.is_empty() {
            return;
        }
        let now = now_secs();
        let mut data = self.data.lock().expect("dirs lock poisoned");
        let entry = data.entries.entry(key).or_insert(DirEntry { rank: 0.0, last: now });
        entry.rank += 1.0;
        entry.last = now;
        if data.entries.len() > MAX_DIRS {
            if let Some(worst) = data
                .entries
                .iter()
                .min_by(|a, b| {
                    frecency(a.1.rank, a.1.last, now)
                        .partial_cmp(&frecency(b.1.rank, b.1.last, now))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, _)| k.clone())
            {
                data.entries.remove(&worst);
            }
        }
        drop(data);
        self.dirty.store(true, Ordering::Release);
    }

    /// The folders matching `query`, most frecent first, that still exist on
    /// disk. An empty query lists the most frecent folders overall.
    pub fn matches(&self, query: &str, limit: usize) -> Vec<String> {
        let terms = terms_of(query);
        let now = now_secs();
        let data = self.data.lock().expect("dirs lock poisoned");
        let mut found: Vec<(&String, f64)> = data
            .entries
            .iter()
            .filter(|(key, _)| matches(&key.to_lowercase(), &terms))
            .filter(|(key, _)| Path::new(key).is_dir())
            .map(|(key, e)| (key, frecency(e.rank, e.last, now)))
            .collect();
        found.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        found.into_iter().take(limit).map(|(key, _)| key.clone()).collect()
    }

    /// The single best match for `query`, for the `z` jump (via the bridge).
    pub fn best(&self, query: &str) -> Option<String> {
        self.matches(query, 1).into_iter().next()
    }

    /// Writes the store to disk when something changed since the last write.
    pub fn save_if_dirty(&self) {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let snapshot = {
            let data = self.data.lock().expect("dirs lock poisoned");
            serde_json::to_vec(&*data)
        };
        if let Ok(bytes) = snapshot {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&self.path, bytes).is_err() {
                // Keep trying next time rather than losing later updates.
                self.dirty.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frecency_favors_recent_and_frequent() {
        let now = 1_000_000;
        // Same age, more visits wins.
        assert!(frecency(5.0, now, now) > frecency(1.0, now, now));
        // Same visits, fresher wins.
        assert!(frecency(3.0, now, now) > frecency(3.0, now - 100_000, now));
    }

    #[test]
    fn ordered_substring_matching() {
        assert!(matches("c:/users/jesus/documents/godesk", &terms_of("god")));
        assert!(matches("c:/users/jesus/documents/godesk", &terms_of("doc desk")));
        assert!(!matches("c:/users/jesus/documents/godesk", &terms_of("desk doc")), "order matters");
        assert!(matches("anything", &terms_of("")), "no terms match everything");
    }

    #[test]
    fn add_ranks_and_queries_existing_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let store = DirStore::load(tmp.path().join("dirs.json"));
        let a = tmp.path().join("alpha");
        let b = tmp.path().join("beta");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        store.add(&a);
        store.add(&b);
        store.add(&b); // beta used more

        let all = store.matches("", 10);
        assert_eq!(all.len(), 2);
        assert!(all[0].ends_with("beta"), "the more-visited folder ranks first: {all:?}");
        assert_eq!(store.best("alph").unwrap(), key_of(&a));

        // A folder that no longer exists is not offered.
        std::fs::remove_dir_all(&b).unwrap();
        let all = store.matches("", 10);
        assert_eq!(all, vec![key_of(&a)]);
    }

    #[test]
    fn saves_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("dirs.json");
        let dir = tmp.path().join("work");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let store = DirStore::load(path.clone());
            store.add(&dir);
            store.save_if_dirty();
        }
        let reloaded = DirStore::load(path);
        assert_eq!(reloaded.best("work").unwrap(), key_of(&dir));
    }
}
