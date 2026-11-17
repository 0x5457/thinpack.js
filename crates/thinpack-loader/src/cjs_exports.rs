use std::collections::BTreeSet;

pub(crate) fn reexport_specifiers(source: &str) -> Vec<String> {
    let tokens = tokenize(source);
    let mut specifiers = Vec::new();

    for index in 0..tokens.len() {
        if tokens[index].is_ident("module")
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.is_punct('.'))
            && tokens
                .get(index + 2)
                .is_some_and(|token| token.is_ident("exports"))
            && tokens
                .get(index + 3)
                .is_some_and(|token| token.is_punct('='))
            && tokens
                .get(index + 4)
                .is_some_and(|token| token.is_ident("require"))
            && tokens
                .get(index + 5)
                .is_some_and(|token| token.is_punct('('))
            && let Some(Token::Str(specifier)) = tokens.get(index + 6)
            && tokens
                .get(index + 7)
                .is_some_and(|token| token.is_punct(')'))
        {
            specifiers.push(specifier.clone());
            continue;
        }
        let is_helper =
            tokens[index].is_ident("__exportStar") || tokens[index].is_ident("__export");
        if !is_helper
            || !tokens
                .get(index + 1)
                .is_some_and(|token| token.is_punct('('))
            || !tokens
                .get(index + 2)
                .is_some_and(|token| token.is_ident("require"))
            || !tokens
                .get(index + 3)
                .is_some_and(|token| token.is_punct('('))
        {
            continue;
        }
        if let Some(Token::Str(specifier)) = tokens.get(index + 4)
            && tokens
                .get(index + 5)
                .is_some_and(|token| token.is_punct(')'))
        {
            specifiers.push(specifier.clone());
        }
    }

    specifiers
}

pub(crate) fn export_names(source: &str) -> BTreeSet<String> {
    let tokens = tokenize(source);
    let mut names = BTreeSet::new();
    let mut index = 0;

    while index < tokens.len() {
        let consumed = scan_assignments(&tokens, index, &mut names)
            .or_else(|| scan_object_keys(&tokens, index, &mut names))
            .or_else(|| scan_loop_assignments(&tokens, index, &mut names));
        index = consumed.map_or(index + 1, |next| next.max(index + 1));
    }

    names.retain(|name| is_identifier(name) && !is_reserved(name));
    names.remove("default");
    names
}

fn scan_loop_assignments(
    tokens: &[Token],
    index: usize,
    names: &mut BTreeSet<String>,
) -> Option<usize> {
    if !tokens.get(index).is_some_and(|token| token.is_punct('[')) {
        return None;
    }
    let listed = string_list(tokens, index)?;
    let mut cursor = index + 1;
    while let Some(token) = tokens.get(cursor) {
        match token {
            Token::Str(_) | Token::Punct(',') => cursor += 1,
            Token::Punct(']') => break,
            _ => return None,
        }
    }
    let after_list = cursor + 1;

    let (variable, body) = if tokens
        .get(index.wrapping_sub(1))
        .is_some_and(|token| token.is_ident("of"))
    {
        let variable = tokens.get(index.checked_sub(2)?)?.ident()?.to_owned();
        (variable, after_list)
    } else {
        let mut cursor = after_list;
        while tokens.get(cursor).is_some_and(|token| token.is_punct(',')) {
            cursor += 1;
        }
        if !tokens.get(cursor).is_some_and(|token| token.is_punct('(')) {
            return None;
        }
        let variable = tokens.get(cursor + 1)?.ident()?.to_owned();
        if !tokens
            .get(cursor + 2)
            .is_some_and(|token| token.is_punct(')'))
            || !tokens
                .get(cursor + 3)
                .is_some_and(|token| token.is_punct('='))
            || !tokens
                .get(cursor + 4)
                .is_some_and(|token| token.is_punct('>'))
        {
            return None;
        }
        (variable, cursor + 4)
    };

    let mut depth = 0usize;
    let mut assigns = false;
    let mut uses_variable = false;
    for token in &tokens[body..] {
        match token {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Ident(name) if name == "setOwnProperty" => assigns = true,
            Token::Ident(name) if assigns && *name == variable => uses_variable = true,
            _ => {}
        }
    }
    if !uses_variable {
        return None;
    }
    names.extend(listed);
    Some(body)
}

fn string_list(tokens: &[Token], index: usize) -> Option<Vec<String>> {
    let mut listed = Vec::new();
    let mut cursor = index + 1;
    while let Some(token) = tokens.get(cursor) {
        match token {
            Token::Str(value) => listed.push(value.clone()),
            Token::Punct(']') => return (!listed.is_empty()).then_some(listed),
            Token::Punct(',') => {}
            _ => return None,
        }
        cursor += 1;
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    Str(String),
    Punct(char),
    Number,
    Regex,
}

impl Token {
    fn ident(&self) -> Option<&str> {
        match self {
            Self::Ident(name) => Some(name),
            _ => None,
        }
    }

    const fn is_punct(&self, character: char) -> bool {
        matches!(self, Self::Punct(found) if *found == character)
    }

    fn is_ident(&self, expected: &str) -> bool {
        self.ident() == Some(expected)
    }
}

fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => index += 1,
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b'/' if regex_allowed(tokens.last()) => {
                if let Some(next) = skip_regex(bytes, index) {
                    tokens.push(Token::Regex);
                    index = next;
                } else {
                    tokens.push(Token::Punct('/'));
                    index += 1;
                }
            }
            b'\'' | b'"' => {
                let (value, next) = read_string(bytes, index, byte);
                tokens.push(Token::Str(value));
                index = next;
            }
            b'`' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index += 2;
                    } else if bytes[index] == b'`' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'0'..=b'9' => {
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'x' | b'e' | b'_')
                {
                    index += 1;
                }
                tokens.push(Token::Number);
            }
            byte if is_identifier_start(byte) => {
                let start = index;
                while index < bytes.len() && is_identifier_part(bytes[index]) {
                    index += 1;
                }
                tokens.push(Token::Ident(source[start..index].to_owned()));
            }
            other => {
                tokens.push(Token::Punct(other as char));
                index += 1;
            }
        }
    }

    tokens
}

fn regex_allowed(previous: Option<&Token>) -> bool {
    match previous {
        None => true,
        Some(Token::Ident(name)) => matches!(
            name.as_str(),
            "return"
                | "typeof"
                | "instanceof"
                | "in"
                | "of"
                | "new"
                | "delete"
                | "void"
                | "do"
                | "else"
                | "yield"
                | "await"
                | "case"
                | "throw"
        ),
        Some(Token::Punct(character)) => !matches!(character, ')' | ']'),
        Some(_) => false,
    }
}

const fn skip_regex(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut in_class = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'[' => {
                in_class = true;
                index += 1;
            }
            b']' => {
                in_class = false;
                index += 1;
            }
            b'/' if !in_class => {
                index += 1;
                while index < bytes.len() && is_identifier_part(bytes[index]) {
                    index += 1;
                }
                return Some(index);
            }
            b'\n' => return None,
            _ => index += 1,
        }
    }
    None
}

fn read_string(bytes: &[u8], start: usize, quote: u8) -> (String, usize) {
    let mut value = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            byte if byte == quote => {
                index += 1;
                break;
            }
            b'\\' => {
                if let Some(&next) = bytes.get(index + 1) {
                    value.push(next as char);
                }
                index += 2;
            }
            byte => {
                value.push(byte as char);
                index += 1;
            }
        }
    }
    (value, index)
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

const fn is_identifier_part(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(first) if is_identifier_start(first) => {}
        _ => return false,
    }
    bytes.all(is_identifier_part)
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "arguments"
            | "eval"
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
    )
}

fn scan_assignments(tokens: &[Token], start: usize, names: &mut BTreeSet<String>) -> Option<usize> {
    let mut index = start;
    let exports_at = if tokens.get(index)?.is_ident("exports") {
        index += 1;
        Some(index)
    } else if tokens.get(index)?.is_ident("module")
        && tokens.get(index + 1)?.is_punct('.')
        && tokens.get(index + 2)?.is_ident("exports")
    {
        index += 3;
        Some(index)
    } else {
        None
    };
    let exports_at = exports_at?;

    match tokens.get(exports_at)? {
        Token::Punct('.') => {
            let name = tokens.get(exports_at + 1)?.ident()?.to_owned();
            if tokens.get(exports_at + 2)?.is_punct('=') {
                names.insert(name);
            }
            Some(exports_at + 3)
        }
        Token::Punct('[') => {
            let Token::Str(name) = tokens.get(exports_at + 1)? else {
                return None;
            };
            if tokens.get(exports_at + 2)?.is_punct(']')
                && tokens.get(exports_at + 3)?.is_punct('=')
            {
                names.insert(name.clone());
            }
            Some(exports_at + 4)
        }
        Token::Punct('=') => {
            let mut consumed = exports_at + 1;
            collect_rhs(tokens, consumed, names, &mut consumed);
            Some(consumed)
        }
        _ => None,
    }
}

fn scan_object_keys(tokens: &[Token], start: usize, names: &mut BTreeSet<String>) -> Option<usize> {
    let token = tokens.get(start)?;
    if token.is_ident("Object")
        && tokens.get(start + 1)?.is_punct('.')
        && tokens.get(start + 2)?.is_ident("defineProperty")
    {
        return scan_defined_property(tokens, start + 3, names);
    }
    if token.is_ident("defineLazyProperties") {
        return scan_lazy_properties(tokens, start + 1, names);
    }
    if token.is_ident("ObjectDefineProperties") {
        return scan_defined_properties(tokens, start + 1, names);
    }
    None
}

fn scan_defined_property(
    tokens: &[Token],
    start: usize,
    names: &mut BTreeSet<String>,
) -> Option<usize> {
    let mut index = start;
    if !tokens.get(index)?.is_punct('(') {
        return None;
    }
    index += 1;
    if tokens.get(index)?.is_ident("exports") {
        index += 1;
    } else if tokens.get(index)?.is_ident("module")
        && tokens.get(index + 1)?.is_punct('.')
        && tokens.get(index + 2)?.is_ident("exports")
    {
        index += 3;
    } else if tokens
        .get(index)
        .and_then(Token::ident)
        .is_some_and(|name| is_export_alias(tokens, name))
    {
        index += 1;
    } else {
        return None;
    }
    if !tokens.get(index)?.is_punct(',') {
        return None;
    }
    if let Some(Token::Str(name)) = tokens.get(index + 1) {
        names.insert(name.clone());
    }
    Some(index + 2)
}

fn scan_lazy_properties(
    tokens: &[Token],
    start: usize,
    names: &mut BTreeSet<String>,
) -> Option<usize> {
    let mut index = start;
    if !tokens.get(index)?.is_punct('(') {
        return None;
    }
    index += 1;
    if tokens.get(index)?.is_ident("module")
        && tokens.get(index + 1)?.is_punct('.')
        && tokens.get(index + 2)?.is_ident("exports")
    {
        index += 3;
    } else if tokens
        .get(index)?
        .ident()
        .is_some_and(|name| name == "exports" || is_export_alias(tokens, name))
    {
        index += 1;
    } else {
        return None;
    }
    if !tokens.get(index)?.is_punct(',')
        || !matches!(tokens.get(index + 1), Some(Token::Str(_)))
        || !tokens.get(index + 2)?.is_punct(',')
        || !tokens.get(index + 3)?.is_punct('[')
    {
        return None;
    }
    let mut index = index + 4;
    while let Some(Token::Str(name)) = tokens.get(index) {
        names.insert(name.clone());
        index += 1;
        if tokens.get(index).is_some_and(|token| token.is_punct(',')) {
            index += 1;
        } else {
            break;
        }
    }
    if !tokens.get(index)?.is_punct(']') {
        return None;
    }
    Some(index + 1)
}

fn scan_defined_properties(
    tokens: &[Token],
    start: usize,
    names: &mut BTreeSet<String>,
) -> Option<usize> {
    let mut index = start;
    if !tokens.get(index)?.is_punct('(') {
        return None;
    }
    index += 1;
    if tokens.get(index)?.is_ident("module") {
        index += 3;
    } else if tokens
        .get(index)?
        .ident()
        .is_some_and(|name| name == "exports" || is_export_alias(tokens, name))
    {
        index += 1;
    } else {
        return None;
    }
    if !tokens.get(index)?.is_punct(',') || !tokens.get(index + 1)?.is_punct('{') {
        return None;
    }
    let consumed = index + 1;
    names.extend(defined_property_names(tokens, index + 1));
    Some(consumed)
}

fn is_export_alias(tokens: &[Token], name: &str) -> bool {
    for index in 0..tokens.len().saturating_sub(3) {
        if !(tokens[index].is_ident("module")
            && tokens[index + 1].is_punct('.')
            && tokens[index + 2].is_ident("exports")
            && tokens[index + 3].is_punct('='))
        {
            continue;
        }
        let mut cursor = index + 4;
        while tokens.get(cursor).and_then(Token::ident).is_some()
            && tokens
                .get(cursor + 1)
                .is_some_and(|token| token.is_punct('='))
        {
            if tokens[cursor].is_ident(name) {
                return true;
            }
            cursor += 2;
        }
        return tokens.get(cursor).and_then(Token::ident) == Some(name);
    }
    false
}

fn collect_rhs(tokens: &[Token], start: usize, names: &mut BTreeSet<String>, consumed: &mut usize) {
    let mut start = start;
    while tokens.get(start).and_then(Token::ident).is_some()
        && tokens
            .get(start + 1)
            .is_some_and(|token| token.is_punct('='))
    {
        start += 2;
    }
    if tokens.get(start).is_some_and(|token| token.is_punct('{')) {
        let keys = object_keys(tokens, start, consumed);
        names.extend(keys);
        return;
    }
    if tokens
        .get(start)
        .is_some_and(|token| token.is_ident("Object"))
        && tokens
            .get(start + 1)
            .is_some_and(|token| token.is_punct('.'))
        && tokens
            .get(start + 2)
            .is_some_and(|token| token.is_ident("freeze"))
    {
        let mut index = start + 3;
        while tokens.get(index).is_some_and(|token| !token.is_punct('{')) {
            index += 1;
        }
        if tokens.get(index).is_some_and(|token| token.is_punct('{')) {
            let keys = object_keys(tokens, index, consumed);
            names.extend(keys);
            return;
        }
    }

    let mut identifiers = BTreeSet::new();
    let mut index = start;
    while let Some(token) = tokens.get(index) {
        if token.is_punct(';') || token.is_punct(')') || token.is_punct('}') {
            break;
        }
        if let Some(name) = token.ident()
            && !matches!(name, "module" | "exports" | "require" | "globalThis")
        {
            identifiers.insert(name.to_owned());
        }
        index += 1;
    }
    *consumed = index;
    if let Some(member) = trailing_member_name(tokens, start) {
        resolve_identifier(tokens, member, names, consumed);
    }

    for identifier in identifiers {
        if let Some(rhs) = find_global_assignment(tokens, &identifier) {
            resolve_identifier_value(tokens, rhs, names, consumed);
        }
        resolve_identifier(tokens, &identifier, names, consumed);
    }
}

fn trailing_member_name(tokens: &[Token], start: usize) -> Option<&str> {
    let mut name = None;
    let mut index = start;
    while let Some(token) = tokens.get(index) {
        if token.is_punct(';') || token.is_punct('}') {
            break;
        }
        if token.is_punct('.')
            && let Some(Token::Ident(property)) = tokens.get(index + 1)
        {
            name = Some(property.as_str());
        }
        index += 1;
    }
    name
}

fn find_global_assignment(tokens: &[Token], name: &str) -> Option<usize> {
    let mut index = 0;
    while index + 3 < tokens.len() {
        let matches = tokens[index].is_ident("globalThis")
            && tokens[index + 1].is_punct('.')
            && tokens[index + 2].is_ident(name)
            && tokens[index + 3].is_punct('=');
        if matches {
            return Some(index + 4);
        }
        index += 1;
    }
    None
}

fn resolve_identifier_value(
    tokens: &[Token],
    start: usize,
    names: &mut BTreeSet<String>,
    consumed: &mut usize,
) {
    if tokens.get(start).is_some_and(|token| token.is_punct('{')) {
        names.extend(object_keys(tokens, start, consumed));
        return;
    }
    if tokens
        .get(start)
        .is_some_and(|token| token.is_ident("Object"))
        && tokens
            .get(start + 1)
            .is_some_and(|token| token.is_punct('.'))
        && tokens
            .get(start + 2)
            .is_some_and(|token| token.is_ident("freeze"))
    {
        let mut index = start + 3;
        while tokens.get(index).is_some_and(|token| !token.is_punct('{')) {
            index += 1;
        }
        if tokens.get(index).is_some_and(|token| token.is_punct('{')) {
            names.extend(object_keys(tokens, index, consumed));
        }
        return;
    }
    if let Some(name) = tokens.get(start).and_then(Token::ident) {
        resolve_identifier(tokens, name, names, consumed);
    }
}

fn resolve_identifier(
    tokens: &[Token],
    identifier: &str,
    names: &mut BTreeSet<String>,
    consumed: &mut usize,
) {
    if matches!(identifier, "module" | "exports" | "require" | "globalThis") {
        return;
    }
    for (position, token) in tokens.iter().enumerate() {
        let declares = matches!(token.ident(), Some("const" | "let" | "var"));
        if declares
            && tokens
                .get(position + 1)
                .is_some_and(|next| next.is_ident(identifier))
            && tokens
                .get(position + 2)
                .is_some_and(|next| next.is_punct('='))
        {
            resolve_identifier_value(tokens, position + 3, names, consumed);
            break;
        }
    }
    collect_property_assignments(tokens, identifier, names);
    collect_defined_properties(tokens, identifier, names);
}

fn collect_property_assignments(tokens: &[Token], target: &str, names: &mut BTreeSet<String>) {
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].is_ident(target) {
            match (
                tokens.get(index + 1),
                tokens.get(index + 2),
                tokens.get(index + 3),
            ) {
                (Some(Token::Punct('.')), Some(Token::Ident(name)), Some(Token::Punct('='))) => {
                    names.insert(name.clone());
                }
                (Some(Token::Punct('[')), Some(Token::Str(name)), Some(Token::Punct(']')))
                    if tokens
                        .get(index + 4)
                        .is_some_and(|token| token.is_punct('=')) =>
                {
                    names.insert(name.clone());
                }
                _ => {}
            }
        }
        index += 1;
    }
}

fn collect_defined_properties(tokens: &[Token], target: &str, names: &mut BTreeSet<String>) {
    if let Some(start) = tokens
        .iter()
        .position(|token| token.is_ident("ObjectDefineProperties"))
    {
        let check = tokens
            .get(start + 2)
            .is_some_and(|token| token.is_ident(target));
        let brace = tokens
            .get(start + 3)
            .is_some_and(|token| token.is_punct('{'));
        if check && brace {
            let mut consumed = start + 3;
            for name in defined_property_names(tokens, start + 3) {
                names.insert(name);
            }
            let _ = &mut consumed;
        }
    }

    let mut index = 0;
    while index + 5 < tokens.len() {
        let is_call = tokens[index].is_ident("Object")
            && tokens[index + 1].is_punct('.')
            && tokens[index + 2].is_ident("defineProperty")
            && tokens[index + 3].is_punct('(')
            && tokens[index + 4].is_ident(target)
            && tokens[index + 5].is_punct(',');
        if is_call && let Some(Token::Str(name)) = tokens.get(index + 6) {
            let descriptor_at =
                (index + 7..tokens.len()).find(|position| tokens[*position].is_punct('{'));
            let is_accessor =
                descriptor_at.is_some_and(|position| object_has_getter(tokens, position));
            if !is_accessor {
                names.insert(name.clone());
            }
        }
        index += 1;
    }
}

fn defined_property_names(tokens: &[Token], brace: usize) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut depth = 0usize;
    let mut index = brace;
    let mut pending_key: Option<String> = None;

    while let Some(token) = tokens.get(index) {
        match token {
            Token::Punct('{') => {
                depth += 1;
            }
            Token::Punct('}') => {
                if (depth == 1 || depth == 2)
                    && let Some(key) = pending_key.take()
                {
                    names.insert(key);
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Punct(',') if depth == 1 => {
                if let Some(key) = pending_key.take() {
                    names.insert(key);
                }
            }
            Token::Ident(name)
                if depth == 1
                    && pending_key.is_none()
                    && tokens.get(index + 1).is_some_and(|next| next.is_punct(':')) =>
            {
                pending_key = Some(name.clone());
            }
            Token::Str(name)
                if depth == 1
                    && pending_key.is_none()
                    && tokens.get(index + 1).is_some_and(|next| next.is_punct(':')) =>
            {
                pending_key = Some(name.clone());
            }
            _ => {}
        }
        index += 1;
    }

    names
}

fn object_has_getter(tokens: &[Token], brace: usize) -> bool {
    let mut depth = 0usize;
    let mut index = brace;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return false;
                }
            }
            Token::Ident(name)
                if name == "get"
                    && depth > 0
                    && tokens.get(index + 1).is_some_and(|next| next.is_punct('(')) =>
            {
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn object_keys(tokens: &[Token], start: usize, consumed: &mut usize) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !tokens.get(start).is_some_and(|token| token.is_punct('{')) {
        *consumed = start;
        return keys;
    }
    let mut depth = 0usize;
    let mut expect_key = true;
    let mut previous: Option<String> = None;
    let mut index = start;

    while let Some(token) = tokens.get(index) {
        match token {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => {
                if depth == 0 {
                    *consumed = index + 1;
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    *consumed = index + 1;
                    break;
                }
            }
            Token::Punct(',') if depth == 1 => expect_key = true,
            Token::Punct(':') if depth == 1 => expect_key = false,
            Token::Punct('.') if depth == 1 => {
                expect_key = false;
            }
            Token::Punct('(' | '[') if depth == 1 => {
                let (open, close) = if token.is_punct('(') {
                    ('(', ')')
                } else {
                    ('[', ']')
                };
                index = skip_group(tokens, index, open, close);
                continue;
            }
            Token::Ident(name) if depth == 1 => {
                if name == "exports"
                    && tokens
                        .get(index + 1)
                        .is_some_and(|token| token.is_punct(':'))
                    && tokens
                        .get(index + 2)
                        .is_some_and(|token| token.is_punct('{'))
                {
                    let mut nested_consumed = index + 2;
                    keys.extend(object_keys(tokens, index + 2, &mut nested_consumed));
                    index = nested_consumed;
                    expect_key = false;
                    continue;
                }
                let next = tokens.get(index + 1);
                let is_key = next.is_some_and(|next| {
                    next.is_punct(':')
                        || next.is_punct(',')
                        || next.is_punct('}')
                        || next.is_punct('(')
                });
                let is_accessor = previous
                    .as_deref()
                    .is_some_and(|previous| matches!(previous, "get" | "set"));
                if expect_key
                    && is_key
                    && !is_accessor
                    && !matches!(name.as_str(), "get" | "set" | "async")
                {
                    keys.insert(name.clone());
                    expect_key = false;
                } else if expect_key && matches!(name.as_str(), "get" | "set" | "async") {
                } else if expect_key && is_key {
                    keys.insert(name.clone());
                    expect_key = false;
                } else {
                    expect_key = false;
                }
                previous = Some(name.clone());
                index += 1;
                continue;
            }
            Token::Str(name) if depth == 1 && expect_key => {
                if tokens.get(index + 1).is_some_and(|next| next.is_punct(':')) {
                    keys.insert(name.clone());
                }
                expect_key = false;
            }
            _ => {}
        }
        index += 1;
    }

    if *consumed < index {
        *consumed = index;
    }
    keys
}

fn skip_group(tokens: &[Token], start: usize, open: char, close: char) -> usize {
    let mut depth = 0usize;
    let mut index = start;
    while let Some(token) = tokens.get(index) {
        if token.is_punct(open) {
            depth += 1;
        } else if token.is_punct(close) {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        export_names(source).into_iter().collect()
    }

    #[test]
    fn reads_names_assigned_in_a_string_loop() {
        let looped = r"
module.exports = assert;
for (const name of ['ok', 'equal', 'notEqual']) {
  setOwnProperty(assert, name, Assert.prototype[name]);
}
";
        assert_eq!(names(looped), vec!["equal", "notEqual", "ok"]);

        let each = r"
module.exports = assert;
ArrayPrototypeForEach(['ok', 'equal'], (name) => {
  setOwnProperty(assert, name, Assert.prototype[name]);
});
";
        assert_eq!(names(each), vec!["equal", "ok"]);
    }

    #[test]
    fn ignores_quotes_inside_regex_literals() {
        let source = r#"
const re = /['"]/g;
const url = /^https?:\/\//;
const divide = 4 / 2;
module.exports = { a: 1 };
exports.b = 2;
"#;
        assert_eq!(names(source), vec!["a", "b"]);
    }

    #[test]
    fn reads_exports_assignments() {
        assert_eq!(
            names("exports.a = 1; exports['b'] = 2; module.exports.c = 3;"),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn reads_object_literal() {
        assert_eq!(
            names("module.exports = {\n  Buffer,\n  'transcode': fn,\n  ...rest,\n};"),
            vec!["Buffer", "transcode"]
        );
    }

    #[test]
    fn resolves_identifier_literals() {
        let source = "const posix = { basename: fn, join: fn };\n\
                      const win32 = { basename: fn };\n\
                      module.exports = isWindows ? win32 : posix;";
        assert_eq!(names(source), vec!["basename", "join"]);
    }

    #[test]
    fn reads_define_property_patterns() {
        assert_eq!(
            names(
                "ObjectDefineProperties(module.exports, { a: kEnumerableProperty });\nObject.defineProperty(exports, 'b', {});"
            ),
            vec!["a", "b"]
        );
    }

    #[test]
    fn reads_chained_object_literal_and_alias_properties() {
        let source = "module.exports = fs = {\n  readFileSync: fn,\n};\n\
                      ObjectDefineProperties(fs, {\n  constants: { value: 1 },\n  promises: { get() {} },\n});\n\
                      defineLazyProperties(fs, 'node:thinpack/internal/fs/dir', ['Dir', 'opendir', 'opendirSync']);";
        assert_eq!(
            names(source),
            vec![
                "Dir",
                "constants",
                "opendir",
                "opendirSync",
                "promises",
                "readFileSync"
            ]
        );
    }

    #[test]
    fn reads_exports_from_member_expression() {
        let source = "const Stream = module.exports = require('internal/streams/legacy').Stream;\n\
                      Stream.Readable = Readable;\n\
                      Stream.pipeline = pipeline;";
        assert_eq!(names(source), vec!["Readable", "pipeline"]);
    }

    #[test]
    fn finds_reexport_specifiers() {
        let source = "__exportStar(require(\"./client\"), exports);\n\
                      __export(require(\"./other\"), exports);\n\
                      require(\"./not-a-reexport\");";
        assert_eq!(reexport_specifiers(source), vec!["./client", "./other"]);
    }

    #[test]
    fn ignores_comments_and_strings() {
        assert_eq!(
            names(
                "// exports.nope = 1;\n/* module.exports.nope2 = 1 */\nconst s = 'exports.nope3';\nexports.yes = 1;"
            ),
            vec!["yes"]
        );
    }

    #[test]
    fn filters_invalid_names() {
        assert_eq!(
            names("exports['has-dash'] = 1; exports.ok = 2;"),
            vec!["ok"]
        );
    }
}
