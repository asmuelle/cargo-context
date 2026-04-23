//! Tokenizers.
//!
//! Dispatch matrix:
//!
//! | Variant          | Backend          | Fidelity                                   |
//! | ---------------- | ---------------- | ------------------------------------------ |
//! | `TiktokenCl100k` | `tiktoken-rs`    | Exact (GPT-4, GPT-4o, Claude approximation)|
//! | `TiktokenO200k`  | `tiktoken-rs`    | Exact (GPT-5 family)                       |
//! | `Claude`         | `tiktoken-rs`    | Approximation via `cl100k_base`            |
//! | `Llama3`         | calibrated       | ~3.5 chars/token (English+code mix)        |
//! | `Llama2`         | calibrated       | ~3.3 chars/token                           |
//! | `CharsDiv4`      | built-in         | Truly offline fallback                     |
//!
//! Claude and llama use calibrated approximations rather than their exact
//! tokenizers because the exact ones either require vendor auth (llama HF
//! vocab is gated) or aren't publicly released (Anthropic). The approximations
//! are within 5-10% of the real count for typical mixed English+code text,
//! which is the same ballpark as the natural variance between tokens and
//! actual model cost.
//!
//! A later revision will add a `HfLlama3 { vocab_path }` variant for users
//! who have a local Llama3 tokenizer.json.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tokenizer {
    #[default]
    Llama3,
    Llama2,
    TiktokenCl100k,
    TiktokenO200k,
    Claude,
    CharsDiv4,
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
        }
    }

    /// Estimate tokens for `text`. Never panics; on backend init failure,
    /// falls back to the `CharsDiv4` heuristic.
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
        // tiktoken-rs's cl100k_base tokenizes "hello world" as two tokens
        // (["hello", " world"]). This guards against a silent backend swap.
        let n = Tokenizer::TiktokenCl100k.count("hello world");
        assert_eq!(n, 2, "cl100k count for 'hello world' should be 2, got {n}");
    }

    #[test]
    fn o200k_matches_known_tokenization() {
        // Same string, o200k encoding. Also 2 tokens.
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
        // An all-ASCII paragraph should land near chars/3.5 for llama3.
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
        // Stable labels are part of the pack's schema; don't change without a
        // schema version bump.
        assert_eq!(Tokenizer::Llama3.label(), "llama3");
        assert_eq!(Tokenizer::TiktokenCl100k.label(), "tiktoken-cl100k");
        assert_eq!(Tokenizer::TiktokenO200k.label(), "tiktoken-o200k");
        assert_eq!(Tokenizer::Claude.label(), "claude");
    }
}
