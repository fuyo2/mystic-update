# Mystic Update

![Mystic Update Logo](https://raw.githubusercontent.com/fuyo2/mystic-update/aee9368beed5c6955e85e43e77c8a4c8ce015a7a/resources/icons/hicolor/scalable/apps/mystic-update.svg)

Mystical Software Updater for Linux Distributions.

## Installation

This application is programed in [rust](https://github.com/rust-lang/rust) using the [libcosmic](https://github.com/pop-os/libcosmic) toolkit:

A [justfile](./justfile) is included by default for the [./casey/just][just] command runner.

- `just` builds the application with the default `just build-release` recipe
- `just run` builds and runs the application
- `just install` installs the project into the system
- `just vendor` creates a vendored tarball
- `just build-vendored` compiles with vendored dependencies from that tarball
- `just check` runs clippy on the project to check for linter warnings
- `just check-json` can be used by IDEs that support LSP