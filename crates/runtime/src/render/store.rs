use super::PageState;
use crate::dom::Document;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub(crate) type Assets = Arc<HashMap<String, PathBuf>>;

#[derive(Default)]
pub struct Page {
    pub doc: Document,
    pub state: PageState,
    pub dirty: bool,
}

// The pages of a host and the window data that their views read and write. The host and the views share it.
#[derive(Default)]
pub struct Store {
    pub pages: rustc_hash::FxHashMap<u64, Page>,
    pub viewports: HashMap<u64, [f32; 2]>,
    pub resized: Vec<u64>,
    pub backgrounds: HashMap<u64, (u32, u8)>,
    pub graphics: HashMap<u64, crate::web::gpu::Gpu>,
    pub media: HashMap<u64, crate::web::media::Media>,
    // The same app as the host's.
    pub app: std::rc::Rc<crate::bundle::AppInfo>,
    // The state that views of a closed page still write to until they go. Nothing reads it back.
    pub(crate) closed: PageState,
}

impl Store {
    pub fn page_state(&self, id: u64) -> Option<&PageState> {
        self.pages.get(&id).map(|page| &page.state)
    }

    pub fn page_state_mut(&mut self, id: u64) -> Option<&mut PageState> {
        self.pages.get_mut(&id).map(|page| &mut page.state)
    }

    // The state of the page in window `id`.
    pub fn state(&mut self, id: u64) -> &mut PageState {
        match self.pages.get_mut(&id) {
            Some(page) => &mut page.state,
            None => &mut self.closed,
        }
    }

    // The document of the page in window `id`, when it is open, its state, and the app assets.
    pub(crate) fn page_parts(&mut self, id: u64) -> (Option<&Document>, &mut PageState, &Assets) {
        match self.pages.get_mut(&id) {
            Some(page) => (Some(&page.doc), &mut page.state, &self.app.assets),
            None => (None, &mut self.closed, &self.app.assets),
        }
    }
}
