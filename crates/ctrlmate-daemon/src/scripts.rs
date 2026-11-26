//! Validation of the custom-script allowlist. Editing is possible over authenticated IPC only
//! (never over Home Assistant). Scripts are *executed* by the agent in the user session; the
//! executable is checked for existence there, at execution time.

use ctrlmate_protocol::AllowlistedScript;
use std::collections::HashSet;
use std::path::Path;

pub const MAX_SCRIPTS: usize = 32;
pub const MAX_ARGS: usize = 32;
pub const MAX_ARG_LEN: usize = 1024;
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_PATH_LEN: usize = 1024;
pub const MAX_ID_LEN: usize = 32;

/// `[a-z0-9_-]{1,32}`: becomes part of the entity id `script_<id>`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn absolute(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_LEN
        && !path.contains('\0')
        && Path::new(path).is_absolute()
}

/// Validates a whole allowlist. The error is `<field path>: <reason>` for diagnostics.
pub fn validate_scripts(scripts: &[AllowlistedScript]) -> Result<(), String> {
    if scripts.len() > MAX_SCRIPTS {
        return Err(format!("scripts: more than {MAX_SCRIPTS} entries"));
    }
    let mut seen = HashSet::new();
    for (i, s) in scripts.iter().enumerate() {
        if !valid_id(&s.id) {
            return Err(format!(
                "scripts[{i}].id: must match [a-z0-9_-]{{1,{MAX_ID_LEN}}}"
            ));
        }
        if !seen.insert(s.id.as_str()) {
            return Err(format!("scripts[{i}].id: duplicate"));
        }
        let name = s.name.trim();
        if name.is_empty()
            || s.name.chars().count() > MAX_NAME_LEN
            || s.name.chars().any(char::is_control)
        {
            return Err(format!("scripts[{i}].name: invalid"));
        }
        if !absolute(&s.executable) {
            return Err(format!("scripts[{i}].executable: must be an absolute path"));
        }
        if s.args.len() > MAX_ARGS {
            return Err(format!("scripts[{i}].args: more than {MAX_ARGS} entries"));
        }
        if s.args
            .iter()
            .any(|a| a.len() > MAX_ARG_LEN || a.contains('\0'))
        {
            return Err(format!(
                "scripts[{i}].args: argument too long or contains NUL"
            ));
        }
        if let Some(dir) = &s.working_dir
            && !absolute(dir)
        {
            return Err(format!(
                "scripts[{i}].working_dir: must be an absolute path"
            ));
        }
    }
    Ok(())
}

/// Allowlist pushed to the agent: enabled scripts only.
pub fn enabled_subset(scripts: &[AllowlistedScript]) -> Vec<AllowlistedScript> {
    scripts.iter().filter(|s| s.enabled).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\{p}")
        } else {
            format!("/{p}")
        }
    }

    fn script(id: &str) -> AllowlistedScript {
        AllowlistedScript {
            id: id.into(),
            name: "Backup".into(),
            executable: abs("tools/backup.exe"),
            args: vec!["--fast".into()],
            working_dir: None,
            enabled: true,
        }
    }

    #[test]
    fn accepts_a_wellformed_allowlist() {
        assert!(validate_scripts(&[script("backup"), script("b_2-x")]).is_ok());
        assert!(validate_scripts(&[]).is_ok());
    }

    #[test]
    fn rejects_bad_ids() {
        for id in ["", "Upper", "has space", "a/b", "ünï", &"x".repeat(33)] {
            assert!(validate_scripts(&[script(id)]).is_err(), "{id:?}");
        }
        assert!(validate_scripts(&[script("a"), script("a")]).is_err());
    }

    #[test]
    fn rejects_relative_or_empty_executables() {
        let mut s = script("a");
        s.executable = "backup.exe".into();
        assert!(validate_scripts(&[s.clone()]).is_err());
        s.executable = String::new();
        assert!(validate_scripts(&[s.clone()]).is_err());
        s.executable = "..\\evil.exe".into();
        assert!(validate_scripts(&[s]).is_err());
    }

    #[test]
    fn rejects_bad_args_and_workdir() {
        let mut s = script("a");
        s.args = vec!["x".repeat(MAX_ARG_LEN + 1)];
        assert!(validate_scripts(&[s.clone()]).is_err());
        s.args = vec!["a\0b".into()];
        assert!(validate_scripts(&[s.clone()]).is_err());
        s.args = vec![];
        s.working_dir = Some("relative".into());
        assert!(validate_scripts(&[s.clone()]).is_err());
        s.working_dir = Some(abs("work"));
        assert!(validate_scripts(&[s]).is_ok());
        let many: Vec<_> = (0..=MAX_SCRIPTS)
            .map(|i| script(&format!("s{i}")))
            .collect();
        assert!(validate_scripts(&many).is_err());
    }

    #[test]
    fn enabled_subset_filters() {
        let mut off = script("off");
        off.enabled = false;
        let v = enabled_subset(&[script("on"), off]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "on");
    }
}
