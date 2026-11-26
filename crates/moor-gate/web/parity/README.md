# Portal parity against the task-flows mock

One tool compares the portal with the mock, one screen at a time. It covers a1, a1-box, a3, a4, b1, b1-plain, c1, c1-plain, c1b, c1c, b2, b3, c2, c3, d1, d2, d3 and h1 to h4.

```
portal-up NAME --worktree WT
NODE_PATH=~/node_modules node parity.cjs --mock ~/magic/moor/.local/hopper-variant --portal NAME [--only a1,h3] [--out DIR]
```

Each screen prints one line. It gives the share of pixels that differ, the text runs (ok, differ, mock only, product only) and the element pairs that differ. A final table sets the numbers against the ceilings. The exit code is 1 when a screen is over its ceiling or fails to load. `--out` keeps the paired PNG files, the red diff and the text and pair lists of each screen.

- `states.cjs` holds the screens, the selector pairs and the clicks on each side.
- `fixtures/` holds the API answers per area (task, automation, rules), with the mock's own data. The automation fixtures carry a `reading`, the controller's ReadingView. It holds the mock's solid and dashed phrases. b1 and c1 rest with the pointer on the phrase the mock rests on.
- `ceilings.cjs` holds today's numbers. It is a ratchet. Lower a ceiling when a screen gets closer to the mock. Raise one only with the reason in its `raisedBecause` field.
- A mask or a skipped screen needs its reason beside it in `states.cjs`.
