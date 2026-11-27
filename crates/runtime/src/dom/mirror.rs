use std::sync::Arc;

// Match the JS buffer layout: 4 header floats (stale, capacity, layout count, moved), 8 floats per node,
// MIRROR_* bytes, then a count of bounds changes, a count of restyles and whether a node has a zoom.
#[derive(Default)]
pub struct LayoutMirror {
    memory: Option<Arc<MirrorMemory>>,
    capacity: usize,
}

// The float buffer that pages share, addressed by word.
pub(super) struct MirrorMemory(std::ptr::NonNull<f32>, usize);

// SAFETY: only the JS thread reads and writes the floats; other threads only drop the last owner after a GC.
unsafe impl Send for MirrorMemory {}
unsafe impl Sync for MirrorMemory {}

impl MirrorMemory {
    fn new(words: usize) -> Self {
        let memory = std::ptr::NonNull::new(Box::into_raw(vec![0f32; words].into_boxed_slice()))
            .expect("Layout memory")
            .cast::<f32>();
        Self(memory, words)
    }

    fn at(&self, word: usize) -> *mut f32 {
        self.0.as_ptr().wrapping_add(word)
    }
}

impl Drop for MirrorMemory {
    fn drop(&mut self) {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.0.as_ptr(), self.1)) });
    }
}

const MIRROR_BOUNDS: u8 = 1;
const MIRROR_SCROLLER: u8 = 2;
const MIRROR_CLIP: u8 = 4;
const MIRROR_POSITIONED: u8 = 8;
const MIRROR_HIDDEN: u8 = 16;
const MIRROR_FIXED: u8 = 32;
const MIRROR_INERT: u8 = 64;
// The box has a transform, or an ancestor has one that changes its size, so the bounds are not its layout box.
const MIRROR_TRANSFORMED: u8 = 128;

impl LayoutMirror {
    fn slot(&self, id: usize) -> *mut f32 {
        self.memory
            .as_ref()
            .map_or(std::ptr::NonNull::dangling().as_ptr(), |memory| {
                memory.at(0)
            })
            .wrapping_add(4 + id * 8)
    }

    pub(crate) fn mark_inert(&self, id: usize, inert: bool) {
        debug_assert!(id < self.capacity, "Reserve the mirror first");
        if id < self.capacity {
            unsafe {
                let flags = self.flags(id);
                *flags = (*flags & !MIRROR_INERT) | (u8::from(inert) * MIRROR_INERT);
            }
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn inert(&self, id: usize) -> bool {
        unsafe { *self.flags(id) & MIRROR_INERT != 0 }
    }

    fn flags(&self, id: usize) -> *mut u8 {
        self.slot(self.capacity).cast::<u8>().wrapping_add(id)
    }

    fn changes(&self) -> *mut f32 {
        self.slot(self.capacity).wrapping_add(self.capacity / 4)
    }

    // Counts the rectangles that layouts changed.
    pub(crate) fn changed(&self) -> f32 {
        match self.memory {
            Some(_) => unsafe { *self.changes() },
            None => 0.,
        }
    }

    // Counts the style changes, which can change a content box without a new border box.
    fn restyles(&self) -> *mut f32 {
        self.changes().wrapping_add(1)
    }

    pub(crate) fn restyled(&mut self) {
        if self.memory.is_some() {
            // Pages compare the count only for equality, so it wraps before an f32 stops counting.
            unsafe { *self.restyles() = (*self.restyles() + 1.) % 16_777_216. };
        }
    }

    pub(crate) fn zoomed(&mut self, on: bool) {
        if self.memory.is_some() {
            unsafe { *self.restyles().wrapping_add(1) = f32::from(u8::from(on)) };
        }
    }

    pub(crate) fn reserve(&mut self, id: usize) {
        if id >= self.capacity {
            self.resize((id + 1).next_power_of_two().max(1024));
        }
    }

    pub(crate) fn shrink(&mut self, len: usize) {
        let capacity = len.next_power_of_two().max(1024);
        if capacity * 4 <= self.capacity {
            self.resize(capacity);
        }
    }

    fn resize(&mut self, capacity: usize) {
        let words = 4 + capacity * 8 + capacity / 4 + 3;
        let memory = MirrorMemory::new(words);
        unsafe {
            *memory.at(1) = capacity as f32;
            if let Some(old) = &self.memory {
                let kept = self.capacity.min(capacity);
                std::ptr::copy_nonoverlapping(self.slot(0), memory.at(4), kept * 8);
                std::ptr::copy_nonoverlapping(
                    self.flags(0),
                    memory.at(4 + capacity * 8).cast::<u8>(),
                    kept,
                );
                *memory.at(2) = *old.at(2);
                *memory.at(words - 3) = *self.changes();
                *memory.at(words - 2) = *self.restyles();
                *memory.at(words - 1) = *self.restyles().wrapping_add(1);
                // Pages keep reading the old buffer until they see this flag and fetch the new one.
                *old.at(0) = 1.;
            }
        }
        self.memory = Some(Arc::new(memory));
        self.capacity = capacity;
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(crate) fn get(&self, id: &usize) -> Option<&[f32; 4]> {
        (*id < self.capacity && unsafe { *self.flags(*id) } & MIRROR_BOUNDS != 0)
            .then(|| unsafe { &*self.slot(*id).cast::<[f32; 4]>() })
    }

    pub(crate) fn get_mut(&mut self, id: &usize) -> Option<&mut [f32; 4]> {
        (*id < self.capacity && unsafe { *self.flags(*id) } & MIRROR_BOUNDS != 0)
            .then(|| unsafe { &mut *self.slot(*id).cast::<[f32; 4]>() })
    }

    pub fn insert(&mut self, id: usize, rect: [f32; 4]) {
        self.reserve(id);
        unsafe {
            let (slot, flags) = (self.slot(id).cast::<[f32; 4]>(), self.flags(id));
            // Pages compare the count only for equality, so it wraps before an f32 stops counting.
            if *flags & MIRROR_BOUNDS == 0 || *slot != rect {
                let changes = self.changes();
                *changes = (*changes + 1.) % 16_777_216.;
            }
            *slot = rect;
            *flags |= MIRROR_BOUNDS;
        }
    }

    pub(crate) fn transformed(&mut self, id: usize, on: bool) {
        debug_assert!(id < self.capacity, "Reserve the mirror first");
        if id < self.capacity {
            unsafe {
                let flags = self.flags(id);
                *flags = *flags & !MIRROR_TRANSFORMED | if on { MIRROR_TRANSFORMED } else { 0 };
            }
        }
    }

    pub(crate) fn shown(&self, id: usize) -> bool {
        id < self.capacity && unsafe { *self.flags(id) } & MIRROR_HIDDEN == 0
    }

    pub(crate) fn is_transformed(&self, id: usize) -> bool {
        id < self.capacity && unsafe { *self.flags(id) } & MIRROR_TRANSFORMED != 0
    }

    // Sets the marks that come from the style of the box: clip, positioned and fixed.
    pub(crate) fn style_flags(&mut self, id: usize, [clip, positioned, fixed]: [bool; 3]) {
        self.reserve(id);
        unsafe {
            let flags = self.flags(id);
            *flags = (*flags
                & (MIRROR_BOUNDS
                    | MIRROR_SCROLLER
                    | MIRROR_HIDDEN
                    | MIRROR_INERT
                    | MIRROR_TRANSFORMED))
                | if clip { MIRROR_CLIP } else { 0 }
                | if positioned { MIRROR_POSITIONED } else { 0 }
                | if fixed { MIRROR_FIXED } else { 0 };
        }
    }

    pub(crate) fn remove(&mut self, id: &usize) {
        if *id < self.capacity {
            unsafe {
                *self.slot(*id).cast::<[f32; 8]>() = [0.; 8];
                *self.flags(*id) = 0;
            }
        }
    }

    pub(crate) fn moved(&self) -> bool {
        self.memory
            .as_ref()
            .is_some_and(|memory| unsafe { *memory.at(3) } != 0.)
    }

    pub(crate) fn settle(&mut self) {
        if let Some(memory) = &self.memory {
            unsafe { *memory.at(3) = 0. };
        }
    }

    pub(crate) fn set_moved(&mut self) {
        if let Some(memory) = &self.memory {
            unsafe { *memory.at(3) = 1. };
        }
    }

    pub(crate) fn touch(&mut self) {
        self.reserve(0);
        if let Some(memory) = &self.memory {
            // An f32 stops counting at 2^24, so the count wraps. Pages only compare it for equality.
            unsafe {
                let count = memory.at(2);
                *count = (*count + 1.) % 16_777_216.;
                *memory.at(3) = 0.;
            }
        }
    }

    // `nodes` must hold every node shown in either list and every node past the end of `old`.
    pub(crate) fn hide(
        &mut self,
        visible: &[bool],
        old: &[bool],
        nodes: impl Iterator<Item = usize>,
    ) {
        if let Some(last) = visible.len().checked_sub(1) {
            self.reserve(last);
        }
        for id in nodes {
            let Some(&shown) = visible.get(id) else {
                continue;
            };
            if old.get(id).copied().unwrap_or(true) != shown {
                self.show(id, shown);
            }
        }
    }

    pub(crate) fn show(&mut self, id: usize, shown: bool) {
        self.reserve(id);
        unsafe {
            let flags = self.flags(id);
            *flags = *flags & !MIRROR_HIDDEN | if shown { 0 } else { MIRROR_HIDDEN };
        }
    }

    pub(crate) fn scroll(&mut self, id: usize, values: [f32; 4]) {
        self.reserve(id);
        unsafe {
            *self.slot(id).add(4).cast::<[f32; 4]>() = values;
            *self.flags(id) |= MIRROR_SCROLLER;
        }
    }

    // Pages take the content extent of a box without the scroller mark, so the mark must go with the handle.
    pub(crate) fn unscroll(&mut self, id: usize) {
        if id < self.capacity {
            unsafe {
                *self.slot(id).add(4).cast::<[f32; 4]>() = [0.; 4];
                *self.flags(id) &= !MIRROR_SCROLLER;
            }
        }
    }

    pub(crate) fn share(&mut self) -> (*mut std::ffi::c_void, usize, *mut std::ffi::c_void) {
        self.reserve(0);
        let memory = self.memory.clone().expect("Layout memory");
        (
            memory.at(0).cast(),
            (4 + self.capacity * 8) * 4 + self.capacity + 12,
            Arc::into_raw(memory).cast_mut().cast(),
        )
    }

    pub(crate) unsafe extern "C" fn release(
        _: *mut std::ffi::c_void,
        owner: *mut std::ffi::c_void,
    ) {
        drop(unsafe { Arc::from_raw(owner.cast::<MirrorMemory>()) });
    }
}

impl Extend<(usize, [f32; 4])> for LayoutMirror {
    fn extend<T: IntoIterator<Item = (usize, [f32; 4])>>(&mut self, items: T) {
        for (id, rect) in items {
            self.insert(id, rect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_count_wraps_before_f32_stops_counting() {
        let mut mirror = LayoutMirror::default();
        mirror.touch();
        let count = |mirror: &LayoutMirror| mirror.memory.as_ref().unwrap().at(2);
        assert_eq!(unsafe { *count(&mirror) }, 1.);
        unsafe { *count(&mirror) = 16_777_215. };
        mirror.touch();
        assert_eq!(unsafe { *count(&mirror) }, 0.);
    }

    // Debug builds catch a caller that did not reserve, and release builds skip the write.
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "Reserve the mirror first"))]
    fn flag_writes_past_the_buffer_do_nothing() {
        let mut mirror = LayoutMirror::default();
        mirror.mark_inert(3, true);
        mirror.transformed(3, true);
        mirror.reserve(0);
        let id = mirror.capacity();
        mirror.mark_inert(id, true);
        mirror.transformed(id, true);
        assert!(!mirror.is_transformed(id) && mirror.capacity() == id);
    }
}
