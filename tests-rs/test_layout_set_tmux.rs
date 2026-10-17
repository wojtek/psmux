// select-layout arithmetic against tmux 3.4 (layout-set.c), on the numbers
// `tmux -L tanaesplit -f /dev/null new-session -d -x 120 -y 30` gives in WSL.

use super::*;

#[test]
fn spread_is_layout_spread_cell() {
    // even-horizontal: 3 panes 39,39,40; 4 panes 29,29,29,30; 5 panes 23x4,24
    assert_eq!(layout_spread_sizes(120, 3), Some(vec![39, 39, 40]));
    assert_eq!(layout_spread_sizes(120, 4), Some(vec![29, 29, 29, 30]));
    assert_eq!(layout_spread_sizes(120, 5), Some(vec![23, 23, 23, 23, 24]));
    // even-vertical on 30 rows, 4 panes: 6,6,6,9
    assert_eq!(layout_spread_sizes(30, 4), Some(vec![6, 6, 6, 9]));
    // no cell each: tmux leaves the layout alone
    assert_eq!(layout_spread_sizes(3, 4), None);
}

#[test]
fn string_percentage_is_args_string_percentage() {
    assert_eq!(layout_string_percentage("80", 119, 119), Some(80));
    assert_eq!(layout_string_percentage("30%", 119, 119), Some(35));
    assert_eq!(layout_string_percentage("200", 119, 119), None, "too large");
    assert_eq!(layout_string_percentage("wide", 119, 119), None);
}

#[test]
fn main_sizes_are_layout_set_main() {
    // main-vertical on 120: default 80 | 39; -w main-pane-width 50: 50 | 69;
    // 30%: 35 | 84; other-pane-width 30: 89 | 30
    assert_eq!(layout_main_sizes(120, "80", "0", 80, 1), (80, 39));
    assert_eq!(layout_main_sizes(120, "50", "0", 80, 1), (50, 69));
    assert_eq!(layout_main_sizes(120, "30%", "0", 80, 1), (35, 84));
    assert_eq!(layout_main_sizes(120, "80", "30", 80, 1), (89, 30));
    // 121 columns: 80 | 40
    assert_eq!(layout_main_sizes(121, "80", "0", 80, 1), (80, 40));
    // too narrow for 80: tmux 78 | 1 on 80 columns
    assert_eq!(layout_main_sizes(80, "80", "0", 80, 1), (78, 1));
    // main-horizontal on 30 rows: default 24 / 5; 10 / 19; 50%: 14 / 15;
    // other-pane-height 10 cannot shrink the main pane, so 24 / 5 stays
    assert_eq!(layout_main_sizes(30, "24", "0", 24, 1), (24, 5));
    assert_eq!(layout_main_sizes(30, "10", "0", 24, 1), (10, 19));
    assert_eq!(layout_main_sizes(30, "50%", "0", 24, 1), (14, 15));
    assert_eq!(layout_main_sizes(30, "24", "10", 24, 1), (24, 5));
    // 20 rows: 18 / 1
    assert_eq!(layout_main_sizes(20, "24", "0", 24, 1), (18, 1));
}

#[test]
fn tiled_grid_is_layout_set_tiled() {
    assert_eq!(layout_tiled_grid(2), (2, 1));
    assert_eq!(layout_tiled_grid(3), (2, 2));
    assert_eq!(layout_tiled_grid(4), (2, 2));
    assert_eq!(layout_tiled_grid(5), (3, 2));
    assert_eq!(layout_tiled_grid(6), (3, 2));
    assert_eq!(layout_tiled_grid(7), (3, 3));
}
