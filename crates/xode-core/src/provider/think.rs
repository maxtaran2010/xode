/// Streaming splitter for `<think>...</think>` tags embedded in content.
#[derive(Default)]
pub struct ThinkSplitter {
    in_think: bool,
    pending: String,
    seen_any: bool,
}

pub enum Chunk {
    Text(String),
    Think(String),
}

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

impl ThinkSplitter {
    pub fn push(&mut self, s: &str) -> Vec<Chunk> {
        self.pending.push_str(s);
        let mut out = vec![];
        loop {
            let tag = if self.in_think { CLOSE } else { OPEN };
            if let Some(i) = self.pending.find(tag) {
                let before: String = self.pending[..i].to_string();
                self.emit(&mut out, before);
                self.pending.drain(..i + tag.len());
                self.in_think = !self.in_think;
                continue;
            }
            // Keep a possible partial tag at the end.
            let keep = partial_suffix(&self.pending, tag);
            let cut = self.pending.len() - keep;
            if cut > 0 {
                let s: String = self.pending[..cut].to_string();
                self.emit(&mut out, s);
                self.pending.drain(..cut);
            }
            break;
        }
        out
    }

    fn emit(&mut self, out: &mut Vec<Chunk>, s: String) {
        if s.is_empty() {
            return;
        }
        if self.in_think {
            out.push(Chunk::Think(s));
        } else {
            // Drop leading whitespace right after a think block.
            let s = if !self.seen_any { s.trim_start().to_string() } else { s };
            if s.is_empty() {
                return;
            }
            self.seen_any = true;
            out.push(Chunk::Text(s));
        }
    }

    pub fn finish(&mut self) -> Vec<Chunk> {
        let mut out = vec![];
        let s = std::mem::take(&mut self.pending);
        self.emit(&mut out, s);
        out
    }
}

fn partial_suffix(s: &str, tag: &str) -> usize {
    let max = tag.len().min(s.len());
    for n in (1..=max).rev() {
        if s.is_char_boundary(s.len() - n) && tag.starts_with(&s[s.len() - n..]) {
            return n;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(chunks: &[&str]) -> (String, String) {
        let mut sp = ThinkSplitter::default();
        let (mut t, mut k) = (String::new(), String::new());
        let mut all = vec![];
        for c in chunks {
            all.extend(sp.push(c));
        }
        all.extend(sp.finish());
        for c in all {
            match c {
                Chunk::Text(s) => t.push_str(&s),
                Chunk::Think(s) => k.push_str(&s),
            }
        }
        (t, k)
    }
    #[test]
    fn split_across_chunks() {
        let (t, k) = run(&["<thi", "nk>abc</th", "ink>\n\nhello"]);
        assert_eq!(k, "abc");
        assert_eq!(t, "hello");
    }
    #[test]
    fn no_tags() {
        let (t, k) = run(&["hello <b>", " world"]);
        assert_eq!(t, "hello <b> world");
        assert_eq!(k, "");
    }
}
