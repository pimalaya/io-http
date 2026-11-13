//! URL building from path segments ([RFC 3986 §3.3]).
//!
//! [RFC 3986 §3.3]: https://www.rfc-editor.org/rfc/rfc3986#section-3.3

use alloc::string::{String, ToString};

use thiserror::Error;
use url::{ParseError, Url};

/// Failure causes when building a URL from path segments.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HttpUrlBuildError {
    /// The base URL does not parse, or cannot hold a path.
    #[error("HTTP build URL failed: invalid base URL `{1}`")]
    InvalidBaseUrl(#[source] ParseError, String),
    /// A segment is empty, a dot segment, or holds a tab or a line
    /// break.
    #[error("HTTP build URL failed: invalid path segment {0:?}")]
    InvalidSegment(String),
}

/// Builds a URL from a base and the path segments to append to it.
///
/// Each segment is percent-encoded on its own, so it never reads as a
/// path separator, a query or a fragment. A segment the URL would drop
/// or resolve is refused: an empty one, `.`, `..`, or one holding a tab
/// or a line break.
///
/// Refs: <https://www.rfc-editor.org/rfc/rfc3986#section-5.2.4>
pub fn http_url_build(
    base: &str,
    segments: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<Url, HttpUrlBuildError> {
    let invalid_base = |err| HttpUrlBuildError::InvalidBaseUrl(err, base.to_string());

    let mut url = Url::parse(base).map_err(invalid_base)?;

    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| invalid_base(ParseError::RelativeUrlWithCannotBeABaseBase))?;

        path.pop_if_empty();

        for segment in segments {
            let segment = segment.as_ref();

            // NOTE: the URL parser strips tabs and line breaks from its
            // input, `.\t.` would resolve as `..`
            if matches!(segment, "" | "." | "..") || segment.contains(['\t', '\n', '\r']) {
                return Err(HttpUrlBuildError::InvalidSegment(segment.to_string()));
            }

            path.push(segment);
        }
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use crate::rfc3986::path::*;

    const BASE: &str = "https://example.com/api/v1";

    #[test]
    fn segments_appended_to_base_path() {
        let url = http_url_build(BASE, ["lists", "42", "tasks"]).unwrap();
        assert_eq!(url.as_str(), "https://example.com/api/v1/lists/42/tasks");
    }

    #[test]
    fn trailing_slash_of_base_not_doubled() {
        let url = http_url_build("https://example.com/api/v1/", ["lists"]).unwrap();
        assert_eq!(url.as_str(), "https://example.com/api/v1/lists");

        let url = http_url_build("https://example.com", ["lists"]).unwrap();
        assert_eq!(url.as_str(), "https://example.com/lists");
    }

    #[test]
    fn base_query_kept() {
        let url = http_url_build("https://example.com/api?key=1", ["lists"]).unwrap();
        assert_eq!(url.as_str(), "https://example.com/api/lists?key=1");
    }

    #[test]
    fn no_segment_keeps_base() {
        let url = http_url_build(BASE, [""; 0]).unwrap();
        assert_eq!(url.as_str(), BASE);
    }

    #[test]
    fn separators_in_segment_are_encoded() {
        for (segment, encoded) in [
            ("a/b", "a%2Fb"),
            ("a?b=c", "a%3Fb=c"),
            ("a#b", "a%23b"),
            ("a b", "a%20b"),
            ("a\\b", "a%5Cb"),
            ("%2e%2e", "%252e%252e"),
            ("%2F", "%252F"),
            ("..a", "..a"),
            (
                "en.usa#holiday@group.v.calendar.google.com",
                "en.usa%23holiday@group.v.calendar.google.com",
            ),
        ] {
            let url = http_url_build(BASE, ["lists", segment, "tasks"]).unwrap();
            let expected = alloc::format!("{BASE}/lists/{encoded}/tasks");
            assert_eq!(url.as_str(), expected, "segment {segment:?}");
        }
    }

    #[test]
    fn segment_the_url_would_drop_or_resolve_errors() {
        for segment in ["", ".", "..", ".\t.", "\n..", ".\r", "\t", "a\nb"] {
            let err = http_url_build(BASE, ["lists", segment, "tasks"]).unwrap_err();
            assert_eq!(err, HttpUrlBuildError::InvalidSegment(segment.into()));
        }
    }

    #[test]
    fn base_without_path_errors() {
        for base in ["mailto:a@example.com", "not a url"] {
            let err = http_url_build(base, ["lists"]).unwrap_err();
            assert!(matches!(err, HttpUrlBuildError::InvalidBaseUrl(_, b) if b == base));
        }
    }
}
