// The write guard (#396): any page open in the browser can POST to
// 127.0.0.1, so a write needs the per-run token only our page carries, sent
// in X-Sd-Token from our own Origin. A custom header also makes another
// site's fetch ask first (a CORS preflight), which nothing here answers.

use std::fs::File;
use std::io::Read;

#[derive(Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub host: String,
    pub origin: Option<String>,
    pub token: Option<String>,
}

pub fn token() -> String {
    let mut b = [0u8; 16];
    File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("/dev/urandom reads");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A browser sends Origin on every POST: a missing one isn't our page.
pub fn refusal(req: &Request, port: u16, token: &str) -> Option<&'static str> {
    let ours = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    if !req.origin.as_ref().is_some_and(|o| ours.contains(o)) {
        Some("writes come from sd view's own page only\n")
    } else if req.token.as_deref() != Some(token) {
        Some("a write needs this run's token\n")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(origin: Option<&str>, token: Option<&str>) -> Request {
        Request { method: "POST".into(), origin: origin.map(Into::into), token: token.map(Into::into), ..Default::default() }
    }

    #[test]
    fn a_token_is_32_hex_and_new_each_time() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn refusal_needs_our_origin_and_the_token() {
        let t = "abc";
        assert_eq!(refusal(&post(Some("http://127.0.0.1:9"), Some(t)), 9, t), None);
        assert_eq!(refusal(&post(Some("http://localhost:9"), Some(t)), 9, t), None);
        for r in [post(None, Some(t)), post(Some("http://localhost:8"), Some(t)), post(Some("http://evil:9"), Some(t))] {
            assert_eq!(refusal(&r, 9, t), Some("writes come from sd view's own page only\n"));
        }
        for r in [post(Some("http://localhost:9"), None), post(Some("http://localhost:9"), Some("abd"))] {
            assert_eq!(refusal(&r, 9, t), Some("a write needs this run's token\n"));
        }
    }
}
