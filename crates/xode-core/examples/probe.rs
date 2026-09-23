//! cargo run -p xode-core --example probe -- http://host:8080
use xode_core::provider::{self, ChatRequest, StreamEvent};
use xode_core::{config::Generation, Message, ToolSpec};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args().nth(1).unwrap_or("http://127.0.0.1:8080".into());
    let g = xode_core::gateway::detect(&url, "").await?;
    println!("detected: {} kind={:?} models={:?}", g.name, g.kind, g.models);
    let t = xode_core::gateway::test(&g).await;
    println!("test: ok={} {}", t.ok, t.message);
    let req = ChatRequest {
        model: g.models[0].id.clone(),
        system: "Be brief.".into(),
        messages: vec![Message::user("List *.txt files using the glob tool.")],
        tools: vec![ToolSpec { name: "glob".into(), description: "find files".into(), parameters: serde_json::json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}) }],
        generation: Generation { reasoning_effort: String::new(), ..Default::default() },
        no_tools: false,
        max_tokens: Some(2000),
    };
    let p = provider::for_gateway(&g);
    let mut think = 0;
    let mut on = |e: StreamEvent| match e {
        StreamEvent::Thinking(t) => think += t.len(),
        StreamEvent::Text(t) => print!("{t}"),
        StreamEvent::ToolCallStart { name, .. } => println!("\n[call {name}]"),
        _ => {}
    };
    let c = p.complete(&req, &mut on, &Default::default()).await?;
    println!("\nthinking chars: {think}\nparts: {:?}\nmeta: {:?}\nfinish: {}", c.parts.iter().filter(|p| !matches!(p, xode_core::Part::Thinking{..})).collect::<Vec<_>>(), c.meta, c.finish_reason);
    Ok(())
}
