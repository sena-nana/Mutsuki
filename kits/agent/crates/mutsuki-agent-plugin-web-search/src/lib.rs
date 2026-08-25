// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use
)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor,
    ContextProviderRequest, ContextProviderResult, ExtractedPage, PageFetchRequest,
    PageFetchResult, SearchHit, SearchProviderConfig, SearchQuery, SearchResult, ToolSideEffect,
    WebCitation, WebSearchServiceRequest, WebSearchServiceResponse,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde_json::{Value, json};
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.web-search";
pub const SERVICE_ID: &str = "mutsuki.agent.service.web-search";
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.web-search";
pub const INLINE_BODY_LIMIT: usize = 2_048;
pub const SUMMARY_LIMIT: usize = 512;

pub trait WebHttpGateway: Send + Sync {
    fn post_json(
        &self,
        endpoint: &str,
        headers: &[(String, String)],
        body: &Value,
        timeout: Duration,
    ) -> Result<(u16, Value), AgentError>;

    fn get_bytes(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<(u16, String, Vec<u8>, Option<String>), AgentError>;
}

pub trait BrowserFetchGateway: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<(String, String, Vec<u8>), AgentError>;
}

pub trait SearchService: Send + Sync {
    fn search(&self, query: SearchQuery) -> Result<SearchResult, AgentError>;
}

pub trait PageFetchService: Send + Sync {
    fn fetch(&self, request: PageFetchRequest) -> Result<PageFetchResult, AgentError>;
    fn extract(&self, request: PageFetchRequest) -> Result<ExtractedPage, AgentError>;
}

#[derive(Clone, Debug)]
pub struct UrlAccessPolicy {
    pub allow_private: bool,
    pub allow_localhost: bool,
    pub allow_metadata: bool,
    pub allowed_schemes: BTreeSet<String>,
}

impl Default for UrlAccessPolicy {
    fn default() -> Self {
        Self {
            allow_private: false,
            allow_localhost: false,
            allow_metadata: false,
            allowed_schemes: BTreeSet::from(["https".into(), "http".into()]),
        }
    }
}

impl UrlAccessPolicy {
    pub fn validate(&self, raw: &str) -> Result<Url, AgentError> {
        let url = Url::parse(raw).map_err(|error| AgentError::invalid_input(error.to_string()))?;
        if !self.allowed_schemes.contains(url.scheme()) {
            return Err(AgentError::new(
                "agent.web_search.scheme_denied",
                format!("URL scheme `{}` is not allowed", url.scheme()),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| AgentError::invalid_input("URL host is required"))?;
        let host_lower = host.to_ascii_lowercase();
        if !self.allow_localhost && matches!(host_lower.as_str(), "localhost" | "127.0.0.1" | "::1")
        {
            return Err(AgentError::new(
                "agent.web_search.ssrf_denied",
                "localhost URLs are denied by default",
            ));
        }
        if !self.allow_metadata
            && (host_lower == "metadata.google.internal"
                || host_lower.ends_with(".metadata.google.internal")
                || host_lower == "169.254.169.254")
        {
            return Err(AgentError::new(
                "agent.web_search.ssrf_denied",
                "cloud metadata endpoints are denied by default",
            ));
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if !self.allow_private && is_private_or_link_local(ip) {
                return Err(AgentError::new(
                    "agent.web_search.ssrf_denied",
                    "private or link-local addresses are denied by default",
                ));
            }
            if !self.allow_localhost && ip.is_loopback() {
                return Err(AgentError::new(
                    "agent.web_search.ssrf_denied",
                    "loopback addresses are denied by default",
                ));
            }
        }
        Ok(url)
    }

    pub fn same_registrable_origin(&self, left: &Url, right: &Url) -> bool {
        left.scheme() == right.scheme()
            && left.host_str().map(str::to_ascii_lowercase)
                == right.host_str().map(str::to_ascii_lowercase)
    }
}

fn is_private_or_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || is_carrier_grade_nat(v4)
        }
        IpAddr::V6(v6) => {
            is_unique_local(v6)
                || is_unicast_link_local(v6)
                || v6.is_unspecified()
                || is_ipv4_mapped_private(v6)
        }
    }
}

fn is_carrier_grade_nat(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000
}

fn is_ipv4_mapped_private(ip: Ipv6Addr) -> bool {
    ip.to_ipv4_mapped()
        .is_some_and(|v4| v4.is_private() || v4.is_loopback() || v4.is_link_local())
}

fn is_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

pub fn canonicalize_url(raw: &str) -> Result<String, AgentError> {
    let mut url = Url::parse(raw).map_err(|error| AgentError::invalid_input(error.to_string()))?;
    url.set_fragment(None);
    if let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) {
        let _ = url.set_host(Some(&host));
    }
    if (url.scheme() == "http" && url.port() == Some(80))
        || (url.scheme() == "https" && url.port() == Some(443))
    {
        let _ = url.set_port(None);
    }
    let mut path = url.path().to_string();
    while path.ends_with('/') && path.len() > 1 {
        path.pop();
    }
    url.set_path(&path);
    if let Some(query) = url.query() {
        let mut pairs = url::form_urlencoded::parse(query.as_bytes())
            .filter(|(key, _)| {
                let key = key.to_ascii_lowercase();
                !matches!(
                    key.as_str(),
                    "utm_source"
                        | "utm_medium"
                        | "utm_campaign"
                        | "utm_term"
                        | "utm_content"
                        | "fbclid"
                        | "gclid"
                )
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        pairs.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        if pairs.is_empty() {
            url.set_query(None);
        } else {
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            url.set_query(Some(&encoded));
        }
    }
    Ok(url.into())
}

pub fn dedup_hits(mut hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut seen = BTreeSet::new();
    hits.retain(|hit| seen.insert(hit.canonical_url.clone()));
    hits
}

pub fn citations_from_hits(hits: &[SearchHit]) -> Vec<WebCitation> {
    hits.iter()
        .map(|hit| WebCitation {
            title: hit.title.clone(),
            url: hit.url.clone(),
            canonical_url: hit.canonical_url.clone(),
            snippet: hit.snippet.clone(),
            published_at: hit.published_at.clone(),
            provenance: "web.search".into(),
            untrusted_content: true,
        })
        .collect()
}

pub fn extract_html(url: &str, html: &str) -> ExtractedPage {
    let title = capture_tag(html, "title").unwrap_or_else(|| url.to_string());
    let canonical = meta_content(html, "property", "og:url")
        .or_else(|| link_rel(html, "canonical"))
        .unwrap_or_else(|| canonicalize_url(url).unwrap_or_else(|_| url.to_string()));
    let author = meta_content(html, "name", "author");
    let published_at = meta_content(html, "property", "article:published_time")
        .or_else(|| meta_content(html, "name", "date"));
    let text = strip_tags(html);
    ExtractedPage {
        url: url.into(),
        canonical_url: canonical,
        title,
        text_summary: truncate(&text, SUMMARY_LIMIT),
        text_ref: None,
        published_at,
        author,
        untrusted_content: true,
    }
}

fn capture_tag(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = lower.find(&open)?;
    let after = &html[start + open.len()..];
    let content_start = after.find('>')? + 1;
    let content = &after[content_start..];
    let end = content.to_ascii_lowercase().find(&close)?;
    Some(decode_entities(content[..end].trim()))
}

fn meta_content(html: &str, attr: &str, value: &str) -> Option<String> {
    let needle = format!("{attr}=\"{value}\"");
    let lower = html.to_ascii_lowercase();
    let idx = lower.find(&needle.to_ascii_lowercase())?;
    let window_start = idx.saturating_sub(160);
    let window_end = (idx + 220).min(html.len());
    let window = &html[window_start..window_end];
    let lower_window = window.to_ascii_lowercase();
    let key = "content=\"";
    let content_idx = lower_window.find(key)?;
    let rest = &window[content_idx + key.len()..];
    let end = rest.find('"')?;
    Some(decode_entities(&rest[..end]))
}

fn link_rel(html: &str, rel: &str) -> Option<String> {
    let needle = format!("rel=\"{rel}\"");
    let lower = html.to_ascii_lowercase();
    let idx = lower.find(&needle.to_ascii_lowercase())?;
    let window_start = idx.saturating_sub(160);
    let window_end = (idx + 220).min(html.len());
    let window = &html[window_start..window_end];
    let lower_window = window.to_ascii_lowercase();
    let key = "href=\"";
    let href_idx = lower_window.find(key)?;
    let rest = &window[href_idx + key.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let lower = html.as_bytes();
    let mut i = 0;
    while i < lower.len() {
        if !in_tag && lower[i] == b'<' {
            let rest = html[i..].to_ascii_lowercase();
            if rest.starts_with("<script") || rest.starts_with("<style") {
                in_script = true;
            }
            if rest.starts_with("</script") || rest.starts_with("</style") {
                in_script = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }
        if in_tag {
            if lower[i] == b'>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }
        if !in_script {
            out.push(html[i..].chars().next().unwrap_or('?'));
        }
        i += html[i..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    decode_entities(&out.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn truncate(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    input.chars().take(limit).collect::<String>() + "…"
}

type FakeGetResponse = (u16, String, Vec<u8>);
type HttpFetchResult = (String, u16, Option<String>, Vec<u8>, bool);

#[derive(Default)]
pub struct FakeHttpTransport {
    pub posts: Mutex<Vec<(String, Value)>>,
    pub gets: Mutex<Vec<String>>,
    post_responses: Mutex<BTreeMap<String, (u16, Value)>>,
    get_responses: Mutex<BTreeMap<String, FakeGetResponse>>,
}

impl FakeHttpTransport {
    pub fn with_post(self, endpoint: impl Into<String>, status: u16, body: Value) -> Self {
        self.post_responses
            .lock()
            .expect("fake http mutex")
            .insert(endpoint.into(), (status, body));
        self
    }

    pub fn with_get(
        self,
        url: impl Into<String>,
        status: u16,
        content_type: impl Into<String>,
        body: impl Into<Vec<u8>>,
    ) -> Self {
        self.get_responses
            .lock()
            .expect("fake http mutex")
            .insert(url.into(), (status, content_type.into(), body.into()));
        self
    }
}

impl WebHttpGateway for FakeHttpTransport {
    fn post_json(
        &self,
        endpoint: &str,
        _headers: &[(String, String)],
        body: &Value,
        _timeout: Duration,
    ) -> Result<(u16, Value), AgentError> {
        self.posts
            .lock()
            .expect("fake http mutex")
            .push((endpoint.into(), body.clone()));
        self.post_responses
            .lock()
            .expect("fake http mutex")
            .get(endpoint)
            .cloned()
            .ok_or_else(|| {
                AgentError::provider_unavailable(format!("no fake POST response for {endpoint}"))
            })
    }

    fn get_bytes(
        &self,
        url: &str,
        _headers: &[(String, String)],
        _timeout: Duration,
        max_bytes: u64,
    ) -> Result<(u16, String, Vec<u8>, Option<String>), AgentError> {
        self.gets.lock().expect("fake http mutex").push(url.into());
        let (status, content_type, mut body) = self
            .get_responses
            .lock()
            .expect("fake http mutex")
            .get(url)
            .cloned()
            .ok_or_else(|| {
                AgentError::provider_unavailable(format!("no fake GET response for {url}"))
            })?;
        let final_url = url.to_string();
        if body.len() as u64 > max_bytes {
            body.truncate(max_bytes as usize);
        }
        Ok((status, final_url, body, Some(content_type)))
    }
}

#[derive(Default)]
pub struct ReqwestHttpTransport;

impl WebHttpGateway for ReqwestHttpTransport {
    fn post_json(
        &self,
        endpoint: &str,
        headers: &[(String, String)],
        body: &Value,
        timeout: Duration,
    ) -> Result<(u16, Value), AgentError> {
        mutsuki_agent_sdk::ensure_http_crypto_provider();
        let mut request = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?
            .post(endpoint)
            .json(body);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?;
        let status = response.status().as_u16();
        let value = response
            .json()
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        Ok((status, value))
    }

    fn get_bytes(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<(u16, String, Vec<u8>, Option<String>), AgentError> {
        mutsuki_agent_sdk::ensure_http_crypto_provider();
        let mut request = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?
            .get(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut body = response
            .bytes()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?
            .to_vec();
        if body.len() as u64 > max_bytes {
            body.truncate(max_bytes as usize);
        }
        Ok((status, final_url, body, content_type))
    }
}

pub struct FakeSearchService {
    hits: Vec<SearchHit>,
}

impl FakeSearchService {
    pub fn new(hits: Vec<SearchHit>) -> Self {
        Self { hits }
    }
}

impl SearchService for FakeSearchService {
    fn search(&self, query: SearchQuery) -> Result<SearchResult, AgentError> {
        let mut hits = self.hits.clone();
        if !query.allow_domains.is_empty() {
            hits.retain(|hit| {
                Url::parse(&hit.canonical_url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .is_some_and(|host| {
                        query
                            .allow_domains
                            .iter()
                            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
                    })
            });
        }
        if !query.deny_domains.is_empty() {
            hits.retain(|hit| {
                Url::parse(&hit.canonical_url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .is_none_or(|host| {
                        !query
                            .deny_domains
                            .iter()
                            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
                    })
            });
        }
        hits.truncate(query.limit as usize);
        Ok(SearchResult {
            query,
            hits: dedup_hits(hits),
            raw_ref: None,
            provider_id: "fake".into(),
        })
    }
}

pub struct HttpJsonSearchService {
    config: SearchProviderConfig,
    transport: Arc<dyn WebHttpGateway>,
    policy: UrlAccessPolicy,
    resources: AgentResourceStore,
}

impl HttpJsonSearchService {
    pub fn new(
        config: SearchProviderConfig,
        transport: Arc<dyn WebHttpGateway>,
        resources: AgentResourceStore,
    ) -> Result<Self, AgentError> {
        if !config.enable_http {
            return Err(AgentError::provider_unavailable(
                "HTTP search transport is disabled",
            ));
        }
        if config.provider_id.trim().is_empty() || config.endpoint.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "provider_id and endpoint are required",
            ));
        }
        let policy = UrlAccessPolicy::default();
        policy.validate(&config.endpoint)?;
        Ok(Self {
            config,
            transport,
            policy,
            resources,
        })
    }
}

impl SearchService for HttpJsonSearchService {
    fn search(&self, query: SearchQuery) -> Result<SearchResult, AgentError> {
        if query.query.trim().is_empty() {
            return Err(AgentError::invalid_input("search query must not be empty"));
        }
        if query.limit == 0 {
            return Err(AgentError::invalid_input("search limit must be positive"));
        }
        let timeout = Duration::from_millis(self.config.timeout_ms.unwrap_or(10_000).max(1));
        let request = json!({
            "query": query.query,
            "locale": query.locale,
            "time_range": query.time_range,
            "allow_domains": query.allow_domains,
            "deny_domains": query.deny_domains,
            "limit": query.limit,
        });
        let (status, body) = self.transport.post_json(
            &self.config.endpoint,
            &self.config.headers,
            &request,
            timeout,
        )?;
        if !(200..300).contains(&status) {
            return Err(AgentError::provider_unavailable(format!(
                "search provider returned HTTP {status}"
            )));
        }
        let raw_ref = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.web_search.raw",
            "mutsuki.agent.web_search.raw@1",
            1,
            &body,
        )?;
        let mut hits = body
            .get("hits")
            .cloned()
            .ok_or_else(|| AgentError::invalid_input("search response missing hits"))
            .and_then(|hits| {
                serde_json::from_value::<Vec<SearchHit>>(hits)
                    .map_err(|error| AgentError::invalid_input(error.to_string()))
            })?;
        for hit in &mut hits {
            self.policy.validate(&hit.url)?;
            hit.canonical_url = canonicalize_url(&hit.url)?;
            hit.untrusted_content = true;
        }
        hits = dedup_hits(hits);
        hits.truncate(query.limit as usize);
        Ok(SearchResult {
            query,
            hits,
            raw_ref: Some(raw_ref),
            provider_id: self.config.provider_id.clone(),
        })
    }
}

pub struct HttpPageFetchService {
    transport: Arc<dyn WebHttpGateway>,
    browser: Option<Arc<dyn BrowserFetchGateway>>,
    policy: UrlAccessPolicy,
    resources: AgentResourceStore,
    enable_http: bool,
    enable_browser_fallback: bool,
}

impl HttpPageFetchService {
    pub fn new(
        transport: Arc<dyn WebHttpGateway>,
        resources: AgentResourceStore,
        enable_http: bool,
        enable_browser_fallback: bool,
        browser: Option<Arc<dyn BrowserFetchGateway>>,
    ) -> Self {
        Self {
            transport,
            browser,
            policy: UrlAccessPolicy::default(),
            resources,
            enable_http,
            enable_browser_fallback,
        }
    }

    fn fetch_http(&self, request: &PageFetchRequest) -> Result<HttpFetchResult, AgentError> {
        if !self.enable_http {
            return Err(AgentError::provider_unavailable(
                "HTTP page fetch is disabled",
            ));
        }
        let current = self.policy.validate(&request.url)?;
        let timeout = Duration::from_millis(request.timeout_ms.max(1));
        let (status, final_url, body, content_type) =
            self.transport
                .get_bytes(current.as_str(), &[], timeout, request.max_bytes)?;
        let truncated = body.len() as u64 >= request.max_bytes;
        if (300..400).contains(&status) {
            if !request.follow_redirects {
                return Err(AgentError::new(
                    "agent.web_search.redirect_denied",
                    "redirects are disabled for this fetch",
                ));
            }
            return Err(AgentError::new(
                "agent.web_search.redirect_limit",
                format!(
                    "transport returned HTTP {status}; inject a redirect-resolving transport within max_redirects={}",
                    request.max_redirects
                ),
            ));
        }
        let final_parsed = self.policy.validate(&final_url)?;
        if final_url != current.as_str()
            && !self.policy.same_registrable_origin(&current, &final_parsed)
        {
            return Err(AgentError::new(
                "agent.web_search.redirect_cross_origin",
                "cross-origin redirects are denied",
            ));
        }
        Ok((final_url, status, content_type, body, truncated))
    }
}

impl PageFetchService for HttpPageFetchService {
    fn fetch(&self, request: PageFetchRequest) -> Result<PageFetchResult, AgentError> {
        if request.max_bytes == 0 || request.timeout_ms == 0 {
            return Err(AgentError::invalid_input(
                "max_bytes and timeout_ms must be positive",
            ));
        }
        let http_result = self.fetch_http(&request);
        let (final_url, status, content_type, body, truncated, used_browser_fallback) =
            match http_result {
                Ok((final_url, status, content_type, body, truncated)) => {
                    (final_url, status, content_type, body, truncated, false)
                }
                Err(_error)
                    if request.allow_browser_fallback
                        && self.enable_browser_fallback
                        && self.browser.is_some() =>
                {
                    let browser = self.browser.as_ref().expect("checked");
                    let (final_url, content_type, body) = browser.fetch(
                        &request.url,
                        Duration::from_millis(request.timeout_ms.max(1)),
                        request.max_bytes,
                    )?;
                    let truncated = body.len() as u64 >= request.max_bytes;
                    (final_url, 200, Some(content_type), body, truncated, true)
                }
                Err(error) => return Err(error),
            };
        let canonical_url = canonicalize_url(&final_url)?;
        let text = String::from_utf8_lossy(&body).into_owned();
        let body_ref = if text.len() > INLINE_BODY_LIMIT {
            Some(self.resources.put_json(
                SERVICE_ID,
                "mutsuki.agent.web_search.page",
                "mutsuki.agent.web_search.page@1",
                1,
                &json!({"text": text}),
            )?)
        } else {
            None
        };
        Ok(PageFetchResult {
            requested_url: request.url,
            final_url,
            canonical_url,
            status,
            content_type,
            body_summary: if body_ref.is_some() {
                truncate(&text, SUMMARY_LIMIT)
            } else {
                text
            },
            body_ref,
            truncated,
            used_browser_fallback,
            untrusted_content: true,
        })
    }

    fn extract(&self, request: PageFetchRequest) -> Result<ExtractedPage, AgentError> {
        let fetched = self.fetch(request)?;
        let text = if let Some(body_ref) = &fetched.body_ref {
            let value = self.resources.read_json(body_ref)?;
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or(&fetched.body_summary)
                .to_string()
        } else {
            fetched.body_summary.clone()
        };
        let mut extracted = if fetched
            .content_type
            .as_deref()
            .is_some_and(|value| value.contains("html"))
            || text.contains('<')
        {
            extract_html(&fetched.final_url, &text)
        } else {
            ExtractedPage {
                url: fetched.final_url.clone(),
                canonical_url: fetched.canonical_url.clone(),
                title: fetched.final_url.clone(),
                text_summary: truncate(&text, SUMMARY_LIMIT),
                text_ref: None,
                published_at: None,
                author: None,
                untrusted_content: true,
            }
        };
        if text.len() > INLINE_BODY_LIMIT {
            extracted.text_ref = Some(self.resources.put_json(
                SERVICE_ID,
                "mutsuki.agent.web_search.extract",
                "mutsuki.agent.web_search.extract@1",
                1,
                &json!({"text": text}),
            )?);
        }
        extracted.canonical_url = fetched.canonical_url;
        Ok(extracted)
    }
}

pub struct SharedWebSearchService {
    descriptor: AgentServiceDescriptor,
    search: Arc<dyn SearchService>,
    pages: Arc<dyn PageFetchService>,
    resources: AgentResourceStore,
    enable_http: bool,
    enable_browser_fallback: bool,
}

impl SharedWebSearchService {
    pub fn new(
        search: Arc<dyn SearchService>,
        pages: Arc<dyn PageFetchService>,
        resources: AgentResourceStore,
        enable_http: bool,
        enable_browser_fallback: bool,
    ) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.web_search.request@1".into(),
                response_schema: "mutsuki.agent.web_search.response@1".into(),
                state: AgentPluginStateKind::Stateless,
                affinity: None,
            },
            search,
            pages,
            resources,
            enable_http,
            enable_browser_fallback,
        }
    }

    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.web_search.request@1".into(),
                response_schema: "mutsuki.agent.web_search.response@1".into(),
                state: AgentPluginStateKind::Stateless,
                affinity: None,
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_capability("network.http")
            .require_service(SERVICE_ID);
        for tool in [
            ("web.search", ToolSideEffect::ExternalRead, false),
            ("web.fetch", ToolSideEffect::ExternalRead, false),
            ("web.extract", ToolSideEffect::ExternalRead, false),
        ] {
            let mut descriptor = AgentToolDescriptor::new(
                tool.0,
                format!("mutsuki.agent.tool.{}@1", tool.0),
                format!("Execute {}", tool.0),
            );
            descriptor.side_effect = tool.1;
            descriptor.requires_approval = tool.2;
            registrar = registrar.tool(descriptor);
        }
        registrar.build()
    }

    pub fn search(&self, query: SearchQuery) -> Result<SearchResult, AgentError> {
        self.search.search(query)
    }

    pub fn fetch(&self, request: PageFetchRequest) -> Result<PageFetchResult, AgentError> {
        self.pages.fetch(request)
    }

    pub fn extract(&self, request: PageFetchRequest) -> Result<ExtractedPage, AgentError> {
        self.pages.extract(request)
    }

    pub fn cite(&self, hits: Vec<SearchHit>) -> Vec<WebCitation> {
        citations_from_hits(&hits)
    }
}

impl AgentService for SharedWebSearchService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: WebSearchServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = match request {
            WebSearchServiceRequest::Search { query } => {
                WebSearchServiceResponse::Search(self.search(query)?)
            }
            WebSearchServiceRequest::Fetch { request } => {
                WebSearchServiceResponse::Fetch(self.fetch(request)?)
            }
            WebSearchServiceRequest::Extract { request } => {
                WebSearchServiceResponse::Extract(self.extract(request)?)
            }
            WebSearchServiceRequest::Cite { hits } => WebSearchServiceResponse::Cite {
                citations: self.cite(hits),
            },
        };
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        Ok(())
    }
}

impl ToolProvider for SharedWebSearchService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        [
            ("web.search", ToolSideEffect::ExternalRead),
            ("web.fetch", ToolSideEffect::ExternalRead),
            ("web.extract", ToolSideEffect::ExternalRead),
        ]
        .into_iter()
        .map(|(name, side_effect)| {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run the {name} web search operation"),
            );
            tool.side_effect = side_effect;
            tool
        })
        .collect()
    }
}

impl ContextProvider for SharedWebSearchService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let query = request
            .input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let summary = if query.is_empty() {
            format!(
                "web-search ready http={} browser_fallback={}",
                self.enable_http, self.enable_browser_fallback
            )
        } else {
            let result = self.search(SearchQuery {
                query: query.into(),
                locale: None,
                time_range: None,
                allow_domains: Vec::new(),
                deny_domains: Vec::new(),
                limit: 3,
            })?;
            format!(
                "web-search `{}` returned {} citation(s)",
                query,
                result.hits.len()
            )
        };
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.web_search.context",
            "mutsuki.agent.web_search.context@1",
            1,
            &json!({
                "http_enabled": self.enable_http,
                "browser_fallback_enabled": self.enable_browser_fallback,
                "query": query,
            }),
        )?;
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            summary,
            details: Some(details),
            estimated_tokens: 32,
            estimated_bytes: 128,
            priority: 0,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hits() -> Vec<SearchHit> {
        vec![
            SearchHit {
                title: "Alpha".into(),
                url: "https://docs.example.com/a?utm_source=x#frag".into(),
                canonical_url: String::new(),
                snippet: Some("alpha".into()),
                published_at: Some("2026-01-01".into()),
                score: Some(1.0),
                untrusted_content: true,
            },
            SearchHit {
                title: "Alpha Dup".into(),
                url: "https://docs.example.com/a".into(),
                canonical_url: String::new(),
                snippet: Some("dup".into()),
                published_at: None,
                score: Some(0.5),
                untrusted_content: true,
            },
            SearchHit {
                title: "Beta".into(),
                url: "https://blog.example.org/b".into(),
                canonical_url: String::new(),
                snippet: Some("beta".into()),
                published_at: None,
                score: None,
                untrusted_content: true,
            },
        ]
    }

    #[test]
    fn fake_search_dedups_canonical_urls_and_builds_citations() {
        let mut hits = sample_hits();
        for hit in &mut hits {
            hit.canonical_url = canonicalize_url(&hit.url).unwrap();
        }
        let service = FakeSearchService::new(hits);
        let result = service
            .search(SearchQuery {
                query: "mutsuki".into(),
                locale: Some("zh-CN".into()),
                time_range: None,
                allow_domains: vec!["example.com".into()],
                deny_domains: Vec::new(),
                limit: 10,
            })
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].canonical_url, "https://docs.example.com/a");
        let citations = citations_from_hits(&result.hits);
        assert_eq!(citations.len(), 1);
        assert!(citations[0].untrusted_content);
    }

    #[test]
    fn url_policy_blocks_ssrf_targets() {
        let policy = UrlAccessPolicy::default();
        assert_eq!(
            policy.validate("http://127.0.0.1/secret").unwrap_err().code,
            "agent.web_search.ssrf_denied"
        );
        assert_eq!(
            policy
                .validate("http://169.254.169.254/latest/meta-data")
                .unwrap_err()
                .code,
            "agent.web_search.ssrf_denied"
        );
        assert_eq!(
            policy.validate("file:///etc/passwd").unwrap_err().code,
            "agent.web_search.scheme_denied"
        );
    }

    #[test]
    fn http_json_search_and_page_extract_use_resource_refs() {
        let resources = AgentResourceStore::default();
        let large = "x".repeat(INLINE_BODY_LIMIT + 64);
        let html = format!(
            "<html><head><title>Guide</title><meta name=\"author\" content=\"Ada\"><link rel=\"canonical\" href=\"https://docs.example.com/guide\"></head><body><p>{large}</p></body></html>"
        );
        let transport = Arc::new(
            FakeHttpTransport::default()
                .with_post(
                    "https://search.example/v1",
                    200,
                    json!({
                        "hits": [{
                            "title": "Guide",
                            "url": "https://docs.example.com/guide?utm_campaign=1",
                            "canonical_url": "",
                            "snippet": "hello",
                            "untrusted_content": true
                        }]
                    }),
                )
                .with_get(
                    "https://docs.example.com/guide",
                    200,
                    "text/html",
                    html.into_bytes(),
                ),
        );
        let search = HttpJsonSearchService::new(
            SearchProviderConfig {
                provider_id: "generic-json".into(),
                endpoint: "https://search.example/v1".into(),
                headers: vec![("x-test".into(), "1".into())],
                credential_env: None,
                timeout_ms: Some(1_000),
                enable_http: true,
                enable_browser_fallback: false,
            },
            transport.clone(),
            resources.clone(),
        )
        .unwrap();
        let pages = HttpPageFetchService::new(transport, resources.clone(), true, false, None);
        let shared =
            SharedWebSearchService::new(Arc::new(search), Arc::new(pages), resources, true, false);
        let result = shared
            .search(SearchQuery {
                query: "guide".into(),
                locale: None,
                time_range: None,
                allow_domains: Vec::new(),
                deny_domains: Vec::new(),
                limit: 5,
            })
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert!(result.raw_ref.is_some());
        assert_eq!(
            result.hits[0].canonical_url,
            "https://docs.example.com/guide"
        );
        let extracted = shared
            .extract(PageFetchRequest {
                url: "https://docs.example.com/guide".into(),
                follow_redirects: true,
                max_redirects: 2,
                max_bytes: 64 * 1024,
                timeout_ms: 1_000,
                allow_browser_fallback: false,
            })
            .unwrap();
        assert_eq!(extracted.title, "Guide");
        assert_eq!(extracted.author.as_deref(), Some("Ada"));
        assert!(extracted.text_ref.is_some());
        assert!(extracted.untrusted_content);
        let descriptor = SharedWebSearchService::plugin_descriptor(3).unwrap();
        assert_eq!(descriptor.plugin_id, PLUGIN_ID);
        assert_eq!(descriptor.tools.len(), 3);
    }

    #[test]
    fn http_and_browser_fallback_can_be_disabled_independently() {
        let resources = AgentResourceStore::default();
        let transport = Arc::new(FakeHttpTransport::default());
        let pages = HttpPageFetchService::new(transport, resources.clone(), false, false, None);
        let err = pages
            .fetch(PageFetchRequest {
                url: "https://docs.example.com/guide".into(),
                follow_redirects: true,
                max_redirects: 1,
                max_bytes: 1024,
                timeout_ms: 100,
                allow_browser_fallback: true,
            })
            .unwrap_err();
        assert_eq!(err.code, "agent.provider_unavailable");
        let _ = SharedWebSearchService::new(
            Arc::new(FakeSearchService::new(Vec::new())),
            Arc::new(pages),
            resources,
            false,
            false,
        );
    }

    #[test]
    fn performance_smoke_search_and_extract() {
        use std::time::Instant;
        let resources = AgentResourceStore::default();
        let html = "<html><head><title>Perf</title></head><body>ok</body></html>";
        let transport = Arc::new(
            FakeHttpTransport::default()
                .with_post(
                    "https://search.example/v1",
                    200,
                    json!({"hits": [{
                        "title": "Perf",
                        "url": "https://docs.example.com/perf",
                        "canonical_url": "",
                        "snippet": "ok",
                        "untrusted_content": true
                    }]}),
                )
                .with_get(
                    "https://docs.example.com/perf",
                    200,
                    "text/html",
                    html.as_bytes().to_vec(),
                ),
        );
        let search = HttpJsonSearchService::new(
            SearchProviderConfig {
                provider_id: "generic-json".into(),
                endpoint: "https://search.example/v1".into(),
                headers: Vec::new(),
                credential_env: None,
                timeout_ms: Some(1_000),
                enable_http: true,
                enable_browser_fallback: false,
            },
            transport.clone(),
            resources.clone(),
        )
        .unwrap();
        let pages = HttpPageFetchService::new(transport, resources.clone(), true, false, None);
        let shared =
            SharedWebSearchService::new(Arc::new(search), Arc::new(pages), resources, true, false);
        let started = Instant::now();
        for index in 0..50 {
            let result = shared
                .search(SearchQuery {
                    query: format!("perf-{index}"),
                    locale: None,
                    time_range: None,
                    allow_domains: Vec::new(),
                    deny_domains: Vec::new(),
                    limit: 3,
                })
                .unwrap();
            let _ = shared
                .extract(PageFetchRequest {
                    url: result.hits[0].url.clone(),
                    follow_redirects: true,
                    max_redirects: 1,
                    max_bytes: 64 * 1024,
                    timeout_ms: 1_000,
                    allow_browser_fallback: false,
                })
                .unwrap();
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed.as_millis() < 5_000,
            "web-search smoke too slow: {elapsed:?}"
        );
    }
}
