# Key Bindings

Default prefix: `Ctrl+b` (same as tmux)

## Prefix Keys

| Key | Action |
|-----|--------|
| `Prefix + c` | Create new window |
| `Prefix + %` | Split pane left/right |
| `Prefix + "` | Split pane top/bottom |
| `Prefix + x` | Kill current pane |
| `Prefix + &` | Kill current window |
| `Prefix + z` | Toggle pane zoom |
| `Prefix + n` | Next window |
| `Prefix + p` | Previous window |
| `Prefix + 0-9` | Select window by number |
| `Prefix + d` | Detach from session |
| `Prefix + ,` | Rename current window |
| `Prefix + t` | Show clock |
| `Prefix + s` | Session chooser/switcher |
| `Prefix + o` | Select next pane |
| `Prefix + w` | Window/pane chooser |
| `Prefix + [` | Enter copy/scroll mode |
| `Prefix + {` | Swap pane up |
| `Prefix + ]` | Paste from buffer |
| `Prefix + q` | Display pane numbers |
| `Prefix + Arrow` | Navigate between panes |
| `Ctrl+q` | Quit |

## Copy/Scroll Mode

Enter copy mode with `Prefix + [` to scroll through terminal history with **53 vim-style keybindings**:

| Key | Action |
|-----|--------|
| `↑` / `k` | Move cursor / scroll up |
| `↓` / `j` | Move cursor / scroll down |
| `h` / `l` | Move cursor left / right |
| `w` / `b` / `e` | Next word / prev word / end of word |
| `W` / `B` / `E` | WORD variants (whitespace-delimited) |
| `0` / `$` / `^` | Start / end / first non-blank of line |
| `g` / `G` | Jump to top / bottom of scrollback |
| `H` / `M` / `L` | Top / middle / bottom of screen |
| `Ctrl+u` / `Ctrl+d` | Scroll half page up / down |
| `Ctrl+b` / `Ctrl+f` | Scroll full page up / down |
| `f{char}` / `F{char}` | Find char forward / backward |
| `t{char}` / `T{char}` | Till char forward / backward |
| `%` | Jump to matching bracket |
| `{` / `}` | Previous / next paragraph |
| `/` / `?` | Search forward / backward |
| `n` / `N` | Next / previous match |
| `v` | Begin selection |
| `V` | Line selection |
| `Ctrl+v` | Rectangle selection |
| `o` | Swap selection ends |
| `y` / `Enter` | Yank (copy) selection |
| `D` | Copy to end of line |
| `"a`–`"z` | Named registers |
| `1`–`9` | Count prefix for motions |
| `Mouse drag` | Select text → copies to clipboard on release |
| `Esc` / `q` | Exit copy mode |

When in copy mode:
- The pane border turns **yellow**
- `[copy mode]` appears in the title
- A scroll position indicator shows in the top-right corner
- Mouse selection in copy mode is copied to the Windows clipboard on release
