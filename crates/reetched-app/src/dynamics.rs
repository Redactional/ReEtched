//! Dynamic strokes: each brush stamp becomes a set of dabs whose placement,
//! size, angle and flow come from small expressions. Painter drives this with
//! Substance graphs; ours are expression graphs over these inputs:
//!
//! - `i`, `n`: this dab's index and the dab count
//! - `t`: distance along the stroke, in brush diameters; `s`: stamp index
//! - `p`: pen pressure
//! - `r1`..`r4`: random numbers in [0, 1), fixed per dab
//!
//! Functions: sin cos tan abs sqrt exp floor fract sign, pow min max step
//! mix(a, b, t) clamp(x, lo, hi) smoothstep(a, b, x) noise(x) rand(x); `pi`.

use anyhow::{Result, bail};

/// A compiled expression.
#[derive(Clone, Debug)]
pub enum Expr {
    Num(f32),
    Var(usize),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(&'static str, Vec<Expr>),
}

/// Names of the input variables, by `Expr::Var` index.
const VARS: [&str; 9] = ["i", "n", "t", "s", "p", "r1", "r2", "r3", "r4"];

const FUNCS: [(&str, usize); 16] = [
    ("sin", 1),
    ("cos", 1),
    ("tan", 1),
    ("abs", 1),
    ("sqrt", 1),
    ("exp", 1),
    ("floor", 1),
    ("fract", 1),
    ("sign", 1),
    ("noise", 1),
    ("rand", 1),
    ("pow", 2),
    ("min", 2),
    ("max", 2),
    ("step", 2),
    ("mix", 3),
];
const MORE_FUNCS: [(&str, usize); 2] = [("clamp", 3), ("smoothstep", 3)];

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.at < self.s.len() && self.s[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip();
        if self.s.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Expr> {
        let mut e = self.term()?;
        loop {
            if self.eat(b'+') {
                e = Expr::Bin('+', Box::new(e), Box::new(self.term()?));
            } else if self.eat(b'-') {
                e = Expr::Bin('-', Box::new(e), Box::new(self.term()?));
            } else {
                return Ok(e);
            }
        }
    }

    fn term(&mut self) -> Result<Expr> {
        let mut e = self.unary()?;
        loop {
            let op = b"*/%".iter().copied().find(|&c| self.eat(c));
            match op {
                Some(c) => e = Expr::Bin(c as char, Box::new(e), Box::new(self.unary()?)),
                None => return Ok(e),
            }
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat(b'-') {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        let base = self.atom()?;
        if self.eat(b'^') {
            return Ok(Expr::Bin('^', Box::new(base), Box::new(self.unary()?)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Expr> {
        self.skip();
        if self.eat(b'(') {
            let e = self.expr()?;
            if !self.eat(b')') {
                bail!("missing ')' at {}", self.at + 1);
            }
            return Ok(e);
        }
        let start = self.at;
        let c = self.s.get(self.at).copied().unwrap_or(0);
        if c.is_ascii_digit() || c == b'.' {
            while self
                .s
                .get(self.at)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
            {
                self.at += 1;
            }
            let text = std::str::from_utf8(&self.s[start..self.at])?;
            return Ok(Expr::Num(text.parse()?));
        }
        if c.is_ascii_alphabetic() {
            while self
                .s
                .get(self.at)
                .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                self.at += 1;
            }
            let name = std::str::from_utf8(&self.s[start..self.at])?;
            if name == "pi" {
                return Ok(Expr::Num(std::f32::consts::PI));
            }
            if let Some(v) = VARS.iter().position(|&v| v == name) {
                return Ok(Expr::Var(v));
            }
            let Some(&(f, arity)) = FUNCS.iter().chain(&MORE_FUNCS).find(|(f, _)| *f == name)
            else {
                bail!("unknown name '{name}'");
            };
            if !self.eat(b'(') {
                bail!("'{name}' needs arguments in parentheses");
            }
            let mut args = vec![self.expr()?];
            while self.eat(b',') {
                args.push(self.expr()?);
            }
            if !self.eat(b')') {
                bail!("missing ')' after the arguments of '{name}'");
            }
            if args.len() != arity {
                bail!("'{name}' takes {arity} argument(s), got {}", args.len());
            }
            return Ok(Expr::Call(f, args));
        }
        if self.at >= self.s.len() {
            bail!("the expression ends too early");
        }
        bail!("unexpected '{}' at {}", c as char, self.at + 1)
    }
}

pub fn parse(src: &str) -> Result<Expr> {
    let mut p = Parser {
        s: src.as_bytes(),
        at: 0,
    };
    let e = p.expr()?;
    p.skip();
    if p.at != p.s.len() {
        bail!("unexpected '{}' at {}", p.s[p.at] as char, p.at + 1);
    }
    Ok(e)
}

/// A hash in [0, 1).
fn hash(x: f32) -> f32 {
    let mut h = x.to_bits().wrapping_mul(0x9E37_79B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= h >> 13;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth 1D value noise in [0, 1].
fn noise(x: f32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f);
    hash(i) * (1.0 - u) + hash(i + 1.0) * u
}

impl Expr {
    pub fn eval(&self, vars: &[f32; 9]) -> f32 {
        match self {
            Expr::Num(v) => *v,
            Expr::Var(i) => vars[*i],
            Expr::Neg(e) => -e.eval(vars),
            Expr::Bin(op, a, b) => {
                let (a, b) = (a.eval(vars), b.eval(vars));
                match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    '%' => a.rem_euclid(b),
                    _ => a.powf(b),
                }
            }
            Expr::Call(f, args) => {
                let a: Vec<f32> = args.iter().map(|e| e.eval(vars)).collect();
                match *f {
                    "sin" => a[0].sin(),
                    "cos" => a[0].cos(),
                    "tan" => a[0].tan(),
                    "abs" => a[0].abs(),
                    "sqrt" => a[0].max(0.0).sqrt(),
                    "exp" => a[0].exp(),
                    "floor" => a[0].floor(),
                    "fract" => a[0].fract(),
                    "sign" => a[0].signum(),
                    "noise" => noise(a[0]),
                    "rand" => hash(a[0]),
                    "pow" => a[0].powf(a[1]),
                    "min" => a[0].min(a[1]),
                    "max" => a[0].max(a[1]),
                    "step" => (a[1] >= a[0]) as u8 as f32,
                    "mix" => a[0] + (a[1] - a[0]) * a[2],
                    "clamp" => a[0].clamp(a[1].min(a[2]), a[2].max(a[1])),
                    _ => {
                        // smoothstep(edge0, edge1, x)
                        let t = ((a[2] - a[0]) / (a[1] - a[0])).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    }
                }
            }
        }
    }
}

/// A dynamic stroke: expressions for the dab count and each dab's behavior.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Dynamics {
    pub name: String,
    /// Dabs per stamp.
    pub count: String,
    /// Offset in brush radii: along the stroke (x) and across it (y).
    pub x: String,
    pub y: String,
    /// Size multiplier.
    pub size: String,
    /// Added rotation, degrees.
    pub angle: String,
    /// Flow multiplier.
    pub flow: String,
}

impl Dynamics {
    pub fn presets() -> Vec<Dynamics> {
        let d = |name: &str, count: &str, x: &str, y: &str, size: &str, angle: &str, flow: &str| {
            Dynamics {
                name: name.into(),
                count: count.into(),
                x: x.into(),
                y: y.into(),
                size: size.into(),
                angle: angle.into(),
                flow: flow.into(),
            }
        };
        vec![
            d(
                "Scatter",
                "3",
                "(r1 - 0.5) * 3",
                "(r2 - 0.5) * 3",
                "0.35 + 0.65 * r3",
                "r4 * 360",
                "1",
            ),
            d(
                "Spray",
                "10",
                "(r1 - 0.5) * 2.5",
                "(r2 - 0.5) * 2.5",
                "0.06 + 0.12 * r3",
                "0",
                "0.8",
            ),
            d(
                "Wave",
                "1",
                "0",
                "sin(t * 4) * 1.5",
                "0.5 + 0.25 * cos(t * 4)",
                "0",
                "1",
            ),
            d(
                "Rope",
                "2",
                "0",
                "sin(t * 6 + i * pi) * 0.6",
                "0.45",
                "t * 90",
                "1",
            ),
            d(
                "Grass",
                "4",
                "(r1 - 0.5) * 2",
                "(r2 - 0.5) * 0.5",
                "0.25 + 0.35 * r3",
                "(r4 - 0.5) * 50",
                "1",
            ),
            d(
                "Pressure jitter",
                "1",
                "0",
                "(noise(t * 3) - 0.5) * 2 * p",
                "0.3 + 0.7 * p",
                "0",
                "0.4 + 0.6 * p",
            ),
        ]
    }

    pub fn compile(&self) -> Result<Compiled> {
        let field = |name: &str, src: &str| parse(src).map_err(|e| anyhow::anyhow!("{name}: {e}"));
        Ok(Compiled {
            count: field("count", &self.count)?,
            x: field("x", &self.x)?,
            y: field("y", &self.y)?,
            size: field("size", &self.size)?,
            angle: field("angle", &self.angle)?,
            flow: field("flow", &self.flow)?,
        })
    }
}

/// A dab's adjustments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DabMod {
    /// Offset in brush radii (x along the stroke).
    pub offset: [f32; 2],
    pub size: f32,
    pub angle_deg: f32,
    pub flow: f32,
}

#[derive(Clone, Debug)]
pub struct Compiled {
    count: Expr,
    x: Expr,
    y: Expr,
    size: Expr,
    angle: Expr,
    flow: Expr,
}

impl Compiled {
    /// The dabs for stamp `stamp` at stroke distance `t` (brush diameters).
    pub fn dabs(&self, stamp: u32, t: f32, pressure: f32) -> Vec<DabMod> {
        let mut vars = [0.0, 1.0, t, stamp as f32, pressure, 0.0, 0.0, 0.0, 0.0];
        let n = (self.count.eval(&vars).round().max(0.0) as u32).min(64);
        vars[1] = n as f32;
        (0..n)
            .map(|i| {
                vars[0] = i as f32;
                for k in 0..4 {
                    vars[5 + k] = hash(stamp as f32 * 131.0 + i as f32 * 17.0 + k as f32 * 3.7);
                }
                let ok = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
                DabMod {
                    offset: [ok(self.x.eval(&vars), 0.0), ok(self.y.eval(&vars), 0.0)],
                    size: ok(self.size.eval(&vars), 1.0).max(0.0),
                    angle_deg: ok(self.angle.eval(&vars), 0.0),
                    flow: ok(self.flow.eval(&vars), 1.0).clamp(0.0, 1.0),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> f32 {
        parse(src)
            .unwrap()
            .eval(&[2.0, 4.0, 0.5, 0.0, 1.0, 0.25, 0.0, 0.0, 0.0])
    }

    #[test]
    fn expressions_follow_precedence_and_functions() {
        assert_eq!(eval("1 + 2 * 3"), 7.0);
        assert_eq!(eval("(1 + 2) * 3"), 9.0);
        assert_eq!(eval("-2 ^ 2"), -4.0);
        assert_eq!(eval("i * n + r1"), 8.25);
        assert_eq!(eval("clamp(5, 0, 1) + mix(0, 10, t)"), 6.0);
        assert!((eval("sin(pi / 2)") - 1.0).abs() < 1e-6);
        assert_eq!(eval("7 % 4"), 3.0);
        assert!(parse("1 +").is_err());
        assert!(parse("foo(1)").is_err());
        assert!(parse("min(1)").is_err());
    }

    #[test]
    fn presets_compile_and_are_deterministic() {
        for d in Dynamics::presets() {
            let c = d.compile().unwrap_or_else(|e| panic!("{}: {e}", d.name));
            assert_eq!(c.dabs(3, 1.5, 0.8), c.dabs(3, 1.5, 0.8));
        }
        let scatter = Dynamics::presets()[0].compile().unwrap();
        let dabs = scatter.dabs(0, 0.0, 1.0);
        assert_eq!(dabs.len(), 3);
        assert!(
            dabs.iter()
                .all(|d| d.offset[0].abs() <= 1.5 && d.size <= 1.0)
        );
        assert_ne!(
            dabs[0].offset, dabs[1].offset,
            "each dab has its own random numbers"
        );
    }
}
