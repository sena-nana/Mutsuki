#![no_main]

//! Chat text is the least trusted input the Bot product accepts: anyone in a group can send it,
//! and the parser walks it with quoting, alias resolution and typed-argument coercion before any
//! runner sees the result. The descriptor set below is fixed so the fuzzer spends its budget on
//! the text, while still covering nested paths, aliases, optionals, defaults and variadics.

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use mutsuki_bot_protocol::{
    BotCommandArgumentDescriptor, BotCommandArgumentKind, BotCommandArgumentValue,
    BotCommandDescriptor,
};
use mutsuki_plugin_bot_command::{CommandParser, validate_command_descriptors};

static PARSER: LazyLock<CommandParser> = LazyLock::new(|| {
    let commands = vec![
        BotCommandDescriptor {
            path: vec!["admin".into(), "ban".into()],
            aliases: vec![vec!["a".into(), "b".into()], vec!["封禁".into()]],
            arguments: vec![
                argument("user", BotCommandArgumentKind::String, false, false, None),
                argument("days", BotCommandArgumentKind::Integer, false, false, None),
                argument(
                    "ratio",
                    BotCommandArgumentKind::Number,
                    true,
                    false,
                    Some(BotCommandArgumentValue::Number(1.0)),
                ),
                argument("silent", BotCommandArgumentKind::Boolean, true, false, None),
                argument("reason", BotCommandArgumentKind::String, true, true, None),
            ],
            summary: Some("ban a user".into()),
        },
        BotCommandDescriptor {
            path: vec!["help".into()],
            aliases: Vec::new(),
            arguments: Vec::new(),
            summary: None,
        },
    ];
    validate_command_descriptors(&commands).expect("fuzz descriptors are valid");
    CommandParser::new(vec!["/".into(), "//".into(), "！".into()]).commands(commands)
});

fn argument(
    name: &str,
    kind: BotCommandArgumentKind,
    optional: bool,
    variadic: bool,
    default: Option<BotCommandArgumentValue>,
) -> BotCommandArgumentDescriptor {
    BotCommandArgumentDescriptor {
        name: name.into(),
        kind,
        optional,
        variadic,
        default,
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match PARSER.parse(text) {
        // A parsed command must stay inside the declared surface, or a downstream runner would be
        // handed a name it never registered.
        Ok(command) => {
            assert!(!command.command_path.is_empty());
            assert_eq!(command.name, command.command_path.join("."));
        }
        // Every failure has to survive projection into the structured reply the Bot sends back.
        Err(error) => {
            let failure = PARSER.parse_failure(&error, &[]);
            assert!(!failure.message.is_empty());
        }
    }
    let _ = PARSER.help(&[text.to_owned()]);
});
