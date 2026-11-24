use crate::css;
use crate::dom::Document;
use crate::style::ComputedStyles;

#[derive(Default)]
pub(crate) struct Measurements {
    // The last key field marks the column and row extents of a grid layout, which intrinsic sizes repeat.
    #[allow(clippy::type_complexity)]
    pub(crate) sizes: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>, bool), [f32; 4]>,
    // The top and bottom margins of each measured box, as [positive, negative] after they collapse with the
    // margins of children that they adjoin.
    #[allow(clippy::type_complexity)]
    pub(crate) margins: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>), [[f32; 2]; 2]>,
    // How far the first line of a text box reaches above its strut, by the keys of `sizes`.
    #[allow(clippy::type_complexity)]
    pub(crate) leads: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>), f32>,
    pub(crate) pixel_ratio: f32,
    pub(crate) naturals: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) texts:
        rustc_hash::FxHashMap<usize, Option<std::rc::Rc<crate::render::inline::InlineText>>>,
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
    /// Keeps the measured sizes, margins and leads of the boxes that `keep` takes.
    pub(crate) fn retain(&mut self, keep: impl Fn(usize) -> bool) {
        self.sizes.retain(|&(id, ..), _| keep(id));
        self.margins.retain(|&(id, ..), _| keep(id));
        self.leads.retain(|&(id, ..), _| keep(id));
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
