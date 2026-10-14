//! Tokeniser for GraphQL documents.

/// Most tokens a document may hold.
const MAX_TOKENS: usize = 20_000;

/// A lexical token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Tok {
    /// A name or keyword.
    Name(String),
    /// A `$variable` reference, without the dollar sign.
    Var(String),
    /// A decoded string literal.
    Str(String),
    /// An int or float literal.
    Num,
    /// A single punctuator.
    Punct(char),
    /// The `...` spread.
    Spread,
}

/// Splits a document into tokens, dropping whitespace, commas and comments.
pub(super) fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut lexer = Lexer {
        chars: src.chars().collect(),
        pos: 0,
    };
    let mut out = Vec::new();
    while let Some(tok) = lexer.next_tok()? {
        out.push(tok);
        if out.len() > MAX_TOKENS {
            return Err("too many tokens".to_owned());
        }
    }
    Ok(out)
}

/// Cursor over the source characters.
struct Lexer {
    /// Source characters.
    chars: Vec<char>,
    /// Index of the next character.
    pos: usize,
}

/// Returns whether a character may start a name.
const fn name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

impl Lexer {
    /// Returns the character `off` places ahead.
    fn peek(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    /// Returns whether the upcoming characters equal `text`.
    fn at(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(i, c)| self.peek(i) == Some(c))
    }

    /// Skips whitespace, commas, the byte order mark and `#` comments.
    fn skip_ignored(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == '#' {
                while self.peek(0).is_some_and(|c| c != '\n' && c != '\r') {
                    self.pos += 1;
                }
            } else if c.is_ascii_whitespace() || c == ',' || c == '\u{feff}' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Reads the next token, or `None` at the end.
    fn next_tok(&mut self) -> Result<Option<Tok>, String> {
        self.skip_ignored();
        let Some(c) = self.peek(0) else {
            return Ok(None);
        };
        let tok = match c {
            '.' if self.at("...") => {
                self.pos += 3;
                Tok::Spread
            }
            '$' => {
                self.pos += 1;
                Tok::Var(self.name()?)
            }
            '"' => Tok::Str(self.string()?),
            '-' | '0'..='9' => self.number(),
            c if name_start(c) => Tok::Name(self.name()?),
            '!' | '&' | '(' | ')' | ':' | '=' | '@' | '[' | ']' | '{' | '|' | '}' => {
                self.pos += 1;
                Tok::Punct(c)
            }
            other => return Err(format!("unexpected character {other:?}")),
        };
        Ok(Some(tok))
    }

    /// Reads a name.
    fn name(&mut self) -> Result<String, String> {
        let mut out = String::new();
        while let Some(c) = self
            .peek(0)
            .filter(|c| name_start(*c) || c.is_ascii_digit())
        {
            out.push(c);
            self.pos += 1;
        }
        if out.is_empty() {
            return Err("expected a name".to_owned());
        }
        Ok(out)
    }

    /// Reads a number and ignores its value.
    fn number(&mut self) -> Tok {
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '.'))
        {
            self.pos += 1;
        }
        Tok::Num
    }

    /// Reads a plain or block string.
    fn string(&mut self) -> Result<String, String> {
        if self.at("\"\"\"") {
            return self.block_string();
        }
        self.pos += 1;
        let mut out = String::new();
        loop {
            let c = self.peek(0).ok_or("unterminated string")?;
            self.pos += 1;
            match c {
                '"' => return Ok(out),
                '\n' | '\r' => return Err("newline in string".to_owned()),
                '\\' => out.push(self.escape()?),
                c => out.push(c),
            }
        }
    }

    /// Reads the escape sequence after a backslash.
    fn escape(&mut self) -> Result<char, String> {
        let c = self.peek(0).ok_or("unterminated string")?;
        self.pos += 1;
        Ok(match c {
            '"' | '\\' | '/' => c,
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => self.unicode()?,
            _ => return Err("bad string escape".to_owned()),
        })
    }

    /// Reads four hex digits of a `\u` escape.
    fn unicode(&mut self) -> Result<char, String> {
        let mut code = 0u32;
        for _ in 0..4 {
            let digit = self.peek(0).and_then(|c| c.to_digit(16));
            code = code * 16 + digit.ok_or("bad unicode escape")?;
            self.pos += 1;
        }
        char::from_u32(code).ok_or_else(|| "bad unicode escape".to_owned())
    }

    /// Reads a `"""` block string verbatim.
    fn block_string(&mut self) -> Result<String, String> {
        self.pos += 3;
        let mut out = String::new();
        loop {
            if self.at("\\\"\"\"") {
                out.push_str("\"\"\"");
                self.pos += 4;
            } else if self.at("\"\"\"") {
                self.pos += 3;
                return Ok(out);
            } else {
                out.push(self.peek(0).ok_or("unterminated block string")?);
                self.pos += 1;
            }
        }
    }
}
