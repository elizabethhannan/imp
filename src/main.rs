#![feature(test)]
#![feature(iter_arith)]
#![feature(step_by)]
#![allow(dead_code)]
#![feature(slice_patterns)]
#![feature(plugin)]
#![plugin(peg_syntax_ext)]

extern crate rand;
extern crate test;
extern crate time;
extern crate regex;

use std::io::prelude::*;
use std::fs::File;
use std::rc::Rc;

macro_rules! time {
    ($name:expr, $expr:expr) => {{
        let start = ::time::precise_time_s();
        let result = $expr;
        let end = ::time::precise_time_s();
        println!("{} took {}s", $name, end - start);
        result
    }};
}

macro_rules! assert_set_eq {
    ($left:expr, $right:expr) => {{
        let mut left = $left.collect::<Vec<_>>();
        left.sort_by(|a,b| a.partial_cmp(b).unwrap()); left.dedup();
        let mut right = $right;
        right.sort_by(|a,b| a.partial_cmp(b).unwrap()); right.dedup();
        assert_eq!(left, right);
    }}
}

mod primitive;
mod runtime;
mod bootstrap;

fn run(filenames: &[String]) -> () {
    print!("Loading...");
    let bootstrap_program = bootstrap::load(filenames);
    print!("compiling...");
    let mut runtime_program = bootstrap::compile(&bootstrap_program);
    print!("injecting...");
    let mut text = String::new();
    File::open("data/imp.imp").unwrap().read_to_string(&mut text).unwrap();
    {
        let runtime::Program{ref mut states, ref mut strings, ..} = runtime_program;
        let mut data = vec![];
        runtime::push_string(&mut data, strings, "program".to_owned());
        data.push(runtime::from_number(0.0));
        data.push(runtime::from_number(text.len() as f64));
        runtime::push_string(&mut data, strings, "program".to_owned());
        data.push(runtime::from_number(0.0));
        data.push(runtime::from_number(text.len() as f64));
        runtime::push_string(&mut data, strings, text);
        let mut chunk = (*states[0]).clone();
        chunk.data = data;
        states[0] = Rc::new(chunk);
    }
    print!("running...");
    runtime_program.run();
    runtime_program.print();
}

fn watch(filenames: &[String]) -> () {
    loop {
        run(filenames);
    }
}

fn main() {
    use std::env;
    use regex::Regex;
    use std::io::prelude::*;
    use std::fs::File;
    let mut text = String::new();
    File::open("data/imp.imp").unwrap().read_to_string(&mut text).unwrap();
    for (a,b) in Regex::new(r"\n\+((\n[^\+-=].*)+)").unwrap().find_iter(&text) {
        println!("{:?}", &text[a..b]);
        println!("");
    }
    let args = env::args().collect::<Vec<String>>();
    match &*args[1] {
        "--run" => run(&args[2..]),
        "--watch" => watch(&args[2..]),
        _ => panic!("Didn't understand this command:\n {:?}", &args[1..]),
    }
}