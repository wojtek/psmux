// Issue #361: the client-side OSC 8 overlay emitter. build_osc8_overlay() takes
// the hyperlink runs collected during a frame and produces the raw escape bytes
// that re-emit each run wrapped in OSC 8 at its screen position.
use crate::client::{build_osc8_overlay, visible_hyperlink_runs, HyperlinkRun};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

#[test]
fn wraps_run_in_osc8_at_position() {
    let runs = vec![HyperlinkRun {
        x: 5,
        y: 2,
        text: "link".into(),
        uri: "https://example.com".into(),
        style: Style::default().fg(Color::Red),
    }];
    let s = build_osc8_overlay(&runs);
    assert!(s.starts_with("\x1b7"), "saves cursor (DECSC)");
    assert!(s.ends_with("\x1b8"), "restores cursor (DECRC)");
    // 1-based cursor move to row 3, col 6
    assert!(s.contains("\x1b[3;6H"), "cursor move: {s:?}");
    // OSC 8 open + text + close
    assert!(
        s.contains("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
        "osc8 wrap: {s:?}"
    );
    // red fg = SGR 31
    assert!(s.contains("31"), "red fg sgr: {s:?}");
}

#[test]
fn empty_runs_produce_no_output() {
    assert_eq!(build_osc8_overlay(&[]), "");
}

#[test]
fn blank_text_run_emits_no_osc8() {
    let runs = vec![HyperlinkRun {
        x: 0,
        y: 0,
        text: String::new(),
        uri: "u".into(),
        style: Style::default(),
    }];
    let s = build_osc8_overlay(&runs);
    assert!(!s.contains("\x1b]8"), "no OSC 8 for blank text: {s:?}");
}

#[test]
fn rgb_color_and_modifiers_emitted() {
    let runs = vec![HyperlinkRun {
        x: 0,
        y: 0,
        text: "x".into(),
        uri: "u".into(),
        style: Style::default()
            .fg(Color::Rgb(1, 2, 3))
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    }];
    let s = build_osc8_overlay(&runs);
    assert!(s.contains("38;2;1;2;3"), "rgb fg: {s:?}");
    // bold(1) and underline(4) present in the SGR params
    assert!(s.contains(";1;") || s.contains(";1m"), "bold: {s:?}");
    assert!(s.contains(";4m") || s.contains(";4;"), "underline: {s:?}");
}

#[test]
fn two_runs_each_wrapped() {
    let runs = vec![
        HyperlinkRun { x: 0, y: 0, text: "a".into(), uri: "u1".into(), style: Style::default() },
        HyperlinkRun { x: 3, y: 1, text: "b".into(), uri: "u2".into(), style: Style::default() },
    ];
    let s = build_osc8_overlay(&runs);
    assert!(s.contains("\x1b]8;;u1\x1b\\a\x1b]8;;\x1b\\"));
    assert!(s.contains("\x1b]8;;u2\x1b\\b\x1b]8;;\x1b\\"));
    assert!(s.contains("\x1b[1;1H")); // run a at (0,0) -> 1;1
    assert!(s.contains("\x1b[2;4H")); // run b at (3,1) -> 2;4
}

// ── visible_hyperlink_runs: the overlay may only repaint cells the finished
// frame actually shows. Without this the pane's own glyphs were written over
// the choose-session modal and the `$` rename dialog, and because ratatui's
// buffer still held the overlay's cells the next diff never repaired them.

/// The style a pane link run carries in these tests.
fn link_style() -> Style {
    Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
}

/// A pane link run: `text` starting at column `x` of row `y`.
fn link(x: u16, y: u16, text: &str) -> HyperlinkRun {
    HyperlinkRun {
        x,
        y,
        text: text.into(),
        uri: "https://example.com/report".into(),
        style: link_style(),
    }
}

#[test]
fn a_run_the_frame_still_shows_survives_untouched() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 2));
    // Drawn the way the pane drew it: same glyphs AND same style. The filter
    // requires both, because re-emitting a cell in a different style is
    // exactly the recolouring the overlay exists to prevent.
    buf.set_string(0, 0, "see the report now", link_style());
    let visible = visible_hyperlink_runs(&buf, &[link(8, 0, "report")]);
    assert_eq!(visible.len(), 1, "an uncovered run must still be emitted");
    assert_eq!(visible[0].x, 8);
    assert_eq!(visible[0].y, 0);
    assert_eq!(visible[0].text, "report");
    assert_eq!(visible[0].uri, "https://example.com/report", "uri survives");
    assert_eq!(visible[0].style.fg, Some(Color::Blue), "style survives");
}

#[test]
fn matching_glyphs_in_a_different_style_are_not_the_pane_run() {
    // The dialog's own text can coincide with link glyphs cell for cell; what
    // it never coincides with is the pane link's colour and underline. Keeping
    // such a run would repaint the dialog in the pane's style.
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
    buf.set_string(0, 0, "see the report now", Style::default());
    assert!(visible_hyperlink_runs(&buf, &[link(8, 0, "report")]).is_empty());
}

#[test]
fn a_link_of_spaces_over_a_blanked_dialog_is_not_repainted() {
    // The Clear widget blanks a dialog to plain spaces with default style. A
    // pane link whose text is spaces matched on glyphs alone and then wrote
    // the pane's background over the dialog's blanked interior.
    let buf = Buffer::empty(Rect::new(0, 0, 30, 1));
    assert!(
        visible_hyperlink_runs(&buf, &[link(2, 0, "     ")]).is_empty(),
        "spaces over a blanked dialog are not the pane's link"
    );
}

#[test]
fn a_visible_link_with_combining_marks_and_emoji_keeps_its_hyperlink() {
    // ratatui stores `e` + U+0301 as ONE cell, and an emoji plus VS16 as one
    // two-column cell. A char-by-char walk rejected the run at the first
    // accent and dropped links the frame plainly shows, with no modal open.
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 2));
    buf.set_string(0, 0, "cafe\u{301} report", link_style());
    buf.set_string(0, 1, "go 👍\u{fe0f} now", link_style());
    let accented = visible_hyperlink_runs(&buf, &[link(0, 0, "cafe\u{301} report")]);
    assert_eq!(accented.len(), 1, "the accented link is fully visible");
    assert_eq!(accented[0].text, "cafe\u{301} report");
    let emoji = visible_hyperlink_runs(&buf, &[link(3, 1, "👍\u{fe0f}")]);
    assert_eq!(emoji.len(), 1, "the emoji link is fully visible");
    assert_eq!(emoji[0].x, 3, "the emoji occupies columns 3 and 4");
}

#[test]
fn a_run_a_modal_drew_over_is_dropped() {
    // Row 0 is what the pane showed while the frame was being built, so the run
    // was collected from it. The dialog then painted over the same row, so the
    // finished frame holds the dialog's border there instead.
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 2));
    buf.set_string(0, 0, "see the report now", Style::default());
    buf.set_string(0, 0, "┌────────────────────────────┐", Style::default());
    assert!(
        visible_hyperlink_runs(&buf, &[link(8, 0, "report")]).is_empty(),
        "a link the frame no longer shows must not be repainted over the dialog"
    );
}

#[test]
fn a_run_on_a_row_the_modal_covers_entirely_is_dropped() {
    // The common shape: the pane's link is fine where it is, but a centred
    // modal occupies that whole row.
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
    buf.set_string(0, 1, "│ name: report          │", Style::default());
    assert!(visible_hyperlink_runs(&buf, &[link(2, 1, "report")]).is_empty());
}

#[test]
fn a_partly_covered_run_keeps_its_visible_head_only() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
    buf.set_string(0, 0, "abcdefghijkl", link_style());
    // A modal border crosses the middle of the run, covering columns 4..7.
    buf.set_string(4, 0, "────", Style::default());
    let visible = visible_hyperlink_runs(&buf, &[link(0, 0, "abcdefghijkl")]);
    let fragments: Vec<(u16, &str)> = visible.iter().map(|r| (r.x, r.text.as_str())).collect();
    assert_eq!(
        fragments,
        vec![(0, "abcd")],
        "the visible head keeps its link; the covered tail has no cells left to match and is cut"
    );
}

#[test]
fn a_character_the_overlay_happens_to_share_is_not_a_visible_run() {
    // Under a dialog the pane's link and the dialog's own text coincide cell
    // by cell here and there — 'htt' and the 'e' of "name" below. Those cells
    // carry the dialog's style, not the link's, so the style check ends the
    // stretch before a single coincidental glyph is emitted; emitting them
    // would recolour the dialog's characters and turn them into links, the
    // reported artifact one cell at a time.
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
    buf.set_string(0, 0, "read https://example.com/report today", Style::default());
    buf.set_string(8, 0, "│ name: today           │", Style::default());
    assert!(visible_hyperlink_runs(&buf, &[link(5, 0, "https://example.com/report")]).is_empty());
}

#[test]
fn a_run_off_the_edge_of_the_buffer_is_dropped() {
    let buf = Buffer::empty(Rect::new(0, 0, 6, 2));
    assert!(visible_hyperlink_runs(&buf, &[link(20, 0, "report")]).is_empty());
    assert!(visible_hyperlink_runs(&buf, &[link(0, 9, "report")]).is_empty());
}

#[test]
fn a_wide_glyph_does_not_desynchronise_the_column_walk() {
    // ratatui draws 漢 as one cell plus one reset continuation cell, so a run
    // containing it has to advance two columns. Off by one and every glyph
    // after it is compared against the wrong cell, which drops a link the
    // frame plainly shows.
    let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
    buf.set_string(0, 0, "a漢b report", link_style());
    let wide = visible_hyperlink_runs(&buf, &[link(0, 0, "a漢b")]);
    assert_eq!(wide.len(), 1);
    assert_eq!(wide[0].text, "a漢b");
    // A run starting after the wide glyph still lines up on its own column.
    let after = visible_hyperlink_runs(&buf, &[link(5, 0, "report")]);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].x, 5);
    assert_eq!(after[0].text, "report");
}

#[test]
fn no_runs_means_nothing_to_emit() {
    let buf = Buffer::empty(Rect::new(0, 0, 10, 1));
    assert!(visible_hyperlink_runs(&buf, &[]).is_empty());
}

#[test]
fn issue361_modal_and_link_together_reproduce_the_reported_overpaint() {
    // The reported symptom end to end: a session prints a bold underlined link,
    // the picker is open, and the overlay used to stamp the link over the
    // dialog. Build the frame the way the client does — pane content first,
    // dialog on top — and assert the emitter has nothing left to paint there.
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
    buf.set_string(0, 2, "read https://example.com/report today", Style::default());
    // The dialog covers columns 8..31 of rows 1..3.
    buf.set_string(8, 1, "┌───────────────────────┐", Style::default());
    buf.set_string(8, 2, "│ name: today           │", Style::default());
    buf.set_string(8, 3, "└───────────────────────┘", Style::default());

    let visible = visible_hyperlink_runs(&buf, &[link(5, 2, "https://example.com/report")]);
    assert!(
        visible.is_empty(),
        "the whole link sits under the dialog, so nothing may be repainted: {:?}",
        visible.iter().map(|r| (r.x, r.text.clone())).collect::<Vec<_>>()
    );
    assert_eq!(build_osc8_overlay(&visible), "", "and so the emitter writes no bytes at all");
}
