//! Well-Known URIs ([RFC 8615]).
//!
//! Reserves the `/.well-known/` path prefix on HTTP/HTTPS origins for
//! service-metadata endpoints; clients GET `/.well-known/{service}` and
//! typically follow a 3xx redirect to the actual service URL.
//!
//! Common service names used by Pimalaya:
//!
//! | Service name | Protocol | RFC |
//! |--------------|----------|-----|
//! | `caldav`    | CalDAV   | RFC 6764 |
//! | `carddav`   | CardDAV  | RFC 6764 |
//! | `oauth-authorization-server` | OAuth 2.0 | RFC 8414 |
//!
//! [RFC 8615]: https://www.rfc-editor.org/rfc/rfc8615

pub mod well_known;
