# Reading a prompt

moor reads a task prompt before the task starts. It shows the prompt back with marks on the words and notes in the margin. A person answers moor's questions, then starts the task. The task is held to the reading they saw.

The same reader reads a repo's note and an automation's prompt. This page settles the data shape, where the reading runs, how answers and edits work, and the order of pull requests. It is the design behind "Read it" in the signed-off portal mock v5. The [position spec](https://github.com/IniZio/moor/blob/db4373fc99339012e785ccd63fd021989c518f2b/docs/specs/position.md) on [#1592](https://github.com/IniZio/moor/pull/1592) proposes the model. This page is the build plan for its reading. The [experience spec](end-user-experience.md) and the [runs spec](runs.md) are unchanged by it, except where open question 4 retires Plan first.

## Decisions

The owner approved this spec with these answers on 2026-11-23.

1. **Default gates for a one-off that names no proof.** None until the definition's defaults exist. The reading says so in an assumed note.
2. **Who pays for a reading.** The person's own token, as for any Box they start. A reading costs one short agent turn.
3. **A check command the reader writes.** Allowed, and each one shows before Start. The reader prefers a named check and asks when unsure.
4. **The Plan first checkbox.** Retired with pull request 7. "Show me a plan first" reads as a gate with plan.md and your yes.
5. **YAML input on `set_repo_config`.** Both are taken until every stored repo has a note, then words only.
6. **Answers moor cannot hold yet.** The reader offers only answers moor can enforce, and its instructions list them. "Wait for my yes before you open it" waits for a pull request capability.
7. **A warm reading Box.** None at first. Add one per person if pull request 4 measures over 45 seconds at the median.
8. **Readings for tasks started over MCP or Slack.** No reading. `start_box` keeps taking gates directly.
9. **Where a reading of an automation sits beside the position spec's definition files.** On the workflow revision for now. It moves with the definition when that ships.

## Gap inventory coverage

| Rows | Where |
| --- | --- |
| A1 Read it | The reading page, pull requests 4 and 5 |
| A2 marked prompt, A3 margins | The reading, mark styles, pull request 5 |
| A4 note kinds | Note kinds. "May" waits for capabilities and reads dashed until then. |
| A5 questions, Start waits | Answers and edits, pull request 6 |
| A6 reading across repos | The reader gets each repo's checks. "Which tests?" and "Add it or leave it" are questions, pull requests 6 and 8. |
| A7 edit in place | Pull request 8 |
| A8 whole prompt on the task, E8 prompt phrase | Pull request 9 |
| A9 frozen plan, one-off gates | Start gives a one-off task its gates, pull request 7 |
| A10 automation edit, B7 Repeat this | Automations, pull request 13 |
| A11 repo note page, A16 setup and kept, A17 YAML view, A18 asked | The repo note, pull requests 10 and 11 |
| A12 draft, A13 repo list | Pull request 12 |
| A14 named checks | Named checks, pull request 2 |
| A15 network line | Shown dashed. Enforcement is the egress work, out of scope. |
| B9 Plan first | Open question 4 |
| C8 inbox clear | Pull request 1 |
| D2 to D5 yes, stuck and no-check cards | Out of scope. One-off tasks get these cards once pull request 7 gives them gates. |
| G1 keys on margin notes | Pull request 6 |
