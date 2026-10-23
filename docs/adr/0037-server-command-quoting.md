# 37. `--server` honors quotes, without escapes

Date: 2026-10-23
Status: Accepted

## Context

Every subcommand that takes `--server` says in its help that the command is
"parsed shell-style". `split_server_command` split on whitespace and kept
quote characters as text. The 2026-10-20 survey
([field-results.md](../field-results.md)) hit this:
`--server "sh -c 'exec npx ...'"` handed `sh` the argument `'exec`, which
failed with "Unterminated quoted string". A server path with a space could
not be given at all, and the test suite had to write Python one-liners with
no spaces in them.

The same string is used on Linux, macOS and Windows, and the Action passes
its `server` input through unchanged on all three runners.

## Decision

`split_server_command` splits on whitespace outside quotes. Single and
double quotes group text, spaces included, into one word; they are removed,
and they may open mid-word (`--name="a b"` gives `--name=a b`). `''` and
`""` are empty arguments. There are no escapes: a backslash is an ordinary
character everywhere. A quote character goes inside the other kind of
quotes. Nothing is expanded (no variables, globs or `~`).

An unterminated quote, or a command that names no program, is an error. The
error message does not repeat the command, so the Action's redacted mode can
keep using it.

This applies wherever the function is used: `deadlock-probe`, `race-check`,
`lost-update`, `doctor`, `replay`, the `serve` tools' `server_command`, and
the server command an `--emit-repro` script starts.

## Alternatives considered

- **POSIX rules (the `shlex` or `shell-words` crates).** A backslash outside
  quotes escapes the next character, so `C:\tools\server.exe` would become
  `C:toolsserver.exe`. Windows users would have to double every backslash,
  and the same Action input would mean different things on different
  runners.
- **Platform rules (POSIX on Unix, `CommandLineToArgvW` on Windows).** Each
  platform would match its own shell, but one workflow file running on a
  matrix of runners would split differently on each.
- **Keep whitespace splitting and fix the help text.** Commands that need a
  space inside an argument would still need a wrapper script.

## Consequences

- A command that contains quotes now loses them. One that relied on literal
  quote characters, such as an unquoted JSON argument
  (`--config {"a":1}`), must wrap that argument in single quotes.
- A backslash cannot escape a space or a quote. Quotes cover both cases.
- The TOML config is unchanged: `command` and `args` were already separate.
