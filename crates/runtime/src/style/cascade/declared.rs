use crate::css::properties::{Prop, Style};
use crate::css::sheet::Declarations;
use crate::dom::Node;
use crate::style::cascade::memo::StyleKey;
use crate::style::cascade::node::NodePass;
use crate::style::cascade::rank::{ANIMATION, AUTHOR, CascadeRank, HINT, UNLAYERED, physical, put};
use crate::style::cascade::scope::StyleScope;
use crate::style::cascade::shadow::{ELEMENT, ShadowRule};
use bbtree::unsync::BBTreeMap;
use std::borrow::Cow;
use std::rc::Rc;

impl NodePass<'_> {
    // The cascade of the declarations of a node: its rules, hints, inline style, animations and the rules of its
    // shadow trees, with logical properties made physical. Also returns whether the node decorates its text.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cascade(
        &self,
        node: &Node,
        key: &StyleKey,
        scoped: &StyleScope,
        shadow: Option<&Rc<[ShadowRule]>>,
        animated: Option<&Style>,
        scheme: Option<&Rc<str>>,
        style: &mut Style,
    ) -> (BBTreeMap<Prop, (CascadeRank, Rc<str>)>, bool) {
        let mut cascade: BBTreeMap<Prop, (CascadeRank, Rc<str>)> = BBTreeMap::new();
        for &(order, specificity) in key.rules.0.iter() {
            let (_, declarations, layer) = &scoped.rules[order];
            for &(prop, ref value, important, position) in declarations.iter() {
                let rank =
                    CascadeRank::rule(*layer, important, specificity, scoped.keys[order], position);
                put(&mut cascade, prop, rank, value.clone());
            }
        }
        for (prop, value) in &key.hints {
            let rank = CascadeRank::new(HINT, false, 0, [0; 3], 0);
            put(&mut cascade, *prop, rank, value.clone());
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (prop, value, important, position) in inline.resolved(true) {
                let rank = CascadeRank {
                    inline: true,
                    position,
                    ..CascadeRank::new(AUTHOR, important, UNLAYERED, [0; 3], u128::MAX)
                };
                put(&mut cascade, prop, rank, value);
            }
        }
        let foreign = shadow
            .iter()
            .flat_map(|rules| rules.iter())
            .filter(|rule| rule.kind == ELEMENT)
            .flat_map(|rule| {
                rule.declarations
                    .iter()
                    .map(move |(prop, value, important, position)| {
                        (
                            *prop,
                            value.clone(),
                            CascadeRank::shadowed(rule, *important, *position),
                        )
                    })
            })
            .chain(
                animated
                    .into_iter()
                    .flat_map(Style::iter)
                    .map(|(prop, value)| {
                        (
                            prop,
                            value.clone(),
                            CascadeRank::new(ANIMATION, false, 0, [0; 3], 0),
                        )
                    }),
            );
        for (prop, value, rank) in foreign {
            put(&mut cascade, prop, rank, value);
        }
        if node.tag == "html"
            && let Some(scheme) = scheme
        {
            cascade
                .entry(Prop::COLOR_SCHEME)
                .or_insert((CascadeRank::default(), scheme.clone()));
        }
        // Remove longhands reset by a stronger shorthand: longhands otherwise win during later expansion.
        for (shorthand, longhands) in [
            (
                Prop::TRANSITION,
                &[
                    Prop::TRANSITION_PROPERTY,
                    Prop::TRANSITION_DURATION,
                    Prop::TRANSITION_TIMING_FUNCTION,
                    Prop::TRANSITION_DELAY,
                    Prop::TRANSITION_BEHAVIOR,
                ][..],
            ),
            (
                Prop::ANIMATION,
                &[
                    Prop::ANIMATION_NAME,
                    Prop::ANIMATION_DURATION,
                    Prop::ANIMATION_TIMING_FUNCTION,
                    Prop::ANIMATION_DELAY,
                    Prop::ANIMATION_ITERATION_COUNT,
                    Prop::ANIMATION_DIRECTION,
                    Prop::ANIMATION_FILL_MODE,
                    Prop::ANIMATION_PLAY_STATE,
                ][..],
            ),
        ] {
            if let Some(&(rank, _)) = cascade.get(&shorthand) {
                for longhand in longhands {
                    if cascade.get(longhand).is_some_and(|(old, _)| *old < rank) {
                        cascade.remove(longhand);
                    }
                }
            }
        }
        if let Some((rank, value)) = cascade.remove(&Prop::ALL) {
            for (prop, _) in style.own() {
                cascade.entry(prop).or_insert((rank, value.clone()));
            }
            for (prop, entry) in &mut cascade {
                if entry.0 < rank
                    && !prop.is_custom()
                    && !matches!(*prop, Prop::DIRECTION | Prop::UNICODE_BIDI)
                {
                    *entry = (rank, value.clone());
                }
            }
        }
        for (&prop, (rank, value)) in cascade
            .iter_mut()
            .filter(|(_, (_, value))| matches!(&**value, "revert" | "revert-layer"))
        {
            let (revert, (origin, layer)) = (&**value == "revert", rank.source());
            let mut best: Option<(CascadeRank, &Rc<str>)> = None;
            for &(order, specificity) in key.rules.0.iter() {
                let (_, declarations, rule_layer) = &scoped.rules[order];
                let from = rule_layer.origin.min(HINT);
                if from > origin
                    || from == origin && (revert || !rank.inline && rule_layer.rank == layer)
                {
                    continue;
                }
                for (_, candidate, important, position) in declarations
                    .iter()
                    .filter(|(name, ..)| *name == prop || *name == Prop::ALL && !prop.is_custom())
                {
                    let candidate_rank = CascadeRank::rule(
                        *rule_layer,
                        *important,
                        specificity,
                        scoped.keys[order],
                        *position,
                    );
                    if !matches!(&**candidate, "revert" | "revert-layer")
                        && best.as_ref().is_none_or(|(old, _)| candidate_rank >= *old)
                    {
                        best = Some((candidate_rank, candidate));
                    }
                }
            }
            *value = best.map_or_else(|| "unset".into(), |(_, value)| value.clone());
        }
        if cascade
            .get(&Prop::OVERLAY)
            .is_some_and(|(rank, _)| !matches!(rank.level, HINT | ANIMATION))
        {
            cascade.remove(&Prop::OVERLAY);
            if let Some((_, value)) = key.hints.iter().find(|(prop, _)| *prop == Prop::OVERLAY) {
                cascade.insert(Prop::OVERLAY, (CascadeRank::default(), value.clone()));
            }
        }
        const DECORATION: [Prop; 4] = [
            Prop::TEXT_DECORATION_LINE,
            Prop::TEXT_DECORATION_STYLE,
            Prop::TEXT_DECORATION_COLOR,
            Prop::TEXT_DECORATION_THICKNESS,
        ];
        let mut decorating = false;
        if !node.tag.starts_with('#') {
            let declared = |prop: Prop| cascade.get(&prop).map(|(_, value)| &**value);
            if declared(Prop::DISPLAY).is_some_and(|value| value.starts_with("inline-"))
                || declared(Prop::FLOAT).is_some_and(|value| value != "none")
                || declared(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            {
                for prop in DECORATION {
                    style.remove(prop);
                }
            }
            match cascade
                .get(&Prop::TEXT_DECORATION_LINE)
                .map(|(_, value)| value.clone())
                .as_deref()
            {
                Some("none") if style.contains(Prop::TEXT_DECORATION_LINE) => {
                    for prop in DECORATION {
                        cascade.remove(&prop);
                    }
                }
                Some(own) if !matches!(own, "none" | "inherit" | "unset" | "initial") => {
                    decorating = true;
                    if let Some(inherited) = style
                        .get(Prop::TEXT_DECORATION_LINE)
                        .filter(|value| *value != "none")
                    {
                        let mut lines: Vec<&str> = inherited.split_ascii_whitespace().collect();
                        for line in own.split_ascii_whitespace() {
                            if !lines.contains(&line) {
                                lines.push(line);
                            }
                        }
                        let merged: Rc<str> = lines.join(" ").into();
                        if let Some((_, value)) = cascade.get_mut(&Prop::TEXT_DECORATION_LINE) {
                            *value = merged;
                        }
                    }
                }
                _ => {}
            }
        }
        physical(&mut cascade, style);
        (cascade, decorating)
    }
}
