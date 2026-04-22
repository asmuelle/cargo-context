#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStrategy {
    #[default]
    Priority,
    Proportional,
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
            strategy: BudgetStrategy::Priority,
        }
    }
}

impl Budget {
    pub fn effective(&self) -> usize {
        self.max_tokens.saturating_sub(self.reserve_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
