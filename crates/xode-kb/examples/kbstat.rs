use xode_db::Connection;
fn try_q(c: &Connection, sql: &str) {
    match c.query_row(sql, (), |r| r.get::<i64>(0)) {
        Ok(n) => println!("  {sql}  => {n}"),
        Err(e) => println!("  {sql}  => ERROR: {e}"),
    }
}
fn main() {
    let p = std::env::args().nth(1).unwrap_or_else(|| xode_core::config::data_dir().join("kb.db").to_string_lossy().into());
    let c = Connection::open_shared(std::path::Path::new(&p)).expect("open");
    println!("db: {p} (ro={})", c.is_read_only());
    try_q(&c, "SELECT COUNT(*) FROM chunks");
    try_q(&c, "SELECT COUNT(*) FROM chunks WHERE emb IS NULL");
    try_q(&c, "SELECT COUNT(*) FROM chunks WHERE emb IS NOT NULL");
    try_q(&c, "SELECT COUNT(emb) FROM chunks");
    try_q(&c, "SELECT COUNT(*) FROM chunks WHERE bits IS NOT NULL");
    try_q(&c, "SELECT COUNT(*) FROM chunks WHERE length(emb) > 0");
}
