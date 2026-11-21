pub struct TokenBucket {
    max_tokens: u64,
    available: u64,
}

impl TokenBucket {
    pub fn new(max_tokens: u64) -> Self {
        Self {
            max_tokens,
            available: max_tokens,
        }
    }

    pub fn try_acquire(&mut self, cost: u64) -> bool {
        if cost == 0 {
            return true;
        }
        if self.available < cost {
            return false;
        }
        self.available -= cost;
        true
    }

    pub fn refill(&mut self, amount: u64) {
        self.available = self.available.saturating_add(amount).min(self.max_tokens);
    }

    pub fn remaining(&self) -> u64 {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_acquire_and_refill() {
        let mut b = TokenBucket::new(10);
        assert!(b.try_acquire(6));
        assert_eq!(b.remaining(), 4);
        assert!(!b.try_acquire(5));
        assert_eq!(b.remaining(), 4);
        b.refill(20);
        assert_eq!(b.remaining(), 10);
    }
}
