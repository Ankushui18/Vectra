//! Chumsky parser (Task 2.1 pipeline stage 2).
//!
//! `logos` tokens → [`Stream`] → `recursive` + `pratt` grammar → [`RawNode`]
//! → [`resolve`] → typed [`ExprNode`].
//!
//! Name resolution and arity checking live in [`resolve`], not in the
//! grammar: the grammar accepts any `ident(args)` shape, and the resolve pass
//! turns names into [`FunctionName`] with precise [`ParseError`]s (including
//! spans). Structure errors and type errors therefore stay cleanly separated.

use crate::ast::{BinOp, ExprNode, FunctionName};
use crate::error::ParseError;
use crate::lexer::{lex, Token};
use chumsky::input::{Stream, ValueInput};
use chumsky::pratt::{infix, left, prefix};
use chumsky::prelude::*;

/// Pre-resolution AST. Identical to [`ExprNode`] except calls still carry
/// unresolved `String` names plus their span for resolve-time diagnostics.
#[derive(Debug, Clone)]
enum RawNode {
    Literal(f64),
    Variable(String),
    Neg(Box<RawNode>),
    Binary {
        op: BinOp,
        left: Box<RawNode>,
        right: Box<RawNode>,
    },
    Call {
        name: String,
        args: Vec<RawNode>,
        span: SimpleSpan,
    },
}

fn parser<'a, I>() -> impl Parser<'a, I, RawNode, extra::Err<Rich<'a, Token>>>
where
    I: ValueInput<'a, Token = Token, Span = SimpleSpan>,
{
    recursive(|expr| {
        let number = select! { Token::Number(n) => RawNode::Literal(n) };
        let variable = select! { Token::Variable(name) => RawNode::Variable(name) };

        let args = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>();
        let call = select! { Token::Ident(name) => name }
            .then(args.delimited_by(just(Token::LParen), just(Token::RParen)))
            .map_with(|(name, args), extra| RawNode::Call {
                name,
                args,
                span: extra.span(),
            });

        let atom = number
            .or(variable)
            .or(call)
            .or(expr.delimited_by(just(Token::LParen), just(Token::RParen)));

        atom.pratt((
            prefix(3, just(Token::Minus), |_, rhs, _| {
                RawNode::Neg(Box::new(rhs))
            }),
            infix(
                left(2),
                just(Token::Star)
                    .to(BinOp::Mul)
                    .or(just(Token::Slash).to(BinOp::Div)),
                |l, op, r, _| RawNode::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                },
            ),
            infix(
                left(1),
                just(Token::Plus)
                    .to(BinOp::Add)
                    .or(just(Token::Minus).to(BinOp::Sub)),
                |l, op, r, _| RawNode::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                },
            ),
        ))
    })
}

/// Resolve names + arity into the typed AST.
fn resolve(raw: &RawNode) -> Result<ExprNode, ParseError> {
    match raw {
        RawNode::Literal(n) => Ok(ExprNode::Literal(*n)),
        RawNode::Variable(name) => Ok(ExprNode::Variable(name.clone())),
        RawNode::Neg(inner) => Ok(ExprNode::Neg(Box::new(resolve(inner)?))),
        RawNode::Binary { op, left, right } => Ok(ExprNode::Binary {
            op: *op,
            left: Box::new(resolve(left)?),
            right: Box::new(resolve(right)?),
        }),
        RawNode::Call { name, args, span } => {
            let func = match name.as_str() {
                "sin" => FunctionName::Sin,
                "cos" => FunctionName::Cos,
                "abs" => FunctionName::Abs,
                "clamp" => FunctionName::Clamp,
                _ => {
                    return Err(ParseError::UnknownFunction {
                        name: name.clone(),
                        position: span.start,
                    });
                }
            };
            if args.len() != func.arity() {
                return Err(ParseError::ArityMismatch {
                    name: name.clone(),
                    expected: func.arity(),
                    got: args.len(),
                    position: span.start,
                });
            }
            let mut resolved = Vec::with_capacity(args.len());
            for arg in args {
                resolved.push(resolve(arg)?);
            }
            Ok(ExprNode::Call {
                name: func,
                args: resolved,
            })
        }
    }
}

fn expected_list(err: &Rich<'_, Token>) -> String {
    let mut items: Vec<String> = err.expected().map(|p| format!("{p:?}")).collect();
    items.sort();
    items.dedup();
    if items.is_empty() {
        "expression".to_string()
    } else {
        items.join(", ")
    }
}

fn map_error(mut errs: Vec<Rich<'_, Token>>) -> ParseError {
    debug_assert!(!errs.is_empty());
    let err = errs.remove(0);
    let position = err.span().start;
    let expected = expected_list(&err);
    match err.found().cloned() {
        Some(token) => ParseError::UnexpectedToken {
            position,
            found: token.to_string(),
            expected,
        },
        None => ParseError::UnexpectedEnd { position, expected },
    }
}

/// Parse a full expression source. Trailing garbage fails (`end()` is
/// enforced) — `1 2` is an error, not a silent truncation.
pub fn parse(src: &str) -> Result<ExprNode, ParseError> {
    let tokens = lex(src)?;
    let eoi: SimpleSpan = (src.len()..src.len()).into();
    let stream = Stream::from_iter(
        tokens
            .into_iter()
            .map(|(token, span)| (token, SimpleSpan::from(span))),
    )
    .map(eoi, |(t, s): (_, _)| (t, s));
    let raw = parser()
        .then_ignore(end())
        .parse(stream)
        .into_result()
        .map_err(map_error)?;
    resolve(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(n: f64) -> ExprNode {
        ExprNode::Literal(n)
    }

    fn var(name: &str) -> ExprNode {
        ExprNode::Variable(name.to_string())
    }

    fn bin(op: BinOp, l: ExprNode, r: ExprNode) -> ExprNode {
        ExprNode::Binary {
            op,
            left: Box::new(l),
            right: Box::new(r),
        }
    }

    #[test]
    fn precedence_mul_binds_tighter() {
        // 1 + 2 * 3 == 1 + (2 * 3)
        assert_eq!(
            parse("1 + 2 * 3").unwrap(),
            bin(BinOp::Add, lit(1.0), bin(BinOp::Mul, lit(2.0), lit(3.0)))
        );
    }

    #[test]
    fn left_associativity() {
        // 10 - 3 - 2 == (10 - 3) - 2
        assert_eq!(
            parse("10 - 3 - 2").unwrap(),
            bin(BinOp::Sub, bin(BinOp::Sub, lit(10.0), lit(3.0)), lit(2.0))
        );
    }

    #[test]
    fn parens_override() {
        assert_eq!(
            parse("(1 + 2) * 3").unwrap(),
            bin(BinOp::Mul, bin(BinOp::Add, lit(1.0), lit(2.0)), lit(3.0))
        );
    }

    #[test]
    fn unary_minus_and_nesting() {
        assert_eq!(parse("-$a").unwrap(), ExprNode::Neg(Box::new(var("a"))));
        assert_eq!(
            parse("--5").unwrap(),
            ExprNode::Neg(Box::new(ExprNode::Neg(Box::new(lit(5.0)))))
        );
        // -2 * 3 == (-2) * 3 (prefix binds tighter than infix)
        assert_eq!(
            parse("-2 * 3").unwrap(),
            bin(BinOp::Mul, ExprNode::Neg(Box::new(lit(2.0))), lit(3.0))
        );
    }

    #[test]
    fn calls_and_nesting() {
        assert_eq!(
            parse("sin($time) * 50").unwrap(),
            bin(
                BinOp::Mul,
                ExprNode::Call {
                    name: FunctionName::Sin,
                    args: vec![var("time")]
                },
                lit(50.0)
            )
        );
        assert_eq!(
            parse("clamp($width, 0, 100)").unwrap(),
            ExprNode::Call {
                name: FunctionName::Clamp,
                args: vec![var("width"), lit(0.0), lit(100.0)]
            }
        );
    }

    #[test]
    fn unknown_function_is_typed() {
        assert_eq!(
            parse("foo(1)").unwrap_err(),
            ParseError::UnknownFunction {
                name: "foo".to_string(),
                position: 0
            }
        );
    }

    #[test]
    fn arity_mismatch_is_typed() {
        assert_eq!(
            parse("sin(1, 2)").unwrap_err(),
            ParseError::ArityMismatch {
                name: "sin".to_string(),
                expected: 1,
                got: 2,
                position: 0
            }
        );
        assert!(matches!(
            parse("clamp(1, 2)").unwrap_err(),
            ParseError::ArityMismatch {
                expected: 3,
                got: 2,
                ..
            }
        ));
    }

    #[test]
    fn structural_errors_are_typed() {
        assert!(matches!(
            parse("$a + ").unwrap_err(),
            ParseError::UnexpectedEnd { .. }
        ));
        assert!(matches!(
            parse("(1").unwrap_err(),
            ParseError::UnexpectedEnd { .. }
        ));
        assert!(matches!(
            parse("1 2").unwrap_err(),
            ParseError::UnexpectedToken { .. }
        ));
        assert!(matches!(
            parse("* 3").unwrap_err(),
            ParseError::UnexpectedToken { .. }
        ));
    }
}
