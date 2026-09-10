# csvdiff — Claude Code

Composite-key table comparison — CSV, newline-delimited JSON and Parquet — as byte-level ports in
C, C++, Rust and Zig, held to one result contract. Key columns, compared columns and normalisation
are runtime parameters; nothing about a specific dataset belongs in the code.

**Read `AGENTS.md` first.** It is the canonical guide for AI coding tools, and this file is the
Claude Code layer on top of it: the layout, the invariants, the working and measuring rules and the
gotchas all live there. This file only carries what is specific to working here as Claude Code.

## Claude Code plumbing

| What | Where |
|---|---|
| Slash command: benchmark a scale | `.claude/commands/bench.md` |
| Slash command: compare two files and summarise | `.claude/commands/compare.md` |
| Slash command: fix the latest failing CI run | `.claude/commands/ci-fix.md` |
| Skill: changing the HTML report | `.claude/skills/csvdiff-report/SKILL.md` |
| Permission allow / ask / deny list | `.claude/settings.json` |

Keep these in step with the repository. They recently pointed at the removed Python package
(`pytest`, `python -m csvdiff`, `scripts/bench.py`) and at a source layout that no longer exists;
everything in `.claude/` now names things `AGENTS.md` also names.

## Working here

- Branch, then a pull request to `main`. Do not commit to `main` directly.
- Wait for the automated code review before calling a PR done. Where a finding is legitimate, fix
  it and push to the same branch; where it is wrong or does not apply, say so on the PR and why.
- Read what landed on `main` recently before starting a round — someone else may have already done
  your change in their port. The same finding usually applies to all four ports, and the right
  answer can differ per port.
- Use the slash commands and the report skill rather than improvising: they encode decisions this
  repository made and reversed over time.