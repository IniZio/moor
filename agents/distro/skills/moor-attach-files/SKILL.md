---
name: moor-attach-files
description: Show a file from the artifacts folder on a pull request, such as a screenshot or a recording. Use before you put any file in a pull request body or comment.
metadata:
  version: "1"
---

# Attach files to a pull request

A pull request body that links a path inside the Box shows a broken image, since GitHub cannot read the Box. Upload the file instead, with gh.

1. Publish the file first: copy it to the top level of the artifacts folder your facts name, with a plain name and its extension.
2. In the body, write `![<name>](<artifacts folder>/<name>)` where the file should appear.
3. Open the pull request with `gh pr create --attach <artifacts folder>/<name>`, or add to an open one with `gh pr edit <number> --attach <artifacts folder>/<name>`. Repeat `--attach` for each file. gh uploads each file and puts its URL in place of the path.
4. Check the body with `gh pr view <number> --json body`. Every image should point at a github.com URL, not a Box path.

Limits, which are GitHub's:

- File types: png, jpg, jpeg, gif, webp, svg, mp4, mov and webm.
- Images up to 10 MB. Videos up to 100 MB on paid plans. At most 50 files.
- An upload cannot be deleted, so attach nothing secret.

When gh is not on the PATH, your facts say how to install it.
