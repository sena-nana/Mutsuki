use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandParser {
    prefixes: Vec<String>,
    case_sensitive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub args: Vec<String>,
    pub raw_text: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("message does not start with a command prefix")]
    MissingPrefix,
    #[error("command name is empty")]
    EmptyName,
}

impl CommandParser {
    pub fn new(prefixes: Vec<String>) -> Self {
        Self {
            prefixes,
            case_sensitive: false,
        }
    }

    pub fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub fn parse(&self, text: &str) -> Result<ParsedCommand, CommandParseError> {
        let trimmed = text.trim();
        let Some(prefix) = self
            .prefixes
            .iter()
            .find(|prefix| trimmed.starts_with(prefix.as_str()))
        else {
            return Err(CommandParseError::MissingPrefix);
        };
        let command_text = trimmed[prefix.len()..].trim();
        if command_text.is_empty() {
            return Err(CommandParseError::EmptyName);
        }
        let parts = command_text
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut name = parts[0].clone();
        if !self.case_sensitive {
            name = name.to_ascii_lowercase();
        }
        Ok(ParsedCommand {
            name,
            args: parts.into_iter().skip(1).collect(),
            raw_text: text.into(),
        })
    }
}
