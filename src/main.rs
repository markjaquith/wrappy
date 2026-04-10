use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command},
};

const HELP: &str = "wrappy

Usage:
  wrappy exec <cmd> [args...]
  wrappy rewrite <cmd> [args...]
  wrappy init zsh
  wrappy complete <cmd> [words...]
  wrappy help
  wrappy --version
";

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    aliases: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AliasRule {
    key: Vec<String>,
    value: Vec<String>,
}

#[derive(Debug, Default)]
struct CommandConfig {
    aliases: Vec<AliasRule>,
}

#[derive(Debug, Serialize)]
struct CompletionOutput {
    rewritten: Vec<String>,
    rewritten_current: Option<usize>,
    aliases: Vec<AliasSuggestion>,
    alias_values: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AliasSuggestion {
    position: usize,
    values: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RewriteOutput {
    args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionFormat {
    Json,
    Zsh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RewriteFormat {
    Json,
    Zsh,
}

#[derive(Debug, Clone, Copy)]
struct CompleteOptions {
    format: CompletionFormat,
    current: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct RewriteOptions {
    format: RewriteFormat,
}

#[derive(Debug)]
struct RewriteMatch<'a> {
    rule: &'a AliasRule,
    consumed: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("wrappy: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let _program_name = args.next();

    let Some(subcommand) = args.next() else {
        print!("{HELP}");
        return Ok(());
    };

    match subcommand.to_string_lossy().as_ref() {
        "exec" => {
            let command_name = args
                .next()
                .ok_or_else(|| "missing wrapped command for `exec`".to_string())?
                .to_string_lossy()
                .into_owned();

            if command_name.is_empty() {
                return Err("wrapped command name cannot be empty".to_string());
            }

            run_exec(&command_name, &args.collect::<Vec<_>>())
        }
        "init" => {
            let shell = args
                .next()
                .ok_or_else(|| "missing shell name for `init`".to_string())?
                .to_string_lossy()
                .into_owned();

            if shell != "zsh" {
                return Err(format!(
                    "unsupported shell `{shell}`; only `zsh` is available in v0"
                ));
            }

            print!("{}", render_zsh_init(&list_configured_commands()?));
            Ok(())
        }
        "rewrite" => {
            let remaining = args.collect::<Vec<_>>();
            let (options, command_name, rewrite_args) = parse_rewrite_args(&remaining)?;
            run_rewrite(&command_name, &rewrite_args, options)
        }
        "complete" => {
            let remaining = args.collect::<Vec<_>>();
            let (options, command_name, words) = parse_complete_args(&remaining)?;
            run_complete(&command_name, &words, options)
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unknown subcommand `{other}`")),
    }
}

fn run_exec(command_name: &str, args: &[OsString]) -> Result<(), String> {
    let config = load_command_config(command_name)?;
    let rewritten_args = config.rewrite_os_strings(args);
    let resolved_command = resolve_real_command(command_name)?;

    if env::var_os("WRAPPY_DEBUG").is_some() {
        debug_rewrite(command_name, &resolved_command, args, &rewritten_args);
    }

    let error = Command::new(&resolved_command)
        .arg0(command_name)
        .args(&rewritten_args)
        .exec();

    Err(format!(
        "failed to exec `{command_name}` via {}: {error}",
        resolved_command.display()
    ))
}

fn run_complete(
    command_name: &str,
    words: &[String],
    options: CompleteOptions,
) -> Result<(), String> {
    let config = load_command_config(command_name)?;
    let has_command_name = words.first().is_some_and(|word| word == command_name);
    let args = if has_command_name { &words[1..] } else { words };
    let relative_current = options.current.map(|current| {
        if has_command_name {
            current.saturating_sub(1)
        } else {
            current
        }
    });

    let mut response = config.completion_output(command_name, args, relative_current);

    if let Some(rewritten_current) = response.rewritten_current {
        response.rewritten_current = Some(rewritten_current + 1);
    }

    match options.format {
        CompletionFormat::Json => {
            let payload = serde_json::to_string(&response)
                .map_err(|error| format!("failed to serialize completion payload: {error}"))?;
            println!("{payload}");
        }
        CompletionFormat::Zsh => {
            print!("{}", render_zsh_completion_reply(&response));
        }
    }

    Ok(())
}

fn run_rewrite(
    command_name: &str,
    args: &[OsString],
    options: RewriteOptions,
) -> Result<(), String> {
    let config = load_command_config(command_name)?;
    let rewritten = config.rewrite_os_strings(args);

    match options.format {
        RewriteFormat::Json => {
            let payload = serde_json::to_string(&RewriteOutput {
                args: rewritten
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            })
            .map_err(|error| format!("failed to serialize rewrite payload: {error}"))?;
            println!("{payload}");
        }
        RewriteFormat::Zsh => {
            print!("{}", render_zsh_rewrite_reply(&rewritten));
        }
    }

    Ok(())
}

fn parse_complete_args(
    args: &[OsString],
) -> Result<(CompleteOptions, String, Vec<String>), String> {
    let mut format = CompletionFormat::Json;
    let mut current = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        let value = arg.to_string_lossy();

        if value == "--" {
            index += 1;
            break;
        }

        if value == "--format" {
            let next = args
                .get(index + 1)
                .ok_or_else(|| "missing value for `wrappy complete --format`".to_string())?;
            format = parse_completion_format(&next.to_string_lossy())?;
            index += 2;
            continue;
        }

        if value == "--current" {
            let next = args
                .get(index + 1)
                .ok_or_else(|| "missing value for `wrappy complete --current`".to_string())?;
            current = Some(parse_completion_current(&next.to_string_lossy())?);
            index += 2;
            continue;
        }

        if value.starts_with('-') {
            return Err(format!("unknown option for `complete`: {value}"));
        }

        break;
    }

    let command_name = args
        .get(index)
        .ok_or_else(|| "missing wrapped command for `complete`".to_string())?
        .to_string_lossy()
        .into_owned();
    let words = args[index + 1..]
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    Ok((CompleteOptions { format, current }, command_name, words))
}

fn parse_rewrite_args(
    args: &[OsString],
) -> Result<(RewriteOptions, String, Vec<OsString>), String> {
    let mut format = RewriteFormat::Json;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        let value = arg.to_string_lossy();

        if value == "--" {
            index += 1;
            break;
        }

        if value == "--format" {
            let next = args
                .get(index + 1)
                .ok_or_else(|| "missing value for `wrappy rewrite --format`".to_string())?;
            format = parse_rewrite_format(&next.to_string_lossy())?;
            index += 2;
            continue;
        }

        if value.starts_with('-') {
            return Err(format!("unknown option for `rewrite`: {value}"));
        }

        break;
    }

    let command_name = args
        .get(index)
        .ok_or_else(|| "missing wrapped command for `rewrite`".to_string())?
        .to_string_lossy()
        .into_owned();
    let rewrite_args = args[index + 1..].to_vec();

    Ok((RewriteOptions { format }, command_name, rewrite_args))
}

fn parse_completion_format(value: &str) -> Result<CompletionFormat, String> {
    match value {
        "json" => Ok(CompletionFormat::Json),
        "zsh" => Ok(CompletionFormat::Zsh),
        _ => Err(format!(
            "unsupported completion format `{value}`; expected `json` or `zsh`"
        )),
    }
}

fn parse_rewrite_format(value: &str) -> Result<RewriteFormat, String> {
    match value {
        "json" => Ok(RewriteFormat::Json),
        "zsh" => Ok(RewriteFormat::Zsh),
        _ => Err(format!(
            "unsupported rewrite format `{value}`; expected `json` or `zsh`"
        )),
    }
}

fn parse_completion_current(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("invalid completion position `{value}`: {error}"))
}

fn load_command_config(command_name: &str) -> Result<CommandConfig, String> {
    let path = config_dir()?.join(format!("{command_name}.toml"));

    if !path.exists() {
        return Ok(CommandConfig::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let config = toml::from_str::<ConfigFile>(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;

    CommandConfig::from_aliases(config.aliases)
}

fn config_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("wrappy"));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        "cannot resolve config directory because neither XDG_CONFIG_HOME nor HOME is set"
            .to_string()
    })?;

    Ok(PathBuf::from(home).join(".config").join("wrappy"))
}

fn list_configured_commands() -> Result<Vec<String>, String> {
    let config_path = config_dir()?;

    if !config_path.exists() {
        return Ok(Vec::new());
    }

    let mut commands = Vec::new();
    let entries = fs::read_dir(&config_path)
        .map_err(|error| format!("failed to read {}: {error}", config_path.display()))?;

    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to inspect {}: {error}", config_path.display()))?;
        let path = entry.path();

        if path.extension().and_then(OsStr::to_str) != Some("toml") {
            continue;
        }

        let Some(command_name) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };

        commands.push(command_name.to_string());
    }

    commands.sort();
    Ok(commands)
}

#[allow(clippy::too_many_lines)]
fn render_zsh_init(commands: &[String]) -> String {
    let mut script = String::from(
        r#"# wrappy init zsh
typeset -gA _wrappy_original_comps
typeset -gA _wrappy_completion_wrapped
typeset -gA _wrappy_original_functions

_wrappy_complete_dispatch() {
  emulate -L zsh
  setopt localoptions noshwordsplit

  local command_name="$1"
  local original_comp="${_wrappy_original_comps[$command_name]-}"
  local output
  output="$(command wrappy complete --format zsh --current "$CURRENT" "$command_name" "${words[@]}")" || return 1

  local -a _wrappy_reply_rewritten _wrappy_reply_alias_values
  local -i _wrappy_reply_rewritten_current=0
  eval "$output"

  local -a saved_words
  saved_words=("${words[@]}")
  local -i saved_current=$CURRENT
  local saved_prefix="$PREFIX"
  local saved_iprefix="$IPREFIX"
  local saved_suffix="$SUFFIX"
  local saved_isuffix="$ISUFFIX"
  local result=1

  if [[ -n "$original_comp" && "$original_comp" != _wrappy_complete_* ]]; then
    words=("${_wrappy_reply_rewritten[@]}")
    CURRENT=$_wrappy_reply_rewritten_current

    while (( CURRENT > ${#words[@]} )); do
      words+=('')
    done

    if (( CURRENT >= 1 && CURRENT <= ${#words[@]} )); then
      PREFIX="${words[CURRENT]}"
    else
      PREFIX=''
    fi

    IPREFIX=''
    SUFFIX=''
    ISUFFIX=''
    "$original_comp"
    result=$?
    words=("${saved_words[@]}")
    CURRENT=$saved_current
    PREFIX="$saved_prefix"
    IPREFIX="$saved_iprefix"
    SUFFIX="$saved_suffix"
    ISUFFIX="$saved_isuffix"
  fi

  if (( ${#_wrappy_reply_alias_values[@]} > 0 )); then
    compadd -Q -U -X 'wrappy aliases' -- "${_wrappy_reply_alias_values[@]}"
    result=0
  fi

  return $result
}

_wrappy_call_original_function() {
  emulate -L zsh
  setopt localoptions noshwordsplit

  local command_name="$1"
  local original_function="$2"
  shift 2

  local output
  output="$(command wrappy rewrite --format zsh "$command_name" "$@")" || return 1

  local -a _wrappy_reply_args
  eval "$output"
  "$original_function" "${_wrappy_reply_args[@]}"
}

_wrappy_install_exec_wrapper() {
  emulate -L zsh
  local command_name="$1"

  eval "function ${command_name}() {
    command wrappy exec ${command_name:q} \"\$@\"
  }"
}

_wrappy_install_function_wrapper() {
  emulate -L zsh
  local command_name="$1"
  local original_function="$2"

  functions -c "$command_name" "$original_function"

  eval "function ${command_name}() {
    _wrappy_call_original_function ${command_name:q} ${original_function:q} \"\$@\"
  }"
}

_wrappy_try_wrap_command() {
  emulate -L zsh
  local command_name="$1"
  local safe_name="$2"
  local original_function="_wrappy_original_${safe_name}"
  local current_body="${functions[$command_name]-}"

  if [[ -n "$current_body" && "$current_body" == *"_wrappy_call_original_function ${command_name}"* ]]; then
    return 0
  fi

  if (( $+functions[$command_name] )); then
    _wrappy_install_function_wrapper "$command_name" "$original_function"
    _wrappy_original_functions[$command_name]="$original_function"
    return 0
  fi

  _wrappy_install_exec_wrapper "$command_name"
  return 0
}

_wrappy_register_completion() {
  emulate -L zsh
  local command_name="$1"
  local completion_name="$2"

  if ! (( $+functions[compdef] && $+_comps )); then
    return 1
  fi

  local existing_completion="${_comps[$command_name]-}"

  if [[ "$existing_completion" != "$completion_name" ]]; then
    _wrappy_original_comps[$command_name]="$existing_completion"
  fi

  compdef "$completion_name" "$command_name"
  _wrappy_completion_wrapped[$command_name]=1
  return 0
}
"#,
    );

    for command_name in commands {
        if !is_safe_zsh_function_name(command_name) {
            let _ = writeln!(
                script,
                "# skipped unsupported command name {}",
                shell_quote(command_name)
            );
            continue;
        }

        let _ = writeln!(
            script,
            "_wrappy_complete_{}() {{\n  _wrappy_complete_dispatch {}\n}}",
            sanitize_zsh_identifier(command_name),
            shell_quote(command_name)
        );
    }

    script.push_str("_wrappy_try_wrap_all() {\n  emulate -L zsh\n");

    for command_name in commands {
        if !is_safe_zsh_function_name(command_name) {
            continue;
        }

        let _ = writeln!(
            script,
            "  _wrappy_try_wrap_command {} {} || return 1",
            shell_quote(command_name),
            shell_quote(&sanitize_zsh_identifier(command_name))
        );
    }

    script.push_str("  return 0\n}\n\n_wrappy_try_register_all() {\n  emulate -L zsh\n");

    for command_name in commands {
        if !is_safe_zsh_function_name(command_name) {
            continue;
        }

        let _ = writeln!(
            script,
            "  _wrappy_register_completion {} _wrappy_complete_{} || return 1",
            shell_quote(command_name),
            sanitize_zsh_identifier(command_name)
        );
    }

    script.push_str(
        r"  return 0
}

_wrappy_activate() {
  emulate -L zsh
  _wrappy_try_wrap_all || return 1
  _wrappy_try_register_all || return 1
  return 0
}

autoload -Uz add-zsh-hook
_wrappy_activate_on_precmd() {
  emulate -L zsh
  _wrappy_activate || return 0
  add-zsh-hook -d precmd _wrappy_activate_on_precmd 2>/dev/null
  unfunction _wrappy_activate_on_precmd 2>/dev/null
}
add-zsh-hook precmd _wrappy_activate_on_precmd

if (( ${+_comps} )); then
  _wrappy_activate || true
fi
",
    );

    script
}

fn sanitize_zsh_identifier(value: &str) -> String {
    let mut output = String::from("wrappy");

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character);
        } else {
            output.push('_');
        }
    }

    output
}

fn is_safe_zsh_function_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn resolve_real_command(command_name: &str) -> Result<PathBuf, String> {
    let path_var = env::var_os("PATH").ok_or_else(|| "PATH is not set".to_string())?;
    let current_executable = env::current_exe()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());

    for directory in env::split_paths(&path_var) {
        let candidate = directory.join(command_name);

        if !is_executable_file(&candidate) {
            continue;
        }

        let canonical_candidate =
            fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());

        if current_executable
            .as_ref()
            .is_some_and(|current| *current == canonical_candidate)
        {
            continue;
        }

        return Ok(candidate);
    }

    Err(format!(
        "could not resolve underlying command `{command_name}` in PATH"
    ))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn debug_rewrite(
    command_name: &str,
    resolved_command: &Path,
    original_args: &[OsString],
    rewritten_args: &[OsString],
) {
    eprintln!("wrappy: command={command_name}");
    eprintln!("wrappy: resolved={}", resolved_command.display());
    eprintln!("wrappy: original={}", format_os_strings(original_args));
    eprintln!("wrappy: rewritten={}", format_os_strings(rewritten_args));
}

fn format_os_strings(values: &[OsString]) -> String {
    values
        .iter()
        .map(|value| shell_quote(&value.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

impl CommandConfig {
    fn from_aliases(aliases: BTreeMap<String, Vec<String>>) -> Result<Self, String> {
        let mut rules = Vec::with_capacity(aliases.len());

        for (key, value) in aliases {
            let key_tokens = split_alias_key(&key)?;

            if value.is_empty() {
                return Err(format!("alias `{key}` must expand to at least one token"));
            }

            rules.push(AliasRule {
                key: key_tokens,
                value,
            });
        }

        rules.sort_by(|left, right| {
            right
                .key
                .len()
                .cmp(&left.key.len())
                .then_with(|| left.key.cmp(&right.key))
        });

        Ok(Self { aliases: rules })
    }

    fn rewrite_os_strings(&self, args: &[OsString]) -> Vec<OsString> {
        let Some(matched) = self.resolve_os_strings(args) else {
            return args.to_vec();
        };

        let mut rewritten = matched
            .rule
            .value
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        rewritten.extend(args[matched.consumed..].iter().cloned());
        rewritten
    }

    #[cfg(test)]
    fn rewrite_strings(&self, args: &[String]) -> Vec<String> {
        let Some(matched) = self.resolve_strings(args) else {
            return args.to_vec();
        };

        let mut rewritten = matched.rule.value.clone();
        rewritten.extend_from_slice(&args[matched.consumed..]);
        rewritten
    }

    fn completion_output(
        &self,
        command_name: &str,
        args: &[String],
        current: Option<usize>,
    ) -> CompletionOutput {
        let matched = self.resolve_strings(args);
        let rewritten_args = matched.as_ref().map_or_else(
            || args.to_vec(),
            |matched| {
                let mut rewritten = matched.rule.value.clone();
                rewritten.extend_from_slice(&args[matched.consumed..]);
                rewritten
            },
        );
        let rewritten_current = current.map(|position| {
            matched.as_ref().map_or(position, |matched| {
                rewrite_completion_position(position, matched.consumed, matched.rule.value.len())
            })
        });
        let aliases = self.alias_suggestions(args);
        let alias_values = current
            .map(|position| self.alias_values_for_position(args, position))
            .unwrap_or_default();
        let mut rewritten = vec![command_name.to_string()];
        rewritten.extend(rewritten_args);

        CompletionOutput {
            rewritten,
            rewritten_current,
            aliases,
            alias_values,
        }
    }

    fn alias_suggestions(&self, args: &[String]) -> Vec<AliasSuggestion> {
        let leading_length = leading_subcommand_len_strings(args);
        let leading_tokens = &args[..leading_length];
        let mut suggestions = BTreeMap::<usize, BTreeSet<String>>::new();

        for rule in &self.aliases {
            let prefix_length = rule.key.len().saturating_sub(1);

            if prefix_length > leading_tokens.len() {
                continue;
            }

            if leading_tokens[..prefix_length] == rule.key[..prefix_length] {
                suggestions
                    .entry(prefix_length + 1)
                    .or_default()
                    .insert(rule.key[prefix_length].clone());
            }
        }

        suggestions
            .into_iter()
            .map(|(position, values)| AliasSuggestion {
                position,
                values: values.into_iter().collect(),
            })
            .collect()
    }

    fn resolve_os_strings<'a>(&'a self, args: &[OsString]) -> Option<RewriteMatch<'a>> {
        let leading_length = leading_subcommand_len_os(args);

        if leading_length == 0 {
            return None;
        }

        let leading_tokens = args[..leading_length]
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        self.match_leading_tokens(&leading_tokens)
    }

    fn resolve_strings<'a>(&'a self, args: &[String]) -> Option<RewriteMatch<'a>> {
        let leading_length = leading_subcommand_len_strings(args);

        if leading_length == 0 {
            return None;
        }

        self.match_leading_tokens(&args[..leading_length])
    }

    fn match_leading_tokens<'a>(&'a self, leading_tokens: &[String]) -> Option<RewriteMatch<'a>> {
        for rule in &self.aliases {
            if leading_tokens.starts_with(&rule.key) {
                return Some(RewriteMatch {
                    rule,
                    consumed: rule.key.len(),
                });
            }
        }

        None
    }

    fn alias_values_for_position(&self, args: &[String], position: usize) -> Vec<String> {
        self.alias_suggestions(args)
            .into_iter()
            .find(|suggestion| suggestion.position == position)
            .map(|suggestion| suggestion.values)
            .unwrap_or_default()
    }
}

fn render_zsh_completion_reply(response: &CompletionOutput) -> String {
    let rewritten_values = response
        .rewritten
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let alias_values = response
        .alias_values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let rewritten_current = response.rewritten_current.unwrap_or(0);

    format!(
        "typeset -ga _wrappy_reply_rewritten=({rewritten_values})\ntypeset -ga _wrappy_reply_alias_values=({alias_values})\ntypeset -gi _wrappy_reply_rewritten_current={rewritten_current}\n"
    )
}

fn render_zsh_rewrite_reply(args: &[OsString]) -> String {
    let values = args
        .iter()
        .map(|value| shell_quote(&value.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");

    format!("typeset -ga _wrappy_reply_args=({values})\n")
}

fn rewrite_completion_position(position: usize, consumed: usize, expanded: usize) -> usize {
    if position <= consumed {
        expanded.max(1)
    } else {
        position - consumed + expanded
    }
}

fn split_alias_key(key: &str) -> Result<Vec<String>, String> {
    let tokens = key
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();

    if tokens.is_empty() {
        return Err("alias keys cannot be empty".to_string());
    }

    Ok(tokens)
}

fn leading_subcommand_len_os(args: &[OsString]) -> usize {
    args.iter()
        .take_while(|value| !is_option_like_os(value))
        .count()
}

fn leading_subcommand_len_strings(args: &[String]) -> usize {
    args.iter()
        .take_while(|value| !is_option_like_string(value))
        .count()
}

fn is_option_like_os(value: &OsStr) -> bool {
    matches!(value.as_bytes().first(), Some(b'-'))
}

fn is_option_like_string(value: &str) -> bool {
    value.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(aliases: &[(&str, &[&str])]) -> CommandConfig {
        let alias_map = aliases
            .iter()
            .map(|(key, value)| {
                (
                    (*key).to_string(),
                    value.iter().map(|token| (*token).to_string()).collect(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        CommandConfig::from_aliases(alias_map).expect("config should parse")
    }

    #[test]
    fn rewrites_simple_aliases() {
        let config = config(&[("rm", &["remove"])]);
        let args = vec!["rm".to_string(), "foo".to_string()];

        assert_eq!(config.rewrite_strings(&args), vec!["remove", "foo"]);
    }

    #[test]
    fn longest_prefix_wins() {
        let config = config(&[("team", &["squad"]), ("team ls", &["team", "list"])]);
        let args = vec!["team".to_string(), "ls".to_string(), "--json".to_string()];

        assert_eq!(
            config.rewrite_strings(&args),
            vec!["team", "list", "--json"]
        );
    }

    #[test]
    fn stops_matching_at_flags() {
        let config = config(&[("rm", &["remove"])]);
        let args = vec!["-C".to_string(), "repo".to_string(), "rm".to_string()];

        assert_eq!(config.rewrite_strings(&args), args);
    }

    #[test]
    fn preserves_double_dash_passthrough() {
        let config = config(&[("rm", &["remove"])]);
        let args = vec![
            "rm".to_string(),
            "--".to_string(),
            "--weird".to_string(),
            "arg".to_string(),
        ];

        assert_eq!(
            config.rewrite_strings(&args),
            vec!["remove", "--", "--weird", "arg"]
        );
    }

    #[test]
    fn suggests_aliases_by_position() {
        let config = config(&[
            ("rm", &["remove"]),
            ("team ls", &["team", "list"]),
            ("team rm", &["team", "remove"]),
        ]);
        let suggestions = config.alias_suggestions(&["team".to_string()]);
        let top_level = suggestions
            .iter()
            .find(|suggestion| suggestion.position == 1)
            .expect("top-level suggestions should exist");
        let nested = suggestions
            .iter()
            .find(|suggestion| suggestion.position == 2)
            .expect("nested suggestions should exist");

        assert_eq!(top_level.values, vec!["rm"]);
        assert_eq!(nested.values, vec!["ls", "rm"]);
    }

    #[test]
    fn renders_zsh_wrappers() {
        let output = render_zsh_init(&["wt".to_string()]);

        assert!(output.contains("_wrappy_complete_wrappywt()"));
        assert!(output.contains("_wrappy_try_wrap_command 'wt' 'wrappywt'"));
        assert!(output.contains("command wrappy rewrite --format zsh"));
        assert!(output.contains("_wrappy_register_completion 'wt' _wrappy_complete_wrappywt"));
    }

    #[test]
    fn completion_output_rewrites_current_and_alias_values() {
        let config = config(&[("cfg", &["config", "get-value"])]);
        let output =
            config.completion_output("wt", &["cfg".to_string(), "foo".to_string()], Some(2));

        assert_eq!(output.rewritten, vec!["wt", "config", "get-value", "foo"]);
        assert_eq!(output.rewritten_current, Some(3));
        assert_eq!(output.alias_values, Vec::<String>::new());
    }

    #[test]
    fn completion_output_returns_alias_values_for_current_position() {
        let config = config(&[
            ("rm", &["remove"]),
            ("team ls", &["team", "list"]),
            ("team rm", &["team", "remove"]),
        ]);
        let output = config.completion_output("wt", &["team".to_string()], Some(2));

        assert_eq!(output.alias_values, vec!["ls", "rm"]);
    }

    #[test]
    fn parse_complete_args_supports_format_and_current() {
        let args = vec![
            OsString::from("--format"),
            OsString::from("zsh"),
            OsString::from("--current"),
            OsString::from("3"),
            OsString::from("wt"),
            OsString::from("wt"),
            OsString::from("rm"),
        ];

        let (options, command_name, words) = parse_complete_args(&args).expect("args should parse");

        assert_eq!(options.format, CompletionFormat::Zsh);
        assert_eq!(options.current, Some(3));
        assert_eq!(command_name, "wt");
        assert_eq!(words, vec!["wt", "rm"]);
    }

    #[test]
    fn parse_rewrite_args_supports_format() {
        let args = vec![
            OsString::from("--format"),
            OsString::from("zsh"),
            OsString::from("wt"),
            OsString::from("ls"),
        ];

        let (options, command_name, rewrite_args) =
            parse_rewrite_args(&args).expect("args should parse");

        assert_eq!(options.format, RewriteFormat::Zsh);
        assert_eq!(command_name, "wt");
        assert_eq!(rewrite_args, vec![OsString::from("ls")]);
    }
}
