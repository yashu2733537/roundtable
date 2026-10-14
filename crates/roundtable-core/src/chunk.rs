//! Text chunking for RAG indexing.
//!
//! Splits documents into overlapping, UTF-8-safe chunks: paragraphs first,
//! then lines, then sentences, then a hard split that prefers whitespace.
//! Packing is greedy and every chunk boundary falls between units, so short
//! source lines are never cut in half.

/// Chunking knobs. Defaults suit plain-text / markdown documents.
#[derive(Debug, Clone)]
pub struct ChunkOptions {
    /// Soft target size of a chunk in characters.
    pub target_chars: usize,
    /// Characters of trailing overlap carried into the next chunk.
    pub overlap_chars: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            target_chars: 1200,
            overlap_chars: 150,
        }
    }
}

/// One indexed unit of a document. `seq` is the chunk order (0-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub seq: usize,
    pub text: String,
}

/// A splittable piece of text plus the separator that originally followed it,
/// so chunks reassemble with the document's own line breaks.
struct Unit {
    text: String,
    sep_after: &'static str,
}

/// Split `text` into overlapping chunks. Never panics on any input, including
/// empty strings and text without whitespace.
pub fn chunk_text(text: &str, opts: &ChunkOptions) -> Vec<Chunk> {
    let target = opts.target_chars.max(64);
    let overlap = opts.overlap_chars.min(target / 2);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let units = split_units(&normalized, target);
    if units.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < units.len() {
        let mut end = start;
        let mut len = 0usize;
        while end < units.len() {
            let unit_len = units[end].text.chars().count();
            let sep_len = if end > start {
                units[end - 1].sep_after.chars().count()
            } else {
                0
            };
            // Units are already capped at `target`, so a single unit always
            // fits; only break *between* units.
            if end > start && len + sep_len + unit_len > target {
                break;
            }
            len += sep_len + unit_len;
            end += 1;
        }
        // Defensive: packing must always advance.
        if end == start {
            end = start + 1;
        }
        chunks.push(Chunk {
            seq: chunks.len(),
            text: join_units(&units[start..end]),
        });
        if end >= units.len() {
            break;
        }
        start = next_start(&units, start, end, overlap);
    }
    chunks
}

/// Overlap-aware start of the next chunk: walk back from `end` covering up
/// to `overlap` characters, always advancing at least one unit.
fn next_start(units: &[Unit], start: usize, end: usize, overlap: usize) -> usize {
    if overlap == 0 {
        return end;
    }
    let mut k = end;
    let mut acc = 0usize;
    while k > start {
        k -= 1;
        acc += units[k].text.chars().count();
        if acc >= overlap {
            break;
        }
    }
    // `k` is now the first overlapping unit; never rewind to or before `start`
    // without also advancing, and never stall.
    let candidate = if k <= start { start + 1 } else { k };
    candidate.max(start + 1).min(end)
}

fn join_units(units: &[Unit]) -> String {
    let mut out = String::new();
    for (i, unit) in units.iter().enumerate() {
        if i > 0 {
            out.push_str(units[i - 1].sep_after);
        }
        out.push_str(&unit.text);
    }
    out.trim().to_string()
}

fn split_units(text: &str, target: usize) -> Vec<Unit> {
    let mut units = Vec::new();
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if para.chars().count() <= target {
            units.push(Unit {
                text: para.to_string(),
                sep_after: "\n\n",
            });
            continue;
        }
        for line in para.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.chars().count() <= target {
                units.push(Unit {
                    text: line.to_string(),
                    sep_after: "\n",
                });
            } else {
                units.extend(split_sentences(line, target));
            }
        }
    }
    units
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '。' | '！' | '？')
}

fn is_closing(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')' | ']' | '}')
}

fn split_sentences(line: &str, target: usize) -> Vec<Unit> {
    let chars: Vec<char> = line.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if is_terminator(chars[i]) {
            let mut j = i + 1;
            while j < chars.len() && is_closing(chars[j]) {
                j += 1;
            }
            if j >= chars.len() || chars[j].is_whitespace() {
                let sentence: String = chars[start..j].iter().collect();
                let sentence = sentence.trim();
                if !sentence.is_empty() {
                    sentences.push(sentence.to_string());
                }
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < chars.len() {
        let rest: String = chars[start..].iter().collect();
        let rest = rest.trim();
        if !rest.is_empty() {
            sentences.push(rest.to_string());
        }
    }

    let mut units = Vec::new();
    for sentence in sentences {
        if sentence.chars().count() <= target {
            units.push(Unit {
                text: sentence,
                sep_after: " ",
            });
        } else {
            units.extend(hard_split(&sentence, target));
        }
    }
    units
}

/// Last-resort split for one over-long run: cut at `target` characters,
/// preferring a whitespace boundary in the last fifth of the window.
/// Never splits a UTF-8 code point (we index `char`s, not bytes).
fn hard_split(text: &str, target: usize) -> Vec<Unit> {
    let chars: Vec<char> = text.chars().collect();
    let mut units = Vec::new();
    let mut idx = 0usize;
    while chars.len() - idx > target {
        let window_end = idx + target;
        let min_cut = idx + target * 4 / 5;
        let mut cut = window_end;
        let mut p = window_end;
        while p > min_cut {
            if chars[p - 1].is_whitespace() {
                cut = p;
                break;
            }
            p -= 1;
        }
        let cut_at_space = cut > idx && chars[cut - 1].is_whitespace();
        let piece: String = chars[idx..cut].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            units.push(Unit {
                text: piece.to_string(),
                sep_after: if cut_at_space { " " } else { "" },
            });
        }
        idx = cut;
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
    }
    let tail: String = chars[idx..].iter().collect();
    let tail = tail.trim();
    if !tail.is_empty() {
        units.push(Unit {
            text: tail.to_string(),
            sep_after: "",
        });
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_chunks() {
        let opts = ChunkOptions::default();
        assert!(chunk_text("", &opts).is_empty());
        assert!(chunk_text("   \n\n  \n ", &opts).is_empty());
    }

    #[test]
    fn short_input_yields_one_chunk() {
        let opts = ChunkOptions::default();
        let chunks = chunk_text("Hello, Roundtable.", &opts);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].seq, 0);
        assert_eq!(chunks[0].text, "Hello, Roundtable.");
    }

    #[test]
    fn chunks_stay_within_budget() {
        let opts = ChunkOptions {
            target_chars: 200,
            overlap_chars: 40,
        };
        let text = (0..60)
            .map(|i| format!("Paragraph {i} has a few sentences. Here is another one! And a third?"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let chunks = chunk_text(&text, &opts);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.text.chars().count() <= 200 + 40 + 8,
                "chunk too big: {} chars",
                chunk.text.chars().count()
            );
        }
    }

    #[test]
    fn short_lines_survive_whole() {
        let opts = ChunkOptions {
            target_chars: 120,
            overlap_chars: 20,
        };
        let lines = vec![
            "first short line",
            "second short line here",
            "third",
            "fourth line with a few more words in it",
        ];
        let text = lines.join("\n\n");
        let chunks = chunk_text(&text, &opts);
        assert!(!chunks.is_empty());
        for line in &lines {
            assert!(
                chunks.iter().any(|c| c.text.contains(line)),
                "line missing from every chunk: {line}"
            );
        }
    }

    #[test]
    fn consecutive_chunks_share_overlap() {
        let opts = ChunkOptions {
            target_chars: 60,
            overlap_chars: 30,
        };
        let text = (0..30)
            .map(|i| format!("sentence number {i} lives here."))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&text, &opts);
        assert!(chunks.len() > 2);
        // With overlap, the tail of one chunk reappears at the head of the next.
        let shared = chunks
            .windows(2)
            .filter(|w| {
                let tail: String = w[0].text.chars().rev().take(10).collect();
                let tail: String = tail.chars().rev().collect();
                w[1].text.contains(&tail)
            })
            .count();
        assert!(shared > 0, "expected overlapping text between chunks");
    }

    #[test]
    fn chunking_is_deterministic() {
        let opts = ChunkOptions::default();
        let text = "Rust ownership moves values.\n\nBorrowing borrows them! Lifetimes constrain borrows? Yes.";
        assert_eq!(chunk_text(text, &opts), chunk_text(text, &opts));
    }

    #[test]
    fn hard_split_never_panics_or_stalls() {
        let opts = ChunkOptions {
            target_chars: 100,
            overlap_chars: 10,
        };
        let text = "x".repeat(5000) + &" 🚀".repeat(500);
        let chunks = chunk_text(&text, &opts);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.text.chars().count() <= 100 + 10 + 8);
        }
    }
}
