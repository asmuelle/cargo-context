//! Tokenizers. The skeleton uses the `chars/4` heuristic for every variant;
//! the real implementations (HuggingFace `tokenizers`, `tiktoken-rs`) are
//! wired in as those dependencies are added.

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

    /// Estimate tokens for `text`.
    pub fn count(&self, text: &str) -> usize {
        text.chars().count().div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_counts_zero() {
        assert_eq!(Tokenizer::Llama3.count(""), 0);
    }

    #[test]
    fn nonempty_counts_at_least_one() {
        assert!(Tokenizer::Llama3.count("hello world") >= 1);
    }
}
