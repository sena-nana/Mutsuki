use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<String>,
    #[serde(default)]
    pub allow_domains: Vec<String>,
    #[serde(default)]
    pub deny_domains: Vec<String>,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
}

fn default_search_limit() -> u32 {
    8
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default)]
    pub untrusted_content: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: SearchQuery,
    pub hits: Vec<SearchHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_ref: Option<ResourceRef>,
    pub provider_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageFetchRequest {
    pub url: String,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u32,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub allow_browser_fallback: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_redirects() -> u32 {
    3
}

fn default_max_bytes() -> u64 {
    1_048_576
}

fn default_timeout_ms() -> u64 {
    10_000
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageFetchResult {
    pub requested_url: String,
    pub final_url: String,
    pub canonical_url: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub body_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<ResourceRef>,
    pub truncated: bool,
    pub used_browser_fallback: bool,
    pub untrusted_content: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractedPage {
    pub url: String,
    pub canonical_url: String,
    pub title: String,
    pub text_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub untrusted_content: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebCitation {
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub provenance: String,
    pub untrusted_content: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchProviderConfig {
    pub provider_id: String,
    pub endpoint: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub credential_env: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub enable_http: bool,
    #[serde(default)]
    pub enable_browser_fallback: bool,
}

impl Default for SearchProviderConfig {
    fn default() -> Self {
        Self {
            provider_id: "generic-json".into(),
            endpoint: String::new(),
            headers: Vec::new(),
            credential_env: None,
            timeout_ms: Some(default_timeout_ms()),
            enable_http: true,
            enable_browser_fallback: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WebSearchServiceRequest {
    Search { query: SearchQuery },
    Fetch { request: PageFetchRequest },
    Extract { request: PageFetchRequest },
    Cite { hits: Vec<SearchHit> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebSearchServiceResponse {
    Search(SearchResult),
    Fetch(PageFetchResult),
    Extract(ExtractedPage),
    Cite { citations: Vec<WebCitation> },
    Ack,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSearchContextInput {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub extra: Value,
}
