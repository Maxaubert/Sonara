# Summarizer instructions

The three built-in instructions of `summaries.style` (`tidy`, `natural`,
`brief`), copied verbatim from the Python plugin's `src/sonara/summarizer.py`
(`INSTRUCTIONS`). `tests/test_agent_prompts.py` keeps them equal until the
Python plugin is removed (M11); change both together.

Each file is the instruction alone, without a trailing newline. The prompt
sent on stdin is the instruction, a blank line, and the message between
`<message>` tags.
