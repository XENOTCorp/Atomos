//! RFC 9110 + common extensions. Unknown codes do not panic.
//! Domain: u16 in 100..=999. Criticality C1.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status(u16);

impl Status {
    pub const CONTINUE: Status = Status(100);
    pub const SWITCHING_PROTOCOLS: Status = Status(101);
    pub const PROCESSING: Status = Status(102);
    pub const EARLY_HINTS: Status = Status(103);
    pub const OK: Status = Status(200);
    pub const CREATED: Status = Status(201);
    pub const ACCEPTED: Status = Status(202);
    pub const NON_AUTHORITATIVE: Status = Status(203);
    pub const NO_CONTENT: Status = Status(204);
    pub const RESET_CONTENT: Status = Status(205);
    pub const PARTIAL_CONTENT: Status = Status(206);
    pub const MULTI_STATUS: Status = Status(207);
    pub const ALREADY_REPORTED: Status = Status(208);
    pub const IM_USED: Status = Status(226);
    pub const MULTIPLE_CHOICES: Status = Status(300);
    pub const MOVED_PERMANENTLY: Status = Status(301);
    pub const FOUND: Status = Status(302);
    pub const SEE_OTHER: Status = Status(303);
    pub const NOT_MODIFIED: Status = Status(304);
    pub const USE_PROXY: Status = Status(305);
    pub const TEMPORARY_REDIRECT: Status = Status(307);
    pub const PERMANENT_REDIRECT: Status = Status(308);
    pub const BAD_REQUEST: Status = Status(400);
    pub const UNAUTHORIZED: Status = Status(401);
    pub const PAYMENT_REQUIRED: Status = Status(402);
    pub const FORBIDDEN: Status = Status(403);
    pub const NOT_FOUND: Status = Status(404);
    pub const METHOD_NOT_ALLOWED: Status = Status(405);
    pub const NOT_ACCEPTABLE: Status = Status(406);
    pub const PROXY_AUTH_REQUIRED: Status = Status(407);
    pub const REQUEST_TIMEOUT: Status = Status(408);
    pub const CONFLICT: Status = Status(409);
    pub const GONE: Status = Status(410);
    pub const LENGTH_REQUIRED: Status = Status(411);
    pub const PRECONDITION_FAILED: Status = Status(412);
    pub const PAYLOAD_TOO_LARGE: Status = Status(413);
    pub const URI_TOO_LONG: Status = Status(414);
    pub const UNSUPPORTED_MEDIA_TYPE: Status = Status(415);
    pub const RANGE_NOT_SATISFIABLE: Status = Status(416);
    pub const EXPECTATION_FAILED: Status = Status(417);
    pub const IM_A_TEAPOT: Status = Status(418);
    pub const MISDIRECTED: Status = Status(421);
    pub const UNPROCESSABLE: Status = Status(422);
    pub const LOCKED: Status = Status(423);
    pub const FAILED_DEPENDENCY: Status = Status(424);
    pub const TOO_EARLY: Status = Status(425);
    pub const UPGRADE_REQUIRED: Status = Status(426);
    pub const PRECONDITION_REQUIRED: Status = Status(428);
    pub const TOO_MANY_REQUESTS: Status = Status(429);
    pub const REQUEST_HEADER_FIELDS_TOO_LARGE: Status = Status(431);
    pub const UNAVAILABLE_LEGAL: Status = Status(451);
    pub const INTERNAL_ERROR: Status = Status(500);
    pub const NOT_IMPLEMENTED: Status = Status(501);
    pub const BAD_GATEWAY: Status = Status(502);
    pub const SERVICE_UNAVAILABLE: Status = Status(503);
    pub const GATEWAY_TIMEOUT: Status = Status(504);
    pub const HTTP_VERSION_NOT_SUPPORTED: Status = Status(505);
    pub const VARIANT_ALSO_NEGOTIATES: Status = Status(506);
    pub const INSUFFICIENT_STORAGE: Status = Status(507);
    pub const LOOP_DETECTED: Status = Status(508);
    pub const NOT_EXTENDED: Status = Status(510);
    pub const NETWORK_AUTH_REQUIRED: Status = Status(511);

    pub const fn from_u16(n: u16) -> Self {
        Status(n)
    }

    pub const fn as_u16(self) -> u16 {
        self.0
    }

    pub fn phrase(self) -> &'static str {
        match self.0 {
            100 => "Continue",
            101 => "Switching Protocols",
            102 => "Processing",
            103 => "Early Hints",
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            203 => "Non-Authoritative Information",
            204 => "No Content",
            205 => "Reset Content",
            206 => "Partial Content",
            207 => "Multi-Status",
            208 => "Already Reported",
            226 => "IM Used",
            300 => "Multiple Choices",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            304 => "Not Modified",
            305 => "Use Proxy",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            402 => "Payment Required",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            406 => "Not Acceptable",
            407 => "Proxy Authentication Required",
            408 => "Request Timeout",
            409 => "Conflict",
            410 => "Gone",
            411 => "Length Required",
            412 => "Precondition Failed",
            413 => "Payload Too Large",
            414 => "URI Too Long",
            415 => "Unsupported Media Type",
            416 => "Range Not Satisfiable",
            417 => "Expectation Failed",
            418 => "I'm a teapot",
            421 => "Misdirected Request",
            422 => "Unprocessable Entity",
            423 => "Locked",
            424 => "Failed Dependency",
            425 => "Too Early",
            426 => "Upgrade Required",
            428 => "Precondition Required",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            451 => "Unavailable For Legal Reasons",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            505 => "HTTP Version Not Supported",
            506 => "Variant Also Negotiates",
            507 => "Insufficient Storage",
            508 => "Loop Detected",
            510 => "Not Extended",
            511 => "Network Authentication Required",
            _ => "Unknown",
        }
    }
}

impl From<u16> for Status {
    fn from(n: u16) -> Self {
        Status(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_404() {
        assert_eq!(Status::from_u16(404).phrase(), "Not Found");
    }

    #[test]
    fn unknown_status_does_not_panic() {
        assert_eq!(Status::from_u16(599).as_u16(), 599);
        assert_eq!(Status::from_u16(599).phrase(), "Unknown");
    }

    #[test]
    fn every_listed_code_has_nonempty_phrase() {
        const CODES: &[u16] = &[
            100, 101, 102, 103, 200, 201, 202, 203, 204, 205, 206, 207, 208, 226, 300, 301, 302,
            303, 304, 305, 307, 308, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411,
            412, 413, 414, 415, 416, 417, 418, 421, 422, 423, 424, 425, 426, 428, 429, 431, 451,
            500, 501, 502, 503, 504, 505, 506, 507, 508, 510, 511,
        ];
        for &c in CODES {
            let p = Status::from_u16(c).phrase();
            assert!(!p.is_empty(), "{c}");
            assert_ne!(p, "Unknown", "{c}");
        }
    }
}
