// Who owns the caret in a frame is decided by the overlays that frame shows.
//
// The client loop decides whether to render from the overlays that were up
// when its pass began, then parses the incoming frame, which replaces the
// server-owned overlay flags (popup, confirm, menu, display-panes, clock), and
// only then draws. The caret decision used the value from the start of the
// pass, so on the frame that closed a server overlay the caret was still
// hidden "for the overlay" while the pane was drawn without it; an idle client
// then had no reason to redraw and the pane's caret stayed hidden, which reads
// as lost input focus. Symmetrically, the frame that opened an overlay could
// park the pane's caret on top of it.
//
// These tests pin FrameOverlays, the loop's per-frame overlay state; they start
// no server (see AGENTS.md).

use super::*;

fn server_overlay(kind: &str) -> OverlayFlags {
    let mut flags = OverlayFlags::default();
    match kind {
        "popup" => flags.popup = true,
        "confirm" => flags.confirm = true,
        "menu" => flags.menu = true,
        "display-panes" => flags.display_panes = true,
        "clock" => flags.clock = true,
        other => panic!("unknown overlay {other}"),
    }
    flags
}

const SERVER_OVERLAYS: [&str; 5] = ["popup", "confirm", "menu", "display-panes", "clock"];

#[test]
fn a_frame_that_closes_a_server_overlay_gives_the_caret_back() {
    for kind in SERVER_OVERLAYS {
        let frame = FrameOverlays { before: server_overlay(kind), drawn: OverlayFlags::default() };
        assert!(
            !frame.overlay_owns_drawn_frame(),
            "after the {kind} closes, the drawn frame has no overlay and the pane owns the caret"
        );
    }
}

#[test]
fn a_frame_that_opens_a_server_overlay_takes_the_caret() {
    for kind in SERVER_OVERLAYS {
        let frame = FrameOverlays { before: OverlayFlags::default(), drawn: server_overlay(kind) };
        assert!(
            frame.overlay_owns_drawn_frame(),
            "the frame that shows the {kind} must not park the pane's caret on top of it"
        );
    }
}

#[test]
fn a_client_overlay_that_stays_open_keeps_the_caret() {
    let open = OverlayFlags { client: true, ..OverlayFlags::default() };
    let frame = FrameOverlays { before: open, drawn: open };
    assert!(frame.overlay_owns_drawn_frame());
}

#[test]
fn no_overlay_before_or_after_leaves_the_caret_to_the_pane() {
    assert!(!FrameOverlays::default().overlay_owns_drawn_frame());
}
