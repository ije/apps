use crate::css::{properties::Registry, sheet::Stylesheet};
use crate::dom::Document;
use crate::style::StyleCache;
use std::collections::HashMap;
use std::rc::Rc;
pub(crate) type Derived = HashMap<
    *const Stylesheet,
    (
        std::rc::Weak<Stylesheet>,
        Rc<Stylesheet>,
        Vec<usize>,
        Vec<usize>,
    ),
>;

pub(crate) fn derive<R, E>(
    derived: &mut Derived,
    sheet: &mut Rc<Stylesheet>,
    top: Option<(usize, bool)>,
    change: impl FnOnce(&mut Stylesheet) -> Result<R, E>,
) -> Result<R, E> {
    let before = Rc::as_ptr(sheet);
    let base = (Rc::strong_count(sheet) > 1).then(|| sheet.clone());
    let result = change(Rc::make_mut(sheet));
    let previous = derived.remove(&before);
    if top.is_some() || result.is_err() {
        let Some((base, mut inserted, mut removed)) = base
            .map(|base| (base, Vec::new(), Vec::new()))
            .or_else(|| previous.map(|(_, base, inserted, removed)| (base, inserted, removed)))
        else {
            return result;
        };
        match top.filter(|_| result.is_ok()) {
            Some((index, true)) => {
                for at in &mut inserted {
                    *at += usize::from(*at >= index);
                }
                inserted.push(index);
            }
            Some((index, false)) => {
                match inserted.iter().position(|&at| at == index) {
                    Some(position) => {
                        inserted.swap_remove(position);
                    }
                    None => {
                        let (mut old, mut current) = (0, 0);
                        while removed.contains(&old)
                            || inserted.contains(&current)
                            || current != index
                        {
                            if removed.contains(&old) {
                                old += 1;
                            } else if inserted.contains(&current) {
                                current += 1;
                            } else {
                                old += 1;
                                current += 1;
                            }
                        }
                        removed.push(old);
                    }
                }
                for at in &mut inserted {
                    *at -= usize::from(*at > index);
                }
            }
            None => {}
        }
        derived.insert(
            Rc::as_ptr(sheet),
            (Rc::downgrade(sheet), base, inserted, removed),
        );
    }
    result
}

#[derive(Clone)]
pub(crate) struct Cssom {
    pub(crate) used: u64,
    pub(crate) text: Rc<str>,
    pub(crate) bundled: bool,
    pub(crate) disabled: bool,
    pub(crate) sheet: Rc<Stylesheet>,
}
pub(crate) type Sheet = (u64, Option<u64>, Rc<str>, Rc<Stylesheet>);

impl StyleCache {
    pub(crate) fn sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    ) -> Option<Rc<Stylesheet>> {
        let node = &doc.nodes[id];
        let pass = self.pass;
        let bundled = node.stylesheet.as_ref().or(match node.children[..] {
            [text] => doc.nodes[text].stylesheet.as_ref(),
            _ => None,
        });
        let same = |text: &str| {
            let mut rest = text;
            let mut pending = vec![id];
            while let Some(child) = pending.pop() {
                let node = &doc.nodes[child];
                if child != id && node.tag == "#comment" {
                    continue;
                }
                match rest.strip_prefix(node.text.as_str()) {
                    Some(tail) if child == id || node.stylesheet.is_none() => rest = tail,
                    _ => return false,
                }
                pending.extend(node.children.iter().rev());
            }
            rest.is_empty()
        };
        if let Some(edited) = self.cssom.get_mut(&id).filter(|edited| {
            edited.bundled == bundled.is_some() && (bundled.is_some() || same(&edited.text))
        }) {
            edited.used = pass;
            return (!edited.disabled).then(|| edited.sheet.clone());
        }
        self.cssom.remove(&id);
        if let Some(sheet) = bundled {
            return Some(sheet.clone());
        }
        let stamp = node
            .children
            .iter()
            .try_fold(doc.nodes.revision(id), |stamp, &child| {
                doc.nodes[child]
                    .children
                    .is_empty()
                    .then(|| stamp.max(doc.nodes.revision(child)))
            });
        if let Some((used, old, text, sheet)) = self.sheets.get_mut(&id)
            && (stamp.is_some() && *old == stamp || same(text))
        {
            *used = pass;
            *old = stamp;
            return Some(sheet.clone());
        }
        let text = doc.text(id);
        let (text, sheet) = match cache.get_key_value(text.as_str()) {
            Some((text, sheet)) => (text.clone(), sheet.clone()),
            None => {
                let previous = self.sheets.get(&id).filter(|(_, _, old, _)| {
                    let (mut depth, mut quote, mut comment, mut escaped) = (0i32, 0u8, None, false);
                    let bytes = old.as_bytes();
                    for (index, &byte) in bytes.iter().enumerate() {
                        match byte {
                            _ if escaped => escaped = false,
                            b'\\' => escaped = true,
                            b'/' if comment.is_some_and(|open| {
                                index > open + 1 && bytes[index - 1] == b'*'
                            }) =>
                            {
                                comment = None
                            }
                            _ if comment.is_some() => {}
                            b'*' if quote == 0 && index > 0 && bytes[index - 1] == b'/' => {
                                comment = Some(index)
                            }
                            b'"' | b'\'' if quote == 0 => quote = byte,
                            _ if quote == byte => quote = 0,
                            b'{' if quote == 0 => depth += 1,
                            b'}' if quote == 0 => depth -= 1,
                            _ => {}
                        }
                    }
                    text.starts_with(&**old)
                        && depth == 0
                        && quote == 0
                        && comment.is_none()
                        && !escaped
                        && old.trim_end().ends_with(['}', ';'])
                });
                let sheet = match previous {
                    Some((_, _, old, base)) => {
                        let mut rules = base.0.clone();
                        let start = rules.len();
                        rules.extend(Stylesheet::parse(&text[old.len()..]).0);
                        let sheet = Rc::new(Stylesheet(rules));
                        self.derived.insert(
                            Rc::as_ptr(&sheet),
                            (
                                Rc::downgrade(&sheet),
                                base.clone(),
                                (start..sheet.0.len()).collect(),
                                Vec::new(),
                            ),
                        );
                        sheet
                    }
                    None => Rc::new(Stylesheet::parse(&text)),
                };
                let text: Rc<str> = text.into();
                cache.insert(text.clone(), sheet.clone());
                (text, sheet)
            }
        };
        self.sheets.insert(id, (pass, stamp, text, sheet.clone()));
        Some(sheet)
    }

    pub(crate) fn edit_constructed(
        &mut self,
        id: u32,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        match self.constructed.get_mut(&id) {
            Some(sheet) => derive(&mut self.derived, sheet, top, change),
            None => Err("InvalidStateError"),
        }
    }

    pub(crate) fn registry(&self) -> Rc<Registry> {
        self.environment.registry.clone()
    }

    pub(crate) fn edit_sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet, &mut bool) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        let current = self.sheet(doc, id, cache);
        let (mut sheet, mut disabled) = self.cssom.remove(&id).map_or_else(
            || (current.unwrap_or_default(), false),
            |edited| (edited.sheet, edited.disabled),
        );
        let result = derive(&mut self.derived, &mut sheet, top, |sheet| {
            change(sheet, &mut disabled)
        });
        let mut text = String::new();
        let mut pending = vec![id];
        while let Some(child) = pending.pop() {
            let node = &doc.nodes[child];
            if child == id || node.tag != "#comment" {
                text.push_str(&node.text);
                pending.extend(node.children.iter().rev());
            }
        }
        let bundled = doc.nodes[id].stylesheet.is_some()
            || matches!(doc.nodes[id].children[..], [child] if doc.nodes[child].stylesheet.is_some());
        self.cssom.insert(
            id,
            Cssom {
                used: self.pass,
                text: text.into(),
                bundled,
                disabled,
                sheet,
            },
        );
        result
    }
}
