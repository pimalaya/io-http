#[derive(Clone, Debug, Default)]
pub struct Response {
    pub bytes: Vec<u8>,
    pub body_start: usize,
    pub body_length: usize,
}
