//! Which design to elaborate: slang-server's `server.json` files, read by its rules
//! (`docs/start/config.md` in the fork) so svtrace sees the design slang-server does.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// The part of the merged configuration that decides what slang is run on.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// The workspace folder, where slang runs.
    pub root: PathBuf,
    /// Variables for `$VAR` in the flags, the build file's contents and the paths below.
    pub env: BTreeMap<String, String>,
    /// The `flags`, expanded and split into arguments.
    pub flags: Vec<String>,
    pub build: PathBuf,
    /// Whether paths in the build file are relative to that file (`-F`) or to the workspace (`-f`).
    pub build_relative_paths: bool,
    pub incdirs: Vec<PathBuf>,
}

/// Names a workspace's `env` can't give to the slang process, because they change how a process is
/// loaded or run (a cloned repository's `server.json` could load its own library into slang with
/// them): the dynamic loader's (`DYLD_*`, `LD_*`), the search path and home, and the like.
fn changes_how_a_process_runs(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["DYLD_", "LD_", "MALLOC", "LC_"].iter().any(|prefix| name.starts_with(prefix))
        || ["PATH", "HOME", "TMPDIR", "SHELL", "IFS", "LANG", "LIBPATH", "GCONV_PATH", "LOCPATH", "NLSPATH", "HOSTALIASES", "RES_OPTIONS", "BASH_ENV", "ENV"]
            .contains(&name.as_str())
        || name.starts_with("__")
}

impl Config {
    /// The variables set on the slang process. slang-server uses `env` only to expand `${VAR}` in
    /// what it reads; svtrace expands the flags and paths itself, and slang expands `$VAR` in the
    /// build file's contents from its own environment, so the variables are passed on, except
    /// those that change how a process loads or runs.
    pub fn child_env(&self) -> impl Iterator<Item = (&String, &String)> {
        self.env.iter().filter(|(name, _)| {
            let refused = changes_how_a_process_runs(name);
            if refused {
                log::warn!("not setting {name} from server.json's env on slang: it changes how a process runs");
            }
            !refused
        })
    }
}

/// Reads `<root>/.slang/server.json`, `<home>/.slang/server.json` and
/// `<root>/.slang/local/server.json`, in that order. Missing files are skipped.
pub fn load(root: &Path, home: Option<&Path>) -> Result<Config> {
    load_with_build(root, home, None)
}

/// [`load`], with `build` (a path, relative to `root`) in place of the configured `build`: what
/// `svtrace.useBuild` chose. Everything else in the three files still applies.
pub fn load_with_build(root: &Path, home: Option<&Path>, build: Option<&str>) -> Result<Config> {
    let workspace = read(&root.join(".slang/server.json"))?;
    let user = match home {
        Some(home) => read(&home.join(".slang/server.json"))?,
        None => None,
    };
    let mut local = read(&root.join(".slang/local/server.json"))?;
    if let Some(build) = build {
        // (A layer of its own after the others: the last `build` wins.)
        let mut layer = local.take().unwrap_or_else(|| Value::Object(Default::default()));
        layer["build"] = Value::String(build.to_string());
        local = Some(layer);
    }
    merge(root, workspace.as_ref(), user.as_ref(), local.as_ref())
}

fn read(path: &Path) -> Result<Option<Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let value: Value = serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    if !value.is_object() {
        bail!("{} is not a JSON object", path.display());
    }
    Ok(Some(value))
}

/// Applies the server's rules: `env` merges by name, lists are appended, other values are
/// overwritten by later files, and `flags` take the workspace's (else the user's) as the base with
/// the local ones appended.
fn merge(root: &Path, workspace: Option<&Value>, user: Option<&Value>, local: Option<&Value>) -> Result<Config> {
    let layers = [workspace, user, local].into_iter().flatten().collect::<Vec<_>>();

    let mut env = BTreeMap::new();
    for layer in &layers {
        if let Some(Value::Object(vars)) = layer.get("env") {
            for (name, value) in vars {
                env.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
            }
        }
    }

    let flags_of = |layer: Option<&Value>| -> String {
        layer.and_then(|layer| layer.get("flags")).and_then(Value::as_str).unwrap_or_default().to_string()
    };
    let (workspace_flags, user_flags, local_flags) = (flags_of(workspace), flags_of(user), flags_of(local));
    let base = if workspace_flags.is_empty() { user_flags } else { workspace_flags };
    let flags = [base, local_flags].into_iter().filter(|flags| !flags.is_empty()).collect::<Vec<_>>().join(" ");

    let last = |key: &str| layers.iter().rev().find_map(|layer| layer.get(key));
    let build = last("build")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("no build configured: set \"build\" in .slang/server.json"))?;
    let build = resolve(root, &expand(build, &env)?);
    let build_relative_paths = last("buildRelativePaths").and_then(Value::as_bool).unwrap_or(false);

    let mut incdirs = Vec::new();
    for layer in &layers {
        if let Some(Value::Array(dirs)) = layer.get("incdirs") {
            for dir in dirs.iter().filter_map(Value::as_str) {
                incdirs.push(resolve(root, &expand(dir, &env)?));
            }
        }
    }

    Ok(Config {
        root: root.to_path_buf(),
        flags: split_flags(&flags, &env)?,
        env,
        build,
        build_relative_paths,
        incdirs,
    })
}

fn resolve(root: &Path, path: &str) -> PathBuf {
    root.join(path)
}

/// Replaces `$VAR`, `${VAR}` and `$(VAR)` with the variable's value: the configured `env` first,
/// then the process's. An unset or empty variable is an error.
pub fn expand(text: &str, env: &BTreeMap<String, String>) -> Result<String> {
    let mut result = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        result.push_str(&rest[..at]);
        rest = &rest[at..];
        result.push_str(&variable(&mut rest, env)?);
    }
    result.push_str(rest);
    Ok(result)
}

/// Takes a reference at the start of `rest` (which begins with `$`) and returns its value.
fn variable(rest: &mut &str, env: &BTreeMap<String, String>) -> Result<String> {
    let text = *rest;
    let (name, length) = match text.as_bytes().get(1) {
        Some(b'{') | Some(b'(') => {
            let close = if text.as_bytes()[1] == b'{' { '}' } else { ')' };
            match text.find(close) {
                Some(end) => (&text[2..end], end + 1),
                None => bail!("unterminated environment variable reference in {text:?}"),
            }
        }
        _ => {
            let end = text[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(text.len(), |n| n + 1);
            (&text[1..end], end)
        }
    };
    let reference = &text[..length];
    *rest = &text[length..];
    if name.is_empty() {
        return Ok(reference.to_string());
    }
    let value = env.get(name).cloned().or_else(|| std::env::var(name).ok()).unwrap_or_default();
    if value.is_empty() {
        bail!("environment variable {reference} is unset or empty");
    }
    Ok(value)
}

/// Splits a flags string into arguments: spaces separate, quotes group (single quotes keep
/// everything literal), a backslash makes the next character literal, and variables are expanded
/// everywhere but in single quotes.
fn split_flags(flags: &str, env: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut rest = flags;
    while let Some(c) = rest.chars().next() {
        let width = c.len_utf8();
        match (c, quote) {
            ('\'', None) | ('\'', Some('\'')) => quote = if quote.is_some() { None } else { Some('\'') },
            ('"', None) => quote = Some('"'),
            ('"', Some('"')) => quote = None,
            ('\\', q) if q != Some('\'') => {
                let next = rest[width..].chars().next();
                current.get_or_insert_with(String::new).push(next.unwrap_or('\\'));
                rest = &rest[width + next.map_or(0, char::len_utf8)..];
                continue;
            }
            ('$', q) if q != Some('\'') => {
                let value = variable(&mut rest, env)?;
                current.get_or_insert_with(String::new).push_str(&value);
                continue;
            }
            (c, None) if c.is_whitespace() => arguments.extend(current.take()),
            (c, _) => current.get_or_insert_with(String::new).push(c),
        }
        if quote.is_some() || c == '\'' || c == '"' {
            current.get_or_insert_with(String::new);
        }
        rest = &rest[width..];
    }
    if quote.is_some() {
        bail!("unterminated quote in the flags: {flags:?}");
    }
    arguments.extend(current);
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn variables_expand_in_all_three_forms() {
        let env = env(&[("A", "x"), ("B", "y")]);
        assert_eq!(expand("$A/${B}/$(A)z", &env).unwrap(), "x/y/xz");
        assert_eq!(expand("no variables", &env).unwrap(), "no variables");
        assert_eq!(expand("cost: $", &env).unwrap(), "cost: $");
    }

    #[test]
    fn unset_or_empty_variables_are_errors() {
        let error = expand("${SVTRACE_SURELY_UNSET}", &env(&[])).unwrap_err().to_string();
        assert_eq!(error, "environment variable ${SVTRACE_SURELY_UNSET} is unset or empty");
        assert!(expand("$E", &env(&[("E", "")])).is_err());
    }

    #[test]
    fn flags_split_like_a_command_line() {
        let env = env(&[("DIR", "my dir")]);
        let flags = split_flags(r#"-DA=1 -f "$DIR/x.f" '$KEEP' a\ b -I ''"#, &env).unwrap();
        assert_eq!(flags, ["-DA=1", "-f", "my dir/x.f", "$KEEP", "a b", "-I", ""]);
        assert!(split_flags("'open", &env).is_err());
    }

    #[test]
    fn layers_merge_by_the_servers_rules() {
        let workspace = json!({ "build": "w.f", "flags": "-W1", "env": { "A": "1", "B": "1" }, "incdirs": ["inc"] });
        let user = json!({ "build": "u.f", "flags": "-U", "env": { "B": "2" }, "incdirs": ["uinc"] });
        let local = json!({ "build": "$B.f", "flags": "-L", "env": { "B": "3" }, "buildRelativePaths": true });
        let config = merge(Path::new("/w"), Some(&workspace), Some(&user), Some(&local)).unwrap();
        assert_eq!(config.flags, ["-W1", "-L"]);
        assert_eq!(config.build, Path::new("/w/3.f"));
        assert!(config.build_relative_paths);
        assert_eq!(config.env, env(&[("A", "1"), ("B", "3")]));
        assert_eq!(config.incdirs, [Path::new("/w/inc"), Path::new("/w/uinc")]);

        // Without workspace flags the user's are the base.
        let config = merge(Path::new("/w"), Some(&json!({ "build": "b.f" })), Some(&user), Some(&local)).unwrap();
        assert_eq!(config.flags, ["-U", "-L"]);
    }

    #[test]
    fn variables_that_change_how_slang_runs_are_not_passed_on() {
        let config = Config {
            env: env(&[("DYLD_INSERT_LIBRARIES", "./x.dylib"), ("LD_PRELOAD", "x.so"), ("PATH", "/tmp/evil"), ("HOME", "/x"), ("UVM_HOME", "/tools/uvm")]),
            ..Config::default()
        };
        assert_eq!(config.child_env().collect::<Vec<_>>(), [(&"UVM_HOME".to_string(), &"/tools/uvm".to_string())]);
    }

    #[test]
    fn a_missing_build_is_reported() {
        let error = merge(Path::new("/w"), Some(&json!({})), None, None).unwrap_err();
        assert!(error.to_string().starts_with("no build configured"));
    }
}
