# wrappy

`wrappy` is a small CLI wrapper for subcommand aliases.

## Install

Requires Rust and Cargo.

```sh
cargo install wrappy
```

## What v0 does

- Loads per-command aliases from `~/.config/wrappy/<command>.toml`
- Rewrites the longest matching leading subcommand path
- Preserves the rest of argv unchanged
- Replaces itself with the real command via `exec`
- Preserves shell-backed command wrappers by rewriting argv before calling the original shell function
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
cargo publish --dry-run
```

## Release Process

### One-Time Setup

1. Create the `markjaquith/wrappy` GitHub repository.
2. Create a crates.io API token.
3. Add the token to GitHub Actions as `CARGO_REGISTRY_TOKEN`:
   Repo Settings -> Secrets and variables -> Actions -> New repository secret
4. Log in locally once:

```sh
cargo login <token>
```

You can verify the GitHub secret exists with:

```sh
gh secret list --repo markjaquith/wrappy
```

### First Publish

```sh
cargo publish
```

### Subsequent Releases

```sh
# patch, or minor, or major
cargo release patch --no-publish --execute
git push --follow-tags
```

The tag push triggers the GitHub release workflow, which reruns CI and publishes to crates.io with `CARGO_REGISTRY_TOKEN`.

You can watch the release workflow with:

```sh
gh run list --repo markjaquith/wrappy --workflow release.yml
```

If a tagged release fails before publish, fix the issue and push a new tag for the corrected version.
If publish succeeds for a version, that exact version can never be reused on crates.io.

## License

Distributed under the MIT License. See [LICENSE](LICENSE) for details.
