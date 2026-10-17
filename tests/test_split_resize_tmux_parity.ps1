# split-window / join-pane / resize-pane / kill-pane geometry against tmux 3.4.
#
# Every expected line below is tmux 3.4's own output (WSL Ubuntu,
# `tmux -L tanaesplit -f /dev/null new-session -d -x X -y Y`, then the case's
# commands), read back as `#{pane_index}:<creation rank>:#{pane_left},#{pane_top}:
# #{pane_width}x#{pane_height}:#{pane_active}` plus `#{window_layout}` with the
# checksum and pane ids blanked (they differ between the two programs).
#
# What this pins (all measured wrong on master 652d08b4 before the fix):
#   * split sizes in cells, tmux layout_split_sizes: 120 -> 60 | 59, 30 -> 15 / 14
#     (psmux gave 59 | 60 and 14 / 15), -l N and -l N% of the PANE
#   * resize-pane in cells, tmux layout_resize_pane: -R 1 three times is
#     61, 62, 63 (psmux 60, 61, 63), -D 1 three times on 30 rows moves 3
#     (psmux 0), -y 10 is 10 rows (psmux 2)
#   * a split the same way as its parent joins the parent, as tmux does
#     ({60,29,29}, psmux nested {59,{29,30}})
#   * a killed pane's cells go to its neighbour (tmux layout_destroy_cell),
#     psmux equalised the survivors
#   * split-window -b and -f (psmux ignored both), and join-pane -b sizes
#
#   * select-layout keeps the active pane (tmux layout_set never touches
#     w->active), even-*, tiled, main-* in cells with tmux's layout-set.c
#     arithmetic, main-pane-width / main-pane-height (defaults 80 and 24,
#     cells or N%), other-pane-width / other-pane-height, and the first
#     next-layout / previous-layout
#   * a zoomed window: a split unzooms first, and list-panes reports the
#     zoomed pane over the window and the others where they were
#
# Not compared here, on purpose: a pane pushed to the minimum (psmux keeps two
# columns, MIN_PTY_COLS, where tmux allows one).
#
# Binary: $env:PSMUX_BIN if set, else psmux on PATH. Own -L namespace;
# honours PSMUX_DATA_DIR. Set PSMUX_PARITY_ONLY to a regex to run some cases.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "splitparity" + (Get-Random -Maximum 99999)
$script:Pass = 0; $script:Fail = 0
$env:PSMUX_RUN_NOPAUSE = '1'
$env:PSMUX_NO_WARM = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }
Write-Host "binary: $PSMUX  namespace: $NS" -ForegroundColor Cyan
function P { & $PSMUX -L $NS @args 2>&1 }

# name | x | y | commands (;; separated, CMD = a command for the new pane) | tmux layout | tmux panes
$cases = @(
    ,@('h', 120, 30, 'split-window -h', 'f09b,120x30,0,0{60x30,0,0,0,59x30,61,0,1}', '0:0:0,0:60x30:0 1:1:61,0:59x30:1')
    ,@('v', 120, 30, 'split-window -v', 'e9aa,120x30,0,0[120x15,0,0,0,120x14,0,16,1]', '0:0:0,0:120x15:0 1:1:0,16:120x14:1')
    ,@('hb', 120, 30, 'split-window -h -b', '709f,120x30,0,0{60x30,0,0,1,59x30,61,0,0}', '0:1:0,0:60x30:1 1:0:61,0:59x30:0')
    ,@('vb', 120, 30, 'split-window -v -b', '69ac,120x30,0,0[120x15,0,0,1,120x14,0,16,0]', '0:1:0,0:120x15:1 1:0:0,16:120x14:0')
    ,@('hb_cmd', 120, 30, 'split-window -h -b CMD', '709f,120x30,0,0{60x30,0,0,1,59x30,61,0,0}', '0:1:0,0:60x30:1 1:0:61,0:59x30:0')
    ,@('vb_cmd', 120, 30, 'split-window -v -b CMD', '69ac,120x30,0,0[120x15,0,0,1,120x14,0,16,0]', '0:1:0,0:120x15:1 1:0:0,16:120x14:0')
    ,@('hbd', 120, 30, 'split-window -h -b -d', '709f,120x30,0,0{60x30,0,0,1,59x30,61,0,0}', '0:1:0,0:60x30:0 1:0:61,0:59x30:1')
    ,@('h_then_sel0_vb', 120, 30, 'split-window -h;;select-pane -t 0;;split-window -v -b', 'deae,120x30,0,0{60x30,0,0[60x15,0,0,2,60x14,0,16,0],59x30,61,0,1}', '0:2:0,0:60x15:1 1:0:0,16:60x14:0 2:1:61,0:59x30:0')
    ,@('h_then_sel1_hb', 120, 30, 'split-window -h;;select-pane -t 1;;split-window -h -b', 'f747,120x30,0,0{60x30,0,0,0,29x30,61,0,2,29x30,91,0,1}', '0:0:0,0:60x30:0 1:2:61,0:29x30:1 2:1:91,0:29x30:0')
    ,@('h_then_vb_on1', 120, 30, 'split-window -h;;split-window -v -b', '8523,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,2,59x14,61,16,1]}', '0:0:0,0:60x30:0 1:2:61,0:59x15:1 2:1:61,16:59x14:0')
    ,@('h_then_fhb', 120, 30, 'split-window -h;;split-window -f -h -b', 'f77f,120x30,0,0{60x30,0,0,2,29x30,61,0,0,29x30,91,0,1}', '0:2:0,0:60x30:1 1:0:61,0:29x30:0 2:1:91,0:29x30:0')
    ,@('h_then_fvb', 120, 30, 'split-window -h;;split-window -f -v -b', '6fb1,120x30,0,0[120x15,0,0,2,120x14,0,16{60x14,0,16,0,59x14,61,16,1}]', '0:2:0,0:120x15:1 1:0:0,16:60x14:0 2:1:61,16:59x14:0')
    ,@('h_then_fv', 120, 30, 'split-window -h;;split-window -f -v', '71f7,120x30,0,0[120x15,0,0{60x15,0,0,0,59x15,61,0,1},120x14,0,16,2]', '0:0:0,0:60x15:0 1:1:61,0:59x15:0 2:2:0,16:120x14:1')
    ,@('v_then_fh', 120, 30, 'split-window -v;;split-window -f -h', '5e91,120x30,0,0{60x30,0,0[60x15,0,0,0,60x14,0,16,1],59x30,61,0,2}', '0:0:0,0:60x15:0 1:1:0,16:60x14:0 2:2:61,0:59x30:1')
    ,@('l30_h', 120, 30, 'split-window -h -l 30', '075c,120x30,0,0{89x30,0,0,0,30x30,90,0,1}', '0:0:0,0:89x30:0 1:1:90,0:30x30:1')
    ,@('l30p_h', 120, 30, 'split-window -h -l 30%', '181c,120x30,0,0{83x30,0,0,0,36x30,84,0,1}', '0:0:0,0:83x30:0 1:1:84,0:36x30:1')
    ,@('l10_vb', 120, 30, 'split-window -v -b -l 10', 'a6ab,120x30,0,0[120x10,0,0,1,120x19,0,11,0]', '0:1:0,0:120x10:1 1:0:0,11:120x19:0')
    ,@('l25p_v', 120, 30, 'split-window -v -l 25%', '30c8,120x30,0,0[120x22,0,0,0,120x7,0,23,1]', '0:0:0,0:120x22:0 1:1:0,23:120x7:1')
    ,@('l30_hb', 120, 30, 'split-window -h -b -l 30', '61cf,120x30,0,0{30x30,0,0,1,89x30,31,0,0}', '0:1:0,0:30x30:1 1:0:31,0:89x30:0')
    ,@('odd_h', 121, 31, 'split-window -h', '0090,121x31,0,0{60x31,0,0,0,60x31,61,0,1}', '0:0:0,0:60x31:0 1:1:61,0:60x31:1')
    ,@('odd_v', 121, 31, 'split-window -v', '0bcc,121x31,0,0[121x15,0,0,0,121x15,0,16,1]', '0:0:0,0:121x15:0 1:1:0,16:121x15:1')
    ,@('odd_hb', 121, 31, 'split-window -h -b', '8093,121x31,0,0{60x31,0,0,1,60x31,61,0,0}', '0:1:0,0:60x31:1 1:0:61,0:60x31:0')
    ,@('even_v', 120, 31, 'split-window -v', 'eaab,120x31,0,0[120x15,0,0,0,120x15,0,16,1]', '0:0:0,0:120x15:0 1:1:0,16:120x15:1')
    ,@('three_h', 120, 30, 'split-window -h;;split-window -h', '7744,120x30,0,0{60x30,0,0,0,29x30,61,0,1,29x30,91,0,2}', '0:0:0,0:60x30:0 1:1:61,0:29x30:0 2:2:91,0:29x30:1')
    ,@('three_h_sel0', 120, 30, 'split-window -h;;select-pane -t 0;;split-window -h', '7377,120x30,0,0{30x30,0,0,0,29x30,31,0,2,59x30,61,0,1}', '0:0:0,0:30x30:0 1:2:31,0:29x30:1 2:1:61,0:59x30:0')
    ,@('four_tiled', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;split-window -v', '435e,120x30,0,0{60x30,0,0[60x15,0,0,0,60x14,0,16,3],59x30,61,0[59x15,61,0,1,59x14,61,16,2]}', '0:0:0,0:60x15:0 1:3:0,16:60x14:1 2:1:61,0:59x15:0 3:2:61,16:59x14:0')
    ,@('resR', 120, 30, 'split-window -h;;resize-pane -t 0 -R 1', 'fa7b,120x30,0,0{61x30,0,0,0,58x30,62,0,1}', '0:0:0,0:61x30:0 1:1:62,0:58x30:1')
    ,@('resR2', 120, 30, 'split-window -h;;resize-pane -t 0 -R 1;;resize-pane -t 0 -R 1', '045c,120x30,0,0{62x30,0,0,0,57x30,63,0,1}', '0:0:0,0:62x30:0 1:1:63,0:57x30:1')
    ,@('resR3', 120, 30, 'split-window -h;;resize-pane -t 0 -R 1;;resize-pane -t 0 -R 1;;resize-pane -t 0 -R 1', '0e3c,120x30,0,0{63x30,0,0,0,56x30,64,0,1}', '0:0:0,0:63x30:0 1:1:64,0:56x30:1')
    ,@('resR6', 120, 30, 'split-window -h;;resize-pane -t 0 -R 6', '2bdc,120x30,0,0{66x30,0,0,0,53x30,67,0,1}', '0:0:0,0:66x30:0 1:1:67,0:53x30:1')
    ,@('resL3', 120, 30, 'split-window -h;;resize-pane -t 0 -L 1;;resize-pane -t 0 -L 1;;resize-pane -t 0 -L 1', '30cc,120x30,0,0{57x30,0,0,0,62x30,58,0,1}', '0:0:0,0:57x30:0 1:1:58,0:62x30:1')
    ,@('resR_on1', 120, 30, 'split-window -h;;resize-pane -t 1 -R 1;;resize-pane -t 1 -R 1;;resize-pane -t 1 -R 1', '0e3c,120x30,0,0{63x30,0,0,0,56x30,64,0,1}', '0:0:0,0:63x30:0 1:1:64,0:56x30:1')
    ,@('resL_on1', 120, 30, 'split-window -h;;resize-pane -t 1 -L 1;;resize-pane -t 1 -L 1;;resize-pane -t 1 -L 1', '30cc,120x30,0,0{57x30,0,0,0,62x30,58,0,1}', '0:0:0,0:57x30:0 1:1:58,0:62x30:1')
    ,@('resD3', 120, 30, 'split-window -v;;resize-pane -t 0 -D 1;;resize-pane -t 0 -D 1;;resize-pane -t 0 -D 1', '5eab,120x30,0,0[120x18,0,0,0,120x11,0,19,1]', '0:0:0,0:120x18:0 1:1:0,19:120x11:1')
    ,@('resU3', 120, 30, 'split-window -v;;resize-pane -t 0 -U 1;;resize-pane -t 0 -U 1;;resize-pane -t 0 -U 1', '74aa,120x30,0,0[120x12,0,0,0,120x17,0,13,1]', '0:0:0,0:120x12:0 1:1:0,13:120x17:1')
    ,@('resx', 120, 30, 'split-window -h;;resize-pane -t 0 -x 40', 'e6bb,120x30,0,0{40x30,0,0,0,79x30,41,0,1}', '0:0:0,0:40x30:0 1:1:41,0:79x30:1')
    ,@('resy', 120, 30, 'split-window -v;;resize-pane -t 0 -y 10', '26aa,120x30,0,0[120x10,0,0,0,120x19,0,11,1]', '0:0:0,0:120x10:0 1:1:0,11:120x19:1')
    ,@('res3R', 120, 30, 'split-window -h;;split-window -h;;resize-pane -t 0 -R 1;;resize-pane -t 0 -R 1;;resize-pane -t 0 -R 1', '6445,120x30,0,0{63x30,0,0,0,26x30,64,0,1,29x30,91,0,2}', '0:0:0,0:63x30:0 1:1:64,0:26x30:0 2:2:91,0:29x30:1')
    ,@('join_hb', 120, 30, 'new-window -d;;join-pane -h -b -s :1 -t :0', '709f,120x30,0,0{60x30,0,0,1,59x30,61,0,0}', '0:1:0,0:60x30:1 1:0:61,0:59x30:0')
    ,@('join_vb', 120, 30, 'new-window -d;;join-pane -v -b -s :1 -t :0', '69ac,120x30,0,0[120x15,0,0,1,120x14,0,16,0]', '0:1:0,0:120x15:1 1:0:0,16:120x14:0')
    ,@('join_h', 120, 30, 'new-window -d;;join-pane -h -s :1 -t :0', 'f09b,120x30,0,0{60x30,0,0,0,59x30,61,0,1}', '0:0:0,0:60x30:0 1:1:61,0:59x30:1')
    ,@('kill_mid3', 120, 30, 'split-window -h;;split-window -h;;kill-pane -t 1', '7f6c,120x30,0,0{90x30,0,0,0,29x30,91,0,2}', '0:0:0,0:90x30:0 1:1:91,0:29x30:1')
    ,@('kill_first3', 120, 30, 'split-window -h;;split-window -h;;kill-pane -t 0', '7f70,120x30,0,0{90x30,0,0,1,29x30,91,0,2}', '0:0:0,0:90x30:0 1:1:91,0:29x30:1')
    ,@('kill_last3', 120, 30, 'split-window -h;;split-window -h;;kill-pane -t 2', 'f09b,120x30,0,0{60x30,0,0,0,59x30,61,0,1}', '0:0:0,0:60x30:0 1:1:61,0:59x30:1')
    ,@('kill_tiled', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;split-window -v;;kill-pane -t 1', 'c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}', '0:0:0,0:60x30:1 1:1:61,0:59x15:0 2:2:61,16:59x14:0')
    ,@('resR_nested', 120, 30, 'split-window -h;;split-window -v;;resize-pane -t 1 -R 1;;resize-pane -t 1 -R 1', 'd897,120x30,0,0{62x30,0,0,0,57x30,63,0[57x15,63,0,1,57x14,63,16,2]}', '0:0:0,0:62x30:0 1:1:63,0:57x15:0 2:2:63,16:57x14:1')
    ,@('resL_nested', 120, 30, 'split-window -h;;split-window -v;;resize-pane -t 2 -L 5', '30ea,120x30,0,0{55x30,0,0,0,64x30,56,0[64x15,56,0,1,64x14,56,16,2]}', '0:0:0,0:55x30:0 1:1:56,0:64x15:0 2:2:56,16:64x14:1')
    ,@('resD_nested', 120, 30, 'split-window -h;;split-window -v;;resize-pane -t 1 -D 2', 'e8a2,120x30,0,0{60x30,0,0,0,59x30,61,0[59x17,61,0,1,59x12,61,18,2]}', '0:0:0,0:60x30:0 1:1:61,0:59x17:0 2:2:61,18:59x12:1')
    ,@('resU_on_last', 120, 30, 'split-window -v;;split-window -v;;resize-pane -t 2 -U 3', '6f9a,120x30,0,0[120x15,0,0,0,120x4,0,16,1,120x9,0,21,2]', '0:0:0,0:120x15:0 1:1:0,16:120x4:0 2:2:0,21:120x9:1')
    ,@('hb_cluster', 120, 30, 'split-window -hb', '709f,120x30,0,0{60x30,0,0,1,59x30,61,0,0}', '0:1:0,0:60x30:1 1:0:61,0:59x30:0')
    ,@('sl_evenh', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout even-horizontal', '04f7,120x30,0,0{29x30,0,0,0,29x30,30,0,1,29x30,60,0,2,30x30,90,0,3}', '0:0:0,0:29x30:0 1:1:30,0:29x30:0 2:2:60,0:29x30:0 3:3:90,0:30x30:1')
    ,@('sl_evenv', 120, 30, 'split-window -v;;split-window -v;;split-window -v;;select-layout even-vertical', '7fc8,120x30,0,0[120x6,0,0,0,120x6,0,7,1,120x6,0,14,2,120x9,0,21,3]', '0:0:0,0:120x6:0 1:1:0,7:120x6:0 2:2:0,14:120x6:0 3:3:0,21:120x9:1')
    ,@('sl_tiled', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout tiled', '69c5,120x30,0,0[120x14,0,0{59x14,0,0,0,60x14,60,0,1},120x15,0,15{59x15,0,15,2,60x15,60,15,3}]', '0:0:0,0:59x14:0 1:1:60,0:60x14:0 2:2:0,15:59x15:0 3:3:60,15:60x15:1')
    ,@('sl_evenh_sel1', 120, 30, 'split-window -h;;split-window -h;;select-pane -t 1;;select-layout even-horizontal', '62c4,120x30,0,0{39x30,0,0,0,39x30,40,0,1,40x30,80,0,2}', '0:0:0,0:39x30:0 1:1:40,0:39x30:1 2:2:80,0:40x30:0')
    ,@('sl_tiled_sel0', 120, 30, 'split-window -h;;split-window -v;;split-window -h;;select-pane -t 0;;select-layout tiled', '69c5,120x30,0,0[120x14,0,0{59x14,0,0,0,60x14,60,0,1},120x15,0,15{59x15,0,15,2,60x15,60,15,3}]', '0:0:0,0:59x14:1 1:1:60,0:60x14:0 2:2:0,15:59x15:0 3:3:60,15:60x15:0')
    ,@('sl_mainv', 120, 30, 'split-window -h;;split-window -h;;select-layout main-vertical', '3d1d,120x30,0,0{80x30,0,0,0,39x30,81,0[39x14,81,0,1,39x15,81,15,2]}', '0:0:0,0:80x30:0 1:1:81,0:39x14:0 2:2:81,15:39x15:1')
    ,@('sl_mainh', 120, 30, 'split-window -v;;split-window -v;;select-layout main-horizontal', '3d63,120x30,0,0[120x24,0,0,0,120x5,0,25{59x5,0,25,1,60x5,60,25,2}]', '0:0:0,0:120x24:0 1:1:0,25:59x5:0 2:2:60,25:60x5:1')
    ,@('sl_mainv_sel1', 120, 30, 'split-window -h;;split-window -h;;select-pane -t 1;;select-layout main-vertical', '3d1d,120x30,0,0{80x30,0,0,0,39x30,81,0[39x14,81,0,1,39x15,81,15,2]}', '0:0:0,0:80x30:0 1:1:81,0:39x14:1 2:2:81,15:39x15:0')
    ,@('sl_mainv4', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout main-vertical', '5ff0,120x30,0,0{80x30,0,0,0,39x30,81,0[39x9,81,0,1,39x9,81,10,2,39x10,81,20,3]}', '0:0:0,0:80x30:0 1:1:81,0:39x9:0 2:2:81,10:39x9:0 3:3:81,20:39x10:1')
    ,@('sl_mainh4', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout main-horizontal', 'f241,120x30,0,0[120x24,0,0,0,120x5,0,25{39x5,0,25,1,39x5,40,25,2,40x5,80,25,3}]', '0:0:0,0:120x24:0 1:1:0,25:39x5:0 2:2:40,25:39x5:0 3:3:80,25:40x5:1')
    ,@('sl_mainv_w50', 120, 30, 'split-window -h;;split-window -h;;set-option -w main-pane-width 50;;select-layout main-vertical', '6e85,120x30,0,0{50x30,0,0,0,69x30,51,0[69x14,51,0,1,69x15,51,15,2]}', '0:0:0,0:50x30:0 1:1:51,0:69x14:0 2:2:51,15:69x15:1')
    ,@('sl_mainv_w30p', 120, 30, 'split-window -h;;split-window -h;;set-option -w main-pane-width 30%;;select-layout main-vertical', '956f,120x30,0,0{35x30,0,0,0,84x30,36,0[84x14,36,0,1,84x15,36,15,2]}', '0:0:0,0:35x30:0 1:1:36,0:84x14:0 2:2:36,15:84x15:1')
    ,@('sl_mainh_h10', 120, 30, 'split-window -v;;split-window -v;;set-option -w main-pane-height 10;;select-layout main-horizontal', '699c,120x30,0,0[120x10,0,0,0,120x19,0,11{59x19,0,11,1,60x19,60,11,2}]', '0:0:0,0:120x10:0 1:1:0,11:59x19:0 2:2:60,11:60x19:1')
    ,@('sl_mainh_h50p', 120, 30, 'split-window -v;;split-window -v;;set-option -w main-pane-height 50%;;select-layout main-horizontal', 'a0c4,120x30,0,0[120x14,0,0,0,120x15,0,15{59x15,0,15,1,60x15,60,15,2}]', '0:0:0,0:120x14:0 1:1:0,15:59x15:0 2:2:60,15:60x15:1')
    ,@('sl_mainh_small', 80, 20, 'split-window -v;;split-window -v;;select-layout main-horizontal', 'd8fe,80x20,0,0[80x18,0,0,0,80x1,0,19{39x1,0,19,1,40x1,40,19,2}]', '0:0:0,0:80x18:0 1:1:0,19:39x1:0 2:2:40,19:40x1:1')
    ,@('sl_mainv_odd', 121, 31, 'split-window -h;;split-window -h;;select-layout main-vertical', '49dd,121x31,0,0{80x31,0,0,0,40x31,81,0[40x15,81,0,1,40x15,81,16,2]}', '0:0:0,0:80x31:0 1:1:81,0:40x15:0 2:2:81,16:40x15:1')
    ,@('sl_next', 120, 30, 'split-window -h;;split-window -h;;next-layout', '62c4,120x30,0,0{39x30,0,0,0,39x30,40,0,1,40x30,80,0,2}', '0:0:0,0:39x30:0 1:1:40,0:39x30:0 2:2:80,0:40x30:1')
    ,@('sl_then_split', 120, 30, 'split-window -h;;split-window -h;;select-layout even-horizontal;;split-window -v', '8c74,120x30,0,0{39x30,0,0,0,39x30,40,0,1,40x30,80,0[40x15,80,0,2,40x14,80,16,3]}', '0:0:0,0:39x30:0 1:1:40,0:39x30:0 2:2:80,0:40x15:0 3:3:80,16:40x14:1')
    ,@('sl_then_res', 120, 30, 'split-window -h;;split-window -h;;select-layout main-vertical;;resize-pane -t 0 -R 1', 'c6d7,120x30,0,0{81x30,0,0,0,38x30,82,0[38x14,82,0,1,38x15,82,15,2]}', '0:0:0,0:81x30:0 1:1:82,0:38x14:0 2:2:82,15:38x15:1')
    ,@('zoom_split_v', 120, 30, 'split-window -h;;resize-pane -Z;;split-window -v', 'c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}', '0:0:0,0:60x30:0 1:1:61,0:59x15:0 2:2:61,16:59x14:1')
    ,@('zoom_split_h', 120, 30, 'split-window -h;;select-pane -t 0;;resize-pane -Z;;split-window -h', '7377,120x30,0,0{30x30,0,0,0,29x30,31,0,2,59x30,61,0,1}', '0:0:0,0:30x30:0 1:2:31,0:29x30:1 2:1:61,0:59x30:0')
    ,@('zoom_split_res', 120, 30, 'split-window -h;;resize-pane -t 0 -R 3;;resize-pane -Z;;split-window -v', '6252,120x30,0,0{63x30,0,0,0,56x30,64,0[56x15,64,0,1,56x14,64,16,2]}', '0:0:0,0:63x30:0 1:1:64,0:56x15:0 2:2:64,16:56x14:1')
    ,@('zoom_kill', 120, 30, 'split-window -h;;split-window -h;;resize-pane -Z;;kill-pane', 'f09b,120x30,0,0{60x30,0,0,0,59x30,61,0,1}', '0:0:0,0:60x30:0 1:1:61,0:59x30:1')
    ,@('zoom_split_Z', 120, 30, 'split-window -h;;resize-pane -Z;;split-window -v -Z', 'c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}', '0:0:0,0:60x30:0 1:1:61,0:59x15:0 2:2:0,0:120x30:1')
    ,@('zoom_only', 120, 30, 'split-window -h;;resize-pane -Z', 'f09b,120x30,0,0{60x30,0,0,0,59x30,61,0,1}', '0:0:0,0:60x30:0 1:1:0,0:120x30:1')
    ,@('zoom_only3', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;resize-pane -Z', 'c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}', '0:0:0,0:120x30:1 1:1:61,0:59x15:0 2:2:61,16:59x14:0')
    ,@('sl_tiled3', 120, 30, 'split-window -h;;split-window -h;;select-layout tiled', '40cf,120x30,0,0[120x14,0,0{59x14,0,0,0,60x14,60,0,1},120x15,0,15,2]', '0:0:0,0:59x14:0 1:1:60,0:60x14:0 2:2:0,15:120x15:1')
    ,@('sl_tiled5', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;split-window -v;;split-window -h;;select-layout tiled', '4631,120x30,0,0[120x9,0,0{59x9,0,0,0,60x9,60,0,3},120x9,0,10{59x9,0,10,4,60x9,60,10,1},120x10,0,20,2]', '0:0:0,0:59x9:0 1:3:60,0:60x9:0 2:4:0,10:59x9:1 3:1:60,10:60x9:0 4:2:0,20:120x10:0')
    ,@('sl_tiled6', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;split-window -v;;split-window -h;;select-pane -t 4;;split-window -h;;select-layout tiled', 'cf05,120x30,0,0[120x9,0,0{59x9,0,0,0,60x9,60,0,3},120x9,0,10{59x9,0,10,4,60x9,60,10,1},120x10,0,20{59x10,0,20,2,60x10,60,20,5}]', '0:0:0,0:59x9:0 1:3:60,0:60x9:0 2:4:0,10:59x9:0 3:1:60,10:60x9:0 4:2:0,20:59x10:0 5:5:60,20:60x10:1')
    ,@('sl_evenh5', 120, 30, 'split-window -h;;split-window -v;;select-pane -t 0;;split-window -v;;split-window -h;;select-layout even-horizontal', 'd7e1,120x30,0,0{23x30,0,0,0,23x30,24,0,3,23x30,48,0,4,23x30,72,0,1,24x30,96,0,2}', '0:0:0,0:23x30:0 1:3:24,0:23x30:0 2:4:48,0:23x30:1 3:1:72,0:23x30:0 4:2:96,0:24x30:0')
    ,@('sl_mainv_ow30', 120, 30, 'split-window -h;;split-window -h;;set-option -w other-pane-width 30;;select-layout main-vertical', '6998,120x30,0,0{89x30,0,0,0,30x30,90,0[30x14,90,0,1,30x15,90,15,2]}', '0:0:0,0:89x30:0 1:1:90,0:30x14:0 2:2:90,15:30x15:1')
    ,@('sl_mainh_oh10', 120, 30, 'split-window -v;;split-window -v;;set-option -w other-pane-height 10;;select-layout main-horizontal', '3d63,120x30,0,0[120x24,0,0,0,120x5,0,25{59x5,0,25,1,60x5,60,25,2}]', '0:0:0,0:120x24:0 1:1:0,25:59x5:0 2:2:60,25:60x5:1')
    ,@('sl_mainv_g', 120, 30, 'set-option -g main-pane-width 60;;split-window -h;;split-window -h;;select-layout main-vertical', 'b362,120x30,0,0{60x30,0,0,0,59x30,61,0[59x14,61,0,1,59x15,61,15,2]}', '0:0:0,0:60x30:0 1:1:61,0:59x14:0 2:2:61,15:59x15:1')
    ,@('sl_prev', 120, 30, 'split-window -h;;split-window -h;;previous-layout', '40cf,120x30,0,0[120x14,0,0{59x14,0,0,0,60x14,60,0,1},120x15,0,15,2]', '0:0:0,0:59x14:0 1:1:60,0:60x14:0 2:2:0,15:120x15:1')
    ,@('evenh_res', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout even-horizontal;;resize-pane -t 1 -R 1', 'fff6,120x30,0,0{29x30,0,0,0,30x30,30,0,1,28x30,61,0,2,30x30,90,0,3}', '0:0:0,0:29x30:0 1:1:30,0:30x30:0 2:2:61,0:28x30:0 3:3:90,0:30x30:1')
    ,@('evenh_kill', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout even-horizontal;;kill-pane -t 1', '3639,120x30,0,0{59x30,0,0,0,29x30,60,0,2,30x30,90,0,3}', '0:0:0,0:59x30:0 1:1:60,0:29x30:0 2:2:90,0:30x30:1')
    ,@('tiled4', 120, 30, 'split-window -h;;split-window -h;;split-window -h;;select-layout tiled', '69c5,120x30,0,0[120x14,0,0{59x14,0,0,0,60x14,60,0,1},120x15,0,15{59x15,0,15,2,60x15,60,15,3}]', '0:0:0,0:59x14:0 1:1:60,0:60x14:0 2:2:0,15:59x15:0 3:3:60,15:60x15:1')
    ,@('mainv', 120, 30, 'split-window -h;;split-window -h;;select-layout main-vertical', '3d1d,120x30,0,0{80x30,0,0,0,39x30,81,0[39x14,81,0,1,39x15,81,15,2]}', '0:0:0,0:80x30:0 1:1:81,0:39x14:0 2:2:81,15:39x15:1')
)

function Strip-Layout([string]$l) {
    $l = $l -replace '^[0-9a-f]{4},', ''
    ($l -replace ',(\d+)([\]\},])', ',P$2') -replace ',(\d+)$', ',P'
}

foreach ($c in $cases) {
    if ($null -eq $c) { continue }
    $name, $x, $y, $cmds, $wantLayout, $wantPanes = $c
    if ($env:PSMUX_PARITY_ONLY -and $name -notmatch $env:PSMUX_PARITY_ONLY) { continue }
    $s = "c" + (Get-Random -Maximum 99999)
    P new-session -d -s $s -x $x -y $y | Out-Null
    foreach ($cmd in ($cmds -split ';;')) {
        $w = @($cmd.Trim() -split '\s+' | ForEach-Object { if ($_ -eq 'CMD') { 'cmd'; '/k'; 'rem' } else { $_ } })
        for ($i = 0; $i -lt $w.Count; $i++) {
            if ($w[$i] -eq '-t' -and $w[$i+1] -match '^\d$') { $w[$i+1] = "${s}:." + $w[$i+1] }
            if ($w[$i] -eq '-s' -and $w[$i+1] -eq ':1') { $w[$i+1] = "${s}:1" }
            if ($w[$i] -eq '-t' -and $w[$i+1] -eq ':0') { $w[$i+1] = "${s}:0" }
        }
        if ($w[0] -eq 'new-window') { $w += @('-t', "${s}:1") }
        elseif ($w -notcontains '-t') { $w = @($w[0], '-t', $s) + @($w | Select-Object -Skip 1) }
        P @w | Out-Null
        Start-Sleep -Milliseconds 400
    }
    Start-Sleep -Milliseconds 300
    $lay = Strip-Layout ((P display-message -p -t "${s}:0" '#{window_layout}') | Out-String).Trim()
    $rows = @((P list-panes -t "${s}:0" -F '#{pane_index} #{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height} #{pane_active}') | ForEach-Object { "$_".Trim() } | Where-Object { $_ -match '^\d+ %' })
    $ids = @($rows | ForEach-Object { [int](($_ -split ' ')[1].Substring(1)) })
    $got = ($rows | ForEach-Object { $f = $_ -split ' '; $id = [int]$f[1].Substring(1); $r = @($ids | Where-Object { $_ -lt $id }).Count; "{0}:{1}:{2},{3}:{4}x{5}:{6}" -f $f[0], $r, $f[2], $f[3], $f[4], $f[5], $f[6] }) -join ' '
    P kill-session -t $s | Out-Null
    if ($got -eq $wantPanes -and $lay -eq (Strip-Layout $wantLayout)) {
        Write-Host "  [PASS] $name  $got" -ForegroundColor Green; $script:Pass++
    } else {
        Write-Host "  [FAIL] $name" -ForegroundColor Red
        Write-Host "         tmux : $wantPanes   $(Strip-Layout $wantLayout)"
        Write-Host "         psmux: $got   $lay"
        $script:Fail++
    }
}

P kill-server | Out-Null
Write-Host "`nPassed: $script:Pass  Failed: $script:Fail" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
