# prowl

**P**ID **R**ecursion: **O**bserve, **W**atch, **L**isten.

A Linux-only TUI that perches on a PID and silently watches its whole
descendant tree, CPU, memory, threads, lineage.

```shell
prowl [pid] [--interval 1000] [--threads]
```

Omit the PID to launch an interactive process picker.

## Keys

| Key         | Action                               |
| :---------- | :----------------------------------- |
| `q` / `Esc` | Quit                                 |
| `↑` / `↓`   | Navigate the tree                    |
| `Enter`     | Collapse / expand the subtree        |
| `t`         | Toggle thread visibility             |
| `+` / `-`   | Slow down / speed up polling (100ms) |
