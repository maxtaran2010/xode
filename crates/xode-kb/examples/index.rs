//! Index a folder as a Library source and run a text search.
//! Usage: cargo run -p xode-kb --example index -- <folder> [query...]
use std::path::Path;
use std::time::{Duration, Instant};
use xode_kb::{EmbedHub, Kb, KbStore, Layer, SearchOpts, Sel};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = args.first().expect("folder arg");
    let query = if args.len() > 1 { args[1..].join(" ") } else { "authentication bypass".into() };
    let tmp = std::env::temp_dir().join(format!("xode-kb-index-{}", std::process::id()));
    let cfg = xode_core::config::Knowledge { embedder: "off".into(), ..Default::default() };
    let hub = EmbedHub::new(&xode_core::config::Config { knowledge: cfg.clone(), ..Default::default() });
    let g = KbStore::open('g', &tmp.join("g.db"), &tmp.join("mem"), Layer::Memory, hub, &cfg).unwrap();
    let src = g.add_source(Layer::Library, Path::new(folder), "security").unwrap();
    let kb = Kb { global: Some(g.clone()), project: None };
    let t = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let s = kb.sources().into_iter().find(|s| s.key == src.key).unwrap();
        let stage = xode_kb::store::KbStore::generation; // touch to avoid unused
        let _ = stage;
        if Instant::now() > deadline {
            println!("timeout; indexed {} notes", s.notes);
            break;
        }
        // Consider done when notes stop growing for a beat.
        std::thread::sleep(Duration::from_millis(400));
        let s2 = kb.sources().into_iter().find(|x| x.key == src.key).unwrap();
        if s2.notes > 0 && s2.notes == s.notes {
            println!("indexed {} notes / {} chunks in {:?}", s2.notes, s2.chunks, t.elapsed());
            break;
        }
    }
    let opts = SearchOpts { k: 6, text_only: true, ..Default::default() };
    let t = Instant::now();
    let hits = kb.search(&Sel::default(), &query, &opts);
    println!("\nsearch \"{query}\" -> {} hits in {:?}", hits.len(), t.elapsed());
    for h in &hits {
        println!("  {} {}  ~{}tok  {}", h.id, h.title, h.tokens, h.snippet.chars().take(90).collect::<String>());
    }
    let gr = g.graph(&[src.id], 5000);
    let dangling = g.dangling_links(&[src.id], 100000).len();
    println!("\ngraph: {} nodes, {} resolved links, {} dangling", gr.nodes.len(), gr.edges.len(), dangling);
    for q in ["Totolink", "notarization", "CVE-2024-0012"] {
        let h = kb.search(&Sel::default(), q, &SearchOpts { k: 3, text_only: true, ..Default::default() });
        println!("  fts \"{q}\": {}", h.iter().map(|x| x.title.clone()).collect::<Vec<_>>().join(", "));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
