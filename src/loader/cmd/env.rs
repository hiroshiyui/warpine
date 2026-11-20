// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's OS/2 environment: the shell's own variables
// (never the host's), their limits, and the block passed to programs.

/// The most variables and bytes of an environment the shell keeps.
pub(super) const MAX_ENV_VARS: usize = 1024;
pub(super) const MAX_ENV_BYTES: usize = 64 * 1024;

/// `NAME=value` strings as (name, value) pairs; strings without `=` are dropped.
pub(super) fn parse_env<S: AsRef<str>>(strings: &[S]) -> Vec<(String, String)> {
    strings.iter().filter_map(|s| {
        let (k, v) = s.as_ref().split_once('=')?;
        (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
    }).take(MAX_ENV_VARS).collect()
}

/// Why `SET` refused a change.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EnvError { NoName, Full }

/// `SET name=value` on `env`: names are matched without case and kept as
/// first written; an empty value removes the variable. The environment
/// stays within `MAX_ENV_VARS` variables and `MAX_ENV_BYTES` bytes.
pub(super) fn env_set(env: &mut Vec<(String, String)>, name: &str, value: &str) -> Result<(), EnvError> {
    if name.is_empty() || name.contains('\0') { return Err(EnvError::NoName); }
    let at = env.iter().position(|(k, _)| k.eq_ignore_ascii_case(name));
    if value.is_empty() {
        if let Some(i) = at { env.remove(i); }
        return Ok(());
    }
    let size: usize = env.iter().enumerate().filter(|&(i, _)| Some(i) != at)
        .map(|(_, (k, v))| k.len() + v.len() + 2).sum();
    if size + name.len() + value.len() + 2 > MAX_ENV_BYTES || (at.is_none() && env.len() >= MAX_ENV_VARS) {
        return Err(EnvError::Full);
    }
    let value = value.replace('\0', "");
    match at {
        Some(i) => env[i].1 = value,
        None => env.push((name.to_string(), value)),
    }
    Ok(())
}

/// The environment block of `env` for `WARPINE_OS2ENV`: `NAME=value`
/// strings, each with its NUL (the reader adds the block's end), as
/// `DosExecPgm` passes it.
pub(super) fn env_block(env: &[(String, String)], codepage: u32) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = env.iter()
        .map(|(k, v)| crate::loader::codepage::cp_encode(&format!("{k}={v}"), codepage)).collect();
    let mut block = crate::loader::inherit::join_env(&strings);
    block.pop();
    block
}

/// The longest variable name `%NAME%` expands.
const MAX_VAR_NAME: usize = 256;

/// `line` with `%NAME%` replaced by the variable's value (names match
/// without case) and `%%` by `%`. An undefined variable expands to nothing;
/// a `%` that starts no `%NAME%` is kept. In a script (`params` given),
/// `%0`–`%9` are its name and arguments, nothing past the last. Expansion
/// is one pass: a value is not expanded again.
pub(super) fn expand_vars(line: &str, env: &[(String, String)], params: Option<&[String]>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(tail) = after.strip_prefix('%') {
            out.push('%');
            rest = tail;
            continue;
        }
        if let (Some(params), Some(digit)) = (params, after.chars().next().and_then(|c| c.to_digit(10))) {
            if let Some(value) = params.get(digit as usize) { out.push_str(value); }
            rest = &after[1..];
            continue;
        }
        match after.find('%') {
            Some(end) if end > 0 && end <= MAX_VAR_NAME && !after[..end].contains(char::is_whitespace) => {
                let name = &after[..end];
                if let Some((_, value)) = env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                    out.push_str(value);
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_vars_env(line: &str, env: &[(String, String)]) -> String {
        expand_vars(line, env, None)
    }

    /// A script's `%0`–`%9`; at the prompt they stay as they are.
    #[test]
    fn script_parameters_expand() {
        let env = parse_env(&["X=1"]);
        let params: Vec<String> = vec!["RUN.CMD".into(), "first".into(), "\"two words\"".into()];
        assert_eq!(expand_vars("%0 %1 %2 [%3] %X% %%1", &env, Some(&params)), "RUN.CMD first \"two words\" [] 1 %1");
        assert_eq!(expand_vars("echo %1", &env, None), "echo %1", "not in a script");
        assert_eq!(expand_vars("%10", &env, Some(&params)), "first0", "one digit");
    }

    /// `%NAME%` takes the variable's value, `%%` is one `%`, an undefined
    /// variable is nothing, and a value is not expanded again.
    #[test]
    fn variables_expand_once() {
        let env = parse_env(&["Path=C:\\OS2", "PCT=%PATH%", "EMPTY_NOT=x"]);
        assert_eq!(expand_vars_env("echo %path%;D:\\", &env), "echo C:\\OS2;D:\\");
        assert_eq!(expand_vars_env("echo %PCT%", &env), "echo %PATH%", "one pass");
        assert_eq!(expand_vars_env("echo [%NOSUCH%]", &env), "echo []");
        assert_eq!(expand_vars_env("echo 100%% %%PATH%%", &env), "echo 100% %PATH%");
        assert_eq!(expand_vars_env("echo 50% off", &env), "echo 50% off", "a lone % stays");
        assert_eq!(expand_vars_env("echo %a b% %", &env), "echo %a b% %", "names have no blanks");
        assert_eq!(expand_vars_env("%PATH%%PATH%", &env), "C:\\OS2C:\\OS2");
        assert_eq!(expand_vars_env("ünï %PATH% ok", &env), "ünï C:\\OS2 ok");
    }

    #[test]
    fn set_changes_only_the_shells_environment() {
        let mut env = parse_env(&["PATH=C:\\", "Os=OS2", "junk", "=x"]);
        assert_eq!(env, vec![("PATH".into(), "C:\\".into()), ("Os".into(), "OS2".into())], "no name, no variable");
        assert_eq!(env_set(&mut env, "os", "Warp"), Ok(()));
        assert_eq!(env[1], ("Os".into(), "Warp".into()), "names match without case, keep their spelling");
        assert_eq!(env_set(&mut env, "LD_PRELOAD", "./x.so"), Ok(()));
        assert_eq!(env_set(&mut env, "", "x"), Err(EnvError::NoName), "SET =x does not panic");
        assert_eq!(env_set(&mut env, "PATH", ""), Ok(()));
        assert!(!env.iter().any(|(k, _)| k == "PATH"), "an empty value removes it");
        // The host process's environment is never touched.
        assert!(std::env::var_os("LD_PRELOAD").is_none_or(|v| v != "./x.so"));
        assert_eq!(env_block(&env, 437), b"Os=Warp\0LD_PRELOAD=./x.so\0".to_vec());
    }

    #[test]
    fn the_shells_environment_is_bounded() {
        let mut env = Vec::new();
        let big = "x".repeat(MAX_ENV_BYTES);
        assert_eq!(env_set(&mut env, "A", &big), Err(EnvError::Full));
        for i in 0..MAX_ENV_VARS { env_set(&mut env, &format!("V{i}"), "1").unwrap(); }
        assert_eq!(env_set(&mut env, "ONE_MORE", "1"), Err(EnvError::Full));
        assert_eq!(env_set(&mut env, "V0", "2"), Ok(()), "changing one still works");
        assert_eq!(env_set(&mut env, "N\0UL", "1"), Err(EnvError::NoName));
    }
}
