//! Scoped, expiring, revocable share tokens for collaborative sessions.

use oxidite_proto::{AttachMode, SessionId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ShareToken {
    pub token: String,
    pub session_id: SessionId,
    pub mode: AttachMode,
    pub expires_at: Instant,
    pub revoked: bool,
}

#[derive(Clone, Default)]
pub struct ShareStore {
    shares: Arc<RwLock<HashMap<String, ShareToken>>>,
}

impl ShareStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new scoped share token.
    pub fn create_share(
        &self,
        session_id: SessionId,
        mode: AttachMode,
        ttl: Duration,
    ) -> ShareToken {
        let token = format!("share_{}", Uuid::new_v4());
        let share = ShareToken {
            token: token.clone(),
            session_id,
            mode,
            expires_at: Instant::now() + ttl,
            revoked: false,
        };

        self.shares.write().unwrap().insert(token, share.clone());
        share
    }

    /// Validates a share token. Returns (SessionId, AttachMode) if valid, active, and unexpired.
    pub fn validate_share(&self, token: &str) -> Option<(SessionId, AttachMode)> {
        let shares = self.shares.read().unwrap();
        if let Some(share) = shares.get(token) {
            if !share.revoked && share.expires_at > Instant::now() {
                return Some((share.session_id, share.mode));
            }
        }
        None
    }

    /// Revokes a share token immediately.
    pub fn revoke_share(&self, token: &str) -> bool {
        let mut shares = self.shares.write().unwrap();
        if let Some(share) = shares.get_mut(token) {
            share.revoked = true;
            true
        } else {
            false
        }
    }

    /// Lists active share tokens for a given session.
    pub fn session_shares(&self, session_id: SessionId) -> Vec<ShareToken> {
        let shares = self.shares.read().unwrap();
        let now = Instant::now();
        shares
            .values()
            .filter(|s| s.session_id == session_id && !s.revoked && s.expires_at > now)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_token_lifecycle() {
        let store = ShareStore::new();
        let sid = SessionId::new_v4();

        let share = store.create_share(sid, AttachMode::Viewer, Duration::from_secs(60));
        assert!(share.token.starts_with("share_"));
        assert_eq!(share.mode, AttachMode::Viewer);

        // Valid token
        let validated = store.validate_share(&share.token);
        assert_eq!(validated, Some((sid, AttachMode::Viewer)));

        // Revoke token
        assert!(store.revoke_share(&share.token));
        assert_eq!(store.validate_share(&share.token), None);
    }
}
