use crate::css;
use crate::dom::Document;
use crate::style::ComputedStyles;

type Key = (usize, Option<u32>, Option<u32>);

// The measures of a box for one width and height key.
#[derive(Default)]
pub(crate) struct Measure {
    key: (Option<u32>, Option<u32>),
    // The intrinsic sizes, and apart from them the column and row extents of a grid layout.
    sizes: [Option<[f32; 4]>; 2],
    // The top and bottom margins of the box, as [positive, negative] after they collapse with the margins of
    // children that they adjoin.
    margins: Option<[[f32; 2]; 2]>,
    // How far the first line of a text box reaches above its strut.
    lead: Option<f32>,
}

#[derive(Default)]
pub(crate) struct Measurements {
    // By node, so a changed node forgets its measures without a walk over all of them.
    pub(crate) boxes: rustc_hash::FxHashMap<usize, Vec<Measure>>,
    pub(crate) pixel_ratio: f32,
    pub(crate) naturals: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) texts:
        rustc_hash::FxHashMap<usize, Option<std::rc::Rc<crate::render::inline::InlineText>>>,
    // The last lines that a measure shaped from each text of `texts`, for one build.
    #[allow(clippy::type_complexity)]
    pub(crate) lines: rustc_hash::FxHashMap<
        usize,
        (
            std::rc::Rc<crate::render::inline::InlineText>,
            renderer::Pixels,
            renderer::Wrapping,
            std::rc::Rc<Vec<renderer::TextLine>>,
        ),
    >,
    // The text nodes in runs of more than one in flex and grid containers: `None` for a later node, which the
    // first builds, and for the first, whether the run has text other than white space.
    pub(crate) joined: rustc_hash::FxHashMap<usize, Option<bool>>,
    // The containers whose runs this build noted.
    pub(crate) noted: rustc_hash::FxHashSet<usize>,
    // Use style-address keys only during grid layout, while styles outlive the map.
    #[allow(clippy::type_complexity)]
    pub(crate) edges:
        Option<rustc_hash::FxHashMap<(*const css::properties::Style, u32), [[f32; 4]; 3]>>,
    // The boxes with `content-visibility: auto`, true while they are near the view and show their contents.
    pub(crate) shown: rustc_hash::FxHashMap<usize, bool>,
    // For `contain-intrinsic-size: auto`: the last content size of a box that showed its contents, and whether it
    // skipped them at its last build.
    pub(crate) remembered: rustc_hash::FxHashMap<usize, (Option<[f32; 2]>, bool)>,
    // Boxes that show their contents until the next frame, because a script reads the geometry of a node in them.
    pub(crate) forced: rustc_hash::FxHashSet<usize>,
    // Boxes that start or stop to skip their contents, which the next build builds again.
    pub(crate) waking: Vec<usize>,
}

impl Measurements {
    fn find(&self, (id, width, height): Key) -> Option<&Measure> {
        let list = self.boxes.get(&id)?;
        list.iter().find(|measure| measure.key == (width, height))
    }

    fn find_mut(&mut self, (id, width, height): Key) -> &mut Measure {
        let list = self.boxes.entry(id).or_default();
        let at = match list
            .iter()
            .position(|measure| measure.key == (width, height))
        {
            Some(at) => at,
            None => {
                list.push(Measure {
                    key: (width, height),
                    ..Measure::default()
                });
                list.len() - 1
            }
        };
        &mut list[at]
    }

    pub(crate) fn size(
        &self,
        (id, width, height, extent): (usize, Option<u32>, Option<u32>, bool),
    ) -> Option<[f32; 4]> {
        self.find((id, width, height))?.sizes[usize::from(extent)]
    }

    pub(crate) fn set_size(
        &mut self,
        (id, width, height, extent): (usize, Option<u32>, Option<u32>, bool),
        size: [f32; 4],
    ) {
        self.find_mut((id, width, height)).sizes[usize::from(extent)] = Some(size);
    }

    pub(crate) fn margins(&self, key: Key) -> Option<[[f32; 2]; 2]> {
        self.find(key)?.margins
    }

    pub(crate) fn set_margins(&mut self, key: Key, margins: [[f32; 2]; 2]) {
        self.find_mut(key).margins = Some(margins);
    }

    pub(crate) fn lead(&self, key: Key) -> Option<f32> {
        self.find(key)?.lead
    }

    pub(crate) fn set_lead(&mut self, key: Key, lead: f32) {
        self.find_mut(key).lead = Some(lead);
    }

    /// Like CSS, each run of text nodes in a flex or grid container is one anonymous item. Its first node builds it
    /// with the text of all. `again` notes the runs of `children` also when this build noted them.
    pub(crate) fn note_runs(
        &mut self,
        doc: &Document,
        styles: &ComputedStyles,
        container: usize,
        children: &[usize],
        again: bool,
    ) {
        if !self.noted.insert(container) && !again {
            return;
        }
        let mut at = 0;
        while at < children.len() {
            if doc.nodes[children[at]].tag != "#text" {
                at += 1;
                continue;
            }
            let end = at
                + children[at..]
                    .iter()
                    .take_while(|&&node| {
                        matches!(doc.nodes[node].tag.as_str(), "#text" | "#comment")
                    })
                    .count();
            let run: Vec<usize> = children[at..end]
                .iter()
                .copied()
                .filter(|&node| doc.nodes[node].tag == "#text")
                .collect();
            if let [first, rest @ ..] = &run[..]
                && !rest.is_empty()
            {
                let text = run
                    .iter()
                    .any(|&node| !doc.nodes[node].text.trim_ascii().is_empty());
                self.joined.insert(*first, Some(text));
                self.joined.extend(rest.iter().map(|&node| (node, None)));
                self.texts.insert(
                    *first,
                    crate::render::inline::inline_text(doc, styles, *first, rest, &[], [true; 2])
                        .map(std::rc::Rc::new),
                );
            } else {
                self.joined.remove(&children[at]);
            }
            at = end;
        }
    }

    /// Whether a text child of a flex or grid container makes an item: not for a later node of a run, and for
    /// white space only with `preserve` or other text in its run.
    pub(crate) fn text_item(&self, doc: &Document, child: usize, preserve: bool) -> bool {
        let blank = doc.nodes[child].text.trim_ascii().is_empty();
        match self.joined.get(&child) {
            Some(None) => false,
            Some(&Some(text)) => text || preserve || !blank,
            None => preserve || !blank,
        }
    }
}
