#[derive(Debug, Clone, PartialEq)]
pub enum StrPartToken {
    Lit(String),
    Interp(Vec<Token>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64),
    Float(f64),
    Symbol(String),
    StringLit(Vec<StrPartToken>),
    Ident(String),
    Const(String),
    Ivar(String),
    Cvar(String),
    Gvar(String),

    // Keywords
    Def,
    End,
    Class,
    Module,
    If,
    Elsif,
    Else,
    Unless,
    Then,
    While,
    Until,
    For,
    In,
    Do,
    Break,
    Next,
    Return,
    Yield,
    Case,
    When,
    Begin,
    Rescue,
    Ensure,
    Super,
    SelfKw,
    Nil,
    True,
    False,
    AndKw,
    OrKw,
    NotKw,

    // Operators & Punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship,
    EqEqEq,
    AndAnd,
    OrOr,
    Bang,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,

    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    StarStarEq,
    AndAndEq,
    OrOrEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    DotDot,
    DotDotDot,
    Arrow,
    FatArrow,
    ColonColon,
    SafeNav,
    Dot,
    Question,
    Colon,
    Semicolon,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,

    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(c) = self.peek() {
                if c == ' ' || c == '\t' || c == '\r' {
                    self.advance();
                } else if c == '\\' && self.peek_n(1) == Some('\n') {
                    self.advance();
                    self.advance();
                } else {
                    break;
                }
            }
            if self.peek() == Some('#') {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
                continue;
            }
            if self.col == 1 && self.starts_with("=begin") {
                while self.pos < self.chars.len() {
                    if self.col == 1 && self.starts_with("=end") {
                        while let Some(c) = self.peek() {
                            self.advance();
                            if c == '\n' {
                                break;
                            }
                        }
                        break;
                    }
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        let sc: Vec<char> = s.chars().collect();
        if self.pos + sc.len() > self.chars.len() {
            return false;
        }
        for (i, c) in sc.iter().enumerate() {
            if self.chars[self.pos + i] != *c {
                return false;
            }
        }
        true
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            self.skip_ws_and_comments();
            let line = self.line;
            let col = self.col;
            let Some(c) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    line,
                    col,
                });
                break;
            };

            if c == '\n' {
                self.advance();
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    line,
                    col,
                });
                continue;
            }

            if c.is_ascii_digit() {
                tokens.push(self.lex_number(line, col)?);
                continue;
            }

            if c == '\'' {
                tokens.push(self.lex_single_string(line, col)?);
                continue;
            }

            if c == '"' {
                tokens.push(self.lex_double_string(line, col)?);
                continue;
            }

            if c == ':' {
                if self.peek_n(1) == Some(':') {
                    self.advance();
                    self.advance();
                    tokens.push(Token { kind: TokenKind::ColonColon, line, col });
                    continue;
                }
                if let Some(nc) = self.peek_n(1) {
                    if nc == '"' {
                        self.advance(); // ':'
                        let tok = self.lex_double_string(line, col)?;
                        if let TokenKind::StringLit(parts) = tok.kind {
                            if parts.len() == 1 {
                                if let StrPartToken::Lit(s) = &parts[0] {
                                    tokens.push(Token { kind: TokenKind::Symbol(s.clone()), line, col });
                                    continue;
                                }
                            }
                            if parts.is_empty() {
                                tokens.push(Token { kind: TokenKind::Symbol(String::new()), line, col });
                                continue;
                            }
                        }
                        return Err(format!("{}:{}: interpolated symbols not supported", line, col));
                    } else if nc.is_ascii_alphabetic() || nc == '_' {
                        self.advance(); // ':'
                        let name = self.lex_ident_name();
                        tokens.push(Token { kind: TokenKind::Symbol(name), line, col });
                        continue;
                    }
                }
                self.advance();
                tokens.push(Token { kind: TokenKind::Colon, line, col });
                continue;
            }

            if c == '@' {
                self.advance();
                if self.peek() == Some('@') {
                    self.advance();
                    let name = self.lex_ident_name_plain();
                    tokens.push(Token { kind: TokenKind::Cvar(format!("@@{}", name)), line, col });
                } else {
                    let name = self.lex_ident_name_plain();
                    tokens.push(Token { kind: TokenKind::Ivar(format!("@{}", name)), line, col });
                }
                continue;
            }

            if c == '$' {
                self.advance();
                let name = self.lex_ident_name_plain();
                tokens.push(Token { kind: TokenKind::Gvar(format!("${}", name)), line, col });
                continue;
            }

            if c.is_ascii_alphabetic() || c == '_' {
                let first_upper = c.is_ascii_uppercase();
                let name = self.lex_ident_name();
                let kind = match name.as_str() {
                    "def" => TokenKind::Def,
                    "end" => TokenKind::End,
                    "class" => TokenKind::Class,
                    "module" => TokenKind::Module,
                    "if" => TokenKind::If,
                    "elsif" => TokenKind::Elsif,
                    "else" => TokenKind::Else,
                    "unless" => TokenKind::Unless,
                    "then" => TokenKind::Then,
                    "while" => TokenKind::While,
                    "until" => TokenKind::Until,
                    "for" => TokenKind::For,
                    "in" => TokenKind::In,
                    "do" => TokenKind::Do,
                    "break" => TokenKind::Break,
                    "next" => TokenKind::Next,
                    "return" => TokenKind::Return,
                    "yield" => TokenKind::Yield,
                    "case" => TokenKind::Case,
                    "when" => TokenKind::When,
                    "begin" => TokenKind::Begin,
                    "rescue" => TokenKind::Rescue,
                    "ensure" => TokenKind::Ensure,
                    "super" => TokenKind::Super,
                    "self" => TokenKind::SelfKw,
                    "nil" => TokenKind::Nil,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "and" => TokenKind::AndKw,
                    "or" => TokenKind::OrKw,
                    "not" => TokenKind::NotKw,
                    _ => {
                        if first_upper && !name.ends_with('?') && !name.ends_with('!') {
                            TokenKind::Const(name)
                        } else {
                            TokenKind::Ident(name)
                        }
                    }
                };
                tokens.push(Token { kind, line, col });
                continue;
            }

            tokens.push(self.lex_operator(line, col)?);
        }
        Ok(tokens)
    }

    fn lex_ident_name_plain(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn lex_ident_name(&mut self) -> String {
        let mut s = self.lex_ident_name_plain();
        if let Some(c) = self.peek() {
            if c == '?' || c == '!' {
                s.push(c);
                self.advance();
            }
        }
        s
    }

    fn lex_number(&mut self, line: usize, col: usize) -> Result<Token, String> {
        let mut s = String::new();
        if self.peek() == Some('0') {
            if let Some(base_ch) = self.peek_n(1) {
                if base_ch == 'x' || base_ch == 'X' {
                    self.advance();
                    self.advance();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_hexdigit() || c == '_' {
                            if c != '_' { s.push(c); }
                            self.advance();
                        } else { break; }
                    }
                    let v = i64::from_str_radix(&s, 16).map_err(|e| format!("{}:{}: {}", line, col, e))?;
                    return Ok(Token { kind: TokenKind::Int(v), line, col });
                } else if base_ch == 'b' || base_ch == 'B' {
                    self.advance();
                    self.advance();
                    while let Some(c) = self.peek() {
                        if c == '0' || c == '1' || c == '_' {
                            if c != '_' { s.push(c); }
                            self.advance();
                        } else { break; }
                    }
                    let v = i64::from_str_radix(&s, 2).map_err(|e| format!("{}:{}: {}", line, col, e))?;
                    return Ok(Token { kind: TokenKind::Int(v), line, col });
                } else if base_ch == 'o' || base_ch == 'O' {
                    self.advance();
                    self.advance();
                    while let Some(c) = self.peek() {
                        if ('0'..='7').contains(&c) || c == '_' {
                            if c != '_' { s.push(c); }
                            self.advance();
                        } else { break; }
                    }
                    let v = i64::from_str_radix(&s, 8).map_err(|e| format!("{}:{}: {}", line, col, e))?;
                    return Ok(Token { kind: TokenKind::Int(v), line, col });
                }
            }
        }

        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                if c != '_' { s.push(c); }
                self.advance();
            } else {
                break;
            }
        }
        if self.peek() == Some('.') && self.peek_n(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            is_float = true;
            s.push('.');
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() || c == '_' {
                    if c != '_' { s.push(c); }
                    self.advance();
                } else {
                    break;
                }
            }
        }
        if let Some(e) = self.peek() {
            if e == 'e' || e == 'E' {
                is_float = true;
                s.push(e);
                self.advance();
                if let Some(sign) = self.peek() {
                    if sign == '+' || sign == '-' {
                        s.push(sign);
                        self.advance();
                    }
                }
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || c == '_' {
                        if c != '_' { s.push(c); }
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }

        if is_float {
            let v: f64 = s.parse().map_err(|e| format!("{}:{}: {}", line, col, e))?;
            Ok(Token { kind: TokenKind::Float(v), line, col })
        } else {
            let v: i64 = s.parse().map_err(|e| format!("{}:{}: {}", line, col, e))?;
            Ok(Token { kind: TokenKind::Int(v), line, col })
        }
    }

    fn lex_single_string(&mut self, line: usize, col: usize) -> Result<Token, String> {
        self.advance(); // '\''
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == '\'' {
                let parts = if s.is_empty() { vec![] } else { vec![StrPartToken::Lit(s)] };
                return Ok(Token { kind: TokenKind::StringLit(parts), line, col });
            }
            if c == '\\' {
                if let Some(nc) = self.peek() {
                    if nc == '\'' || nc == '\\' {
                        s.push(nc);
                        self.advance();
                        continue;
                    }
                }
                s.push('\\');
            } else {
                s.push(c);
            }
        }
        Err(format!("{}:{}: unterminated single-quoted string", line, col))
    }

    fn lex_double_string(&mut self, line: usize, col: usize) -> Result<Token, String> {
        self.advance(); // '"'
        let mut parts = Vec::new();
        let mut cur = String::new();
        while let Some(c) = self.advance() {
            if c == '"' {
                if !cur.is_empty() {
                    parts.push(StrPartToken::Lit(cur));
                }
                return Ok(Token { kind: TokenKind::StringLit(parts), line, col });
            }
            if c == '#' && self.peek() == Some('{') {
                if !cur.is_empty() {
                    parts.push(StrPartToken::Lit(std::mem::take(&mut cur)));
                }
                self.advance(); // '{'
                let mut depth = 1;
                let mut interp_src = String::new();
                let mut in_sq = false;
                let mut in_dq = false;
                while let Some(ic) = self.advance() {
                    if ic == '\\' && (in_sq || in_dq) {
                        interp_src.push(ic);
                        if let Some(next_c) = self.advance() {
                            interp_src.push(next_c);
                        }
                        continue;
                    }
                    if ic == '\'' && !in_dq { in_sq = !in_sq; }
                    else if ic == '"' && !in_sq { in_dq = !in_dq; }
                    else if !in_sq && !in_dq {
                        if ic == '{' { depth += 1; }
                        else if ic == '}' {
                            depth -= 1;
                            if depth == 0 { break; }
                        }
                    }
                    interp_src.push(ic);
                }
                if depth != 0 {
                    return Err(format!("{}:{}: unterminated string interpolation", line, col));
                }
                let mut sub_lexer = Lexer::new(&interp_src);
                let sub_tokens = sub_lexer.tokenize()?;
                parts.push(StrPartToken::Interp(sub_tokens));
                continue;
            }
            if c == '\\' {
                match self.advance() {
                    Some('n') => cur.push('\n'),
                    Some('t') => cur.push('\t'),
                    Some('r') => cur.push('\r'),
                    Some('\\') => cur.push('\\'),
                    Some('"') => cur.push('"'),
                    Some('#') => cur.push('#'),
                    Some('0') => cur.push('\0'),
                    Some(other) => cur.push(other),
                    None => return Err(format!("{}:{}: unterminated escape", line, col)),
                }
            } else {
                cur.push(c);
            }
        }
        Err(format!("{}:{}: unterminated double-quoted string", line, col))
    }

    fn lex_operator(&mut self, line: usize, col: usize) -> Result<Token, String> {
        let c = self.advance().unwrap();
        let kind = match c {
            '+' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::PlusEq }
                else { TokenKind::Plus }
            }
            '-' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::MinusEq }
                else if self.peek() == Some('>') { self.advance(); TokenKind::Arrow }
                else { TokenKind::Minus }
            }
            '*' => {
                if self.peek() == Some('*') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::StarStarEq }
                    else { TokenKind::StarStar }
                } else if self.peek() == Some('=') {
                    self.advance(); TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            '/' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::SlashEq }
                else { TokenKind::Slash }
            }
            '%' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::PercentEq }
                else { TokenKind::Percent }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::EqEqEq }
                    else { TokenKind::EqEq }
                } else if self.peek() == Some('>') {
                    self.advance(); TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::NotEq }
                else { TokenKind::Bang }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('>') { self.advance(); TokenKind::Spaceship }
                    else { TokenKind::Le }
                } else if self.peek() == Some('<') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::ShlEq }
                    else { TokenKind::Shl }
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::Ge }
                else if self.peek() == Some('>') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::ShrEq }
                    else { TokenKind::Shr }
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::AndAndEq }
                    else { TokenKind::AndAnd }
                } else if self.peek() == Some('=') {
                    self.advance(); TokenKind::AmpEq
                } else if self.peek() == Some('.') {
                    self.advance(); TokenKind::SafeNav
                } else {
                    TokenKind::Amp
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    if self.peek() == Some('=') { self.advance(); TokenKind::OrOrEq }
                    else { TokenKind::OrOr }
                } else if self.peek() == Some('=') {
                    self.advance(); TokenKind::PipeEq
                } else {
                    TokenKind::Pipe
                }
            }
            '^' => {
                if self.peek() == Some('=') { self.advance(); TokenKind::CaretEq }
                else { TokenKind::Caret }
            }
            '~' => TokenKind::Tilde,
            '.' => {
                if self.peek() == Some('.') {
                    self.advance();
                    if self.peek() == Some('.') { self.advance(); TokenKind::DotDotDot }
                    else { TokenKind::DotDot }
                } else {
                    TokenKind::Dot
                }
            }
            '?' => TokenKind::Question,
            ';' => TokenKind::Semicolon,
            ',' => TokenKind::Comma,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            other => return Err(format!("{}:{}: unexpected character '{}'", line, col, other)),
        };
        Ok(Token { kind, line, col })
    }
}
