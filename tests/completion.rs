//! Completion, one annotated workspace per test (see `common::parse_fixture`).

mod common;
use common::completion_labels;

#[test]
fn a_component_is_offered_the_classes_of_its_own_style_block() {
    let labels = completion_labels(
        "//- /A.svelte\n<style>\n.mine { }\n</style>\n<p class=\"$0\">\n",
    );
    assert!(labels.contains(&"mine".to_owned()), "{labels:?}");
}

#[test]
fn another_component_is_not_offered_them() {
    let labels = completion_labels(
        "//- /a.css\n.btn { }\n//- /A.svelte\n<style>\n.mine { }\n</style>\n//- /B.svelte\n<p class=\"$0\">\n",
    );
    // The control: completion works here, it just does not include `.mine`.
    assert!(labels.contains(&"btn".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"mine".to_owned()), "{labels:?}");
}

#[test]
fn a_stylesheet_class_is_offered_in_a_page() {
    let labels = completion_labels(
        "//- /a.css\n.btn { }\n//- /index.html\n<p class=\"$0\">\n",
    );
    assert!(labels.contains(&"btn".to_owned()), "{labels:?}");
}
