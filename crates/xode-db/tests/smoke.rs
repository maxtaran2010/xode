use futures::executor::block_on;

#[test]
fn smoke() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        let db = turso::Builder::new_local(p.to_str().unwrap()).experimental_index_method(true).build().await.unwrap();
        let c = db.connect().unwrap();
        c.execute_batch("CREATE TABLE ch(id INTEGER PRIMARY KEY, text TEXT, emb BLOB, bit BLOB); CREATE INDEX ch_fts ON ch USING fts(text);").await.unwrap();
        for (i, t, v) in [(1, "rust borrow checker lifetimes", "[1,0,0,0]"), (2, "python asyncio event loop", "[0,1,0,0]"), (3, "rust async tokio runtime", "[0.7,0.7,0,0]")] {
            c.execute("INSERT INTO ch(id,text,emb,bit) VALUES(?1,?2,vector32(?3),vector1bit(?3))", (i, t, v)).await.unwrap();
        }
        let mut r = c.query("SELECT id, fts_score(text, 'rust') s FROM ch WHERE fts_match(text, 'rust') ORDER BY s DESC", ()).await.unwrap();
        let mut ids = vec![];
        while let Some(row) = r.next().await.unwrap() { ids.push(row.get::<i64>(0).unwrap()); }
        println!("fts {ids:?}");
        let mut r = c.query("SELECT id, vector_distance_cos(emb, vector32('[0.9,0.1,0,0]')) d FROM ch ORDER BY d LIMIT 2", ()).await.unwrap();
        while let Some(row) = r.next().await.unwrap() { println!("vec {:?} {:?}", row.get_value(0), row.get_value(1)); }
        let mut r = c.query("SELECT id, vector_distance_cos(bit, vector1bit('[0.9,0.1,0,0]')) d FROM ch ORDER BY d LIMIT 3", ()).await.unwrap();
        while let Some(row) = r.next().await.unwrap() { println!("bit {:?} {:?}", row.get_value(0), row.get_value(1)); }
        let mut r = c.query("SELECT length(emb), length(bit), fts_highlight(text,'<','>','rust') FROM ch WHERE id=1", ()).await.unwrap();
        while let Some(row) = r.next().await.unwrap() { println!("len {:?} {:?} {:?}", row.get_value(0), row.get_value(1), row.get_value(2)); }
    });
}
