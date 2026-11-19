# For QA

You start a check in plain words, read what the agent found, and report it. You
never need the terminal. Read [Get started](start.md) first.

## Start a check

Choose **+ New task** and say what to check and what to leave behind as
evidence. For example:

> Run the end-to-end tests on the login flow. Save a screenshot of each failing
> page and a short report in .moor/artifacts/.

Pick the repo, and leave **from** blank to test the default branch. Name a
branch there to test a pull request's work instead.

To run the same check every morning, make it a workflow. See
[Workflows for teams](workflows.md).

## Review the evidence

The **Evidence** tab holds what the agent left in `.moor/artifacts/`.
Screenshots show as pictures. Logs, reports and step reports download. Each file
keeps its versions, so a rerun adds a version and keeps the old one.

![The Evidence tab with a screenshot and two files](images/task-evidence.png)

## Read the timeline

The **Timeline** tells the story of the check. Your request is at the top, then
each step the agent took. Routine tool calls fold into one line, and opening it
shows each command. Errors are never folded, so a failing command stands out in
red with its exit code.

![The Timeline of a finished task](images/task-timeline.png)

If the agent asks you something, the task moves to **Needs you** and the card
quotes the question. Pick an answer, or write your own in the box below it.

## Report

- Copy the task's address from the browser. Anyone on the team can open it.
- Download the files you want to attach from Evidence.
- Ask the agent for more in the message box, such as "Retest with the dark
  theme." It runs in the same Box and adds to the same evidence.

When the check is done, close the task from **⋯**. Evidence stays.
