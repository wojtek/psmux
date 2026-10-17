// split-window and resize-pane geometry, in cells, against tmux 3.4.
//
// Ground truth: `tmux -L tanaesplit -f /dev/null new-session -d -x 120 -y 30`
// in WSL Ubuntu (tmux 3.4), then the command named in each test, read back
// with list-panes '#{pane_left},#{pane_width}'. The layout tree here holds
// empty splits as stand in panes (no PTY); only `sizes` matter.
//
// Before: split sizes were 50/50 percentage weights and resize-pane added N
// to a weight, so 120 columns split 59 | 60 (tmux 60 | 59), `-R 1` three
// times went 59 -> 60 -> 61 -> 63, `-D 1` three times on 30 rows moved
// nothing, `-y 10` gave a 2 row pane, and `-b` was dropped.

use super::*;
use crate::tree::{split_leaf_tmux, split_full_tmux, tmux_split_sizes, merge_freed_size, normalize_sizes_to_cells};

fn stub() -> Node { Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] } }

fn app_with_root(root: Node, active: Vec<usize>) -> AppState {
    let mut app = AppState::new("cells".to_string());
    app.windows.push(Window {
        root,
        active_path: active,
        name: "w".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app
}

fn area() -> ratatui::layout::Rect { ratatui::layout::Rect::new(0, 0, 120, 30) }

fn sizes(n: &Node) -> Vec<u16> { match n { Node::Split { sizes, .. } => sizes.clone(), _ => vec![] } }

/// A window with one pane split once by `split-window -h` (or -v).
fn split_once(kind: LayoutKind, size: Option<(u16, bool)>, before: bool) -> Node {
    let mut root = stub_leafish();
    split_leaf_tmux(&mut root, &[], area(), kind, stub(), size, before).unwrap();
    root
}

/// A "pane": an empty split of the OTHER kind, so the tree treats it as one
/// cell and never merges a split into it.
fn stub_leafish() -> Node { stub() }

#[test]
fn layout_split_sizes_matches_tmux() {
    // (ss + 1) / 2 - 1 for the second cell, the first takes the rest.
    assert_eq!(tmux_split_sizes(120, None, false, 2), (60, 59));
    assert_eq!(tmux_split_sizes(121, None, false, 2), (60, 60));
    assert_eq!(tmux_split_sizes(30, None, false, 1), (15, 14));
    assert_eq!(tmux_split_sizes(31, None, false, 1), (15, 15));
    // -l N names the new cell: after, it is the second; -b, the first.
    assert_eq!(tmux_split_sizes(120, Some(30), false, 2), (89, 30));
    assert_eq!(tmux_split_sizes(120, Some(30), true, 2), (30, 89));
    assert_eq!(tmux_split_sizes(30, Some(10), true, 1), (10, 19));
}

#[test]
fn split_h_is_60_59_and_v_is_15_14() {
    // tmux: 0,60 | 61,59   and   0,15 / 16,14
    assert_eq!(sizes(&split_once(LayoutKind::Horizontal, None, false)), vec![60, 59]);
    assert_eq!(sizes(&split_once(LayoutKind::Vertical, None, false)), vec![15, 14]);
}

#[test]
fn split_b_puts_the_new_pane_first_and_returns_its_path() {
    let mut root = stub();
    let p = split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), None, true).unwrap();
    assert_eq!(p, vec![0], "-b: the new pane is index 0 (tmux hb: 0:new 60, 1:old 59)");
    assert_eq!(sizes(&root), vec![60, 59]);
    // -b -l 30: the new, first, pane is 30 wide (tmux l30_hb: 30 | 89)
    let mut root = stub();
    split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), Some((30, false)), true).unwrap();
    assert_eq!(sizes(&root), vec![30, 89]);
}

#[test]
fn percent_size_is_a_share_of_the_pane() {
    // tmux l30p_h: 83 | 36 ; l25p_v on 30 rows: 22 | 7
    assert_eq!(sizes(&split_once(LayoutKind::Horizontal, Some((30, true)), false)), vec![83, 36]);
    assert_eq!(sizes(&split_once(LayoutKind::Vertical, Some((25, true)), false)), vec![22, 7]);
}

#[test]
fn a_second_split_the_same_way_joins_the_parent() {
    // tmux three_h: {60, 29, 29} in ONE split, not {60, {29, 30}}.
    let mut root = stub();
    let p1 = split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), None, false).unwrap();
    let p2 = split_leaf_tmux(&mut root, &p1, area(), LayoutKind::Horizontal, stub(), None, false).unwrap();
    assert_eq!(p2, vec![2]);
    assert_eq!(sizes(&root), vec![60, 29, 29]);
    // and -b on the middle one inserts before it: tmux h_then_sel1_hb {60,29,29}
    let mut root = stub();
    let p1 = split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), None, false).unwrap();
    let p2 = split_leaf_tmux(&mut root, &p1, area(), LayoutKind::Horizontal, stub(), None, true).unwrap();
    assert_eq!(p2, vec![1], "the new pane takes the old one's index");
    assert_eq!(sizes(&root), vec![60, 29, 29]);
}

#[test]
fn full_size_split_wraps_or_joins_the_root() {
    // tmux h_then_fvb: [120x15 new, {60, 59} x14]
    let mut root = stub();
    split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), None, false).unwrap();
    let p = split_full_tmux(&mut root, area(), LayoutKind::Vertical, stub(), None, true);
    assert_eq!(p, vec![0]);
    assert_eq!(sizes(&root), vec![15, 14]);
    // tmux h_then_fhb: {60 new, 29, 29}
    let mut root = stub();
    split_leaf_tmux(&mut root, &[], area(), LayoutKind::Horizontal, stub(), None, false).unwrap();
    let p = split_full_tmux(&mut root, area(), LayoutKind::Horizontal, stub(), None, true);
    assert_eq!(p, vec![0]);
    assert_eq!(sizes(&root), vec![60, 29, 29]);
}

fn resized(kind: LayoutKind, active: usize, steps: &[i32]) -> Vec<u16> {
    let root = split_once(kind, None, false);
    let mut app = app_with_root(root, vec![active]);
    for &s in steps { crate::window_ops::resize_active_pane_cells(&mut app, kind, s); }
    sizes(&app.windows[0].root)
}

#[test]
fn resize_moves_exactly_one_cell_per_step() {
    // tmux resR / resR2 / resR3: 61, 62, 63 (psmux was 60, 61, 63)
    assert_eq!(resized(LayoutKind::Horizontal, 0, &[1]), vec![61, 58]);
    assert_eq!(resized(LayoutKind::Horizontal, 0, &[1, 1]), vec![62, 57]);
    assert_eq!(resized(LayoutKind::Horizontal, 0, &[1, 1, 1]), vec![63, 56]);
    // tmux resL3: 57 | 62
    assert_eq!(resized(LayoutKind::Horizontal, 0, &[-1, -1, -1]), vec![57, 62]);
    // On the last pane the border before it moves, the arrow's way (#81):
    // tmux resR_on1 63 | 56, resL_on1 57 | 62
    assert_eq!(resized(LayoutKind::Horizontal, 1, &[1, 1, 1]), vec![63, 56]);
    assert_eq!(resized(LayoutKind::Horizontal, 1, &[-1, -1, -1]), vec![57, 62]);
    // tmux resD3 18 / 11 and resU3 12 / 17 (psmux moved 0 and 2 rows)
    assert_eq!(resized(LayoutKind::Vertical, 0, &[1, 1, 1]), vec![18, 11]);
    assert_eq!(resized(LayoutKind::Vertical, 0, &[-1, -1, -1]), vec![12, 17]);
}

#[test]
fn resize_stops_at_the_minimum() {
    // tmux resbig: 118 | 1. psmux keeps two columns (MIN_PTY_COLS).
    assert_eq!(resized(LayoutKind::Horizontal, 0, &[200]), vec![117, 2]);
    assert_eq!(resized(LayoutKind::Vertical, 0, &[200]), vec![28, 1]);
}

#[test]
fn resize_to_an_absolute_size() {
    // tmux resx: -x 40 gives 40 | 79 ; resy: -y 10 gives 10 / 19 (psmux gave 2)
    let mut app = app_with_root(split_once(LayoutKind::Horizontal, None, false), vec![0]);
    crate::window_ops::resize_pane_absolute(&mut app, "x", 40);
    assert_eq!(sizes(&app.windows[0].root), vec![40, 79]);
    let mut app = app_with_root(split_once(LayoutKind::Vertical, None, false), vec![0]);
    crate::window_ops::resize_pane_absolute(&mut app, "y", 10);
    assert_eq!(sizes(&app.windows[0].root), vec![10, 19]);
    // On the last pane the size is still that pane's: -x 40 on pane 1.
    let mut app = app_with_root(split_once(LayoutKind::Horizontal, None, false), vec![1]);
    crate::window_ops::resize_pane_absolute(&mut app, "x", 40);
    assert_eq!(sizes(&app.windows[0].root), vec![79, 40]);
}

#[test]
fn a_removed_cell_goes_to_its_neighbour() {
    // tmux kill_mid3 {60,29,29} -> {90, 29}; kill_first3 -> {90, 29}
    let mut s = vec![60, 29, 29];
    merge_freed_size(&mut s, 1);
    assert_eq!(s, vec![90, 29]);
    let mut s = vec![60, 29, 29];
    merge_freed_size(&mut s, 0);
    assert_eq!(s, vec![90, 29]);
    let mut s = vec![60, 29, 29];
    merge_freed_size(&mut s, 2);
    assert_eq!(s, vec![60, 59]);
}

#[test]
fn normalizing_keeps_the_geometry() {
    let mut root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![50, 50], children: vec![stub(), stub()] };
    normalize_sizes_to_cells(&mut root, area());
    assert_eq!(sizes(&root), vec![59, 60], "the cells 50/50 weights were drawn at");
}
