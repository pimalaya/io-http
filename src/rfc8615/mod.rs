//! Well-Known URIs (RFC 8615).
//!
//! RFC 8615 reserves the `/.well-known/` path prefix on HTTP/HTTPS
//! origins for service-metadata endpoints. Clients send a GET request
//! to `/.well-known/{service}` and expect a 3xx redirect to the actual
//! service URL.
//!
//! Common service names used by Pimalaya:
//!
//! | Service name | Protocol | RFC |
//! |-------------|----------|-----|
//! | `caldav`    | CalDAV   | RFC 6764 |
//! | `carddav`   | CardDAV  | RFC 6764 |
//! | `oauth-authorization-server` | OAuth 2.0 | RFC 8414 |

pub mod well_known;
