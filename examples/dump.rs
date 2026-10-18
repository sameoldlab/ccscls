fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_css::LANGUAGE.into()).unwrap();
    let t = p.parse(&src, None).unwrap();
    println!("{}", t.root_node().to_sexp());
    let mut cur = t.walk();
    fn go(c: &mut tree_sitter::TreeCursor, src: &str, d: usize) {
        loop {
            let n = c.node();
            let txt: String = src[n.byte_range()]
                .chars()
                .take(30)
                .collect::<String>()
                .replace('\n', "\\n");
            println!(
                "{}{} [{}..{}] {:?}{}",
                "  ".repeat(d),
                n.kind(),
                n.start_byte(),
                n.end_byte(),
                txt,
                if n.is_error() { " ERR" } else { "" }
            );
            if c.goto_first_child() {
                go(c, src, d + 1);
                c.goto_parent();
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
    }
    go(&mut cur, &src, 0);
}
