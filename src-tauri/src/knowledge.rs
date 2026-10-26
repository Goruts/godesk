//! Indexing and reading of a knowledge base: the folder a `KnowledgeBase`
//! node points at. The agent never receives the folder's real path; it reaches
//! the contents only through the arrow-gated `godesk kb` commands, which call
//! in here. Each base's file list is cached briefly so a burst of commands does
//! not re-walk the tree, and extracted text (for PDFs and Office files) is
//! cached on disk so it is produced once.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// How long a built index is reused before the folder is walked again.
const INDEX_TTL: Duration = Duration::from_secs(3);
/// Largest file read or extracted, so one huge document cannot flood an agent.
pub const MAX_READ_BYTES: usize = 2 * 1024 * 1024;
/// Most entries kept for one base, to bound a pathological tree.
const MAX_ENTRIES: usize = 5000;
/// Longest matching line shown in a search hit's snippet.
const KB_SNIPPET_CHARS: usize = 200;
/// Folders never indexed: noise that is not knowledge.
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__", ".idea", ".vscode"];

/// What a file is, which decides how its text is produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Read directly as UTF-8 (markdown, code, config, csv…).
    Text,
    Pdf,
    /// A Word/PowerPoint/Excel/OpenDocument container.
    Office,
    Image,
    /// Anything else: not offered as readable.
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Pdf => "pdf",
            Kind::Office => "office",
            Kind::Image => "image",
            Kind::Other => "binary",
        }
    }

    /// The kind of a file from its extension.
    pub fn of(path: &Path) -> Kind {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "txt" | "text" | "rst" | "org" | "csv" | "tsv" | "json" | "yaml" | "yml" | "toml"
            | "ini" | "cfg" | "conf" | "log" | "env" | "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py"
            | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "rb" | "php" | "swift" | "kt" | "scala"
            | "sh" | "bash" | "zsh" | "ps1" | "bat" | "html" | "htm" | "css" | "scss" | "sass" | "xml" | "sql"
            | "vue" | "svelte" | "lua" | "pl" | "r" | "dart" | "ex" | "exs" | "clj" | "hs" | "ml" => Kind::Text,
            "pdf" => Kind::Pdf,
            "docx" | "doc" | "pptx" | "ppt" | "xlsx" | "xls" | "ods" | "odt" | "odp" => Kind::Office,
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp" => Kind::Image,
            _ => Kind::Other,
        }
    }
}

/// One file of a base, as the index knows it (metadata only; never its text).
#[derive(Clone, Debug)]
pub struct KbEntry {
    /// Path relative to the base's folder, with `/` separators.
    pub rel: String,
    pub size: u64,
    pub mtime: u64,
    pub kind: Kind,
}

/// A base's file list and when it was built.
pub struct KbIndex {
    pub entries: Vec<KbEntry>,
    built: Instant,
}

/// One matching line of a document, as `godesk kb search` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Path of the file, relative to the base, with `/` separators.
    pub rel: String,
    /// 1-based line number of the match.
    pub line: usize,
    pub snippet: String,
}

/// Caches each base's file index, the text extracted from its documents, and a
/// persistent BM25 search index (see [`crate::search_index`]).
#[derive(Clone)]
pub struct KbStore {
    indices: Arc<Mutex<HashMap<String, Arc<KbIndex>>>>,
    /// Open BM25 handles, one per base, created on first search.
    search: Arc<Mutex<HashMap<String, Arc<crate::search_index::SearchHandle>>>>,
    /// Serializes index syncs: Tantivy allows only one writer per index, and this
    /// also stops two terminals re-indexing the same base at once.
    sync_lock: Arc<Mutex<()>>,
    cache_dir: PathBuf,
}

impl KbStore {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            indices: Arc::new(Mutex::new(HashMap::new())),
            search: Arc::new(Mutex::new(HashMap::new())),
            sync_lock: Arc::new(Mutex::new(())),
            cache_dir,
        }
    }

    /// The base's index, rebuilt if it is missing or older than [`INDEX_TTL`].
    /// Walking the tree is done without the lock, so one base's scan does not
    /// block queries for another.
    pub fn index(&self, kb_id: &str, root: &Path) -> Arc<KbIndex> {
        if let Some(index) = self.indices.lock().expect("kb lock poisoned").get(kb_id) {
            if index.built.elapsed() < INDEX_TTL {
                return index.clone();
            }
        }
        let index = Arc::new(scan(root));
        self.indices.lock().expect("kb lock poisoned").insert(kb_id.to_owned(), index.clone());
        index
    }

    /// Drops the cached index of a base (e.g. when its node is removed). The
    /// persistent search index on disk is left in place; it is reopened and
    /// re-synced if the base is connected again.
    pub fn forget(&self, kb_id: &str) {
        self.indices.lock().expect("kb lock poisoned").remove(kb_id);
        self.search.lock().expect("kb search lock poisoned").remove(kb_id);
    }

    /// Full-text search over one base, ranked by BM25. The base's BM25 index is
    /// brought up to date (changed files re-indexed, removed files dropped) and
    /// then queried; the matching lines of the top files are returned. `entries`
    /// is the base's current file list (from [`Self::index`]).
    pub fn search_text(
        &self,
        kb_id: &str,
        root: &Path,
        entries: &[KbEntry],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, String> {
        self.sync(kb_id, root, entries)?;
        let handle = self.handle(kb_id)?;
        // Pull more candidate files than lines wanted, since one file can hold
        // several matching lines and the best file need not hold the most.
        let files = handle.top_files(query, limit.max(20))?;
        let words = crate::search_index::tokenize(query);
        let mut hits = Vec::new();
        for (rel, content) in files {
            for (number, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                if words.iter().any(|word| lower.contains(word.as_str())) {
                    hits.push(Hit {
                        rel: rel.clone(),
                        line: number + 1,
                        snippet: line.trim().chars().take(KB_SNIPPET_CHARS).collect(),
                    });
                    if hits.len() >= limit {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    /// Builds a base's file index and brings its BM25 search index up to date
    /// ahead of any query. Meant to run on a background thread the moment a
    /// base's node is created or connected, so the first `godesk kb search` has
    /// no indexing to do and returns at once. Best effort: a failure just means
    /// the first query does the work instead, so the error is logged, not
    /// returned. Idempotent — the manifest means an already-indexed base does no
    /// work on a repeat call.
    pub fn warm(&self, kb_id: &str, root: &Path) {
        let entries = self.index(kb_id, root).entries.clone();
        if let Err(err) = self.sync(kb_id, root, &entries) {
            eprintln!("godesk: cannot pre-index knowledge base {kb_id}: {err}");
        }
    }

    /// The open BM25 handle for a base, opened (and cached) on first use.
    fn handle(&self, kb_id: &str) -> Result<Arc<crate::search_index::SearchHandle>, String> {
        if let Some(handle) = self.search.lock().expect("kb search lock poisoned").get(kb_id) {
            return Ok(handle.clone());
        }
        let dir = self.cache_dir.join(kb_id).join("search");
        let handle = Arc::new(crate::search_index::SearchHandle::open(&dir)?);
        self.search.lock().expect("kb search lock poisoned").insert(kb_id.to_owned(), handle.clone());
        Ok(handle)
    }

    /// Brings a base's BM25 index in step with its files, using a manifest of
    /// `path -> mtime` so only new or changed files are re-extracted and only
    /// removed files are dropped. Images are not indexed (reading one runs OCR).
    fn sync(&self, kb_id: &str, root: &Path, entries: &[KbEntry]) -> Result<(), String> {
        let _guard = self.sync_lock.lock().expect("kb sync lock poisoned");
        let handle = self.handle(kb_id)?;
        let dir = self.cache_dir.join(kb_id).join("search");
        let manifest_path = dir.join("manifest.json");
        let mut manifest: HashMap<String, u64> = std::fs::read(&manifest_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        let desired: HashMap<&str, u64> = entries
            .iter()
            .filter(|entry| entry.kind != Kind::Image)
            .map(|entry| (entry.rel.as_str(), entry.mtime))
            .collect();

        let mut writer_handle = None;

        // New or changed files: re-extract and re-index.
        for (rel, mtime) in &desired {
            if manifest.get(*rel) == Some(mtime) {
                continue;
            }
            if writer_handle.is_none() {
                writer_handle = Some(handle.writer()?);
            }
            let w = writer_handle.as_ref().expect("writer just created");
            handle.delete(w, rel);
            match self.read(kb_id, root, rel) {
                Ok(text) => {
                    handle.add(w, rel, &text)?;
                    manifest.insert((*rel).to_owned(), *mtime);
                }
                // Unreadable now (e.g. an empty or broken document): keep it out
                // of the index and the manifest so it is retried next time.
                Err(_) => {
                    manifest.remove(*rel);
                }
            }
        }

        // Files gone from the base: drop them from the index.
        let removed: Vec<String> =
            manifest.keys().filter(|rel| !desired.contains_key(rel.as_str())).cloned().collect();
        for rel in &removed {
            if writer_handle.is_none() {
                writer_handle = Some(handle.writer()?);
            }
            handle.delete(writer_handle.as_ref().expect("writer just created"), rel);
            manifest.remove(rel);
        }

        if let Some(mut w) = writer_handle {
            w.commit().map_err(|e| format!("godesk: cannot update the search index: {e}"))?;
            handle.reload();
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(bytes) = serde_json::to_vec(&manifest) {
                let _ = std::fs::write(&manifest_path, bytes);
            }
        }
        Ok(())
    }

    /// The text of `rel` inside the base rooted at `root`. Text files are read
    /// directly; PDFs and Office files are extracted and cached on disk. The
    /// path is confined to the base, so `..` or an absolute path is refused.
    pub fn read(&self, kb_id: &str, root: &Path, rel: &str) -> Result<String, String> {
        let full = safe_join(root, rel)?;
        let meta = std::fs::metadata(&full).map_err(|_| format!("godesk: no file \"{rel}\" in this knowledge base"))?;
        if !meta.is_file() {
            return Err(format!("godesk: \"{rel}\" is not a file"));
        }
        let kind = Kind::of(&full);
        match kind {
            Kind::Text => read_text(&full),
            Kind::Pdf | Kind::Office | Kind::Image => {
                let mtime = mtime_of(&meta);
                self.cached_extract(kb_id, rel, &full, kind, meta.len(), mtime)
            }
            Kind::Other => Err(format!("godesk: \"{rel}\" is not a readable document")),
        }
    }

    /// Extracted text for a non-text document, produced once and cached under
    /// the base's folder in the app data dir, keyed by the file's size+mtime so
    /// an edit re-extracts it.
    fn cached_extract(
        &self,
        kb_id: &str,
        rel: &str,
        full: &Path,
        kind: Kind,
        size: u64,
        mtime: u64,
    ) -> Result<String, String> {
        let dir = self.cache_dir.join(kb_id);
        let key = format!("{:x}-{size}-{mtime}", stable_hash(rel));
        let cache = dir.join(format!("{key}.txt"));
        if let Ok(text) = std::fs::read_to_string(&cache) {
            return Ok(text);
        }
        let text = crate::extract::extract(full, kind)?;
        let text = cap(text);
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(&cache, &text);
        }
        Ok(text)
    }
}

/// Reads a text file as UTF-8 (lossily), capped to [`MAX_READ_BYTES`].
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("godesk: cannot read the file: {e}"))?;
    let cut = bytes.len().min(MAX_READ_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
    if bytes.len() > MAX_READ_BYTES {
        text.push_str("\n…[truncated]");
    }
    Ok(text)
}

/// Shortens extracted text to the read limit.
pub fn cap(mut text: String) -> String {
    if text.len() > MAX_READ_BYTES {
        text.truncate(MAX_READ_BYTES);
        text.push_str("\n…[truncated]");
    }
    text
}

/// Joins `rel` onto `root`, refusing anything that would leave the folder:
/// an absolute path, a `..`, or (after resolving) a target outside the base.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().replace('\\', "/");
    let rel_path = Path::new(&rel);
    if rel_path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("godesk: the file must be inside the knowledge base".into());
    }
    let full = root.join(rel_path);
    // Confirm the resolved target really sits under the base (guards symlinks).
    match (full.canonicalize(), root.canonicalize()) {
        (Ok(full), Ok(root)) if full.starts_with(&root) => Ok(full),
        (Ok(_), Ok(_)) => Err("godesk: the file is outside the knowledge base".into()),
        _ => Err("godesk: the file is outside the knowledge base".into()),
    }
}

/// Walks `root`, collecting readable files with their metadata. Hidden entries
/// and well-known noise folders are skipped; the walk stops at [`MAX_ENTRIES`].
fn scan(root: &Path) -> KbIndex {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for item in read.flatten() {
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = item.file_type() else { continue };
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let kind = Kind::of(&path);
            if kind == Kind::Other {
                continue; // Not readable: leave it out of the index.
            }
            let Ok(meta) = item.metadata() else { continue };
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            entries.push(KbEntry { rel, size: meta.len(), mtime: mtime_of(&meta), kind });
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    KbIndex { entries, built: Instant::now() }
}

fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// A small stable hash of a string, for cache file names (not security).
fn stable_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Human-readable byte size, for the index listing.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{size:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_text_files_and_skips_noise() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "hello").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/notes.txt"), "world").unwrap();
        std::fs::write(dir.path().join("image.bin"), [0u8, 1, 2]).unwrap();

        let store = KbStore::new(dir.path().join("cache"));
        let index = store.index("kb1", dir.path());
        let rels: Vec<&str> = index.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["readme.md", "sub/notes.txt"], "hidden, noise and binary left out");
    }

    #[test]
    fn reads_a_text_file_and_refuses_escapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "content").unwrap();
        let store = KbStore::new(dir.path().join("cache"));
        assert_eq!(store.read("kb1", dir.path(), "a.txt").unwrap(), "content");
        assert!(store.read("kb1", dir.path(), "../secret").is_err());
        assert!(store.read("kb1", dir.path(), "/etc/passwd").is_err());
        assert!(store.read("kb1", dir.path(), "missing.txt").is_err());
    }

    #[test]
    fn search_finds_lines_and_drops_removed_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "intro line\nthe quick brown fox\nbye").unwrap();
        std::fs::write(dir.path().join("b.md"), "nothing relevant here").unwrap();
        let store = KbStore::new(dir.path().join("cache"));

        let entries = store.index("kb1", dir.path()).entries.clone();
        let hits = store.search_text("kb1", dir.path(), &entries, "fox", 10).unwrap();
        assert_eq!(hits.len(), 1, "one line matches \"fox\"");
        assert_eq!(hits[0].rel, "a.md");
        assert_eq!(hits[0].line, 2, "the fox is on the second line");

        // Remove a.md and re-sync with the new file list: it must drop out.
        std::fs::remove_file(dir.path().join("a.md")).unwrap();
        let after = vec![KbEntry { rel: "b.md".into(), size: 0, mtime: 0, kind: Kind::Text }];
        let gone = store.search_text("kb1", dir.path(), &after, "fox", 10).unwrap();
        assert!(gone.is_empty(), "the removed file is no longer indexed");
    }

    #[test]
    fn human_size_reads_well() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
