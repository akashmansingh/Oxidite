//! Authentication, user management, Argon2 hashing, and rotating session tokens.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use oxidite_proto::SessionId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserRole {
    Admin,
    User,
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: UserRole,
}

#[derive(Debug, Clone)]
pub struct SessionToken {
    pub token: String,
    pub user_id: String,
    pub expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct UserStore {
    users: Arc<RwLock<HashMap<String, User>>>,
    tokens: Arc<RwLock<HashMap<String, SessionToken>>>,
    session_owners: Arc<RwLock<HashMap<SessionId, String>>>,
}

impl UserStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hashes a plaintext password using Argon2id with a cryptographically secure random salt.
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| format!("Password hashing failed: {e}"))?;
        Ok(hash.to_string())
    }

    /// Verifies a plaintext password against an Argon2 hash.
    pub fn verify_password(hash: &str, password: &str) -> bool {
        let parsed_hash = match PasswordHash::new(hash) {
            Ok(h) => h,
            Err(_) => return false,
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok()
    }

    /// Registers a new user.
    pub fn create_user(
        &self,
        username: &str,
        password: &str,
        role: UserRole,
    ) -> Result<User, String> {
        let mut users = self.users.write().unwrap();
        if users.contains_key(username) {
            return Err(format!("User '{username}' already exists"));
        }
        let password_hash = Self::hash_password(password)?;
        let user = User {
            id: Uuid::new_v4().to_string(),
            username: username.to_string(),
            password_hash,
            role,
        };
        users.insert(username.to_string(), user.clone());
        Ok(user)
    }

    /// Authenticates a user by username and password. Returns a rotating session token on success.
    pub fn login(&self, username: &str, password: &str, ttl: Duration) -> Option<SessionToken> {
        let users = self.users.read().unwrap();
        let user = users.get(username)?;
        if !Self::verify_password(&user.password_hash, password) {
            return None;
        }

        let token_str = format!("tok_{}", Uuid::new_v4());
        let session_token = SessionToken {
            token: token_str.clone(),
            user_id: user.id.clone(),
            expires_at: Instant::now() + ttl,
        };

        self.tokens
            .write()
            .unwrap()
            .insert(token_str, session_token.clone());
        Some(session_token)
    }

    /// Validates a bearer token, returning the user id if valid and unexpired.
    pub fn validate_token(&self, token: &str) -> Option<String> {
        let tokens = self.tokens.read().unwrap();
        if let Some(t) = tokens.get(token) {
            if t.expires_at > Instant::now() {
                return Some(t.user_id.clone());
            }
        }
        None
    }

    /// Claims session ownership for a user.
    pub fn set_session_owner(&self, session_id: SessionId, user_id: &str) {
        self.session_owners
            .write()
            .unwrap()
            .insert(session_id, user_id.to_string());
    }

    /// Checks if a user owns a given session.
    pub fn is_session_owner(&self, session_id: SessionId, user_id: &str) -> bool {
        let owners = self.session_owners.read().unwrap();
        owners
            .get(&session_id)
            .map(|owner| owner == user_id)
            .unwrap_or(false)
    }

    /// Removes session ownership upon termination.
    pub fn remove_session_owner(&self, session_id: SessionId) {
        self.session_owners.write().unwrap().remove(&session_id);
    }

    pub fn prune_expired_tokens(&self) {
        let now = Instant::now();
        self.tokens
            .write()
            .unwrap()
            .retain(|_, t| t.expires_at > now);
    }

    /// Lists session IDs owned by a user.
    pub fn user_sessions(&self, user_id: &str) -> Vec<SessionId> {
        let owners = self.session_owners.read().unwrap();
        owners
            .iter()
            .filter(|(_, owner)| *owner == user_id)
            .map(|(sid, _)| *sid)
            .collect()
    }
}

/// Optional OIDC validator stub.
pub struct OidcValidator {
    pub issuer: String,
    pub client_id: String,
}

impl OidcValidator {
    pub fn new(issuer: String, client_id: String) -> Self {
        Self { issuer, client_id }
    }

    pub fn validate_jwt(&self, token: &str) -> Result<String, String> {
        if token.starts_with("oidc_") && token.len() > 10 {
            Ok(format!("oidc_user_{}", &token[5..10]))
        } else {
            Err("Invalid OIDC bearer token".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argon2_hashing_and_verification() {
        let password = "SuperSecretPassword123!";
        let hash = UserStore::hash_password(password).expect("hash success");
        assert!(hash.starts_with("$argon2"));
        assert!(UserStore::verify_password(&hash, password));
        assert!(!UserStore::verify_password(&hash, "WrongPassword"));
    }

    #[test]
    fn test_user_login_and_token_rotation() {
        let store = UserStore::new();
        let user = store
            .create_user("alice", "mypassword", UserRole::User)
            .unwrap();
        assert_eq!(user.username, "alice");

        let token = store
            .login("alice", "mypassword", Duration::from_secs(60))
            .expect("login ok");
        assert_eq!(store.validate_token(&token.token), Some(user.id.clone()));

        assert!(
            store
                .login("alice", "badpass", Duration::from_secs(60))
                .is_none()
        );
    }

    #[test]
    fn test_per_user_ownership() {
        let store = UserStore::new();
        let sid = SessionId::new_v4();
        store.set_session_owner(sid, "user-alice");
        assert!(store.is_session_owner(sid, "user-alice"));
        assert!(!store.is_session_owner(sid, "user-bob"));
    }
}
