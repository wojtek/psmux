// A Ctrl+V never sends the clipboard to a pane hidden behind a psmux dialog.
//
// On Windows with paste detection on, the Ctrl+V Release sets
// `paste_confirmed` whatever is on screen. When nothing was buffered for the
// pane, the fallback then read the system clipboard and sent it with
// `send-paste`, and the server writes a `send-paste` to the active pane
// (`input::send_paste_to_active` special-cases only clock mode). With the
// command prompt, a rename dialog, a chooser or a server popup open, and the
// paste delivered as characters into the dialog (or not delivered at all), the
// clipboard went into the pane behind it, where a newline could run a command.
//
// Routing of `send-paste` while each server overlay is up, from the server
// code: a PTY popup, a static popup, a menu, a confirm prompt, display-panes
// and customize all leave the active pane as the target, so the text lands in
// the pane behind them; clock mode consumes the paste by closing the clock
// and writes nothing. So the read-back is suppressed for every overlay except
// the clock.
//
// These tests pin the extracted decision; they start no server (see AGENTS.md).

use super::*;

fn nothing_open() -> OverlayFlags {
    OverlayFlags::default()
}

fn allowed(overlays: OverlayFlags) -> bool {
    clipboard_read_back_allowed(overlays, false, false, false)
}

#[test]
fn with_nothing_open_the_read_back_goes_to_the_pane() {
    assert!(allowed(nothing_open()));
}

#[test]
fn an_open_paste_window_still_blocks_the_read_back() {
    assert!(!clipboard_read_back_allowed(nothing_open(), false, false, true));
}

#[test]
fn an_open_client_side_dialog_blocks_the_read_back() {
    // The command prompt, rename and pane-rename dialogs, the picker `$`
    // rename, the session/tree/buffer choosers, the key viewer and a client
    // confirm prompt are all `OverlayFlags::client`.
    assert!(!allowed(OverlayFlags { client: true, ..nothing_open() }));
}

#[test]
fn the_window_index_prompt_blocks_the_read_back() {
    assert!(!clipboard_read_back_allowed(nothing_open(), true, false, false));
}

#[test]
fn server_overlays_that_leave_the_paste_to_the_pane_behind_block_it() {
    let cases = [
        ("popup (PTY or static)", OverlayFlags { popup: true, ..nothing_open() }),
        ("confirm", OverlayFlags { confirm: true, ..nothing_open() }),
        ("menu", OverlayFlags { menu: true, ..nothing_open() }),
        ("display-panes", OverlayFlags { display_panes: true, ..nothing_open() }),
    ];
    for (name, overlays) in cases {
        assert!(!allowed(overlays), "{name}: send-paste would reach the pane behind it");
    }
    assert!(
        !clipboard_read_back_allowed(nothing_open(), false, true, false),
        "customize: send-paste would reach the pane behind it"
    );
}

#[test]
fn clock_mode_consumes_the_paste_so_the_read_back_is_unchanged() {
    assert!(allowed(OverlayFlags { clock: true, ..nothing_open() }));
}
