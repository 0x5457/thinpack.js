#[derive(Debug, PartialEq, Clone)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl Json {
    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }
    pub(crate) fn object_map(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub(crate) fn subpath_map(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(entries) if entries.iter().all(|(key, _)| key.starts_with('.')) => {
                Some(entries)
            }
            _ => None,
        }
    }
}

pub(crate) fn parse(text: &str) -> Result<Json, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        index: 0,
    };
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.index != parser.bytes.len() {
        return Err(format!("trailing input at byte {}", parser.index));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn value(&mut self) -> Result<Json, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(byte) => Err(format!("unexpected byte {byte:?} at {}", self.index)),
            None => Err("unexpected end of input".to_owned()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.index += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            entries.push((key, self.value()?));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.index += 1,
                Some(b'}') => {
                    self.index += 1;
                    return Ok(Json::Object(entries));
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.index)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.index += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.index += 1,
                Some(b']') => {
                    self.index += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.index)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| "unterminated string".to_owned())?;
            match byte {
                b'"' => {
                    return String::from_utf8(out)
                        .map_err(|_| "string is not valid UTF-8".to_owned());
                }
                b'\\' => {
                    let escaped = self.escape()?;
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(escaped.encode_utf8(&mut buffer).as_bytes());
                }
                0x00..=0x1f => return Err("control character in string".to_owned()),
                byte => out.push(byte),
            }
        }
    }

    fn escape(&mut self) -> Result<char, String> {
        let byte = self
            .next()
            .ok_or_else(|| "unterminated escape".to_owned())?;
        let escaped = match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(),
            other => return Err(format!("invalid escape {:?}", other as char)),
        };
        Ok(escaped)
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let code = self.hex4()?;
        if (0xd800..0xdc00).contains(&code) {
            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                return Err("unpaired high surrogate".to_owned());
            }
            let low = self.hex4()?;
            if !(0xdc00..0xe000).contains(&low) {
                return Err("invalid low surrogate".to_owned());
            }
            let combined = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
            return char::from_u32(combined).ok_or_else(|| "invalid surrogate pair".to_owned());
        }
        char::from_u32(code).ok_or_else(|| "invalid unicode escape".to_owned())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut code = 0u32;
        for _ in 0..4 {
            let byte = self
                .next()
                .ok_or_else(|| "short unicode escape".to_owned())?;
            let digit = (byte as char)
                .to_digit(16)
                .ok_or_else(|| format!("invalid hex digit {:?}", byte as char))?;
            code = code * 16 + digit;
        }
        Ok(code)
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.index;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.index += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.index])
            .map_err(|_| "invalid number".to_owned())?;
        text.parse::<f64>()
            .map(Json::Number)
            .map_err(|error| format!("invalid number {text:?}: {error}"))
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.index..].starts_with(text.as_bytes()) {
            self.index += text.len();
            return Ok(value);
        }
        Err(format!("invalid literal at {}", self.index))
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        match self.next() {
            Some(actual) if actual == byte => Ok(()),
            Some(actual) => Err(format!(
                "expected {:?}, found {:?} at {}",
                byte as char,
                actual as char,
                self.index - 1
            )),
            None => Err(format!("expected {:?}, found end", byte as char)),
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        Some(byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_object_key_order() {
        let value = parse(r#"{"require": "./a.js", "default": "./b.js"}"#).unwrap();
        let Json::Object(entries) = value else {
            panic!("expected object");
        };
        assert_eq!(entries[0].0, "require");
        assert_eq!(entries[1].0, "default");
    }

    #[test]
    fn parses_nested_values() {
        let value = parse(r#"{"exports": {".": ["./a.js", {"default": "./b.js"}]}}"#).unwrap();
        let exports = value.get("exports").unwrap();
        let Json::Array(targets) = exports.get(".").unwrap() else {
            panic!("expected array");
        };
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn rejects_trailing_input() {
        assert!(parse("{} {}").is_err());
    }

    #[test]
    fn decodes_escapes() {
        assert_eq!(parse(r#""a\u00e9\n""#).unwrap().as_str(), Some("aé\n"));
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap().as_str(), Some("😀"));
    }
}

#[cfg(test)]
mod unicode_tests {
    use super::*;

    #[test]
    fn keeps_non_ascii_strings() {
        let value = parse(r#"{"main":"entrée.js"}"#).unwrap();
        assert_eq!(value.get("main").and_then(Json::as_str), Some("entrée.js"));
    }
}
