use std::ops::{Index, IndexMut};

/// A small, safe inline vector with a lazily allocated overflow arena.
///
/// Values fill the inline array first. Once it is full, additional values are
/// stored in `spill`, preserving ordinary vector ordering without unsafe code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineVec<T, const N: usize> {
    inline: [Option<T>; N],
    inline_len: usize,
    spill: Vec<T>,
}

impl<T, const N: usize> InlineVec<T, N> {
    pub(crate) fn new() -> Self {
        Self {
            inline: std::array::from_fn(|_| None),
            inline_len: 0,
            spill: Vec::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inline_len + self.spill.len()
    }

    pub(crate) fn push(&mut self, value: T) {
        if self.inline_len < N {
            debug_assert!(self.spill.is_empty());
            self.inline[self.inline_len] = Some(value);
            self.inline_len += 1;
        } else {
            self.spill.push(value);
        }
    }

    pub(crate) fn pop(&mut self) -> Option<T> {
        if let Some(value) = self.spill.pop() {
            return Some(value);
        }
        let index = self.inline_len.checked_sub(1)?;
        self.inline_len = index;
        self.inline[index].take()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inline_len == 0 && self.spill.is_empty()
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.spill.last().or_else(|| {
            self.inline_len
                .checked_sub(1)
                .and_then(|index| self.inline[index].as_ref())
        })
    }

    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        if self.spill.is_empty() {
            let index = self.inline_len.checked_sub(1)?;
            self.inline[index].as_mut()
        } else {
            self.spill.last_mut()
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        if index < self.inline_len {
            self.inline[index].as_ref()
        } else {
            self.spill.get(index.checked_sub(self.inline_len)?)
        }
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.inline[..self.inline_len]
            .iter()
            .map(|value| value.as_ref().expect("occupied inline slot"))
            .chain(self.spill.iter())
    }
}

impl<T, const N: usize> Default for InlineVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Index<usize> for InlineVec<T, N> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("index is outside inline vector")
    }
}

impl<T, const N: usize> IndexMut<usize> for InlineVec<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        if index < self.inline_len {
            self.inline[index].as_mut().expect("occupied inline slot")
        } else {
            &mut self.spill[index - self.inline_len]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InlineVec;

    #[test]
    fn stores_inline_then_spills_in_order() {
        let mut values = InlineVec::<u32, 2>::new();
        assert_eq!(values.len(), 0);
        values.push(1);
        values.push(2);
        values.push(3);
        assert_eq!(values.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(values.pop(), Some(3));
        assert_eq!(values.pop(), Some(2));
        assert_eq!(values.pop(), Some(1));
        assert_eq!(values.pop(), None);
    }

    #[test]
    fn mutable_indexing_covers_inline_and_spill() {
        let mut values = InlineVec::<u32, 1>::new();
        values.push(2);
        values.push(4);
        values[0] += 1;
        values[1] += 2;
        assert_eq!(values.iter().copied().collect::<Vec<_>>(), vec![3, 6]);
    }
}
