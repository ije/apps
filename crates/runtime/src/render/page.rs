use crate::css::properties::Prop;
use crate::dom::{ABSENT, Document, Node};
use crate::style;
use std::sync::Arc;

// What a page keeps besides its document: its nodes, geometry, focus and selection, styles, layout, media and
// fonts, and what its next frame must do.
#[derive(Default)]
pub struct PageState {
    pub tree: crate::dom::Tree,
    pub geometry: crate::dom::Geometry,
    pub interaction: crate::dom::Interaction,
    pub(crate) styles: style::Styles,
    pub layout: super::layout::Layout,
    pub rasters: super::layout::Rasters,
    pub media: super::media::Media,
    pub fonts: super::fonts::Fonts,
    pub base: Option<url::Url>,
    pub(crate) dirty: bool,
    pub frame_requested: bool,
    pub(crate) wheel: bool,
    pub(crate) window: renderer::WindowState,
    pub pixel_ratio: f32,
    pub(crate) paint_requested: bool,
}

impl PageState {
    // The page shows in this window: its fonts, pixel ratio and frame.
    pub(crate) fn attach_window(&mut self, window: &renderer::Window) {
        self.fonts.system = Some(window.text_system().clone());
        self.pixel_ratio = window.scale_factor();
        self.window = window.window_state();
    }

    pub(crate) fn theme(&self) -> u32 {
        crate::native::theme_state()
            | ((self.pixel_ratio.clamp(0., 600.) * 100.).round() as u32) << 16
    }

    // Moves the mirror for the user scrolls since the last prepaint. See `Geometry::shift_scrolls`.
    pub(crate) fn shift_scrolls(&mut self) -> bool {
        let computed = self.styles.computed.as_ref();
        let top_layer = self.interaction.top_layer.is_empty();
        let walk = &self.styles.walk;
        self.geometry.shift_scrolls(
            walk,
            computed.map(|_| self.styles.epoch),
            self.pixel_ratio,
            |id, nodes| {
                let computed = computed?;
                // A transform, zoom or perspective on the scroll, around it or in it doesn't move boxes by the offset.
                let flat = |node: usize| {
                    computed.values.get(node).is_some_and(|style| {
                        [
                            Prop::TRANSFORM,
                            Prop::TRANSLATE,
                            Prop::ROTATE,
                            Prop::SCALE,
                            Prop::PERSPECTIVE,
                        ]
                        .into_iter()
                        .all(|prop| style.get(prop).is_none_or(|value| value == "none"))
                            && style.get(Prop::ZOOM).is_none_or(|value| {
                                matches!(value, "normal" | "reset" | "1" | "100%")
                            })
                    })
                };
                let contains = super::effects::establishes(&computed.values[id]).absolute;
                Some(
                    top_layer
                        && std::iter::successors(Some(id as u32), |&node| {
                            walk.parent(node as usize)
                                .filter(|&parent| parent != ABSENT)
                        })
                        .all(|node| flat(node as usize))
                        && nodes.iter().all(|&(node, _)| {
                            let position = computed
                                .values
                                .get(node as usize)
                                .map_or("static", |style| {
                                    style.get(Prop::POSITION).unwrap_or("static")
                                });
                            !matches!(position, "fixed" | "sticky")
                                && (contains || position != "absolute")
                                && flat(node as usize)
                        }),
                )
            },
        )
    }

    // Free removed nodes' data now: row churn can outpace GC and retain more than the live document.
    pub(crate) fn drop_removed(&mut self, doc: &Document) {
        let gone = self.tree.take_removed(doc);
        self.styles.cache.forget(&gone);
        for id in gone {
            self.layout.forget_removed(id);
        }
    }

    // Frees the detached subtrees that no wrapper holds. Returns the media elements among them and all freed
    // nodes.
    pub(crate) fn release(&mut self, doc: &mut Document) -> (Vec<usize>, Vec<usize>) {
        self.drop_removed(doc);
        let freed = self.tree.collect(doc);
        let retired = self.media.retired.len();
        let mut canvases = Vec::new();
        for &id in &freed {
            if matches!(
                doc.nodes[id].tag.as_str(),
                "canvas" | "apps-canvas" | "video" | "audio" | "terminal"
            ) {
                canvases.push(id);
            }
            self.tree.free(doc, id);
            self.geometry.forget(id);
            self.layout.forget(id);
            self.media.forget(id);
            self.rasters.forget(id, &mut self.media.retired);
            self.styles.cache.adopted.remove(&id);
            self.styles.motion.forget(id);
            self.interaction.forget(id);
            if doc.target == Some(id) {
                doc.target = None;
            }
        }
        if !freed.is_empty() {
            self.styles.cache.forget(&freed);
            let gone: rustc_hash::FxHashSet<usize> = freed.iter().copied().collect();
            self.interaction.forget_all(&gone);
            self.layout.forget_all(&gone);
            self.rasters.forget_all(&gone, &mut self.media.retired);
            self.dirty |= self.media.retired.len() > retired;
        }
        (canvases, freed)
    }

    pub(crate) fn trim(&mut self, doc: &mut Document) -> bool {
        self.drop_removed(doc);
        let before = doc.nodes.len();
        let len = self.tree.trim(before);
        self.styles.walk = Default::default();
        self.styles.select_marks = Vec::new();
        self.styles.cache.trim(len);
        self.styles.motion.trim(len);
        self.layout.trim(len);
        self.geometry.mirror.shrink(len);
        if len < before {
            doc.nodes.truncate(len);
            self.styles.invalidate();
        }
        len < before
    }

    pub(crate) fn update_containers(&mut self, doc: &Document) -> bool {
        let changed = self
            .styles
            .cache
            .set_containers(&self.layout.containers, doc);
        if changed {
            self.styles.invalidate();
        }
        changed
    }

    pub(crate) fn allocate(&mut self, doc: &mut Document, node: Node) -> usize {
        let id = self.tree.slot(doc);
        self.place(doc, id, node);
        id
    }

    pub(crate) fn place(&mut self, doc: &mut Document, id: usize, node: Node) {
        self.tree.place(doc, id, node);
        self.styles.place(id, doc.nodes.len());
    }

    // The styles of the page now: the last pass when it still holds, or a new pass.
    pub(crate) fn restyle(
        &mut self,
        doc: &mut Document,
        viewport: [f32; 2],
    ) -> std::rc::Rc<style::ComputedStyles> {
        let scale = self.fonts.system.is_some().then_some(self.pixel_ratio);
        let pending = match self.styles.begin(
            doc,
            viewport,
            self.theme(),
            scale,
            &mut self.geometry.mirror,
        ) {
            Ok(kept) => return kept,
            Err(pending) => pending,
        };
        self.drop_removed(doc);
        self.styles.update(
            doc,
            pending,
            style::Page {
                shadows: &self.tree.shadows,
                hosts: &self.tree.hosts,
                top_layer: &self.interaction.top_layer,
                font_aliases: &self.fonts.aliases,
                text_system: self.fonts.system.as_deref(),
                bounds: &mut self.geometry.mirror,
            },
        )
    }

    // Shows a new frame or image of node `id`, and returns the old one when it can be freed now.
    pub(crate) fn show_frame(
        &mut self,
        id: usize,
        image: Option<Arc<renderer::RenderImage>>,
    ) -> Option<Arc<renderer::RenderImage>> {
        if self.media.recycled.is_empty() {
            self.media.retired_canvases.clear();
        }
        // A poster that decodes later must not replace the frame.
        self.media.decoding.remove(&id);
        let size = image.as_ref().map(|image| image.size());
        match size {
            Some(size) => self
                .layout
                .measurements
                .naturals
                .insert(id, [size.width as u32 as f32, size.height as u32 as f32]),
            None => self.layout.measurements.naturals.remove(&id),
        };
        let old = match image {
            Some(image) => self.media.images.insert(id, image),
            None => self.media.images.remove(&id),
        };
        if old.as_ref().map(|old| old.size()) != size {
            self.layout.relayout(id);
        }
        self.dirty = true;
        // Only the first replaced frame can be on the GPU. Recycle later frames immediately to bound memory in
        // unrendered windows.
        match old {
            Some(old) if self.media.retired_canvases.insert(id) => {
                self.media.recycled.push((id, old));
                None
            }
            old => old,
        }
    }
}
