use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use clap::Parser;
use mutsuki_create_bot::{
    CreateBotOptions, DEFAULT_REVISION, create_bot_project, source_is_dirty, validate_project_name,
    validate_revision,
};

#[derive(Debug, Parser)]
#[command(
    name = "cargo create-bot",
    about = "Create a thin Bot product pinned to one Mutsuki revision"
)]
struct Arguments {
    /// Cargo package name and default output directory. Omit it for interactive setup.
    name: Option<String>,
    /// Output directory. It must not already exist.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Full 40-character Mutsuki Git revision. Defaults to this CLI's clean source revision.
    #[arg(long)]
    revision: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse_from(normalized_cargo_args(std::env::args_os()));
    let interactive = arguments.name.is_none();
    if interactive && !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return Err(io::Error::other(
            "a Bot name is required when input is not an interactive terminal; run `cargo create-bot <name> ...`",
        )
        .into());
    }

    let mut input = io::stdin().lock();
    let mut output = io::stderr().lock();
    let Some(options) = resolve_options(arguments, source_is_dirty(), &mut input, &mut output)?
    else {
        println!("Cancelled; no files were created.");
        return Ok(());
    };

    let created = create_bot_project(&options)?;
    println!("Created Bot project at {}", created.root.display());
    println!("Pinned Mutsuki revision: {}", created.revision);
    println!("Next: cd {} && cargo run", created.root.display());
    Ok(())
}

fn normalized_cargo_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    let Some(program) = args.next() else {
        return Vec::new();
    };
    let mut remaining: Vec<_> = args.collect();
    if remaining
        .first()
        .is_some_and(|argument| argument == "create-bot")
    {
        remaining.remove(0);
    }
    std::iter::once(program).chain(remaining).collect()
}

fn resolve_options(
    arguments: Arguments,
    dirty_source: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<CreateBotOptions>, Box<dyn std::error::Error>> {
    let interactive = arguments.name.is_none();
    let name = match arguments.name {
        Some(name) => name,
        None => prompt_until_valid(
            input,
            output,
            "Bot package name",
            None,
            validate_project_name,
        )?,
    };

    let target = match (arguments.output, interactive) {
        (Some(target), _) => target,
        (None, true) => PathBuf::from(prompt(input, output, "Output directory", Some(&name))?),
        (None, false) => PathBuf::from(&name),
    };
    let revision = match (arguments.revision, interactive, dirty_source) {
        (Some(revision), _, _) => revision,
        (None, true, true) => prompt_until_valid(
            input,
            output,
            "Published Mutsuki revision (40 hex characters)",
            None,
            validate_revision,
        )?,
        (None, true, false) => prompt_until_valid(
            input,
            output,
            "Mutsuki revision",
            Some(DEFAULT_REVISION),
            validate_revision,
        )?,
        (None, false, true) => {
            return Err(io::Error::other(
                "create-bot was built from a dirty Mutsuki checkout; commit the candidate or pass an explicit published --revision",
            )
            .into());
        }
        (None, false, false) => DEFAULT_REVISION.into(),
    };

    let options = CreateBotOptions::new(name, target).with_revision(revision);

    if interactive {
        writeln!(output)?;
        writeln!(output, "Package:    {}", options.name)?;
        writeln!(output, "Directory:  {}", options.output.display())?;
        writeln!(output, "Revision:   {}", options.revision)?;
        if !confirm(input, output)? {
            return Ok(None);
        }
    }

    Ok(Some(options))
}

fn prompt_until_valid<E>(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: Option<&str>,
    validate: impl Fn(&str) -> Result<(), E>,
) -> io::Result<String>
where
    E: std::fmt::Display,
{
    loop {
        let value = prompt(input, output, label, default)?;
        match validate(&value) {
            Ok(()) => return Ok(value),
            Err(error) => writeln!(output, "{error}")?,
        }
    }
}

fn prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: Option<&str>,
) -> io::Result<String> {
    match default {
        Some(default) => write!(output, "{label} [{default}]: ")?,
        None => write!(output, "{label}: ")?,
    }
    output.flush()?;

    let mut value = String::new();
    if input.read_line(&mut value)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("input ended while reading {label}"),
        ));
    }
    let value = value.trim();
    Ok(if value.is_empty() {
        default.unwrap_or_default().into()
    } else {
        value.into()
    })
}

fn confirm(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<bool> {
    loop {
        let answer = prompt(input, output, "Create this Bot?", Some("Y/n"))?;
        match answer.to_ascii_lowercase().as_str() {
            "y" | "yes" | "y/n" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "Enter y or n.")?,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn cargo_subcommand_argument_is_removed_before_parsing() {
        let arguments = normalized_cargo_args([
            OsString::from("cargo-create-bot"),
            OsString::from("create-bot"),
            OsString::from("hello-bot"),
        ]);
        let parsed = Arguments::try_parse_from(arguments).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("hello-bot"));
    }

    #[test]
    fn interactive_defaults_produce_reproducible_options() {
        let mut input = Cursor::new("hello-bot\n\n\n\n");
        let mut output = Vec::new();
        let options = resolve_options(
            Arguments {
                name: None,
                output: None,
                revision: None,
            },
            false,
            &mut input,
            &mut output,
        )
        .unwrap()
        .unwrap();

        assert_eq!(options.name, "hello-bot");
        assert_eq!(options.output, PathBuf::from("hello-bot"));
        assert_eq!(options.revision, DEFAULT_REVISION);
    }

    #[test]
    fn interactive_confirmation_can_cancel_creation() {
        let input = format!("hello-bot\n\n{DEFAULT_REVISION}\nn\n");
        let mut input = Cursor::new(input);
        let mut output = Vec::new();
        let options = resolve_options(
            Arguments {
                name: None,
                output: None,
                revision: None,
            },
            true,
            &mut input,
            &mut output,
        )
        .unwrap();

        assert!(options.is_none());
    }
}
