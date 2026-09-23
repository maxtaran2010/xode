//! Multi-process check: run twice on the same path; the second opens read-only.
fn main() {
    let p = std::env::args().nth(1).expect("path");
    let c = xode_db::Connection::open_shared(&p).unwrap();
    if !c.is_read_only() {
        c.execute_batch("CREATE TABLE IF NOT EXISTS t(x INTEGER)").unwrap();
        c.execute("INSERT INTO t VALUES(1)", ()).unwrap();
    }
    println!("read_only={} count={:?}", c.is_read_only(), c.scalar::<i64>("SELECT COUNT(*) FROM t", ()));
    std::thread::sleep(std::time::Duration::from_secs(3));
}
