# prowl

**P**ID **R**ecursion: **O**bserve, **W**atch, **L**isten.

A Linux-only TUI that perches on a PID and silently watches its whole
descendant tree, CPU, memory, threads, lineage.

[![asciicast](https://asciinema.org/a/txpR2lr9w0tuG42z.svg)](https://asciinema.org/a/txpR2lr9w0tuG42z)

```shell
prowl [pid] [--interval 1000] [--threads]
```

Omit the PID to launch an interactive process picker.

## Keys

| Key                 | Action                               |
| :------------------ | :----------------------------------- |
| `q` / `Esc`         | Quit                                 |
| `↑` / `↓`           | Navigate the tree                    |
| `←` / `→`           | Scroll the command column            |
| `Ctrl+↑` / `Ctrl+↓` | Jump to the first / last row         |
| `Ctrl+←` / `Ctrl+→` | Jump to the leftmost / rightmost     |
| `Space`             | Collapse / expand the subtree        |
| `Enter`             | Open / close the detail panel        |
| `t`                 | Toggle thread visibility             |
| `+` / `-`           | Slow down / speed up polling (100ms) |

