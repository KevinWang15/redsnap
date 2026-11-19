//! The calculator: a small expression parser over `f64`. Division is always
//! floating point (`1/2` is `0.5`, never `0`).
//!
//! Grammar: numbers (`1`, `1.5`, `.5`, `1e3`, `1_000`, `1,000`), `+ - * / %`,
//! `^` (right-associative), unary minus, parentheses, the constants `pi` and
//! `e`, and functions `sqrt abs round floor ceil sin cos tan asin acos atan
//! ln log log2 exp`. `x` and `×` multiply, `÷` divides.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Op(char),
    Open,
    Close,
    Ident(&'static str),
}

const FUNCS: &[&str] =
    &["sqrt", "abs", "round", "floor", "ceil", "sin", "cos", "tan", "asin", "acos", "atan", "ln", "log", "log2", "exp"];

fn lex(s: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                let mut text = String::new();
                while i < chars.len() {
                    let d = chars[i];
                    if d.is_ascii_digit() || d == '.' {
                        text.push(d);
                    } else if (d == '_' || d == ',')
                        && i + 3 < chars.len() + 1
                        && chars.get(i + 1..i + 4).is_some_and(|w| w.iter().all(char::is_ascii_digit))
                        && !chars.get(i + 4).is_some_and(char::is_ascii_digit)
                    {
                        // Thousands separator: exactly three digits follow.
                    } else if (d == 'e' || d == 'E')
                        && i > start
                        && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit() || *n == '-' || *n == '+')
                    {
                        text.push('e');
                        i += 1;
                        text.push(chars[i]);
                    } else {
                        break;
                    }
                    i += 1;
                }
                out.push(Tok::Num(text.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            'x' | '×' if !out.is_empty() && chars.get(i + 1).is_none_or(|n| !n.is_alphabetic()) => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
                let ident = match word.as_str() {
                    "pi" => "pi",
                    "e" => "e",
                    w => *FUNCS.iter().find(|f| **f == w)?,
                };
                out.push(Tok::Ident(ident));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<Tok> {
        self.toks.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.peek();
        self.pos += 1;
        t
    }

    /// Pratt loop. Binding powers (left, right): `+ -` (1, 2), `* / %` and
    /// implicit multiplication (3, 4), `^` (6, 5: right-associative). Unary
    /// minus binds at 5, so `-2^2` is `-(2^2)`.
    fn expr(&mut self, min_bp: u8) -> Option<f64> {
        let mut lhs = match self.next()? {
            Tok::Num(n) => n,
            Tok::Op('-') => -self.expr(5)?,
            Tok::Op('+') => self.expr(5)?,
            Tok::Open => {
                let v = self.expr(0)?;
                // A missing closing parenthesis at the end is forgiven: "(1+2".
                match self.next() {
                    Some(Tok::Close) | None => v,
                    _ => return None,
                }
            }
            Tok::Ident("pi") => std::f64::consts::PI,
            Tok::Ident("e") => std::f64::consts::E,
            Tok::Ident(f) => {
                let arg = match self.peek() {
                    // Just the parenthesized argument: `sqrt(16)^2` is `(sqrt 16)^2`.
                    Some(Tok::Open) => self.expr(7)?,
                    _ => return None,
                };
                apply(f, arg)?
            }
            _ => return None,
        };
        loop {
            let op = match self.peek() {
                Some(Tok::Op(op)) => op,
                // Implicit multiplication: "2(3+4)", "2pi".
                Some(Tok::Open) | Some(Tok::Ident(_)) => '*',
                _ => break,
            };
            let (l_bp, r_bp) = match op {
                '+' | '-' => (1, 2),
                '*' | '/' | '%' => (3, 4),
                '^' => (6, 5),
                _ => return None,
            };
            if l_bp < min_bp {
                break;
            }
            if matches!(self.peek(), Some(Tok::Op(_))) {
                self.pos += 1;
            }
            let rhs = self.expr(r_bp)?;
            lhs = match op {
                '+' => lhs + rhs,
                '-' => lhs - rhs,
                '*' => lhs * rhs,
                '/' => lhs / rhs,
                '%' => lhs % rhs,
                '^' => lhs.powf(rhs),
                _ => unreachable!(),
            };
        }
        Some(lhs)
    }
}

fn apply(f: &str, x: f64) -> Option<f64> {
    Some(match f {
        "sqrt" => x.sqrt(),
        "abs" => x.abs(),
        "round" => x.round(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "asin" => x.asin(),
        "acos" => x.acos(),
        "atan" => x.atan(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "log2" => x.log2(),
        "exp" => x.exp(),
        _ => return None,
    })
}

/// Evaluates `input` if it is an expression worth showing a result for: it
/// must contain an operator or a function, so a bare number (or a bare `e`)
/// isn't a calculation. Returns `None` for anything else, including results
/// that aren't finite (`1/0`).
pub fn evaluate(input: &str) -> Option<f64> {
    let input = input.trim().trim_end_matches('=').trim();
    let toks = lex(input)?;
    let has_work = toks.iter().any(|t| matches!(t, Tok::Op(_) | Tok::Open)) || toks.len() > 1;
    if !has_work || toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0 };
    let v = p.expr(0)?;
    (p.pos >= p.toks.len() && v.is_finite()).then_some(v)
}

/// A result formatted without float noise: 12 significant digits, trailing
/// zeros dropped (`0.1 + 0.2` → `0.3`), scientific notation for very large or
/// small values.
pub fn format(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let abs = v.abs();
    if !(1e-9..1e15).contains(&abs) {
        let s = format!("{v:.11e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        return format!("{mantissa}e{exp}");
    }
    let digits = 12 - (abs.log10().floor() as i32 + 1);
    let s = format!("{v:.prec$}", prec = digits.clamp(0, 15) as usize);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(s: &str) -> Option<String> {
        evaluate(s).map(format)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(calc("1/2").as_deref(), Some("0.5"));
        assert_eq!(calc("2*(3+4)").as_deref(), Some("14"));
        assert_eq!(calc("2 + 3 * 4").as_deref(), Some("14"));
        assert_eq!(calc("10 % 3").as_deref(), Some("1"));
        assert_eq!(calc("2^10").as_deref(), Some("1024"));
        assert_eq!(calc("2^3^2").as_deref(), Some("512"));
        assert_eq!(calc("-2^2").as_deref(), Some("-4"));
        assert_eq!(calc("0.1 + 0.2").as_deref(), Some("0.3"));
        assert_eq!(calc("1,000 * 3").as_deref(), Some("3000"));
        assert_eq!(calc("1e3 + 1").as_deref(), Some("1001"));
        assert_eq!(calc("3 x 4").as_deref(), Some("12"));
        assert_eq!(calc("8 ÷ 2 =").as_deref(), Some("4"));
        assert_eq!(calc("(1+2").as_deref(), Some("3"));
        assert_eq!(calc("2(3+4)").as_deref(), Some("14"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(calc("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(calc("2pi").as_deref(), Some("6.28318530718"));
        assert_eq!(calc("round(2.5)").as_deref(), Some("3"));
        assert_eq!(calc("log(1000)").as_deref(), Some("3"));
    }

    #[test]
    fn not_calculations() {
        for s in ["", "42", "e", "safari", "1/0", "2 +", "exit", "x", "sqrt", "1 2"] {
            assert_eq!(calc(s), None, "{s:?}");
        }
    }

    #[test]
    fn formatting() {
        assert_eq!(format(1e20), "1e20");
        assert_eq!(format(-0.000_000_000_12), "-1.2e-10");
        assert_eq!(format(1.0 / 3.0), "0.333333333333");
    }
}
