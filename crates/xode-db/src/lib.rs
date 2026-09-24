//! Embedded database layer: a small synchronous, rusqlite-style facade over Turso
//! (SQLite-compatible, with native vectors and full-text search).
//!
//! Local Turso I/O completes synchronously, so each call is driven with `block_on`;
//! callers stay sync (index threads, `Outliner`) and never need a runtime.

use futures::executor::block_on;
use std::path::Path;

pub use turso::{IntoParams, Value};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("query returned no rows")]
    NoRows,
    #[error("{0}")]
    Db(#[from] turso::Error),
    #[error("{0}")]
    Conv(String),
}

impl Error {
    /// The file is locked by another process.
    pub fn is_locked(&self) -> bool {
        matches!(self, Error::Db(e) if e.to_string().to_ascii_lowercase().contains("locked by another process"))
    }
}

/// `params![a, b, c]` → `Vec<Value>` from anything implementing [`ToValue`].
#[macro_export]
macro_rules! params {
    () => { ::std::vec::Vec::<$crate::Value>::new() };
    ($($x:expr),+ $(,)?) => { vec![$($crate::ToValue::to_value(&$x)),+] };
}

pub trait ToValue {
    fn to_value(&self) -> Value;
}

macro_rules! int_to_value {
    ($($t:ty),*) => {$(
        impl ToValue for $t {
            fn to_value(&self) -> Value { Value::Integer(*self as i64) }
        }
    )*};
}
int_to_value!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl ToValue for bool {
    fn to_value(&self) -> Value {
        Value::Integer(*self as i64)
    }
}
impl ToValue for f32 {
    fn to_value(&self) -> Value {
        Value::Real(*self as f64)
    }
}
impl ToValue for f64 {
    fn to_value(&self) -> Value {
        Value::Real(*self)
    }
}
impl ToValue for str {
    fn to_value(&self) -> Value {
        Value::Text(self.to_string())
    }
}
impl ToValue for String {
    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }
}
impl ToValue for [u8] {
    fn to_value(&self) -> Value {
        Value::Blob(self.to_vec())
    }
}
impl ToValue for Vec<u8> {
    fn to_value(&self) -> Value {
        Value::Blob(self.clone())
    }
}
impl ToValue for Value {
    fn to_value(&self) -> Value {
        self.clone()
    }
}
impl<T: ToValue + ?Sized> ToValue for &T {
    fn to_value(&self) -> Value {
        (**self).to_value()
    }
}
impl<T: ToValue> ToValue for Option<T> {
    fn to_value(&self) -> Value {
        self.as_ref().map(|v| v.to_value()).unwrap_or(Value::Null)
    }
}

/// Column value conversion, lenient like SQLite's own type affinity.
pub trait FromValue: Sized {
    fn from_value(v: Value) -> Result<Self>;
}

fn conv<T>(want: &str, v: &Value) -> Result<T> {
    Err(Error::Conv(format!("expected {want}, got {v:?}")))
}

macro_rules! int_from_value {
    ($($t:ty),*) => {$(
        impl FromValue for $t {
            fn from_value(v: Value) -> Result<Self> {
                match v {
                    Value::Integer(i) => Ok(i as $t),
                    Value::Real(f) => Ok(f as $t),
                    Value::Null => Ok(0 as $t),
                    Value::Text(ref s) => s.trim().parse::<$t>().or_else(|_| conv("integer", &v)),
                    _ => conv("integer", &v),
                }
            }
        }
    )*};
}
int_from_value!(i32, i64, u32, u64, usize, f32, f64);

impl FromValue for bool {
    fn from_value(v: Value) -> Result<Self> {
        Ok(i64::from_value(v)? != 0)
    }
}
impl FromValue for String {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Text(s) => Ok(s),
            Value::Integer(i) => Ok(i.to_string()),
            Value::Real(f) => Ok(f.to_string()),
            Value::Null => Ok(String::new()),
            Value::Blob(b) => String::from_utf8(b).map_err(|e| Error::Conv(e.to_string())),
        }
    }
}
impl FromValue for Vec<u8> {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Blob(b) => Ok(b),
            Value::Text(s) => Ok(s.into_bytes()),
            Value::Null => Ok(vec![]),
            _ => conv("blob", &v),
        }
    }
}
impl FromValue for Value {
    fn from_value(v: Value) -> Result<Self> {
        Ok(v)
    }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Null => Ok(None),
            v => T::from_value(v).map(Some),
        }
    }
}

pub struct Row(turso::Row);

impl Row {
    pub fn get<T: FromValue>(&self, idx: usize) -> Result<T> {
        T::from_value(self.0.get_value(idx)?)
    }
    pub fn value(&self, idx: usize) -> Result<Value> {
        Ok(self.0.get_value(idx)?)
    }
    pub fn len(&self) -> usize {
        self.0.column_count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub trait OptionalExt<T> {
    /// Maps `Error::NoRows` to `Ok(None)`.
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub struct Connection {
    conn: turso::Connection,
    _db: turso::Database,
    read_only: bool,
}

impl Connection {
    /// Open (or create) a database file with FTS / vector index methods enabled.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref().to_string_lossy().to_string();
        Self::build(turso::Builder::new_local(&p))
    }

    /// Open read-write, or read-only when another process holds the write lock
    /// (Turso locks the file per process; readers can still query it).
    pub fn open_shared(path: impl AsRef<Path>) -> Result<Self> {
        match Self::open(path.as_ref()) {
            Err(e) if e.is_locked() => {
                let p = path.as_ref().to_string_lossy().to_string();
                let mut c = Self::build(turso::Builder::new_local(&p).read_only(true))?;
                c.read_only = true;
                Ok(c)
            }
            r => r,
        }
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::build(turso::Builder::new_local(":memory:"))
    }

    fn build(b: turso::Builder) -> Result<Self> {
        let db = block_on(b.experimental_index_method(true).build())?;
        let conn = db.connect()?;
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let c = Self { conn, _db: db, read_only: false };
        // 256 MB page cache: the security KB is ~1.6 GB, and cold reads dominate search.
        let _ = c.execute_batch("PRAGMA cache_size=-262144");
        Ok(c)
    }

    /// A second connection to the same database (shares the in-process page cache and WAL).
    /// Used for reads that must not wait behind a writer holding the main connection.
    pub fn sibling(&self) -> Result<Self> {
        let conn = self._db.connect()?;
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let c = Self { conn, _db: self._db.clone(), read_only: self.read_only };
        let _ = c.execute_batch("PRAGMA cache_size=-262144");
        Ok(c)
    }

    /// True when opened by [`Connection::open_shared`] while another process owns the file.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64> {
        Ok(block_on(self.conn.execute(sql, params))?)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        Ok(block_on(self.conn.execute_batch(sql))?)
    }

    pub fn query_map<T>(&self, sql: &str, params: impl IntoParams, mut f: impl FnMut(&Row) -> Result<T>) -> Result<Vec<T>> {
        block_on(async {
            let mut rows = self.conn.query(sql, params).await?;
            let mut out = vec![];
            while let Some(r) = rows.next().await? {
                out.push(f(&Row(r))?);
            }
            Ok(out)
        })
    }

    /// Visit rows without collecting (stop early by returning `false`).
    pub fn for_each(&self, sql: &str, params: impl IntoParams, mut f: impl FnMut(&Row) -> bool) -> Result<()> {
        block_on(async {
            let mut rows = self.conn.query(sql, params).await?;
            while let Some(r) = rows.next().await? {
                if !f(&Row(r)) {
                    break;
                }
            }
            Ok(())
        })
    }

    pub fn query_row<T>(&self, sql: &str, params: impl IntoParams, f: impl FnOnce(&Row) -> Result<T>) -> Result<T> {
        block_on(async {
            let mut rows = self.conn.query(sql, params).await?;
            match rows.next().await? {
                Some(r) => f(&Row(r)),
                None => Err(Error::NoRows),
            }
        })
    }

    /// First column of the first row, if any.
    pub fn scalar<T: FromValue>(&self, sql: &str, params: impl IntoParams) -> Result<Option<T>> {
        self.query_row(sql, params, |r| r.get(0)).optional()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Run `f` inside a transaction; rolls back on error.
    pub fn transaction<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.execute_batch("BEGIN")?;
        match f(self) {
            Ok(v) => {
                self.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn user_version(&self) -> i64 {
        self.scalar::<i64>("PRAGMA user_version", ()).ok().flatten().unwrap_or(0)
    }

    pub fn set_user_version(&self, v: i64) -> Result<()> {
        self.execute_batch(&format!("PRAGMA user_version = {v}"))
    }
}

/// Little-endian f32 blob accepted by `vector32(?)` / stored in F32 columns.
pub fn f32_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

/// `[0.1,0.2,…]` text form for `vector32('…')` / `vector1bit('…')`.
pub fn vec_text(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{x:.6}"));
    }
    s.push(']');
    s
}
