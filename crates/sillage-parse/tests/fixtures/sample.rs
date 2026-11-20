//! Crate docs.
use std::collections::HashMap;
use std::fmt::{self, Display};

/// A point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub enum Shape {
    Circle { center: Point, radius: f64 },
    Rect(Point, Point),
    Empty,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn dist(&self, other: &Point) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

pub trait Area {
    fn area(&self) -> f64;
    fn describe(&self) -> String {
        format!("area={:.2}", self.area())
    }
}

impl Area for Shape {
    fn area(&self) -> f64 {
        match self {
            Shape::Circle { radius, .. } => std::f64::consts::PI * radius * radius,
            Shape::Rect(a, b) => ((b.x - a.x) * (b.y - a.y)).abs(),
            Shape::Empty => 0.0,
        }
    }
}

pub fn histogram<'a>(words: &[&'a str]) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for &w in words.iter() {
        *counts.entry(w).or_insert(0) += 1;
    }
    let total: usize = counts.values().copied().sum();
    if let Some((word, n)) = counts.iter().max_by_key(|(_, n)| **n) {
        println!("{word}: {n} of {total}");
    }
    let raw = r#"raw "string""#;
    let bytes = b"bytes";
    let c = 'c';
    let mut i = 0u32;
    while i < 3 {
        i += 1;
    }
    'outer: loop {
        break 'outer;
    }
    let _ = (raw, bytes, c);
    counts
}

fn nested(v: Option<Vec<i32>>) -> i32 {
    fn helper(x: i32) -> i32 { x * 2 }
    let add = |a: i32, b| a + b;
    match v {
        Some(ref items) if !items.is_empty() => items.iter().fold(0, |acc, x| add(acc, helper(*x))),
        Some(_) | None => -1,
    }
}
