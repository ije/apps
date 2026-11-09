use std::rc::Rc;

const BITS: usize = 8;
const SIZE: usize = 1 << BITS;
const MASK: usize = SIZE - 1;

// WARNING: per-node outputs are as long as the node arena, while a pass writes few nodes, so copies share chunks of 256
// entries and a write copies only its chunk. Copying the whole list every pass cost more than styling the new rows.
#[derive(Clone)]
pub(crate) struct Chunked<T> {
    chunks: Vec<Rc<Vec<T>>>,
    len: usize,
}

impl<T> Default for Chunked<T> {
    fn default() -> Self {
        Self { chunks: Vec::new(), len: 0 }
    }
}

impl<T: Clone> Chunked<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| &self.chunks[index >> BITS][index & MASK])
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|chunk| chunk.iter()).take(self.len)
    }

    pub(crate) fn clear(&mut self) {
        self.chunks.clear();
        self.len = 0;
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        self.chunks.truncate(len.div_ceil(SIZE));
        if let Some(last) = self.chunks.last_mut()
            && len & MASK != 0
        {
            Rc::make_mut(last).truncate(len & MASK);
        }
        self.len = len;
    }

    pub(crate) fn resize(&mut self, len: usize, value: T) {
        self.resize_with(len, || value.clone());
    }

    pub(crate) fn resize_with(&mut self, len: usize, mut value: impl FnMut() -> T) {
        if len <= self.len {
            self.truncate(len);
            return;
        }
        while self.len < len {
            if self.len & MASK == 0 {
                self.chunks.push(Rc::new(Vec::with_capacity(SIZE)));
            }
            let chunk = Rc::make_mut(self.chunks.last_mut().expect("a chunk for the next entry"));
            let count = (SIZE - (self.len & MASK)).min(len - self.len);
            chunk.extend(std::iter::repeat_with(&mut value).take(count));
            self.len += count;
        }
    }
}

impl<T: Clone + PartialEq> PartialEq for Chunked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<T: Clone + std::fmt::Debug> std::fmt::Debug for Chunked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Clone> std::ops::Index<usize> for Chunked<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        assert!(index < self.len, "index {index} out of range {}", self.len);
        &self.chunks[index >> BITS][index & MASK]
    }
}

impl<T: Clone> std::ops::IndexMut<usize> for Chunked<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len, "index {index} out of range {}", self.len);
        &mut Rc::make_mut(&mut self.chunks[index >> BITS])[index & MASK]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_share_chunks_until_written() {
        let mut list = Chunked::default();
        list.resize(600, 0u32);
        list[300] = 7;
        let mut copy = list.clone();
        copy[5] = 9;
        assert_eq!((list[5], copy[5], list[300], copy[300]), (0, 9, 7, 7));
        assert!(Rc::ptr_eq(&list.chunks[1], &copy.chunks[1]));
        copy.truncate(257);
        copy.resize(258, 4);
        assert_eq!((copy.len(), copy[257], copy.get(258)), (258, 4, None));
        assert_eq!(copy.iter().copied().sum::<u32>(), 9 + 4);
    }
}
