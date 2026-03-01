/// Test for GitHub Issue #52: Cursor not in the right position in Claude session
///
/// Tests the vt100 parser's handling of:
///   - CSI s / CSI u (SCOSC/SCORC — ANSI cursor save/restore)
///   - CSI f (HVP — Horizontal and Vertical Position, same as CUP)
///   - hide_cursor flag (CSI ?25l / CSI ?25h)
///   - TUI render patterns that use save/restore cursor
///
/// With ConPTY passthrough mode (Windows 11 22H2+), raw VT sequences from
/// child processes reach the vt100 parser directly. Missing CSI s/u support
/// causes cursor position desync in TUI apps like Claude Code.

fn main() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── Test 1: Basic CUP (CSI H) ──────────────────────────────────────────
    {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x1b[6;11H"); // Move to row 6, col 11 (1-based)
        let (r, c) = p.screen().cursor_position();
        if (r, c) == (5, 10) {
            println!("TEST_CUP_PASS: CUP correctly positions cursor at (5, 10)");
            passed += 1;
        } else {
            println!("TEST_CUP_FAIL: CUP set ({}, {}), expected (5, 10)", r, c);
            failed += 1;
        }
    }

    // ── Test 2: CSI f (HVP) ────────────────────────────────────────────────
    {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x1b[10;20f"); // HVP to row 10, col 20 (1-based)
        let (r, c) = p.screen().cursor_position();
        if (r, c) == (9, 19) {
            println!("TEST_HVP_PASS: HVP (CSI f) correctly positions cursor at (9, 19)");
            passed += 1;
        } else {
            println!("TEST_HVP_FAIL: HVP set ({}, {}), expected (9, 19)", r, c);
            failed += 1;
        }
    }

    // ── Test 3: ESC 7 / ESC 8 (DECSC/DECRC) ────────────────────────────────
    {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x1b[10;15H"); // Position at (9, 14)
        p.process(b"\x1b7");        // DECSC — save
        p.process(b"\x1b[1;1H");    // Move to origin
        let (r, c) = p.screen().cursor_position();
        assert_eq!((r, c), (0, 0), "Should be at origin");
        p.process(b"\x1b8");        // DECRC — restore
        let (r, c) = p.screen().cursor_position();
        if (r, c) == (9, 14) {
            println!("TEST_DECSC_PASS: DECSC/DECRC correctly save/restore cursor");
            passed += 1;
        } else {
            println!("TEST_DECSC_FAIL: DECRC restored ({}, {}), expected (9, 14)", r, c);
            failed += 1;
        }
    }

    // ── Test 4: CSI s / CSI u (SCOSC/SCORC) ────────────────────────────────
    // This is the CRITICAL test for Issue #52
    {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x1b[10;15H"); // Position at (9, 14)
        p.process(b"\x1b[s");       // CSI s — save cursor
        p.process(b"\x1b[1;1H");    // Move to origin
        let (r, c) = p.screen().cursor_position();
        assert_eq!((r, c), (0, 0), "Should be at origin after CUP");
        p.process(b"\x1b[u");       // CSI u — restore cursor
        let (r, c) = p.screen().cursor_position();
        if (r, c) == (9, 14) {
            println!("TEST_SCOSC_PASS: CSI s/u correctly save/restore cursor at (9, 14)");
            passed += 1;
        } else {
            println!("TEST_SCOSC_FAIL: CSI u restored ({}, {}), expected (9, 14) — ROOT CAUSE of #52", r, c);
            failed += 1;
        }
    }

    // ── Test 5: hide_cursor flag ────────────────────────────────────────────
    {
        let mut p = vt100::Parser::new(24, 80, 0);
        assert!(!p.screen().hide_cursor(), "Cursor should be visible initially");

        p.process(b"\x1b[?25l"); // Hide cursor
        if p.screen().hide_cursor() {
            println!("TEST_HIDE_PASS: hide_cursor correctly set by CSI ?25l");
            passed += 1;
        } else {
            println!("TEST_HIDE_FAIL: hide_cursor not set after CSI ?25l");
            failed += 1;
        }

        p.process(b"\x1b[?25h"); // Show cursor
        if !p.screen().hide_cursor() {
            println!("TEST_SHOW_PASS: cursor correctly shown by CSI ?25h");
            passed += 1;
        } else {
            println!("TEST_SHOW_FAIL: cursor still hidden after CSI ?25h");
            failed += 1;
        }
    }

    // ── Test 6: TUI render pattern (Claude-like) ────────────────────────────
    {
        let mut p = vt100::Parser::new(30, 120, 0);

        // Simulate Claude's render cycle:
        // 1. Enter alternate screen
        p.process(b"\x1b[?1049h");
        // 2. Hide cursor during render
        p.process(b"\x1b[?25l");
        // 3. Clear and render header
        p.process(b"\x1b[2J");
        p.process(b"\x1b[1;1H");
        p.process(b"+----- Claude Code -----+");
        // 4. Render conversation
        p.process(b"\x1b[3;1H");
        p.process(b"| Human: Hello          |");
        p.process(b"\x1b[4;1H");
        p.process(b"| Claude: Hi!           |");
        // 5. Render input area at bottom
        p.process(b"\x1b[28;1H");
        p.process(b"+------------------------+");
        p.process(b"\x1b[27;3H");
        p.process(b"> ");
        // 6. Position cursor at input box and show cursor
        p.process(b"\x1b[27;5H");
        p.process(b"\x1b[?25h");

        let (r, c) = p.screen().cursor_position();
        let hidden = p.screen().hide_cursor();
        if (r, c) == (26, 4) && !hidden {
            println!("TEST_TUI_PASS: TUI cursor at (26, 4), visible — correct");
            passed += 1;
        } else {
            println!("TEST_TUI_FAIL: cursor=({}, {}), hidden={}, expected (26, 4, false)", r, c, hidden);
            failed += 1;
        }
    }

    // ── Test 7: Ink-style render with CSI s/u on Windows ────────────────────
    // This is the key pattern: Ink on Windows saves cursor, renders, restores
    {
        let mut p = vt100::Parser::new(30, 120, 0);
        p.process(b"\x1b[?1049h"); // Alternate screen

        // Initial render — position cursor at input
        p.process(b"\x1b[27;5H");

        // Ink partial update: save, render, restore
        p.process(b"\x1b[s");              // Save cursor at input (row 27, col 5)
        p.process(b"\x1b[?25l");           // Hide during render
        p.process(b"\x1b[1;40H");          // Move to status area
        p.process(b"[typing...]");          // Update status
        p.process(b"\x1b[u");              // Restore cursor to input area
        p.process(b"\x1b[?25h");           // Show cursor

        let (r, c) = p.screen().cursor_position();
        let hidden = p.screen().hide_cursor();
        if (r, c) == (26, 4) && !hidden {
            println!("TEST_INK_PASS: Ink-style CSI s/u correctly restores cursor to (26, 4)");
            passed += 1;
        } else {
            println!("TEST_INK_FAIL: cursor=({}, {}), hidden={}, expected (26, 4, false) — ISSUE #52 ROOT CAUSE", r, c, hidden);
            failed += 1;
        }
    }

    // ── Test 8: Multiple save/restore cycles ────────────────────────────────
    {
        let mut p = vt100::Parser::new(24, 80, 0);

        // First save/restore
        p.process(b"\x1b[5;5H");
        p.process(b"\x1b[s");
        p.process(b"\x1b[20;70H");
        p.process(b"\x1b[u");
        let (r, c) = p.screen().cursor_position();
        let ok1 = (r, c) == (4, 4);

        // Second save/restore (should overwrite previous save)
        p.process(b"\x1b[15;30H");
        p.process(b"\x1b[s");
        p.process(b"\x1b[1;1H");
        p.process(b"\x1b[u");
        let (r, c) = p.screen().cursor_position();
        let ok2 = (r, c) == (14, 29);

        if ok1 && ok2 {
            println!("TEST_MULTI_PASS: Multiple CSI s/u cycles work correctly");
            passed += 1;
        } else {
            println!("TEST_MULTI_FAIL: Multiple save/restore cycles broken");
            failed += 1;
        }
    }

    // ── Summary ─────────────────────────────────────────────────────────────
    println!("\n═══════════════════════════════════════════════");
    println!("  Results: {} passed, {} failed", passed, failed);
    println!("═══════════════════════════════════════════════");

    if failed > 0 {
        println!("ISSUE_52_REPRODUCED: Missing CSI s/u support causes cursor desync");
        std::process::exit(1);
    } else {
        println!("ALL_TESTS_PASSED: Issue #52 fixes verified");
    }
}
