//! The nexus's SQLite database: the account, devices, sessions, pairing
//! requests, workspaces (with their last state), push subscriptions and the
//! audit log. Tokens are kept only as SHA-256 hashes.
//!
//! The schema grows by numbered migrations (`MIGRATIONS`, recorded in
//! `PRAGMA user_version`); a database from an older nexus is brought up to
//! date when it opens. Calls are short and run under one lock.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use shed_api::http::{AuditEntry, DeviceKind, PushSubscribe};
use shed_api::wire::{NotificationKind, Policy};

/// Each entry moves the schema one version up; never edit a released one.
const MIGRATIONS: &[&str] = &[
    // 1: the first schema.
    "CREATE TABLE account (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        username TEXT NOT NULL,
        password TEXT NOT NULL,
        totp_secret TEXT NOT NULL,
        totp_step INTEGER NOT NULL DEFAULT 0,
        created INTEGER NOT NULL
    );
    CREATE TABLE recovery_codes (
        hash TEXT PRIMARY KEY,
        used INTEGER
    );
    CREATE TABLE devices (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        token_hash TEXT UNIQUE,
        created INTEGER NOT NULL,
        seen INTEGER NOT NULL,
        address TEXT NOT NULL DEFAULT '',
        country TEXT,
        revoked INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        csrf TEXT NOT NULL,
        device TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
        expires INTEGER NOT NULL
    );
    CREATE TABLE pairings (
        code TEXT PRIMARY KEY,
        poll_hash TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        address TEXT NOT NULL,
        country TEXT,
        expires INTEGER NOT NULL,
        state TEXT NOT NULL,
        approved_name TEXT
    );
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
        policy TEXT NOT NULL DEFAULT 'full',
        version TEXT NOT NULL DEFAULT '',
        state TEXT,
        state_at INTEGER
    );
    CREATE TABLE push (
        device TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
        endpoint TEXT,
        p256dh TEXT,
        auth TEXT,
        kinds TEXT NOT NULL
    );
    CREATE TABLE audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        what TEXT NOT NULL,
        device TEXT,
        address TEXT NOT NULL DEFAULT '',
        country TEXT,
        detail TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX audit_at ON audit(at);",
    // 2: device tokens that end (desktops', some time after signing in).
    "ALTER TABLE devices ADD COLUMN expires INTEGER;",
];

pub struct Store {
    conn: Mutex<Connection>,
}

/// The one account.
#[derive(Clone, Debug)]
pub struct Account {
    pub username: String,
    /// argon2, PHC string.
    pub password: String,
    /// Base32.
    pub totp_secret: String,
    /// The last TOTP time step used, so a code is not taken twice.
    pub totp_step: u64,
}

#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub id: String,
    pub kind: DeviceKind,
    pub name: String,
    pub created: u64,
    pub seen: u64,
    pub address: String,
    pub country: Option<String>,
    pub revoked: bool,
    /// When its token stops working (Unix seconds); `None`: until revoked.
    pub expires: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub device: String,
    pub csrf: String,
    pub expires: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingState {
    Pending,
    /// Approved under this name; the token goes out on the next poll.
    Approved(String),
    Rejected,
}

#[derive(Clone, Debug)]
pub struct Pairing {
    /// Names it in lists (the start of its poll secret's hash): not the code,
    /// which only the workspace shows.
    pub id: String,
    pub code: String,
    pub name: String,
    pub version: String,
    pub address: String,
    pub country: Option<String>,
    pub expires: u64,
    pub state: PairingState,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub policy: Policy,
    pub version: String,
    pub seen: u64,
    /// The last state it sent, as JSON.
    pub state: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Subscription {
    pub device: String,
    /// The device's kind: a browser, or a phone (UnifiedPush).
    pub kind: DeviceKind,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub kinds: Vec<NotificationKind>,
}

/// What an audit entry records besides its kind.
#[derive(Clone, Debug, Default)]
pub struct AuditInfo<'a> {
    pub device: Option<&'a str>,
    pub address: &'a str,
    pub country: Option<&'a str>,
    pub detail: &'a str,
}

/// Every kind, the default for a device that has not chosen.
pub fn all_kinds() -> Vec<NotificationKind> {
    vec![
        NotificationKind::AiWaiting,
        NotificationKind::AiDone,
        NotificationKind::CommandDone,
        NotificationKind::WorkspaceOffline,
    ]
}

impl Store {
    /// Opens (or makes) the database at `path` and migrates it.
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let conn = Connection::open(path)?;
        Self::with(conn)
    }

    /// A database in memory, for tests.
    pub fn in_memory() -> anyhow::Result<Store> {
        Self::with(Connection::open_in_memory()?)
    }

    fn with(conn: Connection) -> anyhow::Result<Store> {
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Store { conn: Mutex::new(conn) };
        store.migrate()?;
        Ok(store)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let mut conn = self.lock();
        let version: usize = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
        if version > MIGRATIONS.len() {
            anyhow::bail!("the database is from a newer nexus (schema {version})");
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------- account

    pub fn account(&self) -> anyhow::Result<Option<Account>> {
        Ok(self
            .lock()
            .query_row("SELECT username, password, totp_secret, totp_step FROM account WHERE id = 1", [], |r| {
                Ok(Account {
                    username: r.get(0)?,
                    password: r.get(1)?,
                    totp_secret: r.get(2)?,
                    totp_step: r.get::<_, i64>(3)? as u64,
                })
            })
            .optional()?)
    }

    /// Makes the account, with the hashes of its recovery codes. Fails if
    /// there is one already.
    pub fn create_account(&self, account: &Account, recovery: &[String], now: u64) -> anyhow::Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO account (id, username, password, totp_secret, totp_step, created) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![account.username, account.password, account.totp_secret, account.totp_step as i64, now as i64],
        )?;
        for hash in recovery {
            tx.execute("INSERT INTO recovery_codes (hash) VALUES (?1)", params![hash])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records that TOTP step `step` was used; false if it (or a later one)
    /// already was.
    pub fn use_totp_step(&self, step: u64) -> anyhow::Result<bool> {
        let changed = self.lock().execute(
            "UPDATE account SET totp_step = ?1 WHERE id = 1 AND totp_step < ?1",
            params![step as i64],
        )?;
        Ok(changed == 1)
    }

    pub fn set_password(&self, hash: &str) -> anyhow::Result<()> {
        self.lock().execute("UPDATE account SET password = ?1 WHERE id = 1", params![hash])?;
        Ok(())
    }

    /// Uses up a recovery code by its hash; false if there is no such unused one.
    pub fn use_recovery_code(&self, hash: &str, now: u64) -> anyhow::Result<bool> {
        let changed = self.lock().execute(
            "UPDATE recovery_codes SET used = ?2 WHERE hash = ?1 AND used IS NULL",
            params![hash, now as i64],
        )?;
        Ok(changed == 1)
    }

    // ------------------------------------------------------------- devices

    /// Adds a device; `token_hash` for phones and workspaces.
    pub fn add_device(
        &self,
        kind: DeviceKind,
        name: &str,
        token_hash: Option<&str>,
        address: &str,
        country: Option<&str>,
        now: u64,
    ) -> anyhow::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        self.lock().execute(
            "INSERT INTO devices (id, kind, name, token_hash, created, seen, address, country)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
            params![id, kind_str(kind), name, token_hash, now as i64, address, country],
        )?;
        Ok(id)
    }

    /// The device a token belongs to, if it is not revoked and has not
    /// expired by `now`.
    pub fn device_by_token(&self, token_hash: &str, now: u64) -> anyhow::Result<Option<DeviceRow>> {
        Ok(self.device_where("token_hash = ?1 AND revoked = 0", token_hash)?.filter(|d| d.expires.is_none_or(|e| e > now)))
    }

    /// Sets when a device's token stops working.
    pub fn set_device_expires(&self, id: &str, expires: Option<u64>) -> anyhow::Result<()> {
        self.lock().execute("UPDATE devices SET expires = ?2 WHERE id = ?1", params![id, expires.map(|e| e as i64)])?;
        Ok(())
    }

    pub fn device(&self, id: &str) -> anyhow::Result<Option<DeviceRow>> {
        self.device_where("id = ?1", id)
    }

    fn device_where(&self, cond: &str, arg: &str) -> anyhow::Result<Option<DeviceRow>> {
        let sql = format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE {cond}");
        Ok(self.lock().query_row(&sql, params![arg], device_row).optional()?)
    }

    /// Devices not revoked, newest first; browsers only while signed in.
    pub fn devices(&self, now: u64) -> anyhow::Result<Vec<DeviceRow>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT {DEVICE_COLUMNS} FROM devices d WHERE revoked = 0 AND (d.expires IS NULL OR d.expires > ?1) AND (kind != 'browser'
               OR EXISTS (SELECT 1 FROM sessions s WHERE s.device = d.id AND s.expires > ?1))
             ORDER BY created DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![now as i64], device_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Notes that a device was seen now, from `address`.
    pub fn touch_device(&self, id: &str, address: &str, country: Option<&str>, now: u64) -> anyhow::Result<()> {
        self.lock().execute(
            "UPDATE devices SET seen = ?2, address = ?3, country = ?4 WHERE id = ?1",
            params![id, now as i64, address, country],
        )?;
        Ok(())
    }

    pub fn rename_device(&self, id: &str, name: &str) -> anyhow::Result<bool> {
        Ok(self.lock().execute("UPDATE devices SET name = ?2 WHERE id = ?1 AND revoked = 0", params![id, name])? == 1)
    }

    /// Revokes a device: its token stops working, its sessions and push
    /// subscription go. False if it was not there (or already revoked).
    pub fn revoke_device(&self, id: &str) -> anyhow::Result<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let changed =
            tx.execute("UPDATE devices SET revoked = 1, token_hash = NULL WHERE id = ?1 AND revoked = 0", params![id])?;
        tx.execute("DELETE FROM sessions WHERE device = ?1", params![id])?;
        tx.execute("DELETE FROM push WHERE device = ?1", params![id])?;
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Revokes every browser, phone and desktop but `keep` (the devices a
    /// person signed in on, not paired workspaces); returns their ids.
    pub fn revoke_people_devices_but(&self, keep: &str) -> anyhow::Result<Vec<String>> {
        let ids: Vec<String> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT id FROM devices WHERE revoked = 0 AND kind IN ('browser', 'phone', 'desktop') AND id != ?1",
            )?;
            stmt.query_map(params![keep], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for id in &ids {
            self.revoke_device(id)?;
        }
        Ok(ids)
    }

    // ------------------------------------------------------------ sessions

    pub fn add_session(&self, token_hash: &str, csrf: &str, device: &str, expires: u64) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO sessions (token_hash, csrf, device, expires) VALUES (?1, ?2, ?3, ?4)",
            params![token_hash, csrf, device, expires as i64],
        )?;
        Ok(())
    }

    /// A session that has not expired, of a device not revoked.
    pub fn session(&self, token_hash: &str, now: u64) -> anyhow::Result<Option<Session>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT s.device, s.csrf, s.expires FROM sessions s JOIN devices d ON d.id = s.device
                 WHERE s.token_hash = ?1 AND s.expires > ?2 AND d.revoked = 0",
                params![token_hash, now as i64],
                |r| Ok(Session { device: r.get(0)?, csrf: r.get(1)?, expires: r.get::<_, i64>(2)? as u64 }),
            )
            .optional()?)
    }

    /// Whether a device may still be connected: not revoked, and for a
    /// browser, with a session that has not expired.
    pub fn device_valid(&self, id: &str, now: u64) -> anyhow::Result<bool> {
        Ok(self
            .lock()
            .query_row(
                "SELECT 1 FROM devices d WHERE d.id = ?1 AND d.revoked = 0 AND (d.expires IS NULL OR d.expires > ?2) AND (d.kind != 'browser'
                   OR EXISTS (SELECT 1 FROM sessions s WHERE s.device = d.id AND s.expires > ?2))",
                params![id, now as i64],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    // ------------------------------------------------------------ pairings

    pub fn add_pairing(&self, pairing: &Pairing, poll_hash: &str) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO pairings (code, poll_hash, name, version, address, country, expires, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')",
            params![
                pairing.code,
                poll_hash,
                pairing.name,
                pairing.version,
                pairing.address,
                pairing.country,
                pairing.expires as i64
            ],
        )?;
        Ok(())
    }

    pub fn pairing_by_code(&self, code: &str) -> anyhow::Result<Option<Pairing>> {
        self.pairing_where("code = ?1", code)
    }

    pub fn pairing_by_poll(&self, poll_hash: &str) -> anyhow::Result<Option<Pairing>> {
        self.pairing_where("poll_hash = ?1", poll_hash)
    }

    /// By the id lists show (`Pairing::id`).
    pub fn pairing_by_id(&self, id: &str) -> anyhow::Result<Option<Pairing>> {
        if id.len() != PAIRING_ID_LEN {
            return Ok(None);
        }
        self.pairing_where(&format!("substr(poll_hash, 1, {PAIRING_ID_LEN}) = ?1"), id)
    }

    fn pairing_where(&self, cond: &str, arg: &str) -> anyhow::Result<Option<Pairing>> {
        let sql = format!(
            "SELECT code, name, version, address, country, expires, state, approved_name, poll_hash FROM pairings WHERE {cond}"
        );
        Ok(self.lock().query_row(&sql, params![arg], pairing_row).optional()?)
    }

    /// Pending requests that have not expired.
    pub fn pending_pairings(&self, now: u64) -> anyhow::Result<Vec<Pairing>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT code, name, version, address, country, expires, state, approved_name, poll_hash FROM pairings
             WHERE state = 'pending' AND expires > ?1 ORDER BY expires",
        )?;
        let rows = stmt.query_map(params![now as i64], pairing_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Approves or rejects a pending request; false if it was not pending.
    pub fn decide_pairing(&self, code: &str, state: &PairingState, expires: u64) -> anyhow::Result<bool> {
        let (state, name) = match state {
            PairingState::Pending => ("pending", None),
            PairingState::Approved(name) => ("approved", Some(name.as_str())),
            PairingState::Rejected => ("rejected", None),
        };
        Ok(self.lock().execute(
            "UPDATE pairings SET state = ?2, approved_name = ?3, expires = ?4 WHERE code = ?1 AND state = 'pending'",
            params![code, state, name, expires as i64],
        )? == 1)
    }

    pub fn delete_pairing(&self, code: &str) -> anyhow::Result<()> {
        self.lock().execute("DELETE FROM pairings WHERE code = ?1", params![code])?;
        Ok(())
    }

    /// Ends an approved pairing by making its workspace: a device with the
    /// token and a workspace row. Once only: the pairing is gone after.
    pub fn complete_pairing(
        &self,
        pairing: &Pairing,
        name: &str,
        token_hash: &str,
        now: u64,
    ) -> anyhow::Result<Option<String>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        if tx.execute("DELETE FROM pairings WHERE code = ?1 AND state = 'approved'", params![pairing.code])? != 1 {
            return Ok(None);
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO devices (id, kind, name, token_hash, created, seen, address, country)
             VALUES (?1, 'workspace', ?2, ?3, ?4, ?4, ?5, ?6)",
            params![id, name, token_hash, now as i64, pairing.address, pairing.country],
        )?;
        tx.execute("INSERT INTO workspaces (id, version) VALUES (?1, ?2)", params![id, pairing.version])?;
        tx.commit()?;
        Ok(Some(id))
    }

    // ---------------------------------------------------------- workspaces

    /// Paired workspaces not revoked, by name.
    pub fn workspaces(&self) -> anyhow::Result<Vec<WorkspaceRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT w.id, d.name, w.policy, w.version, d.seen, w.state FROM workspaces w
             JOIN devices d ON d.id = w.id WHERE d.revoked = 0 ORDER BY d.name, w.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkspaceRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    policy: policy_from(&r.get::<_, String>(2)?),
                    version: r.get(3)?,
                    seen: r.get::<_, i64>(4)? as u64,
                    state: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// A paired workspace, not revoked.
    pub fn workspace_exists(&self, id: &str) -> anyhow::Result<bool> {
        Ok(self
            .lock()
            .query_row(
                "SELECT 1 FROM workspaces w JOIN devices d ON d.id = w.id WHERE w.id = ?1 AND d.revoked = 0",
                params![id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Records what a workspace said when its link connected.
    pub fn workspace_hello(&self, id: &str, policy: Policy, version: &str) -> anyhow::Result<()> {
        self.lock().execute(
            "UPDATE workspaces SET policy = ?2, version = ?3 WHERE id = ?1",
            params![id, policy_str(policy), version],
        )?;
        Ok(())
    }

    pub fn save_state(&self, id: &str, state: &str, now: u64) -> anyhow::Result<()> {
        self.lock().execute(
            "UPDATE workspaces SET state = ?2, state_at = ?3 WHERE id = ?1",
            params![id, state, now as i64],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- push

    pub fn set_subscription(&self, device: &str, sub: &PushSubscribe) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO push (device, endpoint, p256dh, auth, kinds) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(device) DO UPDATE SET endpoint = ?2, p256dh = ?3, auth = ?4",
            params![device, sub.endpoint, sub.p256dh, sub.auth, kinds_json(&all_kinds())],
        )?;
        Ok(())
    }

    pub fn remove_subscription(&self, device: &str) -> anyhow::Result<()> {
        self.lock().execute(
            "UPDATE push SET endpoint = NULL, p256dh = NULL, auth = NULL WHERE device = ?1",
            params![device],
        )?;
        Ok(())
    }

    /// Whether the device has a subscription to push to.
    pub fn subscribed(&self, device: &str) -> anyhow::Result<bool> {
        Ok(self
            .lock()
            .query_row("SELECT 1 FROM push WHERE device = ?1 AND endpoint IS NOT NULL", params![device], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn set_push_kinds(&self, device: &str, kinds: &[NotificationKind]) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO push (device, kinds) VALUES (?1, ?2) ON CONFLICT(device) DO UPDATE SET kinds = ?2",
            params![device, kinds_json(kinds)],
        )?;
        Ok(())
    }

    pub fn push_kinds(&self, device: &str) -> anyhow::Result<Vec<NotificationKind>> {
        let kinds: Option<String> = self
            .lock()
            .query_row("SELECT kinds FROM push WHERE device = ?1", params![device], |r| r.get(0))
            .optional()?;
        Ok(kinds.map(|k| kinds_from(&k)).unwrap_or_else(all_kinds))
    }

    /// Subscriptions that want `kind`.
    pub fn subscriptions(&self, kind: NotificationKind) -> anyhow::Result<Vec<Subscription>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT p.device, p.endpoint, p.p256dh, p.auth, p.kinds, d.kind FROM push p JOIN devices d ON d.id = p.device
             WHERE d.revoked = 0 AND p.endpoint IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Subscription {
                    device: r.get(0)?,
                    endpoint: r.get(1)?,
                    p256dh: r.get(2)?,
                    auth: r.get(3)?,
                    kinds: kinds_from(&r.get::<_, String>(4)?),
                    kind: kind_from(&r.get::<_, String>(5)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter(|s| s.kinds.contains(&kind)).collect())
    }

    /// Whether some phone wants `kind`, for a ntfy topic all phones share:
    /// any signed-in phone that wants it (one that never chose wants all),
    /// or every kind when no phone is signed in.
    pub fn phones_want(&self, kind: NotificationKind) -> anyhow::Result<bool> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT p.kinds FROM devices d LEFT JOIN push p ON p.device = d.id WHERE d.revoked = 0 AND d.kind = 'phone'",
        )?;
        let phones = stmt.query_map([], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(phones.is_empty() || phones.iter().any(|k| k.as_deref().map(kinds_from).unwrap_or_else(all_kinds).contains(&kind)))
    }

    // --------------------------------------------------------------- audit

    pub fn audit(&self, what: &str, info: AuditInfo<'_>, now: u64) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO audit (at, what, device, address, country, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now as i64, what, info.device, info.address, info.country, info.detail],
        )?;
        Ok(())
    }

    /// The latest entries, newest first.
    pub fn audit_entries(&self, limit: u32) -> anyhow::Result<Vec<AuditEntry>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT at, what, device, address, country, detail FROM audit ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(AuditEntry {
                    at: r.get::<_, i64>(0)? as u32,
                    what: r.get(1)?,
                    device: r.get(2)?,
                    address: r.get(3)?,
                    country: r.get(4)?,
                    detail: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------- cleanup

    /// Drops what has expired: sessions (and the browsers left without one),
    /// device tokens past their end, pairing requests, and audit entries
    /// older than a year.
    pub fn cleanup(&self, now: u64) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM sessions WHERE expires <= ?1", params![now as i64])?;
        conn.execute(
            "UPDATE devices SET revoked = 1 WHERE kind = 'browser' AND revoked = 0
             AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.device = devices.id)",
            [],
        )?;
        conn.execute(
            "UPDATE devices SET revoked = 1, token_hash = NULL WHERE revoked = 0 AND expires IS NOT NULL AND expires <= ?1",
            params![now as i64],
        )?;
        conn.execute("DELETE FROM pairings WHERE expires <= ?1", params![now as i64])?;
        conn.execute("DELETE FROM audit WHERE at < ?1", params![now.saturating_sub(366 * 86400) as i64])?;
        Ok(())
    }
}

const DEVICE_COLUMNS: &str = "id, kind, name, created, seen, address, country, revoked, expires";

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        id: r.get(0)?,
        kind: kind_from(&r.get::<_, String>(1)?),
        name: r.get(2)?,
        created: r.get::<_, i64>(3)? as u64,
        seen: r.get::<_, i64>(4)? as u64,
        address: r.get(5)?,
        country: r.get(6)?,
        revoked: r.get::<_, i64>(7)? != 0,
        expires: r.get::<_, Option<i64>>(8)?.map(|e| e as u64),
    })
}

/// Hex characters of a pairing's id.
const PAIRING_ID_LEN: usize = 16;

/// A pairing's id, from its poll secret's hash.
pub fn pairing_id(poll_hash: &str) -> String {
    poll_hash.chars().take(PAIRING_ID_LEN).collect()
}

fn pairing_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pairing> {
    let state: String = r.get(6)?;
    let approved: Option<String> = r.get(7)?;
    let poll_hash: String = r.get(8)?;
    Ok(Pairing {
        id: pairing_id(&poll_hash),
        code: r.get(0)?,
        name: r.get(1)?,
        version: r.get(2)?,
        address: r.get(3)?,
        country: r.get(4)?,
        expires: r.get::<_, i64>(5)? as u64,
        state: match state.as_str() {
            "approved" => PairingState::Approved(approved.unwrap_or_default()),
            "rejected" => PairingState::Rejected,
            _ => PairingState::Pending,
        },
    })
}

pub fn kind_str(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Browser => "browser",
        DeviceKind::Phone => "phone",
        DeviceKind::Workspace => "workspace",
        DeviceKind::Desktop => "desktop",
    }
}

fn kind_from(s: &str) -> DeviceKind {
    match s {
        "browser" => DeviceKind::Browser,
        "workspace" => DeviceKind::Workspace,
        "desktop" => DeviceKind::Desktop,
        _ => DeviceKind::Phone,
    }
}

fn policy_str(policy: Policy) -> &'static str {
    match policy {
        Policy::View => "view",
        Policy::Full => "full",
    }
}

fn policy_from(s: &str) -> Policy {
    if s == "view" { Policy::View } else { Policy::Full }
}

fn kinds_json(kinds: &[NotificationKind]) -> String {
    serde_json::to_string(kinds).unwrap_or_else(|_| "[]".into())
}

fn kinds_from(s: &str) -> Vec<NotificationKind> {
    // Kinds this nexus does not know (from a newer one) are dropped.
    let values: Vec<serde_json::Value> = serde_json::from_str(s).unwrap_or_default();
    values.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_keeps_devices() {
        let store = Store::in_memory().unwrap();
        let id = store.add_device(DeviceKind::Phone, "Pixel", Some("h"), "1.2.3.4", None, 10).unwrap();
        assert_eq!(store.device_by_token("h", 10).unwrap().unwrap().id, id);
        assert!(store.revoke_device(&id).unwrap());
        assert!(store.device_by_token("h", 10).unwrap().is_none());
        assert!(!store.revoke_device(&id).unwrap());
    }

    #[test]
    fn a_totp_step_is_used_once() {
        let store = Store::in_memory().unwrap();
        let account =
            Account { username: "u".into(), password: "p".into(), totp_secret: "s".into(), totp_step: 0 };
        store.create_account(&account, &["r1".into()], 1).unwrap();
        assert!(store.use_totp_step(100).unwrap());
        assert!(!store.use_totp_step(100).unwrap());
        assert!(!store.use_totp_step(99).unwrap());
        assert!(store.use_totp_step(101).unwrap());
        assert!(store.use_recovery_code("r1", 2).unwrap());
        assert!(!store.use_recovery_code("r1", 3).unwrap());
    }

    #[test]
    fn browsers_without_a_session_are_not_listed() {
        let store = Store::in_memory().unwrap();
        let browser = store.add_device(DeviceKind::Browser, "Firefox", None, "", None, 10).unwrap();
        store.add_session("t", "c", &browser, 100).unwrap();
        assert_eq!(store.devices(50).unwrap().len(), 1);
        assert!(store.device_valid(&browser, 50).unwrap());
        assert!(store.devices(150).unwrap().is_empty());
        assert!(!store.device_valid(&browser, 150).unwrap());
        store.cleanup(150).unwrap();
        assert!(store.device(&browser).unwrap().unwrap().revoked);
    }

    #[test]
    fn a_desktops_token_ends() {
        let store = Store::in_memory().unwrap();
        let desktop = store.add_device(DeviceKind::Desktop, "Shed", Some("d"), "", None, 10).unwrap();
        store.set_device_expires(&desktop, Some(100)).unwrap();
        assert!(store.device_by_token("d", 99).unwrap().is_some());
        assert!(store.device_valid(&desktop, 99).unwrap());
        assert_eq!(store.devices(99).unwrap().len(), 1);
        assert!(store.device_by_token("d", 100).unwrap().is_none());
        assert!(!store.device_valid(&desktop, 100).unwrap());
        assert!(store.devices(100).unwrap().is_empty());
        store.cleanup(100).unwrap();
        assert!(store.device(&desktop).unwrap().unwrap().revoked);
    }

    #[test]
    fn push_kinds_filter_subscriptions() {
        let store = Store::in_memory().unwrap();
        let phone = store.add_device(DeviceKind::Phone, "P", Some("h"), "", None, 1).unwrap();
        let sub = PushSubscribe { endpoint: "https://push.example/x".into(), p256dh: "k".into(), auth: "a".into() };
        store.set_subscription(&phone, &sub).unwrap();
        assert_eq!(store.subscriptions(NotificationKind::AiDone).unwrap().len(), 1);
        store.set_push_kinds(&phone, &[NotificationKind::AiWaiting]).unwrap();
        assert!(store.subscriptions(NotificationKind::AiDone).unwrap().is_empty());
        assert_eq!(store.subscriptions(NotificationKind::AiWaiting).unwrap().len(), 1);
        assert_eq!(store.subscriptions(NotificationKind::AiWaiting).unwrap()[0].kind, DeviceKind::Phone);
    }

    #[test]
    fn a_shared_topic_carries_what_any_phone_wants() {
        let store = Store::in_memory().unwrap();
        // No phone: everything.
        assert!(store.phones_want(NotificationKind::AiDone).unwrap());
        let browser = store.add_device(DeviceKind::Browser, "B", None, "", None, 1).unwrap();
        store.set_push_kinds(&browser, &[]).unwrap();
        assert!(store.phones_want(NotificationKind::AiDone).unwrap());
        // A phone that never chose wants everything.
        let a = store.add_device(DeviceKind::Phone, "A", Some("a"), "", None, 1).unwrap();
        assert!(store.phones_want(NotificationKind::CommandDone).unwrap());
        store.set_push_kinds(&a, &[NotificationKind::AiWaiting]).unwrap();
        assert!(!store.phones_want(NotificationKind::CommandDone).unwrap());
        assert!(store.phones_want(NotificationKind::AiWaiting).unwrap());
        // Another phone's choices count too; a revoked one's do not.
        let b = store.add_device(DeviceKind::Phone, "B", Some("b"), "", None, 1).unwrap();
        store.set_push_kinds(&b, &[NotificationKind::CommandDone]).unwrap();
        assert!(store.phones_want(NotificationKind::CommandDone).unwrap());
        store.revoke_device(&b).unwrap();
        assert!(!store.phones_want(NotificationKind::CommandDone).unwrap());
    }
}
