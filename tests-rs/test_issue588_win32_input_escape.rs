// Issue #588: after one `psmux send-keys ... C-m`, no Escape key ever reached
// the pane again -- in cygwin bash, in a WSL pane, and in nvim's insert mode.
//
// The mechanism, measured under a BARE pseudoconsole with no psmux involved
// (tests/conptyfeed610.cs hosting tests/keylog_child.cs, fed raw bytes):
//
//   feed 1b                      -> char=0x1B key=Escape mods=0
//   feed the C-m win32 sequence  -> char=0x0D key=M      mods=Control
//   feed 1b, 1b, 1b              -> NOTHING AT ALL
//   feed "abc"                   -> char=0x61 key=A mods=ALT, then B, then C
//   feed the ESC win32 sequence  -> char=0x1B key=Escape mods=0
//
// So writing ONE win32 input mode sequence permanently changes how conhost's
// input state machine treats the end of a write on that ConPTY: it concludes
// the terminal speaks win32 input mode and stops dispatching a dangling ESC as
// the Escape key, holding it as the possible start of a longer sequence -- note
// the held ESC fusing with the next `a` into Alt+A. The same key written AS a
// win32 sequence is accepted either way, which is what the fix uses.
//
// These are the exact wire strings from that measurement. They are the contract
// with conhost, so they are pinned here rather than left to drift: the C-m one
// is what issue #305 (Set-PSReadLineKeyHandler) needs, the Escape one is what
// issue #588 needs to survive it.

use crate::input::{win32_input_key_seq, win32_input_special_key_seq};

/// `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`, press (Kd=1) then release (Kd=0).
#[test]
fn win32_input_sequence_has_the_windows_terminal_wire_form() {
    // Ctrl+M: VK_M=77, scan=50, u_char='m'&0x1F=13, LEFT_CTRL_PRESSED=8.
    // Delivered as char=0x0D key=M mods=Control.  This is the #305 sequence and
    // it must not change.
    assert_eq!(
        win32_input_key_seq(77, 50, 13, 8),
        "\x1b[77;50;13;1;8;1_\x1b[77;50;13;0;8;1_"
    );
}

#[test]
fn escape_in_win32_form_is_the_measured_sequence() {
    // VK_ESCAPE=27, scan=1, u_char=27, no modifiers.
    // Delivered as char=0x1B key=Escape mods=0 even on a ConPTY that has been
    // latched into win32 input mode and no longer accepts a bare 0x1b.
    assert_eq!(
        win32_input_key_seq(27, 1, 27, 0),
        "\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_"
    );
}

/// The repair looks the scan code up rather than hard-coding it, so pin what
/// the platform actually returns for VK_ESCAPE: the sequence above is only
/// correct while this is 1.
#[test]
fn vk_escape_maps_to_scan_code_one() {
    assert_eq!(crate::platform::mouse_inject::vk_to_scan(0x1B), 1);
}

#[test]
fn ctrl_j_uses_the_win32_record_codex_accepts() {
    assert_eq!(
        win32_input_special_key_seq(b"\n").as_deref(),
        Some("\x1b[74;36;10;1;8;1_\x1b[74;36;10;0;8;1_")
    );
}

#[test]
fn modified_enter_uses_the_win32_record_codex_accepts() {
    assert_eq!(
        win32_input_special_key_seq(b"\x1b\r").as_deref(),
        Some("\x1b[13;28;13;1;2;1_\x1b[13;28;13;0;2;1_")
    );
}
