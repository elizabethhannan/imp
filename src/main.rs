#![feature(proc_macro)]
#![feature(conservative_impl_trait)]
#![feature(slice_patterns)]

extern crate websocket;
#[macro_use(json, json_internal)]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate nom;

use std::thread;
use std::sync::{Arc, Mutex};

use websocket::OwnedMessage;
use websocket::sync::Server;

use std::fs::File;
use std::io::prelude::*;

use std::collections::{HashMap, BTreeMap};
use std::iter::Iterator;

use std::borrow::Cow;
use std::borrow::Borrow;

use nom::*;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Clone)]
struct Entity {
    avs: Vec<(Attribute, Value)>,
}

type Attribute = String;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Clone)]
enum Value {
    Boolean(bool),
    Integer(i64),
    String(String),
    Entity(Entity),
}

impl From<bool> for Value {
    fn from(bool: bool) -> Value {
        Value::Boolean(bool)
    }
}

impl From<i64> for Value {
    fn from(integer: i64) -> Value {
        Value::Integer(integer)
    }
}

impl From<String> for Value {
    fn from(string: String) -> Value {
        Value::String(string)
    }
}

impl<'a> From<&'a str> for Value {
    fn from(string: &'a str) -> Value {
        Value::String(string.to_owned())
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            &Value::Boolean(bool) => bool.fmt(f),
            &Value::Integer(integer) => integer.fmt(f),
            &Value::String(ref string) => write!(f, "{:?}", string),
            &Value::Entity(ref entity) => {
                f.write_str("{")?;
                let mut avs = entity.avs.iter().peekable();
                while let Some(&(ref a, ref v)) = avs.next() {
                    write!(f, "{}={}", a, v)?;
                    if avs.peek().is_some() {
                        write!(f, ", ")?;
                    }
                }
                f.write_str("}")
            }
        }
    }
}

impl Value {
    fn as_str(&self) -> Option<&str> {
        match self {
            &Value::String(ref this) => Some(this),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            &Value::Integer(this) => Some(this),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            &Value::Boolean(this) => Some(this),
            _ => None,
        }
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        match self {
            &Value::Boolean(ref this) if this == other => true,
            _ => false,
        }
    }
}

impl PartialEq<i64> for Value {
    fn eq(&self, other: &i64) -> bool {
        match self {
            &Value::Integer(ref this) if this == other => true,
            _ => false,
        }
    }
}

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        match self {
            &Value::String(ref this) if this == other => true,
            _ => false,
        }
    }
}

impl PartialEq<Entity> for Value {
    fn eq(&self, other: &Entity) -> bool {
        match self {
            &Value::Entity(ref this) if this == other => true,
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
struct Bag {
    eavs: BTreeMap<(Entity, Attribute), Value>,
}

impl Bag {
    fn new() -> Self {
        Bag { eavs: BTreeMap::new() }
    }

    fn create<A>(&mut self, avs: Vec<(A, Value)>) -> Entity
    where
        A: Into<Attribute>,
    {
        let avs = avs.into_iter()
            .map(|(a, v)| (a.into(), v))
            .collect::<Vec<_>>();
        let entity = Entity { avs: avs.clone() };
        for (attribute, value) in avs {
            self.insert(entity.clone(), attribute, value);
        }
        entity
    }

    fn insert<A, V>(&mut self, entity: Entity, attribute: A, value: V)
    where
        A: Into<Attribute>,
        V: Into<Value>,
    {
        self.eavs.insert((entity, attribute.into()), value.into());
    }
}

fn load() -> Bag {
    let mut file = File::open("/home/jamie/imp.db").unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let eavs: Vec<((Entity, Attribute), Value)> = serde_json::from_str(&*contents).unwrap();
    Bag { eavs: eavs.into_iter().collect() }
}

fn save(bag: &Bag) {
    let mut file = File::create("/home/jamie/imp.db").unwrap();
    write!(
        file,
        "{}",
        json!(bag.eavs.clone().into_iter().collect::<Vec<_>>())
    ).unwrap();
}

// f should be |t| t < value or |t| t <= value
fn gallop<'a, T, F: Fn(&T) -> bool>(slice: &'a [T], mut lo: usize, hi: usize, f: F) -> usize {
    if lo < hi && f(&slice[lo]) {
        let mut step = 1;
        while lo + step < hi && f(&slice[lo + step]) {
            lo = lo + step;
            step = step << 1;
        }

        step = step >> 1;
        while step > 0 {
            if lo + step < hi && f(&slice[lo + step]) {
                lo = lo + step;
            }
            step = step >> 1;
        }

        lo += 1
    }
    lo
}

#[derive(Debug, Clone)]
enum Function {
    Add(usize, usize),
}

impl Function {
    fn apply(&self, variables: &mut [Cow<Value>]) -> Result<Value, String> {
        match self {
            &Function::Add(a, b) => {
                match (variables[a].borrow(), variables[b].borrow()) {
                    (&Value::Integer(a), &Value::Integer(b)) => Ok(Value::Integer(a + b)),
                    (a, b) => Err(format!("Type error: {} + {}", a, b)),
                }
            }
        }
    }
}

type LoHi = (usize, usize);
type RowCol = (usize, usize);

#[derive(Debug, Clone)]
enum Constraint {
    Narrow(RowCol, usize),
    Join(Vec<RowCol>, usize),
    Apply(usize, bool, Function),
    Assert([usize; 3]),
    Debug(Vec<(String, usize)>),
}

fn constrain<'a>(
    constraints: &[Constraint],
    indexes: &'a [[Vec<Value>; 3]],
    ranges: &mut [LoHi],
    variables: &mut [Cow<'a, Value>],
    results: &mut Vec<Value>,
    asserts: &mut Vec<[Value; 3]>,
) -> Result<(), String> {
    if constraints.len() > 0 {
        match &constraints[0] {
            &Constraint::Narrow((row_ix, col_ix), var_ix) => {
                let (old_lo, old_hi) = ranges[row_ix];
                let column = &indexes[row_ix][col_ix];
                let (lo, hi) = {
                    let value = variables[var_ix].borrow();
                    let lo = gallop(column, old_lo, old_hi, |v| v < value);
                    let hi = gallop(column, lo, old_hi, |v| v <= value);
                    (lo, hi)
                };
                if lo < hi {
                    ranges[row_ix] = (lo, hi);
                    variables[var_ix] = Cow::Borrowed(&column[lo]);
                    constrain(
                        &constraints[1..],
                        indexes,
                        ranges,
                        variables,
                        results,
                        asserts,
                    )?;
                    ranges[row_ix] = (old_lo, old_hi);
                }
            }
            &Constraint::Join(ref rowcols, var_ix) => {
                let mut buffer = vec![(0, 0); rowcols.len()]; // TODO pre-allocate
                let (row_ix, col_ix) = rowcols[0]; // TODO pick smallest
                let column = &indexes[row_ix][col_ix];
                let (old_lo, old_hi) = ranges[row_ix];
                let mut lo = old_lo;
                // loop over rowcols[0]
                while lo < old_hi {
                    let value = &column[lo];
                    let hi = gallop(column, lo + 1, old_hi, |v| v <= value);
                    ranges[row_ix] = (lo, hi);
                    {
                        // loop over rowcols[1..]
                        let mut i = 1;
                        while i < rowcols.len() {
                            let column = &indexes[row_ix][col_ix];
                            let (old_lo, old_hi) = ranges[row_ix];
                            let lo = gallop(column, old_lo, old_hi, |v| v < value);
                            let hi = gallop(column, lo, old_hi, |v| v <= value);
                            if lo < hi {
                                ranges[row_ix] = (lo, hi);
                                buffer[i] = (old_lo, old_hi);
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        // if all succeeded, continue with rest of constraints
                        if i == rowcols.len() {
                            variables[var_ix] = Cow::Borrowed(&column[lo]);
                            constrain(
                                &constraints[1..],
                                indexes,
                                ranges,
                                variables,
                                results,
                                asserts,
                            )?;
                        }
                        // restore state for rowcols[1..i]
                        while i > 1 {
                            i -= 1;
                            let (row_ix, _) = rowcols[i];
                            ranges[row_ix] = buffer[i];
                        }
                    }
                    lo = hi;
                }
                // restore state for rowcols[0]
                ranges[row_ix] = (old_lo, old_hi);
            }
            &Constraint::Apply(result_ix, result_already_fixed, ref function) => {
                let result = Cow::Owned(function.apply(variables)?);
                if result_already_fixed {
                    if variables[result_ix] == result {
                        constrain(
                            &constraints[1..],
                            indexes,
                            ranges,
                            variables,
                            results,
                            asserts,
                        )?;
                    } else {
                        // failed, backtrack
                    }
                } else {
                    variables[result_ix] = result;
                    constrain(
                        &constraints[1..],
                        indexes,
                        ranges,
                        variables,
                        results,
                        asserts,
                    )?;
                }
            }
            &Constraint::Assert(var_ixes) => {
                {
                    let v0: &Value = variables[var_ixes[0]].borrow();
                    let v1: &Value = variables[var_ixes[1]].borrow();
                    let v2: &Value = variables[var_ixes[2]].borrow();
                    asserts.push([v0.to_owned(), v1.to_owned(), v2.to_owned()]);
                }
                constrain(
                    &constraints[1..],
                    indexes,
                    ranges,
                    variables,
                    results,
                    asserts,
                )?;
            }
            &Constraint::Debug(ref named_variables) => {
                for &(_, var_ix) in named_variables.iter() {
                    results.push(variables[var_ix].clone().into_owned());
                }
                constrain(
                    &constraints[1..],
                    indexes,
                    ranges,
                    variables,
                    results,
                    asserts,
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Block {
    row_orderings: Vec<[usize; 3]>,
    variables: Vec<Value>,
    constraints: Vec<Constraint>,
}

impl Block {
    fn run(&self, bag: &Bag) -> Result<(Vec<Value>, Vec<[Value; 3]>), String> {
        // TODO strip out this compat layer
        let eavs: Vec<[Value; 3]> = bag.eavs
            .iter()
            .map(|(&(ref e, ref a), v)| {
                [
                    Value::Entity(e.clone()),
                    Value::String(a.clone()),
                    v.clone(),
                ]
            })
            .collect();
        let indexes: Vec<[Vec<Value>; 3]> = self.row_orderings
            .iter()
            .map(|row_ordering| {
                let mut ordered_eavs: Vec<[Value; 3]> = eavs.iter()
                    .map(|eav| {
                        [
                            eav[row_ordering[0]].clone(),
                            eav[row_ordering[1]].clone(),
                            eav[row_ordering[2]].clone(),
                        ]
                    })
                    .collect();
                ordered_eavs.sort_unstable();
                let mut reverse_ordering = [0, 0, 0];
                for i in 0..3 {
                    reverse_ordering[row_ordering[i]] = i;
                }
                [
                    ordered_eavs
                        .iter()
                        .map(|eav| eav[reverse_ordering[0]].clone())
                        .collect(),
                    ordered_eavs
                        .iter()
                        .map(|eav| eav[reverse_ordering[1]].clone())
                        .collect(),
                    ordered_eavs
                        .iter()
                        .map(|eav| eav[reverse_ordering[2]].clone())
                        .collect(),
                ]
            })
            .collect();
        let mut variables: Vec<Cow<Value>> =
            self.variables.iter().map(|v| Cow::Borrowed(v)).collect();
        let mut ranges: Vec<LoHi> = indexes.iter().map(|index| (0, index[0].len())).collect();
        let mut results = vec![];
        let mut asserts = vec![];
        constrain(
            &*self.constraints,
            &*indexes,
            &mut *ranges,
            &mut *variables,
            &mut results,
            &mut asserts,
        )?;
        Ok((results, asserts))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprIr {
    Constant(Value),
    Variable(String),
    Function(FunctionIr),
    Dot(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionIr {
    name: String,
    args: Vec<usize>,
}

impl ExprIr {
    fn is_constant(&self) -> bool {
        match self {
            &ExprIr::Constant(_) => true,
            _ => false,
        }
    }

    fn is_variable(&self) -> bool {
        match self {
            &ExprIr::Variable(_) => true,
            _ => false,
        }
    }

    fn is_function(&self) -> bool {
        match self {
            &ExprIr::Function(_) => true,
            _ => false,
        }
    }
}

fn translate(expr: &ExprAst, exprs: &mut Vec<ExprIr>) -> usize {
    let expr = match expr {
        &ExprAst::Constant(ref value) => ExprIr::Constant(value.clone()),
        &ExprAst::Variable(ref variable) => ExprIr::Variable(variable.clone()),
        &ExprAst::Function(FunctionAst { ref name, ref args }) => ExprIr::Function(FunctionIr {
            name: name.clone(),
            args: args.iter().map(|e| translate(e, exprs)).collect(),
        }),
        &ExprAst::Dot(ref expr1, ref expr2) => {
            ExprIr::Dot(translate(expr1, exprs), translate(expr2, exprs))
        }
    };
    exprs.push(expr);
    exprs.len() - 1
}

fn compile(block: &BlockAst) -> Result<Block, String> {

    // label exprs in pre-order and flatten tree
    let mut expr_irs: Vec<ExprIr> = vec![];
    let mut pattern_exprs: Vec<[usize; 2]> = vec![];
    let mut assert_exprs: Vec<[usize; 3]> = vec![];
    for statement_or_error in block.statements.iter() {
        match statement_or_error {
            &Ok(ref statement) => {
                match statement {
                    &StatementAst::Pattern([ref e1, ref e2]) => {
                        pattern_exprs.push(
                            [
                                translate(e1, &mut expr_irs),
                                translate(e2, &mut expr_irs),
                            ],
                        );
                    }
                    &StatementAst::Assert([ref e, ref a, ref v]) => {
                        assert_exprs.push(
                            [
                                translate(e, &mut expr_irs),
                                translate(a, &mut expr_irs),
                                translate(v, &mut expr_irs),
                            ],
                        );
                    }
                }
            }
            &Err(_) => (),
        }
    }

    // group exprs that must be equal
    let mut expr_group: Vec<usize> = (0..expr_irs.len()).collect();

    // all variables with same name must be equal
    let mut variable_group: HashMap<&str, usize> = HashMap::new();
    for (expr, ir) in expr_irs.iter().enumerate() {
        match ir {
            &ExprIr::Variable(ref variable) => {
                match variable_group.get(&**variable) {
                    Some(&group) => expr_group[expr] = group,
                    None => {
                        variable_group.insert(variable, expr);
                    }
                }
            }
            _ => (),
        }
    }

    // both exprs in a top-level pattern (eg a = 2) must be equal
    for &[expr1, expr2] in pattern_exprs.iter() {
        let group1 = expr_group[expr1];
        let group2 = expr_group[expr2];
        for group in expr_group.iter_mut() {
            if *group == group2 {
                *group = group1;
            }
        }
    }

    // gather up groups
    let mut group_exprs: HashMap<usize, Vec<usize>> = HashMap::new();
    for (expr, group) in expr_group.iter().enumerate() {
        group_exprs.entry(*group).or_insert_with(|| vec![]).push(
            expr,
        );
    }

    // sort by order of appearance in code
    let mut slot_exprs: Vec<Vec<usize>> = group_exprs
        .iter()
        .map(|(_, exprs)| {
            let mut exprs = exprs.clone();
            exprs.sort_unstable();
            exprs
        })
        .collect();
    slot_exprs.sort_unstable_by_key(|exprs| exprs[0]);

    // move slots that contain constants and no functions to the start
    for slot in 0..slot_exprs.len() {
        if slot_exprs[slot].iter().any(
            |&expr| expr_irs[expr].is_constant(),
        ) &&
            slot_exprs[slot].iter().all(
                |&expr| !expr_irs[expr].is_function(),
            )
        {
            let exprs = slot_exprs.remove(slot);
            slot_exprs.insert(0, exprs);
        }
    }

    // index in the other direction
    let mut expr_slot: Vec<usize> = (0..expr_irs.len()).collect();
    for (slot, exprs) in slot_exprs.iter().enumerate() {
        for expr in exprs.iter() {
            expr_slot[*expr] = slot;
        }
    }

    // collect exprs that directly query the database
    let mut row_exprs: Vec<[usize; 3]> = vec![];
    for (v, ir) in expr_irs.iter().enumerate() {
        match ir {
            &ExprIr::Dot(e, a) => row_exprs.push([e, a, v]),
            _ => (),
        }
    }

    // choose row indexes with columns in the order they appear in the slots
    let row_orderings: Vec<[usize; 3]> = row_exprs
        .iter()
        .map(|exprs| {
            let mut ordering = [0, 1, 2];
            ordering.sort_unstable_by_key(|&ix| expr_slot[exprs[ix]]);
            ordering
        })
        .collect();

    // produce constraints
    let mut values: Vec<Value> = (0..slot_exprs.len())
        .map(|_| Value::Boolean(false))
        .collect();
    let mut constraints: Vec<Constraint> = vec![];
    for (slot, exprs) in slot_exprs.iter().enumerate() {
        // gather up everything that constrains this slot
        let constants: Vec<&ExprIr> = exprs
            .iter()
            .map(|&expr| &expr_irs[expr])
            .filter(|ir| ir.is_constant())
            .collect();
        let functions: Vec<&ExprIr> = exprs
            .iter()
            .map(|&expr| &expr_irs[expr])
            .filter(|ir| ir.is_function())
            .collect();
        let mut rowcols: Vec<(usize, usize)> = vec![];
        for expr in exprs.iter() {
            for (row, cols) in row_exprs.iter().enumerate() {
                for (col, rc_expr) in cols.iter().enumerate() {
                    if expr == rc_expr {
                        rowcols.push((row, col));
                    }
                }
            }
        }

        // check constants are sane
        for i in 1..constants.len() {
            if constants[i] != constants[0] {
                return Err(format!(
                    "Impossible constraint: {:?} = {:?}",
                    constants[i],
                    constants[0]
                ));
            }
        }

        // check that something constrains this slot
        if (constants.len() == 0) && (functions.len() == 0) && (rowcols.len() == 0) {
            return Err(format!("No constraints on slot {}", slot)); // TODO how to identify?
        }

        // after first constant or function, the rest just have to check their result is equal
        let mut slot_fixed_yet = false;

        // constants just get stuck in the values vec
        if constants.len() > 0 {
            values[slot] = match constants[0] {
                &ExprIr::Constant(ref value) => value.clone(),
                _ => unreachable!(),
            };
            slot_fixed_yet = true;
        }

        // functions get run next
        for function in functions.iter() {
            match function {
                &&ExprIr::Function(FunctionIr { ref name, ref args }) => {
                    let function = match (&**name, &**args) {
                        ("+", &[a, b]) => Function::Add(expr_slot[a], expr_slot[b]),
                        _ => {
                            return Err(format!(
                                "I don't know any function called {:?} with {} arguments",
                                name,
                                args.len()
                            ))
                        }
                    };
                    constraints.push(Constraint::Apply(slot, slot_fixed_yet, function));
                    slot_fixed_yet = true;
                }
                _ => unreachable!(),
            }
        }

        // if there is already a value just look it up in indexes
        // otherwise join indexes
        if slot_fixed_yet {
            for &(row, col) in rowcols.iter() {
                constraints.push(Constraint::Narrow((row, col), slot));
            }
        } else {
            constraints.push(Constraint::Join(rowcols, slot));
        }
    }

    // asserts are constraints too
    // TODO constraints is the wrong name for anything that includes asserts
    for exprs in assert_exprs.iter() {
        constraints.push(Constraint::Assert(
            [
                expr_slot[exprs[0]],
                expr_slot[exprs[1]],
                expr_slot[exprs[2]],
            ],
        ));
    }

    let mut named_variables: Vec<(String, usize)> = vec![];
    for (slot, exprs) in slot_exprs.iter().enumerate() {
        if let Some(&ExprIr::Variable(ref name)) =
            exprs
                .iter()
                .map(|&expr| &expr_irs[expr])
                .filter(|ir| ir.is_variable())
                .next()
        {
            named_variables.push((name.clone(), slot));
        }
    }
    constraints.push(Constraint::Debug(named_variables));

    // println!(
    //     "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
    //     expr_irs,
    //     pattern_exprs,
    //     assert_exprs,
    //     expr_group,
    //     group_exprs,
    //     slot_exprs,
    //     expr_slot,
    //     row_exprs,
    //     row_orderings,
    //     values,
    //     constraints,
    // );

    Ok(Block {
        row_orderings,
        variables: values,
        constraints,
    })
}

#[derive(Debug, Clone)]
struct CodeAst {
    blocks: Vec<BlockAst>,
    focused: Option<usize>,
}

#[derive(Debug, Clone)]
struct BlockAst {
    statements: Vec<Result<StatementAst, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprAst {
    Constant(Value),
    Variable(String),
    Function(FunctionAst),
    Dot(Box<ExprAst>, Box<ExprAst>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionAst {
    name: String,
    args: Vec<ExprAst>,
}

#[derive(Debug, Clone)]
enum StatementAst {
    Pattern([ExprAst; 2]),
    Assert([ExprAst; 3]),
}

fn simplify_errors<Output>(result: IResult<&[u8], Output>) -> Result<Output, String> {
    match result {
        IResult::Done(remaining, output) => {
            if remaining.len() <= 1 {
                // because of the hacky \n on the end
                Ok(output)
            } else {
                Err(format!(
                    "Remaining: {}",
                    std::str::from_utf8(remaining).unwrap()
                ))
            }
        }
        IResult::Error(error) => Err(format!("Nom error: {:?}", error)),
        IResult::Incomplete(needed) => Err(format!("Nom incomplete: {:?}", needed)),
    }
}

fn code_ast(text: &str, cursor: i64) -> CodeAst {
    let blocks = text.split("\n\n")
        .map(|block| block_ast(block))
        .collect::<Vec<_>>();
    let mut focused = None;
    let mut remaining_cursor = cursor;
    for (i, block_src) in text.split("\n\n").enumerate() {
        remaining_cursor -= block_src.len() as i64;
        if remaining_cursor <= 0 {
            focused = Some(i);
            break;
        }
        remaining_cursor -= 2; // \n\n
        if remaining_cursor < 0 {
            break;
        }
    }
    CodeAst { blocks, focused }
}

fn block_ast(text: &str) -> BlockAst {
    BlockAst {
        statements: text.split("\n")
            .map(|statement| {
                let statement = format!("{}\n", statement); // hack to get nom to stop streaming
                simplify_errors(statement_ast(statement.as_bytes()))
            })
            .collect::<Vec<_>>(),
    }
}

named!(statement_ast(&[u8]) -> StatementAst, alt!(assert_ast | pattern_ast));

named!(assert_ast(&[u8]) -> StatementAst, do_parse!(
    tag!("+") >>
    space >>
    e: simple_expr_ast >>
    tag!(".") >>
    a: symbol_ast >>
    opt!(space) >>
    tag!("=") >>
    opt!(space) >>
    v: expr_ast >>
    (StatementAst::Assert([e, ExprAst::Constant(Value::String(a)), v]))
));

named!(pattern_ast(&[u8]) -> StatementAst, do_parse!(
    e1: expr_ast >>
    opt!(space) >>
    tag!("=") >>
    opt!(space) >>
    e2: expr_ast >>
    (StatementAst::Pattern([e1, e2]))
));

named!(expr_ast(&[u8]) -> ExprAst, do_parse!(
    e: simple_expr_ast >>
        ts: many0!(dot_ast) >>
        f: opt!(infix_function_ast) >>
        ({
            let mut e = e;
            for t in ts {
                e = ExprAst::Dot(Box::new(e), Box::new(ExprAst::Constant(Value::String(t))));
            }
            if let Some((name, arg)) = f {
                e = ExprAst::Function(FunctionAst{name, args: vec![e, arg]});
            }
            e
        })
));

named!(simple_expr_ast(&[u8]) -> ExprAst, alt!(
        map!(prefix_function_ast, ExprAst::Function) |
        map!(value_ast, ExprAst::Constant) |
        map!(symbol_ast, ExprAst::Variable) |
        paren_ast
));

named!(dot_ast(&[u8]) -> String, do_parse!(
    tag!(".") >>
    a: symbol_ast >>
        (a)
        ));

named!(prefix_function_ast(&[u8]) -> FunctionAst, do_parse!(
        name: symbol_ast >>
        tag!("(") >>
        args: separated_list_complete!(tuple!(tag!(","), opt!(space)), expr_ast) >>
        tag!(")") >>
        (FunctionAst{name, args})
    ));

named!(infix_function_ast(&[u8]) -> (String, ExprAst), do_parse!(
        opt!(space) >>
        name: map_res!(
            alt!(tag!("+")),
            |b| std::str::from_utf8(b).map(|s| s.to_owned())
        ) >>
        opt!(space) >>
        arg: expr_ast >>
        (name, arg)
));

named!(symbol_ast(&[u8]) -> String, map_res!(
    verify!(
        take_while1_s!(|c| is_alphanumeric(c) || c == ('-' as u8)),
        |b: &[u8]| is_alphabetic(b[0])
    ),
    |b| std::str::from_utf8(b).map(|s| s.to_owned())
));

named!(value_ast(&[u8]) -> Value, alt!(
    map!(integer_ast, Value::Integer) |
    map!(boolean_ast, Value::Boolean) |
    map!(string_ast, Value::String)
));

named!(integer_ast(&[u8]) -> i64, map_res!(
        digit,
        |b| std::str::from_utf8(b).unwrap().parse::<i64>()
    ));

named!(boolean_ast(&[u8]) -> bool, do_parse!(
        b: alt!(tag!("true") | tag!("false")) >>
        (b == b"true")
    ));

// TODO escaping
named!(string_ast(&[u8]) -> String, map_res!(
        delimited!(char!('"'), take_until!("\""), char!('"')),
        |b| std::str::from_utf8(b).map(|s| s.to_owned())
    ));

named!(paren_ast(&[u8]) -> ExprAst, do_parse!(
        tag!("(") >>
        opt!(space) >>
        e: expr_ast >>
        opt!(space) >>
        tag!(")") >>
        (e)
    ));

fn run_code(bag: &Bag, code: &str, cursor: i64) {
    let code_ast = code_ast(code, cursor);
    let mut status: Vec<Result<(Block, Vec<Value>, Vec<[Value; 3]>), String>> = vec![];
    for block in code_ast.blocks.iter() {
        // print!("{:?}\n\n", block);
        if let Some(&Err(ref error)) = block.statements.iter().find(|s| s.is_err()) {
            status.push(Err(format!("Parse error: {}", error)));
        } else {
            match compile(block) {
                Err(error) => status.push(Err(format!("Compile error: {}", error))),
                Ok(block) => {
                    // print!("{:?}\n\n", block);
                    match block.run(&bag) {
                        Err(error) => status.push(Err(format!("Run error: {}", error))),
                        Ok((results, asserts)) => status.push(Ok((block, results, asserts))),
                    }
                }
            }
        }
    }

    if let Some(ix) = code_ast.focused {
        match &status[ix] {
            &Err(ref error) => print!("{}\n\n", error),
            &Ok((ref block, ref results, ref asserts)) => {
                print!(
                    "Ok: {} results, {} asserts\n\n",
                    results.len(),
                    asserts.len()
                );
                if let Some(&Constraint::Debug(ref named_variables)) = block.constraints.last() {
                    if named_variables.len() > 0 {
                        for row in results.chunks(named_variables.len()) {
                            for (&(ref name, _), value) in named_variables.iter().zip(row.iter()) {
                                print!("{}={}\t", name, value);
                            }
                            print!("\n");
                        }
                        print!("\n");
                    }
                }

                for assert in asserts.iter() {
                    print!(
                        "+ {}.{} = {}\n",
                        assert[0],
                        assert[1].as_str().unwrap(),
                        assert[2]
                    );
                }
                print!("\n");
            }
        }

    } else {
        print!("Nothing focused\n\n");
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum EditorEvent {
    State(String, i64),
}

// #[derive(Debug, Serialize, Deserialize)]
// enum Command {
//     Render(String),
// }

// fn send_command(sender: &mut websocket::sender::Writer<std::net::TcpStream>, c: Command) {
//     sender
//         .send_message(&OwnedMessage::Text(json!(c).to_string()))
//         .unwrap()
// }

fn serve_editor() {
    let bag = Arc::new(Mutex::new(load()));

    let server = Server::bind("127.0.0.1:8081").unwrap();

    for request in server.filter_map(Result::ok) {
        let bag = bag.clone();
        thread::spawn(move || {
            let client = request.accept().unwrap();
            let ip = client.peer_addr().unwrap();
            println!("Connection from {}", ip);

            let (mut receiver, mut sender) = client.split().unwrap();

            for message in receiver.incoming_messages() {
                let message = message.unwrap();

                match message {
                    OwnedMessage::Close(_) => {
                        let message = OwnedMessage::Close(None);
                        sender.send_message(&message).unwrap();
                        println!("Client {} disconnected", ip);
                        return;
                    }
                    OwnedMessage::Ping(ping) => {
                        let message = OwnedMessage::Pong(ping);
                        sender.send_message(&message).unwrap();
                    }
                    OwnedMessage::Text(ref text) => {
                        // println!("Received: {}", text);
                        let event: EditorEvent = serde_json::from_str(text).unwrap();
                        let mut bag = bag.lock().unwrap();
                        match event {
                            EditorEvent::State(code, cursor) => {
                                print!("\x1b[2J\x1b[1;1H");
                                run_code(&*bag, &*code, cursor);
                            }
                        }
                    }
                    _ => {
                        panic!("A weird message! {:?}", message);
                    }
                }
            }
        });
    }
}

fn main() {
    serve_editor();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let col = (0..1000).collect::<Vec<_>>();
        for i in 0..1000 {
            assert_eq!(col[gallop(&*col, 0, 1000, |&x| x < i)], i);
        }
        assert_eq!(gallop(&*col, 0, col.len(), |&x| x < -1), 0);
        assert_eq!(gallop(&*col, 0, col.len(), |&x| x < 1000), 1000);

        for i in 0..999 {
            assert_eq!(col[gallop(&*col, 0, col.len(), |&x| x <= i)], i + 1);
        }
        assert_eq!(gallop(&*col, 0, col.len(), |&x| x <= -1), 0);
        assert_eq!(gallop(&*col, 0, col.len(), |&x| x <= 999), 1000);
    }
}
