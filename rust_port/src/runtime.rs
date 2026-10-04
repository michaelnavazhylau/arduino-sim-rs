// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native Rust scenario runner and simulator adapter contract.
use crate::scenario::*;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

/// Backend-owned object or memory-view identity. No pointers cross the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub usize);
#[derive(Clone, Debug)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
    Undefined,
    Array(Rc<RefCell<Vec<Value>>>),
    Object(Rc<RefCell<BTreeMap<String, Value>>>),
    /// Test-owned typed array. CPU/memory views may instead use backend Handles.
    Buffer {
        bytes: Rc<RefCell<Vec<u8>>>,
        width: usize,
    },
    Handle(Handle),
    Function(usize),
    Native(String),
    Method(Box<Value>, String),
    Runner(Box<Value>, Box<Value>),
}
impl Value {
    pub fn number(&self) -> f64 {
        match self {
            Self::Number(n) => *n,
            _ => panic!("expected number, got {self:?}"),
        }
    }
    pub fn string(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Null => "null".into(),
            Self::Undefined => "undefined".into(),
            Self::Array(a) => a
                .borrow()
                .iter()
                .map(Value::string)
                .collect::<Vec<_>>()
                .join(","),
            _ => panic!("unsupported string conversion: {self:?}"),
        }
    }
    pub fn truthy(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Null | Self::Undefined => false,
            Self::Number(n) => *n != 0.0 && !n.is_nan(),
            Self::Text(s) => !s.is_empty(),
            _ => true,
        }
    }
    pub fn array(values: Vec<Value>) -> Self {
        Self::Array(Rc::new(RefCell::new(values)))
    }
    pub fn object(values: BTreeMap<String, Value>) -> Self {
        Self::Object(Rc::new(RefCell::new(values)))
    }
    pub fn buffer(bytes: Vec<u8>, width: usize) -> Self {
        assert!(width == 1 || width == 2);
        assert_eq!(bytes.len() % width, 0);
        Self::Buffer {
            bytes: Rc::new(RefCell::new(bytes)),
            width,
        }
    }
    pub fn values(&self) -> Vec<Value> {
        match self {
            Self::Array(v) => v.borrow().clone(),
            Self::Buffer { bytes, width } => bytes
                .borrow()
                .chunks(*width)
                .map(|b| {
                    Self::Number(if *width == 1 {
                        b[0] as f64
                    } else {
                        u16::from_le_bytes([b[0], b[1]]) as f64
                    })
                })
                .collect(),
            _ => panic!("expected test array/buffer, got {self:?}"),
        }
    }
}

/// Adapter boundary for the **native Rust** simulator, never a JS bridge.
///
/// `resolve` supplies imported configs/enums and free functions. `construct`
/// creates CPU/peripheral objects. Memory/register views must share state with
/// their CPU. Direct `set` on data is a raw write, unlike `writeData` method calls.
/// `properties` materializes configurations for spread overrides.
///
/// Methods use `&self` to allow synchronous reentry from callbacks. Release all
/// internal RefCell/lock guards before `runtime.invoke(callback, args)`; callbacks
/// can read/write the same CPU/peripheral and assert its state. A clock event
/// stores the callback Value, including its identity, until native tick dispatch.
/// Unknown operations must fail, not return fabricated defaults.
pub trait Backend {
    fn resolve(&self, name: &str) -> Value;
    fn construct(&self, runtime: &mut Runtime, kind: &str, args: Vec<Value>) -> Value;
    fn get(&self, object: Handle, key: &str) -> Value;
    fn set(&self, runtime: &mut Runtime, object: Handle, key: &str, value: Value);
    fn call(
        &self,
        runtime: &mut Runtime,
        receiver: Option<Handle>,
        name: &str,
        args: Vec<Value>,
    ) -> Value;
    fn properties(&self, object: Handle) -> BTreeMap<String, Value>;
}
struct Env {
    parent: Option<usize>,
    values: BTreeMap<String, Value>,
}
struct Function {
    params: Vec<Param>,
    body: Vec<Step>,
    env: usize,
    calls: Vec<Vec<Value>>,
    mocked: bool,
    original: Option<Value>,
}
#[derive(Debug)]
enum Flow {
    Continue,
    Return(Value),
}
pub struct Runtime {
    backend: Rc<dyn Backend>,
    envs: Vec<Env>,
    functions: Vec<Function>,
    source: String,
    name: String,
    line: usize,
    remaining: usize,
    /// Number of dynamically executed assertions (callback assertions included).
    pub assertions_executed: usize,
}
impl Runtime {
    pub fn new(backend: Rc<dyn Backend>) -> Self {
        Self {
            backend,
            envs: vec![Env {
                parent: None,
                values: BTreeMap::new(),
            }],
            functions: Vec::new(),
            source: "<harness>".into(),
            name: String::new(),
            line: 0,
            remaining: 1_000_000,
            assertions_executed: 0,
        }
    }
    /// A Runtime represents one case; create a fresh instance for every test.
    pub fn run(&mut self, case: &Case) {
        self.source = case.source.into();
        self.name = case.name.into();
        self.line = case.line;
        assert!(
            matches!(self.steps(&case.setup, 0), Flow::Continue),
            "setup returned unexpectedly"
        );
        let body_env = self.child_env(0);
        self.steps(&case.body, body_env);
    }
    fn child_env(&mut self, parent: usize) -> usize {
        let id = self.envs.len();
        self.envs.push(Env {
            parent: Some(parent),
            values: BTreeMap::new(),
        });
        id
    }
    fn budget(&mut self) {
        assert!(
            self.remaining > 0,
            "scenario budget exhausted at {}:{} ({})",
            self.source,
            self.line,
            self.name
        );
        self.remaining -= 1;
    }
    fn lookup(&self, mut env: usize, name: &str) -> Value {
        loop {
            if let Some(v) = self.envs[env].values.get(name) {
                return v.clone();
            }
            if let Some(parent) = self.envs[env].parent {
                env = parent;
            } else {
                break;
            }
        }
        match name {
            "vi" | "console" | "bytes" | "asmProgram" | "parseInt" => Value::Native(name.into()),
            _ => self.backend.resolve(name),
        }
    }
    fn write_var(&mut self, mut env: usize, name: &str, value: Value) {
        loop {
            if self.envs[env].values.contains_key(name) {
                self.envs[env].values.insert(name.into(), value);
                return;
            }
            env = self.envs[env]
                .parent
                .unwrap_or_else(|| panic!("assignment to undeclared variable {name}"));
        }
    }
    fn steps(&mut self, steps: &[Step], env: usize) -> Flow {
        for step in steps {
            self.line = step.line;
            self.budget();
            match &step.action {
                Action::Bind(binding, expr) => {
                    let value = self.eval(expr, env);
                    match binding {
                        Binding::Name(n) => {
                            self.envs[env].values.insert((*n).into(), value);
                        }
                        Binding::Object(fields) => {
                            for (key, name) in fields {
                                let v = self.get(&value, key);
                                self.envs[env].values.insert((*name).into(), v);
                            }
                        }
                    }
                }
                Action::Eval(e) => {
                    self.eval(e, env);
                }
                Action::Return(e) => return Flow::Return(self.eval(e, env)),
                Action::Throw(e) => {
                    let error = self.eval(e, env);
                    panic!("{}:{}: {}", self.source, self.line, error.string());
                }
                Action::If(e, a, b) => {
                    let yes = self.eval(e, env).truthy();
                    let branch_env = self.child_env(env);
                    if let Flow::Return(v) = self.steps(if yes { a } else { b }, branch_env) {
                        return Flow::Return(v);
                    }
                }
                Action::For(init, test, next, body) => {
                    let loop_env = self.child_env(env);
                    self.steps(init, loop_env);
                    while self.eval(test, loop_env).truthy() {
                        self.budget();
                        let body_env = self.child_env(loop_env);
                        if let Flow::Return(v) = self.steps(body, body_env) {
                            return Flow::Return(v);
                        }
                        self.eval(next, loop_env);
                    }
                }
                Action::ForOf(name, values, body) => {
                    let values = self.eval(values, env).values();
                    for v in values {
                        self.budget();
                        let loop_env = self.child_env(env);
                        self.envs[loop_env].values.insert((*name).into(), v);
                        if let Flow::Return(v) = self.steps(body, loop_env) {
                            return Flow::Return(v);
                        }
                    }
                }
                Action::Assert {
                    actual,
                    matcher,
                    negated,
                    expected,
                } => {
                    let actual = self.eval(actual, env);
                    let expected: Vec<_> = expected.iter().map(|e| self.eval(e, env)).collect();
                    self.assertions_executed += 1;
                    let matched = self.matches(&actual, matcher, &expected);
                    assert_ne!(
                        matched,
                        *negated,
                        "{}:{} ({}) {}{} failed: actual={actual:?}, expected={expected:?}",
                        self.source,
                        step.line,
                        self.name,
                        if *negated { "not." } else { "" },
                        matcher
                    );
                }
            }
        }
        Flow::Continue
    }
    fn eval(&mut self, expr: &Expr, env: usize) -> Value {
        self.budget();
        match expr {
            Expr::Number(n) => Value::Number(*n),
            Expr::Text(s) => Value::Text((*s).into()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Null => Value::Null,
            Expr::Undefined => Value::Undefined,
            Expr::Var(n) => self.lookup(env, n),
            Expr::Get(o, k) => {
                let o = self.eval(o, env);
                let k = self.eval(k, env).string();
                self.get(&o, &k)
            }
            Expr::Array(v) => Value::array(v.iter().map(|e| self.eval(e, env)).collect()),
            Expr::Object(fields) => {
                let mut values = BTreeMap::new();
                for field in fields {
                    match field {
                        Field::Entry(k, e) => {
                            values.insert((*k).into(), self.eval(e, env));
                        }
                        Field::Spread(e) => {
                            let v = self.eval(e, env);
                            values.extend(self.properties(&v));
                        }
                    }
                }
                Value::object(values)
            }
            Expr::Binary(op, a, b) => {
                let a = self.eval(a, env);
                let b = self.eval(b, env);
                binary_value(op, a, b)
            }
            Expr::Unary(op, e) => {
                let v = self.eval(e, env);
                match *op {
                    "-" => Value::Number(-v.number()),
                    "+" => Value::Number(v.number()),
                    "!" => Value::Bool(!v.truthy()),
                    "~" => Value::Number((!int32(v.number())) as f64),
                    _ => panic!("unknown unary operation {op}"),
                }
            }
            Expr::Assign(target, rhs, op) => {
                let target = self.target(target, env);
                // Evaluate the lvalue once and in source order, including for +=.
                let previous = if *op == "+=" {
                    Some(self.read_target(&target))
                } else {
                    None
                };
                let rhs = self.eval(rhs, env);
                let value = if let Some(old) = previous {
                    binary_value("+", old, rhs)
                } else {
                    assert_eq!(*op, "=");
                    rhs
                };
                self.set_target(target, value.clone());
                value
            }
            Expr::Update(e, delta, prefix) => {
                let target = self.target(e, env);
                let old = self.read_target(&target);
                let new = Value::Number(old.number() + *delta as f64);
                self.set_target(target, new.clone());
                if *prefix {
                    new
                } else {
                    old
                }
            }
            Expr::Call(f, args) => {
                let f = self.eval(f, env);
                let args = args.iter().map(|e| self.eval(e, env)).collect();
                self.invoke(f, args)
            }
            Expr::New(kind, args) => {
                let args: Vec<_> = args.iter().map(|e| self.eval(e, env)).collect();
                match *kind {
                    "Uint8Array" | "Uint16Array" => {
                        let width = if *kind == "Uint8Array" { 1 } else { 2 };
                        match args.first().expect("typed array requires argument") {
                            Value::Number(n) => {
                                assert!(*n >= 0.0 && n.fract() == 0.0);
                                Value::buffer(vec![0; *n as usize * width], width)
                            }
                            Value::Array(_) => {
                                let result =
                                    Value::buffer(vec![0; args[0].values().len() * width], width);
                                for (i, v) in args[0].values().into_iter().enumerate() {
                                    self.set(&result, &i.to_string(), v);
                                }
                                result
                            }
                            v => panic!("unsupported typed array constructor {v:?}"),
                        }
                    }
                    "TestProgramRunner" => Value::Runner(
                        Box::new(args[0].clone()),
                        Box::new(args.get(1).cloned().unwrap_or(Value::Undefined)),
                    ),
                    "Error" => Value::Text(args[0].string()),
                    _ => {
                        let backend = self.backend.clone();
                        backend.construct(self, kind, args)
                    }
                }
            }
            Expr::Function(params, body) => {
                self.function(params.clone(), body.clone(), env, false, None)
            }
            Expr::Template(parts) => {
                Value::Text(parts.iter().map(|e| self.eval(e, env).string()).collect())
            }
        }
    }
    fn function(
        &mut self,
        params: Vec<Param>,
        body: Vec<Step>,
        env: usize,
        mocked: bool,
        original: Option<Value>,
    ) -> Value {
        let id = self.functions.len();
        self.functions.push(Function {
            params,
            body,
            env,
            calls: Vec::new(),
            mocked,
            original,
        });
        Value::Function(id)
    }
    /// Native callbacks must enter here to execute captured state and record spies.
    pub fn invoke(&mut self, function: Value, args: Vec<Value>) -> Value {
        self.budget();
        match function {
            Value::Function(id) => {
                let f = &mut self.functions[id];
                if f.mocked {
                    f.calls.push(args.clone());
                }
                let (params, body, parent, original) =
                    (f.params.clone(), f.body.clone(), f.env, f.original.clone());
                if let Some(original) = original {
                    return self.invoke(original, args);
                }
                let env = self.child_env(parent);
                for (i, p) in params.iter().enumerate() {
                    let v = if p.rest {
                        Value::array(args[i.min(args.len())..].to_vec())
                    } else {
                        args.get(i).cloned().unwrap_or(Value::Undefined)
                    };
                    self.envs[env].values.insert(p.name.into(), v);
                }
                let line = self.line;
                let result = match self.steps(&body, env) {
                    Flow::Return(v) => v,
                    Flow::Continue => Value::Undefined,
                };
                self.line = line;
                result
            }
            Value::Native(name) => self.free_call(&name, args),
            Value::Method(receiver, name) => self.method(*receiver, &name, args),
            v => panic!("value is not callable: {v:?}"),
        }
    }
    pub fn get(&self, value: &Value, key: &str) -> Value {
        match value {
            Value::Object(fields) => fields
                .borrow()
                .get(key)
                .cloned()
                .unwrap_or(Value::Undefined),
            Value::Array(values) => {
                if key == "length" {
                    Value::Number(values.borrow().len() as f64)
                } else if let Ok(i) = key.parse::<usize>() {
                    values.borrow().get(i).cloned().unwrap_or(Value::Undefined)
                } else {
                    Value::Method(Box::new(value.clone()), key.into())
                }
            }
            Value::Buffer { bytes, width } => {
                if key == "length" {
                    Value::Number((bytes.borrow().len() / width) as f64)
                } else if let Ok(i) = key.parse::<usize>() {
                    let b = bytes.borrow();
                    let offset = i * width;
                    if offset + width > b.len() {
                        Value::Undefined
                    } else {
                        Value::Number(if *width == 1 {
                            b[offset] as f64
                        } else {
                            u16::from_le_bytes([b[offset], b[offset + 1]]) as f64
                        })
                    }
                } else {
                    Value::Method(Box::new(value.clone()), key.into())
                }
            }
            Value::Text(text) if key == "length" => Value::Number(text.len() as f64),
            Value::Handle(h) => self.backend.get(*h, key),
            Value::Native(_)
            | Value::Function(_)
            | Value::Runner(_, _)
            | Value::Number(_)
            | Value::Text(_) => Value::Method(Box::new(value.clone()), key.into()),
            v => panic!("cannot get {key} on {v:?}"),
        }
    }
    pub fn set(&mut self, value: &Value, key: &str, new: Value) {
        match value {
            Value::Object(fields) => {
                fields.borrow_mut().insert(key.into(), new);
            }
            Value::Array(values) => {
                let i: usize = key.parse().expect("array index");
                let mut values = values.borrow_mut();
                if values.len() <= i {
                    values.resize(i + 1, Value::Undefined);
                }
                values[i] = new;
            }
            Value::Buffer { bytes, width } => {
                let i: usize = key.parse().expect("buffer index");
                let offset = i * width;
                let mut bytes = bytes.borrow_mut();
                if offset + width <= bytes.len() {
                    let n = uint32(new.number());
                    bytes[offset] = n as u8;
                    if *width == 2 {
                        bytes[offset + 1] = (n >> 8) as u8;
                    }
                }
            }
            Value::Handle(h) => {
                let backend = self.backend.clone();
                backend.set(self, *h, key, new);
            }
            v => panic!("cannot set {key} on {v:?}"),
        }
    }
    fn properties(&self, v: &Value) -> BTreeMap<String, Value> {
        match v {
            Value::Object(fields) => fields.borrow().clone(),
            Value::Handle(h) => self.backend.properties(*h),
            _ => panic!("cannot spread {v:?}"),
        }
    }
    fn target(&mut self, e: &Expr, env: usize) -> Target {
        match e {
            Expr::Var(n) => Target::Variable(env, n),
            Expr::Get(o, k) => Target::Property(self.eval(o, env), self.eval(k, env).string()),
            _ => panic!("invalid assignment target {e:?}"),
        }
    }
    fn read_target(&self, t: &Target) -> Value {
        match t {
            Target::Variable(env, n) => self.lookup(*env, n),
            Target::Property(o, k) => self.get(o, k),
        }
    }
    fn set_target(&mut self, t: Target, v: Value) {
        match t {
            Target::Variable(env, n) => self.write_var(env, n, v),
            Target::Property(o, k) => self.set(&o, &k, v),
        }
    }
    fn free_call(&mut self, name: &str, args: Vec<Value>) -> Value {
        match name {
            "bytes" => {
                let hex = args[0].string();
                assert_eq!(hex.len() % 2, 0);
                Value::buffer(
                    (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex byte"))
                        .collect(),
                    1,
                )
            }
            "parseInt" => Value::Number(
                i64::from_str_radix(&args[0].string(), args[1].number() as u32).expect("integer")
                    as f64,
            ),
            "asmProgram" => {
                let backend = self.backend.clone();
                let result = backend.call(self, None, "assemble", args);
                let errors = self.get(&result, "errors").values();
                assert!(errors.is_empty(), "Assembly failed: {errors:?}");
                let bytes = self.get(&result, "bytes").values();
                assert_eq!(bytes.len() % 2, 0);
                let raw: Vec<u8> = bytes.iter().map(|v| v.number() as u8).collect();
                let lines = self.get(&result, "lines");
                let count = self.get(&lines, "length");
                let labels = self.get(&result, "labels");
                Value::object(BTreeMap::from([
                    ("program".into(), Value::buffer(raw, 2)),
                    ("lines".into(), lines),
                    ("instructionCount".into(), count),
                    ("labels".into(), labels),
                ]))
            }
            _ => {
                let backend = self.backend.clone();
                backend.call(self, None, name, args)
            }
        }
    }
    fn method(&mut self, receiver: Value, name: &str, args: Vec<Value>) -> Value {
        match &receiver {
            Value::Native(n) if n == "vi" => match name {
                "fn" => {
                    if let Some(Value::Function(id)) = args.first() {
                        self.functions[*id].mocked = true;
                        args[0].clone()
                    } else {
                        assert!(args.is_empty());
                        self.function(Vec::new(), Vec::new(), 0, true, None)
                    }
                }
                "spyOn" => {
                    let target = args[0].clone();
                    let key = args[1].string();
                    let original = self.get(&target, &key);
                    let spy = self.function(Vec::new(), Vec::new(), 0, true, Some(original));
                    self.set(&target, &key, spy.clone());
                    spy
                }
                _ => panic!("unknown mock operation {name}"),
            },
            Value::Native(n) if n == "console" && name == "log" => {
                eprintln!("{args:?}");
                Value::Undefined
            }
            Value::Function(id) => {
                let f = &mut self.functions[*id];
                assert!(f.mocked, "not a spy");
                match name {
                    "mockClear" => f.calls.clear(),
                    "mockReset" => {
                        f.calls.clear();
                        f.body.clear();
                        f.original = None;
                    }
                    _ => panic!("unknown spy method {name}"),
                }
                Value::Undefined
            }
            Value::Array(a) => match name {
                "push" => {
                    a.borrow_mut().extend(args);
                    Value::Number(a.borrow().len() as f64)
                }
                "join" => Value::Text(
                    a.borrow()
                        .iter()
                        .map(Value::string)
                        .collect::<Vec<_>>()
                        .join(&args[0].string()),
                ),
                "filter" => {
                    let values = a.borrow().clone();
                    let mut filtered = Vec::new();
                    for (i, v) in values.into_iter().enumerate() {
                        if self
                            .invoke(
                                args[0].clone(),
                                vec![v.clone(), Value::Number(i as f64), receiver.clone()],
                            )
                            .truthy()
                        {
                            filtered.push(v);
                        }
                    }
                    Value::array(filtered)
                }
                _ => panic!("unknown array method {name}"),
            },
            Value::Buffer { .. } if name == "set" => {
                let offset = args.get(1).map_or(0, |v| v.number() as usize);
                let values = args[0].values();
                let length = self.get(&receiver, "length").number() as usize;
                assert!(
                    offset + values.len() <= length,
                    "typed array set out of bounds"
                );
                for (i, v) in values.into_iter().enumerate() {
                    self.set(&receiver, &(offset + i).to_string(), v);
                }
                Value::Undefined
            }
            Value::Number(n) if name == "toString" => {
                let radix = args.first().map_or(10, |v| v.number() as u32);
                Value::Text(match radix {
                    10 => n.to_string(),
                    16 => format!("{:x}", *n as i64),
                    _ => panic!("unsupported radix {radix}"),
                })
            }
            Value::Text(s) if name == "substr" => {
                let start = args[0].number() as usize;
                let len = args[1].number() as usize;
                Value::Text(s[start..(start + len).min(s.len())].into())
            }
            Value::Runner(cpu, on_break) => self.run_program(cpu, on_break, name, args),
            Value::Handle(h) => {
                let backend = self.backend.clone();
                backend.call(self, Some(*h), name, args)
            }
            _ => panic!("unknown method {name} on {receiver:?}"),
        }
    }
    fn run_program(
        &mut self,
        cpu: &Value,
        on_break: &Value,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        let count = if name == "runInstructions" {
            args[0].number() as usize
        } else {
            args.get(1).map_or(5000, |v| v.number() as usize)
        };
        assert!(["runInstructions", "runUntil", "runToBreak", "runToAddress"].contains(&name));
        for _ in 0..count {
            self.budget();
            let pc = self.get(cpu, "pc");
            let prog = self.get(cpu, "progMem");
            let opcode = self.get(&prog, &pc.string());
            let is_break = deep_equal(&opcode, &Value::Number(0x9598 as f64));
            // Preserve upstream's BREAK-before-predicate ordering.
            if is_break {
                if matches!(on_break, Value::Undefined) {
                    panic!("BREAK instruction encountered");
                } else {
                    self.invoke(on_break.clone(), vec![cpu.clone()]);
                }
            }
            let done = match name {
                "runInstructions" => false,
                "runToBreak" => is_break,
                "runToAddress" => self.get(cpu, "pc").number() * 2.0 == args[0].number(),
                "runUntil" => self.invoke(args[0].clone(), vec![cpu.clone()]).truthy(),
                _ => unreachable!(),
            };
            if done {
                return Value::Undefined;
            }
            let backend = self.backend.clone();
            backend.call(self, None, "avrInstruction", vec![cpu.clone()]);
            self.invoke(
                Value::Method(Box::new(cpu.clone()), "tick".into()),
                Vec::new(),
            );
        }
        assert_eq!(
            name, "runInstructions",
            "Test program ran for too long, check your predicate"
        );
        Value::Undefined
    }
    fn matches(&self, actual: &Value, matcher: &str, expected: &[Value]) -> bool {
        match matcher {
            "toEqual" => deep_equal(actual, &expected[0]),
            "toBe" => same_value(actual, &expected[0]),
            "toBeGreaterThanOrEqual" => actual.number() >= expected[0].number(),
            "toHaveBeenCalled" | "toHaveBeenCalledTimes" | "toHaveBeenCalledWith" => {
                let Value::Function(id) = actual else {
                    panic!("not a mock: {actual:?}");
                };
                let f = &self.functions[*id];
                assert!(f.mocked, "not a mock");
                match matcher {
                    "toHaveBeenCalled" => !f.calls.is_empty(),
                    "toHaveBeenCalledTimes" => f.calls.len() as f64 == expected[0].number(),
                    _ => f.calls.iter().any(|args| {
                        args.len() == expected.len()
                            && args.iter().zip(expected).all(|(a, b)| deep_equal(a, b))
                    }),
                }
            }
            _ => panic!("unknown matcher {matcher}"),
        }
    }
}
enum Target {
    Variable(usize, &'static str),
    Property(Value, String),
}
fn uint32(n: f64) -> u32 {
    if !n.is_finite() {
        0
    } else {
        n.trunc().rem_euclid(4294967296.0) as u32
    }
}
fn int32(n: f64) -> i32 {
    uint32(n) as i32
}
fn binary_value(op: &str, a: Value, b: Value) -> Value {
    match op {
        "+" if matches!(a, Value::Text(_)) || matches!(b, Value::Text(_)) => {
            Value::Text(a.string() + &b.string())
        }
        "+" => Value::Number(a.number() + b.number()),
        "-" => Value::Number(a.number() - b.number()),
        "*" => Value::Number(a.number() * b.number()),
        "/" => Value::Number(a.number() / b.number()),
        "|" => Value::Number((int32(a.number()) | int32(b.number())) as f64),
        "&" => Value::Number((int32(a.number()) & int32(b.number())) as f64),
        "<<" => Value::Number(int32(a.number()).wrapping_shl(uint32(b.number()) & 31) as f64),
        "<" => Value::Bool(a.number() < b.number()),
        "==" | "===" => Value::Bool(same_value(&a, &b)), // source == compares only same-typed opcode strings
        _ => panic!("unknown binary operation {op}"),
    }
}
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) | (Value::Undefined, Value::Undefined) => true,
        (Value::Handle(a), Value::Handle(b)) => a == b,
        (Value::Function(a), Value::Function(b)) => a == b,
        (Value::Native(a), Value::Native(b)) => a == b,
        (Value::Method(a, an), Value::Method(b, bn)) => an == bn && same_value(a, b),
        (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (
            Value::Buffer {
                bytes: a,
                width: aw,
            },
            Value::Buffer {
                bytes: b,
                width: bw,
            },
        ) => aw == bw && Rc::ptr_eq(a, b),
        _ => false,
    }
}
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Array(a), Value::Array(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| deep_equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|b| deep_equal(v, b)))
        }
        (
            Value::Buffer {
                bytes: a,
                width: aw,
            },
            Value::Buffer {
                bytes: b,
                width: bw,
            },
        ) => aw == bw && *a.borrow() == *b.borrow(),
        _ => same_value(a, b),
    }
}
