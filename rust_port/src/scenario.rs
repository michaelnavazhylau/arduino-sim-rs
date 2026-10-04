//! Typed, inspectable Rust scenarios, not embedded JavaScript source.
#[derive(Clone, Debug)]
pub enum Expr {
    Number(f64),
    Text(&'static str),
    Bool(bool),
    Null,
    Undefined,
    Var(&'static str),
    Get(Box<Expr>, Box<Expr>),
    Array(Vec<Expr>),
    Object(Vec<Field>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>, &'static str),
    Update(Box<Expr>, i32, bool),
    Call(Box<Expr>, Vec<Expr>),
    New(&'static str, Vec<Expr>),
    Function(Vec<Param>, Vec<Step>),
    Template(Vec<Expr>),
}
#[derive(Clone, Debug)]
pub enum Field {
    Entry(&'static str, Expr),
    Spread(Expr),
}
#[derive(Clone, Debug)]
pub struct Param {
    pub name: &'static str,
    pub rest: bool,
}
#[derive(Clone, Debug)]
pub enum Binding {
    Name(&'static str),
    Object(Vec<(&'static str, &'static str)>),
}
#[derive(Clone, Debug)]
pub struct Step {
    pub line: usize,
    pub action: Action,
}
#[derive(Clone, Debug)]
pub enum Action {
    Bind(Binding, Expr),
    Eval(Expr),
    Return(Expr),
    Throw(Expr),
    If(Expr, Vec<Step>, Vec<Step>),
    For(Vec<Step>, Expr, Expr, Vec<Step>),
    ForOf(&'static str, Expr, Vec<Step>),
    Assert {
        actual: Expr,
        matcher: &'static str,
        negated: bool,
        expected: Vec<Expr>,
    },
}
#[derive(Clone, Debug)]
pub struct Case {
    pub source: &'static str,
    pub name: &'static str,
    pub line: usize,
    pub assertions: usize,
    pub setup: Vec<Step>,
    pub body: Vec<Step>,
}

pub fn num(v: f64) -> Expr {
    Expr::Number(v)
}
pub fn text(v: &'static str) -> Expr {
    Expr::Text(v)
}
pub fn boolean(v: bool) -> Expr {
    Expr::Bool(v)
}
pub fn var(v: &'static str) -> Expr {
    Expr::Var(v)
}
pub fn get(o: Expr, k: Expr) -> Expr {
    Expr::Get(Box::new(o), Box::new(k))
}
pub fn array(v: Vec<Expr>) -> Expr {
    Expr::Array(v)
}
pub fn object(v: Vec<Field>) -> Expr {
    Expr::Object(v)
}
pub fn entry(k: &'static str, v: Expr) -> Field {
    Field::Entry(k, v)
}
pub fn spread(v: Expr) -> Field {
    Field::Spread(v)
}
pub fn binary(op: &'static str, a: Expr, b: Expr) -> Expr {
    Expr::Binary(op, Box::new(a), Box::new(b))
}
pub fn unary(op: &'static str, a: Expr) -> Expr {
    Expr::Unary(op, Box::new(a))
}
pub fn assign(a: Expr, b: Expr, op: &'static str) -> Expr {
    Expr::Assign(Box::new(a), Box::new(b), op)
}
pub fn update(a: Expr, delta: i32, prefix: bool) -> Expr {
    Expr::Update(Box::new(a), delta, prefix)
}
pub fn call(f: Expr, a: Vec<Expr>) -> Expr {
    Expr::Call(Box::new(f), a)
}
pub fn new(kind: &'static str, a: Vec<Expr>) -> Expr {
    Expr::New(kind, a)
}
pub fn function(p: Vec<Param>, b: Vec<Step>) -> Expr {
    Expr::Function(p, b)
}
pub fn param(name: &'static str, rest: bool) -> Param {
    Param { name, rest }
}
pub fn template(v: Vec<Expr>) -> Expr {
    Expr::Template(v)
}
pub fn at(line: usize, action: Action) -> Step {
    Step { line, action }
}
pub fn bind(n: &'static str, e: Expr) -> Action {
    Action::Bind(Binding::Name(n), e)
}
pub fn destructure(n: Vec<(&'static str, &'static str)>, e: Expr) -> Action {
    Action::Bind(Binding::Object(n), e)
}
pub fn eval(e: Expr) -> Action {
    Action::Eval(e)
}
pub fn ret(e: Expr) -> Action {
    Action::Return(e)
}
pub fn throw(e: Expr) -> Action {
    Action::Throw(e)
}
pub fn if_(e: Expr, a: Vec<Step>, b: Vec<Step>) -> Action {
    Action::If(e, a, b)
}
pub fn for_(init: Vec<Step>, test: Expr, next: Expr, body: Vec<Step>) -> Action {
    Action::For(init, test, next, body)
}
pub fn for_of(name: &'static str, values: Expr, body: Vec<Step>) -> Action {
    Action::ForOf(name, values, body)
}
pub fn check(actual: Expr, matcher: &'static str, negated: bool, expected: Vec<Expr>) -> Action {
    Action::Assert {
        actual,
        matcher,
        negated,
        expected,
    }
}

/// Count assertions including those in callback bodies, not only top-level checks.
pub fn assertion_count(steps: &[Step]) -> usize {
    fn expr(e: &Expr) -> usize {
        match e {
            Expr::Function(_, b) => assertion_count(b),
            Expr::Array(v) | Expr::Template(v) | Expr::New(_, v) => v.iter().map(expr).sum(),
            Expr::Object(v) => v
                .iter()
                .map(|f| match f {
                    Field::Entry(_, e) | Field::Spread(e) => expr(e),
                })
                .sum(),
            Expr::Get(a, b) | Expr::Binary(_, a, b) | Expr::Assign(a, b, _) => expr(a) + expr(b),
            Expr::Unary(_, e) | Expr::Update(e, _, _) => expr(e),
            Expr::Call(e, v) => expr(e) + v.iter().map(expr).sum::<usize>(),
            _ => 0,
        }
    }
    steps
        .iter()
        .map(|s| match &s.action {
            Action::Assert {
                actual, expected, ..
            } => 1 + expr(actual) + expected.iter().map(expr).sum::<usize>(),
            Action::Bind(_, e) | Action::Eval(e) | Action::Return(e) | Action::Throw(e) => expr(e),
            Action::If(e, a, b) => expr(e) + assertion_count(a) + assertion_count(b),
            Action::For(a, e, n, b) => assertion_count(a) + expr(e) + expr(n) + assertion_count(b),
            Action::ForOf(_, e, b) => expr(e) + assertion_count(b),
        })
        .sum()
}
