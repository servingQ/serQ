//! The text frontend: `.sq` source to IR. Nothing here is part of the
//! definition of a program (`crate::ir` is); it is one way to write one.

pub mod ast;
pub mod diagnostic;
pub mod fmt;
pub mod lexer;
pub mod link;
pub mod lint;
pub mod parser;
pub mod queue;
