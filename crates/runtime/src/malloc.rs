use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_int};
use std::ptr::{copy_nonoverlapping, null_mut, write_bytes};

// WARNING: macOS malloc keeps freed memory in the footprint unless `MallocSpaceEfficient=1` is set, and reads it before
// any code here runs, so a process without it starts itself again.
const SETTING: &CStr = c"MallocSpaceEfficient=1";
const NAME: &CStr = c"MallocSpaceEfficient";

#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static START: unsafe extern "C" fn(c_int, *const *const c_char, *const *const c_char) = start;

unsafe extern "C" {
    fn _NSGetExecutablePath(path: *mut c_char, size: *mut u32) -> c_int;
}

// WARNING: runs before `main`, before any code of the app.
unsafe extern "C" fn start(
    _count: c_int,
    arguments: *const *const c_char,
    environment: *const *const c_char,
) {
    let mut variables = vec![SETTING.as_ptr()];
    let mut next = environment;
    // SAFETY: the kernel ends the list with null, and its strings stay for the life of the process.
    unsafe {
        while !(*next).is_null() {
            if CStr::from_ptr(*next)
                .to_bytes()
                .strip_prefix(NAME.to_bytes())
                .is_some_and(|rest| rest.first() == Some(&b'='))
            {
                libc::unsetenv(NAME.as_ptr());
                return;
            }
            variables.push(*next);
            next = next.add(1);
        }
    }
    variables.push(std::ptr::null());
    let mut path = [0; 4096];
    let mut size = path.len() as u32;
    // SAFETY: the buffer has `size` bytes, and `execve` either replaces the process or fails and changes nothing.
    unsafe {
        if _NSGetExecutablePath(path.as_mut_ptr(), &mut size) == 0 {
            libc::execve(path.as_ptr(), arguments.cast(), variables.as_ptr().cast());
        }
    }
}

const SMALL: usize = 512;
const CLASSES: usize = SMALL / 16 + 1;
const KEPT: u32 = 1024 * 1024;

// WARNING: `MallocSpaceEfficient` makes malloc and free slow, and frames build and drop thousands of small blocks, so
// each thread keeps freed small blocks, up to 1 MB per size, until it is quiet.
struct Bins {
    heads: [*mut u8; CLASSES],
    bytes: [u32; CLASSES],
}

impl Bins {
    fn flush(&mut self) {
        for head in &mut self.heads {
            let mut next = std::mem::replace(head, null_mut());
            while !next.is_null() {
                // SAFETY: every kept block starts with the next one, and malloc gave it.
                unsafe {
                    let block = next;
                    next = block.cast::<*mut u8>().read();
                    libc::free(block.cast());
                }
            }
        }
        self.bytes = [0; CLASSES];
    }
}

impl Drop for Bins {
    fn drop(&mut self) {
        self.flush();
    }
}

thread_local! {
    static BINS: UnsafeCell<Bins> = const {
        UnsafeCell::new(Bins { heads: [null_mut(); CLASSES], bytes: [0; CLASSES] })
    };
}

pub(crate) fn flush() {
    // SAFETY: only this thread touches its bins, and `flush` calls no allocator method that uses them.
    let _ = BINS.try_with(|bins| unsafe { (*bins.get()).flush() });
}

pub(crate) struct Allocator;

fn small(layout: Layout) -> bool {
    layout.size() <= SMALL && layout.align() <= 16
}

fn rounded(class: usize) -> Layout {
    // SAFETY: the size is at most `SMALL` and 16 is a power of two.
    unsafe { Layout::from_size_align_unchecked(class << 4, 16) }
}

// SAFETY: small blocks are malloc blocks of their class size, so each fits its class on any thread, and `small` gives
// the same answer for both ends of a block, because they see the same layout.
// WARNING: these stay out of line, or fat LTO copies them to every allocation and the runtime grows by 900 KB.
unsafe impl GlobalAlloc for Allocator {
    #[inline(never)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !small(layout) {
            return unsafe { System.alloc(layout) };
        }
        let class = layout.size().div_ceil(16);
        let kept = BINS.try_with(|bins| {
            // SAFETY: only this thread touches its bins, and kept blocks start with the next one.
            unsafe {
                let bins = &mut *bins.get();
                let block = bins.heads[class];
                if !block.is_null() {
                    bins.heads[class] = block.cast::<*mut u8>().read();
                    bins.bytes[class] -= (class << 4) as u32;
                }
                block
            }
        });
        match kept {
            Ok(block) if !block.is_null() => block,
            _ => unsafe { System.alloc(rounded(class)) },
        }
    }

    #[inline(never)]
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        if !small(layout) {
            return unsafe { System.dealloc(block, layout) };
        }
        let class = layout.size().div_ceil(16);
        let kept = BINS.try_with(|bins| {
            // SAFETY: only this thread touches its bins, and the block has room for a pointer.
            unsafe {
                let bins = &mut *bins.get();
                let bytes = bins.bytes[class] + (class << 4) as u32;
                let room = bytes <= KEPT;
                if room {
                    block.cast::<*mut u8>().write(bins.heads[class]);
                    bins.heads[class] = block;
                    bins.bytes[class] = bytes;
                }
                room
            }
        });
        if kept != Ok(true) {
            unsafe { System.dealloc(block, rounded(class)) };
        }
    }

    #[inline(never)]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !small(layout) {
            return unsafe { System.alloc_zeroed(layout) };
        }
        let block = unsafe { self.alloc(layout) };
        if !block.is_null() {
            unsafe { write_bytes(block, 0, layout.size()) };
        }
        block
    }

    #[inline(never)]
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new = unsafe { Layout::from_size_align_unchecked(size, layout.align()) };
        match (small(layout), small(new)) {
            (false, false) => unsafe { System.realloc(block, layout, size) },
            (true, true) if layout.size().div_ceil(16) == size.div_ceil(16) => block,
            _ => {
                let next = unsafe { self.alloc(new) };
                if !next.is_null() {
                    unsafe {
                        copy_nonoverlapping(block, next, layout.size().min(size));
                        self.dealloc(block, layout);
                    }
                }
                next
            }
        }
    }
}
