//! Embeddings for RAG retrieval.
//!
//! Two backends behind one call shape:
//! - [`EmbedderConfig::Local`]: offline lexical vectors (hashed unigram +
//!   bigram + char-trigram features, sublinear TF, L2-normalized). No network,
//!   no key, good enough for word-overlap retrieval.
//! - [`EmbedderConfig::Remote`]: an OpenAI-compatible `/embeddings` endpoint
//!   for semantic vectors. The API key is passed per call and never stored.
//!
//! Every vector carries an embedder [`tag`](EmbedderConfig::tag) so vectors
//! from different backends/models never mix in one search.

use std::collections::HashMap;
use std::time::Duration;

use crate::RagError;

/// Local (offline) embedding width.
pub const LOCAL_DIMS: usize = 384;

/// Which embedder produced a vector. Stored per document/chunk as
/// [`tag`](EmbedderConfig::tag).
#[derive(Debug, Clone)]
pub enum EmbedderConfig {
    Local,
    Remote { base_url: String, model: String },
}

impl EmbedderConfig {
    /// Stable identity used to keep vector spaces apart: `"local"` or
    /// `"remote:<model>"`.
    pub fn tag(&self) -> String {
        match self {
            Self::Local => "local".to_string(),
            Self::Remote { model, .. } => format!("remote:{model}"),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Local => "Local (offline)".to_string(),
            Self::Remote { base_url, model } => format!("{model} via {base_url}"),
        }
    }
}

/// Embed many texts at once (batching handled internally for remote).
pub async fn embed_texts(
    cfg: &EmbedderConfig,
    api_key: Option<&str>,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, RagError> {
    match cfg {
        EmbedderConfig::Local => Ok(texts.iter().map(|t| embed_local(t)).collect()),
        EmbedderConfig::Remote { base_url, model } => {
            embed_remote(base_url, model, api_key, texts).await
        }
    }
}

// ---------------------------------------------------------------------------
// Local hashed embedder
// ---------------------------------------------------------------------------

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// Deterministic offline embedding: signed feature hashing over unigrams,
/// adjacent bigrams, and per-token char trigrams, then L2 normalization.
pub fn embed_local(text: &str) -> Vec<f32> {
    let tokens = tokenize(text);
    let mut counts: HashMap<String, u32> = HashMap::new();
    for token in &tokens {
        *counts.entry(token.clone()).or_default() += 1;
    }
    for pair in tokens.windows(2) {
        *counts.entry(format!("{}\u{1}{}", pair[0], pair[1])).or_default() += 1;
    }
    for token in &tokens {
        let chars: Vec<char> = token.chars().collect();
        if chars.len() >= 3 {
            for i in 0..=chars.len() - 3 {
                let trigram: String = chars[i..i + 3].iter().collect();
                *counts.entry(trigram).or_default() += 1;
            }
        }
    }

    let mut vec = vec![0f32; LOCAL_DIMS];
    for (feature, tf) in counts {
        let hash = fnv1a(feature.as_bytes());
        let index = (hash >> 1) as usize % LOCAL_DIMS;
        let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
        let weight = 1.0 + f32::ln(tf as f32);
        vec[index] += sign * weight;
    }
    let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut vec {
            *x /= norm;
        }
    }
    vec
}

// ---------------------------------------------------------------------------
// Remote OpenAI-compatible embedder
// ---------------------------------------------------------------------------

const REMOTE_BATCH: usize = 64;

async fn embed_remote(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, RagError> {
    let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| RagError::Embed(format!("Could not start the HTTP client: {e}")))?;

    let mut out = Vec::with_capacity(texts.len());
    for batch in texts.chunks(REMOTE_BATCH) {
        let mut request = client.post(&url).json(&serde_json::json!({
            "model": model,
            "input": batch,
        }));
        if let Some(key) = api_key {
            if !key.is_empty() {
                request = request.bearer_auth(key);
            }
        }
        let response = request.send().await.map_err(|e| {
            RagError::Embed(format!(
                "Could not reach {url} — check the base URL and that the server is running. ({e})"
            ))
        })?;
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(RagError::Embed(format!(
                "API key rejected (HTTP {}) — update it in Settings → Embeddings.",
                status.as_u16()
            )));
        }
        if status.as_u16() == 404 {
            return Err(RagError::Embed(format!(
                "{url} returned 404 — the base URL should usually end in /v1."
            )));
        }
        if status.as_u16() == 429 {
            return Err(RagError::Embed(
                "The endpoint is rate-limiting requests (HTTP 429) — wait a moment and try again."
                    .to_string(),
            ));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(RagError::Embed(format!(
                "{url} returned HTTP {}: {}",
                status.as_u16(),
                truncate(&body, 200)
            )));
        }
        let payload: serde_json::Value = response.json().await.map_err(|_| {
            RagError::Embed(format!(
                "{url} did not return an OpenAI-compatible /embeddings payload."
            ))
        })?;
        let mut data = payload
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| {
                RagError::Embed(format!(
                    "{url} did not return an OpenAI-compatible /embeddings payload."
                ))
            })?
            .clone();
        data.sort_by_key(|item| {
            item.get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        });
        if data.len() != batch.len() {
            return Err(RagError::Embed(format!(
                "{url} returned {} vectors for {} inputs — the endpoint is misbehaving.",
                data.len(),
                batch.len()
            )));
        }
        for item in &data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| {
                    RagError::Embed(format!(
                        "{url} did not return an OpenAI-compatible /embeddings payload."
                    ))
                })?;
            let vec: Vec<f32> = embedding
                .iter()
                .map(|n| n.as_f64().unwrap_or(0.0) as f32)
                .collect();
            if vec.is_empty() {
                return Err(RagError::Embed(format!(
                    "{url} returned an empty vector — the endpoint is misbehaving."
                )));
            }
            out.push(vec);
        }
    }
    Ok(out)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.trim().to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…", head.trim())
}

// ---------------------------------------------------------------------------
// Vector math + storage encoding
// ---------------------------------------------------------------------------

/// Cosine similarity in [-1, 1]. Returns 0 for mismatched or degenerate
/// vectors instead of NaN, so bad rows degrade to "no match".
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Encode a vector for the SQLite BLOB column (little-endian f32s).
pub fn vec_to_blob(vec: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vec.len() * 4);
    for value in vec {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

/// Decode a BLOB column back into a vector.
pub fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>, RagError> {
    if blob.len() % 4 != 0 {
        return Err(RagError::Db("Stored embedding is corrupt.".to_string()));
    }
    Ok(blob
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_embedding_is_deterministic_and_normalized() {
        let text = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(embed_local(text), embed_local(text));
        let vec = embed_local(text);
        assert_eq!(vec.len(), LOCAL_DIMS);
        let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
        assert_eq!(embed_local(""), vec![0.0; LOCAL_DIMS]);
    }

    #[test]
    fn local_embedding_ranks_topical_match_first() {
        let rust_doc = embed_local("Rust ownership moves values; borrowing borrows them; the checker enforces lifetimes.");
        let cooking_doc =
            embed_local("Sourdough starter hydration percentages and overnight proofing schedules.");
        let query = embed_local("rust ownership rules");
        assert!(
            cosine(&query, &rust_doc) > cosine(&query, &cooking_doc),
            "rust doc should win for a rust query"
        );
    }

    #[test]
    fn cosine_handles_edge_cases() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]), -1.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn blob_roundtrip() {
        let vec = vec![1.5, -2.25, 0.0, std::f32::consts::PI];
        assert_eq!(blob_to_vec(&vec_to_blob(&vec)).unwrap(), vec);
        assert!(blob_to_vec(&[1, 2, 3]).is_err());
    }

    #[test]
    fn embedder_tags_keep_spaces_apart() {
        assert_eq!(EmbedderConfig::Local.tag(), "local");
        let remote = EmbedderConfig::Remote {
            base_url: "https://api.example.com/v1".to_string(),
            model: "mock-model".to_string(),
        };
        assert_eq!(remote.tag(), "remote:mock-model");
    }

    /// The OpenAI-compatible `/embeddings` wire protocol, asserted against a
    /// hand-rolled local HTTP server (no external network): auth header is
    /// sent and out-of-order `data` is re-sorted by `index`.
    #[tokio::test]
    async fn remote_protocol_parses_and_authenticates() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let headers = &head[..pos];
                    let content_len = headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if read >= pos + 4 + content_len {
                        break;
                    }
                }
                if read >= buf.len() {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            let body = r#"{"object":"list","data":[{"object":"embedding","index":1,"embedding":[0.0,1.0]},{"object":"embedding","index":0,"embedding":[1.0,0.0]}],"model":"mock"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let cfg = EmbedderConfig::Remote {
            base_url: format!("http://{addr}/v1"),
            model: "mock".to_string(),
        };
        let out = embed_texts(&cfg, Some("sekrit"), &["hello".to_string(), "world".to_string()])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![1.0f32, 0.0]);
        assert_eq!(out[1], vec![0.0f32, 1.0]);

        let request = server.await.unwrap();
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sekrit"),
            "expected bearer auth header"
        );
        assert!(request.contains("\"model\":\"mock\""));
    }
}
