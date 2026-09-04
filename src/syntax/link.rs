use nom::{
    bytes::complete::take_while,
    combinator::{map, opt},
    sequence::tuple,
    IResult, InputTake,
};

use super::{
    combinator::{
        l_bracket2_token, l_bracket_token, node, r_bracket2_token, r_bracket_token, GreenElement,
    },
    input::Input,
    object::link_description_object_nodes,
    SyntaxKind::*,
};

/// Consumes a link path, honoring org's backslash escapes.
///
/// Stops at the first `<`, `>`, newline, or `]` that is *not* escaped.
/// A backslash makes the following character part of the path, so
/// `\]` and `\[` do not delimit anything and `\\` is consumed as one
/// unit (its second backslash cannot escape a subsequent `]`).
///
/// This mirrors Emacs, which strips those backslashes with
/// `org-link-unescape` after parsing. Decoding is the caller's job:
/// the token stays verbatim so the source can be reproduced byte for
/// byte.
fn link_path(input: Input) -> IResult<Input, Input, ()> {
    let s = input.as_str();
    let mut end = s.len();
    let mut chars = s.char_indices();
    while let Some((idx, c)) = chars.next() {
        match c {
            // Consume the escaped character along with the backslash.
            // A trailing backslash escapes nothing and is kept as-is.
            '\\' => {
                chars.next();
            }
            '<' | '>' | '\n' | ']' => {
                end = idx;
                break;
            }
            _ => {}
        }
    }
    let (rest, path) = input.take_split(end);
    Ok((rest, path))
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(level = "debug", skip(input), fields(input = input.s))
)]
pub fn link_node(input: Input) -> IResult<Input, GreenElement, ()> {
    let mut parser = map(
        tuple((
            l_bracket2_token,
            link_path,
            opt(tuple((
                r_bracket_token,
                l_bracket_token,
                take_while(|c: char| c != '[' && c != ']'),
            ))),
            r_bracket2_token,
        )),
        |(l_bracket2, path, desc, r_bracket2)| {
            let mut children = vec![l_bracket2, path.token(LINK_PATH)];

            if let Some((r_bracket, l_bracket, desc)) = desc {
                children.extend([r_bracket, l_bracket]);
                children.extend(link_description_object_nodes(desc));
            }

            children.push(r_bracket2);

            node(LINK, children)
        },
    );
    crate::lossless_parser!(parser, input)
}

#[test]
fn parse() {
    use crate::{ast::Link, tests::to_ast, ParseConfig};

    let to_link = to_ast::<Link>(link_node);

    let link = to_link("[[#id]]");
    insta::assert_debug_snapshot!(
        link.syntax,
        @r###"
    LINK@0..7
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..5 "#id"
      R_BRACKET2@5..7 "]]"
    "###
    );

    let link = to_link("[[#id][desc]]");
    insta::assert_debug_snapshot!(
        link.syntax,
        @r###"
    LINK@0..13
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..5 "#id"
      R_BRACKET@5..6 "]"
      L_BRACKET@6..7 "["
      TEXT@7..11 "desc"
      R_BRACKET2@11..13 "]]"
    "###
    );

    let link = to_link("[[file:/home/dominik/images/jupiter.jpg]]");
    insta::assert_debug_snapshot!(
        link.syntax,
        @r###"
    LINK@0..41
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..39 "file:/home/dominik/im ..."
      R_BRACKET2@39..41 "]]"
    "###
    );

    let link = to_link("[[https://orgmode.org][*bold* description]]");
    insta::assert_debug_snapshot!(
        link.syntax,
        @r###"
    LINK@0..43
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..21 "https://orgmode.org"
      R_BRACKET@21..22 "]"
      L_BRACKET@22..23 "["
      BOLD@23..29
        STAR@23..24 "*"
        TEXT@24..28 "bold"
        STAR@28..29 "*"
      TEXT@29..41 " description"
      R_BRACKET2@41..43 "]]"
    "###
    );

    let config = &ParseConfig::default();

    assert!(link_node(("[[#id][desc]", config).into()).is_err());
}

/// A `]` written as `\]` inside a link path is escaped content, not the
/// end of the path. Org itself emits this form: `org-store-link` on a
/// headline titled `[2026-07-15] Note` produces
/// `[[file:a.org::*\[2026-07-15\] Note][…]]`, and `org-link-unescape`
/// turns `\[`/`\]` back into `[`/`]` when the link is followed.
#[test]
fn parse_escaped_brackets_in_path() {
    use crate::{ast::Link, tests::to_ast};

    let to_link = to_ast::<Link>(link_node);

    // Escaped closing bracket must not terminate the path.
    let link = to_link(r"[[file:a.org::*A\]B][desc]]");
    assert_eq!(link.path(), r"file:a.org::*A\]B");

    // Escaped opening bracket, already accepted before this fix.
    let link = to_link(r"[[file:a.org::*A\[B][desc]]");
    assert_eq!(link.path(), r"file:a.org::*A\[B");

    // Both, as produced by `org-store-link` for a bracketed headline.
    let link = to_link(r"[[file:a.org::*\[2026-07-15\] Note][desc]]");
    assert_eq!(link.path(), r"file:a.org::*\[2026-07-15\] Note");

    // Without a description.
    let link = to_link(r"[[file:a.org::*\[X\]]]");
    assert_eq!(link.path(), r"file:a.org::*\[X\]");

    // An escaped backslash is itself atomic: the `\\` pair is consumed
    // whole, so the following `]` still closes the path.
    let link = to_link(r"[[file:a\\][desc]]");
    assert_eq!(link.path(), r"file:a\\");

    // Unescaped `]` keeps terminating the path (no behavior change).
    let link = to_link("[[file:a.org][desc]]");
    assert_eq!(link.path(), "file:a.org");
}
