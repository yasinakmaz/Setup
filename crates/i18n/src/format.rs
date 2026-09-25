//! Minimal positional formatting for translated templates (`{0}`, `{1}`).
//!
//! Translations reorder arguments freely, so `format!` (which needs the
//! template at compile time) cannot be used. The formatter writes into any
//! [`core::fmt::Write`] sink so callers can reuse a buffer.

use core::fmt::{self, Write};

/// Positional arguments for [`format_into`].
pub type FormatArgs<'a> = &'a [&'a dyn fmt::Display];

/// Writes `template` into `out`, substituting `{N}` with `args[N]`.
///
/// `{{` and `}}` produce literal braces. Unknown or malformed placeholders are
/// written verbatim, so a broken translation can never panic an installer.
pub fn format_into<W: Write + ?Sized>(
    out: &mut W,
    template: &str,
    args: FormatArgs<'_>,
) -> fmt::Result {
    let bytes = template.as_bytes();
    let mut i = 0;
    let mut literal_start = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                out.write_str(&template[literal_start..i + 1])?;
                i += 2;
                literal_start = i;
            }
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                out.write_str(&template[literal_start..i + 1])?;
                i += 2;
                literal_start = i;
            }
            b'{' => {
                let rest = &bytes[i + 1..];
                let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
                if digits > 0 && rest.get(digits) == Some(&b'}') {
                    let index = template[i + 1..i + 1 + digits].parse::<usize>().ok();
                    if let Some(arg) = index.and_then(|n| args.get(n)) {
                        out.write_str(&template[literal_start..i])?;
                        write!(out, "{arg}")?;
                        i += digits + 2;
                        literal_start = i;
                        continue;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.write_str(&template[literal_start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Buf {
        data: [u8; 256],
        len: usize,
    }

    impl Buf {
        fn new() -> Self {
            Buf {
                data: [0; 256],
                len: 0,
            }
        }
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.data[..self.len]).unwrap_or_default()
        }
    }

    impl Write for Buf {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let end = self.len + s.len();
            self.data
                .get_mut(self.len..end)
                .ok_or(fmt::Error)?
                .copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    fn fmt(template: &str, args: FormatArgs<'_>) -> Buf {
        let mut buf = Buf::new();
        format_into(&mut buf, template, args).expect("buffer large enough");
        buf
    }

    #[test]
    fn substitutes_and_reorders() {
        assert_eq!(fmt("{1} / {0}", &[&"a", &2]).as_str(), "2 / a");
        assert_eq!(fmt("تثبيت {0}", &[&"App"]).as_str(), "تثبيت App");
    }

    #[test]
    fn keeps_malformed_placeholders() {
        assert_eq!(fmt("{x} {5} {", &[&1]).as_str(), "{x} {5} {");
        assert_eq!(fmt("{{0}} {0}", &[&7]).as_str(), "{0} 7");
    }
}
