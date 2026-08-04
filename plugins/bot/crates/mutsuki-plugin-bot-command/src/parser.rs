use std::collections::{BTreeMap, BTreeSet};

use mutsuki_bot_protocol::{
    BotCommandArgumentDescriptor, BotCommandArgumentKind, BotCommandArgumentValue,
    BotCommandDescriptor, BotCommandHelpEntry, BotCommandHelpResult, BotCommandParseErrorCode,
    BotCommandParseFailure,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub struct CommandParser {
    prefixes: Vec<String>,
    commands: Vec<BotCommandDescriptor>,
    case_sensitive: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCommand {
    pub name: String,
    pub command_path: Vec<String>,
    pub args: Vec<String>,
    pub typed_args: BTreeMap<String, BotCommandArgumentValue>,
    pub raw_text: String,
}

pub fn validate_command_descriptors(commands: &[BotCommandDescriptor]) -> Result<(), String> {
    let mut paths = BTreeSet::new();
    for command in commands {
        if command.path.is_empty() || command.path.iter().any(|part| part.trim().is_empty()) {
            return Err("command path must contain non-empty parts".into());
        }
        for path in std::iter::once(&command.path).chain(command.aliases.iter()) {
            if path.is_empty() || path.iter().any(|part| part.trim().is_empty()) {
                return Err("command aliases must contain non-empty parts".into());
            }
            let normalized = path
                .iter()
                .map(|part| part.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("\u{0}");
            if !paths.insert(normalized) {
                return Err(format!(
                    "duplicate command path or alias: {}",
                    path.join(" ")
                ));
            }
        }
        let mut names = BTreeSet::new();
        let mut optional_seen = false;
        for (index, argument) in command.arguments.iter().enumerate() {
            if argument.name.trim().is_empty() || !names.insert(argument.name.clone()) {
                return Err(format!(
                    "command {} has an empty or duplicate argument name",
                    command.path.join(" ")
                ));
            }
            if argument.variadic && index + 1 != command.arguments.len() {
                return Err(format!("variadic argument {} must be last", argument.name));
            }
            let optional = argument.optional || argument.default.is_some();
            if optional_seen && !optional {
                return Err(format!(
                    "required argument {} cannot follow an optional argument",
                    argument.name
                ));
            }
            optional_seen |= optional;
            if let Some(default) = &argument.default
                && !argument_default_matches(argument, default)
            {
                return Err(format!(
                    "default value does not match argument {}",
                    argument.name
                ));
            }
        }
    }
    Ok(())
}

fn argument_default_matches(
    descriptor: &BotCommandArgumentDescriptor,
    value: &BotCommandArgumentValue,
) -> bool {
    if descriptor.variadic {
        return matches!(value, BotCommandArgumentValue::Strings(_));
    }
    matches!(
        (descriptor.kind, value),
        (
            BotCommandArgumentKind::String,
            BotCommandArgumentValue::String(_)
        ) | (
            BotCommandArgumentKind::Integer,
            BotCommandArgumentValue::Integer(_)
        ) | (
            BotCommandArgumentKind::Number,
            BotCommandArgumentValue::Number(_)
        ) | (
            BotCommandArgumentKind::Boolean,
            BotCommandArgumentValue::Boolean(_)
        )
    )
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("message does not start with a command prefix")]
    MissingPrefix,
    #[error("command name is empty")]
    EmptyName,
    #[error("quoted argument is not terminated")]
    UnterminatedQuote,
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("missing argument: {0}")]
    MissingArgument(String),
    #[error("invalid value for argument {name}: {value}")]
    InvalidArgument { name: String, value: String },
    #[error("unexpected argument: {0}")]
    UnexpectedArgument(String),
}

impl CommandParser {
    pub fn new(prefixes: Vec<String>) -> Self {
        Self {
            prefixes,
            commands: Vec::new(),
            case_sensitive: false,
        }
    }

    pub fn commands(mut self, commands: Vec<BotCommandDescriptor>) -> Self {
        self.commands = commands;
        self
    }

    pub fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub fn parse(&self, text: &str) -> Result<ParsedCommand, CommandParseError> {
        let trimmed = text.trim();
        let prefix = self
            .prefixes
            .iter()
            .filter(|prefix| trimmed.starts_with(prefix.as_str()))
            .max_by_key(|prefix| prefix.len())
            .ok_or(CommandParseError::MissingPrefix)?;
        let command_text = trimmed[prefix.len()..].trim();
        if command_text.is_empty() {
            return Err(CommandParseError::EmptyName);
        }
        let tokens = tokenize(command_text)?;
        if tokens.first().is_none_or(|token| token.is_empty()) {
            return Err(CommandParseError::EmptyName);
        }
        if self.commands.is_empty() {
            return Ok(ParsedCommand {
                name: normalize_part(&tokens[0], self.case_sensitive),
                command_path: vec![normalize_part(&tokens[0], self.case_sensitive)],
                args: tokens.iter().skip(1).cloned().collect(),
                typed_args: BTreeMap::new(),
                raw_text: text.into(),
            });
        }
        let (descriptor, path_len) = self
            .commands
            .iter()
            .filter_map(|descriptor| {
                matching_path_len(descriptor, &tokens, self.case_sensitive)
                    .map(|len| (descriptor, len))
            })
            .max_by_key(|(_, len)| *len)
            .ok_or_else(|| CommandParseError::UnknownCommand(tokens[0].clone()))?;
        let raw_args = tokens.iter().skip(path_len).cloned().collect::<Vec<_>>();
        let typed_args = parse_arguments(&descriptor.arguments, &raw_args)?;
        let path = normalize_path(&descriptor.path, self.case_sensitive);
        Ok(ParsedCommand {
            name: path.join("."),
            command_path: path,
            args: raw_args,
            typed_args,
            raw_text: text.into(),
        })
    }

    pub fn help(&self, path: &[String]) -> BotCommandHelpResult {
        let query = normalize_path(path, self.case_sensitive);
        let prefix = self.prefixes.first().map(String::as_str).unwrap_or("/");
        BotCommandHelpResult {
            commands: self
                .commands
                .iter()
                .filter(|descriptor| {
                    query.is_empty()
                        || normalize_path(&descriptor.path, self.case_sensitive).starts_with(&query)
                        || descriptor.aliases.iter().any(|alias| {
                            normalize_path(alias, self.case_sensitive).starts_with(&query)
                        })
                })
                .map(|descriptor| command_help(descriptor, prefix))
                .collect(),
        }
    }

    pub fn parse_failure(
        &self,
        error: &CommandParseError,
        attempted_path: &[String],
    ) -> BotCommandParseFailure {
        let (code, argument, value) = match error {
            CommandParseError::MissingPrefix => {
                (BotCommandParseErrorCode::MissingPrefix, None, None)
            }
            CommandParseError::EmptyName => (BotCommandParseErrorCode::EmptyName, None, None),
            CommandParseError::UnterminatedQuote => {
                (BotCommandParseErrorCode::UnterminatedQuote, None, None)
            }
            CommandParseError::UnknownCommand(value) => (
                BotCommandParseErrorCode::UnknownCommand,
                None,
                Some(value.clone()),
            ),
            CommandParseError::MissingArgument(argument) => (
                BotCommandParseErrorCode::MissingArgument,
                Some(argument.clone()),
                None,
            ),
            CommandParseError::InvalidArgument { name, value } => (
                BotCommandParseErrorCode::InvalidArgument,
                Some(name.clone()),
                Some(value.clone()),
            ),
            CommandParseError::UnexpectedArgument(value) => (
                BotCommandParseErrorCode::UnexpectedArgument,
                None,
                Some(value.clone()),
            ),
        };
        BotCommandParseFailure {
            code,
            message: error.to_string(),
            argument,
            value,
            help: self.help(attempted_path).commands.into_iter().next(),
        }
    }
}

fn command_help(descriptor: &BotCommandDescriptor, prefix: &str) -> BotCommandHelpEntry {
    let mut usage = format!("{prefix}{}", descriptor.path.join(" "));
    for argument in &descriptor.arguments {
        let placeholder = if argument.variadic {
            format!("{}...", argument.name)
        } else {
            argument.name.clone()
        };
        usage.push(' ');
        if argument.optional || argument.default.is_some() {
            usage.push_str(&format!("[{placeholder}]"));
        } else {
            usage.push_str(&format!("<{placeholder}>"));
        }
    }
    BotCommandHelpEntry {
        path: descriptor.path.clone(),
        aliases: descriptor.aliases.clone(),
        arguments: descriptor.arguments.clone(),
        summary: descriptor.summary.clone(),
        usage,
    }
}

fn tokenize(input: &str) -> Result<Vec<String>, CommandParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut quoted = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if let Some(expected) = quote {
            if ch == expected {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            quoted = true;
        } else if ch.is_whitespace() {
            if !current.is_empty() || quoted {
                tokens.push(std::mem::take(&mut current));
                quoted = false;
            }
        } else {
            current.push(ch);
        }
    }
    if quote.is_some() || escaped {
        return Err(CommandParseError::UnterminatedQuote);
    }
    if !current.is_empty() || quoted {
        tokens.push(current);
    }
    Ok(tokens)
}

fn matching_path_len(
    descriptor: &BotCommandDescriptor,
    tokens: &[String],
    case_sensitive: bool,
) -> Option<usize> {
    std::iter::once(&descriptor.path)
        .chain(descriptor.aliases.iter())
        .filter(|path| path.len() <= tokens.len())
        .filter(|path| {
            path.iter().zip(tokens).all(|(expected, actual)| {
                if case_sensitive {
                    expected == actual
                } else {
                    expected.eq_ignore_ascii_case(actual)
                }
            })
        })
        .map(Vec::len)
        .max()
}

fn normalize_path(path: &[String], case_sensitive: bool) -> Vec<String> {
    if case_sensitive {
        path.to_vec()
    } else {
        path.iter().map(|part| part.to_ascii_lowercase()).collect()
    }
}

fn normalize_part(part: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        part.to_owned()
    } else {
        part.to_ascii_lowercase()
    }
}

fn parse_arguments(
    descriptors: &[BotCommandArgumentDescriptor],
    values: &[String],
) -> Result<BTreeMap<String, BotCommandArgumentValue>, CommandParseError> {
    let mut parsed = BTreeMap::new();
    let mut cursor = 0;
    for descriptor in descriptors {
        if descriptor.variadic {
            let rest = values[cursor..].to_vec();
            if rest.is_empty() && !descriptor.optional && descriptor.default.is_none() {
                return Err(CommandParseError::MissingArgument(descriptor.name.clone()));
            }
            parsed.insert(
                descriptor.name.clone(),
                if rest.is_empty() {
                    descriptor
                        .default
                        .clone()
                        .unwrap_or(BotCommandArgumentValue::Strings(Vec::new()))
                } else {
                    BotCommandArgumentValue::Strings(rest)
                },
            );
            cursor = values.len();
            break;
        }
        let Some(value) = values.get(cursor) else {
            if let Some(default) = descriptor.default.clone() {
                parsed.insert(descriptor.name.clone(), default);
            } else if !descriptor.optional {
                return Err(CommandParseError::MissingArgument(descriptor.name.clone()));
            }
            continue;
        };
        parsed.insert(descriptor.name.clone(), parse_value(descriptor, value)?);
        cursor += 1;
    }
    if let Some(unexpected) = values.get(cursor) {
        return Err(CommandParseError::UnexpectedArgument(unexpected.clone()));
    }
    Ok(parsed)
}

fn parse_value(
    descriptor: &BotCommandArgumentDescriptor,
    value: &str,
) -> Result<BotCommandArgumentValue, CommandParseError> {
    let invalid = || CommandParseError::InvalidArgument {
        name: descriptor.name.clone(),
        value: value.into(),
    };
    match descriptor.kind {
        BotCommandArgumentKind::String => Ok(BotCommandArgumentValue::String(value.into())),
        BotCommandArgumentKind::Integer => value
            .parse()
            .map(BotCommandArgumentValue::Integer)
            .map_err(|_| invalid()),
        BotCommandArgumentKind::Number => value
            .parse()
            .map(BotCommandArgumentValue::Number)
            .map_err(|_| invalid()),
        BotCommandArgumentKind::Boolean => match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(BotCommandArgumentValue::Boolean(true)),
            "false" | "no" | "off" | "0" => Ok(BotCommandArgumentValue::Boolean(false)),
            _ => Err(invalid()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_group_alias_quoted_and_typed_arguments() {
        let parser = CommandParser::new(vec!["/".into()]).commands(vec![BotCommandDescriptor {
            path: vec!["admin".into(), "ban".into()],
            aliases: vec![vec!["a".into(), "b".into()]],
            arguments: vec![
                BotCommandArgumentDescriptor {
                    name: "user".into(),
                    kind: BotCommandArgumentKind::String,
                    optional: false,
                    variadic: false,
                    default: None,
                },
                BotCommandArgumentDescriptor {
                    name: "days".into(),
                    kind: BotCommandArgumentKind::Integer,
                    optional: false,
                    variadic: false,
                    default: None,
                },
            ],
            summary: None,
        }]);

        let parsed = parser.parse("/a b \"Alice Smith\" 7").unwrap();
        assert_eq!(parsed.name, "admin.ban");
        assert_eq!(
            parsed.typed_args["user"],
            BotCommandArgumentValue::String("Alice Smith".into())
        );
        assert_eq!(
            parsed.typed_args["days"],
            BotCommandArgumentValue::Integer(7)
        );

        let unquoted = parser.parse("/a b Alice 7").unwrap();
        assert_eq!(
            unquoted.typed_args["user"],
            BotCommandArgumentValue::String("Alice".into())
        );
    }

    #[test]
    fn help_and_parse_failure_are_structured_and_discoverable() {
        let parser = CommandParser::new(vec!["!".into()]).commands(vec![BotCommandDescriptor {
            path: vec!["admin".into(), "ban".into()],
            aliases: vec![vec!["a".into(), "b".into()]],
            arguments: vec![BotCommandArgumentDescriptor {
                name: "user".into(),
                kind: BotCommandArgumentKind::String,
                optional: false,
                variadic: false,
                default: None,
            }],
            summary: Some("Ban a user".into()),
        }]);
        let help = parser.help(&["admin".into()]);
        assert_eq!(help.commands[0].usage, "!admin ban <user>");
        let error = parser.parse("!admin ban").unwrap_err();
        let failure = parser.parse_failure(&error, &["admin".into(), "ban".into()]);
        assert_eq!(failure.code, BotCommandParseErrorCode::MissingArgument);
        assert_eq!(failure.argument.as_deref(), Some("user"));
        assert_eq!(failure.help.unwrap().summary.as_deref(), Some("Ban a user"));
    }
}
