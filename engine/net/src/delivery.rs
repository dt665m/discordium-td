//! Bounded application delivery feedback, independent of execution or transport ACKs.
use std::collections::VecDeque;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptWindowError {
    Invalid,
    Full,
    Unissued,
}
#[derive(Debug, Clone)]
pub struct ReceiptWindow<T> {
    acknowledged: T,
    issued: VecDeque<T>,
    limit: usize,
}
impl<T: Copy + Ord> ReceiptWindow<T> {
    pub fn new(initial: T, limit: usize) -> Result<Self, ReceiptWindowError> {
        if limit == 0 {
            return Err(ReceiptWindowError::Invalid);
        }
        Ok(Self {
            acknowledged: initial,
            issued: VecDeque::with_capacity(limit),
            limit,
        })
    }
    pub fn len(&self) -> usize {
        self.issued.len()
    }
    pub fn is_empty(&self) -> bool {
        self.issued.is_empty()
    }
    pub fn is_full(&self) -> bool {
        self.issued.len() == self.limit
    }
    pub fn acknowledged(&self) -> T {
        self.acknowledged
    }
    pub fn issue(&mut self, endpoint: T) -> Result<(), ReceiptWindowError> {
        if endpoint <= self.issued.back().copied().unwrap_or(self.acknowledged) {
            return Err(ReceiptWindowError::Invalid);
        }
        if self.is_full() {
            return Err(ReceiptWindowError::Full);
        }
        self.issued.push_back(endpoint);
        Ok(())
    }
    pub fn acknowledge(&mut self, endpoint: T) -> Result<(), ReceiptWindowError> {
        if endpoint <= self.acknowledged {
            return Ok(());
        }
        if !self.issued.contains(&endpoint) {
            return Err(ReceiptWindowError::Unissued);
        }
        while self.issued.front().is_some_and(|value| *value <= endpoint) {
            self.issued.pop_front();
        }
        self.acknowledged = endpoint;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_issued_endpoints_free_bounded_delivery_capacity() {
        let mut window = ReceiptWindow::new(0, 2).unwrap();
        window.issue(2).unwrap();
        window.issue(4).unwrap();
        assert_eq!(window.issue(5), Err(ReceiptWindowError::Full));
        for endpoint in [3, 5] {
            assert_eq!(
                window.acknowledge(endpoint),
                Err(ReceiptWindowError::Unissued)
            );
            assert!(window.is_full());
        }
        window.acknowledge(2).unwrap();
        window.acknowledge(1).unwrap();
        window.issue(6).unwrap();
        window.acknowledge(6).unwrap();
        assert!(window.is_empty());
        assert_eq!(window.acknowledged(), 6);
        assert_eq!(window.issue(6), Err(ReceiptWindowError::Invalid));
    }
}
