//! Slash-command parsing for the chat input box.
//!
//! Slash commands run in the foreground process and operate on local state
//! (the ignore set, the friends file). The dispatcher in `join.rs` translates
//! the parsed `SlashCommand` into the appropriate action; this module is pure
//! parsing.

/// A parsed slash command. The actual side effects (resolving names, updating
/// the friends file, mutating the ignore set) happen in the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// `/quit`, `/exit` — clean shutdown.
    Quit,
    /// `/help` — list available commands.
    Help,
    /// `/ignore` — print current ignore set.
    IgnoreList,
    /// `/ignore <name>` — add to ignore set. `name` is the unresolved
    /// identifier the user typed; the dispatcher resolves it.
    Ignore(String),
    /// `/unignore <name>` — remove from ignore set.
    Unignore(String),
    /// `/friend` — print current friends list.
    FriendList,
    /// `/friend <name>` — add to friends.
    Friend(String),
    /// `/unfriend <name>` — remove from friends.
    Unfriend(String),
    /// `/foo` — unknown command. Stored verbatim (without leading `/`) so the
    /// dispatcher can print a useful message.
    Unknown(String),
    /// `/` alone or only whitespace after the slash.
    Empty,
}

/// Parse a line of user input as a slash command.
///
/// Returns `None` if `line` does not start with `/`. Otherwise always returns
/// some `SlashCommand` (`Unknown` for unrecognised verbs, `Empty` for a bare
/// `/`).
pub fn parse(line: &str) -> Option<SlashCommand> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('/')?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(SlashCommand::Empty);
    }

    // Split on first whitespace run into verb + argument.
    let (verb, arg) = match rest.split_once(char::is_whitespace) {
        Some((v, a)) => (v, a.trim()),
        None => (rest, ""),
    };

    let cmd = match verb {
        "quit" | "exit" => SlashCommand::Quit,
        "help" | "?" => SlashCommand::Help,
        "ignore" => {
            if arg.is_empty() {
                SlashCommand::IgnoreList
            } else {
                SlashCommand::Ignore(arg.to_string())
            }
        }
        "unignore" => {
            if arg.is_empty() {
                SlashCommand::Unknown("unignore: missing argument".to_string())
            } else {
                SlashCommand::Unignore(arg.to_string())
            }
        }
        "friend" => {
            if arg.is_empty() {
                SlashCommand::FriendList
            } else {
                SlashCommand::Friend(arg.to_string())
            }
        }
        "unfriend" => {
            if arg.is_empty() {
                SlashCommand::Unknown("unfriend: missing argument".to_string())
            } else {
                SlashCommand::Unfriend(arg.to_string())
            }
        }
        other => SlashCommand::Unknown(other.to_string()),
    };
    Some(cmd)
}

/// One-line help text listing every command.
pub fn help_text() -> &'static str {
    "available commands: /help, /quit (alias /exit), /ignore [name], /unignore <name>, /friend [name], /unfriend <name>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_slash_returns_none() {
        assert_eq!(parse("hello world"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("  hello /quit"), None);
    }

    #[test]
    fn quit_aliases() {
        assert_eq!(parse("/quit"), Some(SlashCommand::Quit));
        assert_eq!(parse("/exit"), Some(SlashCommand::Quit));
        assert_eq!(parse("  /quit  "), Some(SlashCommand::Quit));
    }

    #[test]
    fn help() {
        assert_eq!(parse("/help"), Some(SlashCommand::Help));
        assert_eq!(parse("/?"), Some(SlashCommand::Help));
    }

    #[test]
    fn ignore_with_and_without_arg() {
        assert_eq!(parse("/ignore"), Some(SlashCommand::IgnoreList));
        assert_eq!(parse("/ignore alice"), Some(SlashCommand::Ignore("alice".to_string())));
        assert_eq!(parse("/ignore  alice  "), Some(SlashCommand::Ignore("alice".to_string())));
    }

    #[test]
    fn unignore_requires_arg() {
        assert!(matches!(parse("/unignore"), Some(SlashCommand::Unknown(_))));
        assert_eq!(
            parse("/unignore bob"),
            Some(SlashCommand::Unignore("bob".to_string()))
        );
    }

    #[test]
    fn friend_with_and_without_arg() {
        assert_eq!(parse("/friend"), Some(SlashCommand::FriendList));
        assert_eq!(
            parse("/friend alice"),
            Some(SlashCommand::Friend("alice".to_string()))
        );
    }

    #[test]
    fn unfriend_requires_arg() {
        assert!(matches!(parse("/unfriend"), Some(SlashCommand::Unknown(_))));
        assert_eq!(
            parse("/unfriend alice"),
            Some(SlashCommand::Unfriend("alice".to_string()))
        );
    }

    #[test]
    fn unknown_verb() {
        assert_eq!(
            parse("/foo bar"),
            Some(SlashCommand::Unknown("foo".to_string()))
        );
    }

    #[test]
    fn bare_slash() {
        assert_eq!(parse("/"), Some(SlashCommand::Empty));
        assert_eq!(parse("/   "), Some(SlashCommand::Empty));
    }
}
