pub enum Chunk {
    Text(String),
    ParagraphBreak,
}

#[derive(Default)]
pub struct ProseAssembler;

impl ProseAssembler {
    pub fn new() -> Self {
        Self
    }

    pub fn feed(&mut self, _delta: &str, _index: u32, _is_final: bool) -> Vec<Chunk> {
        Vec::new()
    }
}
