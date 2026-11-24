pub trait TeamRoster: Send + Sync {
    fn prompt_block(&self) -> Option<String>;
}
