use nom::{
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

/// Consumes a link description, stopping at the first `]]`.
///
/// Brackets and newlines are ordinary description content: only `]]`
/// terminates. This mirrors Emacs, whose `org-link-bracket-re` matches
/// the description with `\([^z-a]+?\)` — any character, newline
/// included, non-greedy up to the closing `]]`. A scan that stopped at
/// any `[` or `]` instead made every link with a bracketed description
/// fail to parse and vanish from the tree.
///
/// An empty description is accepted, so `[[path][]]` stays the link it
/// has always been. Emacs parses that form too (reporting no
/// description); rejecting it here would delete a link the rest of the
/// world can follow.
///
/// The token stays verbatim: decoding escapes is the caller's job,
/// which keeps the parser lossless and lets the source be reproduced
/// byte for byte.
fn link_description(input: Input) -> IResult<Input, Input, ()> {
    let s = input.as_str();
    let end = s
        .char_indices()
        .find(|&(idx, c)| c == ']' && s[idx..].starts_with("]]"))
        .map_or(s.len(), |(idx, _)| idx);
    let (rest, description) = input.take_split(end);
    Ok((rest, description))
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
            opt(tuple((r_bracket_token, l_bracket_token, link_description))),
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

/// Parse an Org plain link as the same LINK/LINK_PATH pair as a bracket link.
pub fn plain_link_node(input: Input) -> IResult<Input, GreenElement, ()> {
    let s = input.as_str();
    let prefix = [
        "https:", "http:", "ftp:", "mailto:", "file:", "doi:", "news:",
    ]
    .into_iter()
    .find(|prefix| s.starts_with(prefix))
    .ok_or(nom::Err::Error(()))?;
    let mut end = s
        .char_indices()
        .find(|&(i, c)| {
            i >= prefix.len() && (c.is_whitespace() || matches!(c, '<' | '>' | '[' | ']'))
        })
        .map_or(s.len(), |(i, _)| i);
    while end > prefix.len()
        && matches!(
            s.as_bytes()[end - 1],
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b')'
        )
    {
        end -= 1;
    }
    if end == prefix.len() {
        return Err(nom::Err::Error(()));
    }
    let (rest, path) = input.take_split(end);
    Ok((rest, node(LINK, [path.token(LINK_PATH)])))
}

#[test]
fn plain_links_respect_protocols_and_terminal_punctuation() {
    use crate::{ast::Link, tests::to_ast, ParseConfig};

    let parse = to_ast::<Link>(plain_link_node);
    for path in [
        "https://orgmode.org",
        "http://example.org/x",
        "ftp://host/a",
        "mailto:a@example.org",
        "file:/tmp/a.org",
        "doi:10.1000/xyz",
        "news:comp.emacs",
    ] {
        assert_eq!(parse(path).path().to_string(), path);
    }
    let config = ParseConfig::default();
    let (rest, link) = plain_link_node(("https://orgmode.org.),", &config).into()).unwrap();
    assert_eq!(link.to_string(), "https://orgmode.org");
    assert_eq!(rest.s, ".),");
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

#[test]
fn parse_description_according_to_org_mode() {
    use crate::{ast::Link, tests::to_ast};

    let to_link = to_ast::<Link>(link_node);
    let cases = [
        "[[file:A.org][plain]]",
        "[[file:A.org][has [bracket] pair]]",
        "[[file:A.org][ends with date [2026-08-30]]]",
        "[[file:A.org][open [ only]]",
        "[[file:A.org][close ] only]]",
        "[[file:A.org][nested [[inner]] thing]]",
        "[[file:A.org][a]b]]",
        "[[file:A.org][a]]b]]",
        "[[file:A.org][trailing bracket ]]]",
        r"[[file:A.org][desc with \] escape]]",
        "[[file:A.org]]",
        "[[file:A.org][multi\nline desc]]",
    ];
    let parsed = cases.map(|input| {
        let link = to_link(input);
        (
            input,
            link.path().to_string(),
            link.has_description().then(|| link.description_raw()),
        )
    });

    insta::assert_debug_snapshot!(parsed, @r###"
    [
        (
            "[[file:A.org][plain]]",
            "file:A.org",
            Some(
                "plain",
            ),
        ),
        (
            "[[file:A.org][has [bracket] pair]]",
            "file:A.org",
            Some(
                "has [bracket] pair",
            ),
        ),
        (
            "[[file:A.org][ends with date [2026-08-30]]]",
            "file:A.org",
            Some(
                "ends with date [2026-08-30",
            ),
        ),
        (
            "[[file:A.org][open [ only]]",
            "file:A.org",
            Some(
                "open [ only",
            ),
        ),
        (
            "[[file:A.org][close ] only]]",
            "file:A.org",
            Some(
                "close ] only",
            ),
        ),
        (
            "[[file:A.org][nested [[inner]] thing]]",
            "file:A.org",
            Some(
                "nested [[inner",
            ),
        ),
        (
            "[[file:A.org][a]b]]",
            "file:A.org",
            Some(
                "a]b",
            ),
        ),
        (
            "[[file:A.org][a]]b]]",
            "file:A.org",
            Some(
                "a",
            ),
        ),
        (
            "[[file:A.org][trailing bracket ]]]",
            "file:A.org",
            Some(
                "trailing bracket ",
            ),
        ),
        (
            "[[file:A.org][desc with \\] escape]]",
            "file:A.org",
            Some(
                "desc with \\] escape",
            ),
        ),
        (
            "[[file:A.org]]",
            "file:A.org",
            None,
        ),
        (
            "[[file:A.org][multi\nline desc]]",
            "file:A.org",
            Some(
                "multi\nline desc",
            ),
        ),
    ]
    "###);
}

#[test]
fn parse_objects_in_description() {
    use crate::{ast::Link, tests::to_ast};

    let link = to_ast::<Link>(link_node)("[[file:A.org][*bold* and /it/]]");

    insta::assert_debug_snapshot!(link.syntax, @r###"
    LINK@0..31
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..12 "file:A.org"
      R_BRACKET@12..13 "]"
      L_BRACKET@13..14 "["
      BOLD@14..20
        STAR@14..15 "*"
        TEXT@15..19 "bold"
        STAR@19..20 "*"
      TEXT@20..25 " and "
      ITALIC@25..29
        SLASH@25..26 "/"
        TEXT@26..28 "it"
        SLASH@28..29 "/"
      R_BRACKET2@29..31 "]]"
    "###);
    assert_eq!(link.description_raw(), "*bold* and /it/");
}

/// `[[path][]]` is a link, not a parse failure.
///
/// Emacs parses this form and simply reports no description, so
/// rejecting it would delete a link that every other org consumer
/// follows. Pinned because the obvious reading of "the description is
/// one-or-more characters" says the opposite, and acting on that
/// reading silently dropped the `[[id:empty][]]` node.
#[test]
fn parse_empty_description_as_a_link() {
    use crate::{ast::Link, tests::to_ast};

    let link = to_ast::<Link>(link_node)("[[file:A.org][]]");

    insta::assert_debug_snapshot!(link.syntax, @r###"
    LINK@0..16
      L_BRACKET2@0..2 "[["
      LINK_PATH@2..12 "file:A.org"
      R_BRACKET@12..13 "]"
      L_BRACKET@13..14 "["
      R_BRACKET2@14..16 "]]"
    "###);
    assert_eq!(link.path(), "file:A.org");
}

#[test]
fn parse_two_links_without_swallowing() {
    use rowan::ast::AstNode;

    use crate::{ast::Link, Org};

    let org = Org::parse("text [[file:A.org][desc]] then [[file:B.org][second]]");
    let links = org
        .document()
        .syntax()
        .descendants()
        .filter_map(Link::cast)
        .map(|link| (link.path().to_string(), link.description_raw()))
        .collect::<Vec<_>>();

    insta::assert_debug_snapshot!(links, @r###"
    [
        (
            "file:A.org",
            "desc",
        ),
        (
            "file:B.org",
            "second",
        ),
    ]
    "###);
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
