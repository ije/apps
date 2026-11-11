use crate::node::sqlite::ffi as sql;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    ffi::{CStr, CString},
    path::PathBuf,
    ptr,
    sync::{Mutex, OnceLock},
};

struct Database(
    *mut sql::sqlite3,
    RefCell<HashMap<String, *mut sql::sqlite3_stmt>>,
);
impl Drop for Database {
    fn drop(&mut self) {
        unsafe {
            for &statement in self.1.get_mut().values() {
                sql::sqlite3_finalize(statement);
            }
            sql::sqlite3_close(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_quota_tracks_updates_and_multiple_connections() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = State {
            path: directory.path().join("browser.sqlite"),
            ..State::default()
        };
        let mut second = State {
            path: first.path.clone(),
            ..State::default()
        };
        let value = "é".repeat((5 * 1024 * 1024 - 2) / 2);
        first.call("local.set", &json!(["é", value])).unwrap();
        assert!(
            second
                .call("local.set", &json!(["x", "x"]))
                .unwrap_err()
                .to_string()
                .contains("QuotaExceededError")
        );
        assert_eq!(
            second.call("local.get", &json!(["x"])).unwrap(),
            Value::Null
        );
        first.call("local.set", &json!(["é", "small"])).unwrap();
        second.call("local.set", &json!(["x", "x"])).unwrap();
        first.call("local.remove", &json!(["é"])).unwrap();
        let db = first.local_database.as_ref().unwrap();
        assert_eq!(
            db.query("SELECT bytes FROM local_size", &[]).unwrap()[0][0].as_deref(),
            Some("2")
        );
        second.call("local.clear", &json!([])).unwrap();
        assert_eq!(
            db.query("SELECT bytes FROM local_size", &[]).unwrap()[0][0].as_deref(),
            Some("0")
        );
    }

    #[test]
    fn json_keys_move_to_ordered_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.sqlite");
        let old = Database::open(&path).unwrap();
        old.query("CREATE TABLE records (db TEXT, store TEXT, key TEXT, value TEXT NOT NULL, PRIMARY KEY(db,store,key))", &[]).unwrap();
        for key in [r#"[2,"b"]"#, "[0,10]", "[0,2]"] {
            old.query("INSERT INTO records VALUES('db','items',?,'1')", &[key])
                .unwrap();
        }
        drop(old);
        let mut state = State {
            path,
            ..State::default()
        };
        let id = state.call("begin", &json!(["db", "readonly"])).unwrap()["id"]
            .as_u64()
            .unwrap();
        assert_eq!(
            state
                .call(
                    "scan",
                    &json!([id, "items", null, null, false, false, false, 0, "keys"])
                )
                .unwrap(),
            json!([[0, 2.0], [0, 10.0], [2, "b"]])
        );
    }

    #[test]
    fn transactions_reuse_connections_after_commit_and_abort() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = State {
            path: directory.path().join("browser.sqlite"),
            ..State::default()
        };
        let mut connection = None;
        for operation in ["commit", "abort", "commit"] {
            let id = state
                .call("begin", &json!(["database", "readwrite"]))
                .unwrap()["id"]
                .as_u64()
                .unwrap();
            let current = state.transactions[&id].database.0;
            if let Some(previous) = connection {
                assert_eq!(current, previous);
            }
            connection = Some(current);
            assert_eq!(
                state
                    .call("begin", &json!(["database", "readonly"]))
                    .unwrap(),
                json!({"busy":true})
            );
            state.call(operation, &json!([id])).unwrap();
        }
    }

    #[test]
    fn busy_writers_wake_when_the_other_writer_ends() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.sqlite");
        let [mut first, mut second] = [0, 1].map(|_| State {
            path: path.clone(),
            ..State::default()
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        crate::host::Waker::install(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let id = first.call("begin", &json!(["a", "readwrite"])).unwrap()["id"].clone();
        let seen = wakes.load(Ordering::Relaxed);
        assert_eq!(
            second.call("begin", &json!(["b", "readwrite"])).unwrap(),
            json!({"busy":true})
        );
        first.call("commit", &json!([id])).unwrap();
        assert!(wakes.load(Ordering::Relaxed) > seen);
        assert!(second.call("begin", &json!(["b", "readwrite"])).unwrap()["id"].is_u64());
    }
}
impl Database {
    fn open(path: &std::path::Path) -> Result<Self> {
        let mut raw = ptr::null_mut();
        let path = CString::new(path.to_string_lossy().as_bytes())?;
        let code = unsafe {
            sql::sqlite3_open_v2(
                path.as_ptr(),
                &mut raw,
                sql::SQLITE_OPEN_READWRITE | sql::SQLITE_OPEN_CREATE | sql::SQLITE_OPEN_FULLMUTEX,
                ptr::null(),
            )
        };
        ensure!(!raw.is_null(), "Cannot allocate browser storage");
        let db = Self(raw, RefCell::default());
        ensure!(code == sql::SQLITE_OK, "Cannot open browser storage");
        Ok(db)
    }
    fn query(&self, source: &str, parameters: &[&str]) -> Result<Vec<Vec<Option<String>>>> {
        // A cached statement holds its read lock and bound values until it is reset.
        struct Statement(*mut sql::sqlite3_stmt);
        impl Drop for Statement {
            fn drop(&mut self) {
                unsafe {
                    sql::sqlite3_reset(self.0);
                    sql::sqlite3_clear_bindings(self.0);
                }
            }
        }
        let cached = self.1.borrow().get(source).copied();
        let statement = Statement(match cached {
            Some(statement) => statement,
            None => {
                let text = CString::new(source)?;
                let mut raw = ptr::null_mut();
                let code = unsafe {
                    sql::sqlite3_prepare_v3(
                        self.0,
                        text.as_ptr(),
                        -1,
                        sql::SQLITE_PREPARE_PERSISTENT,
                        &mut raw,
                        ptr::null_mut(),
                    )
                };
                ensure!(
                    code == sql::SQLITE_OK && !raw.is_null(),
                    "{}",
                    unsafe { CStr::from_ptr(sql::sqlite3_errmsg(self.0)) }.to_string_lossy()
                );
                self.1.borrow_mut().insert(source.to_owned(), raw);
                raw
            }
        });
        let mut rows = Vec::new();
        for (index, value) in parameters.iter().enumerate() {
            ensure!(
                unsafe {
                    sql::sqlite3_bind_text(
                        statement.0,
                        index as i32 + 1,
                        value.as_ptr().cast(),
                        i32::try_from(value.len())?,
                        sql::SQLITE_TRANSIENT(),
                    )
                } == sql::SQLITE_OK,
                "Cannot bind browser storage value"
            );
        }
        loop {
            let code = unsafe { sql::sqlite3_step(statement.0) };
            if code == sql::SQLITE_DONE {
                break;
            }
            ensure!(
                code == sql::SQLITE_ROW,
                "{}",
                unsafe { CStr::from_ptr(sql::sqlite3_errmsg(self.0)) }.to_string_lossy()
            );
            let row = (0..unsafe { sql::sqlite3_column_count(statement.0) })
                .map(|index| unsafe {
                    if sql::sqlite3_column_type(statement.0, index) == sql::SQLITE_NULL {
                        None
                    } else {
                        let data = sql::sqlite3_column_text(statement.0, index);
                        let size = sql::sqlite3_column_bytes(statement.0, index) as usize;
                        Some(
                            String::from_utf8_lossy(std::slice::from_raw_parts(data, size))
                                .into_owned(),
                        )
                    }
                })
                .collect();
            rows.push(row);
        }
        Ok(rows)
    }
}

// Preserve key order: numbers, dates, strings, binary, arrays. Zero terminates string and binary units.
fn encode_key(key: &Value, out: &mut Vec<u8>) -> Result<()> {
    let unit = |out: &mut Vec<u8>, unit: u32| {
        if unit < 0x7f {
            out.push(unit as u8 + 1);
        } else if unit < 0x407f {
            out.extend(((unit - 0x7f + 0x8000) as u16).to_be_bytes());
        } else {
            out.extend(&((unit << 6) | 0xc0_0000).to_be_bytes()[1..]);
        }
    };
    let invalid = || anyhow::anyhow!("Invalid IndexedDB key");
    match key[0].as_u64() {
        Some(tag @ (0 | 1)) => {
            let number = match &key[1] {
                Value::String(sign) if sign == "-" => f64::NEG_INFINITY,
                Value::String(_) => f64::INFINITY,
                value => value.as_f64().ok_or_else(invalid)?,
            } + 0.;
            let bits = number.to_bits();
            let flip = if bits >> 63 == 1 { u64::MAX } else { 1 << 63 };
            out.push(0x10 << tag);
            out.extend((bits ^ flip).to_be_bytes());
        }
        Some(2) => {
            out.push(0x30);
            for code in key[1].as_str().ok_or_else(invalid)?.encode_utf16() {
                unit(out, code.into());
            }
            out.push(0);
        }
        Some(3) => {
            out.push(0x40);
            for byte in key[1].as_array().ok_or_else(invalid)? {
                unit(
                    out,
                    byte.as_u64()
                        .filter(|byte| *byte < 256)
                        .ok_or_else(invalid)? as u32,
                );
            }
            out.push(0);
        }
        Some(4) => {
            out.push(0x50);
            for item in key[1].as_array().ok_or_else(invalid)? {
                encode_key(item, out)?;
            }
            out.push(0);
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

fn decode_key(bytes: &[u8], at: &mut usize) -> Option<Value> {
    let tag = *bytes.get(*at)?;
    *at += 1;
    let mut units = Vec::new();
    if matches!(tag, 0x30 | 0x40) {
        loop {
            let first = *bytes.get(*at)?;
            let (code, size) = match first {
                0 => break,
                1..=0x7f => (u32::from(first) - 1, 1),
                0x80..=0xbf => {
                    let pair = u16::from_be_bytes([first, *bytes.get(*at + 1)?]);
                    (u32::from(pair) - 0x8000 + 0x7f, 2)
                }
                _ => {
                    let triple =
                        u32::from_be_bytes([0, first, *bytes.get(*at + 1)?, *bytes.get(*at + 2)?]);
                    ((triple & 0x3f_ffff) >> 6, 3)
                }
            };
            units.push(code);
            *at += size;
        }
        *at += 1;
    }
    Some(match tag {
        0x10 | 0x20 => {
            let bits = u64::from_be_bytes(bytes.get(*at..*at + 8)?.try_into().ok()?);
            *at += 8;
            let flip = if bits >> 63 == 1 { 1 << 63 } else { u64::MAX };
            let number = f64::from_bits(bits ^ flip);
            let value = if number.is_infinite() {
                json!(if number > 0. { "+" } else { "-" })
            } else {
                json!(number)
            };
            json!([tag >> 5, value])
        }
        0x30 => json!([
            2,
            String::from_utf16_lossy(&units.iter().map(|&unit| unit as u16).collect::<Vec<_>>())
        ]),
        0x40 => json!([3, units]),
        0x50 => {
            let mut items = Vec::new();
            while *bytes.get(*at)? != 0 {
                items.push(decode_key(bytes, at)?);
            }
            *at += 1;
            json!([4, items])
        }
        _ => return None,
    })
}

// The stored form of an IndexedDB key: hex keeps the byte order in a TEXT column.
fn hex_key(wire: &str) -> Result<String> {
    let mut bytes = Vec::new();
    encode_key(&serde_json::from_str(wire)?, &mut bytes)?;
    Ok(bytes
        .iter()
        .flat_map(|byte| {
            [
                b"0123456789abcdef"[usize::from(byte >> 4)] as char,
                b"0123456789abcdef"[usize::from(byte & 15)] as char,
            ]
        })
        .collect())
}

fn wire_key(hex: Option<String>) -> Value {
    let hex = hex.unwrap_or_default();
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|index| u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect();
    decode_key(&bytes, &mut 0).unwrap_or(Value::Null)
}

type SessionKey = (String, u64);
type Session = (usize, BTreeMap<String, String>);
static SESSIONS: OnceLock<Mutex<HashMap<SessionKey, Session>>> = OnceLock::new();
static WAITING: Mutex<Vec<crate::host::Waker>> = Mutex::new(Vec::new());
// Serialize journal-mode changes: SQLite ignores the busy timeout for them, and windows open databases on
// separate threads.
static SETUP: Mutex<()> = Mutex::new(());
struct Transaction {
    database: Database,
    name: String,
    writable: bool,
}
#[derive(Default)]
pub(crate) struct State {
    namespace: String,
    window: u64,
    database: Option<Database>,
    local_database: Option<Database>,
    path: PathBuf,
    transactions: HashMap<u64, Transaction>,
    next_id: u64,
    idle: Vec<Database>,
    keys: Option<(Option<String>, Vec<String>)>,
}

pub(crate) fn forget(window: u64) {
    if let Some(sessions) = SESSIONS.get()
        && let Ok(mut sessions) = sessions.lock()
    {
        sessions.retain(|(_, id), _| *id != window);
    }
}
impl State {
    pub(crate) fn configure(&mut self, namespace: &str, window: Option<u64>) {
        self.namespace = ring::digest::digest(&ring::digest::SHA256, namespace.as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.window = window.unwrap_or(0);
        let root = std::env::var_os("APPS_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(target_os = "windows") {
                    return PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
                        .join("apps");
                }
                let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                if cfg!(target_os = "macos") {
                    home.join("Library/Application Support/apps")
                } else {
                    std::env::var_os("XDG_DATA_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join(".local/share"))
                        .join("apps")
                }
            });
        self.path = root.join(&self.namespace).join("browser.sqlite");
    }
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        if let Some(op) = op.strip_prefix("session.") {
            let mut sessions = SESSIONS
                .get_or_init(Mutex::default)
                .lock()
                .map_err(|_| anyhow::anyhow!("Session storage lock failed"))?;
            let (size, values) = sessions
                .entry((self.namespace.clone(), self.window))
                .or_default();
            let key = args[0].as_str().unwrap_or("");
            return match op {
                "get" => Ok(json!(values.get(key))),
                "keys" => Ok(json!(values.keys().collect::<Vec<_>>())),
                "length" => Ok(json!(values.len())),
                "key" => Ok(json!(
                    args[0]
                        .as_u64()
                        .and_then(|index| values.keys().nth(index as usize))
                )),
                "set" => {
                    let value = args[1].as_str().context("Expected storage value")?;
                    let old = values.get(key).map_or(0, |old| key.len() + old.len());
                    ensure!(
                        *size - old + key.len() + value.len() <= 5 << 20,
                        "QuotaExceededError: sessionStorage is full"
                    );
                    *size = *size - old + key.len() + value.len();
                    values.insert(key.to_owned(), value.to_owned());
                    Ok(Value::Null)
                }
                "remove" => {
                    if let Some(old) = values.remove(key) {
                        *size -= key.len() + old.len();
                    }
                    Ok(Value::Null)
                }
                "clear" => {
                    *size = 0;
                    values.clear();
                    Ok(Value::Null)
                }
                _ => bail!("Unknown session storage operation"),
            };
        }
        if self.database.is_none() {
            let _setup = SETUP
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ensure!(
                !self.path.as_os_str().is_empty(),
                "Browser storage namespace is not configured"
            );
            std::fs::create_dir_all(self.path.parent().unwrap())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    self.path.parent().unwrap(),
                    std::fs::Permissions::from_mode(0o700),
                )?;
            }
            let database = Database::open(&self.path)?;
            unsafe { sql::sqlite3_busy_timeout(database.0, 1000) };
            database.query("PRAGMA journal_mode=WAL", &[])?;
            database.query("PRAGMA synchronous=NORMAL", &[])?;
            database.query("CREATE TABLE IF NOT EXISTS databases (name TEXT PRIMARY KEY, version INTEGER NOT NULL)",&[])?;
            database.query("CREATE TABLE IF NOT EXISTS stores (db TEXT, name TEXT, options TEXT NOT NULL, PRIMARY KEY(db,name))",&[])?;
            database.query("CREATE TABLE IF NOT EXISTS entries (db TEXT, store TEXT, key TEXT, value TEXT NOT NULL, PRIMARY KEY(db,store,key))",&[])?;
            if !database
                .query("SELECT 1 FROM sqlite_master WHERE name='records'", &[])?
                .is_empty()
            {
                database.query("BEGIN IMMEDIATE", &[])?;
                let moved = database.query("SELECT db,store,key,value FROM records", &[]).and_then(|rows| {
                    for row in rows {
                        let [db, store, key, value] = [0, 1, 2, 3].map(|index| row[index].as_deref().unwrap_or_default());
                        database.query("INSERT OR REPLACE INTO entries(db,store,key,value) VALUES(?,?,?,?)", &[db, store, &hex_key(key)?, value])?;
                    }
                    database.query("DROP TABLE records", &[])
                });
                database.query(if moved.is_ok() { "COMMIT" } else { "ROLLBACK" }, &[])?;
                moved?;
            }
            self.database = Some(database);
        }
        let db = self.database.as_ref().unwrap();
        if let Some(op) = op.strip_prefix("local.") {
            if self.local_database.is_none() {
                let _setup = SETUP
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let database = Database::open(&self.path.with_file_name("local.sqlite"))?;
                unsafe { sql::sqlite3_busy_timeout(database.0, 1000) };
                database.query("PRAGMA journal_mode=WAL", &[])?;
                database.query("PRAGMA synchronous=NORMAL", &[])?;
                database.query(
                    "CREATE TABLE IF NOT EXISTS local (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                    &[],
                )?;
                database.query("CREATE TABLE IF NOT EXISTS local_size (id INTEGER PRIMARY KEY CHECK(id=1), bytes INTEGER NOT NULL)", &[])?;
                database.query("INSERT OR IGNORE INTO local_size SELECT 1,coalesce(sum(length(CAST(key AS BLOB))+length(CAST(value AS BLOB))),0) FROM local", &[])?;
                for (name, operation, delta) in [
                    (
                        "insert",
                        "INSERT",
                        "length(CAST(new.key AS BLOB))+length(CAST(new.value AS BLOB))",
                    ),
                    (
                        "delete",
                        "DELETE",
                        "-length(CAST(old.key AS BLOB))-length(CAST(old.value AS BLOB))",
                    ),
                    (
                        "update",
                        "UPDATE",
                        "length(CAST(new.key AS BLOB))+length(CAST(new.value AS BLOB))-length(CAST(old.key AS BLOB))-length(CAST(old.value AS BLOB))",
                    ),
                ] {
                    database.query(&format!("CREATE TRIGGER IF NOT EXISTS local_size_{name} AFTER {operation} ON local BEGIN UPDATE local_size SET bytes=bytes+({delta}) WHERE id=1; SELECT CASE WHEN (SELECT bytes FROM local_size WHERE id=1)>5242880 THEN RAISE(ABORT,'QuotaExceededError: localStorage is full') END; END"), &[])?;
                }
                self.local_database = Some(database);
            }
            let db = self.local_database.as_ref().unwrap();
            let key = args[0].as_str().unwrap_or("");
            if matches!(op, "keys" | "length" | "key") {
                // Other connections can change the keys, and `data_version` only moves for their commits.
                let version = db
                    .query("PRAGMA data_version", &[])?
                    .into_iter()
                    .flatten()
                    .next()
                    .flatten();
                if self
                    .keys
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != version)
                {
                    let keys = db
                        .query("SELECT key FROM local ORDER BY rowid", &[])?
                        .into_iter()
                        .map(|row| row.into_iter().flatten().next().unwrap_or_default())
                        .collect();
                    self.keys = Some((version, keys));
                }
                let keys = &self.keys.as_ref().unwrap().1;
                return Ok(match op {
                    "length" => json!(keys.len()),
                    "key" => json!(args[0].as_u64().and_then(|index| keys.get(index as usize))),
                    _ => json!(keys),
                });
            }
            if matches!(op, "set" | "remove" | "clear") {
                self.keys = None;
            }
            return match op {
                "get" => Ok(json!(
                    db.query("SELECT value FROM local WHERE key=?", &[key])?
                        .first()
                        .and_then(|r| r[0].as_ref())
                )),
                "set" => {
                    let value = args[1].as_str().context("Expected storage value")?;
                    db.query("INSERT INTO local(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",&[key,value])?;
                    Ok(Value::Null)
                }
                "remove" => {
                    db.query("DELETE FROM local WHERE key=?", &[key])?;
                    Ok(Value::Null)
                }
                "clear" => {
                    db.query("DELETE FROM local", &[])?;
                    Ok(Value::Null)
                }
                _ => bail!("Unknown local storage operation"),
            };
        }
        match op {
            "open"=>{
                let name=args[0].as_str().context("Expected database name")?;
                let version=db.query("SELECT version FROM databases WHERE name=?",&[name])?.first().and_then(|r|r[0].as_ref()).map(|v|v.parse::<u64>()).transpose()?.unwrap_or(0);
                let stores=db.query("SELECT name,options FROM stores WHERE db=?",&[name])?.into_iter().map(|r|Ok((r[0].clone().unwrap_or_default(),serde_json::from_str::<Value>(r[1].as_deref().unwrap_or("null"))?))).collect::<Result<HashMap<_,_>>>()?;
                Ok(json!({"version":version,"stores":stores}))
            }
            "begin"=>{
                let name=args[0].as_str().context("Expected database name")?.to_owned();let writable=args[1]!="readonly";
                // Pages also retry on a timer, so a waker is only a shortcut. The list is capped, because another process can hold the lock for as long as it likes.
                let busy=||{if let Ok(mut waiting)=WAITING.lock()&&waiting.len()<64{waiting.push(crate::host::Waker::current());}Ok(json!({"busy":true}))};
                if self.transactions.values().any(|transaction| transaction.name == name) { return busy(); }
                let database=match self.idle.pop(){Some(database)=>database,None=>{let database=Database::open(&self.path)?;database.query("PRAGMA synchronous=NORMAL",&[])?;database}};
                match database.query(if writable {"BEGIN IMMEDIATE"}else{"BEGIN"},&[]) {Ok(_)=>{},Err(error) => {
                    if self.idle.len() < 4 { self.idle.push(database); }
                    if error.to_string().contains("locked") { return busy(); } return Err(error); }}
                self.next_id+=1;self.transactions.insert(self.next_id,Transaction{database,name,writable});Ok(json!({"id":self.next_id}))
            }
            "commit"|"abort"=>{
                let id=args[0].as_u64().context("Expected transaction ID")?;
                let transaction=self.transactions.remove(&id).context("Transaction is inactive")?;
                let ended=transaction.database.query(if op=="commit"{"COMMIT"}else{"ROLLBACK"},&[]);
                if ended.is_ok() && self.idle.len() < 4 { self.idle.push(transaction.database); }
                for waker in WAITING.lock().map(|mut waiting|std::mem::take(&mut *waiting)).unwrap_or_default() { waker.wake(); }
                ended.map(|_|Value::Null)
            }
            "databases"=>Ok(json!(db.query("SELECT name,version FROM databases",&[])?.into_iter().map(|r|json!({"name":r[0],"version":r[1].as_deref().and_then(|v|v.parse::<u64>().ok())})).collect::<Vec<_>>())),
            _=>{
                let id=args[0].as_u64().context("Expected transaction ID")?;let tx=self.transactions.get(&id).context("Transaction is inactive")?;
                let store=args[1].as_str().unwrap_or("");
                if op!="scan" {ensure!(tx.writable,"ReadOnlyError: transaction is readonly");}
                match op {
                    "version"=>{tx.database.query("INSERT INTO databases(name,version) VALUES(?,?) ON CONFLICT(name) DO UPDATE SET version=excluded.version",&[&tx.name,&args[1].to_string()])?;},
                    "createStore"=>{tx.database.query("INSERT INTO stores(db,name,options) VALUES(?,?,?)",&[&tx.name,store,&args[2].to_string()])?;},
                    "deleteStore"=>{tx.database.query("DELETE FROM entries WHERE db=? AND store=?",&[&tx.name,store])?;tx.database.query("DELETE FROM stores WHERE db=? AND name=?",&[&tx.name,store])?;},
                    "deleteDatabase"=>{tx.database.query("DELETE FROM entries WHERE db=?",&[&tx.name])?;tx.database.query("DELETE FROM stores WHERE db=?",&[&tx.name])?;tx.database.query("DELETE FROM databases WHERE name=?",&[&tx.name])?;},
                    "scan"|"delete"=>{
                        // Stored keys never prefix each other, so a key with `00` appended is the smallest key above it.
                        let bound=|index:usize,after:bool,end:&str|args[index].as_str().map_or(Ok(end.to_owned()),|key|hex_key(key).map(|mut hex|{if after{hex.push_str("00")}hex}));
                        let (lower,upper)=(bound(2,args[4]==true,"")?,bound(3,args[5]!=true,"z")?);
                        let range=[tx.name.as_str(),store,&lower,&upper];
                        if op=="delete" {tx.database.query("DELETE FROM entries WHERE db=? AND store=? AND key>=? AND key<?",&range)?;return Ok(Value::Null);}
                        let kind=args[8].as_str().unwrap_or("rows");
                        if kind=="count" {return Ok(json!(tx.database.query("SELECT count(*) FROM entries WHERE db=? AND store=? AND key>=? AND key<?",&range)?.first().and_then(|row|row[0].as_deref()?.parse::<u64>().ok())));}
                        let limit=args[7].as_u64().filter(|count|*count>0).map_or_else(||"-1".to_owned(),|count|count.to_string());
                        let columns=match kind {"keys"=>"key","values"=>"value",_=>"key,value"};
                        let rows=tx.database.query(&format!("SELECT {columns} FROM entries WHERE db=? AND store=? AND key>=? AND key<? ORDER BY key {} LIMIT ?",if args[6]==true{"DESC"}else{"ASC"}),&[&tx.name,store,&lower,&upper,&limit])?;
                        return Ok(rows.into_iter().map(|mut row|match kind {"keys"=>wire_key(row[0].take()),"values"=>row[0].take().into(),_=>Value::Array(vec![wire_key(row[0].take()),row[1].take().into()])}).collect());
                    }
                    "put"|"add"=>{let (key,value)=(hex_key(args[2].as_str().context("Expected record key")?)?,args[3].as_str().context("Expected record value")?);tx.database.query(if op=="put"{"INSERT INTO entries(db,store,key,value) VALUES(?,?,?,?) ON CONFLICT(db,store,key) DO UPDATE SET value=excluded.value"}else{"INSERT INTO entries(db,store,key,value) VALUES(?,?,?,?)"},&[&tx.name,store,&key,value])?;},
                    "clear"=>{tx.database.query("DELETE FROM entries WHERE db=? AND store=?",&[&tx.name,store])?;},
                    _=>bail!("Unknown browser storage operation: {op}"),
                }Ok(Value::Null)
            }
        }
    }
}
