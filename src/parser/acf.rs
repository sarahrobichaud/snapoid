//! Component 3: ACF parser.
//!
//! Parses Steam's VDF-ish ACF files (e.g. appworkshop_108600.acf,
//! libraryfolders.vdf) into a tree. Hand-rolled to avoid depending on a
//! VDF crate with unknown ACF quirk support.

use std::collections::HashMap;

use anyhow::{bail, ensure, Result};

#[derive(Debug, Clone)]
pub enum Acf {
    Str(String),
    Map(HashMap<String, Acf>),
}

impl Acf {
    pub fn get(&self, key: &str) -> Option<&Acf> {
        match self {
            Acf::Map(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Acf::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    chars: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn read_quoted(&mut self) -> Result<String> {
        ensure!(
            self.peek() == Some(b'"'),
            "expected quoted string at byte {}",
            self.pos
        );
        self.pos += 1;
        let mut bytes = Vec::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                b'"' => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
                b'\\' => {
                    let escaped = self.peek();
                    self.pos += 1;
                    match escaped {
                        Some(b'n') => bytes.push(b'\n'),
                        Some(b'r') => bytes.push(b'\r'),
                        Some(b't') => bytes.push(b'\t'),
                        Some(b'\\') => bytes.push(b'\\'),
                        Some(b'"') => bytes.push(b'"'),
                        // Unknown escape: keep the character as-is.
                        Some(other) => bytes.push(other),
                        None => bail!("unterminated escape at end of input"),
                    }
                }
                other => bytes.push(other),
            }
        }
        bail!("unterminated string starting near byte {}", self.pos)
    }

    fn parse_node(&mut self) -> Result<Acf> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                let mut map = HashMap::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Acf::Map(map));
                        }
                        Some(b'"') => {
                            let key = self.read_quoted()?;
                            let value = self.parse_node()?;
                            map.insert(key, value);
                        }
                        _ => bail!("unexpected byte in ACF at {}", self.pos),
                    }
                }
            }
            Some(b'"') => Ok(Acf::Str(self.read_quoted()?)),
            _ => bail!("unexpected byte in ACF at {}", self.pos),
        }
    }
}

/// Parse an ACF document. Top level is `"RootName" { ... }`.
pub fn parse(input: &str) -> Result<Acf> {
    let mut parser = Parser {
        chars: input.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let _root_name = parser.read_quoted()?;
    parser.parse_node()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_maps_and_escapes() {
        let input = r#""root"
{
	"name"		"hello"
	"count"		"42"
	"path"		"C:\\Program Files\\Steam"
	"child"
	{
		"inner"		"value"
	}
}"#;

        let root = parse(input).unwrap();
        assert_eq!(root.get("name").unwrap().as_str(), Some("hello"));
        assert_eq!(root.get("count").unwrap().as_str(), Some("42"));
        assert_eq!(
            root.get("path").unwrap().as_str(),
            Some(r"C:\Program Files\Steam")
        );
        assert_eq!(
            root.get("child")
                .unwrap()
                .get("inner")
                .unwrap()
                .as_str(),
            Some("value")
        );
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse("no quotes here").is_err());
        assert!(parse(r#""unterminated"#).is_err());
    }
}
