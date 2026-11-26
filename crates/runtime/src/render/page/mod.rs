mod fonts;

use crate::css::properties::Prop;
use crate::dom::{ABSENT, Document, Node};
use crate::style;
use anyhow::{Context, Result, bail, ensure};
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
    pub rasters: super::raster::Rasters,
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
                            Prop::OFFSET_PATH,
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

    // Places a node whose parent is already in the arena.
    pub(crate) fn place_attached(&mut self, doc: &mut Document, id: usize, node: Node) {
        doc.nodes[id] = node;
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

    // A change to node `id` restyles the page when the node is in the tree.
    fn mutated(&mut self, doc: &Document, id: usize) {
        if self.styles.computed.is_some() && self.tree.reaches(doc, id, 0) {
            self.styles.invalidate();
        }
    }

    // Applies a batch of tree changes from dom.js. Returns whether the page changed, and the errors.
    pub(crate) fn batch(
        &mut self,
        doc: &mut Document,
        codes: &[u8],
        mut text: &str,
    ) -> (bool, Vec<String>) {
        let node = |doc: &Document, value: i32| {
            usize::try_from(value)
                .ok()
                .filter(|&id| doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty()))
                .context("Invalid node")
        };
        fn prune(doc: &mut Document, parent: usize) {
            let mut children = std::mem::take(&mut doc.nodes[parent].children);
            children.retain(|&child| doc.nodes[child].parent == Some(parent));
            doc.nodes[parent].children = children;
        }
        fn insert(
            doc: &mut Document,
            (parent, before): (usize, Option<usize>),
            run: &mut Vec<usize>,
        ) {
            let children = &mut doc.nodes[parent].children;
            let index = before
                .and_then(|before| children.iter().position(|&id| id == before))
                .unwrap_or(children.len());
            children.splice(index..index, run.drain(..));
        }
        let (mut dirty, mut errors) = (false, Vec::new());
        // Parent links are current during a batch. `stale` has obsolete child entries. `run` has children
        // awaiting insertion.
        let (mut stale, mut run, mut runs) = (None, None, Vec::new());
        // The last parent and child, and whether they are in the document.
        let mut linked: Option<([usize; 2], bool)> = None;
        'ops: for op in codes.as_chunks::<16>().0 {
            let [code, a, b, c] =
                [0, 4, 8, 12].map(|i| i32::from_ne_bytes([op[i], op[i + 1], op[i + 2], op[i + 3]]));
            let mut parts = ["", ""];
            for (part, units) in parts.iter_mut().zip([b, c]).take(match code {
                0 | 4 => 2,
                1 | 5 | 7 | 11 => 1,
                _ => 0,
            }) {
                let units = usize::try_from(units).unwrap_or(usize::MAX);
                let end = if text.as_bytes().get(..units).is_some_and(<[u8]>::is_ascii) {
                    units
                } else {
                    let (mut end, mut count) = (0, 0);
                    for character in text.chars() {
                        if count >= units {
                            break;
                        }
                        count += character.len_utf16();
                        end += character.len_utf8();
                    }
                    if count != units {
                        errors.push("Invalid DOM batch".into());
                        break 'ops;
                    }
                    end
                };
                (*part, text) = text.split_at(end);
            }
            if !matches!(code, 0 | 2 | 4 | 5 | 7 | 8 | 10) {
                linked = None;
            }
            let result = (|| -> Result<bool> {
                Ok(match code {
                    0 => {
                        let (id, tag) = (usize::try_from(a).context("Invalid node")?, parts[0]);
                        ensure!(
                            matches!(
                                tag,
                                "#text" | "#comment" | "#fragment" | "#document" | "#doctype"
                            ) || !tag.is_empty()
                                && !tag.starts_with('#')
                                && !tag.contains(['<', '>', '/', ' ', '\0']),
                            "Invalid tag"
                        );
                        ensure!(self.tree.claim(id), "Invalid node");
                        self.place(
                            doc,
                            id,
                            // Pages lower-case HTML names themselves; SVG and other namespaces keep their case.
                            Node {
                                tag: tag.into(),
                                text: parts[1].into(),
                                ..Node::default()
                            },
                        );
                        false
                    }
                    1 => {
                        let (id, value) = (node(doc, a)?, parts[0]);
                        if let Some(pending) = run.take_if(|&mut (parent, _)| parent == id) {
                            insert(doc, pending, &mut runs);
                        }
                        if stale.take_if(|&mut parent| parent == id).is_some() {
                            prune(doc, id);
                        }
                        let current = &doc.nodes[id];
                        if current.children.is_empty() && current.text == value {
                            return Ok(false);
                        }
                        if !(current.tag == "#text"
                            && current.text.is_empty() == value.is_empty()
                            && renderer::right_to_left(&current.text)
                                == renderer::right_to_left(value)
                            && current
                                .parent
                                .is_none_or(|parent| doc.nodes[parent].tag != "style"))
                            && self.styles.computed.is_some()
                            && self.tree.reaches(doc, id, 0)
                        {
                            self.styles.invalidate();
                        }
                        if doc.nodes[id].children.is_empty()
                            && let Some(current) = doc.nodes.edit(id)
                        {
                            current.text = value.into();
                            current.stylesheet = None;
                            return Ok(true);
                        }
                        doc.nodes[id].text = value.into();
                        doc.nodes[id].stylesheet = None;
                        self.tree.detach_children(doc, id);
                        true
                    }
                    2 => {
                        let (parent, child) = (node(doc, a)?, node(doc, b)?);
                        ensure!(child > 0, "Invalid child");
                        let tag = &doc.nodes[parent].tag;
                        ensure!(
                            !tag.starts_with('#') || tag == "#document" || tag == "#fragment",
                            "Invalid parent"
                        );
                        let before = usize::try_from(c).ok();
                        ensure!(
                            before.is_none_or(|before| doc
                                .nodes
                                .get(before)
                                .is_some_and(|node| node.parent == Some(parent))),
                            "Reference is not a child"
                        );
                        // A node without children is above nothing, so building a deep tree downward walks up for
                        // no node.
                        let leaf = child != parent
                            && doc.nodes[child].children.is_empty()
                            && run.is_none_or(|(pending, _)| pending != child)
                            && !self.tree.shadows.contains_key(&child);
                        ensure!(
                            leaf || !self.tree.reaches(doc, parent, child),
                            "Cannot create a DOM cycle"
                        );
                        if self.styles.computed.is_some() {
                            let connected = match linked {
                                Some((known, connected)) if known.contains(&parent) => connected,
                                _ => self.tree.reaches(doc, parent, 0),
                            };
                            if connected || self.tree.reaches(doc, child, 0) {
                                self.styles.invalidate();
                            }
                            linked = Some(([parent, child], connected));
                        }
                        if before == Some(child) {
                            return Ok(false);
                        }
                        if let Some(pending) = run.take_if(|&mut key| key != (parent, before)) {
                            insert(doc, pending, &mut runs);
                        }
                        if stale.take_if(|&mut stale| stale == parent).is_some() {
                            prune(doc, parent);
                        }
                        match doc.nodes[child].parent {
                            Some(old) if old == parent => {
                                if let Some(pending) = run.take() {
                                    insert(doc, pending, &mut runs);
                                }
                                let children = &mut doc.nodes[parent].children;
                                if let Some(index) = children.iter().position(|&id| id == child) {
                                    children.remove(index);
                                }
                                let index = before
                                    .and_then(|before| children.iter().position(|&id| id == before))
                                    .unwrap_or(children.len());
                                children.insert(index, child);
                                return Ok(true);
                            }
                            Some(old) => {
                                if let Some(previous) = stale.replace(old)
                                    && previous != old
                                {
                                    prune(doc, previous);
                                }
                            }
                            None => {}
                        }
                        doc.nodes[child].parent = Some(parent);
                        run = Some((parent, before));
                        runs.push(child);
                        true
                    }
                    3 => {
                        let parent = node(doc, a)?;
                        let child = usize::try_from(b)
                            .ok()
                            .filter(|&child| {
                                doc.nodes
                                    .get(child)
                                    .is_some_and(|node| node.parent == Some(parent))
                            })
                            .context("Node is not a child")?;
                        if let Some(pending) = run.take() {
                            insert(doc, pending, &mut runs);
                        }
                        if let Some(previous) = stale.replace(parent)
                            && previous != parent
                        {
                            prune(doc, previous);
                        }
                        self.mutated(doc, parent);
                        self.tree.detach(doc, child);
                        true
                    }
                    4 | 5 => {
                        let (id, key) = (node(doc, a)?, parts[0]);
                        let value = (code == 4).then_some(parts[1]);
                        if doc.nodes[id].attrs.get(key).map(String::as_str) == value {
                            return Ok(false);
                        }
                        self.mutated(doc, id);
                        // Kept walks of the tree read these attributes, so their edits must use `get_mut` and
                        // count as tree changes.
                        let current =
                            if matches!(key, "slot" | "name" | "contenteditable" | "inert") {
                                doc.nodes.get_mut(id)
                            } else {
                                doc.nodes.edit(id)
                            };
                        if let Some(current) = current {
                            match value {
                                Some(value) => current.attrs.set(key, value),
                                None => drop(current.attrs.remove(key)),
                            }
                            if key == "style" {
                                current.inline = None;
                            }
                        }
                        true
                    }
                    6 => {
                        let (host, root) = (node(doc, a)?, node(doc, b)?);
                        self.tree.attach_shadow(doc, host, root)?;
                        self.mutated(doc, host);
                        true
                    }
                    7 => {
                        let id = node(doc, a)?;
                        let value = (c == 0).then(|| parts[0].to_owned());
                        let current = &doc.nodes[id];
                        if current.value == value {
                            return Ok(false);
                        }
                        let fallback = current.attrs.get("value").map(String::as_str);
                        // Selectors read whether the value is empty, and `dir=auto` its direction.
                        let read = |value: Option<&String>| {
                            let value = value.map(String::as_str).or(fallback).unwrap_or("");
                            (value.is_empty(), renderer::right_to_left(value))
                        };
                        if self.styles.computed.is_some()
                            && read(current.value.as_ref()) != read(value.as_ref())
                            && self.tree.reaches(doc, id, 0)
                        {
                            self.styles.invalidate();
                        }
                        if let Some(current) = doc.nodes.edit(id) {
                            current.value = value;
                        }
                        true
                    }
                    8 => {
                        use crate::dom::{
                            BROKEN, CHECKED, DIRTY, FORM_ASSOCIATED, FULLSCREEN, INDETERMINATE,
                            INVALID, LEAVING, MODAL, POPOVER, TOP_LAYER, USER,
                        };
                        let id = node(doc, a)?;
                        let bits = [
                            CHECKED,
                            DIRTY,
                            MODAL,
                            POPOVER,
                            FULLSCREEN,
                            BROKEN,
                            INDETERMINATE,
                            INVALID,
                            USER,
                            FORM_ASSOCIATED,
                        ]
                        .into_iter()
                        .enumerate()
                        .fold(0, |bits, (index, bit)| {
                            bits | if b & 1 << index != 0 { bit } else { 0 }
                        });
                        let mask = CHECKED
                            | DIRTY
                            | MODAL
                            | POPOVER
                            | FULLSCREEN
                            | BROKEN
                            | INDETERMINATE
                            | INVALID
                            | USER
                            | FORM_ASSOCIATED;
                        let old = doc.nodes[id].state;
                        if old & mask == bits {
                            return Ok(false);
                        }
                        let leaving = if bits & TOP_LAYER == 0
                            && self.fonts.system.is_some()
                            && self.interaction.top_layer.contains(&id)
                        {
                            old & LEAVING | (old & TOP_LAYER) << 5
                        } else {
                            0
                        };
                        self.mutated(doc, id);
                        if let Some(node) = doc.nodes.edit(id) {
                            node.state = node.state & !(mask | LEAVING) | bits | leaving;
                        }
                        true
                    }
                    9 => {
                        let id = node(doc, a)?;
                        if b != 0 || doc.nodes[id].state & crate::dom::LEAVING == 0 {
                            self.interaction.top_layer.retain(|&other| other != id);
                        }
                        if b != 0 {
                            self.interaction.top_layer.push(id);
                        }
                        true
                    }
                    // A copy of a node for `cloneNode`: tag, attributes, and the text of text and comments.
                    10 => {
                        let (id, from) =
                            (usize::try_from(a).context("Invalid node")?, node(doc, b)?);
                        ensure!(self.tree.claim(id), "Invalid node");
                        let from = &doc.nodes[from];
                        let text = match &from.stylesheet {
                            Some(sheet) if from.tag == "#text" => sheet.source(),
                            _ if from.tag.starts_with('#') => from.text.clone(),
                            _ => Default::default(),
                        };
                        let copy = Node {
                            tag: from.tag.clone(),
                            text,
                            attrs: from.attrs.clone(),
                            ..Node::default()
                        };
                        self.place(doc, id, copy);
                        false
                    }
                    // The custom states of an element, for `:state()`.
                    11 => {
                        let id = node(doc, a)?;
                        self.mutated(doc, id);
                        if let Some(node) = doc.nodes.edit(id) {
                            node.value = (!parts[0].is_empty()).then(|| parts[0].to_owned());
                        }
                        true
                    }
                    _ => bail!("Unknown DOM operation"),
                })
            })();
            match result {
                // Offscreen canvas changes must not dirty the page or schedule frames.
                Ok(changed) => {
                    dirty |= changed
                        && usize::try_from(a)
                            .ok()
                            .and_then(|id| doc.nodes.get(id))
                            .is_none_or(|node| node.tag != "apps-canvas");
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        if let Some(pending) = run {
            insert(doc, pending, &mut runs);
        }
        if let Some(parent) = stale {
            prune(doc, parent);
        }
        (dirty, errors)
    }
}
