//! HTTP status code (RFC 9110 §15).

use core::ops::Deref;

/// HTTP status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusCode(pub u16);

impl StatusCode {
    /// Returns `true` if the status code is in the `2xx` range.
    pub fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    /// Returns `true` if the status code is in the `3xx` range.
    pub fn is_redirection(self) -> bool {
        self.0 >= 300 && self.0 < 400
    }
}

impl Deref for StatusCode {
    type Target = u16;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_range() {
        assert!(StatusCode(200).is_success());
        assert!(StatusCode(204).is_success());
        assert!(StatusCode(299).is_success());
        assert!(!StatusCode(199).is_success());
        assert!(!StatusCode(300).is_success());
    }

    #[test]
    fn redirection_range() {
        assert!(StatusCode(300).is_redirection());
        assert!(StatusCode(301).is_redirection());
        assert!(StatusCode(399).is_redirection());
        assert!(!StatusCode(299).is_redirection());
        assert!(!StatusCode(400).is_redirection());
    }
}
