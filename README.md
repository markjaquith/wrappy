# wrappy

`wrappy` adds subcommand aliases to other programs.

e.g. you have a program `foo` with a subcommand `bartholemew`. Ew. That's too long. But `foo` doesn't have any way to register an alias for a subcommand.

With wrappy, you create:

```toml
# ~/.config/wrappy/foo.toml
[aliases]
bar = ["bartholomew"]
```

Now you can run `foo bar`.

> [!NOTE]
> Wrappy currently requires zsh
> If you want to add support for other shells, PRs are welcome!

## Config

Create a config file such as `~/.config/wrappy/yourcommand.toml`:

```toml
[aliases]
rm = ["remove"]
"team ls" = ["team", "list"]
```

With this config...

```zsh
yourcommand rm foo
# executes: yourcommand remove foo

yourcommand team ls --json
# executes: yourcommand team list --json
```

## Installation

1. Install Rust: `curl https://sh.rustup.rs -sSf | sh`
2. Install wrappy: `cargo install wrappy`
3. Add zsh initialization code to `.zshrc`:

```zsh
eval "$(wrappy init zsh)"
```

## How it Works

- Loads per-command aliases from `~/.config/wrappy/<command>.toml`
- Rewrites the longest matching leading subcommand path
- Preserves the rest of argv unchanged
- Replaces itself with the real command via `exec`
- Preserves shell-backed command wrappers by rewriting argv before calling the original shell function
- Generates zsh wrapper functions with `wrappy init zsh`
- Delegates zsh completion to the original command and merges alias suggestions
- Exposes `wrappy complete` in JSON and zsh-friendly formats

## Commands

```text
wrappy exec <cmd> [args...]
wrappy rewrite [--format json|zsh] <cmd> [args...]
wrappy init zsh
wrappy complete [--format json|zsh] [--current N] <cmd> [words...]
```

`WRAPPY_DEBUG=1` prints rewrite and resolution details before `exec`.

## Development

```sh
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```

## License

Distributed under the MIT License. See [LICENSE](LICENSE) for details.
