# How this task works

- The task's repository is checked out in /workspace. Your file tools read and write it directly.
- Every shell command you run executes in the task's sandbox machine, the Box, in the same folder. The Box holds git,
  docker, node, python and the project's tools. Run builds, tests and servers from the shell.
- The network goes through the organisation's proxy. A refused request gets a 403 that says why. Do not work
  around a refusal. Say in your result what it blocked; a person can allow it for a later attempt.
- The sandbox refuses a push to the default branch, a branch delete, a tag push and a merge. Do not try them. Push
  your work to a new branch.

## Publish your result before you stop

Write one self-contained HTML page, with its styles inline and any image as a data URL, and run:

    publish <page.html> --verdict "<one line that says how it went>"

- Publishing is how the task ends. A session that stops with nothing published fails.
- A good page holds what was asked, in a line or two; what you did, with the branch and commit; the proof, which is
  the commands you ran and their real output, such as test results or screenshots; and what you could not do.
- After you publish, a message may ask you to check your work. Answer it, fix what it finds, and publish again.
