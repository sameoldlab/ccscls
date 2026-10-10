// Usage: cargo run --example index_file -- <file.css>   (prints class count and time)
fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let n = css_class_lsp::css::CssIndexer::new().index(&src).len();
    println!("{n} classes, {} bytes, {:?}", src.len(), t.elapsed());
}
