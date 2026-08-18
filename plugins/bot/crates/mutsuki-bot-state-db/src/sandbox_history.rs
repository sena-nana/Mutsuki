use mutsuki_bot_protocol::{BotConversationKind, QqConversationRef};
use mutsuki_bot_sandbox::{
    SANDBOX_MAX_MEDIA_ITEMS, SANDBOX_MAX_MESSAGES, SandboxConversationView, SandboxError,
    SandboxHistoryConversation, SandboxHistoryKind, SandboxHistorySnapshot, SandboxHistoryStore,
    SandboxMediaBlob, SandboxMessageView, SandboxMode, SandboxSpeakerRole, SandboxUserView,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{BotStateDbError, BotStateDbRepository, decode, encode, immediate, sqlite_integer};

pub(super) const SANDBOX_SCHEMA_SQL: &str = "
         CREATE TABLE IF NOT EXISTS bot_sandbox_meta(
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             mode TEXT NOT NULL,
             account_id TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_sandbox_conversation(
             store TEXT NOT NULL CHECK(store IN ('simulate', 'live')),
             conversation_id TEXT NOT NULL,
             account_id TEXT NOT NULL,
             kind TEXT NOT NULL,
             title TEXT NOT NULL,
             avatar_url TEXT,
             conversation_json TEXT NOT NULL,
             last_preview TEXT,
             last_activity_unix_ms INTEGER NOT NULL,
             message_count INTEGER NOT NULL,
             active_message INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY(store, conversation_id)
         );
         CREATE TABLE IF NOT EXISTS bot_sandbox_user(
             store TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             user_id TEXT NOT NULL,
             display_name TEXT NOT NULL,
             avatar_url TEXT,
             last_seen_unix_ms INTEGER NOT NULL,
             message_count INTEGER NOT NULL,
             PRIMARY KEY(store, conversation_id, user_id),
             FOREIGN KEY(store, conversation_id)
                 REFERENCES bot_sandbox_conversation(store, conversation_id)
                 ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS bot_sandbox_message(
             store TEXT NOT NULL,
             message_id TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             sender_id TEXT NOT NULL,
             sender_name TEXT NOT NULL,
             role TEXT NOT NULL,
             text TEXT NOT NULL,
             segments_json TEXT NOT NULL,
             reply_to TEXT,
             time_ms INTEGER NOT NULL,
             PRIMARY KEY(store, message_id),
             FOREIGN KEY(store, conversation_id)
                 REFERENCES bot_sandbox_conversation(store, conversation_id)
                 ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS bot_sandbox_message_conversation
             ON bot_sandbox_message(store, conversation_id, time_ms);
         CREATE TABLE IF NOT EXISTS bot_sandbox_media(
             media_id TEXT PRIMARY KEY,
             mime TEXT NOT NULL,
             name TEXT NOT NULL,
             bytes BLOB NOT NULL,
             created_at_unix_ms INTEGER NOT NULL
         );";

pub(super) fn load(connection: &Connection) -> Result<SandboxHistorySnapshot, BotStateDbError> {
    let (mode, account_id) = load_meta(connection)?;
    let mut simulate = Vec::new();
    let mut live = Vec::new();
    for conversation in load_conversations(connection, None, true)? {
        match conversation.kind {
            SandboxHistoryKind::Simulate => simulate.push(conversation.record),
            SandboxHistoryKind::Live => live.push(conversation.record),
        }
    }
    Ok(SandboxHistorySnapshot {
        mode,
        account_id,
        simulate,
        live,
        media: load_media(connection)?,
    })
}

fn load_meta(connection: &Connection) -> Result<(SandboxMode, String), BotStateDbError> {
    Ok(connection
        .query_row(
            "SELECT mode, account_id FROM bot_sandbox_meta WHERE singleton=1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .map(|(mode, account_id)| {
            let mode = if mode == "live" {
                SandboxMode::Live
            } else {
                SandboxMode::Simulate
            };
            (mode, account_id)
        })
        .unwrap_or((SandboxMode::Simulate, String::new())))
}

pub(super) fn load_conversation_views(
    connection: &Connection,
    kind: SandboxHistoryKind,
) -> Result<Vec<SandboxConversationView>, BotStateDbError> {
    load_conversations(connection, Some(kind), false)
        .map(|items| items.into_iter().map(|item| item.record.view).collect())
}

pub(super) fn load_conversation_messages(
    connection: &Connection,
    kind: SandboxHistoryKind,
    conversation_id: &str,
) -> Result<Vec<SandboxMessageView>, BotStateDbError> {
    load_messages(connection, kind.as_str(), conversation_id)
}

pub(super) fn load_media_by_id(
    connection: &Connection,
    media_id: &str,
) -> Result<Option<SandboxMediaBlob>, BotStateDbError> {
    connection
        .query_row(
            "SELECT media_id, mime, name, bytes FROM bot_sandbox_media WHERE media_id=?1",
            params![media_id],
            |row| {
                Ok(SandboxMediaBlob {
                    media_id: row.get(0)?,
                    mime: row.get(1)?,
                    name: row.get(2)?,
                    bytes: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(BotStateDbError::from)
}

struct LoadedConversation {
    kind: SandboxHistoryKind,
    record: SandboxHistoryConversation,
}

fn load_conversations(
    connection: &Connection,
    kind: Option<SandboxHistoryKind>,
    include_messages: bool,
) -> Result<Vec<LoadedConversation>, BotStateDbError> {
    let sql = if kind.is_some() {
        "SELECT store, conversation_id, account_id, kind, title, avatar_url, conversation_json,
                last_preview, last_activity_unix_ms, message_count, active_message
         FROM bot_sandbox_conversation
         WHERE store=?1
         ORDER BY conversation_id"
    } else {
        "SELECT store, conversation_id, account_id, kind, title, avatar_url, conversation_json,
                last_preview, last_activity_unix_ms, message_count, active_message
         FROM bot_sandbox_conversation
         ORDER BY store, conversation_id"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = if let Some(kind) = kind {
        statement
            .query_map(params![kind.as_str()], map_conversation_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        statement
            .query_map([], map_conversation_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut loaded = Vec::new();
    for (
        store,
        conversation_id,
        account_id,
        kind,
        title,
        avatar_url,
        conversation_json,
        last_preview,
        last_activity_unix_ms,
        message_count,
        active_message,
    ) in rows
    {
        let history_kind = parse_kind(&store)?;
        let conversation: QqConversationRef = decode(&conversation_json)?;
        let kind: BotConversationKind = decode(&format!("\"{kind}\""))?;
        let users = load_users(connection, &store, &conversation_id)?;
        let messages = if include_messages {
            load_messages(connection, &store, &conversation_id)?
        } else {
            Vec::new()
        };
        loaded.push(LoadedConversation {
            kind: history_kind,
            record: SandboxHistoryConversation {
                view: SandboxConversationView {
                    conversation_id,
                    account_id,
                    kind,
                    title,
                    avatar_url,
                    conversation,
                    users: users.clone(),
                    last_preview,
                    last_activity_unix_ms: super::sqlite_unsigned(
                        last_activity_unix_ms,
                        "last_activity_unix_ms",
                    )?,
                    message_count: super::sqlite_unsigned(message_count, "message_count")?,
                    active_message: active_message != 0,
                },
                users,
                messages,
            },
        });
    }
    Ok(loaded)
}

fn map_conversation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    i64,
    i64,
    i64,
)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn load_users(
    connection: &Connection,
    store: &str,
    conversation_id: &str,
) -> Result<Vec<SandboxUserView>, BotStateDbError> {
    let mut statement = connection.prepare(
        "SELECT user_id, display_name, avatar_url, last_seen_unix_ms, message_count
         FROM bot_sandbox_user
         WHERE store=?1 AND conversation_id=?2
         ORDER BY user_id",
    )?;
    statement
        .query_map(params![store, conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .map(|row| {
            let (user_id, display_name, avatar_url, last_seen_unix_ms, message_count) = row?;
            Ok(SandboxUserView {
                user_id,
                display_name,
                avatar_url,
                last_seen_unix_ms: super::sqlite_unsigned(last_seen_unix_ms, "last_seen_unix_ms")?,
                message_count: super::sqlite_unsigned(message_count, "message_count")?,
            })
        })
        .collect()
}

fn load_messages(
    connection: &Connection,
    store: &str,
    conversation_id: &str,
) -> Result<Vec<SandboxMessageView>, BotStateDbError> {
    let mut statement = connection.prepare(
        "SELECT message_id, sender_id, sender_name, role, text, segments_json, reply_to, time_ms
         FROM bot_sandbox_message
         WHERE store=?1 AND conversation_id=?2
         ORDER BY time_ms ASC, message_id ASC",
    )?;
    let rows = statement
        .query_map(params![store, conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut messages = Vec::new();
    for (message_id, sender_id, sender_name, role, text, segments_json, reply_to, time_ms) in rows {
        messages.push(SandboxMessageView {
            message_id,
            conversation_id: conversation_id.to_owned(),
            sender_id,
            sender_name,
            role: parse_role(&role)?,
            text,
            segments: decode(&segments_json)?,
            reply_to,
            time_ms,
        });
    }
    if messages.len() > SANDBOX_MAX_MESSAGES {
        messages.drain(0..messages.len() - SANDBOX_MAX_MESSAGES);
    }
    Ok(messages)
}

fn load_media(connection: &Connection) -> Result<Vec<SandboxMediaBlob>, BotStateDbError> {
    let mut statement = connection.prepare(
        "SELECT media_id, mime, name, bytes
         FROM bot_sandbox_media
         ORDER BY created_at_unix_ms ASC, media_id ASC",
    )?;
    statement
        .query_map([], |row| {
            Ok(SandboxMediaBlob {
                media_id: row.get(0)?,
                mime: row.get(1)?,
                name: row.get(2)?,
                bytes: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(BotStateDbError::from)
}

pub(super) fn save(
    connection: &mut Connection,
    snapshot: &SandboxHistorySnapshot,
) -> Result<(), BotStateDbError> {
    let transaction = immediate(connection)?;
    transaction.execute(
        "INSERT INTO bot_sandbox_meta(singleton, mode, account_id) VALUES (1, ?1, ?2)
         ON CONFLICT(singleton) DO UPDATE SET mode=excluded.mode, account_id=excluded.account_id",
        params![mode_name(snapshot.mode), snapshot.account_id],
    )?;
    transaction.execute("DELETE FROM bot_sandbox_message", [])?;
    transaction.execute("DELETE FROM bot_sandbox_user", [])?;
    transaction.execute("DELETE FROM bot_sandbox_conversation", [])?;
    transaction.execute("DELETE FROM bot_sandbox_media", [])?;
    write_conversations(
        &transaction,
        SandboxHistoryKind::Simulate,
        &snapshot.simulate,
    )?;
    write_conversations(&transaction, SandboxHistoryKind::Live, &snapshot.live)?;
    let start = snapshot.media.len().saturating_sub(SANDBOX_MAX_MEDIA_ITEMS);
    for (index, media) in snapshot.media[start..].iter().enumerate() {
        transaction.execute(
            "INSERT INTO bot_sandbox_media(media_id, mime, name, bytes, created_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                media.media_id,
                media.mime,
                media.name,
                media.bytes,
                sqlite_integer(u64::try_from(index).unwrap_or(u64::MAX))?,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn write_conversations(
    connection: &Connection,
    kind: SandboxHistoryKind,
    items: &[SandboxHistoryConversation],
) -> Result<(), BotStateDbError> {
    for conversation in items {
        insert_conversation_row(connection, kind, &conversation.view)?;
        for user in &conversation.users {
            insert_user(connection, kind, &conversation.view.conversation_id, user)?;
        }
        let messages = if conversation.messages.len() > SANDBOX_MAX_MESSAGES {
            &conversation.messages[conversation.messages.len() - SANDBOX_MAX_MESSAGES..]
        } else {
            conversation.messages.as_slice()
        };
        for message in messages {
            insert_message(connection, kind, message)?;
        }
    }
    Ok(())
}

fn insert_conversation_row(
    connection: &Connection,
    kind: SandboxHistoryKind,
    conversation: &SandboxConversationView,
) -> Result<(), BotStateDbError> {
    connection.execute(
        "INSERT INTO bot_sandbox_conversation(
             store, conversation_id, account_id, kind, title, avatar_url, conversation_json,
             last_preview, last_activity_unix_ms, message_count, active_message
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            kind.as_str(),
            conversation.conversation_id,
            conversation.account_id,
            kind_name(conversation.kind),
            conversation.title,
            conversation.avatar_url,
            encode(&conversation.conversation)?,
            conversation.last_preview,
            sqlite_integer(conversation.last_activity_unix_ms)?,
            sqlite_integer(conversation.message_count)?,
            i64::from(conversation.active_message),
        ],
    )?;
    Ok(())
}

fn insert_user(
    connection: &Connection,
    kind: SandboxHistoryKind,
    conversation_id: &str,
    user: &SandboxUserView,
) -> Result<(), BotStateDbError> {
    connection.execute(
        "INSERT INTO bot_sandbox_user(
             store, conversation_id, user_id, display_name, avatar_url, last_seen_unix_ms, message_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            kind.as_str(),
            conversation_id,
            user.user_id,
            user.display_name,
            user.avatar_url,
            sqlite_integer(user.last_seen_unix_ms)?,
            sqlite_integer(user.message_count)?,
        ],
    )?;
    Ok(())
}

fn insert_message(
    connection: &Connection,
    kind: SandboxHistoryKind,
    message: &SandboxMessageView,
) -> Result<(), BotStateDbError> {
    connection.execute(
        "INSERT INTO bot_sandbox_message(
             store, message_id, conversation_id, sender_id, sender_name, role, text, segments_json, reply_to, time_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            kind.as_str(),
            message.message_id,
            message.conversation_id,
            message.sender_id,
            message.sender_name,
            role_name(message.role),
            message.text,
            encode(&message.segments)?,
            message.reply_to,
            message.time_ms,
        ],
    )?;
    Ok(())
}

fn parse_kind(value: &str) -> Result<SandboxHistoryKind, BotStateDbError> {
    SandboxHistoryKind::parse(value).map_err(|error| BotStateDbError::Invariant(error.message))
}

fn parse_role(value: &str) -> Result<SandboxSpeakerRole, BotStateDbError> {
    match value {
        "user" => Ok(SandboxSpeakerRole::User),
        "bot" => Ok(SandboxSpeakerRole::Bot),
        "system" => Ok(SandboxSpeakerRole::System),
        _ => Err(BotStateDbError::Invariant(format!(
            "unknown sandbox speaker role `{value}`"
        ))),
    }
}

fn mode_name(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::Simulate => "simulate",
        SandboxMode::Live => "live",
    }
}

fn kind_name(kind: BotConversationKind) -> &'static str {
    match kind {
        BotConversationKind::Private => "private",
        BotConversationKind::Group => "group",
        BotConversationKind::Channel => "channel",
    }
}

fn role_name(role: SandboxSpeakerRole) -> &'static str {
    match role {
        SandboxSpeakerRole::User => "user",
        SandboxSpeakerRole::Bot => "bot",
        SandboxSpeakerRole::System => "system",
    }
}

fn sandbox_error(error: BotStateDbError) -> SandboxError {
    SandboxError::new("sandbox.history", error.to_string())
}

impl BotStateDbRepository {
    pub fn sandbox_conversations(
        &self,
        kind: SandboxHistoryKind,
    ) -> Result<Vec<SandboxConversationView>, BotStateDbError> {
        self.call_sync(|reply| super::DbJob::SandboxConversations { kind, reply })
    }

    pub fn sandbox_messages(
        &self,
        kind: SandboxHistoryKind,
        conversation_id: &str,
    ) -> Result<Vec<SandboxMessageView>, BotStateDbError> {
        let conversation_id = conversation_id.to_owned();
        self.call_sync(|reply| super::DbJob::SandboxMessages {
            kind,
            conversation_id,
            reply,
        })
    }

    pub fn sandbox_media(
        &self,
        media_id: &str,
    ) -> Result<Option<SandboxMediaBlob>, BotStateDbError> {
        let media_id = media_id.to_owned();
        self.call_sync(|reply| super::DbJob::SandboxMedia { media_id, reply })
    }
}

impl SandboxHistoryStore for BotStateDbRepository {
    fn load(&self) -> Result<SandboxHistorySnapshot, SandboxError> {
        self.call_sync(|reply| super::DbJob::SandboxLoad { reply })
            .map_err(sandbox_error)
    }

    fn save(&self, snapshot: &SandboxHistorySnapshot) -> Result<(), SandboxError> {
        let snapshot = snapshot.clone();
        self.call_sync(|reply| super::DbJob::SandboxSave { snapshot, reply })
            .map_err(sandbox_error)
    }
}
