//! Durable device enrollment, authentication, and revocation backed by SQLite.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DeviceAuthError {
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Invalid or expired enrollment grant")]
    InvalidGrant,
    #[error("Device not found")]
    NotFound,
    #[error("Database error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub user_id: String,
    pub device_id: Uuid,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: Uuid,
    pub user_id: String,
    pub label: String,
    pub created_at_secs: u64,
    pub last_seen_at_secs: Option<u64>,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentResult {
    pub device_id: Uuid,
    pub user_id: String,
    pub token: String,
    pub label: String,
}

struct EnrollmentGrant {
    user_id: String,
    expires_at: Option<Instant>,
}

#[derive(Clone)]
pub struct DeviceStore {
    pool: SqlitePool,
    grants: Arc<Mutex<HashMap<String, EnrollmentGrant>>>,
}

impl Default for DeviceStore {
    fn default() -> Self {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy("sqlite::memory:")
            .expect("in-memory sqlite pool");
        Self {
            pool,
            grants: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DeviceStore {
    /// Opens an in-memory SQLite store for testing or loopback defaults.
    pub async fn open_in_memory() -> Result<Self, DeviceAuthError> {
        let store = Self::default();
        store.ensure_migrated().await?;
        Ok(store)
    }

    /// Opens or creates a durable SQLite store at `db_path`.
    pub async fn open(db_path: &str) -> Result<Self, DeviceAuthError> {
        let connection_url = if db_path.starts_with("sqlite:") {
            db_path.to_string()
        } else {
            format!("sqlite:{db_path}?mode=rwc")
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_url)
            .await
            .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let store = Self {
            pool,
            grants: Arc::new(Mutex::new(HashMap::new())),
        };
        store.ensure_migrated().await?;
        Ok(store)
    }

    pub async fn ensure_migrated(&self) -> Result<(), DeviceAuthError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS devices (
                device_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                secret_hash TEXT NOT NULL,
                label TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_seen_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_devices_user ON devices(user_id);",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        Ok(())
    }

    /// Registers the bootstrap token as a single-use enrollment grant.
    pub fn register_bootstrap_grant(&self, token: &str, user_id: &str) {
        if !token.is_empty() {
            let mut grants = self.grants.lock().unwrap();
            grants.insert(
                token.to_string(),
                EnrollmentGrant {
                    user_id: user_id.to_string(),
                    expires_at: None,
                },
            );
        }
    }

    /// Issues a short-lived, single-use enrollment grant for a user.
    pub async fn issue_enrollment_grant(
        &self,
        user_id: &str,
        ttl: Duration,
    ) -> Result<String, DeviceAuthError> {
        let grant_token = format!("eg_{}_{}", Uuid::new_v4(), Uuid::new_v4());
        let expires_at = Instant::now() + ttl;

        let mut grants = self.grants.lock().unwrap();
        grants.insert(
            grant_token.clone(),
            EnrollmentGrant {
                user_id: user_id.to_string(),
                expires_at: Some(expires_at),
            },
        );

        Ok(grant_token)
    }

    /// Enrolls a new device using a single-use enrollment grant.
    pub async fn enroll(
        &self,
        grant: &str,
        label: &str,
    ) -> Result<EnrollmentResult, DeviceAuthError> {
        self.ensure_migrated().await?;
        let grant_info = {
            let mut grants = self.grants.lock().unwrap();
            grants.remove(grant)
        };

        let grant_info = grant_info.ok_or(DeviceAuthError::InvalidGrant)?;
        if let Some(expires_at) = grant_info.expires_at {
            if Instant::now() > expires_at {
                return Err(DeviceAuthError::InvalidGrant);
            }
        }

        let user_id = grant_info.user_id;

        // Generate 256 bits of CSPRNG entropy
        let mut raw_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut raw_bytes);
        let secret: String = raw_bytes.iter().map(|b| format!("{b:02x}")).collect();

        // Hash with Argon2id
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let secret_hash = argon2
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|e| DeviceAuthError::Storage(format!("Hash failed: {e}")))?
            .to_string();

        let device_id = Uuid::new_v4();
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let clean_label = if label.trim().is_empty() {
            "Default Device"
        } else {
            label.trim()
        };

        sqlx::query(
            "INSERT INTO devices (device_id, user_id, secret_hash, label, created_at, last_seen_at, revoked)
             VALUES (?, ?, ?, ?, ?, NULL, 0)",
        )
        .bind(device_id.to_string())
        .bind(&user_id)
        .bind(&secret_hash)
        .bind(clean_label)
        .bind(now_secs)
        .execute(&self.pool)
        .await
        .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let token = format!("od1.{device_id}.{secret}");

        Ok(EnrollmentResult {
            device_id,
            user_id,
            token,
            label: clean_label.to_string(),
        })
    }

    /// Authenticates an opaque `od1.<uuid>.<secret>` token in constant time.
    pub async fn authenticate(&self, token: &str) -> Result<Principal, DeviceAuthError> {
        self.ensure_migrated().await?;
        // Pre-computed Argon2id dummy hash to equalize timing on invalid / missing device IDs
        const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$eWF6aTB5YXppMHlhemkweQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 || parts[0] != "od1" {
            return Err(DeviceAuthError::Unauthorized);
        }

        let device_id = match Uuid::parse_str(parts[1]) {
            Ok(id) => id,
            Err(_) => return Err(DeviceAuthError::Unauthorized),
        };

        let raw_secret = parts[2];
        if raw_secret.len() < 32 {
            return Err(DeviceAuthError::Unauthorized);
        }

        let row =
            sqlx::query("SELECT user_id, secret_hash, revoked FROM devices WHERE device_id = ?")
                .bind(device_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let (user_id, secret_hash, revoked) = match row {
            Some(r) => {
                let user_id: String = r
                    .try_get("user_id")
                    .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
                let secret_hash: String = r
                    .try_get("secret_hash")
                    .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
                let revoked: i64 = r
                    .try_get("revoked")
                    .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
                (user_id, secret_hash, revoked != 0)
            }
            None => {
                // Execute dummy password verification to protect against timing side-channels
                if let Ok(dummy) = PasswordHash::new(DUMMY_HASH) {
                    let _ = Argon2::default().verify_password(raw_secret.as_bytes(), &dummy);
                }
                return Err(DeviceAuthError::Unauthorized);
            }
        };

        if revoked {
            // Also perform dummy verification
            if let Ok(dummy) = PasswordHash::new(DUMMY_HASH) {
                let _ = Argon2::default().verify_password(raw_secret.as_bytes(), &dummy);
            }
            return Err(DeviceAuthError::Unauthorized);
        }

        let parsed_hash = match PasswordHash::new(&secret_hash) {
            Ok(h) => h,
            Err(_) => return Err(DeviceAuthError::Unauthorized),
        };

        if Argon2::default()
            .verify_password(raw_secret.as_bytes(), &parsed_hash)
            .is_err()
        {
            return Err(DeviceAuthError::Unauthorized);
        }

        // Update last_seen_at
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let _ = sqlx::query("UPDATE devices SET last_seen_at = ? WHERE device_id = ?")
            .bind(now_secs)
            .bind(device_id.to_string())
            .execute(&self.pool)
            .await;

        Ok(Principal {
            user_id,
            device_id,
            role: "user".to_string(),
        })
    }

    /// Revokes a device. Ensures user isolation (cannot revoke another user's device).
    pub async fn revoke(&self, user_id: &str, device_id: Uuid) -> Result<(), DeviceAuthError> {
        self.ensure_migrated().await?;
        let row = sqlx::query("SELECT user_id FROM devices WHERE device_id = ?")
            .bind(device_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let owner: String = match row {
            Some(r) => r
                .try_get("user_id")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?,
            None => return Err(DeviceAuthError::NotFound),
        };

        if owner != user_id {
            return Err(DeviceAuthError::Unauthorized);
        }

        sqlx::query("UPDATE devices SET revoked = 1 WHERE device_id = ?")
            .bind(device_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        Ok(())
    }

    /// Checks whether a device is marked as revoked or not found in the durable database.
    pub async fn is_revoked(&self, device_id: Uuid) -> Result<bool, DeviceAuthError> {
        self.ensure_migrated().await?;
        let row = sqlx::query("SELECT revoked FROM devices WHERE device_id = ?")
            .bind(device_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        match row {
            Some(r) => {
                let revoked: i64 = r
                    .try_get("revoked")
                    .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
                Ok(revoked != 0)
            }
            None => Ok(true), // Missing devices are treated as revoked
        }
    }

    /// Lists all active (non-revoked) devices for a user.
    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<DeviceInfo>, DeviceAuthError> {
        self.ensure_migrated().await?;
        let rows = sqlx::query(
            "SELECT device_id, user_id, label, created_at, last_seen_at, revoked
             FROM devices
             WHERE user_id = ? AND revoked = 0
             ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let mut devices = Vec::with_capacity(rows.len());
        for r in rows {
            let id_str: String = r
                .try_get("device_id")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
            let device_id = match Uuid::parse_str(&id_str) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let u_id: String = r
                .try_get("user_id")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
            let label: String = r
                .try_get("label")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
            let created_at: i64 = r
                .try_get("created_at")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
            let last_seen: Option<i64> = r
                .try_get("last_seen_at")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;
            let revoked: i64 = r
                .try_get("revoked")
                .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

            devices.push(DeviceInfo {
                device_id,
                user_id: u_id,
                label,
                created_at_secs: created_at.max(0) as u64,
                last_seen_at_secs: last_seen.map(|s| s.max(0) as u64),
                revoked: revoked != 0,
            });
        }

        Ok(devices)
    }

    /// Testing helper to inspect stored hashes and confirm no plaintext secrets exist.
    pub async fn debug_get_all_hashes_for_test(&self) -> Result<Vec<String>, DeviceAuthError> {
        self.ensure_migrated().await?;
        let rows = sqlx::query("SELECT secret_hash FROM devices")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| DeviceAuthError::Storage(e.to_string()))?;

        let hashes = rows
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>("secret_hash").ok())
            .collect();
        Ok(hashes)
    }
}
