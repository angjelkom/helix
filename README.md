<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

[![Build status](https://github.com/helix-editor/helix/actions/workflows/build.yml/badge.svg)](https://github.com/helix-editor/helix/actions)
[![GitHub Release](https://img.shields.io/github/v/release/helix-editor/helix)](https://github.com/helix-editor/helix/releases/latest)
[![Documentation](https://shields.io/badge/-documentation-452859)](https://docs.helix-editor.com/)
[![GitHub contributors](https://img.shields.io/github/contributors/helix-editor/helix)](https://github.com/helix-editor/helix/graphs/contributors)
[![Matrix Space](https://img.shields.io/matrix/helix-community:matrix.org)](https://matrix.to/#/#helix-community:matrix.org)

</div>

![Screenshot](./screenshot.png)

A [Kakoune](https://github.com/mawww/kakoune) / [Neovim](https://github.com/neovim/neovim) inspired editor, written in Rust.

The editing model is very heavily based on Kakoune; during development I found
myself agreeing with most of Kakoune's design decisions.

For more information, see the [website](https://helix-editor.com) or
[documentation](https://docs.helix-editor.com/).

All shortcuts/keymaps can be found [in the documentation on the website](https://docs.helix-editor.com/keymap.html).

[Troubleshooting](https://github.com/helix-editor/helix/wiki/Troubleshooting)

# Features

- Vim-like modal editing
- Multiple selections
- Built-in language server support
- Smart, incremental syntax highlighting and code editing via tree-sitter

Although it's primarily a terminal-based editor, I am interested in exploring
a custom renderer (similar to Emacs) using wgpu.

Note: Only certain languages have indentation definitions at the moment. Check
`runtime/queries/<lang>/` for `indents.scm`.

# Installation

[Installation documentation](https://docs.helix-editor.com/install.html).

[![Packaging status](https://repology.org/badge/vertical-allrepos/helix-editor.svg?exclude_unsupported=1)](https://repology.org/project/helix-editor/versions)

# This fork

This is a personal fork that adds two things on top of upstream Helix:

- **Steel plugin system.** Merged from the upstream `steel-engine` work. See [`STEEL.md`](./STEEL.md) and [`steel-docs.md`](./steel-docs.md).
- **Claude Code bridge.** Lets [Claude Code](https://claude.com/claude-code) read live editor state and drive the editor through Unix-socket RPC. Design spec: [`docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md`](./docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md). Bridge crate readme: [`helix-claude-mcp/README.md`](./helix-claude-mcp/README.md).

## Building

Build the editor and the MCP bridge binary, both with `target-cpu=native`:

```bash
cargo install \
  --profile opt \
  --config 'build.rustflags="-C target-cpu=native"' \
  --path helix-term \
  --locked

cargo install \
  --profile opt \
  --config 'build.rustflags="-C target-cpu=native"' \
  --path helix-claude-mcp \
  --locked
```

That places `hx` and `helix-claude-mcp` in `~/.cargo/bin`. Restart any running Helix sessions and Claude Code sessions after a rebuild — both cache the spawned process per session.

## Helix configuration

In `~/.config/helix/config.toml`:

```toml
[editor.context-logger]
enabled = true                              # writes <workspace>/.helix/context.json on focus loss
include-selection-text = true               # default — current selection appears in the snapshot
include-buffer-text = false                 # default — flip on for full buffer dumps (large)
max-selection-bytes = 8192                  # default — truncates long selections

[editor.control-socket]
enabled = true                              # binds <workspace>/.helix/control-<pid>.sock for the MCP bridge
```

Both default to `enabled = false`, so opting in is explicit. With both enabled:

- A JSON snapshot of editor state is written to `.helix/context.json` whenever the terminal loses focus (or when you run `:write-context` manually).
- A per-process Unix socket appears at `.helix/control-<pid>.sock`, mode 0600, listening for control RPC from the bridge.

## Claude Code MCP server

Tell Claude Code about the bridge. Either project-scoped (committed to the repo) in `<workspace>/.mcp.json`:

```json
{
  "mcpServers": {
    "helix": {
      "command": "helix-claude-mcp",
      "args": ["serve"]
    }
  }
}
```

…or globally in `~/.claude.json` under the same `"mcpServers"` key. Claude Code spawns the binary per session and sets `CLAUDE_PROJECT_DIR` automatically.

## Claude Code hooks (passive context injection)

The bridge also ships a `hook` subcommand that injects a `<helix-editor-context>` block into each prompt. In `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook", "timeout": 5 }
        ]
      }
    ],
    "PostCompact": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker" }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker" }
        ]
      }
    ]
  }
}
```

The `UserPromptSubmit` hook injects the current snapshot into every prompt (dedup'd per session and per snapshot mtime). The `PostCompact` and `SessionStart` matcher=compact hooks clear the dedup marker so the snapshot re-injects after Claude Code compacts the conversation.

## What Claude can do once it's wired up

Resources (read-only, cheap):

- `helix://state/current` — active buffer, cursor, selection, mode
- `helix://state/buffers` — list of open buffers
- `helix://state/snapshot` — the full snapshot

Tools (live RPC to the editor):

| Tool | What it does |
|---|---|
| `helix_open_file` | Open a file in Helix and focus it. |
| `helix_goto_line` | Move cursor to a 1-indexed line/column. |
| `helix_select` | Select a range from `(start_line, start_column)` to `(end_line, end_column)`; view recenters. |
| `helix_get_diagnostics` | LSP diagnostics for a buffer. |
| `helix_get_hover` | LSP hover at a position. |
| `helix_get_definition` | LSP goto-definition. |
| `helix_get_references` | LSP find-references. |
| `helix_get_workspace_symbols` | LSP workspace symbol search. |
| `helix_format_document` | Format a buffer via its LSP formatter. |
| `helix_run_command` | Execute any Helix typable command. **Powerful** — can `:write`, `:reload`, `:run-shell-command`, etc. See spec §10b. |

Tools refuse cleanly with a structured error when Helix isn't running; resources still serve the last-written snapshot via the file path so passive context survives a closed editor.

# Contributing

Contributing guidelines can be found [here](./docs/CONTRIBUTING.md).

# Getting help

Your question might already be answered on the [FAQ](https://github.com/helix-editor/helix/wiki/FAQ).

Discuss the project on the community [Matrix Space](https://matrix.to/#/#helix-community:matrix.org) (make sure to join `#helix-editor:matrix.org` if you're on a client that doesn't support Matrix Spaces yet).

# Credits

Thanks to [@jakenvac](https://github.com/jakenvac) for designing the logo!
