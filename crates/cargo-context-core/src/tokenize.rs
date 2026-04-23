//! Tokenizers.
//!
//! Dispatch matrix:
//!
//! | Variant               | Backend              | Fidelity                              |
//! | --------------------- | -------------------- | ------------------------------------- |
//! | `HfLlama3 { path }`   | HuggingFace `tokenizers` (lazy-loaded from a local `tokenizer.json`) | Exact, matches Llama3 billing |
//! | `TiktokenCl100k`      | `tiktoken-rs`        | Exact (GPT-4, GPT-4o)                 |
//! | `TiktokenO200k`       | `tiktoken-rs`        | Exact (GPT-5 family)                  |
//! | `Claude`              | `tiktoken-rs`        | Approximation via `cl100k_base`       |
//! | `Llama3`              | calibrated           | ~3.5 chars/token (mixed text)         |
//! | `Llama2`              | calibrated           | ~3.3 chars/token                      |
//! | `CharsDiv4`           | built-in             | Offline fallback                      |
//!
//! `HfLlama3` lets users point at any local `tokenizer.json` (Meta-Llama-3,
//! Mistral, Qwen, etc. — anything the HuggingFace `tokenizers` crate can
//! load). The file is loaded once per path and cached in a process-global
//! `HashMap`, so repeated `count()` calls pay tokenization cost only.
//!
//! A missing or unparseable vocab path silently falls back to the calibrated
//! heuristic; the scrubber/budget never aborts a pack build because the user
//! pointed at a bad path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tiktoken_rs::CoreBPE;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Tokenizer {
    #[default]
    Llama3,
    Llama2,
    TiktokenCl100k,
    TiktokenO200k,
    Claude,
    CharsDiv4,
    /// Exact counting via a locally-available `tokenizer.json` (HuggingFace
    /// format). Pointed at Meta-Llama-3's vocab it produces bill-accurate
    /// counts for Llama-family models; works equally well with any other
    /// tokenizer.json.
    HfLlama3 {
        vocab_path: PathBuf,
    },
}

impl Tokenizer {
    pub fn label(&self) -> &'static str {
        match self {
            Tokenizer::Llama3 => "llama3",
            Tokenizer::Llama2 => "llama2",
            Tokenizer::TiktokenCl100k => "tiktoken-cl100k",
            Tokenizer::TiktokenO200k => "tiktoken-o200k",
            Tokenizer::Claude => "claude",
            Tokenizer::CharsDiv4 => "chars-div-4",
            Tokenizer::HfLlama3 { .. } => "hf-llama3",
        }
    }

    /// Estimate tokens for `text`. Never panics; on backend init failure,
    /// falls back to the calibrated or `CharsDiv4` heuristic.
    pub fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        match self {
            Tokenizer::TiktokenCl100k | Tokenizer::Claude => cl100k()
                .map(|bpe| bpe.encode_with_special_tokens(text).len())
                .unwrap_or_else(|| chars_div_4(text)),
            Tokenizer::TiktokenO200k => o200k()
                .map(|bpe| bpe.encode_with_special_tokens(text).len())
                .unwrap_or_else(|| chars_div_4(text)),
            Tokenizer::Llama3 => calibrated(text, 3.5),
            Tokenizer::Llama2 => calibrated(text, 3.3),
            Tokenizer::CharsDiv4 => chars_div_4(text),
            Tokenizer::HfLlama3 { vocab_path } => count_hf(vocab_path, text),
        }
    }
}

/// `ceil(chars / 4)`.
fn chars_div_4(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// `ceil(chars / chars_per_token)`.
fn calibrated(text: &str, chars_per_token: f64) -> usize {
    let chars = text.chars().count() as f64;
    (chars / chars_per_token).ceil() as usize
}

fn cl100k() -> Option<&'static CoreBPE> {
    static CL100K: OnceLock<Option<CoreBPE>> = OnceLock::new();
    CL100K
        .get_or_init(|| tiktoken_rs::cl100k_base().ok())
        .as_ref()
}

fn o200k() -> Option<&'static CoreBPE> {
    static O200K: OnceLock<Option<CoreBPE>> = OnceLock::new();
    O200K
        .get_or_init(|| tiktoken_rs::o200k_base().ok())
        .as_ref()
}

/// Process-global cache of loaded HuggingFace tokenizers keyed by absolute
/// vocab path. Loading is expensive (parses a multi-MB JSON and compiles
/// regex patterns); this makes repeated `count()` calls cheap.
fn hf_cache() -> &'static Mutex<HashMap<PathBuf, Arc<tokenizers::Tokenizer>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<tokenizers::Tokenizer>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_hf(path: &Path) -> Option<Arc<tokenizers::Tokenizer>> {
    let key = path.to_path_buf();
    let mut cache = hf_cache().lock().ok()?;
    if let Some(t) = cache.get(&key) {
        return Some(Arc::clone(t));
    }
    let t = tokenizers::Tokenizer::from_file(path).ok()?;
    let arc = Arc::new(t);
    cache.insert(key, Arc::clone(&arc));
    Some(arc)
}

fn count_hf(vocab_path: &Path, text: &str) -> usize {
    match load_hf(vocab_path) {
        Some(t) => match t.encode(text, false) {
            Ok(enc) => enc.len(),
            Err(_) => calibrated(text, 3.5),
        },
        None => calibrated(text, 3.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_counts_zero() {
        for t in [
            Tokenizer::Llama3,
            Tokenizer::Llama2,
            Tokenizer::TiktokenCl100k,
            Tokenizer::TiktokenO200k,
            Tokenizer::Claude,
            Tokenizer::CharsDiv4,
            Tokenizer::HfLlama3 {
                vocab_path: PathBuf::from("/nonexistent"),
            },
        ] {
            assert_eq!(t.count(""), 0, "tokenizer {t:?}");
        }
    }

    #[test]
    fn nonempty_counts_at_least_one() {
        assert!(Tokenizer::Llama3.count("hello world") >= 1);
        assert!(Tokenizer::TiktokenCl100k.count("hello world") >= 1);
    }

    #[test]
    fn cl100k_matches_known_tokenization() {
        let n = Tokenizer::TiktokenCl100k.count("hello world");
        assert_eq!(n, 2, "cl100k count for 'hello world' should be 2, got {n}");
    }

    #[test]
    fn o200k_matches_known_tokenization() {
        let n = Tokenizer::TiktokenO200k.count("hello world");
        assert_eq!(n, 2, "o200k count for 'hello world' should be 2, got {n}");
    }

    #[test]
    fn claude_aliases_cl100k() {
        let s = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(
            Tokenizer::Claude.count(s),
            Tokenizer::TiktokenCl100k.count(s)
        );
    }

    #[test]
    fn llama_calibration_is_reasonable() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(10);
        let n = Tokenizer::Llama3.count(&text);
        let chars = text.chars().count();
        let expected_low = (chars as f64 / 4.0) as usize;
        let expected_high = (chars as f64 / 3.0) as usize;
        assert!(
            n >= expected_low && n <= expected_high,
            "llama3 count {n} outside sane range [{expected_low}, {expected_high}] for {chars} chars"
        );
    }

    #[test]
    fn label_is_stable() {
        // Stable labels are part of the pack's schema; don't change without
        // a schema version bump.
        assert_eq!(Tokenizer::Llama3.label(), "llama3");
        assert_eq!(Tokenizer::TiktokenCl100k.label(), "tiktoken-cl100k");
        assert_eq!(Tokenizer::TiktokenO200k.label(), "tiktoken-o200k");
        assert_eq!(Tokenizer::Claude.label(), "claude");
        assert_eq!(
            Tokenizer::HfLlama3 {
                vocab_path: PathBuf::from("/nowhere")
            }
            .label(),
            "hf-llama3"
        );
    }

    #[test]
    fn hf_llama3_missing_path_falls_back_gracefully() {
        let t = Tokenizer::HfLlama3 {
            vocab_path: PathBuf::from("/definitely/not/a/real/path/tokenizer.json"),
        };
        let n = t.count("hello world");
        // Falls back to calibrated ~3.5 chars/token → "hello world" = 11 chars → 4 tokens.
        assert!(n >= 1, "fallback should still produce a positive count");
    }

    #[test]
    fn hf_llama3_loads_real_tokenizer_without_falling_back() {
        // Build a minimal BPE tokenizer, persist it, load via HfLlama3.
        // The default BPE has an empty vocab so it counts 0 tokens — what we
        // verify is that the loader did not fall back to the calibrated
        // heuristic (which would give a positive count for non-empty input).
        use tokenizers::models::bpe::BPE;
        use tokenizers::Tokenizer as HfTokenizer;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tokenizer.json");
        let hf = HfTokenizer::new(BPE::default());
        hf.save(&path, false).expect("save tokenizer fixture");

        let our = Tokenizer::HfLlama3 {
            vocab_path: path.clone(),
        };
        let fallback = Tokenizer::HfLlama3 {
            vocab_path: PathBuf::from("/does/not/exist"),
        };

        // Real loader: 0 (empty vocab). Fallback: calibrated chars/3.5 > 0.
        assert_eq!(
            our.count("hello"),
            0,
            "real loader should short-circuit on empty vocab"
        );
        assert!(
            fallback.count("hello") >= 1,
            "missing path must fall back to calibrated"
        );
    }
}
