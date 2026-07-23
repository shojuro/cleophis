//! Safe on-device scientific expression evaluator for the calc() tool.
//! Pure, dependency-free, no `eval` of arbitrary code. See the workspace
//! spec docs/superpowers/specs/2026-07-23-calc-tool-design.md.

use std::fmt;

const MAX_DEPTH: usize = 32; // Task 3 enforces; Task 1 threads the counter.

#[derive(Debug, PartialEq)]
pub enum CalcError {
    TooLong,
    TooDeep,
    Syntax(String),
    ThousandsSep,
    DivByZero,
    Domain(String),
    UnknownName(String),
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::TooLong => write!(f, "expression is too long (max 500 characters)"),
            CalcError::TooDeep => write!(f, "expression is nested too deeply"),
            CalcError::Syntax(m) => write!(f, "couldn't parse that expression: {m}"),
            CalcError::ThousandsSep => write!(f, "write 1000, not 1,000"),
            CalcError::DivByZero => write!(f, "division by zero"),
            CalcError::Domain(m) => write!(f, "{m}"),
            CalcError::UnknownName(n) => write!(f, "unknown name '{n}'"),
        }
    }
}
impl std::error::Error for CalcError {}

/// Evaluate an expression to a number. Task 1 handles arithmetic; Tasks 2–3
/// add functions/constants/`deg`, the hygiene caps, and `evaluate_display`.
pub fn evaluate(expr: &str) -> Result<f64, CalcError> {
    let tokens = lex(expr)?;
    let mut p = Parser { tokens: &tokens, pos: 0, depth: 0 };
    let v = p.expr(0)?;                 // 0 = lowest binding power
    if p.pos != p.tokens.len() {
        return Err(CalcError::Syntax(format!("unexpected trailing input")));
    }
    Ok(v)
}

// --- lexer ---------------------------------------------------------------
#[derive(Debug, Clone, PartialEq)]
enum Tok { Num(f64), Ident(String), Plus, Minus, Star, Slash, Caret, LParen, RParen }

fn lex(src: &str) -> Result<Vec<Tok>, CalcError> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\n' | '\r' => { i += 1; }
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '*' => { out.push(Tok::Star); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '^' => { out.push(Tok::Caret); i += 1; }
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            '0'..='9' | '.' => {
                let start = i;
                while i < b.len() && matches!(b[i] as char, '0'..='9' | '.') { i += 1; }
                // a comma inside/adjacent to a number is a thousands separator
                let s = &src[start..i];
                let n: f64 = s.parse().map_err(|_| CalcError::Syntax(format!("bad number '{s}'")))?;
                out.push(Tok::Num(n));
            }
            ',' => return Err(CalcError::ThousandsSep),
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < b.len() && matches!(b[i] as char, 'a'..='z'|'A'..='Z'|'_'|'0'..='9') { i += 1; }
                out.push(Tok::Ident(src[start..i].to_string()));
            }
            other => return Err(CalcError::Syntax(format!("unexpected character '{other}'"))),
        }
    }
    Ok(out)
}

// --- parser/evaluator (Pratt) --------------------------------------------
struct Parser<'a> { tokens: &'a [Tok], pos: usize, depth: usize }

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> { self.tokens.get(self.pos) }
    fn next(&mut self) -> Option<&Tok> { let t = self.tokens.get(self.pos); if t.is_some() { self.pos += 1; } t }

    // min_bp: minimum binding power. + - = 1, * / = 2, ^ = 4 (right).
    fn expr(&mut self, min_bp: u8) -> Result<f64, CalcError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { return Err(CalcError::TooDeep); }
        let mut lhs = self.unary()?;
        if let Some(Tok::Ident(id)) = self.peek() {
            if id == "deg" { self.pos += 1; lhs = lhs * std::f64::consts::PI / 180.0; }
        }
        loop {
            let (bp, right_assoc, op) = match self.peek() {
                Some(Tok::Plus) => (1, false, '+'),
                Some(Tok::Minus) => (1, false, '-'),
                Some(Tok::Star) => (2, false, '*'),
                Some(Tok::Slash) => (2, false, '/'),
                Some(Tok::Caret) => (4, true, '^'),
                _ => break,
            };
            if bp < min_bp { break; }
            self.pos += 1;
            let next_min = if right_assoc { bp } else { bp + 1 };
            let rhs = self.expr(next_min)?;
            lhs = match op {
                '+' => lhs + rhs,
                '-' => lhs - rhs,
                '*' => lhs * rhs,
                '/' => { if rhs == 0.0 { return Err(CalcError::DivByZero); } lhs / rhs }
                '^' => lhs.powf(rhs),
                _ => unreachable!(),
            };
        }
        self.depth -= 1;
        Ok(lhs)
    }

    // Unary minus binds looser than ^ (so -3^2 = -(3^2)): parse the unary
    // operand at the power level (bp 3), which allows ^ but not + - * /.
    fn unary(&mut self) -> Result<f64, CalcError> {
        if matches!(self.peek(), Some(Tok::Minus)) {
            self.pos += 1;
            return Ok(-self.expr(3)?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<f64, CalcError> {
        match self.next().cloned() {
            Some(Tok::Num(n)) => Ok(n),
            Some(Tok::LParen) => {
                let v = self.expr(0)?;
                match self.next() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err(CalcError::Syntax("expected ')'".into())),
                }
            }
            Some(Tok::Ident(name)) => {
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    "e" => Ok(std::f64::consts::E),
                    "deg" => Err(CalcError::Syntax("'deg' must follow a number, e.g. 30 deg".into())),
                    _ => {
                        // function call: name '(' expr ')'
                        match self.next() {
                            Some(Tok::LParen) => {}
                            _ => return Err(CalcError::UnknownName(name)),
                        }
                        let arg = self.expr(0)?;
                        match self.next() {
                            Some(Tok::RParen) => {}
                            _ => return Err(CalcError::Syntax(format!("expected ')' after {name}("))),
                        }
                        apply_fn(&name, arg)
                    }
                }
            }
            Some(t) => Err(CalcError::Syntax(format!("unexpected token {t:?}"))),
            None => Err(CalcError::Syntax("unexpected end of expression".into())),
        }
    }
}

fn apply_fn(name: &str, x: f64) -> Result<f64, CalcError> {
    let v = match name {
        "sqrt" => { if x < 0.0 { return Err(CalcError::Domain("sqrt of a negative number".into())); } x.sqrt() }
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "log" => { if x <= 0.0 { return Err(CalcError::Domain("log of a non-positive number".into())); } x.log10() }
        "ln" => { if x <= 0.0 { return Err(CalcError::Domain("ln of a non-positive number".into())); } x.ln() }
        "abs" => x.abs(),
        "round" => round_half_away(x),
        _ => return Err(CalcError::UnknownName(name.to_string())),
    };
    Ok(v)
}

/// Round to nearest integer, half-AWAY-from-zero (Rust's f64::round already
/// does this; wrapped + tested so the contract is explicit and can't drift).
fn round_half_away(x: f64) -> f64 { x.round() }

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 { evaluate(s).unwrap() }

    #[test]
    fn arithmetic_precedence_and_associativity() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("10 - 4 - 3"), 3.0);       // left-assoc
        assert_eq!(ev("2 ^ 3 ^ 2"), 512.0);      // ^ right-assoc: 2^(3^2)
        assert_eq!(ev("-3 ^ 2"), -9.0);          // unary minus LOOSER than ^: -(3^2)
        assert_eq!(ev("(-3) ^ 2"), 9.0);
        assert_eq!(ev("-2 * -3"), 6.0);
        assert_eq!(ev("3.5 * 2"), 7.0);
        assert_eq!(ev("(3/4)*88"), 66.0);
    }

    #[test]
    fn syntax_and_divzero_errors() {
        assert!(matches!(evaluate("1 +"), Err(CalcError::Syntax(_))));
        assert!(matches!(evaluate("(1 + 2"), Err(CalcError::Syntax(_))));
        assert!(matches!(evaluate("1 2"), Err(CalcError::Syntax(_))));
        assert!(matches!(evaluate("1/0"), Err(CalcError::DivByZero)));
        assert!(matches!(evaluate(""), Err(CalcError::Syntax(_))));
    }

    fn approx(a: f64, b: f64) { assert!((a - b).abs() < 1e-9, "{a} vs {b}"); }

    #[test]
    fn functions_constants_and_degrees() {
        approx(ev("sqrt(144) + 5"), 17.0);
        approx(ev("abs(-7)"), 7.0);
        approx(ev("round(2.5)"), 3.0);      // half-away-from-zero
        approx(ev("round(-2.5)"), -3.0);
        approx(ev("log(1000)"), 3.0);       // base 10
        approx(ev("ln(e)"), 1.0);
        approx(ev("pi"), std::f64::consts::PI);
        approx(ev("sin(0)"), 0.0);          // radians
        approx(ev("sin(30 deg)"), 0.5);     // deg postfix unit
        approx(ev("cos(60 deg)"), 0.5);
        approx(ev("pi * 5 ^ 2"), std::f64::consts::PI * 25.0);
    }

    #[test]
    fn function_and_domain_errors() {
        assert!(matches!(evaluate("nope(2)"), Err(CalcError::UnknownName(_))));
        assert!(matches!(evaluate("bogus"), Err(CalcError::UnknownName(_))));
        assert!(matches!(evaluate("sqrt(-1)"), Err(CalcError::Domain(_))));
        assert!(matches!(evaluate("ln(0)"), Err(CalcError::Domain(_))));
        assert!(matches!(evaluate("sqrt()"), Err(CalcError::Syntax(_))));
    }
}
