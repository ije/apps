use super::{Tree, index_of};
use crate::{Child, node, px, size};

fn live(tree: &Tree) -> usize {
    tree.slots.iter().filter(|slot| slot.live).count()
}

fn slot(tree: &Tree, id: usize) -> &super::Slot {
    &tree.slots[tree.index(id.into()).unwrap() as usize]
}

#[test]
fn kept_subtrees_link_and_dropped_boxes_free() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let page = |rows: Vec<Child>| node().id(0usize).children(rows).into();
    let row = |id: usize| node().id(id).child(node().w(px(1.))).child("text").into();
    tree.begin(1., 0);
    tree.commit_root(page(vec![row(1), row(2), row(3)]), viewport, 1.);
    assert_eq!(live(&tree), 1 + 3 * 3);
    tree.begin(1., 0);
    let kept = slot(&tree, 2).children.clone();
    tree.commit_root(page(vec![row(1), Child::Kept(2usize.into())]), viewport, 1.);
    assert_eq!(live(&tree), 1 + 2 * 3);
    assert!(tree.contains(2usize) && !tree.contains(3usize));
    assert_eq!(slot(&tree, 2).children, kept);
    tree.begin(1., 0);
    let moved = node().id(4usize).child(Child::Kept(2usize.into())).into();
    tree.commit_root(page(vec![moved]), viewport, 1.);
    assert!(tree.contains(2usize) && !tree.contains(1usize));
    assert_eq!(live(&tree), 1 + 1 + 3);
    let parent = tree.index(4usize.into()).unwrap();
    assert_eq!(slot(&tree, 2).parent, parent);
    assert!(
        slot(&tree, 2)
            .children
            .iter()
            .all(|&child| { tree.slots[index_of(child) as usize].live })
    );
}

#[test]
fn mostly_free_arenas_compact() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let page = |rows: usize, kept: bool| {
        let rows = (1..=rows).map(|id| match kept {
            true if id <= 3 => Child::Kept(id.into()),
            _ => node().id(id).child(node()).into(),
        });
        node()
            .id(0usize)
            .children(rows.collect::<Vec<Child>>())
            .into()
    };
    tree.begin(1., 0);
    tree.commit_root(page(2000, false), viewport, 1.);
    let kept = slot(&tree, 3).children.clone();
    tree.begin(1., 0);
    tree.commit_root(page(3, true), viewport, 1.).unwrap();
    tree.bury();
    let root = tree.root.unwrap();
    assert_eq!(tree.slots.len(), 1 + 3 * 2);
    assert_eq!(slot(&tree, 3).children.len(), kept.len());
    let child = index_of(slot(&tree, 3).children[0]);
    assert_eq!(
        tree.slots[child as usize].parent,
        tree.index(3usize.into()).unwrap()
    );
    assert_eq!(slot(&tree, 3).parent, root);
}

#[test]
fn kept_text_follows_the_inherited_style() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let page = |font: f32, child: Child| {
        let mut page = node().id(0usize).child(child);
        page.style().text.set_font_size(px(font));
        page.into()
    };
    tree.begin(1., 0);
    tree.commit_root(page(10., node().id(1usize).child("a").into()), viewport, 1.);
    tree.begin(1., 0);
    tree.commit_root(page(20., Child::Kept(1usize.into())), viewport, 1.);
    let text = index_of(slot(&tree, 1).children[0]);
    assert_eq!(tree.slots[text as usize].text.font_size, px(20.));
    assert!(tree.slots[text as usize].cache.is_empty());
}

#[test]
fn paint_only_text_changes_keep_the_measured_state() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let page = |color: u32, child: Child| {
        let mut page = node().id(0usize).child(child);
        page.style().text.set_color(crate::Color::hex(color));
        page.into()
    };
    let text = |tree: &Tree| index_of(slot(tree, 1).children[0]) as usize;
    let state = |tree: &Tree| tree.slots[text(tree)].state.clone().unwrap();
    tree.begin(1., 0);
    tree.commit_root(
        page(0xff0000ff, node().id(1usize).child("a").into()),
        viewport,
        1.,
    );
    let first = state(&tree);
    tree.begin(1., 0);
    tree.commit_root(page(0x0000ffff, Child::Kept(1usize.into())), viewport, 1.);
    assert_eq!(
        tree.slots[text(&tree)].text.color,
        crate::Color::hex(0x0000ffff)
    );
    assert!(std::rc::Rc::ptr_eq(&first, &state(&tree)));
    tree.begin(1., 0);
    tree.commit_root(
        page(0x00ff00ff, node().id(1usize).child("a").into()),
        viewport,
        1.,
    );
    assert_eq!(
        tree.slots[text(&tree)].text.color,
        crate::Color::hex(0x00ff00ff)
    );
    assert!(std::rc::Rc::ptr_eq(&first, &state(&tree)));
}

#[test]
fn patches_replace_kept_boxes_in_place() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let field = |width: f32| node().id(2usize).w(px(width)).child(node().child("a"));
    let page = |child: Child| {
        node()
            .id(0usize)
            .child(node().id(1usize).child(child))
            .into()
    };
    let width = |tree: &Tree| slot(tree, 2).style.size.width;
    tree.begin(1., 0);
    tree.commit_root(page(field(10.).into()), viewport, 1.);
    let (parent, own, inner) = (
        tree.index(1usize.into()).unwrap(),
        tree.index(2usize.into()).unwrap(),
        slot(&tree, 2).children.clone(),
    );
    let slots = live(&tree);
    tree.begin(1., 0);
    tree.commit_root(Child::Kept(0usize.into()), viewport, 1.);
    tree.patch(field(20.), 1.);
    assert_eq!(width(&tree), px(20.).into());
    assert_eq!(slot(&tree, 1).children, [(own as usize).into()]);
    assert_eq!(slot(&tree, 2).parent, parent);
    assert_eq!(slot(&tree, 2).children, inner);
    assert_eq!(live(&tree), slots);
    tree.begin(1., 0);
    tree.commit_root(page(field(30.).into()), viewport, 1.);
    tree.patch(field(40.), 1.);
    assert_eq!(
        width(&tree),
        px(30.).into(),
        "a box built in this pass keeps its build"
    );
}

#[test]
fn placed_boxes_keep_their_subtree_at_a_new_place() {
    let mut tree = Tree::default();
    let viewport = size(px(100.), px(100.));
    let page = |child: Child| node().id(0usize).child(child).into();
    let place = |x: f32, width: f32| {
        Child::Placed(
            1usize.into(),
            crate::Rect {
                origin: crate::point(px(x), px(2.)),
                size: size(px(width), px(8.)),
            },
        )
    };
    tree.begin(1., 0);
    let cell = node()
        .id(1usize)
        .absolute()
        .left(px(1.))
        .top(px(2.))
        .w(px(4.))
        .h(px(8.));
    tree.commit_root(page(cell.child("a").into()), viewport, 1.);
    let (inner, built) = (
        slot(&tree, 1).children.clone(),
        slot(&tree, 1).style.clone(),
    );
    tree.begin(1., 0);
    tree.commit_root(page(place(1., 4.)), viewport, 1.);
    assert!(
        std::rc::Rc::ptr_eq(&built, &slot(&tree, 1).style),
        "the same place keeps the style"
    );
    tree.begin(1., 0);
    tree.commit_root(page(place(10., 30.)), viewport, 1.);
    let cell = slot(&tree, 1);
    assert_eq!(
        [
            cell.style.inset.left,
            cell.style.inset.top,
            cell.style.size.width
        ],
        [px(10.).into(), px(2.).into(), px(30.).into()]
    );
    assert_eq!(cell.children, inner);
}
