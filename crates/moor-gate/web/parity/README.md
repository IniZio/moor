# Portal parity against the task-flows mock

One tool compares the portal with the mock screen by screen (a1, a1-box, a3, a4, b1, b1-plain, c1, c1-plain, c1b, c1c, b2, b3, c2, c3, d1, d2, d3, h1-h4):

```
portal-up NAME --worktree WT
NODE_PATH=~/node_modules node parity.cjs --mock ~/magic/moor/.local/hopper-variant --portal NAME [--only a1,h3] [--out DIR]
```

It prints one line per screen (pixels differing, text runs ok / differ / mock only / product only, element pairs differing), then a table against `ceilings.cjs`, and exits 1 when a screen is over a ceiling or fails to load. `--out` keeps the paired PNGs, the red diff and the per-screen text and pair lists.

- `states.cjs`: the screens, the pairs of selectors, what to click on each side.
- `fixtures/`: the API answers per area (task, automation, rules), carrying the mock's own data.
- `ceilings.cjs`: today's numbers. A ratchet: lower a ceiling when a screen gets closer to the mock; raise one only with the reason written on the line above it.
- A mask or a skipped screen needs a reason beside it in `states.cjs`.
