//! [`split_server_command`]: turn a `--server` string into a program and its
//! arguments. See ADR 0037 for the quoting rules.

use super::ConfigError;

/// Split a shell-style server command (e.g. `"python -m foo"`) into
/// `(command, args)`.
///
/// Whitespace separates words. Single or double quotes group text, spaces
/// included, into one word and may open mid-word (`--name="a b"` gives
/// `--name=a b`); `''` or `""` is an empty argument. There are no escapes:
/// a backslash is an ordinary character, so Windows paths need no doubling,
/// and a quote character goes inside the other kind of quotes. No variable,
/// glob or `~` expansion happens.
///
/// Returns [`ConfigError::Invalid`] when the input names no program or
/// leaves a quote open. The message never repeats the input, so a caller
/// that must keep the command out of its output can show it as is.
pub fn split_server_command(s: &str) -> Result<(String, Vec<String>), ConfigError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in s.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if let Some(open) = quote {
        return Err(ConfigError::Invalid(format!(
            "server command has an unterminated {open} quote"
        )));
    }
    if in_word {
        words.push(word);
    }
    let mut words = words.into_iter();
    match words.next() {
        Some(command) if !command.is_empty() => Ok((command, words.collect())),
        _ => Err(ConfigError::Invalid("server command is empty".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> (String, Vec<String>) {
        split_server_command(s).expect("splits")
    }

    fn invalid(s: &str) -> String {
        match split_server_command(s) {
            Err(ConfigError::Invalid(message)) => message,
            other => panic!("expected Invalid for {s:?}, got {other:?}"),
        }
    }

    #[test]
    fn splits_on_whitespace() {
        assert_eq!(
            split("  python\t-m my_mcp\n--foo  bar "),
            (
                "python".into(),
                vec!["-m".into(), "my_mcp".into(), "--foo".into(), "bar".into()]
            )
        );
    }

    #[test]
    fn quotes_group_words_and_are_removed() {
        assert_eq!(
            split("sh -c 'exec npx -y pkg \"two words\"'"),
            (
                "sh".into(),
                vec!["-c".into(), "exec npx -y pkg \"two words\"".into()]
            )
        );
        assert_eq!(
            split(r#"node srv.js --config '{"a": 1}' "it's""#),
            (
                "node".into(),
                vec![
                    "srv.js".into(),
                    "--config".into(),
                    r#"{"a": 1}"#.into(),
                    "it's".into()
                ]
            )
        );
    }

    #[test]
    fn quotes_join_the_text_around_them() {
        assert_eq!(
            split(r#"run --name="a b"c 'x'"y""#),
            ("run".into(), vec!["--name=a bc".into(), "xy".into()])
        );
    }

    #[test]
    fn empty_quotes_are_an_empty_argument() {
        assert_eq!(
            split("prog '' \"\""),
            ("prog".into(), vec![String::new(), String::new()])
        );
    }

    #[test]
    fn backslashes_are_kept() {
        assert_eq!(
            split(r#"C:\tools\server.exe "C:\Program Files\data" \\host\share a\ b"#),
            (
                r"C:\tools\server.exe".into(),
                vec![
                    r"C:\Program Files\data".into(),
                    r"\\host\share".into(),
                    r"a\".into(),
                    "b".into()
                ]
            )
        );
    }

    #[test]
    fn rejects_an_unterminated_quote_without_repeating_the_input() {
        let message = invalid("sh -c 'exec npx secret-token");
        assert_eq!(message, "server command has an unterminated ' quote");
        assert_eq!(
            invalid(r#"node "srv.js"#),
            "server command has an unterminated \" quote"
        );
    }

    #[test]
    fn rejects_an_empty_command() {
        for s in ["", "   ", "''", "\"\" --flag"] {
            assert_eq!(invalid(s), "server command is empty", "input {s:?}");
        }
    }
}
