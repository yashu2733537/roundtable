//! SQLite knowledge store for RAG.
//!
//! Tables: `settings` (embedder config, non-secret only), `documents` and
//! `chunks` (chunk text + embedding BLOB). Chunks are matched by embedder tag
//! so vectors from different backends/models never mix in one search.
//!
//! This module does no networking: callers embed chunk texts first (see
//! [`crate::embed`]) and hand the vectors in.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::embed::{blob_to_vec, cosine, vec_to_blob};
use crate::{Chunk, RagError};

/// One indexed document, as listed in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct KbDocument {
    pub id: String,
    pub name: String,
    pub origin: String,
    pub chars: usize,
    pub chunks: usize,
    pub embedder: String,
    pub created_at: i64,
}

/// One retrieval hit, best match first.
#[derive(Debug, Clone, Serialize)]
pub struct KbHit {
    pub document_id: String,
    pub document_name: String,
    pub seq: usize,
    pub text: String,
    pub score: f32,
}

fn db_err(e: rusqlite::Error) -> RagError {
    RagError::Db(e.to_string())
}

/// Milliseconds since the Unix epoch. Crate-internal clock for rows.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct KbStore {
    pub(crate) conn: Connection,
}

impl KbStore {
    /// Open (creating) the database at `path`, running migrations.
    pub fn open(path: &Path) -> Result<Self, RagError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    RagError::Db(format!("Could not create the data directory: {e}"))
                })?;
            }
        }
        let conn = Connection::open(path)
            .map_err(|e| RagError::Db(format!("Could not open the database: {e}")))?;
        let store = Self { conn };
        // WAL keeps reads concurrent with writes; foreign keys make document
        // deletion cascade to chunks.
        let _: String = store
            .conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(db_err)?;
        store
            .conn
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(db_err)?;
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), RagError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_err)?;
        if version < 1 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS settings (
                        key TEXT PRIMARY KEY,
                        value TEXT NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS documents (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        origin TEXT NOT NULL,
                        chars INTEGER NOT NULL,
                        chunks INTEGER NOT NULL,
                        embedder TEXT NOT NULL,
                        created_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS chunks (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                        seq INTEGER NOT NULL,
                        text TEXT NOT NULL,
                        embedding BLOB NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);
                    PRAGMA user_version = 1;",
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    // -- settings ----------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, RagError> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_err)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), RagError> {
        self.conn
            .execute(
                "INSERT INTO settings(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    // -- documents ----------------------------------------------------------

    /// Store a document with its pre-computed chunk embeddings. The id is
    /// generated here so callers never have to.
    pub fn add_document(
        &self,
        name: &str,
        origin: &str,
        chars: usize,
        chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        embedder_tag: &str,
    ) -> Result<KbDocument, RagError> {
        if chunks.is_empty() {
            return Err(RagError::Input(
                "Nothing to index — the document has no content.".to_string(),
            ));
        }
        if chunks.len() != embeddings.len() {
            return Err(RagError::Input(format!(
                "Embedding count ({}) does not match chunk count ({}).",
                embeddings.len(),
                chunks.len()
            )));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_ms();
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        tx.execute(
            "INSERT INTO documents(id, name, origin, chars, chunks, embedder, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                origin,
                chars as i64,
                chunks.len() as i64,
                embedder_tag,
                created_at
            ],
        )
        .map_err(db_err)?;
        for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
            tx.execute(
                "INSERT INTO chunks(document_id, seq, text, embedding)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, chunk.seq as i64, chunk.text, vec_to_blob(embedding)],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(KbDocument {
            id,
            name: name.to_string(),
            origin: origin.to_string(),
            chars,
            chunks: chunks.len(),
            embedder: embedder_tag.to_string(),
            created_at,
        })
    }

    pub fn document(&self, id: &str) -> Result<Option<KbDocument>, RagError> {
        self.conn
            .query_row(
                "SELECT id, name, origin, chars, chunks, embedder, created_at
                 FROM documents WHERE id = ?1",
                [id],
                row_to_document,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn list_documents(&self) -> Result<Vec<KbDocument>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, origin, chars, chunks, embedder, created_at
                 FROM documents ORDER BY created_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_document).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn delete_document(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM documents WHERE id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    /// Chunk texts of one document, in order — the inputs for re-embedding.
    pub fn chunk_texts(&self, document_id: &str) -> Result<Vec<String>, RagError> {
        let mut stmt = self
            .conn
            .prepare("SELECT text FROM chunks WHERE document_id = ?1 ORDER BY seq")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([document_id], |row| row.get::<_, String>(0))
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Replace a document's chunk vectors after the embedder changed, keeping
    /// the chunk texts. Returns the updated document row.
    pub fn reindex_document(
        &self,
        id: &str,
        embeddings: &[Vec<f32>],
        embedder_tag: &str,
    ) -> Result<KbDocument, RagError> {
        let doc = self.document(id)?.ok_or_else(|| {
            RagError::Input("That document is gone — it may have been deleted.".to_string())
        })?;
        let chunk_ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM chunks WHERE document_id = ?1 ORDER BY seq")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([id], |row| row.get::<_, i64>(0))
                .map_err(db_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
        };
        if chunk_ids.len() != embeddings.len() {
            return Err(RagError::Input(format!(
                "Embedding count ({}) does not match chunk count ({}).",
                embeddings.len(),
                chunk_ids.len()
            )));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        for (chunk_id, embedding) in chunk_ids.iter().zip(embeddings.iter()) {
            tx.execute(
                "UPDATE chunks SET embedding = ?1 WHERE id = ?2",
                params![vec_to_blob(embedding), chunk_id],
            )
            .map_err(db_err)?;
        }
        tx.execute(
            "UPDATE documents SET embedder = ?1 WHERE id = ?2",
            params![embedder_tag, id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(KbDocument {
            embedder: embedder_tag.to_string(),
            ..doc
        })
    }

    // -- search --------------------------------------------------------------

    /// Top-`k` chunks for `query`, restricted to one embedder tag so vector
    /// spaces never mix. Chunks whose stored width differs from the query are
    /// skipped (stale vectors from a changed model score 0 by construction —
    /// here we drop them outright).
    pub fn search(
        &self,
        query: &[f32],
        top_k: usize,
        embedder_tag: &str,
    ) -> Result<Vec<KbHit>, RagError> {
        let top_k = top_k.clamp(1, 50);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.document_id, d.name, c.seq, c.text, c.embedding
                 FROM chunks c JOIN documents d ON d.id = c.document_id
                 WHERE d.embedder = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([embedder_tag], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(db_err)?;

        let mut hits = Vec::new();
        for row in rows {
            let (document_id, document_name, seq, text, blob) = row.map_err(db_err)?;
            let vec = blob_to_vec(&blob)?;
            if vec.len() != query.len() {
                continue;
            }
            hits.push(KbHit {
                document_id,
                document_name,
                seq: seq as usize,
                text,
                score: cosine(query, &vec),
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    }
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<KbDocument> {
    Ok(KbDocument {
        id: row.get(0)?,
        name: row.get(1)?,
        origin: row.get(2)?,
        chars: row.get::<_, i64>(3)? as usize,
        chunks: row.get::<_, i64>(4)? as usize,
        embedder: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{chunk_text, ChunkOptions};
    use crate::embed::embed_local;

    fn temp_db(name: &str) -> (std::path::PathBuf, KbStore) {
        let path = std::env::temp_dir().join(format!(
            "roundtable-kb-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let store = KbStore::open(&path).unwrap();
        (path, store)
    }

    fn cleanup(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let p = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn settings_roundtrip() {
        let (path, store) = temp_db("settings");
        assert_eq!(store.get_setting("k").unwrap(), None);
        store.set_setting("k", "v1").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v1"));
        store.set_setting("k", "v2").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v2"));
        cleanup(&path);
    }

    #[test]
    fn add_list_search_delete() {
        let (path, store) = temp_db("crud");
        let opts = ChunkOptions::default();
        let text = "Rust ownership moves values.\n\nSourdough starter hydration is high.";
        let chunks = chunk_text(text, &opts);
        let embeddings: Vec<Vec<f32>> =
            chunks.iter().map(|c| embed_local(&c.text)).collect();
        let doc = store
            .add_document("notes", "pasted", text.chars().count(), &chunks, &embeddings, "local")
            .unwrap();
        assert_eq!(doc.chunks, chunks.len());

        let docs = store.list_documents().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, doc.id);

        let query = embed_local("rust ownership rules");
        let hits = store.search(&query, 5, "local").unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].text.to_lowercase().contains("ownership"));

        // Wrong tag → no hits, never mixed spaces.
        assert!(store.search(&query, 5, "remote:other").unwrap().is_empty());

        assert!(store.delete_document(&doc.id).unwrap());
        assert!(store.list_documents().unwrap().is_empty());
        assert!(store.chunk_texts(&doc.id).unwrap().is_empty());
        assert!(!store.delete_document(&doc.id).unwrap());
        cleanup(&path);
    }

    #[test]
    fn reindex_moves_document_to_new_tag() {
        let (path, store) = temp_db("reindex");
        let opts = ChunkOptions::default();
        let chunks = chunk_text("Some content about wisselstroom.", &opts);
        let embeddings: Vec<Vec<f32>> =
            chunks.iter().map(|c| embed_local(&c.text)).collect();
        let doc = store
            .add_document("d", "pasted", 31, &chunks, &embeddings, "local")
            .unwrap();

        let texts = store.chunk_texts(&doc.id).unwrap();
        assert_eq!(texts.len(), chunks.len());
        let updated = store
            .reindex_document(&doc.id, &embeddings, "remote:mock")
            .unwrap();
        assert_eq!(updated.embedder, "remote:mock");

        let query = embed_local("wisselstroom");
        assert!(store.search(&query, 5, "local").unwrap().is_empty());
        assert_eq!(store.search(&query, 5, "remote:mock").unwrap().len(), chunks.len());
        cleanup(&path);
    }
}
