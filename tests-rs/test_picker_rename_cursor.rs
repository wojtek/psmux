// The caret of the picker's `$` rename dialog.
//
// The dialog always took the keystrokes, but it never showed a caret of its
// own, and the post-draw cursor write went on re-showing the ACTIVE PANE's
// caret over it — the same defect #507 fixed for server PTY popups, which that
// fix never extended to the client-side overlays. So the field looked
// unfocused whenever the pane underneath happened to be showing a cursor (a
// shell at its prompt), and looked cursorless when it was not (an app that
// hides its own cursor). That is the "just sometimes" in the report.
//
// As in #507, the draw pass and the cursor pass must agree on the geometry to
// the cell, so both go through these two functions and these tests pin them.

use ratatui::layout::Rect;

use crate::client::{picker_rename_cursor_screen_pos, picker_rename_overlay_rect};

// ── picker_rename_overlay_rect: geometry both passes share ──────────────────

#[test]
fn dialog_takes_sixty_percent_of_the_content_width() {
    let content = Rect::new(0, 0, 120, 30);
    let r = picker_rename_overlay_rect(content, false);
    // The centring layout is vertical, so every band spans the full width and
    // the dialog's own width is decided before it is placed.
    assert_eq!(r.width, 72, "60% of 120");
    assert_eq!(r.x, 24, "centred: (120-72)/2");
}

#[test]
fn dialog_grows_by_one_row_for_the_error_or_progress_line() {
    let content = Rect::new(0, 0, 120, 30);
    let plain = picker_rename_overlay_rect(content, false);
    let extra = picker_rename_overlay_rect(content, true);
    assert_eq!(plain.height, 3, "border + name row + border");
    assert_eq!(extra.height, 4, "one more row for 'renaming...' or the error");
    assert_eq!((plain.x, plain.width), (extra.x, extra.width), "same box, one row taller");
}

#[test]
fn dialog_stays_inside_the_content_area_on_a_short_terminal() {
    let content = Rect::new(0, 1, 40, 6);
    let r = picker_rename_overlay_rect(content, true);
    assert!(r.y >= content.y, "top edge inside the content area");
    assert!(r.y + r.height <= content.y + content.height, "bottom edge inside it too");
    assert!(r.x + r.width <= content.x + content.width, "right edge inside it too");
}

// ── picker_rename_cursor_screen_pos: the cell the caret belongs on ──────────

#[test]
fn caret_sits_just_past_the_prompt_on_the_row_inside_the_border() {
    let content = Rect::new(0, 0, 120, 30);
    let r = picker_rename_overlay_rect(content, false);
    let (cx, cy) = picker_rename_cursor_screen_pos(r, "").unwrap();
    assert_eq!(cy, r.y + 1, "on the name row, not on the border");
    assert_eq!(cx, r.x + 1 + "name: ".len() as u16, "immediately after 'name: '");
}

#[test]
fn caret_moves_one_column_per_typed_character() {
    let content = Rect::new(0, 0, 120, 30);
    let r = picker_rename_overlay_rect(content, false);
    let empty = picker_rename_cursor_screen_pos(r, "").unwrap();
    let typed = picker_rename_cursor_screen_pos(r, "build").unwrap();
    assert_eq!(typed.0 - empty.0, 5, "typing 5 characters moves the caret 5 columns");
    assert_eq!(typed.1, empty.1, "and it stays on the name row");
}

#[test]
fn caret_counts_a_wide_name_in_cells_not_characters() {
    let r = Rect::new(0, 0, 40, 3);
    let ascii = picker_rename_cursor_screen_pos(r, "ab").unwrap().0;
    let wide = picker_rename_cursor_screen_pos(r, "漢字").unwrap().0;
    assert_eq!(wide - ascii, 2, "two CJK characters occupy four cells, not two");
}

#[test]
fn a_name_that_fills_the_dialog_leaves_no_caret_cell() {
    // 20 columns: 1 border + 6 prompt + 12 name + 1 border. The next cell is
    // the closing border, so the caret must be dropped rather than drawn on
    // the border — or, worse, left wherever the pane had it.
    let r = Rect::new(0, 0, 20, 3);
    assert!(picker_rename_cursor_screen_pos(r, "abcdefghijkl").is_none());
    assert!(
        picker_rename_cursor_screen_pos(r, "abcdefghijk").is_some(),
        "one character shorter still has a cell to sit on"
    );
}

#[test]
fn a_dialog_too_small_to_have_an_interior_has_no_caret() {
    assert!(picker_rename_cursor_screen_pos(Rect::new(0, 0, 2, 3), "").is_none(), "no interior column");
    assert!(picker_rename_cursor_screen_pos(Rect::new(0, 0, 20, 2), "").is_none(), "no interior row");
}

#[test]
fn caret_stays_inside_the_dialog_for_every_name_length_that_fits() {
    let content = Rect::new(0, 0, 120, 30);
    let r = picker_rename_overlay_rect(content, false);
    for len in 0..80usize {
        let name = "n".repeat(len);
        if let Some((cx, cy)) = picker_rename_cursor_screen_pos(r, &name) {
            assert!(cx > r.x && cx < r.x + r.width - 1, "{len} chars put the caret at {cx}, outside {r:?}");
            assert_eq!(cy, r.y + 1, "{len} chars moved the caret off the name row");
        }
    }
}

// ── Regression guard: the caret must never land on the pane ─────────────────

#[test]
fn the_caret_is_never_parked_on_the_pane_outside_the_dialog() {
    // The reported symptom: the visible caret stayed on the prompt of the pane
    // behind the picker, so the field looked like it did not have focus.
    let content = Rect::new(0, 0, 120, 29); // the status bar owns the last row
    let r = picker_rename_overlay_rect(content, false);
    let (cx, cy) = picker_rename_cursor_screen_pos(r, "tsunami").unwrap();
    assert!(
        cx >= r.x && cx < r.x + r.width && cy >= r.y && cy < r.y + r.height,
        "caret ({cx},{cy}) is outside the dialog {r:?} — that is the pane underneath"
    );
    assert_ne!((cx, cy), (0, 0), "and it is not parked at the top-left of the screen");
}

#[test]
fn a_combining_mark_does_not_move_the_caret() {
    // The accent renders into the previous cell, so the caret must not step
    // over it. Measuring characters instead of displayed columns put the caret
    // one column past the end of the name for every combining mark typed.
    let r = Rect::new(0, 0, 40, 3);
    let plain = picker_rename_cursor_screen_pos(r, "cafe").unwrap().0;
    let accented = picker_rename_cursor_screen_pos(r, "cafe\u{301}").unwrap().0;
    assert_eq!(plain, accented, "the accent occupies no column of its own");
}

#[test]
fn an_absurdly_long_pasted_name_loses_the_caret_instead_of_overflowing() {
    // Paste accepts an unbounded string. The column arithmetic must saturate
    // and drop the caret, not wrap around and park it inside the dialog.
    let r = Rect::new(0, 0, 40, 3);
    let huge = "n".repeat(200_000);
    assert!(picker_rename_cursor_screen_pos(r, &huge).is_none());
}
