use xode_db::{f32_blob, params, vec_text, Connection, OptionalExt};

#[test]
fn vectors_fts_and_tx() {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(dir.path().join("t.db")).unwrap();
    c.execute_batch(
        "CREATE TABLE ch(id INTEGER PRIMARY KEY, text TEXT, emb BLOB, bit BLOB);
         CREATE INDEX ch_fts ON ch USING fts(text);",
    )
    .unwrap();
    let rows: [(i64, &str, [f32; 4]); 3] = [
        (1, "rust borrow checker lifetimes", [1.0, 0.0, 0.0, 0.0]),
        (2, "python asyncio event loop", [0.0, 1.0, 0.0, 0.0]),
        (3, "rust async tokio runtime", [0.7, 0.7, 0.0, 0.0]),
    ];
    c.transaction(|c| {
        for (i, t, v) in rows {
            c.execute(
                "INSERT INTO ch(id,text,emb,bit) VALUES(?1,?2,vector32(?3),vector1bit(?4))",
                params![i, t, f32_blob(&v), vec_text(&v)],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let ids = c
        .query_map("SELECT id FROM ch WHERE fts_match(text, ?1) ORDER BY fts_score(text, ?1) DESC", params!["rust"], |r| r.get::<i64>(0))
        .unwrap();
    assert_eq!(ids.len(), 2);
    let q = f32_blob(&[0.9, 0.1, 0.0, 0.0]);
    let near = c
        .query_map("SELECT id FROM ch ORDER BY vector_distance_cos(emb, vector32(?1)) LIMIT 1", params![q], |r| r.get::<i64>(0))
        .unwrap();
    assert_eq!(near, vec![1]);
    let none = c.query_row("SELECT id FROM ch WHERE id = 99", (), |r| r.get::<i64>(0)).optional().unwrap();
    assert!(none.is_none());
    // Failed transaction rolls back.
    let _ = c.transaction(|c| {
        c.execute("DELETE FROM ch", ())?;
        Err::<(), _>(xode_db::Error::NoRows)
    });
    assert_eq!(c.scalar::<i64>("SELECT COUNT(*) FROM ch", ()).unwrap(), Some(3));
    c.set_user_version(7).unwrap();
    assert_eq!(c.user_version(), 7);
    // Reopen: data and FTS index persist.
    drop(c);
    let c = Connection::open(dir.path().join("t.db")).unwrap();
    assert_eq!(c.scalar::<i64>("SELECT COUNT(*) FROM ch WHERE fts_match(text, 'tokio')", ()).unwrap(), Some(1));
    // Second connection to the same file in-process.
    let c2 = Connection::open(dir.path().join("t.db"));
    println!("second open: {:?}", c2.as_ref().err());
}
