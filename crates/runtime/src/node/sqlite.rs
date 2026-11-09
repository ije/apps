use crate::host::Role;
use anyhow::{Context, Result, bail, ensure};
#[cfg(not(target_os = "macos"))]
pub(crate) use libsqlite3_sys as ffi;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    ptr,
    rc::Rc,
};

// WARNING: macOS uses the system SQLite, declared here, because libsqlite3-sys without `bundled` can link another copy
// that pkg-config finds.
#[cfg(target_os = "macos")]
#[allow(non_camel_case_types)]
pub(crate) mod ffi {
    use std::ffi::{c_char, c_int, c_uint, c_void};
    pub enum sqlite3 {}
    pub enum sqlite3_stmt {}
    pub enum sqlite3_backup {}
    type Destructor = Option<unsafe extern "C" fn(*mut c_void)>;
    type Authorizer = Option<
        unsafe extern "C" fn(
            *mut c_void,
            c_int,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
        ) -> c_int,
    >;
    type Callback = Option<
        unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
    >;
    #[allow(non_snake_case)]
    pub fn SQLITE_TRANSIENT() -> Destructor {
        Some(unsafe { std::mem::transmute::<isize, unsafe extern "C" fn(*mut c_void)>(-1) })
    }
    macro_rules! constants {
        ($($name:ident = $value:literal),*) => { $(pub const $name: c_int = $value;)* };
    }
    constants! {
        SQLITE_OK = 0, SQLITE_BUSY = 5, SQLITE_LOCKED = 6, SQLITE_ROW = 100, SQLITE_DONE = 101,
        SQLITE_INTEGER = 1, SQLITE_FLOAT = 2, SQLITE_TEXT = 3, SQLITE_BLOB = 4, SQLITE_NULL = 5,
        SQLITE_OPEN_READONLY = 0x1, SQLITE_OPEN_READWRITE = 0x2, SQLITE_OPEN_CREATE = 0x4,
        SQLITE_OPEN_NOMUTEX = 0x8000, SQLITE_OPEN_FULLMUTEX = 0x10000,
        SQLITE_DBCONFIG_ENABLE_FKEY = 1002, SQLITE_DBCONFIG_DEFENSIVE = 1010,
        SQLITE_DBCONFIG_DQS_DML = 1013, SQLITE_DBCONFIG_DQS_DDL = 1014,
        SQLITE_DENY = 1, SQLITE_IGNORE = 2,
        SQLITE_CREATE_INDEX = 1, SQLITE_CREATE_TABLE = 2, SQLITE_CREATE_TEMP_INDEX = 3,
        SQLITE_CREATE_TEMP_TABLE = 4, SQLITE_CREATE_TEMP_TRIGGER = 5, SQLITE_CREATE_TEMP_VIEW = 6,
        SQLITE_CREATE_TRIGGER = 7, SQLITE_CREATE_VIEW = 8, SQLITE_DELETE = 9, SQLITE_DROP_INDEX = 10,
        SQLITE_DROP_TABLE = 11, SQLITE_DROP_TEMP_INDEX = 12, SQLITE_DROP_TEMP_TABLE = 13,
        SQLITE_DROP_TEMP_TRIGGER = 14, SQLITE_DROP_TEMP_VIEW = 15, SQLITE_DROP_TRIGGER = 16,
        SQLITE_DROP_VIEW = 17, SQLITE_INSERT = 18, SQLITE_PRAGMA = 19, SQLITE_READ = 20,
        SQLITE_SELECT = 21, SQLITE_TRANSACTION = 22, SQLITE_UPDATE = 23, SQLITE_ATTACH = 24,
        SQLITE_DETACH = 25, SQLITE_ALTER_TABLE = 26, SQLITE_REINDEX = 27, SQLITE_ANALYZE = 28,
        SQLITE_CREATE_VTABLE = 29, SQLITE_DROP_VTABLE = 30, SQLITE_FUNCTION = 31,
        SQLITE_SAVEPOINT = 32, SQLITE_RECURSIVE = 33
    }
    pub const SQLITE_PREPARE_PERSISTENT: c_uint = 1;
    #[link(name = "sqlite3")]
    unsafe extern "C" {
        pub fn sqlite3_open_v2(
            name: *const c_char,
            db: *mut *mut sqlite3,
            flags: c_int,
            vfs: *const c_char,
        ) -> c_int;
        pub fn sqlite3_close(db: *mut sqlite3) -> c_int;
        pub fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
        pub fn sqlite3_errstr(code: c_int) -> *const c_char;
        pub fn sqlite3_extended_errcode(db: *mut sqlite3) -> c_int;
        pub fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
        pub fn sqlite3_db_config(db: *mut sqlite3, op: c_int, ...) -> c_int;
        pub fn sqlite3_set_authorizer(
            db: *mut sqlite3,
            callback: Authorizer,
            data: *mut c_void,
        ) -> c_int;
        pub fn sqlite3_exec(
            db: *mut sqlite3,
            sql: *const c_char,
            callback: Callback,
            data: *mut c_void,
            error: *mut *mut c_char,
        ) -> c_int;
        pub fn sqlite3_get_autocommit(db: *mut sqlite3) -> c_int;
        pub fn sqlite3_changes64(db: *mut sqlite3) -> i64;
        pub fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64;
        pub fn sqlite3_prepare_v2(
            db: *mut sqlite3,
            sql: *const c_char,
            size: c_int,
            statement: *mut *mut sqlite3_stmt,
            tail: *mut *const c_char,
        ) -> c_int;
        pub fn sqlite3_prepare_v3(
            db: *mut sqlite3,
            sql: *const c_char,
            size: c_int,
            flags: c_uint,
            statement: *mut *mut sqlite3_stmt,
            tail: *mut *const c_char,
        ) -> c_int;
        pub fn sqlite3_finalize(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_reset(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_step(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_clear_bindings(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_sql(statement: *mut sqlite3_stmt) -> *const c_char;
        pub fn sqlite3_expanded_sql(statement: *mut sqlite3_stmt) -> *mut c_char;
        pub fn sqlite3_free(data: *mut c_void);
        pub fn sqlite3_bind_parameter_count(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_bind_parameter_name(
            statement: *mut sqlite3_stmt,
            index: c_int,
        ) -> *const c_char;
        pub fn sqlite3_bind_null(statement: *mut sqlite3_stmt, index: c_int) -> c_int;
        pub fn sqlite3_bind_int64(statement: *mut sqlite3_stmt, index: c_int, value: i64) -> c_int;
        pub fn sqlite3_bind_double(statement: *mut sqlite3_stmt, index: c_int, value: f64)
        -> c_int;
        pub fn sqlite3_bind_text(
            statement: *mut sqlite3_stmt,
            index: c_int,
            value: *const c_char,
            size: c_int,
            free: Destructor,
        ) -> c_int;
        pub fn sqlite3_bind_blob(
            statement: *mut sqlite3_stmt,
            index: c_int,
            value: *const c_void,
            size: c_int,
            free: Destructor,
        ) -> c_int;
        pub fn sqlite3_column_count(statement: *mut sqlite3_stmt) -> c_int;
        pub fn sqlite3_column_type(statement: *mut sqlite3_stmt, index: c_int) -> c_int;
        pub fn sqlite3_column_name(statement: *mut sqlite3_stmt, index: c_int) -> *const c_char;
        pub fn sqlite3_column_decltype(statement: *mut sqlite3_stmt, index: c_int)
        -> *const c_char;
        pub fn sqlite3_column_database_name(
            statement: *mut sqlite3_stmt,
            index: c_int,
        ) -> *const c_char;
        pub fn sqlite3_column_table_name(
            statement: *mut sqlite3_stmt,
            index: c_int,
        ) -> *const c_char;
        pub fn sqlite3_column_origin_name(
            statement: *mut sqlite3_stmt,
            index: c_int,
        ) -> *const c_char;
        pub fn sqlite3_column_int64(statement: *mut sqlite3_stmt, index: c_int) -> i64;
        pub fn sqlite3_column_double(statement: *mut sqlite3_stmt, index: c_int) -> f64;
        pub fn sqlite3_column_text(statement: *mut sqlite3_stmt, index: c_int) -> *const u8;
        pub fn sqlite3_column_blob(statement: *mut sqlite3_stmt, index: c_int) -> *const c_void;
        pub fn sqlite3_column_bytes(statement: *mut sqlite3_stmt, index: c_int) -> c_int;
        pub fn sqlite3_backup_init(
            target: *mut sqlite3,
            target_name: *const c_char,
            source: *mut sqlite3,
            source_name: *const c_char,
        ) -> *mut sqlite3_backup;
        pub fn sqlite3_backup_step(backup: *mut sqlite3_backup, pages: c_int) -> c_int;
        pub fn sqlite3_backup_finish(backup: *mut sqlite3_backup) -> c_int;
        pub fn sqlite3_backup_remaining(backup: *mut sqlite3_backup) -> c_int;
        pub fn sqlite3_backup_pagecount(backup: *mut sqlite3_backup) -> c_int;
    }
}

pub(crate) type AuthorizerCallback = Rc<dyn Fn(u64, i32, [Option<String>; 4]) -> Result<i32>>;
struct Authorization {
    database: u64,
    callback: AuthorizerCallback,
}
unsafe extern "C" fn authorize(
    data: *mut std::ffi::c_void,
    action: i32,
    a: *const std::ffi::c_char,
    b: *const std::ffi::c_char,
    c: *const std::ffi::c_char,
    d: *const std::ffi::c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let data = unsafe { &*(data as *const Authorization) };
        let values = [a, b, c, d].map(|value| {
            if value.is_null() {
                None
            } else {
                Some(
                    unsafe { CStr::from_ptr(value) }
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        });
        (data.callback)(data.database, action, values)
            .ok()
            .filter(|value| {
                matches!(
                    *value,
                    ffi::SQLITE_OK | ffi::SQLITE_DENY | ffi::SQLITE_IGNORE
                )
            })
            .unwrap_or(ffi::SQLITE_DENY)
    }))
    .unwrap_or(ffi::SQLITE_DENY)
}

struct Backup {
    raw: *mut ffi::sqlite3_backup,
    _destination: Database,
    source: u64,
}
impl Drop for Backup {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_backup_finish(self.raw);
        }
    }
}

struct Database(*mut ffi::sqlite3);

impl Database {
    fn check(&self, code: i32) -> Result<()> {
        if matches!(code, ffi::SQLITE_OK | ffi::SQLITE_ROW | ffi::SQLITE_DONE) {
            return Ok(());
        }
        // WARNING: sqlite.js splits the message at U+0001 into message, errcode and errstr.
        let code = unsafe { ffi::sqlite3_extended_errcode(self.0) };
        bail!(
            "{}\u{1}{code}\u{1}{}",
            unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(self.0)) }.to_string_lossy(),
            unsafe { CStr::from_ptr(ffi::sqlite3_errstr(code)) }.to_string_lossy()
        )
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_close(self.0);
        }
    }
}

struct Statement {
    raw: *mut ffi::sqlite3_stmt,
    database: u64,
}

impl Drop for Statement {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_finalize(self.raw);
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    backups: HashMap<u64, Backup>,
    statements: HashMap<u64, Statement>,
    databases: HashMap<u64, Database>,
    authorizers: HashMap<u64, Box<Authorization>>,
    authorizer_callback: Option<AuthorizerCallback>,
    next_id: u64,
}

impl State {
    pub(crate) fn set_authorizer_callback(&mut self, callback: AuthorizerCallback) {
        self.authorizer_callback = Some(callback);
    }
    pub(crate) fn call(&mut self, role: Role, op: &str, args: &Value) -> Result<Value> {
        ensure!(
            !matches!(role, Role::Renderer(_)),
            "SQLite is unavailable in page scripts"
        );
        if op == "sqlite.constants" {
            return Ok(json!({
                "SQLITE_OK":ffi::SQLITE_OK,"SQLITE_DENY":ffi::SQLITE_DENY,"SQLITE_IGNORE":ffi::SQLITE_IGNORE,
                "SQLITE_CREATE_INDEX":ffi::SQLITE_CREATE_INDEX,"SQLITE_CREATE_TABLE":ffi::SQLITE_CREATE_TABLE,"SQLITE_CREATE_TEMP_INDEX":ffi::SQLITE_CREATE_TEMP_INDEX,"SQLITE_CREATE_TEMP_TABLE":ffi::SQLITE_CREATE_TEMP_TABLE,
                "SQLITE_CREATE_TEMP_TRIGGER":ffi::SQLITE_CREATE_TEMP_TRIGGER,"SQLITE_CREATE_TEMP_VIEW":ffi::SQLITE_CREATE_TEMP_VIEW,"SQLITE_CREATE_TRIGGER":ffi::SQLITE_CREATE_TRIGGER,"SQLITE_CREATE_VIEW":ffi::SQLITE_CREATE_VIEW,
                "SQLITE_DELETE":ffi::SQLITE_DELETE,"SQLITE_DROP_INDEX":ffi::SQLITE_DROP_INDEX,"SQLITE_DROP_TABLE":ffi::SQLITE_DROP_TABLE,"SQLITE_DROP_TEMP_INDEX":ffi::SQLITE_DROP_TEMP_INDEX,"SQLITE_DROP_TEMP_TABLE":ffi::SQLITE_DROP_TEMP_TABLE,
                "SQLITE_DROP_TEMP_TRIGGER":ffi::SQLITE_DROP_TEMP_TRIGGER,"SQLITE_DROP_TEMP_VIEW":ffi::SQLITE_DROP_TEMP_VIEW,"SQLITE_DROP_TRIGGER":ffi::SQLITE_DROP_TRIGGER,"SQLITE_DROP_VIEW":ffi::SQLITE_DROP_VIEW,
                "SQLITE_INSERT":ffi::SQLITE_INSERT,"SQLITE_PRAGMA":ffi::SQLITE_PRAGMA,"SQLITE_READ":ffi::SQLITE_READ,"SQLITE_SELECT":ffi::SQLITE_SELECT,"SQLITE_TRANSACTION":ffi::SQLITE_TRANSACTION,"SQLITE_UPDATE":ffi::SQLITE_UPDATE,
                "SQLITE_ATTACH":ffi::SQLITE_ATTACH,"SQLITE_DETACH":ffi::SQLITE_DETACH,"SQLITE_ALTER_TABLE":ffi::SQLITE_ALTER_TABLE,"SQLITE_REINDEX":ffi::SQLITE_REINDEX,"SQLITE_ANALYZE":ffi::SQLITE_ANALYZE,
                "SQLITE_CREATE_VTABLE":ffi::SQLITE_CREATE_VTABLE,"SQLITE_DROP_VTABLE":ffi::SQLITE_DROP_VTABLE,"SQLITE_FUNCTION":ffi::SQLITE_FUNCTION,"SQLITE_SAVEPOINT":ffi::SQLITE_SAVEPOINT,"SQLITE_RECURSIVE":ffi::SQLITE_RECURSIVE
            }));
        }
        // WARNING: Handles belong to this JSC context. Statements must be finalized before their database closes.
        unsafe {
            if op == "sqlite.open" {
                let path = CString::new(args[0].as_str().context("Expected database path")?)?;
                let options = &args[1];
                let flags = if options["readOnly"] == true {
                    ffi::SQLITE_OPEN_READONLY
                } else {
                    ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE
                };
                let mut raw = ptr::null_mut();
                let code = ffi::sqlite3_open_v2(
                    path.as_ptr(),
                    &mut raw,
                    flags | ffi::SQLITE_OPEN_NOMUTEX,
                    ptr::null(),
                );
                ensure!(!raw.is_null(), "Cannot allocate SQLite database");
                let database = Database(raw);
                database.check(code)?;
                database.check(ffi::sqlite3_busy_timeout(
                    raw,
                    options["timeout"].as_i64().unwrap_or(0).try_into()?,
                ))?;
                for (setting, enabled) in [
                    (
                        ffi::SQLITE_DBCONFIG_ENABLE_FKEY,
                        options["enableForeignKeyConstraints"] != false,
                    ),
                    (
                        ffi::SQLITE_DBCONFIG_DQS_DML,
                        options["enableDoubleQuotedStringLiterals"] == true,
                    ),
                    (
                        ffi::SQLITE_DBCONFIG_DQS_DDL,
                        options["enableDoubleQuotedStringLiterals"] == true,
                    ),
                    (
                        ffi::SQLITE_DBCONFIG_DEFENSIVE,
                        options["defensive"] != false,
                    ),
                ] {
                    database.check(ffi::sqlite3_db_config(
                        raw,
                        setting,
                        i32::from(enabled),
                        ptr::null_mut::<i32>(),
                    ))?;
                }
                // WARNING: the macOS SQLite turns recursive triggers on by default, and Node's doesn't.
                database.check(ffi::sqlite3_exec(
                    raw,
                    c"PRAGMA recursive_triggers = OFF".as_ptr(),
                    None,
                    ptr::null_mut(),
                    ptr::null_mut(),
                ))?;
                self.next_id += 1;
                self.databases.insert(self.next_id, database);
                return Ok(json!(self.next_id));
            }
            let id = args[0].as_u64().context("Invalid SQLite handle")?;
            if op == "sqlite.backup.step" {
                let backup = self.backups.get(&id).context("Backup is closed")?;
                let pages = i32::try_from(args[1].as_i64().unwrap_or(100))?;
                ensure!(pages > 0, "Backup page count must be positive");
                let code = ffi::sqlite3_backup_step(backup.raw, pages);
                ensure!(
                    matches!(
                        code,
                        ffi::SQLITE_OK | ffi::SQLITE_DONE | ffi::SQLITE_BUSY | ffi::SQLITE_LOCKED
                    ),
                    "SQLite backup failed: {code}"
                );
                return Ok(
                    json!({"done":code == ffi::SQLITE_DONE,"remainingPages":ffi::sqlite3_backup_remaining(backup.raw),"totalPages":ffi::sqlite3_backup_pagecount(backup.raw)}),
                );
            }
            if op == "sqlite.backup.finish" {
                self.backups.remove(&id);
                return Ok(Value::Null);
            }
            if matches!(op, "sqlite.close" | "sqlite.releaseDatabase") {
                ensure!(
                    op == "sqlite.releaseDatabase" || self.databases.contains_key(&id),
                    "Database is not open"
                );
                self.statements
                    .retain(|_, statement| statement.database != id);
                self.backups.retain(|_, backup| backup.source != id);
                self.databases.remove(&id);
                self.authorizers.remove(&id);
                return Ok(Value::Null);
            }
            if op == "sqlite.authorizer" {
                let database = self.databases.get(&id).context("Database is not open")?;
                if args[1] == true {
                    let mut authorization = Box::new(Authorization {
                        database: id,
                        callback: self
                            .authorizer_callback
                            .clone()
                            .context("SQLite authorizer bridge is unavailable")?,
                    });
                    database.check(ffi::sqlite3_set_authorizer(
                        database.0,
                        Some(authorize),
                        (&mut *authorization as *mut Authorization).cast(),
                    ))?;
                    self.authorizers.insert(id, authorization);
                } else {
                    database.check(ffi::sqlite3_set_authorizer(
                        database.0,
                        None,
                        ptr::null_mut(),
                    ))?;
                    self.authorizers.remove(&id);
                }
                return Ok(Value::Null);
            }
            if op == "sqlite.backup.start" {
                let source = self.databases.get(&id).context("Database is not open")?;
                let path = CString::new(args[1].as_str().context("Expected backup path")?)?;
                let mut raw = ptr::null_mut();
                let code = ffi::sqlite3_open_v2(
                    path.as_ptr(),
                    &mut raw,
                    ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_NOMUTEX,
                    ptr::null(),
                );
                ensure!(!raw.is_null(), "Cannot open backup database");
                let destination = Database(raw);
                destination.check(code)?;
                let source_name = CString::new(args[2].as_str().unwrap_or("main"))?;
                let target_name = CString::new(args[3].as_str().unwrap_or("main"))?;
                let raw = ffi::sqlite3_backup_init(
                    destination.0,
                    target_name.as_ptr(),
                    source.0,
                    source_name.as_ptr(),
                );
                ensure!(!raw.is_null(), "Cannot initialize SQLite backup");
                self.next_id += 1;
                self.backups.insert(
                    self.next_id,
                    Backup {
                        raw,
                        _destination: destination,
                        source: id,
                    },
                );
                return Ok(json!(self.next_id));
            }
            if op == "sqlite.releaseStatement" {
                self.statements.remove(&id);
                return Ok(Value::Null);
            }
            if matches!(
                op,
                "sqlite.exec" | "sqlite.prepare" | "sqlite.isTransaction"
            ) {
                let database = self.databases.get(&id).context("Database is not open")?;
                if op == "sqlite.isTransaction" {
                    return Ok(json!(ffi::sqlite3_get_autocommit(database.0) == 0));
                }
                let sql = CString::new(args[1].as_str().context("Expected SQL string")?)?;
                if op == "sqlite.exec" {
                    database.check(ffi::sqlite3_exec(
                        database.0,
                        sql.as_ptr(),
                        None,
                        ptr::null_mut(),
                        ptr::null_mut(),
                    ))?;
                    return Ok(Value::Null);
                }
                let mut raw = ptr::null_mut();
                let code = ffi::sqlite3_prepare_v2(
                    database.0,
                    sql.as_ptr(),
                    -1,
                    &mut raw,
                    ptr::null_mut(),
                );
                let statement = Statement { raw, database: id };
                database.check(code)?;
                ensure!(!raw.is_null(), "SQL contains no statement");
                let parameters: Vec<_> = (1..=ffi::sqlite3_bind_parameter_count(raw))
                    .map(|index| {
                        let name = ffi::sqlite3_bind_parameter_name(raw, index);
                        if name.is_null() {
                            Value::Null
                        } else {
                            json!(CStr::from_ptr(name).to_string_lossy())
                        }
                    })
                    .collect();
                let source = CStr::from_ptr(ffi::sqlite3_sql(raw))
                    .to_string_lossy()
                    .into_owned();
                self.next_id += 1;
                self.statements.insert(self.next_id, statement);
                return Ok(
                    json!({"id": self.next_id, "parameters": parameters, "sourceSQL": source}),
                );
            }
            let statement = self
                .statements
                .get(&id)
                .context("Statement is closed or its database is not open")?;
            let database = self
                .databases
                .get(&statement.database)
                .context("Database is not open")?;
            let raw = statement.raw;
            let bind = |bindings: &[Value]| -> Result<()> {
                ffi::sqlite3_reset(raw);
                ffi::sqlite3_clear_bindings(raw);
                for binding in bindings.chunks(3) {
                    let [index, kind, value] = binding else {
                        bail!("Invalid SQLite parameter");
                    };
                    let index: i32 = index
                        .as_i64()
                        .context("Invalid parameter index")?
                        .try_into()?;
                    let code = match kind.as_u64().context("Invalid parameter type")? {
                        0 => ffi::sqlite3_bind_null(raw, index),
                        1 => ffi::sqlite3_bind_int64(
                            raw,
                            index,
                            value.as_str().context("Expected integer")?.parse()?,
                        ),
                        2 => ffi::sqlite3_bind_double(
                            raw,
                            index,
                            match value.as_f64() {
                                Some(value) => value,
                                None => value.as_str().context("Expected number")?.parse()?,
                            },
                        ),
                        3 => {
                            let text = value.as_str().context("Expected string")?;
                            ffi::sqlite3_bind_text(
                                raw,
                                index,
                                text.as_ptr().cast(),
                                text.len().try_into()?,
                                ffi::SQLITE_TRANSIENT(),
                            )
                        }
                        4 => {
                            let bytes = crate::node::bytes(value)?;
                            ffi::sqlite3_bind_blob(
                                raw,
                                index,
                                bytes.as_ptr().cast(),
                                bytes.len().try_into()?,
                                ffi::SQLITE_TRANSIENT(),
                            )
                        }
                        _ => bail!("Unsupported SQLite parameter type"),
                    };
                    database.check(code)?;
                }
                Ok(())
            };
            let rest = |from: usize| {
                args.as_array()
                    .and_then(|args| args.get(from..))
                    .unwrap_or_default()
            };
            match op {
                "sqlite.columns" => {
                    let columns: Vec<_> = (0..ffi::sqlite3_column_count(raw)).map(|index| {
                        let text = |value: *const std::ffi::c_char| if value.is_null() { Value::Null } else { json!(CStr::from_ptr(value).to_string_lossy()) };
                        json!({"name":text(ffi::sqlite3_column_name(raw,index)),"column":text(ffi::sqlite3_column_origin_name(raw,index)),"table":text(ffi::sqlite3_column_table_name(raw,index)),"database":text(ffi::sqlite3_column_database_name(raw,index)),"type":text(ffi::sqlite3_column_decltype(raw,index))})
                    }).collect();
                    Ok(json!(columns))
                }
                "sqlite.reset" => {
                    database.check(ffi::sqlite3_reset(raw))?;
                    Ok(Value::Null)
                }
                "sqlite.expandedSQL" => {
                    let text = ffi::sqlite3_expanded_sql(raw);
                    ensure!(!text.is_null(), "Cannot expand SQL");
                    let result = json!(CStr::from_ptr(text).to_string_lossy());
                    ffi::sqlite3_free(text.cast());
                    Ok(result)
                }
                "sqlite.run" => {
                    bind(rest(1))?;
                    let code = ffi::sqlite3_step(raw);
                    let result = database.check(code);
                    let reset = ffi::sqlite3_reset(raw);
                    result?;
                    database.check(reset)?;
                    let number = |value: i64| {
                        if value.unsigned_abs() < 1 << 53 {
                            json!(value)
                        } else {
                            json!(value.to_string())
                        }
                    };
                    Ok(json!([
                        number(ffi::sqlite3_changes64(database.0)),
                        number(ffi::sqlite3_last_insert_rowid(database.0)),
                    ]))
                }
                "sqlite.rows" => {
                    let mode = args[1].as_str().context("Expected query mode")?;
                    let bigints = args[2] == true;
                    if args[3] == true {
                        bind(rest(4))?;
                    }
                    let result = (|| -> Result<Value> {
                        let mut code = ffi::sqlite3_step(raw);
                        database.check(code)?;
                        let columns: Vec<_> = (0..ffi::sqlite3_column_count(raw))
                            .map(|index| {
                                CStr::from_ptr(ffi::sqlite3_column_name(raw, index))
                                    .to_string_lossy()
                                    .into_owned()
                            })
                            .collect();
                        let mut rows = Vec::new();
                        while code == ffi::SQLITE_ROW {
                            let mut row = Vec::with_capacity(columns.len());
                            for index in 0..columns.len() as i32 {
                                row.push(match ffi::sqlite3_column_type(raw, index) {
                                    ffi::SQLITE_NULL => Value::Null,
                                    ffi::SQLITE_INTEGER => {
                                        let value = ffi::sqlite3_column_int64(raw, index);
                                        if !bigints && value.unsigned_abs() < 1 << 53 {
                                            json!(value)
                                        } else {
                                            json!({"integer": value.to_string()})
                                        }
                                    }
                                    ffi::SQLITE_FLOAT => {
                                        let value = ffi::sqlite3_column_double(raw, index);
                                        if value.is_finite() {
                                            json!(value)
                                        } else {
                                            json!({"real": value.to_string()})
                                        }
                                    }
                                    ffi::SQLITE_TEXT | ffi::SQLITE_BLOB => {
                                        let text = ffi::sqlite3_column_type(raw, index)
                                            == ffi::SQLITE_TEXT;
                                        let data = if text {
                                            ffi::sqlite3_column_text(raw, index)
                                        } else {
                                            ffi::sqlite3_column_blob(raw, index).cast()
                                        };
                                        let size = ffi::sqlite3_column_bytes(raw, index) as usize;
                                        ensure!(
                                            !data.is_null() || size == 0,
                                            "Cannot read SQLite value"
                                        );
                                        let bytes = if size == 0 {
                                            &[]
                                        } else {
                                            std::slice::from_raw_parts(data, size)
                                        };
                                        if text {
                                            json!(String::from_utf8_lossy(bytes))
                                        } else {
                                            json!({"blob": crate::web::base64(bytes)})
                                        }
                                    }
                                    _ => bail!("Unsupported SQLite value"),
                                });
                            }
                            rows.push(row);
                            if mode != "all" {
                                break;
                            }
                            code = ffi::sqlite3_step(raw);
                            database.check(code)?;
                        }
                        Ok(json!({"columns": columns, "rows": rows}))
                    })();
                    let reset = if mode != "next"
                        || result.as_ref().map_or(true, |value| {
                            value["rows"].as_array().is_none_or(Vec::is_empty)
                        }) {
                        ffi::sqlite3_reset(raw)
                    } else {
                        ffi::SQLITE_OK
                    };
                    let value = result?;
                    database.check(reset)?;
                    Ok(value)
                }
                _ => bail!("Unsupported SQLite operation: {op}"),
            }
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.backups.clear();
        self.statements.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_cannot_open_or_access_databases() {
        let mut state = State::default();
        for op in [
            "sqlite.open",
            "sqlite.prepare",
            "sqlite.rows",
            "sqlite.close",
        ] {
            assert!(
                state
                    .call(Role::Renderer(1), op, &json!([":memory:"]))
                    .unwrap_err()
                    .to_string()
                    .contains("unavailable in page scripts")
            );
        }
    }
}
