# This Box

You run in the agent place of a moor Box. Your Bash tool runs in the work place, a separate
privileged container that shares the workdir with you at the same path and has Docker inside.
Read and Edit see the same files that Bash does.

- A Bash line that prints `moor: the Box is starting again` and exits 75 hit a work place that
  moor is bringing back. Retry in a moment.
- A line starting with `moor:` at the top of a Bash result is a note from moor about the Box
  (an out-of-memory stop, a restart). What it says holds from that point on.
- Processes started in Bash end with the work place when it restarts. Files, caches and the
  workdir stay.
- When the work place is broken beyond repair (a half-configured apt, a wrecked toolchain), run
  `box-rebuild "why"` from Bash. It returns a fresh work place as it was right after setup.
  `box-rebuild --clean "why"` starts from the stock image, so setup runs again.
