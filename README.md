# wrappy

`wrappy` is a small CLI wrapper for subcommand aliases.

## What v0 does

- Loads per-command aliases from `~/.config/wrappy/<command>.toml`
- Rewrites the longest matching leading subcommand path
- Preserves the rest of argv unchanged
- Replaces itself with the real command via `exec`
- Generates zsh wrapper functions with `wrappy init zsh`
- Delegates zsh completion to the original command and merges alias suggestions
- Exposes `wrappy complete` in JSON and zsh-friendly formats

## Config

Create a config file such as `~/.config/wrappy/wt.toml`:

```toml
[aliases]
rm = ["remove"]
"team ls" = ["team", "list"]
```

## Usage

Build and install however you prefer, then enable wrapping in zsh:

```zsh
eval "$(wrappy init zsh)"
```

If `compinit` is already loaded, `wrappy` registers completion immediately.
If not, it defers registration until the first prompt after completion is available.

With the config above:

```zsh
wt rm foo
# executes: wt remove foo

wt team ls --json
# executes: wt team list --json
```

## Commands

```text
wrappy exec <cmd> [args...]
wrappy init zsh
wrappy complete [--format json|zsh] [--current N] <cmd> [words...]
```

`WRAPPY_DEBUG=1` prints rewrite and resolution details before `exec`.
