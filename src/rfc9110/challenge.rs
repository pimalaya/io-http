//! HTTP authentication challenges ([RFC 9110 §11.6.1]).
//!
//! Parses `WWW-Authenticate` (and `Proxy-Authenticate`) header values
//! into their challenge list: each challenge names a scheme and
//! carries auth parameters. Challenges and their parameters share the
//! comma separator, so an element counts as a new challenge when it
//! does not read as a `name=value` parameter of the current one;
//! `token68` blobs are recognized and skipped.
//!
//! [RFC 9110 §11.6.1]: https://www.rfc-editor.org/rfc/rfc9110#section-11.6.1

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use crate::rfc9110::{headers::WWW_AUTHENTICATE, response::HttpResponse};

/// One authentication challenge: its scheme and auth parameters.
///
/// Scheme and parameter names are lowercased (both compare
/// case-insensitively on the wire); parameter values are unquoted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpChallenge {
    /// The lowercased authentication scheme (`basic`, `bearer`, ...).
    pub scheme: String,
    /// The auth parameters, names lowercased and values unquoted.
    pub params: Vec<(String, String)>,
}

impl HttpChallenge {
    /// Returns the first parameter matching `name` (case-insensitive).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl HttpResponse {
    /// Parses the challenges of every `WWW-Authenticate` header of the
    /// response, in order.
    pub fn challenges(&self) -> Vec<HttpChallenge> {
        let mut challenges = Vec::new();

        for (name, value) in &self.headers {
            if name.eq_ignore_ascii_case(WWW_AUTHENTICATE) {
                challenges.extend(parse_challenges(value));
            }
        }

        challenges
    }
}

/// Parses one header value into its challenge list.
pub fn parse_challenges(value: &str) -> Vec<HttpChallenge> {
    let mut challenges: Vec<HttpChallenge> = Vec::new();

    for element in split_unquoted_commas(value) {
        let element = element.trim();
        if element.is_empty() {
            continue;
        }

        match element.split_once(|c: char| c.is_ascii_whitespace()) {
            // A scheme followed by its first parameter or a token68
            // blob starts a new challenge.
            Some((scheme, rest)) => {
                challenges.push(HttpChallenge {
                    scheme: scheme.to_ascii_lowercase(),
                    params: Vec::new(),
                });
                push_param(&mut challenges, rest.trim_start());
            }
            // A lone element is a parameter of the current challenge
            // when it reads as `name=value`, a new bare challenge
            // otherwise.
            None => {
                if element.contains('=') {
                    push_param(&mut challenges, element);
                } else {
                    challenges.push(HttpChallenge {
                        scheme: element.to_ascii_lowercase(),
                        params: Vec::new(),
                    });
                }
            }
        }
    }

    challenges
}

/// Attaches one `name=value` element to the last challenge; a
/// `token68` blob (whose only `=` signs are trailing padding) and a
/// parameter preceding any scheme are skipped.
fn push_param(challenges: &mut [HttpChallenge], element: &str) {
    let Some(challenge) = challenges.last_mut() else {
        return;
    };
    if element.is_empty() {
        return;
    }

    let stripped = element.trim_end_matches('=');
    if !stripped.contains('=') {
        // A bare token or a token68 blob, not a parameter.
        return;
    }

    let Some((name, value)) = element.split_once('=') else {
        return;
    };
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return;
    }

    challenge.params.push((name, unquote(value.trim())));
}

/// Splits on the commas outside quoted strings (a quoted parameter
/// value may itself contain commas).
fn split_unquoted_commas(value: &str) -> Vec<&str> {
    let mut elements = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;

    for (index, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                elements.push(&value[start..index]);
                start = index + 1;
            }
            _ => (),
        }
    }
    elements.push(&value[start..]);

    elements
}

/// Strips the surrounding quotes of a quoted-string value and resolves
/// its escape sequences; a plain token passes through.
fn unquote(value: &str) -> String {
    let Some(quoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_string();
    };

    let mut unquoted = String::with_capacity(quoted.len());
    let mut escaped = false;

    for c in quoted.chars() {
        if escaped {
            unquoted.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            unquoted.push(c);
        }
    }

    unquoted
}

#[cfg(test)]
mod tests {

    use crate::rfc9110::challenge::parse_challenges;

    #[test]
    fn single_challenge_with_params() {
        let challenges = parse_challenges(
            r#"Bearer resource_metadata="https://api.example.com/.well-known/oauth-protected-resource""#,
        );

        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "bearer");
        assert_eq!(
            challenges[0].param("Resource_Metadata"),
            Some("https://api.example.com/.well-known/oauth-protected-resource"),
        );
    }

    #[test]
    fn multiple_challenges_share_the_comma_separator() {
        let challenges = parse_challenges(
            r#"Basic realm="carddav.example.com", Bearer, Digest qop="auth,auth-int""#,
        );

        assert_eq!(challenges.len(), 3);
        assert_eq!(challenges[0].scheme, "basic");
        assert_eq!(challenges[0].param("realm"), Some("carddav.example.com"));
        assert_eq!(challenges[1].scheme, "bearer");
        assert!(challenges[1].params.is_empty());
        // The quoted comma does not split the digest parameter.
        assert_eq!(challenges[2].param("qop"), Some("auth,auth-int"));
    }

    #[test]
    fn token68_blobs_are_not_parameters() {
        let challenges = parse_challenges("Negotiate YWJjZGVmZw==, Basic realm=dav");

        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "negotiate");
        assert!(challenges[0].params.is_empty());
        assert_eq!(challenges[1].param("realm"), Some("dav"));
    }

    #[test]
    fn quoted_escapes_resolve() {
        let challenges = parse_challenges(r#"Basic realm="a \"quoted\" realm""#);
        assert_eq!(challenges[0].param("realm"), Some(r#"a "quoted" realm"#));
    }
}
