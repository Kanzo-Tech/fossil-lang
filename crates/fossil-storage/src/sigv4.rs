//! AWS Signature Version 4, query-string form — a presigned S3 request.
//!
//! <https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html>.
//! Only the payload-less form: `UNSIGNED-PAYLOAD` and `host` as the one signed
//! header, which is what makes the URL usable by a plain `fetch`.

// A private module: `unreachable_pub` wants `pub(crate)` and `redundant_pub_crate`
// the opposite, as in `fossil-cli`'s `system.rs`.
#![allow(clippy::redundant_pub_crate)]

use std::fmt::Write as _;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub(crate) struct Presign<'a> {
    pub(crate) method: &'a str,
    pub(crate) host: &'a str,
    /// Already encoded with [`encode`] (`keep_slash = true`), starting with `/`.
    pub(crate) path: &'a str,
    pub(crate) region: &'a str,
    pub(crate) key_id: &'a str,
    pub(crate) secret: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) now_ms: u64,
    pub(crate) expires_secs: u32,
}

/// The query string of the presigned request, `X-Amz-Signature` included.
pub(crate) fn presign(p: &Presign<'_>) -> String {
    let (date, amz_date) = timestamp(p.now_ms);
    let scope = format!("{date}/{}/s3/aws4_request", p.region);
    let mut query: Vec<(&str, String)> = vec![
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
        ("X-Amz-Credential", format!("{}/{scope}", p.key_id)),
        ("X-Amz-Date", amz_date.clone()),
        ("X-Amz-Expires", p.expires_secs.to_string()),
        ("X-Amz-SignedHeaders", "host".to_string()),
    ];
    if let Some(token) = p.token {
        query.push(("X-Amz-Security-Token", token.to_string()));
    }
    query.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k, false), encode(v, false)))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_request = format!(
        "{}\n{}\n{canonical_query}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
        p.method, p.path, p.host
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    );
    let mut key = hmac(format!("AWS4{}", p.secret).as_bytes(), date.as_bytes());
    for part in [p.region, "s3", "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, string_to_sign.as_bytes()));
    format!("{canonical_query}&X-Amz-Signature={signature}")
}

/// RFC 3986 percent-encoding as `SigV4` specifies it: every byte but the
/// unreserved set, and `/` too unless it separates a path.
pub(crate) fn encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(char::from(b));
        } else {
            write!(out, "%{b:02X}").expect("writing to a String never fails");
        }
    }
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("writing to a String never fails");
            s
        })
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` for a Unix time in milliseconds.
fn timestamp(now_ms: u64) -> (String, String) {
    let secs = now_ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil(i64::try_from(days).expect("a date after the year 292 million"));
    let date = format!("{y:04}{m:02}{d:02}");
    let time = format!("{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60);
    (date.clone(), format!("{date}T{time}Z"))
}

/// Days since 1970-01-01 → proleptic Gregorian `(year, month, day)` —
/// Howard Hinnant's `civil_from_days`.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let d = u32::try_from(day_of_year - (153 * mp + 2) / 5 + 1).expect("1..=31");
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).expect("1..=12");
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example of AWS's own page on query-string authentication.
    #[test]
    fn signs_the_aws_documented_example() {
        let query = presign(&Presign {
            method: "GET",
            host: "examplebucket.s3.amazonaws.com",
            path: "/test.txt",
            region: "us-east-1",
            key_id: "AKIAIOSFODNN7EXAMPLE",
            secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            token: None,
            now_ms: 1_369_353_600_000,
            expires_secs: 86_400,
        });
        assert_eq!(
            query,
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn the_session_token_is_signed_in_the_query() {
        let query = presign(&Presign {
            method: "PUT",
            host: "localhost:9000",
            path: "/b/k",
            region: "us-east-1",
            key_id: "K",
            secret: "S",
            token: Some("t/o+k=en"),
            now_ms: 0,
            expires_secs: 300,
        });
        assert!(
            query.contains("&X-Amz-Security-Token=t%2Fo%2Bk%3Den&"),
            "{query}"
        );
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(timestamp(0).1, "19700101T000000Z");
        assert_eq!(timestamp(951_782_400_000).1, "20000229T000000Z");
        assert_eq!(timestamp(1_790_000_000_123).1, "20260921T141320Z");
    }

    #[test]
    fn encoding_keeps_only_unreserved_and_path_slashes() {
        assert_eq!(encode("a b/c~é", true), "a%20b/c~%C3%A9");
        assert_eq!(encode("a/b", false), "a%2Fb");
    }
}
