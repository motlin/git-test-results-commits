set dotenv-filename := ".envrc"

import ".just/console.just"
import ".just/git.just"
import ".just/git-test.just"

# `just --list --unsorted`
default:
    @just --list --unsorted

# `mise install`
mise:
    mise install --quiet
    mise current

# `cargo +nightly fmt`
fix:
    cargo +nightly fmt

# `cargo +nightly build`
build: fix
	cargo +nightly build

# `cargo +nightly test`
test: fix
	cargo +nightly test

# `cargo +nightly run`
run: fix
	cargo +nightly run

# `cargo build --release`
release: fix
    cargo +nightly build --release

# clean
@clean: _clean-git

# Run all pre-commit checks
precommit:
    pre-commit run --all-files

# Install binary to $XDG_BIN_HOME
install: release
    #!/usr/bin/env bash
    set -Eeuo pipefail

    # Use XDG_BIN_HOME if set; otherwise default to ~/.local/bin
    TARGET_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"

    # Ensure the target directory exists
    mkdir -p "$TARGET_DIR"

    # Define the source binary path
    SOURCE_BINARY="{{justfile_directory()}}/target/release/test-results"

    # Create or update the symlink
    ln -sf "$SOURCE_BINARY" "$TARGET_DIR/"

    echo "Symlink created: $SOURCE_BINARY -> $TARGET_DIR/"

# Override this with a command called `woof` which notifies you in whatever ways you prefer.
# My `woof` command uses `echo`, `say`, and sends a Pushover notification.
echo_command := env('ECHO_COMMAND', "echo")
