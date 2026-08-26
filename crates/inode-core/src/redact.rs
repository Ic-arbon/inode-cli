//! Log/diagnostic redaction. Passwords and cookies must never reach logs,
//! status output, or diagnostic bundles.

pub const REDACTED: &str = "<redacted>";

#[derive(Debug, Default, Clone)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| !s.is_empty()).collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Self { secrets }
    }

    pub fn add(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if !secret.is_empty() && !self.secrets.contains(&secret) {
            self.secrets.push(secret);
            self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut out = input.to_string();
        for secret in &self.secrets {
            if out.contains(secret) {
                out = out.replace(secret, REDACTED);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password_and_cookie() {
        let r = Redactor::new([
            "Tianyi@123wise".into(),
            "svpnginfo=ctxid@abc+uid@def".into(),
        ]);
        assert_eq!(
            r.redact("login failed for Tianyi@123wise cookie svpnginfo=ctxid@abc+uid@def"),
            "login failed for <redacted> cookie <redacted>"
        );
    }

    #[test]
    fn empty_secret_is_ignored() {
        let r = Redactor::new(["".into(), "x".into()]);
        assert_eq!(r.redact("axb"), "a<redacted>b");
    }
}
