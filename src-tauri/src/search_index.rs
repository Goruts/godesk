//! A persistent BM25 full-text index for one knowledge base, so `godesk kb
//! search` picks candidate files from an index instead of re-reading and
//! scanning every document on every query. One Tantivy index lives per base
//! under the base's cache folder; [`crate::knowledge::KbStore`] keeps a handle
//! open and syncs it incrementally (see there). This module is the low-level
//! index: open, add/delete a document, commit, and query. It never touches the
//! base's real folder — the caller supplies already-extracted text.

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, TantivyDocument, Value, STORED, STRING, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term};

/// Memory budget for an index writer. One commit's worth of documents is held
/// here before it is flushed; 15 MB is ample for a knowledge base's text.
const WRITER_HEAP: usize = 15_000_000;

/// An open BM25 index for one base: the Tantivy index, a reader, and the two
/// fields — the file's relative path (stored, used as the delete key) and its
/// full extracted text (indexed for BM25 and stored for snippet extraction).
pub struct SearchHandle {
    index: Index,
    reader: IndexReader,
    path_field: Field,
    content_field: Field,
}

impl SearchHandle {
    /// Opens the index under `dir`, creating it (and the folder) the first time.
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("godesk: cannot create the search index: {e}"))?;
        let mut builder = Schema::builder();
        let path_field = builder.add_text_field("path", STRING | STORED);
        let content_field = builder.add_text_field("content", TEXT | STORED);
        let schema = builder.build();
        let mmap = MmapDirectory::open(dir).map_err(|e| format!("godesk: cannot open the search index: {e}"))?;
        let index =
            Index::open_or_create(mmap, schema).map_err(|e| format!("godesk: cannot open the search index: {e}"))?;
        // Reloaded explicitly after each commit, so a reader never reflects a
        // half-written index.
        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| format!("godesk: cannot open the search index: {e}"))?;
        Ok(Self { index, reader, path_field, content_field })
    }

    /// A writer for a batch of changes. Tantivy allows only one at a time, so the
    /// caller holds it for the whole sync and commits once.
    pub fn writer(&self) -> Result<IndexWriter, String> {
        self.index.writer(WRITER_HEAP).map_err(|e| format!("godesk: cannot write the search index: {e}"))
    }

    /// Removes any document previously indexed for `rel` (a no-op if none). Used
    /// before re-adding a changed file and when a file is gone.
    pub fn delete(&self, writer: &IndexWriter, rel: &str) {
        writer.delete_term(Term::from_field_text(self.path_field, rel));
    }

    /// Indexes `rel` with its extracted `content`.
    pub fn add(&self, writer: &IndexWriter, rel: &str, content: &str) -> Result<(), String> {
        let mut doc = TantivyDocument::new();
        doc.add_text(self.path_field, rel);
        doc.add_text(self.content_field, content);
        writer.add_document(doc).map_err(|e| format!("godesk: cannot index a document: {e}"))?;
        Ok(())
    }

    /// Makes the latest commit visible to searches. Called after a sync commits.
    pub fn reload(&self) {
        let _ = self.reader.reload();
    }

    /// The stored `(rel, content)` of the top files for `query`, ranked by BM25.
    /// The query is tokenized into words and matched as an OR of term queries, so
    /// a user phrase never trips Tantivy's query syntax. The caller pulls the
    /// matching lines out of each returned document's text.
    pub fn top_files(&self, query: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
        let words = tokenize(query);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let clauses: Vec<(Occur, Box<dyn Query>)> = words
            .iter()
            .map(|word| {
                let term = Term::from_field_text(self.content_field, word);
                (Occur::Should, Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)) as Box<dyn Query>)
            })
            .collect();
        let query = BooleanQuery::new(clauses);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| format!("godesk: search failed: {e}"))?;
        let mut out = Vec::with_capacity(top.len());
        for (_score, address) in top {
            let doc: TantivyDocument =
                searcher.doc(address).map_err(|e| format!("godesk: search failed: {e}"))?;
            let rel = doc.get_first(self.path_field).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            let content = doc.get_first(self.content_field).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            if !rel.is_empty() {
                out.push((rel, content));
            }
        }
        Ok(out)
    }
}

/// Splits text into lowercased alphanumeric words — the same shape Tantivy's
/// default tokenizer produces, so query words match indexed terms.
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_and_ranks_by_relevance() {
        let dir = tempfile::tempdir().unwrap();
        let handle = SearchHandle::open(dir.path()).unwrap();
        {
            let mut writer = handle.writer().unwrap();
            handle.add(&writer, "a.md", "the quick brown fox jumps").unwrap();
            handle.add(&writer, "b.md", "a lazy dog sleeps all day").unwrap();
            writer.commit().unwrap();
        }
        handle.reload();
        let hits = handle.top_files("fox", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "a.md");
    }

    #[test]
    fn delete_removes_a_file_and_query_syntax_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let handle = SearchHandle::open(dir.path()).unwrap();
        {
            let mut writer = handle.writer().unwrap();
            handle.add(&writer, "a.md", "alpha beta gamma").unwrap();
            writer.commit().unwrap();
        }
        handle.reload();
        // A query full of operator characters must not error.
        assert!(handle.top_files("alpha AND (beta OR", 10).unwrap().len() == 1);
        {
            let mut writer = handle.writer().unwrap();
            handle.delete(&writer, "a.md");
            writer.commit().unwrap();
        }
        handle.reload();
        assert!(handle.top_files("alpha", 10).unwrap().is_empty());
    }
}
