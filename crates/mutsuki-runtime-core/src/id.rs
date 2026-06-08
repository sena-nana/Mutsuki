pub trait IdSource {
    fn next_id(&mut self, prefix: &str) -> String;
}

#[derive(Clone, Debug, Default)]
pub struct SequentialIdSource {
    next: u64,
}

impl SequentialIdSource {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdSource for SequentialIdSource {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{:026}", self.next)
    }
}
