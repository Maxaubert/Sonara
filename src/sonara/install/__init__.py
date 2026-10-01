"""Installing Sonara: the runtime copy, the Python dependencies, stopping and
starting the daemon around file changes, and the Claude Code hooks. cli.py
only parses arguments and calls in here. Kept free of heavy imports: the
Windows supervisor imports claude_hooks from this package."""
