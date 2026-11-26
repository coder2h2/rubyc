use crate::ast::*;
use crate::lexer::{Lexer, StrPartToken, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek_n(&self, n: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn cur_loc(&self) -> (usize, usize) {
        self.tokens
            .get(self.pos)
            .map(|t| (t.line, t.col))
            .unwrap_or((0, 0))
    }

    fn advance(&mut self) -> TokenKind {
        let k = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        k
    }

    fn skip_seps(&mut self) {
        while matches!(self.peek(), TokenKind::Newline | TokenKind::Semicolon) {
            self.advance();
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            let (l, c) = self.cur_loc();
            Err(format!("{}:{}: expected {:?}, got {:?}", l, c, expected, self.peek()))
        }
    }

    fn expect_sep(&mut self) -> Result<(), String> {
        if matches!(
            self.peek(),
            TokenKind::Newline
                | TokenKind::Semicolon
                | TokenKind::Eof
                | TokenKind::End
                | TokenKind::Else
                | TokenKind::Elsif
                | TokenKind::When
                | TokenKind::Rescue
                | TokenKind::Ensure
                | TokenKind::RBrace
                | TokenKind::RParen
        ) {
            self.skip_seps();
            Ok(())
        } else {
            let (l, c) = self.cur_loc();
            Err(format!("{}:{}: expected statement separator, got {:?}", l, c, self.peek()))
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, String> {
        let mut stmts = Vec::new();
        self.skip_seps();
        while !matches!(self.peek(), TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
            self.expect_sep()?;
        }
        Ok(Program { stmts })
    }

    fn parse_body_until(&mut self, stops: &[TokenKind]) -> Result<Vec<Expr>, String> {
        let mut stmts = Vec::new();
        self.skip_seps();
        while !stops.contains(self.peek()) && !matches!(self.peek(), TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
            self.expect_sep()?;
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_expr_with_multi_assign()?;
        loop {
            match self.peek() {
                TokenKind::If => {
                    self.advance();
                    let cond = self.parse_expr()?;
                    expr = Expr::If {
                        cond: Box::new(cond),
                        then_body: vec![expr],
                        elsifs: vec![],
                        else_body: None,
                    };
                }
                TokenKind::Unless => {
                    self.advance();
                    let cond = self.parse_expr()?;
                    expr = Expr::If {
                        cond: Box::new(Expr::UnaryOp {
                            op: UnaryOp::Not,
                            expr: Box::new(cond),
                        }),
                        then_body: vec![expr],
                        elsifs: vec![],
                        else_body: None,
                    };
                }
                TokenKind::While => {
                    self.advance();
                    let cond = self.parse_expr()?;
                    expr = Expr::While {
                        cond: Box::new(cond),
                        body: vec![expr],
                        is_until: false,
                    };
                }
                TokenKind::Until => {
                    self.advance();
                    let cond = self.parse_expr()?;
                    expr = Expr::While {
                        cond: Box::new(cond),
                        body: vec![expr],
                        is_until: true,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn is_assign_target_token(k: &TokenKind) -> bool {
        matches!(
            k,
            TokenKind::Ident(_)
                | TokenKind::Ivar(_)
                | TokenKind::Cvar(_)
                | TokenKind::Gvar(_)
                | TokenKind::Const(_)
        )
    }

    fn expr_to_assign_target(e: &Expr) -> Option<AssignTarget> {
        match e {
            Expr::LocalVar(s) => Some(AssignTarget::Local(s.clone())),
            Expr::Ivar(s) => Some(AssignTarget::Ivar(s.clone())),
            Expr::Cvar(s) => Some(AssignTarget::Cvar(s.clone())),
            Expr::Gvar(s) => Some(AssignTarget::Gvar(s.clone())),
            Expr::Const(s) => Some(AssignTarget::Const(s.clone())),
            _ => None,
        }
    }

    fn parse_expr_with_multi_assign(&mut self) -> Result<Expr, String> {
        // Check for multi-assignment: a, b = ...
        if Self::is_assign_target_token(self.peek()) && matches!(self.peek_n(1), TokenKind::Comma) {
            let save = self.pos;
            let mut targets = Vec::new();
            let mut ok = true;
            loop {
                match self.peek().clone() {
                    TokenKind::Ident(s) => { self.advance(); targets.push(AssignTarget::Local(s)); }
                    TokenKind::Ivar(s) => { self.advance(); targets.push(AssignTarget::Ivar(s)); }
                    TokenKind::Cvar(s) => { self.advance(); targets.push(AssignTarget::Cvar(s)); }
                    TokenKind::Gvar(s) => { self.advance(); targets.push(AssignTarget::Gvar(s)); }
                    TokenKind::Const(s) => { self.advance(); targets.push(AssignTarget::Const(s)); }
                    _ => { ok = false; break; }
                }
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            if ok && targets.len() >= 2 && matches!(self.peek(), TokenKind::Eq) {
                self.advance(); // '='
                let mut values = vec![self.parse_expr()?];
                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                    values.push(self.parse_expr()?);
                }
                return Ok(Expr::MultiAssign { targets, values });
            }
            self.pos = save;
        }
        self.parse_expr()
    }

    pub fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_kw_logic()
    }

    fn parse_kw_logic(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), TokenKind::NotKw) {
            self.advance();
            let e = self.parse_kw_logic()?;
            return Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(e),
            });
        }
        let mut left = self.parse_assign()?;
        loop {
            match self.peek() {
                TokenKind::AndKw => {
                    self.advance();
                    let right = self.parse_assign()?;
                    left = Expr::BinOp { op: BinOp::And, left: Box::new(left), right: Box::new(right) };
                }
                TokenKind::OrKw => {
                    self.advance();
                    let right = self.parse_assign()?;
                    left = Expr::BinOp { op: BinOp::Or, left: Box::new(left), right: Box::new(right) };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn assign_op_kind(k: &TokenKind) -> Option<Option<BinOp>> {
        match k {
            TokenKind::Eq => Some(None),
            TokenKind::PlusEq => Some(Some(BinOp::Add)),
            TokenKind::MinusEq => Some(Some(BinOp::Sub)),
            TokenKind::StarEq => Some(Some(BinOp::Mul)),
            TokenKind::SlashEq => Some(Some(BinOp::Div)),
            TokenKind::PercentEq => Some(Some(BinOp::Mod)),
            TokenKind::StarStarEq => Some(Some(BinOp::Pow)),
            TokenKind::AndAndEq => Some(Some(BinOp::And)),
            TokenKind::OrOrEq => Some(Some(BinOp::Or)),
            TokenKind::AmpEq => Some(Some(BinOp::BitAnd)),
            TokenKind::PipeEq => Some(Some(BinOp::BitOr)),
            TokenKind::CaretEq => Some(Some(BinOp::BitXor)),
            TokenKind::ShlEq => Some(Some(BinOp::Shl)),
            TokenKind::ShrEq => Some(Some(BinOp::Shr)),
            _ => None,
        }
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let left = self.parse_ternary()?;
        if let Some(op_opt) = Self::assign_op_kind(self.peek()) {
            self.advance();
            let val = self.parse_assign()?;
            if let Some(target) = Self::expr_to_assign_target(&left) {
                return Ok(Expr::Assign {
                    target,
                    op: op_opt,
                    value: Box::new(val),
                });
            }
            match left {
                Expr::Index { receiver, args } => {
                    return Ok(Expr::IndexAssign {
                        receiver,
                        args,
                        op: op_opt,
                        value: Box::new(val),
                    });
                }
                Expr::Call {
                    receiver: Some(recv),
                    name,
                    args,
                    safe_nav,
                    ..
                } if args.is_empty() && op_opt.is_none() => {
                    return Ok(Expr::Call {
                        receiver: Some(recv),
                        name: format!("{}=", name),
                        args: vec![CallArg { expr: val, is_splat: false }],
                        block: None,
                        safe_nav,
                    });
                }
                _ => {
                    let (l, c) = self.cur_loc();
                    return Err(format!("{}:{}: invalid assignment target", l, c));
                }
            }
        }
        Ok(left)
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let cond = self.parse_range()?;
        if matches!(self.peek(), TokenKind::Question) {
            self.advance();
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon)?;
            let else_expr = self.parse_expr()?;
            return Ok(Expr::If {
                cond: Box::new(cond),
                then_body: vec![then_expr],
                elsifs: vec![],
                else_body: Some(vec![else_expr]),
            });
        }
        Ok(cond)
    }

    fn parse_range(&mut self) -> Result<Expr, String> {
        let left = self.parse_or()?;
        match self.peek() {
            TokenKind::DotDot => {
                self.advance();
                let right = self.parse_or()?;
                Ok(Expr::Range {
                    start: Box::new(left),
                    end: Box::new(right),
                    exclusive: false,
                })
            }
            TokenKind::DotDotDot => {
                self.advance();
                let right = self.parse_or()?;
                Ok(Expr::Range {
                    start: Box::new(left),
                    end: Box::new(right),
                    exclusive: true,
                })
            }
            _ => Ok(left),
        }
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), TokenKind::OrOr) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinOp { op: BinOp::Or, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_eq()?;
        while matches!(self.peek(), TokenKind::AndAnd) {
            self.advance();
            let right = self.parse_eq()?;
            left = Expr::BinOp { op: BinOp::And, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_eq(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_cmp()?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::NotEq => BinOp::Neq,
                TokenKind::Spaceship => BinOp::Cmp,
                TokenKind::EqEqEq => BinOp::CaseEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_cmp()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitor()?;
        loop {
            let op = match self.peek() {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::Ge => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_bitor()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bitor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitand()?;
        loop {
            let op = match self.peek() {
                TokenKind::Pipe => BinOp::BitOr,
                TokenKind::Caret => BinOp::BitXor,
                _ => break,
            };
            self.advance();
            let right = self.parse_bitand()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_bitand(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_shift()?;
        while matches!(self.peek(), TokenKind::Amp) {
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::BinOp { op: BinOp::BitAnd, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek() {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.advance();
            let right = self.parse_add()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            TokenKind::Minus => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::Neg, expr: Box::new(e) })
            }
            TokenKind::Plus => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::Pos, expr: Box::new(e) })
            }
            TokenKind::Bang => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::Not, expr: Box::new(e) })
            }
            TokenKind::Tilde => {
                self.advance();
                let e = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::BitNot, expr: Box::new(e) })
            }
            _ => self.parse_pow(),
        }
    }

    fn parse_pow(&mut self) -> Result<Expr, String> {
        let base = self.parse_postfix()?;
        if matches!(self.peek(), TokenKind::StarStar) {
            self.advance();
            let exp = self.parse_unary()?;
            return Ok(Expr::BinOp {
                op: BinOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            });
        }
        Ok(base)
    }

    fn is_arg_start(k: &TokenKind) -> bool {
        matches!(
            k,
            TokenKind::Int(_)
                | TokenKind::Float(_)
                | TokenKind::Symbol(_)
                | TokenKind::StringLit(_)
                | TokenKind::Ident(_)
                | TokenKind::Const(_)
                | TokenKind::Ivar(_)
                | TokenKind::Cvar(_)
                | TokenKind::Gvar(_)
                | TokenKind::SelfKw
                | TokenKind::Nil
                | TokenKind::True
                | TokenKind::False
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::Minus
                | TokenKind::Plus
                | TokenKind::Star
                | TokenKind::Arrow
                | TokenKind::NotKw
        )
    }

    fn is_cmd_arg_start(k: &TokenKind) -> bool {
        Self::is_arg_start(k)
            && !matches!(
                k,
                TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::LBracket
            )
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                TokenKind::Dot | TokenKind::SafeNav => {
                    let safe_nav = matches!(self.peek(), TokenKind::SafeNav);
                    self.advance();
                    self.skip_newlines();
                    let name = self.parse_method_name()?;
                    let args = if matches!(self.peek(), TokenKind::LParen) {
                        self.parse_paren_args()?
                    } else if Self::is_cmd_arg_start(self.peek())
                        && !matches!(self.peek(), TokenKind::LBrace)
                    {
                        self.parse_cmd_args()?
                    } else {
                        vec![]
                    };
                    let block = self.try_parse_block()?;
                    expr = Expr::Call {
                        receiver: Some(Box::new(expr)),
                        name,
                        args,
                        block,
                        safe_nav,
                    };
                }
                TokenKind::ColonColon => {
                    self.advance();
                    match self.peek().clone() {
                        TokenKind::Const(cname) => {
                            self.advance();
                            expr = Expr::ConstPath(Box::new(expr), cname);
                        }
                        _ => {
                            let name = self.parse_method_name()?;
                            let args = if matches!(self.peek(), TokenKind::LParen) {
                                self.parse_paren_args()?
                            } else {
                                vec![]
                            };
                            let block = self.try_parse_block()?;
                            expr = Expr::Call {
                                receiver: Some(Box::new(expr)),
                                name,
                                args,
                                block,
                                safe_nav: false,
                            };
                        }
                    }
                }
                TokenKind::LBracket => {
                    self.advance();
                    let mut args = Vec::new();
                    self.skip_newlines();
                    if !matches!(self.peek(), TokenKind::RBracket) {
                        args.push(self.parse_expr()?);
                        while matches!(self.peek(), TokenKind::Comma) {
                            self.advance();
                            self.skip_newlines();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.skip_newlines();
                    self.expect(&TokenKind::RBracket)?;
                    expr = Expr::Index {
                        receiver: Box::new(expr),
                        args,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_method_name(&mut self) -> Result<String, String> {
        match self.advance() {
            TokenKind::Ident(s) | TokenKind::Const(s) => Ok(s),
            TokenKind::Class => Ok("class".to_string()),
            TokenKind::Next => Ok("next".to_string()),
            TokenKind::Plus => Ok("+".to_string()),
            TokenKind::Minus => Ok("-".to_string()),
            TokenKind::Star => Ok("*".to_string()),
            TokenKind::Slash => Ok("/".to_string()),
            TokenKind::Percent => Ok("%".to_string()),
            TokenKind::StarStar => Ok("**".to_string()),
            TokenKind::EqEq => Ok("==".to_string()),
            TokenKind::NotEq => Ok("!=".to_string()),
            TokenKind::Lt => Ok("<".to_string()),
            TokenKind::Le => Ok("<=".to_string()),
            TokenKind::Gt => Ok(">".to_string()),
            TokenKind::Ge => Ok(">=".to_string()),
            TokenKind::Spaceship => Ok("<=>".to_string()),
            TokenKind::EqEqEq => Ok("===".to_string()),
            TokenKind::Shl => Ok("<<".to_string()),
            TokenKind::Shr => Ok(">>".to_string()),
            other => {
                let (l, c) = self.cur_loc();
                Err(format!("{}:{}: expected method name, got {:?}", l, c, other))
            }
        }
    }

    fn parse_paren_args(&mut self) -> Result<Vec<CallArg>, String> {
        self.expect(&TokenKind::LParen)?;
        self.skip_newlines();
        let mut args = Vec::new();
        if !matches!(self.peek(), TokenKind::RParen) {
            loop {
                let is_splat = if matches!(self.peek(), TokenKind::Star) {
                    self.advance();
                    true
                } else {
                    false
                };
                if matches!(self.peek(), TokenKind::Ident(_)) && matches!(self.peek_n(1), TokenKind::Colon) {
                    // Keyword arg `key: val` -> pass as Symbol/Hash or simple expr?
                    // For simplicity in this compiler, treat `k: v` as hash entry if collected, or parse normally
                    let mut pairs = Vec::new();
                    loop {
                        if let TokenKind::Ident(k) = self.peek().clone() {
                            if matches!(self.peek_n(1), TokenKind::Colon) {
                                self.advance();
                                self.advance();
                                let v = self.parse_expr()?;
                                pairs.push((Expr::Symbol(k), v));
                                if matches!(self.peek(), TokenKind::Comma) {
                                    self.advance();
                                    self.skip_newlines();
                                    continue;
                                }
                            }
                        }
                        break;
                    }
                    args.push(CallArg { expr: Expr::Hash(pairs), is_splat: false });
                    break;
                }
                args.push(CallArg {
                    expr: self.parse_expr()?,
                    is_splat,
                });
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                    self.skip_newlines();
                } else {
                    break;
                }
            }
        }
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;
        Ok(args)
    }

    fn parse_cmd_args(&mut self) -> Result<Vec<CallArg>, String> {
        let mut args = Vec::new();
        loop {
            let is_splat = if matches!(self.peek(), TokenKind::Star) {
                self.advance();
                true
            } else {
                false
            };
            args.push(CallArg {
                expr: self.parse_expr()?,
                is_splat,
            });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn try_parse_block(&mut self) -> Result<Option<Block>, String> {
        if matches!(self.peek(), TokenKind::Do) {
            self.advance();
            let params = self.parse_block_params()?;
            let body = self.parse_body_until(&[TokenKind::End])?;
            self.expect(&TokenKind::End)?;
            return Ok(Some(Block { params, body }));
        }
        if matches!(self.peek(), TokenKind::LBrace) {
            self.advance();
            let params = self.parse_block_params()?;
            let body = self.parse_body_until(&[TokenKind::RBrace])?;
            self.expect(&TokenKind::RBrace)?;
            return Ok(Some(Block { params, body }));
        }
        Ok(None)
    }

    fn parse_block_params(&mut self) -> Result<Vec<Param>, String> {
        if !matches!(self.peek(), TokenKind::Pipe) {
            return Ok(vec![]);
        }
        self.advance(); // '|'
        let mut params = Vec::new();
        if !matches!(self.peek(), TokenKind::Pipe) {
            loop {
                let is_splat = if matches!(self.peek(), TokenKind::Star) {
                    self.advance();
                    true
                } else {
                    false
                };
                if let TokenKind::Ident(name) = self.advance() {
                    params.push(Param {
                        name,
                        default: None,
                        is_splat,
                        is_block: false,
                    });
                } else {
                    let (l, c) = self.cur_loc();
                    return Err(format!("{}:{}: expected block parameter name", l, c));
                }
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(&TokenKind::Pipe)?;
        Ok(params)
    }

    fn parse_def_params(&mut self) -> Result<Vec<Param>, String> {
        if !matches!(self.peek(), TokenKind::LParen) {
            if Self::is_arg_start(self.peek()) {
                return self.parse_bare_def_params();
            }
            return Ok(vec![]);
        }
        self.advance(); // '('
        let mut params = Vec::new();
        self.skip_newlines();
        if !matches!(self.peek(), TokenKind::RParen) {
            loop {
                let mut is_splat = false;
                let mut is_block = false;
                if matches!(self.peek(), TokenKind::Star) {
                    self.advance();
                    is_splat = true;
                } else if matches!(self.peek(), TokenKind::Amp) {
                    self.advance();
                    is_block = true;
                }
                let name = match self.advance() {
                    TokenKind::Ident(s) => s,
                    other => {
                        let (l, c) = self.cur_loc();
                        return Err(format!("{}:{}: expected parameter name, got {:?}", l, c, other));
                    }
                };
                let default = if matches!(self.peek(), TokenKind::Eq) {
                    self.advance();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(Param { name, default, is_splat, is_block });
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                    self.skip_newlines();
                } else {
                    break;
                }
            }
        }
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;
        Ok(params)
    }

    fn parse_bare_def_params(&mut self) -> Result<Vec<Param>, String> {
        let mut params = Vec::new();
        loop {
            let mut is_splat = false;
            let mut is_block = false;
            if matches!(self.peek(), TokenKind::Star) {
                self.advance();
                is_splat = true;
            } else if matches!(self.peek(), TokenKind::Amp) {
                self.advance();
                is_block = true;
            }
            let name = match self.advance() {
                TokenKind::Ident(s) => s,
                other => {
                    let (l, c) = self.cur_loc();
                    return Err(format!("{}:{}: expected parameter name, got {:?}", l, c, other));
                }
            };
            let default = if matches!(self.peek(), TokenKind::Eq) {
                self.advance();
                Some(self.parse_expr()?)
            } else {
                None
            };
            params.push(Param { name, default, is_splat, is_block });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(params)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let (line, col) = self.cur_loc();
        match self.peek().clone() {
            TokenKind::Int(n) => { self.advance(); Ok(Expr::Int(n)) }
            TokenKind::Float(f) => { self.advance(); Ok(Expr::Float(f)) }
            TokenKind::True => { self.advance(); Ok(Expr::Bool(true)) }
            TokenKind::False => { self.advance(); Ok(Expr::Bool(false)) }
            TokenKind::Nil => { self.advance(); Ok(Expr::Nil) }
            TokenKind::SelfKw => { self.advance(); Ok(Expr::SelfExpr) }
            TokenKind::Symbol(s) => { self.advance(); Ok(Expr::Symbol(s)) }
            TokenKind::Ivar(s) => { self.advance(); Ok(Expr::Ivar(s)) }
            TokenKind::Cvar(s) => { self.advance(); Ok(Expr::Cvar(s)) }
            TokenKind::Gvar(s) => { self.advance(); Ok(Expr::Gvar(s)) }
            TokenKind::Const(s) => { self.advance(); Ok(Expr::Const(s)) }

            TokenKind::StringLit(parts) => {
                self.advance();
                let mut ast_parts = Vec::new();
                for p in parts {
                    match p {
                        StrPartToken::Lit(s) => ast_parts.push(StringPart::Literal(s)),
                        StrPartToken::Interp(toks) => {
                            let mut sub_p = Parser::new(toks);
                            let prog = sub_p.parse_program()?;
                            let e = if prog.stmts.is_empty() {
                                Expr::Nil
                            } else if prog.stmts.len() == 1 {
                                prog.stmts.into_iter().next().unwrap()
                            } else {
                                Expr::Begin { body: prog.stmts, rescues: vec![], ensure: None }
                            };
                            ast_parts.push(StringPart::Interp(e));
                        }
                    }
                }
                Ok(Expr::StringLit(ast_parts))
            }

            TokenKind::Ident(name) => {
                self.advance();
                if matches!(self.peek(), TokenKind::LParen) {
                    let args = self.parse_paren_args()?;
                    let block = self.try_parse_block()?;
                    Ok(Expr::Call {
                        receiver: None,
                        name,
                        args,
                        block,
                        safe_nav: false,
                    })
                } else if matches!(self.peek(), TokenKind::Do | TokenKind::LBrace) {
                    let block = self.try_parse_block()?;
                    Ok(Expr::Call {
                        receiver: None,
                        name,
                        args: vec![],
                        block,
                        safe_nav: false,
                    })
                } else if Self::is_arg_start(self.peek())
                    && !matches!(
                        self.peek(),
                        TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::LBracket
                    )
                {
                    let args = self.parse_cmd_args()?;
                    let block = self.try_parse_block()?;
                    Ok(Expr::Call {
                        receiver: None,
                        name,
                        args,
                        block,
                        safe_nav: false,
                    })
                } else {
                    Ok(Expr::LocalVar(name))
                }
            }

            TokenKind::LParen => {
                self.advance();
                self.skip_newlines();
                let mut stmts = Vec::new();
                while !matches!(self.peek(), TokenKind::RParen | TokenKind::Eof) {
                    stmts.push(self.parse_stmt()?);
                    if matches!(self.peek(), TokenKind::Newline | TokenKind::Semicolon) {
                        self.skip_seps();
                    } else {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
                if stmts.is_empty() {
                    Ok(Expr::Nil)
                } else if stmts.len() == 1 {
                    Ok(stmts.into_iter().next().unwrap())
                } else {
                    Ok(Expr::Begin { body: stmts, rescues: vec![], ensure: None })
                }
            }

            TokenKind::LBracket => {
                self.advance();
                self.skip_newlines();
                let mut items = Vec::new();
                if !matches!(self.peek(), TokenKind::RBracket) {
                    items.push(self.parse_expr()?);
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        self.skip_newlines();
                        if matches!(self.peek(), TokenKind::RBracket) { break; }
                        items.push(self.parse_expr()?);
                    }
                }
                self.skip_newlines();
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::Array(items))
            }

            TokenKind::LBrace => {
                self.advance();
                self.skip_newlines();
                let mut pairs = Vec::new();
                if !matches!(self.peek(), TokenKind::RBrace) {
                    loop {
                        if let TokenKind::Ident(k) = self.peek().clone() {
                            if matches!(self.peek_n(1), TokenKind::Colon) {
                                self.advance();
                                self.advance();
                                let v = self.parse_expr()?;
                                pairs.push((Expr::Symbol(k), v));
                                if matches!(self.peek(), TokenKind::Comma) {
                                    self.advance();
                                    self.skip_newlines();
                                    if matches!(self.peek(), TokenKind::RBrace) { break; }
                                    continue;
                                } else {
                                    break;
                                }
                            }
                        }
                        let k = self.parse_expr()?;
                        self.expect(&TokenKind::FatArrow)?;
                        let v = self.parse_expr()?;
                        pairs.push((k, v));
                        if matches!(self.peek(), TokenKind::Comma) {
                            self.advance();
                            self.skip_newlines();
                            if matches!(self.peek(), TokenKind::RBrace) { break; }
                        } else {
                            break;
                        }
                    }
                }
                self.skip_newlines();
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::Hash(pairs))
            }

            TokenKind::Arrow => {
                self.advance();
                let params = if matches!(self.peek(), TokenKind::LParen) {
                    self.parse_def_params()?
                } else {
                    vec![]
                };
                if matches!(self.peek(), TokenKind::LBrace) {
                    self.advance();
                    let body = self.parse_body_until(&[TokenKind::RBrace])?;
                    self.expect(&TokenKind::RBrace)?;
                    Ok(Expr::Lambda { params, body })
                } else if matches!(self.peek(), TokenKind::Do) {
                    self.advance();
                    let body = self.parse_body_until(&[TokenKind::End])?;
                    self.expect(&TokenKind::End)?;
                    Ok(Expr::Lambda { params, body })
                } else {
                    Err(format!("{}:{}: expected {{ or do after ->", line, col))
                }
            }

            TokenKind::If | TokenKind::Unless => {
                let is_unless = matches!(self.advance(), TokenKind::Unless);
                let mut cond = self.parse_expr()?;
                if is_unless {
                    cond = Expr::UnaryOp { op: UnaryOp::Not, expr: Box::new(cond) };
                }
                if matches!(self.peek(), TokenKind::Then) { self.advance(); }
                let then_body = self.parse_body_until(&[TokenKind::Elsif, TokenKind::Else, TokenKind::End])?;
                let mut elsifs = Vec::new();
                while matches!(self.peek(), TokenKind::Elsif) {
                    self.advance();
                    let e_cond = self.parse_expr()?;
                    if matches!(self.peek(), TokenKind::Then) { self.advance(); }
                    let e_body = self.parse_body_until(&[TokenKind::Elsif, TokenKind::Else, TokenKind::End])?;
                    elsifs.push((e_cond, e_body));
                }
                let else_body = if matches!(self.peek(), TokenKind::Else) {
                    self.advance();
                    Some(self.parse_body_until(&[TokenKind::End])?)
                } else {
                    None
                };
                self.expect(&TokenKind::End)?;
                Ok(Expr::If { cond: Box::new(cond), then_body, elsifs, else_body })
            }

            TokenKind::While | TokenKind::Until => {
                let is_until = matches!(self.advance(), TokenKind::Until);
                let cond = self.parse_expr()?;
                if matches!(self.peek(), TokenKind::Do) { self.advance(); }
                let body = self.parse_body_until(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Expr::While { cond: Box::new(cond), body, is_until })
            }

            TokenKind::For => {
                self.advance();
                let var = match self.advance() {
                    TokenKind::Ident(s) => s,
                    _ => return Err(format!("{}:{}: expected loop variable after for", line, col)),
                };
                self.expect(&TokenKind::In)?;
                let iter = self.parse_expr()?;
                if matches!(self.peek(), TokenKind::Do) { self.advance(); }
                let body = self.parse_body_until(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Expr::For { var, iter: Box::new(iter), body })
            }

            TokenKind::Case => {
                self.advance();
                let expr = if !matches!(self.peek(), TokenKind::When | TokenKind::Newline | TokenKind::Semicolon) {
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                self.skip_seps();
                let mut whens = Vec::new();
                while matches!(self.peek(), TokenKind::When) {
                    self.advance();
                    let mut pats = vec![self.parse_expr()?];
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        pats.push(self.parse_expr()?);
                    }
                    if matches!(self.peek(), TokenKind::Then) { self.advance(); }
                    let body = self.parse_body_until(&[TokenKind::When, TokenKind::Else, TokenKind::End])?;
                    whens.push((pats, body));
                }
                let else_body = if matches!(self.peek(), TokenKind::Else) {
                    self.advance();
                    Some(self.parse_body_until(&[TokenKind::End])?)
                } else {
                    None
                };
                self.expect(&TokenKind::End)?;
                Ok(Expr::Case { expr, whens, else_body })
            }

            TokenKind::Break => {
                self.advance();
                let val = if Self::is_arg_start(self.peek()) { Some(Box::new(self.parse_expr()?)) } else { None };
                Ok(Expr::Break(val))
            }
            TokenKind::Next => {
                self.advance();
                let val = if Self::is_arg_start(self.peek()) { Some(Box::new(self.parse_expr()?)) } else { None };
                Ok(Expr::Next(val))
            }
            TokenKind::Return => {
                self.advance();
                let val = if Self::is_arg_start(self.peek()) { Some(Box::new(self.parse_expr()?)) } else { None };
                Ok(Expr::Return(val))
            }
            TokenKind::Yield => {
                self.advance();
                let args = if matches!(self.peek(), TokenKind::LParen) {
                    self.parse_paren_args()?.into_iter().map(|a| a.expr).collect()
                } else if Self::is_cmd_arg_start(self.peek()) {
                    self.parse_cmd_args()?.into_iter().map(|a| a.expr).collect()
                } else {
                    vec![]
                };
                Ok(Expr::Yield(args))
            }
            TokenKind::Super => {
                self.advance();
                let args = if matches!(self.peek(), TokenKind::LParen) {
                    self.parse_paren_args()?
                } else if Self::is_cmd_arg_start(self.peek()) {
                    self.parse_cmd_args()?
                } else {
                    vec![]
                };
                Ok(Expr::Super(args))
            }

            TokenKind::Def => {
                self.advance();
                let mut singleton = None;
                let mut name = match self.peek().clone() {
                    TokenKind::SelfKw => {
                        self.advance();
                        self.expect(&TokenKind::Dot)?;
                        singleton = Some(Box::new(Expr::SelfExpr));
                        self.parse_method_name()?
                    }
                    TokenKind::Const(c) if matches!(self.peek_n(1), TokenKind::Dot) => {
                        self.advance();
                        self.advance();
                        singleton = Some(Box::new(Expr::Const(c)));
                        self.parse_method_name()?
                    }
                    TokenKind::Ident(v) if matches!(self.peek_n(1), TokenKind::Dot) => {
                        self.advance();
                        self.advance();
                        singleton = Some(Box::new(Expr::LocalVar(v)));
                        self.parse_method_name()?
                    }
                    _ => self.parse_method_name()?,
                };
                if matches!(self.peek(), TokenKind::Eq) {
                    self.advance();
                    name.push('=');
                }
                let params = self.parse_def_params()?;
                let body = self.parse_body_until(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Expr::Def { singleton, name, params, body })
            }

            TokenKind::Class => {
                self.advance();
                let name = match self.advance() {
                    TokenKind::Const(s) => s,
                    other => return Err(format!("{}:{}: expected class name, got {:?}", line, col, other)),
                };
                let superclass = if matches!(self.peek(), TokenKind::Lt) {
                    self.advance();
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                let body = self.parse_body_until(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Expr::Class { name, superclass, body })
            }

            TokenKind::Module => {
                self.advance();
                let name = match self.advance() {
                    TokenKind::Const(s) => s,
                    other => return Err(format!("{}:{}: expected module name, got {:?}", line, col, other)),
                };
                let body = self.parse_body_until(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Expr::Module { name, body })
            }

            TokenKind::Begin => {
                self.advance();
                let body = self.parse_body_until(&[TokenKind::Rescue, TokenKind::Ensure, TokenKind::End])?;
                let mut rescues = Vec::new();
                while matches!(self.peek(), TokenKind::Rescue) {
                    self.advance();
                    let mut classes = Vec::new();
                    let mut var = None;
                    if !matches!(self.peek(), TokenKind::Newline | TokenKind::Semicolon | TokenKind::FatArrow) {
                        classes.push(self.parse_expr()?);
                        while matches!(self.peek(), TokenKind::Comma) {
                            self.advance();
                            classes.push(self.parse_expr()?);
                        }
                    }
                    if matches!(self.peek(), TokenKind::FatArrow) {
                        self.advance();
                        if let TokenKind::Ident(v) = self.advance() {
                            var = Some(v);
                        } else {
                            return Err(format!("{}:{}: expected rescue variable name", line, col));
                        }
                    }
                    let rbody = self.parse_body_until(&[TokenKind::Rescue, TokenKind::Ensure, TokenKind::End])?;
                    rescues.push(RescueClause { classes, var, body: rbody });
                }
                let ensure = if matches!(self.peek(), TokenKind::Ensure) {
                    self.advance();
                    Some(self.parse_body_until(&[TokenKind::End])?)
                } else {
                    None
                };
                self.expect(&TokenKind::End)?;
                Ok(Expr::Begin { body, rescues, ensure })
            }

            other => Err(format!("{}:{}: unexpected token {:?}", line, col, other)),
        }
    }
}

pub fn parse_source(src: &str) -> Result<Program, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}
