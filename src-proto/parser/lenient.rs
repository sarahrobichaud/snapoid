//! Tolerant scanner for steamcmd's quoted-key tree output
//! (`app_info_print`), which interleaves prompt and banner noise with
//! the VDF-ish structure.

/// Return the value of the leaf whose key path ends with `path`.
///
/// `app_info_print` emits a quoted-key tree whose top levels vary (an
/// appid or "appinfo" wrapper), so callers name the path relative to the
/// nearest unambiguous ancestor. Nesting is tracked with a stack of
/// section keys:
///
/// ```text
/// "appinfo"
/// {
///     "branches"                  <- pushed
///     {
///         "public"                <- pushed
///         {
///             "buildid" "17774562"  <- leaf at [.., "branches", "public", "buildid"]
/// ```
pub fn value_at_path(output: &str, path: &[&str]) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        match line {
            "{" => continue,
            "}" => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let mut tokens = line.split_whitespace();
        let Some(key_token) = tokens.next() else {
            continue;
        };
        if !(key_token.starts_with('"') && key_token.ends_with('"')) {
            continue; // prompt/banner noise between quoted sections
        }
        let key = key_token.trim_matches('"');
        match tokens.next() {
            // Section header; its "{" opens on this or a later line.
            None | Some("{") => stack.push(key),
            // Leaf; the last `path.len()` names from the root must match.
            Some(value) => {
                let mut full: Vec<&str> = stack.clone();
                full.push(key);
                if full.ends_with(path) {
                    return Some(value.trim_matches('"').to_owned());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_INFO: &str = r#""108600"
{
    "appinfo"
    {
        "branches"
        {
            "public"
            {
                "buildid"    "17774562"
            }
        }
        "depots"
        {
            "108604"
            {
                "manifests"
                {
                    "public"
                    {
                        "gid"    "2378094061158458897"
                    }
                }
                "encryptedmanifests"
                {
                    "beta"
                    {
                        "gid"    "encrypted"
                    }
                }
            }
        }
    }
}
"#;

    #[test]
    fn finds_leaves_at_paths() {
        assert_eq!(
            value_at_path(APP_INFO, &["branches", "public", "buildid"]),
            Some("17774562".to_owned())
        );
        assert_eq!(
            value_at_path(APP_INFO, &["manifests", "public", "gid"]),
            Some("2378094061158458897".to_owned())
        );
        // Encrypted branches must not leak into the public path.
        assert_eq!(
            value_at_path(APP_INFO, &["branches", "beta", "buildid"]),
            None
        );
    }

    #[test]
    fn tolerates_noise() {
        let noisy = format!("Redirecting stderr...\n{APP_INFO}Steam> ");
        assert_eq!(
            value_at_path(&noisy, &["branches", "public", "buildid"]),
            Some("17774562".to_owned())
        );
    }
}
