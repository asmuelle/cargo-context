//! Token budget allocation.
//!
//! The pack builder produces a list of *candidate* sections; this module
//! decides which survive within a token ceiling.

use crate::pack::Section;
use crate::tokenize::Tokenizer;

/// Strategy for reconciling candidate sections with the token limit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStrategy {
    /// Sort by priority, drop whole sections from the tail until the total
    /// fits. P0 sections (user prompt) are always retained even if they
    /// individually exceed the budget.
    #[default]
    Priority,

    /// Scale every competing section proportionally to its share of the
    /// total. Nothing is dropped unless its proportional slot rounds to
    /// zero tokens. Good when you want a balanced snapshot rather than a
    /// priority-biased triage.
    Proportional,

    /// Keep sections in priority order until one overflows; hard-truncate
    /// that section's content and stop.
    Truncate,
}

#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_tokens: usize,
    pub reserve_tokens: usize,
    pub strategy: BudgetStrategy,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_tokens: 8000,
            reserve_tokens: 2000,
            strategy: BudgetStrategy::default(),
        }
    }
}

impl Budget {
    /// Tokens available for pack content (i.e. `max - reserve`).
    pub fn effective(&self) -> usize {
        self.max_tokens.saturating_sub(self.reserve_tokens)
    }
}

/// Numeric priority for a candidate section. Lower = more important.
pub type Priority = u8;

/// Exempt from budget pressure. Used for the user prompt so the reader's
/// own question is never dropped.
pub const P_EXEMPT: Priority = 0;
pub const P_ERROR: Priority = 1;
pub const P_DIFF: Priority = 2;
pub const P_MAP: Priority = 3;
pub const P_ENTRY: Priority = 4;
pub const P_TESTS: Priority = 5;

/// Outcome of budget allocation: sections that fit + names of those dropped.
#[derive(Debug, Default)]
pub struct Allocation {
    pub kept: Vec<Section>,
    pub dropped: Vec<String>,
    pub tokens_used: usize,
    pub tokens_budget: usize,
}

/// Reconcile candidate sections with `budget` using its strategy.
pub fn allocate(
    candidates: Vec<(Priority, Section)>,
    budget: &Budget,
    tokenizer: &Tokenizer,
) -> Allocation {
    let limit = budget.effective();
    match budget.strategy {
        BudgetStrategy::Priority => apply_priority(candidates, limit),
        BudgetStrategy::Proportional => apply_proportional(candidates, limit, tokenizer),
        BudgetStrategy::Truncate => apply_truncate(candidates, limit, tokenizer),
    }
}

fn apply_priority(candidates: Vec<(Priority, Section)>, limit: usize) -> Allocation {
    // Split exempt sections off; they are unconditionally kept.
    let (exempt, mut competing): (Vec<Section>, Vec<(Priority, Section)>) =
        candidates.into_iter().partition_map(|(p, s)| {
            if p == P_EXEMPT {
                Left(s)
            } else {
                Right((p, s))
            }
        });
    competing.sort_by_key(|(p, _)| *p);

    let exempt_tokens: usize = exempt.iter().map(|s| s.token_estimate).sum();
    let remaining = limit.saturating_sub(exempt_tokens);

    let mut kept = exempt;
    let mut dropped: Vec<String> = Vec::new();
    let mut running: usize = 0;

    for (_p, s) in competing {
        if running + s.token_estimate <= remaining {
            running += s.token_estimate;
            kept.push(s);
        } else {
            dropped.push(s.name);
        }
    }

    let tokens_used = exempt_tokens + running;
    Allocation {
        kept,
        dropped,
        tokens_used,
        tokens_budget: limit,
    }
}

fn apply_truncate(
    candidates: Vec<(Priority, Section)>,
    limit: usize,
    tokenizer: &Tokenizer,
) -> Allocation {
    let (exempt, mut competing): (Vec<Section>, Vec<(Priority, Section)>) =
        candidates.into_iter().partition_map(|(p, s)| {
            if p == P_EXEMPT {
                Left(s)
            } else {
                Right((p, s))
            }
        });
    competing.sort_by_key(|(p, _)| *p);

    let exempt_tokens: usize = exempt.iter().map(|s| s.token_estimate).sum();
    let mut kept = exempt;
    let mut dropped: Vec<String> = Vec::new();
    let mut running: usize = 0;
    let remaining_after_exempt = limit.saturating_sub(exempt_tokens);

    for (_p, mut s) in competing {
        let budget_left = remaining_after_exempt.saturating_sub(running);
        if budget_left == 0 {
            dropped.push(s.name);
            continue;
        }
        if s.token_estimate <= budget_left {
            running += s.token_estimate;
            kept.push(s);
            continue;
        }
        // Oversized: hard-truncate content to roughly `budget_left` tokens.
        let orig_tokens = s.token_estimate.max(1);
        let ratio = (budget_left as f64 / orig_tokens as f64).clamp(0.0, 1.0);
        // Subtract ~5% for the truncation marker.
        let char_count = s.content.chars().count();
        let target_chars = ((char_count as f64) * ratio * 0.95) as usize;
        let mut new_content: String = s.content.chars().take(target_chars).collect();
        new_content.push_str("\n\n[... truncated by --budget-strategy=truncate ...]");
        s.content = new_content;
        s.token_estimate = tokenizer.count(&s.content);
        running += s.token_estimate;
        kept.push(s);
        // Remainder of competing sections get dropped.
        break;
    }

    // Any competing section we didn't process (because we broke out of the
    // loop above) was already fully consumed; nothing else to do.
    Allocation {
        kept,
        dropped,
        tokens_used: exempt_tokens + running,
        tokens_budget: limit,
    }
}

fn apply_proportional(
    candidates: Vec<(Priority, Section)>,
    limit: usize,
    tokenizer: &Tokenizer,
) -> Allocation {
    let (exempt, mut competing): (Vec<Section>, Vec<(Priority, Section)>) =
        candidates.into_iter().partition_map(|(p, s)| {
            if p == P_EXEMPT {
                Left(s)
            } else {
                Right((p, s))
            }
        });
    competing.sort_by_key(|(p, _)| *p);

    let exempt_tokens: usize = exempt.iter().map(|s| s.token_estimate).sum();
    let remaining = limit.saturating_sub(exempt_tokens);
    let total_competing: usize = competing.iter().map(|(_, s)| s.token_estimate).sum();

    let mut kept = exempt;
    let mut dropped: Vec<String> = Vec::new();
    let mut running: usize = 0;

    if total_competing == 0 {
        return Allocation {
            kept,
            dropped,
            tokens_used: exempt_tokens,
            tokens_budget: limit,
        };
    }

    // Fast path: everything fits, no truncation needed.
    if total_competing <= remaining {
        for (_, s) in competing {
            running += s.token_estimate;
            kept.push(s);
        }
        return Allocation {
            kept,
            dropped,
            tokens_used: exempt_tokens + running,
            tokens_budget: limit,
        };
    }

    // Scale every competing section by the same ratio.
    let ratio = remaining as f64 / total_competing as f64;
    let marker = "\n\n[... truncated by --budget-strategy=proportional ...]";
    let marker_tokens = tokenizer.count(marker);
    for (_, mut s) in competing {
        let target_tokens = ((s.token_estimate as f64) * ratio).floor() as usize;
        if target_tokens == 0 {
            // Section would be reduced to nothing; drop it rather than emit
            // an empty rendered section.
            dropped.push(s.name);
            continue;
        }
        if target_tokens >= s.token_estimate {
            running += s.token_estimate;
            kept.push(s);
            continue;
        }
        // Budget for body = target minus the marker's cost.
        let body_tokens = target_tokens.saturating_sub(marker_tokens);
        if body_tokens == 0 {
            dropped.push(s.name);
            continue;
        }
        let orig_chars = s.content.chars().count();
        let orig_tokens = s.token_estimate.max(1);
        let char_target =
            ((orig_chars as f64) * (body_tokens as f64) / (orig_tokens as f64)) as usize;
        let mut new_content: String = s.content.chars().take(char_target).collect();
        new_content.push_str(marker);
        s.content = new_content;
        s.token_estimate = tokenizer.count(&s.content);
        running += s.token_estimate;
        kept.push(s);
    }

    Allocation {
        kept,
        dropped,
        tokens_used: exempt_tokens + running,
        tokens_budget: limit,
    }
}

// Minimal partition_map to avoid pulling `itertools` just for this.
enum Either<L, R> {
    Left(L),
    Right(R),
}
use Either::{Left, Right};

trait PartitionMap<I> {
    fn partition_map<A, B, F>(self, f: F) -> (Vec<A>, Vec<B>)
    where
        F: FnMut(I) -> Either<A, B>;
}

impl<It, I> PartitionMap<I> for It
where
    It: Iterator<Item = I>,
{
    fn partition_map<A, B, F>(self, mut f: F) -> (Vec<A>, Vec<B>)
    where
        F: FnMut(I) -> Either<A, B>,
    {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for item in self {
            match f(item) {
                Left(x) => a.push(x),
                Right(x) => b.push(x),
            }
        }
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str, tokens: usize) -> Section {
        Section {
            name: name.into(),
            content: "x".repeat(tokens * 4),
            token_estimate: tokens,
        }
    }

    #[test]
    fn effective_subtracts_reserve() {
        let b = Budget {
            max_tokens: 8000,
            reserve_tokens: 2000,
            strategy: BudgetStrategy::Priority,
        };
        assert_eq!(b.effective(), 6000);
    }

    #[test]
    fn effective_saturates_at_zero() {
        let b = Budget {
            max_tokens: 100,
            reserve_tokens: 500,
            strategy: BudgetStrategy::Priority,
        };
        assert_eq!(b.effective(), 0);
    }

    #[test]
    fn priority_drops_lowest_priority_when_over_budget() {
        let candidates = vec![
            (P_ERROR, mk("errors", 200)),
            (P_DIFF, mk("diff", 500)),
            (P_MAP, mk("map", 400)),
        ];
        let b = Budget {
            max_tokens: 800,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Priority,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert_eq!(a.kept.len(), 2);
        assert_eq!(a.kept[0].name, "errors");
        assert_eq!(a.kept[1].name, "diff");
        assert_eq!(a.dropped, vec!["map"]);
        assert_eq!(a.tokens_used, 700);
    }

    #[test]
    fn priority_keeps_prompt_even_when_oversized() {
        let candidates = vec![
            (P_EXEMPT, mk("📝 User Prompt", 5000)),
            (P_ERROR, mk("errors", 100)),
        ];
        let b = Budget {
            max_tokens: 1000,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Priority,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert!(a.kept.iter().any(|s| s.name.contains("Prompt")));
        assert_eq!(a.dropped, vec!["errors"]);
    }

    #[test]
    fn truncate_hard_cuts_overflowing_section() {
        let candidates = vec![(P_ERROR, mk("errors", 200)), (P_DIFF, mk("diff", 1500))];
        let b = Budget {
            max_tokens: 800,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Truncate,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert_eq!(a.kept.len(), 2);
        assert_eq!(a.kept[0].name, "errors");
        assert_eq!(a.kept[1].name, "diff");
        // Diff was truncated — its new content contains the marker.
        assert!(a.kept[1].content.contains("truncated"));
        assert!(a.tokens_used <= 800);
    }

    #[test]
    fn zero_budget_drops_everything_except_exempt() {
        let candidates = vec![
            (P_EXEMPT, mk("📝 User Prompt", 50)),
            (P_ERROR, mk("errors", 100)),
            (P_DIFF, mk("diff", 100)),
        ];
        let b = Budget {
            max_tokens: 0,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Priority,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert_eq!(a.kept.len(), 1);
        assert_eq!(a.kept[0].name, "📝 User Prompt");
        assert_eq!(a.dropped, vec!["errors", "diff"]);
    }

    #[test]
    fn empty_candidates_produce_empty_allocation() {
        let a = allocate(Vec::new(), &Budget::default(), &Tokenizer::CharsDiv4);
        assert!(a.kept.is_empty());
        assert!(a.dropped.is_empty());
        assert_eq!(a.tokens_used, 0);
    }

    #[test]
    fn proportional_scales_all_sections_when_over_budget() {
        let candidates = vec![(P_ERROR, mk("errors", 400)), (P_DIFF, mk("diff", 600))];
        let b = Budget {
            max_tokens: 500,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Proportional,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        // Both sections survive (the whole point of proportional: no drops).
        assert_eq!(a.kept.len(), 2);
        assert!(a.dropped.is_empty());
        // Both got truncated.
        assert!(
            a.kept.iter().all(|s| s.content.contains("truncated")),
            "all oversized sections should carry the truncation marker"
        );
        // Budget is honored.
        assert!(
            a.tokens_used <= b.max_tokens,
            "tokens_used {} exceeded budget {}",
            a.tokens_used,
            b.max_tokens
        );
    }

    #[test]
    fn proportional_keeps_whole_when_under_budget() {
        let candidates = vec![(P_ERROR, mk("errors", 100)), (P_DIFF, mk("diff", 200))];
        let b = Budget {
            max_tokens: 1000,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Proportional,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert_eq!(a.kept.len(), 2);
        assert_eq!(a.tokens_used, 300);
        assert!(a.dropped.is_empty());
        assert!(
            a.kept.iter().all(|s| !s.content.contains("truncated")),
            "under-budget allocation should not truncate"
        );
    }

    #[test]
    fn proportional_always_keeps_exempt() {
        let candidates = vec![
            (P_EXEMPT, mk("📝 User Prompt", 100)),
            (P_ERROR, mk("errors", 500)),
        ];
        let b = Budget {
            max_tokens: 200,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Proportional,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        // Prompt kept whole; errors proportionally scaled into remaining 100.
        assert!(a.kept.iter().any(|s| s.name.contains("Prompt")));
    }

    #[test]
    fn proportional_drops_zero_slot_sections() {
        // Tiny section + huge section with tight budget: tiny's share rounds
        // to zero. Preferable to emit an empty section? No — drop it and
        // report.
        let candidates = vec![(P_ERROR, mk("tiny", 1)), (P_DIFF, mk("huge", 10_000))];
        let b = Budget {
            max_tokens: 100,
            reserve_tokens: 0,
            strategy: BudgetStrategy::Proportional,
        };
        let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
        assert!(
            a.dropped.contains(&"tiny".to_string()),
            "zero-slot section should be dropped, got dropped={:?}",
            a.dropped
        );
    }

    #[test]
    fn non_exempt_strategies_do_not_exceed_effective_budget() {
        for strategy in [
            BudgetStrategy::Priority,
            BudgetStrategy::Proportional,
            BudgetStrategy::Truncate,
        ] {
            let candidates = vec![
                (P_ERROR, mk("errors", 300)),
                (P_DIFF, mk("diff", 800)),
                (P_MAP, mk("map", 200)),
                (P_TESTS, mk("tests", 500)),
            ];
            let b = Budget {
                max_tokens: 700,
                reserve_tokens: 100,
                strategy,
            };
            let a = allocate(candidates, &b, &Tokenizer::CharsDiv4);
            assert!(
                a.tokens_used <= b.effective(),
                "{strategy:?} used {} tokens over effective budget {}",
                a.tokens_used,
                b.effective()
            );
        }
    }
}
