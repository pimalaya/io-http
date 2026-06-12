//! HTTP status code ([RFC 9110 §15]).
//!
//! [RFC 9110 §15]: https://www.rfc-editor.org/rfc/rfc9110#section-15

use core::ops::Deref;

/// HTTP status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpStatusCode(pub u16);

impl HttpStatusCode {
    /// Returns `true` if the status code is in the `2xx` range.
    pub fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    /// Returns `true` if the status code is in the `3xx` range.
    pub fn is_redirection(self) -> bool {
        self.0 >= 300 && self.0 < 400
    }
}

impl Deref for HttpStatusCode {
    type Target = u16;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use crate::rfc9110::status::*;

    #[test]
    fn success_range() {
        assert!(HttpStatusCode(200).is_success());
        assert!(HttpStatusCode(204).is_success());
        assert!(HttpStatusCode(299).is_success());
        assert!(!HttpStatusCode(199).is_success());
        assert!(!HttpStatusCode(300).is_success());
    }

    #[test]
    fn redirection_range() {
        assert!(HttpStatusCode(300).is_redirection());
        assert!(HttpStatusCode(301).is_redirection());
        assert!(HttpStatusCode(399).is_redirection());
        assert!(!HttpStatusCode(299).is_redirection());
        assert!(!HttpStatusCode(400).is_redirection());
    }
}
