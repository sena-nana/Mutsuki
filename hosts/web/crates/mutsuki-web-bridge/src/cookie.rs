//! Browser login sessions carried by an `HttpOnly` cookie.
//!
//! `POST /auth/session` trades the console token for an opaque random id once; the browser
//! then reconnects with the cookie alone, so the token never sits in page memory or storage
//! and a reload does not ask for it again. Ids live only in this process and expire after an
//! idle period.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::AuthGrant;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "mutsuki_session";

/// Lifetime and capacity of cookie sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieSessionPolicy {
    /// A session unused for this long is gone; every successful use renews it.
    pub idle_ttl: Duration,
    /// Live sessions kept at most; the least recently used one is evicted first.
    pub max_sessions: usize,
}

impl Default for CookieSessionPolicy {
    fn default() -> Self {
        Self {
            idle_ttl: Duration::from_hours(12),
            max_sessions: 64,
        }
    }
}

/// A freshly issued cookie session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCookie {
    /// Opaque cookie value. Not derived from the token.
    pub value: String,
    pub max_age: Duration,
}

struct Entry {
    grant: AuthGrant,
    last_used: Instant,
}

/// In-memory cookie session table.
pub struct CookieSessions {
    policy: CookieSessionPolicy,
    entries: Mutex<HashMap<String, Entry>>,
}

impl CookieSessions {
    pub fn new(policy: CookieSessionPolicy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn policy(&self) -> CookieSessionPolicy {
        self.policy
    }

    /// Stores `grant` under a new random id.
    ///
    /// # Panics
    ///
    /// Panics if the session table lock is poisoned.
    pub fn issue(&self, grant: AuthGrant) -> IssuedCookie {
        self.issue_at(grant, Instant::now())
    }

    fn issue_at(&self, grant: AuthGrant, now: Instant) -> IssuedCookie {
        // Two v4 UUIDs: 244 random bits, hex so it is a valid cookie value as-is.
        let value = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut entries = self.entries.lock().unwrap();
        self.prune(&mut entries, now);
        while entries.len() >= self.policy.max_sessions {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        entries.insert(
            value.clone(),
            Entry {
                grant,
                last_used: now,
            },
        );
        IssuedCookie {
            value,
            max_age: self.policy.idle_ttl,
        }
    }

    /// The grant behind a live cookie, renewing its idle timer.
    ///
    /// # Panics
    ///
    /// Panics if the session table lock is poisoned.
    pub fn resolve(&self, value: &str) -> Option<AuthGrant> {
        self.resolve_at(value, Instant::now())
    }

    fn resolve_at(&self, value: &str, now: Instant) -> Option<AuthGrant> {
        let mut entries = self.entries.lock().unwrap();
        self.prune(&mut entries, now);
        let entry = entries.get_mut(value)?;
        entry.last_used = now;
        Some(entry.grant.clone())
    }

    /// Ends one session. Unknown values are ignored.
    ///
    /// # Panics
    ///
    /// Panics if the session table lock is poisoned.
    pub fn revoke(&self, value: &str) {
        self.entries.lock().unwrap().remove(value);
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn prune(&self, entries: &mut HashMap<String, Entry>, now: Instant) {
        let ttl = self.policy.idle_ttl;
        entries.retain(|_, entry| now.saturating_duration_since(entry.last_used) < ttl);
    }
}

/// The session cookie value from a `Cookie` request header, if present.
pub fn session_cookie_value(cookie_header: &str) -> Option<&str> {
    cookie_header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> AuthGrant {
        AuthGrant {
            principal_id: "local-web-console".into(),
            capabilities: vec!["runtime.read".into()],
        }
    }

    #[test]
    fn issued_cookie_resolves_until_idle_ttl_and_renews_on_use() {
        let sessions = CookieSessions::new(CookieSessionPolicy {
            idle_ttl: Duration::from_secs(10),
            max_sessions: 4,
        });
        let start = Instant::now();
        let cookie = sessions.issue_at(grant(), start);
        assert_eq!(cookie.value.len(), 64);
        assert!(
            sessions
                .resolve_at(&cookie.value, start + Duration::from_secs(9))
                .is_some()
        );
        // The use at +9s renewed it.
        assert!(
            sessions
                .resolve_at(&cookie.value, start + Duration::from_secs(18))
                .is_some()
        );
        assert!(
            sessions
                .resolve_at(&cookie.value, start + Duration::from_secs(29))
                .is_none()
        );
        assert!(sessions.is_empty());
    }

    #[test]
    fn capacity_evicts_least_recently_used_and_revoke_ends_session() {
        let sessions = CookieSessions::new(CookieSessionPolicy {
            idle_ttl: Duration::from_secs(60),
            max_sessions: 2,
        });
        let start = Instant::now();
        let first = sessions.issue_at(grant(), start);
        let second = sessions.issue_at(grant(), start + Duration::from_secs(1));
        assert!(
            sessions
                .resolve_at(&first.value, start + Duration::from_secs(2))
                .is_some()
        );
        let third = sessions.issue_at(grant(), start + Duration::from_secs(3));
        assert_eq!(sessions.len(), 2);
        assert!(sessions.resolve(&second.value).is_none());
        sessions.revoke(&first.value);
        assert!(sessions.resolve(&first.value).is_none());
        assert!(sessions.resolve(&third.value).is_some());
    }

    #[test]
    fn parses_session_cookie_from_header() {
        assert_eq!(
            session_cookie_value("theme=dark; mutsuki_session=abc123 ; other=1"),
            Some("abc123")
        );
        assert_eq!(session_cookie_value("mutsuki_session="), None);
        assert_eq!(session_cookie_value("theme=dark"), None);
    }
}
