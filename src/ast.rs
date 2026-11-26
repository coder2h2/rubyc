#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    Cmp,
    CaseEq,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
    BitNot,
}

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Interp(Expr),
}

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Local(String),
    Ivar(String),
    Cvar(String),
    Gvar(String),
    Const(String),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    pub is_splat: bool,
    pub is_block: bool,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub params: Vec<Param>,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct CallArg {
    pub expr: Expr,
    #[allow(dead_code)]
    pub is_splat: bool,
}

#[derive(Debug, Clone)]
pub struct RescueClause {
    pub classes: Vec<Expr>,
    pub var: Option<String>,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Bool(bool),
    Nil,
    Symbol(String),
    StringLit(Vec<StringPart>),
    Array(Vec<Expr>),
    Hash(Vec<(Expr, Expr)>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        exclusive: bool,
    },
    LocalVar(String),
    Ivar(String),
    Cvar(String),
    Gvar(String),
    Const(String),
    ConstPath(Box<Expr>, String),
    SelfExpr,
    Assign {
        target: AssignTarget,
        op: Option<BinOp>,
        value: Box<Expr>,
    },
    MultiAssign {
        targets: Vec<AssignTarget>,
        values: Vec<Expr>,
    },
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_body: Vec<Expr>,
        elsifs: Vec<(Expr, Vec<Expr>)>,
        else_body: Option<Vec<Expr>>,
    },
    While {
        cond: Box<Expr>,
        body: Vec<Expr>,
        is_until: bool,
    },
    For {
        var: String,
        iter: Box<Expr>,
        body: Vec<Expr>,
    },
    Case {
        expr: Option<Box<Expr>>,
        whens: Vec<(Vec<Expr>, Vec<Expr>)>,
        else_body: Option<Vec<Expr>>,
    },
    Break(Option<Box<Expr>>),
    Next(Option<Box<Expr>>),
    Return(Option<Box<Expr>>),
    Yield(Vec<Expr>),
    Call {
        receiver: Option<Box<Expr>>,
        name: String,
        args: Vec<CallArg>,
        block: Option<Block>,
        safe_nav: bool,
    },
    Index {
        receiver: Box<Expr>,
        args: Vec<Expr>,
    },
    IndexAssign {
        receiver: Box<Expr>,
        args: Vec<Expr>,
        #[allow(dead_code)]
        op: Option<BinOp>,
        value: Box<Expr>,
    },
    Def {
        singleton: Option<Box<Expr>>,
        name: String,
        params: Vec<Param>,
        body: Vec<Expr>,
    },
    Class {
        name: String,
        superclass: Option<Box<Expr>>,
        body: Vec<Expr>,
    },
    Module {
        name: String,
        body: Vec<Expr>,
    },
    Lambda {
        params: Vec<Param>,
        body: Vec<Expr>,
    },
    Begin {
        body: Vec<Expr>,
        rescues: Vec<RescueClause>,
        ensure: Option<Vec<Expr>>,
    },
    Super(Vec<CallArg>),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Expr>,
}
