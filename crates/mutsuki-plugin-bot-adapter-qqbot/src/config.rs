#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqBotConfig {
    pub account_id: String,
    pub app_id: String,
    pub client_secret: String,
    pub token_url: String,
    pub openapi_base_url: String,
    pub gateway_intents: u64,
    pub shard: [u64; 2],
    pub token_refresh_margin_secs: u64,
    pub max_retry_attempts: u8,
}

impl QqBotConfig {
    pub fn new(
        account_id: impl Into<String>,
        app_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            app_id: app_id.into(),
            client_secret: client_secret.into(),
            token_url: "https://bots.qq.com/app/getAppAccessToken".into(),
            openapi_base_url: "https://api.sgroup.qq.com".into(),
            gateway_intents: 1_325_405_185,
            shard: [0, 1],
            token_refresh_margin_secs: 120,
            max_retry_attempts: 3,
        }
    }
}
