use mutsuki_bot_protocol::ConversationIclEntry;

pub trait ConversationContextStore: Send + Sync {
    fn record_icl(
        &self,
        origin_key: &str,
        entry: ConversationIclEntry,
        max_count: usize,
    ) -> Result<(), String>;
    fn load_icl(
        &self,
        origin_key: &str,
        max_count: usize,
    ) -> Result<Vec<ConversationIclEntry>, String>;
}
